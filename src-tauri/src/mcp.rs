//! `lectern.exe mcp`: an agent's MCP server, over this process's stdin and stdout, relaying each
//! tool call to the running Lectern over its pipe (`agent.rs`). It runs before, and instead of,
//! everything else Lectern does at start: no window, no boot mutex, no single-instance hand-over.
//!
//! - **Connecting:** at the first tool call, not at start, so an agent that starts with the
//!   computer doesn't start Lectern too. When no Lectern is running, it starts one, without
//!   handing it any of this process's handles or its console, and outside this process's job:
//!   otherwise the MCP client would wait on the stdio Lectern held, and stopping the server would
//!   end Lectern.
//!   A connection that fails is made again at the next call.
//! - **Calls** each run on a thread of their own, so a slow one doesn't hold up the others, nor the
//!   protocol's own messages.
//! - **The pipe's owner:** before anything is written, the pipe's server must run as this user.
//!   Another logged-on user could make the pipe first, as its name comes from a SID, which isn't
//!   secret. Such a pipe is let go and every call refused, each with a single try.
//! - **It ends** when stdin closes.

use std::collections::HashMap;
use std::io::{self, ErrorKind, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::agent::lines::{Line, LineReader};
use lectern_core::agent::mcp::{code, Handled, Mcp};
use lectern_core::agent::request::ClientInfo;
use lectern_core::agent::{
    pipe_name, same_user, AgentRequest, Answer, PipeReply, PipeRequest, ANSWER_WITHIN,
    FOREIGN_PIPE, MAX_LINE,
};
use serde_json::{json, Value};
use windows::core::{HSTRING, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED};
use windows::Win32::System::Threading::{
    CreateProcessW, CREATE_BREAKAWAY_FROM_JOB, DETACHED_PROCESS, PROCESS_CREATION_FLAGS,
    PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::logging;
use crate::pipe::{self, Pipe};

/// How long a started Lectern has to open its pipe.
const LAUNCH_WAIT: Duration = Duration::from_secs(10);
/// How often the pipe is tried while Lectern starts.
const LAUNCH_POLL: Duration = Duration::from_millis(100);
/// How much longer than Lectern's own limit an answer is waited for.
const GRACE: Duration = Duration::from_secs(5);

/// What relays a request to Lectern, as the client named itself.
pub type Relay =
    Arc<dyn Fn(AgentRequest, Option<ClientInfo>) -> Result<Value, String> + Send + Sync>;

/// Runs the MCP server for the app `identifier` until stdin closes; the process's exit code.
pub fn run(identifier: &str) -> i32 {
    logging::init_stderr_logging();
    let sid = match pipe::user_sid() {
        Ok(sid) => sid,
        Err(e) => {
            log::error!("can't name Lectern's pipe without the user's SID: {e}");
            return 1;
        }
    };
    let lectern = Lectern::new(pipe_name(identifier, &sid), sid, Box::new(launch_lectern));
    let relay: Relay = Arc::new(move |request, client| lectern.call(request, client));
    match serve(io::stdin().lock(), io::stdout(), relay) {
        Ok(()) => 0,
        Err(e) => {
            log::error!("stdin failed: {e}");
            1
        }
    }
}

/// Answers the MCP messages read from `input` on `output`, one per line, relaying tool calls with
/// `relay`, until `input` ends.
pub fn serve(
    input: impl Read,
    output: impl Write + Send + 'static,
    relay: Relay,
) -> io::Result<()> {
    let mcp = Arc::new(Mcp::new(lectern_core::version()));
    let output: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(output));
    let mut lines = LineReader::new(input, MAX_LINE);
    loop {
        let line = match lines.next_line()? {
            Line::End => return Ok(()),
            Line::TooLong => {
                let refused = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": code::INVALID_REQUEST,
                        "message": format!("Invalid Request: longer than {MAX_LINE} bytes"),
                    },
                });
                write_line(&output, &refused.to_string());
                continue;
            }
            Line::Text(line) => line,
        };
        match mcp.handle(&line) {
            Handled::Nothing => {}
            Handled::Reply(reply) => write_line(&output, &reply),
            Handled::Call(call) => {
                let (mcp, output, relay) =
                    (Arc::clone(&mcp), Arc::clone(&output), Arc::clone(&relay));
                let calling = thread::Builder::new()
                    .name("lectern-mcp-call".to_owned())
                    .spawn(move || {
                        let answer = relay(call.request.clone(), mcp.client());
                        write_line(&output, &mcp.finish(call, answer));
                    });
                if let Err(e) = calling {
                    log::error!("couldn't relay a tool call: {e}");
                }
            }
        }
    }
}

