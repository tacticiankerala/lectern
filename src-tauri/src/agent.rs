//! The agent pipe's server, in the Lectern window process: `lectern.exe mcp` connects and sends
//! one request per line, each checked against its tool's limits and answered in time
//! (`answer_within`: `ANSWER_WITHIN`, and a `show`'s wait for the reader), on a thread of its own,
//! by the app (`App::agent_answer`), for the agent its `hello` named: who it is, and the WSL
//! distribution it runs in. Connections are served side by side. Past `MAX_CONNECTIONS`
//! connections, or `MAX_IN_FLIGHT` requests on one, the answer is that Lectern is busy. A
//! connection idle for `IDLE` is closed, freeing its place, and `mcp` connects again. The pipe is
//! made as soon as it can be, and kept for as long as Lectern runs: agent control never dies
//! quietly.

use std::io::{self, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use lectern_core::agent::lines::{Line, LineReader};
use lectern_core::agent::{
    answer_within, printable, Agent, AgentRequest, Answer, PipeReply, PipeRequest, MAX_LINE,
};
use serde_json::{json, Value};

#[cfg(test)]
use crate::pipe::Stopper;
use crate::pipe::{self, Listener, Pipe};
use crate::state::App;

/// What answers a request, from the agent (`Agent`: as the reader sees it, and the WSL
/// distribution it runs in).
pub type Handler = Arc<dyn Fn(AgentRequest, &Agent) -> Result<Value, String> + Send + Sync>;

/// The answer when the app takes too long.
const TOO_SLOW: &str = "Lectern didn't answer in time.";

/// The most connections served at once.
const MAX_CONNECTIONS: usize = 16;
/// The most requests one connection has Lectern working on at once.
const MAX_IN_FLIGHT: usize = 8;
/// The answer to a request past either limit.
const BUSY: &str = "Lectern is busy.";

/// How long a connection may be idle, nothing in flight, before it is closed: its place goes to
/// another agent, and `mcp` connects again at its next call.
const IDLE: Duration = Duration::from_secs(10 * 60);

/// The pause before a pipe that couldn't be named or made is tried again, the first time, and the
/// longest.
const LISTEN_RETRY_FIRST: Duration = Duration::from_secs(1);
const LISTEN_RETRY_MOST: Duration = Duration::from_secs(60);

/// Starts serving agents for the app `identifier`, off the main thread, on its pipe as soon as it
/// can be made (`listen_until_up`).
pub fn start(app: Arc<App>, identifier: &str) {
    let identifier = identifier.to_owned();
    let started = thread::Builder::new()
        .name("lectern-agent-pipe".to_owned())
        .spawn(move || {
            let listener = listen_until_up(
                || pipe::agent_pipe(&identifier).map(|(name, _)| name),
                pipe::listen,
                LISTEN_RETRY_FIRST,
                LISTEN_RETRY_MOST,
            );
            if let Some(listener) = listener {
                accept_loop(
                    listener,
                    Arc::new(move |r, agent| app.agent_answer(r, agent)),
                    IDLE,
                );
            }
        });
    if let Err(e) = started {
        log::error!("couldn't start the agent pipe: {e}");
    }
}

/// The listener of the pipe `name` names, made with `listen`. Agent control doesn't die quietly:
/// while the name can't be worked out (the user's SID, the session) or the pipe can't be made, it
/// is logged and tried again, after a pause doubling from `first` up to `most`, for as long as
/// Lectern runs. Only another process holding the name (a second Lectern, or another user who made
/// it first: access denied) ends it, with agent control off for this run, which is logged.
fn listen_until_up(
    name: impl Fn() -> io::Result<String>,
    listen: impl Fn(&str) -> io::Result<Listener>,
    first: Duration,
    most: Duration,
) -> Option<Listener> {
    let mut pause = first;
    loop {
        match name().and_then(|name| listen(&name)) {
            Ok(listener) => return Some(listener),
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                log::warn!(
                    "agent control is off this run: another process holds the agent pipe's name \
                     ({e})"
                );
                return None;
            }
            Err(e) => {
                log::warn!("the agent pipe can't be made yet ({e}); trying again in {pause:?}");
                thread::sleep(pause);
                pause = (pause * 2).min(most);
            }
        }
    }
}

