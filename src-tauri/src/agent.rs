//! The agent pipe's server, in the Lectern window process: `lectern.exe mcp` connects and sends
//! one request per line, each answered within `ANSWER_WITHIN`, on a thread of its own, by the app
//! (`App::agent_answer`), and connections are served side by side. Past `MAX_CONNECTIONS`
//! connections, or `MAX_IN_FLIGHT` requests on one, the answer is that Lectern is busy.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use lectern_core::agent::lines::{Line, LineReader};
use lectern_core::agent::{
    pipe_name, printable, AgentRequest, Answer, PipeReply, PipeRequest, ANSWER_WITHIN, MAX_LINE,
};
use serde_json::{json, Value};

#[cfg(test)]
use crate::pipe::Stopper;
use crate::pipe::{self, Listener, Pipe};
use crate::state::App;

/// What answers a request.
pub type Handler = Arc<dyn Fn(AgentRequest) -> Result<Value, String> + Send + Sync>;

/// The answer when the app takes too long.
const TOO_SLOW: &str = "Lectern didn't answer in time.";

/// The most connections served at once.
const MAX_CONNECTIONS: usize = 16;
/// The most requests one connection has Lectern working on at once.
const MAX_IN_FLIGHT: usize = 8;
/// The answer to a request past either limit.
const BUSY: &str = "Lectern is busy.";

/// Starts serving agents for the app `identifier`, off the main thread. Without a pipe (another
/// process holds its name) agent control is off for this run, which is logged.
pub fn start(app: Arc<App>, identifier: &str) {
    let identifier = identifier.to_owned();
    let started = thread::Builder::new()
        .name("lectern-agent-pipe".to_owned())
        .spawn(move || {
            let name = match pipe::user_sid() {
                Ok(sid) => pipe_name(&identifier, &sid),
                Err(e) => {
                    log::warn!("agent control is off this run: no user SID for its pipe ({e})");
                    return;
                }
            };
            if let Some(listener) = listen_or_warn(&name) {
                accept_loop(listener, Arc::new(move |r| app.agent_answer(r)));
            }
        });
    if let Err(e) = started {
        log::error!("couldn't start the agent pipe: {e}");
    }
}