/// Writes `line` and a `\n`, whole, and flushes it.
fn write_line(output: &Mutex<dyn Write + Send>, line: &str) {
    let mut output = output.lock().unwrap_or_else(PoisonError::into_inner);
    let written = output
        .write_all(line.as_bytes())
        .and_then(|()| output.write_all(b"\n"))
        .and_then(|()| output.flush());
    if let Err(e) = written {
        log::error!("couldn't write to stdout: {e}");
    }
}

/// The running Lectern, reached over its pipe `name`, served as the user `user` (a SID), and
/// started with `launch` when it isn't running.
pub struct Lectern {
    name: String,
    user: String,
    launch: Box<dyn Fn() -> io::Result<()> + Send + Sync>,
    connection: Mutex<Option<Arc<Connection>>>,
}

impl Lectern {
    pub fn new(
        name: String,
        user: String,
        launch: Box<dyn Fn() -> io::Result<()> + Send + Sync>,
    ) -> Self {
        Self {
            name,
            user,
            launch,
            connection: Mutex::new(None),
        }
    }

    /// Lectern's answer to `request`, from `client`; the error is a sentence for the agent.
    pub fn call(&self, request: AgentRequest, client: Option<ClientInfo>) -> Result<Value, String> {
        let connection = self.connection(client)?;
        match connection.ask(request, ANSWER_WITHIN + GRACE) {
            Ok(answer) => answer,
            Err(Failure::Lost) => Err("Lost the connection to Lectern. Try again.".to_owned()),
            Err(Failure::TimedOut) => Err("Lectern didn't answer in time.".to_owned()),
        }
    }

    /// The connection, made when there is none or the last one is lost, starting Lectern when it
    /// isn't running. A new connection says hello for `client`.
    fn connection(&self, client: Option<ClientInfo>) -> Result<Arc<Connection>, String> {
        let mut slot = self
            .connection
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(connection) = slot.as_ref().filter(|c| c.alive()) {
            return Ok(Arc::clone(connection));
        }
        let connection = Connection::start(self.open()?)
            .map_err(|e| format!("Couldn't talk to Lectern: {e}"))?;
        if let Some(client) = client {
            connection.say(AgentRequest::Hello { client });
        }
        *slot = Some(Arc::clone(&connection));
        Ok(connection)
    }

    /// The pipe, once Lectern has it open: started first when it isn't running. A pipe served
    /// as another user is let go at once, with nothing written to it.
    fn open(&self) -> Result<Pipe, String> {
        match pipe::connect(&self.name) {
            Ok(pipe) => return self.ours(pipe),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(format!("Couldn't reach Lectern: {e}")),
        }
        (self.launch)().map_err(|e| format!("Couldn't start Lectern: {e}"))?;
        let deadline = Instant::now() + LAUNCH_WAIT;
        loop {
            thread::sleep(LAUNCH_POLL);
            match pipe::connect(&self.name) {
                Ok(pipe) => return self.ours(pipe),
                Err(e) if e.kind() == ErrorKind::NotFound && Instant::now() < deadline => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    return Err("Lectern didn't start within 10 seconds.".to_owned())
                }
                Err(e) => return Err(format!("Couldn't reach Lectern: {e}")),
            }
        }
    }

    /// `pipe` when its server runs as this user; otherwise it is closed, and the refusal logged.
    /// Another user can work out the pipe's name and make it first.
    fn ours(&self, pipe: Pipe) -> Result<Pipe, String> {
        match pipe.server_user() {
            Ok(server) if same_user(&self.user, &server) => Ok(pipe),
            Ok(server) => {
                log::error!("Lectern's pipe is served as {server}, not this user; refusing it");
                Err(FOREIGN_PIPE.to_owned())
            }
            Err(e) => {
                log::error!("can't tell which user serves Lectern's pipe: {e}; refusing it");
                Err(UNKNOWN_OWNER.to_owned())
            }
        }
    }
}

/// The answer to a tool call when the user serving Lectern's pipe can't be told.
const UNKNOWN_OWNER: &str = "Lectern's pipe can't be checked for its owner; refusing to connect.";