/// A server whose accept loop runs on a thread of its own, stopped when dropped: for tests, as
/// Lectern's own serves until it exits.
#[cfg(test)]
pub struct Server {
    stop: Stopper,
    accepting: Option<thread::JoinHandle<()>>,
}

#[cfg(test)]
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(accepting) = self.accepting.take() {
            let _ = accepting.join();
        }
    }
}

/// Serves the pipe `name`, answering with `handle`.
#[cfg(test)]
pub fn serve(name: &str, handle: Handler) -> io::Result<Server> {
    serve_idle(name, handle, IDLE)
}

/// As `serve`, closing a connection once it is `idle` that long.
#[cfg(test)]
pub fn serve_idle(name: &str, handle: Handler, idle: Duration) -> io::Result<Server> {
    let listener = pipe::listen(name)?;
    let stop = listener.stopper();
    let accepting = thread::Builder::new()
        .name("lectern-agent-pipe".to_owned())
        .spawn(move || accept_loop(listener, handle, idle))?;
    Ok(Server {
        stop,
        accepting: Some(accepting),
    })
}

/// Serves each client on a thread of its own, at most [`MAX_CONNECTIONS`] at once, until the
/// listener stops, closing one `idle` that long. A client past that is told Lectern is busy, by
/// a thread of its own too, as many again at most; past those, its connection is closed at once.
fn accept_loop(mut listener: Listener, handle: Handler, idle: Duration) {
    let serving = Arc::new(AtomicUsize::new(0));
    let refusing = Arc::new(AtomicUsize::new(0));
    while let Some(pipe) = listener.accept() {
        let pipe = Arc::new(pipe);
        let started = if let Some(place) = Taken::place(&serving, MAX_CONNECTIONS) {
            let handle = Arc::clone(&handle);
            thread::Builder::new()
                .name("lectern-agent".to_owned())
                .spawn(move || {
                    serve_connection(pipe, &handle, idle);
                    drop(place);
                })
        } else if let Some(place) = Taken::place(&refusing, MAX_CONNECTIONS) {
            log::warn!("{MAX_CONNECTIONS} agents are connected already; telling another it's busy");
            thread::Builder::new()
                .name("lectern-agent-busy".to_owned())
                .spawn(move || {
                    refuse_connection(&pipe);
                    drop(place);
                })
        } else {
            log::warn!("too many agents are connected; closing a connection");
            continue;
        };
        if let Err(e) = started {
            log::warn!("couldn't serve an agent: {e}");
        }
    }
}

/// A place taken in a count, given back when dropped.
struct Taken(Arc<AtomicUsize>);

impl Taken {
    /// A place in `count`, when fewer than `most` are taken.
    fn place(count: &Arc<AtomicUsize>, most: usize) -> Option<Self> {
        count
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n < most).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(Arc::clone(count)))
    }
}