/// The pipe `name`'s listener, unless it can't be made: then agents can't reach this Lectern,
/// which is logged, and nothing else changes.
fn listen_or_warn(name: &str) -> Option<Listener> {
    match pipe::listen(name) {
        Ok(listener) => Some(listener),
        Err(e) => {
            log::warn!(
                "agent control is off this run: another process holds the agent pipe's name ({e})"
            );
            None
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
pub fn serve(name: &str, handle: Handler) -> std::io::Result<Server> {
    let listener = pipe::listen(name)?;
    let stop = listener.stopper();
    let accepting = thread::Builder::new()
        .name("lectern-agent-pipe".to_owned())
        .spawn(move || accept_loop(listener, handle))?;
    Ok(Server {
        stop,
        accepting: Some(accepting),
    })
}

/// Serves each client on a thread of its own, at most [`MAX_CONNECTIONS`] at once, until the
/// listener stops. A client past that is told Lectern is busy, by a thread of its own too, as
/// many again at most; past those, its connection is closed at once.
fn accept_loop(mut listener: Listener, handle: Handler) {
    let serving = Arc::new(AtomicUsize::new(0));
    let refusing = Arc::new(AtomicUsize::new(0));
    while let Some(pipe) = listener.accept() {
        let pipe = Arc::new(pipe);
        let started = if let Some(place) = Taken::place(&serving, MAX_CONNECTIONS) {
            let handle = Arc::clone(&handle);
            thread::Builder::new()
                .name("lectern-agent".to_owned())
                .spawn(move || {
                    serve_connection(pipe, &handle);
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
/// it closes or sends something that isn't a request. At most [`MAX_IN_FLIGHT`] are worked on at
/// once; one past that is answered [`BUSY`] at once.
fn serve_connection(pipe: Arc<Pipe>, handle: &Handler) {
    let writing = Arc::new(Mutex::new(()));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let mut lines = LineReader::new(&*pipe, MAX_LINE);
    loop {
        let line = match lines.next_line() {
            Ok(Line::Text(line)) => line,
            Ok(Line::TooLong) => {
                log::warn!("an agent sent a line over {MAX_LINE} bytes; closing its connection");
                return;
            }
            Ok(Line::End) => return,
            Err(e) => {
                log::warn!("an agent's connection failed: {e}");
                return;
            }
        };
        let PipeRequest { id, request } = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(e) => {
                log::warn!("an agent sent a line that isn't a request ({e}); closing it");
                return;
            }
        };
        if let AgentRequest::Hello { client } = &request {
            log::info!(
                "an agent connected: {} {}",
                printable(&client.name),
                printable(&client.version)
            );
            reply(
                &pipe,
                &writing,
                id,
                Ok(json!({"lectern": lectern_core::version()})),
            );
            continue;
        }
        let Some(place) = Taken::place(&in_flight, MAX_IN_FLIGHT) else {
            reply(&pipe, &writing, id, Err(BUSY.to_owned()));
            continue;
        };
        let (pipe, writing, handle) = (Arc::clone(&pipe), Arc::clone(&writing), Arc::clone(handle));
        let answering = thread::Builder::new()
            .name("lectern-agent-call".to_owned())
            .spawn(move || reply(&pipe, &writing, id, answer_within(&handle, request, place)));
        if let Err(e) = answering {
            log::warn!("couldn't answer an agent: {e}");
        }
    }
}

/// `handle`'s answer to `request`, or an error once `ANSWER_WITHIN` has passed without one. The
/// request keeps its `place` until `handle` is done with it, however long that takes.
fn answer_within(handle: &Handler, request: AgentRequest, place: Taken) -> Result<Value, String> {
    let (done, answered) = mpsc::channel();
    let handle = Arc::clone(handle);
    let working = thread::Builder::new()
        .name("lectern-agent-work".to_owned())
        .spawn(move || {
            let answer = handle(request);
            // Given back before the answer, so a client that has it may ask again at once.
            drop(place);
            let _ = done.send(answer);
        });
    if let Err(e) = working {
        return Err(format!("Lectern couldn't take the request: {e}"));
    }
    answered
        .recv_timeout(ANSWER_WITHIN)
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
    use lectern_core::agent::request::ClientInfo;
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
        Arc::new(|request| match request {
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
        let hello = AgentRequest::Hello {
            client: ClientInfo {
                name: "claude-code".to_owned(),
                version: "2.1.0".to_owned(),
            },
        };
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

    #[test]
    fn a_slow_answer_does_not_hold_up_the_next() {
        let name = test_name("slow");
        let (release, released) = mpsc::channel::<()>();
        let released = Mutex::new(released);
        let handler: Handler = Arc::new(move |request| match request {
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

    /// Another process holding the name (a second Lectern, or another user who made it first)
    /// leaves agent control off for this run, and nothing worse.
    #[test]
    fn a_taken_name_leaves_agent_control_off() {
        let name = test_name("taken");
        let _first = pipe::listen(&name).unwrap();
        assert!(listen_or_warn(&name).is_none());
    }

    /// Shut until set to true, then notified.
    type Gate = Arc<(Mutex<bool>, Condvar)>;

    /// A handler whose `status` waits until the gate opens, counting the requests it has.
    fn gated() -> (Handler, Gate, Arc<AtomicUsize>) {
        let gate: Gate = Arc::new((Mutex::new(false), Condvar::new()));
        let entered = Arc::new(AtomicUsize::new(0));
        let handler: Handler = {
            let (gate, entered) = (Arc::clone(&gate), Arc::clone(&entered));
            Arc::new(move |request| match request {
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
        // Answered, they leave room for more.
        assert_eq!(
            ask(
                &pipe,
                &mut lines,
                100,
                AgentRequest::Read(Default::default())
            ),
            Answer::Ok(json!("quick"))
        );
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
        let hello = AgentRequest::Hello {
            client: ClientInfo {
                name: "claude-code".to_owned(),
                version: "2.1.0".to_owned(),
            },
        };
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