/// Why a request got no answer.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// The connection failed or closed.
    Lost,
    TimedOut,
}

/// A connection to Lectern: requests written whole, one at a time, and answers read on a thread
/// of its own and handed to whoever waits on their id.
struct Connection {
    pipe: Arc<Pipe>,
    writing: Mutex<()>,
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, Sender<Answer>>>,
    alive: AtomicBool,
}

impl Connection {
    fn start(pipe: Pipe) -> io::Result<Arc<Self>> {
        let connection = Arc::new(Self {
            pipe: Arc::new(pipe),
            writing: Mutex::new(()),
            next: AtomicU64::new(0),
            waiting: Mutex::new(HashMap::new()),
            alive: AtomicBool::new(true),
        });
        let reading = Arc::clone(&connection);
        thread::Builder::new()
            .name("lectern-mcp-pipe".to_owned())
            .spawn(move || reading.read_answers())?;
        Ok(connection)
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Hands each answer to whoever waits on its id, until the pipe ends; then everyone still
    /// waiting learns the connection is lost.
    fn read_answers(&self) {
        let mut lines = LineReader::new(&*self.pipe, MAX_LINE);
        while let Ok(Line::Text(line)) = lines.next_line() {
            let Ok(PipeReply { id, answer }) = serde_json::from_str(&line) else {
                log::warn!("Lectern sent a line that isn't an answer");
                break;
            };
            let waiter = lock(&self.waiting).remove(&id);
            if let Some(waiter) = waiter {
                let _ = waiter.send(answer);
            }
        }
        // Under the lock `ask` checks it under, so no one waits on a connection gone.
        let mut waiting = lock(&self.waiting);
        self.alive.store(false, Ordering::SeqCst);
        waiting.clear();
    }

    /// Sends `request` and waits up to `wait` for its answer; lost at once on a connection whose
    /// answers have stopped.
    fn ask(&self, request: AgentRequest, wait: Duration) -> Result<Result<Value, String>, Failure> {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (answered, answer) = mpsc::channel();
        {
            let mut waiting = lock(&self.waiting);
            if !self.alive() {
                return Err(Failure::Lost);
            }
            waiting.insert(id, answered);
        }
        if !self.send(id, request) {
            lock(&self.waiting).remove(&id);
            return Err(Failure::Lost);
        }
        match answer.recv_timeout(wait) {
            Ok(answer) => Ok(answer.into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&self.waiting).remove(&id);
                Err(Failure::TimedOut)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(Failure::Lost),
        }
    }

    /// Sends `request` without waiting for its answer.
    fn say(&self, request: AgentRequest) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        self.send(id, request);
    }

    /// Writes the request `id` as one line; false, and the connection is lost, when that fails.
    fn send(&self, id: u64, request: AgentRequest) -> bool {
        let mut line = match serde_json::to_string(&PipeRequest { id, request }) {
            Ok(line) => line,
            Err(e) => {
                log::error!("couldn't write a request: {e}");
                return false;
            }
        };
        line.push('\n');
        let _writing = lock(&self.writing);
        match self.pipe.write_all(line.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                log::warn!("lost the connection to Lectern: {e}");
                self.alive.store(false, Ordering::SeqCst);
                self.pipe.cancel();
                false
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts this Lectern, as if from its shortcut.
fn launch_lectern() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    launch_detached(&exe, &[]).map(|_| ())
}

/// Starts `exe` with `args`, handing it none of this process's handles and, when the job this
/// process is in allows it, outside that job; in the folder `exe` is in, so it holds no other
/// folder open. A debug build is a console program, and is kept off this process's console too:
/// sharing it kept the MCP client's stdio open after this process ended. Its process id.
pub fn launch_detached(exe: &Path, args: &[&str]) -> io::Result<u32> {
    let mut command = quote(&exe.to_string_lossy());
    for arg in args {
        command.push(' ');
        command.push_str(&quote(arg));
    }
    let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    let folder = HSTRING::from(exe.parent().unwrap_or(exe).as_os_str());
    let exe = HSTRING::from(exe.as_os_str());
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        ..STARTUPINFOW::default()
    };
    let mut started = PROCESS_INFORMATION::default();
    let mut create = |flags: PROCESS_CREATION_FLAGS| {
        // SAFETY: every string is NUL-terminated and outlives the call; `command` is writable, as
        // the call may change it; no handles are inherited.
        unsafe {
            CreateProcessW(
                &exe,
                Some(PWSTR(command.as_mut_ptr())),
                None,
                None,
                false,
                flags,
                None,
                &folder,
                &startup,
                &mut started,
            )
        }
    };
    match create(CREATE_BREAKAWAY_FROM_JOB | DETACHED_PROCESS) {
        Ok(()) => {}
        // The job this process is in doesn't allow breaking away.
        Err(e) if e.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            log::info!("starting Lectern inside this process's job, which keeps it");
            create(DETACHED_PROCESS).map_err(|e| io::Error::other(e.message()))?;
        }
        Err(e) => return Err(io::Error::other(e.message())),
    }
    // SAFETY: closes the two handles the call returned, once.
    unsafe {
        let _ = CloseHandle(started.hThread);
        let _ = CloseHandle(started.hProcess);
    }
    Ok(started.dwProcessId)
}