impl Drop for Taken {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Answers a connection past the limit: each request is answered [`BUSY`], until the first that
/// isn't a `hello`; then it is closed, so the client connects again for its next request.
fn refuse_connection(pipe: &Pipe) {
    let writing = Mutex::new(());
    let mut lines = LineReader::new(pipe, MAX_LINE);
    while let Ok(Line::Text(line)) = lines.next_line() {
        let Ok(PipeRequest { id, request }) = serde_json::from_str(&line) else {
            return;
        };
        reply(pipe, &writing, id, Err(BUSY.to_owned()));
        if !matches!(request, AgentRequest::Hello { .. }) {
            return;
        }
    }
}

/// Reads the client's requests one line at a time, answering each on a thread of its own, until
/// it closes or sends something that isn't a request. Each is checked against its tool's limits
/// (`AgentRequest::check`), and one past them refused. At most [`MAX_IN_FLIGHT`] are in flight
/// at once, each until its answer is written; one past that is answered [`BUSY`] at once. Each is
/// answered for the agent the last `hello` named; a `hello` naming a WSL distribution that can't
/// be one is refused, and closes the connection.
fn serve_connection(pipe: Arc<Pipe>, handle: &Handler, idle: Duration) {
    let writing = Arc::new(Mutex::new(()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let used = Arc::new(Mutex::new(Instant::now()));
    let mut agent = Agent::default();
    let reader = Idle {
        pipe: &pipe,
        idle,
        used: &used,
        in_flight: &in_flight,
    };
    let mut lines = LineReader::new(reader, MAX_LINE);
    loop {
        let line = match lines.next_line() {
            Ok(Line::Text(line)) => line,
            Ok(Line::TooLong) => {
                log::warn!("an agent sent a line over {MAX_LINE} bytes; closing its connection");
                return;
            }
            Ok(Line::End) => return,
            Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                log::info!("closing an agent's connection, idle for {idle:?}");
                return;
            }
            Err(e) => {
                log::warn!("an agent's connection failed: {e}");
                return;
            }
        };
        used_now(&used);
        let PipeRequest { id, request } = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                log::warn!("an agent sent a line that isn't a request ({e}); closing it");
                return;
            }
        };
        if let AgentRequest::Hello { client, distro } = &request {
            agent = match Agent::hello(client.as_ref(), distro.as_deref()) {
                Ok(named) => named,
                Err(refusal) => {
                    log::warn!("an agent named a WSL distribution that can't be one; closing it");
                    reply(&pipe, &writing, id, Err(refusal));
                    return;
                }
            };
            log::info!(
                "an agent connected: {}{}",
                client.as_ref().map_or_else(
                    || "no name given".to_owned(),
                    |c| format!("{} {}", printable(&c.name), printable(&c.version))
                ),
                agent
                    .distro
                    .as_ref()
                    .map_or_else(String::new, |d| format!(", in WSL's {d}"))
            );
            reply(
                &pipe,
                &writing,
                id,
                Ok(json!({"lectern": lectern_core::version()})),
            );
            continue;
        }
        // Checked again here, as any of the user's processes may write to the pipe.
        if let Err(refusal) = request.check() {
            reply(&pipe, &writing, id, Err(refusal));
            continue;
        }
        let Some(place) = Taken::place(&in_flight, MAX_IN_FLIGHT) else {
            reply(&pipe, &writing, id, Err(BUSY.to_owned()));
            continue;
        };
        let (pipe, writing, handle) = (Arc::clone(&pipe), Arc::clone(&writing), Arc::clone(handle));
        let (agent, used) = (agent.clone(), Arc::clone(&used));
        let answering = thread::Builder::new()
            .name("lectern-agent-call".to_owned())
            .spawn(move || {
                // Held until both the app is done with the request and its answer is written.
                let place = Arc::new(place);
                let answer = answer_in_time(&handle, request, agent, Arc::clone(&place));
                reply(&pipe, &writing, id, answer);
                used_now(&used);
                drop(place);
            });
        if let Err(e) = answering {
            log::warn!("couldn't answer an agent: {e}");
        }
    }
}

/// A connection's pipe, as `serve_connection` reads it: a read that waits until the connection
/// has been `idle` that long since it was last `used` (a request read, or an answer written), with
/// nothing `in_flight`, ends it as timed out.
struct Idle<'a> {
    pipe: &'a Pipe,
    idle: Duration,
    used: &'a Mutex<Instant>,
    in_flight: &'a AtomicUsize,
}

impl Read for Idle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let unused = self
                .used
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .elapsed();
            let busy = self.in_flight.load(Ordering::SeqCst) > 0;
            if !busy && unused >= self.idle {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "idle"));
            }
            // Asked again once the rest of the time has passed, or, in flight, a whole one.
            let wait = if busy { self.idle } else { self.idle - unused };
            match self.pipe.read_within(buf, wait) {
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {}
                read => return read,
            }
        }
    }
}

/// Notes that a connection is used now (`Idle`).
fn used_now(used: &Mutex<Instant>) {
    *used.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
}

/// `handle`'s answer to `request` from `agent`, or an error once its time (`answer_within`) has
/// passed without one. The request keeps its `place` until `handle` is done with it, however long
/// that takes, as well as until its answer is written, which the caller holds it for.
fn answer_in_time(
    handle: &Handler,
    request: AgentRequest,
    agent: Agent,
    place: Arc<Taken>,
) -> Result<Value, String> {
    let (done, answered) = mpsc::channel();
    let handle = Arc::clone(handle);
    let within = answer_within(&request);
    let working = thread::Builder::new()
        .name("lectern-agent-work".to_owned())
        .spawn(move || {
            let answer = handle(request, &agent);
            drop(place);
            let _ = done.send(answer);
        });
    if let Err(e) = working {
        return Err(format!("Lectern couldn't take the request: {e}"));
    }
    answered
        .recv_timeout(within)
        .unwrap_or_else(|_| Err(TOO_SLOW.to_owned()))
}