/// `arg` quoted for a command line, when it needs it.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_owned();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent;
    use std::io::Cursor;

    /// What the stdio loop writes, shared with the test.
    #[derive(Clone, Default)]
    struct Output(Arc<Mutex<Vec<u8>>>);

    impl Write for Output {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            lock(&self.0).extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Output {
        fn lines(&self) -> Vec<Value> {
            String::from_utf8(lock(&self.0).clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    fn wait_for(what: &str, f: impl Fn() -> bool) {
        let started = Instant::now();
        while !f() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out: {what}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// The stdio loop, with in-memory stdin and stdout: protocol messages answered at once, tool
    /// calls relayed, and every line ending in `\n` alone.
    #[test]
    fn the_stdio_loop_answers_each_line_and_relays_tool_calls() {
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.0"}}}"#,
            "\r\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            "not json\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"status","arguments":{}}}"#,
            "\n",
        );
        let output = Output::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let relay: Relay = {
            let seen = Arc::clone(&seen);
            Arc::new(move |request, client| {
                lock(&seen).push((request, client));
                Ok(json!({"windows": []}))
            })
        };
        serve(Cursor::new(input), output.clone(), relay).unwrap();
        wait_for("the tool call's answer", || output.lines().len() == 3);
        let text = String::from_utf8(lock(&output.0).clone()).unwrap();
        assert!(!text.contains('\r'), "{text}");
        let lines = output.lines();
        assert_eq!(lines[0]["result"]["serverInfo"]["name"], "lectern");
        assert_eq!(lines[1]["error"]["code"], code::PARSE_ERROR);
        assert_eq!(lines[2]["id"], 2);
        assert_eq!(
            lines[2]["result"]["structuredContent"],
            json!({"windows": []})
        );
        let seen = lock(&seen);
        assert_eq!(seen[0].0, AgentRequest::Status);
        assert_eq!(seen[0].1.as_ref().unwrap().name, "claude-code");
    }

    /// The pipe the stub Lectern serves, from its command line (a test name filter it matches
    /// none of), when this run is that stub.
    fn stub_pipe() -> Option<String> {
        std::env::args().find(|arg| arg.starts_with(r"\\.\pipe\lectern-test-stub-"))
    }

    /// A stand-in Lectern, started by `mcp_starts_lectern_when_none_runs` as its own process:
    /// serves `status` on the pipe its command line names until its first client leaves, or 30
    /// seconds pass.
    #[test]
    #[ignore = "started by mcp_starts_lectern_when_none_runs, as its own process"]
    fn stub_lectern() {
        let Some(name) = stub_pipe() else {
            return;
        };
        let served = Arc::new(AtomicBool::new(false));
        let handler: agent::Handler = {
            let served = Arc::clone(&served);
            Arc::new(move |request| {
                served.store(true, Ordering::SeqCst);
                match request {
                    AgentRequest::Status => Ok(json!({"stub": std::process::id()})),
                    _ => Err("Not yet available.".to_owned()),
                }
            })
        };
        let _server = agent::serve(&name, handler).unwrap();
        let started = Instant::now();
        while !served.load(Ordering::SeqCst) && started.elapsed() < Duration::from_secs(30) {
            thread::sleep(Duration::from_millis(50));
        }
        // Long enough for the answer to be written.
        thread::sleep(Duration::from_millis(500));
    }

    /// With no Lectern on the pipe, the first call starts one (here the stub, this test binary
    /// run as `stub_lectern`), detached, and is answered by it.
    #[test]
    fn mcp_starts_lectern_when_none_runs() {
        let name = format!(r"\\.\pipe\lectern-test-stub-{}", std::process::id());
        let launched = Arc::new(Mutex::new(None));
        let lectern = {
            let (name, launched) = (name.clone(), Arc::clone(&launched));
            Lectern::new(
                name.clone(),
                pipe::user_sid().unwrap(),
                Box::new(move || {
                    let exe = std::env::current_exe()?;
                    let pid = launch_detached(
                        &exe,
                        &["mcp::tests::stub_lectern", "--exact", "--ignored", &name],
                    )?;
                    *lock(&launched) = Some(pid);
                    Ok(())
                }),
            )
        };
        let answer = lectern.call(AgentRequest::Status, None).unwrap();
        let pid = lock(&launched).expect("the stub was started");
        assert_eq!(answer, json!({"stub": pid}));
    }

    /// A pipe whose server runs as another user (here: this test claims a user it isn't) is
    /// never written to, nor is Lectern started in its place: every call is refused, each with
    /// one try.
    #[test]
    fn a_pipe_served_by_another_user_is_refused() {
        let name = format!(r"\\.\pipe\lectern-test-foreign-{}", std::process::id());
        let asked = Arc::new(AtomicU64::new(0));
        let handler: agent::Handler = {
            let asked = Arc::clone(&asked);
            Arc::new(move |_| {
                asked.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            })
        };
        let _server = agent::serve(&name, handler).unwrap();
        let launched = Arc::new(AtomicU64::new(0));
        let lectern = {
            let launched = Arc::clone(&launched);
            Lectern::new(
                name,
                "S-1-5-21-1000-2000-3000-1001".to_owned(),
                Box::new(move || {
                    launched.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            )
        };
        let client = Some(ClientInfo {
            name: "claude-code".to_owned(),
            version: "2.1.0".to_owned(),
        });
        for _ in 0..2 {
            assert_eq!(
                lectern.call(AgentRequest::Status, client.clone()),
                Err(FOREIGN_PIPE.to_owned())
            );
        }
        thread::sleep(Duration::from_millis(200));
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        assert_eq!(launched.load(Ordering::SeqCst), 0);
    }

    /// A line on stdin over the limit is refused with no id, and the next line is answered.
    #[test]
    fn a_line_over_the_limit_on_stdin_is_refused_and_the_next_answered() {
        let mut input = vec![b' '; MAX_LINE + 1];
        input.push(b'\n');
        input.extend_from_slice(br#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
        input.push(b'\n');
        let output = Output::default();
        let relay: Relay = Arc::new(|_, _| panic!("nothing to relay"));
        serve(Cursor::new(input), output.clone(), relay).unwrap();
        let lines = output.lines();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0]["id"], Value::Null);
        assert_eq!(lines[0]["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(lines[1], json!({"jsonrpc": "2.0", "id": 2, "result": {}}));
    }

    /// A connection whose answers stopped (Lectern sent a line that isn't one) answers a request
    /// as lost at once, instead of waiting it out.
    #[test]
    fn a_connection_whose_answers_stopped_is_lost_at_once() {
        let name = format!(r"\\.\pipe\lectern-test-dead-{}", std::process::id());
        let mut listener = pipe::listen(&name).unwrap();
        let server = thread::spawn(move || {
            let pipe = listener.accept().expect("a client");
            pipe.write_all(b"not an answer\n").unwrap();
            pipe
        });
        let connection = Connection::start(pipe::connect(&name).unwrap()).unwrap();
        // Lectern's end stays open.
        let _lectern = server.join().unwrap();
        wait_for("the answers to stop", || !connection.alive());
        let started = Instant::now();
        assert_eq!(
            connection.ask(AgentRequest::Status, Duration::from_secs(3)),
            Err(Failure::Lost)
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn arguments_are_quoted_when_they_need_it() {
        assert_eq!(quote("--exact"), "--exact");
        assert_eq!(
            quote(r"C:\Program Files\Lectern\lectern.exe"),
            r#""C:\Program Files\Lectern\lectern.exe""#
        );
        assert_eq!(quote(""), r#""""#);
    }
}