/// Writes the answer to request `id` as one line, whole, while no other answer is being written.
fn reply(pipe: &Pipe, writing: &Mutex<()>, id: u64, answer: Result<Value, String>) {
    let mut line = line_of(id, Answer::from(answer));
    if line.len() >= MAX_LINE {
        line = line_of(id, Answer::Err("Lectern's answer was too long.".to_owned()));
    }
    line.push('\n');
    let _writing = writing.lock().unwrap_or_else(PoisonError::into_inner);
    if let Err(e) = pipe.write_all(line.as_bytes()) {
        log::warn!("couldn't answer an agent: {e}");
    }
}

fn line_of(id: u64, answer: Answer) -> String {
    serde_json::to_string(&PipeReply { id, answer }).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipe::connect;
    use lectern_core::agent::request::{ClientInfo, HighlightArgs, ShowArgs, Target};
    use std::io::Read;
    use std::sync::Condvar;
    use std::time::Duration;

    fn test_name(what: &str) -> String {
        format!(r"\\.\pipe\lectern-test-agent-{what}-{}", std::process::id())
    }

    /// Sends `request` as `id` and reads lines until that id's answer.
    fn ask(pipe: &Pipe, lines: &mut LineReader<&Pipe>, id: u64, request: AgentRequest) -> Answer {
        let mut line = serde_json::to_string(&PipeRequest { id, request }).unwrap();
        line.push('\n');
        pipe.write_all(line.as_bytes()).unwrap();
        loop {
            match lines.next_line().unwrap() {
                Line::Text(line) => {
                    let reply: PipeReply = serde_json::from_str(&line).unwrap();
                    if reply.id == id {
                        return reply.answer;
                    }
                }
                other => panic!("no answer: {other:?}"),
            }
        }
    }

    fn status_handler() -> Handler {
        Arc::new(|request, _| match request {
            AgentRequest::Status => Ok(json!({"windows": [], "closedWorkspaces": []})),
            _ => Err("Not yet available.".to_owned()),
        })
    }

    #[test]
    fn two_clients_exchange_status_with_one_server() {
        let name = test_name("two");
        let _server = serve(&name, status_handler()).unwrap();
        let first = connect(&name).unwrap();
        let second = connect(&name).unwrap();
        let mut first_lines = LineReader::new(&first, MAX_LINE);
        let mut second_lines = LineReader::new(&second, MAX_LINE);
        let hello = hello_from(None);
        assert!(matches!(
            ask(&first, &mut first_lines, 1, hello),
            Answer::Ok(_)
        ));
        for (pipe, lines) in [(&first, &mut first_lines), (&second, &mut second_lines)] {
            assert_eq!(
                ask(pipe, lines, 2, AgentRequest::Status),
                Answer::Ok(json!({"windows": [], "closedWorkspaces": []}))
            );
        }
        assert_eq!(
            ask(
                &second,
                &mut second_lines,
                3,
                AgentRequest::Read(Default::default())
            ),
            Answer::Err("Not yet available.".to_owned())
        );
    }

    /// Each request is answered for the agent its connection's `hello` named, as the reader sees
    /// it; before any `hello`, for an unnamed agent.
    #[test]
    fn requests_are_answered_for_the_agent_hello_named() {
        let name = test_name("named");
        let _server = serve(&name, Arc::new(|_, agent| Ok(json!(agent.name)))).unwrap();
        let pipe = connect(&name).unwrap();
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        assert_eq!(
            ask(&pipe, &mut lines, 1, AgentRequest::Status),
            Answer::Ok(json!("An agent"))
        );
        let hello = hello_from(None);
        assert!(matches!(ask(&pipe, &mut lines, 2, hello), Answer::Ok(_)));
        assert_eq!(
            ask(&pipe, &mut lines, 3, AgentRequest::Status),
            Answer::Ok(json!("Claude"))
        );
    }

    /// Claude Code's `hello`, from the WSL distribution `distro` when one is given
    /// (`lectern.exe mcp --distro`).
    fn hello_from(distro: Option<&str>) -> AgentRequest {
        AgentRequest::Hello {
            client: Some(ClientInfo {
                name: "claude-code".to_owned(),
                version: "2.1.0".to_owned(),
            }),
            distro: distro.map(str::to_owned),
        }
    }

    /// The WSL distribution a connection's `hello` named is the agent's, for each request after
    /// it, on that connection alone; a `hello` without one leaves the agent in none of its own.
    #[test]
    fn requests_are_answered_in_the_distro_hello_named() {
        let name = test_name("distro");
        let _server = serve(&name, Arc::new(|_, agent| Ok(json!(agent.distro)))).unwrap();
        let debian = connect(&name).unwrap();
        let mut debian_lines = LineReader::new(&debian, MAX_LINE);
        let windows = connect(&name).unwrap();
        let mut windows_lines = LineReader::new(&windows, MAX_LINE);
        assert!(matches!(
            ask(&debian, &mut debian_lines, 1, hello_from(Some("Debian"))),
            Answer::Ok(_)
        ));
        assert!(matches!(
            ask(&windows, &mut windows_lines, 1, hello_from(None)),
            Answer::Ok(_)
        ));
        assert_eq!(
            ask(&debian, &mut debian_lines, 2, AgentRequest::Status),
            Answer::Ok(json!("Debian"))
        );
        assert_eq!(
            ask(&windows, &mut windows_lines, 2, AgentRequest::Status),
            Answer::Ok(Value::Null)
        );
    }

    /// A `hello` naming a distribution that can't be one (it would go into a path) is refused,
    /// saying why, and its connection closed; nothing it sent is answered by the app.
    #[test]
    fn a_hello_with_a_distro_that_cant_be_one_is_refused_and_closed() {
        let name = test_name("bad-distro");
        let asked = Arc::new(AtomicUsize::new(0));
        let handler: Handler = {
            let asked = Arc::clone(&asked);
            Arc::new(move |_, _| {
                asked.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            })
        };
        let _server = serve(&name, handler).unwrap();
        let pipe = connect(&name).unwrap();
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        let bad = r"..\..\nas2";
        assert_eq!(
            ask(&pipe, &mut lines, 1, hello_from(Some(bad))),
            Answer::Err(lectern_core::agent::check_distro(bad).unwrap_err())
        );
        assert_eq!(lines.next_line().unwrap(), Line::End);
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_slow_answer_does_not_hold_up_the_next() {
        let name = test_name("slow");
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let handler: Handler = Arc::new(move |request, _| match request {
            AgentRequest::Status => {
                let _ = released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5));
                Ok(json!("slow"))
            }
            _ => Ok(json!("quick")),
        });
        let _server = serve(&name, handler).unwrap();
        let pipe = connect(&name).unwrap();
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        let mut status = serde_json::to_string(&PipeRequest {
            id: 1,
            request: AgentRequest::Status,
        })
        .unwrap();
        status.push('\n');
        pipe.write_all(status.as_bytes()).unwrap();
        assert_eq!(
            ask(&pipe, &mut lines, 2, AgentRequest::Read(Default::default())),
            Answer::Ok(json!("quick"))
        );
        release.send(()).unwrap();
        match lines.next_line().unwrap() {
            Line::Text(line) => assert_eq!(
                serde_json::from_str::<PipeReply>(&line).unwrap(),
                PipeReply {
                    id: 1,
                    answer: Answer::Ok(json!("slow"))
                }
            ),
            other => panic!("{other:?}"),
        }
    }

    /// A request is checked against its tool's limits on the pipe too, as any of the user's
    /// processes may write to it: one past them is refused, saying why, and never reaches the app.
    #[test]
    fn a_request_past_its_tools_limits_is_refused_unanswered_by_the_app() {
        let name = test_name("limits");
        let asked = Arc::new(AtomicUsize::new(0));
        let handler: Handler = {
            let asked = Arc::clone(&asked);
            Arc::new(move |_, _| {
                asked.fetch_add(1, Ordering::SeqCst);
                Ok(json!({}))
            })
        };
        let _server = serve(&name, handler).unwrap();
        let pipe = connect(&name).unwrap();
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        let waits_for_ever = AgentRequest::Show(ShowArgs {
            path: r"S:\Notes\My Vault\plan.md".to_owned(),
            workspace: None,
            target: None,
            new_window: false,
            wait: u32::MAX,
        });
        let refused = |answer: Answer, says: &str| match answer {
            Answer::Err(message) => assert!(message.contains(says), "{message}"),
            other => panic!("answered: {other:?}"),
        };
        refused(ask(&pipe, &mut lines, 1, waits_for_ever), "wait");
        let long_quote = AgentRequest::Highlight(HighlightArgs {
            path: None,
            targets: vec![Target::Quote {
                quote: "q".repeat(2001),
                prefix: None,
                suffix: None,
            }],
            clear: false,
        });
        refused(ask(&pipe, &mut lines, 2, long_quote), "2,000");
        assert_eq!(asked.load(Ordering::SeqCst), 0);
        assert!(matches!(
            ask(&pipe, &mut lines, 3, AgentRequest::Status),
            Answer::Ok(_)
        ));
    }

    /// A request keeps its place among those in flight until its answer is written, not only
    /// until the app has answered: answers larger than the pipe holds, unread, keep theirs, so one
    /// more request is answered busy.
    #[test]
    fn a_place_in_flight_is_held_until_its_answer_is_written() {
        let name = test_name("written");
        let entered = Arc::new(AtomicUsize::new(0));
        let handler: Handler = {
            let entered = Arc::clone(&entered);
            let big = "x".repeat(200 * 1024);
            Arc::new(move |request, _| match request {
                AgentRequest::Status => {
                    entered.fetch_add(1, Ordering::SeqCst);
                    Ok(json!(big))
                }
                _ => Ok(json!("quick")),
            })
        };
        let _server = serve(&name, handler).unwrap();
        let pipe = connect(&name).unwrap();
        for id in 1..=MAX_IN_FLIGHT as u64 {
            send(&pipe, id, AgentRequest::Status);
        }
        wait_until("the app answers every one", || {
            entered.load(Ordering::SeqCst) == MAX_IN_FLIGHT
        });
        // Long enough for a place given back with the app's answer to be free.
        thread::sleep(Duration::from_millis(200));
        send(&pipe, 99, AgentRequest::Read(Default::default()));
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        let mut answers = std::collections::HashMap::new();
        while answers.len() <= MAX_IN_FLIGHT {
            let Line::Text(line) = lines.next_line().unwrap() else {
                panic!("closed");
            };
            let reply: PipeReply = serde_json::from_str(&line).unwrap();
            answers.insert(reply.id, reply.answer);
        }
        assert_eq!(answers[&99], Answer::Err(BUSY.to_owned()));
    }

    /// Another process holding the name (a second Lectern, or another user who made it first)
    /// leaves agent control off for this run, at once, and nothing worse.
    #[test]
    fn a_taken_name_leaves_agent_control_off() {
        let name = test_name("taken");
        let _first = pipe::listen(&name).unwrap();
        let pause = Duration::from_millis(50);
        assert!(listen_until_up(|| Ok(name.clone()), pipe::listen, pause, pause).is_none());
    }

    /// Agent control doesn't die quietly: a pipe that can't be named or made yet is tried again,
    /// until it can be.
    #[test]
    fn a_pipe_that_cant_be_made_yet_is_tried_again_until_it_can() {
        let name = test_name("retry");
        let (named, made) = (AtomicUsize::new(0), AtomicUsize::new(0));
        let listener = listen_until_up(
            || match named.fetch_add(1, Ordering::SeqCst) {
                0 => Err(io::Error::other("no session yet, for a test")),
                _ => Ok(name.clone()),
            },
            |name| match made.fetch_add(1, Ordering::SeqCst) {
                0 | 1 => Err(io::Error::other("no pipe yet, for a test")),
                _ => pipe::listen(name),
            },
            Duration::from_millis(10),
            Duration::from_millis(40),
        );
        assert!(listener.is_some());
        assert_eq!(made.load(Ordering::SeqCst), 3);
        assert!(listen_until_up(
            || Ok(name.clone()),
            pipe::listen,
            Duration::ZERO,
            Duration::ZERO
        )
        .is_none());
    }

    /// A connection idle for its time, nothing in flight, is closed, freeing its place; one with a
    /// request in flight isn't, however long the answer takes.
    #[test]
    fn an_idle_connection_is_closed_but_not_one_waiting_on_an_answer() {
        let name = test_name("idle");
        let (handler, gate, entered) = gated();
        let _server = serve_idle(&name, handler, Duration::from_millis(300)).unwrap();
        let pipe = Arc::new(connect(&name).unwrap());
        let (read, lines) = mpsc::channel();
        let reading = {
            let pipe = Arc::clone(&pipe);
            thread::spawn(move || {
                let mut reader = LineReader::new(&*pipe, MAX_LINE);
                loop {
                    let line = reader.next_line().map_err(|e| e.kind());
                    let more = matches!(line, Ok(Line::Text(_)));
                    if read.send(line).is_err() || !more {
                        break;
                    }
                }
            })
        };
        send(&pipe, 1, AgentRequest::Status);
        wait_until("the request is worked on", || {
            entered.load(Ordering::SeqCst) == 1
        });
        thread::sleep(Duration::from_millis(900));
        assert!(lines.try_recv().is_err(), "closed with a request in flight");
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let Ok(Ok(Line::Text(line))) = lines.recv_timeout(Duration::from_secs(5)) else {
            panic!("no answer");
        };
        let reply: PipeReply = serde_json::from_str(&line).unwrap();
        assert_eq!((reply.id, reply.answer), (1, Answer::Ok(json!("slow"))));
        assert_eq!(
            lines.recv_timeout(Duration::from_secs(5)),
            Ok(Ok(Line::End)),
            "kept open, idle"
        );
        reading.join().unwrap();
    }

    /// Shut until set to true, then notified.
    type Gate = Arc<(Mutex<bool>, Condvar)>;

    /// A handler whose `status` waits until the gate opens, counting the requests it has.
    fn gated() -> (Handler, Gate, Arc<AtomicUsize>) {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let handler: Handler = {
            let (gate, entered) = (Arc::clone(&gate), Arc::clone(&entered));
            Arc::new(move |request, _| match request {
                AgentRequest::Status => {
                    entered.fetch_add(1, Ordering::SeqCst);
                    let (open, opened) = &*gate;
                    let open = open.lock().unwrap();
                    let _open = opened
                        .wait_timeout_while(open, Duration::from_secs(9), |open| !*open)
                        .unwrap();
                    Ok(json!("slow"))
                }
                _ => Ok(json!("quick")),
            })
        };
        (handler, gate, entered)
    }

    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let started = std::time::Instant::now();
        while !done() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "timed out: {what}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn send(pipe: &Pipe, id: u64, request: AgentRequest) {
        let mut line = serde_json::to_string(&PipeRequest { id, request }).unwrap();
        line.push('\n');
        pipe.write_all(line.as_bytes()).unwrap();
    }

    #[test]
    fn requests_past_the_limit_in_flight_are_answered_busy_at_once() {
        let name = test_name("in-flight");
        let (handler, gate, entered) = gated();
        let _server = serve(&name, handler).unwrap();
        let pipe = connect(&name).unwrap();
        let mut lines = LineReader::new(&pipe, MAX_LINE);
        for id in 1..=MAX_IN_FLIGHT as u64 {
            send(&pipe, id, AgentRequest::Status);
        }
        wait_until("every slow request is worked on", || {
            entered.load(Ordering::SeqCst) == MAX_IN_FLIGHT
        });
        assert_eq!(
            ask(
                &pipe,
                &mut lines,
                99,
                AgentRequest::Read(Default::default())
            ),
            Answer::Err(BUSY.to_owned())
        );
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let mut slow = 0;
        while slow < MAX_IN_FLIGHT {
            let Line::Text(line) = lines.next_line().unwrap() else {
                panic!("closed");
            };
            let reply: PipeReply = serde_json::from_str(&line).unwrap();
            assert_eq!(reply.answer, Answer::Ok(json!("slow")));
            slow += 1;
        }
        // Answered, they leave room for more, each once its answer is written: a request sent the
        // moment the last answer arrives may still find none.
        let started = std::time::Instant::now();
        for id in 100.. {
            match ask(
                &pipe,
                &mut lines,
                id,
                AgentRequest::Read(Default::default()),
            ) {
                Answer::Err(busy)
                    if busy == BUSY && started.elapsed() < Duration::from_secs(20) =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                answer => {
                    assert_eq!(answer, Answer::Ok(json!("quick")));
                    break;
                }
            }
        }
    }

    #[test]
    fn connections_past_the_limit_are_answered_busy_and_closed() {
        let name = test_name("connections");
        let _server = serve(&name, status_handler()).unwrap();
        let mut served: Vec<Pipe> = (0..MAX_CONNECTIONS)
            .map(|_| connect(&name).unwrap())
            .collect();
        for pipe in &served {
            let mut lines = LineReader::new(pipe, MAX_LINE);
            assert!(matches!(
                ask(pipe, &mut lines, 1, AgentRequest::Status),
                Answer::Ok(_)
            ));
        }
        let extra = connect(&name).unwrap();
        let mut lines = LineReader::new(&extra, MAX_LINE);
        let hello = hello_from(None);
        assert_eq!(
            ask(&extra, &mut lines, 1, hello),
            Answer::Err(BUSY.to_owned())
        );
        assert_eq!(
            ask(&extra, &mut lines, 2, AgentRequest::Status),
            Answer::Err(BUSY.to_owned())
        );
        assert_eq!(lines.next_line().unwrap(), Line::End);
        // Once one leaves, a new client is served.
        drop(served.pop());
        wait_until("a new client is served", || {
            let pipe = connect(&name).unwrap();
            let mut lines = LineReader::new(&pipe, MAX_LINE);
            ask(&pipe, &mut lines, 1, AgentRequest::Status) != Answer::Err(BUSY.to_owned())
        });
    }

    /// A line over the limit closes its connection alone: the others are still served, and new
    /// ones too.
    #[test]
    fn an_oversized_line_closes_only_its_connection() {
        let name = test_name("oversized");
        let _server = serve(&name, status_handler()).unwrap();
        let other = connect(&name).unwrap();
        let mut other_lines = LineReader::new(&other, MAX_LINE);
        let big = connect(&name).unwrap();
        let mut line = vec![b'x'; MAX_LINE + 1];
        line.push(b'\n');
        big.write_all(&line).unwrap();
        let mut rest = Vec::new();
        (&big).read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
        assert!(matches!(
            ask(&other, &mut other_lines, 1, AgentRequest::Status),
            Answer::Ok(_)
        ));
        let new = connect(&name).unwrap();
        let mut new_lines = LineReader::new(&new, MAX_LINE);
        assert!(matches!(
            ask(&new, &mut new_lines, 1, AgentRequest::Status),
            Answer::Ok(_)
        ));
    }

    /// A client that leaves halfway through a line closes its connection alone.
    #[test]
    fn a_half_written_line_then_leaving_closes_only_its_connection() {
        let name = test_name("half");
        let _server = serve(&name, status_handler()).unwrap();
        let other = connect(&name).unwrap();
        let mut other_lines = LineReader::new(&other, MAX_LINE);
        let half = connect(&name).unwrap();
        half.write_all(br#"{"id":1,"request":{"tool":"sta"#)
            .unwrap();
        drop(half);
        assert!(matches!(
            ask(&other, &mut other_lines, 1, AgentRequest::Status),
            Answer::Ok(_)
        ));
        let new = connect(&name).unwrap();
        let mut new_lines = LineReader::new(&new, MAX_LINE);
        assert!(matches!(
            ask(&new, &mut new_lines, 1, AgentRequest::Status),
            Answer::Ok(_)
        ));
    }

    #[test]
    fn a_line_that_is_not_a_request_closes_the_connection() {
        let name = test_name("garbage");
        let _server = serve(&name, status_handler()).unwrap();
        let pipe = connect(&name).unwrap();
        pipe.write_all(b"not json\n").unwrap();
        let mut rest = Vec::new();
        (&pipe).read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
        // The server goes on serving others.
        let other = connect(&name).unwrap();
        let mut lines = LineReader::new(&other, MAX_LINE);
        assert!(matches!(
            ask(&other, &mut lines, 1, AgentRequest::Status),
            Answer::Ok(_)
        ));
    }
}
