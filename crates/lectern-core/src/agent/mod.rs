//! Agent control: an agent (Claude Code, or any MCP client) drives Lectern through
//! `lectern.exe mcp`.
//!
//! - `lectern.exe mcp` speaks MCP over its stdin and stdout ([`mcp`]). Each tool call
//!   ([`tools`]) becomes an [`AgentRequest`], which it relays to the running Lectern over a
//!   per-user named pipe ([`pipe_name`]): one JSON line each way, [`PipeRequest`] then
//!   [`PipeReply`], each at most [`MAX_LINE`] bytes ([`lines`]).
//! - Lectern answers each request within [`ANSWER_WITHIN`]. What needs a window's view goes to
//!   that window's UI as an [`AgentCall`], which answers with an [`AgentReply`] ([`status`]).
//! - Paths from an agent may be Windows or WSL paths ([`paths`]); every path in an answer comes
//!   in both forms.
//! - `show` and `workspace` go to the window [`show::route_show`] picks; a window already showing
//!   another file may ask the reader first, and the agent is named to them as [`agent_name`] has it.
//! - A target (a heading, a line, a quote or a PDF's page) is found by core in a note or a text
//!   file, and by the window in an HTML page ([`target`]); `read` gives the text as shown, a page
//!   at a time ([`read`]).

pub mod lines;
pub mod mcp;
pub mod paths;
pub mod read;
pub mod request;
pub mod show;
pub mod status;
pub mod target;
pub mod tools;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use self::request::AgentRequest;
use self::request::ClientInfo;
pub use self::status::{AgentCall, AgentReply};

/// The longest line either end of the pipe sends, and the longest MCP message read from stdin, in
/// bytes.
pub const MAX_LINE: usize = 1024 * 1024;

/// How long Lectern takes at most to answer a request on the pipe; past it, the answer is an
/// error.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(10);

/// How long Lectern takes at most to answer `request` on the pipe: [`ANSWER_WITHIN`], and for a
/// `show`, the seconds it may wait for the reader's answer on top.
pub fn answer_within(request: &AgentRequest) -> Duration {
    match request {
        AgentRequest::Show(args) => ANSWER_WITHIN + Duration::from_secs(args.wait.into()),
        _ => ANSWER_WITHIN,
    }
}

/// The longest name the reader sees an agent by, in characters.
pub const AGENT_NAME_MAX: usize = 32;

/// What the reader sees an agent called when its client gives no name Lectern can show.
pub const UNNAMED_AGENT: &str = "An agent";

/// The name the reader sees an agent by, from the name its MCP client gives (`hello`): "Claude"
/// for Claude Code (`claude-code`) or any name starting `claude`, "Codex" for `codex`, in any
/// letter case. Any other name keeps only its ASCII letters, digits, spaces and `._-`, trimmed, at
/// most [`AGENT_NAME_MAX`] of them. With no client, or nothing left, or "You" left (the reader's
/// own name in a review sidecar), it is [`UNNAMED_AGENT`].
pub fn agent_name(client: Option<&ClientInfo>) -> String {
    let Some(client) = client else {
        return UNNAMED_AGENT.to_owned();
    };
    let given = client.name.trim().to_lowercase();
    if given.starts_with("claude") {
        return "Claude".to_owned();
    }
    if given == "codex" {
        return "Codex".to_owned();
    }
    let kept: String = client
        .name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
        .collect();
    let name: String = kept.trim().chars().take(AGENT_NAME_MAX).collect();
    let name = name.trim_end();
    if name.is_empty() || name.eq_ignore_ascii_case("you") {
        return UNNAMED_AGENT.to_owned();
    }
    name.to_owned()
}

/// The named pipe Lectern serves agents on, for the app `identifier` and the user whose SID (as a
/// string, `S-1-5-21-…`) is `user_sid`: `\\.\pipe\lectern-<identifier>-<16 hex digits>`, the
/// digits the start of the SID's SHA-256. Another build of Lectern (another identifier) and
/// another user each have their own.
pub fn pipe_name(identifier: &str, user_sid: &str) -> String {
    let digest = Sha256::digest(user_sid.as_bytes());
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!(r"\\.\pipe\lectern-{identifier}-{hex}")
}

/// The answer to a tool call when Lectern's pipe is served by a process of another user.
pub const FOREIGN_PIPE: &str = "Lectern's pipe is owned by another user; refusing to connect.";

/// Whether a pipe server running as the user `server_sid` is one the user `our_sid` may talk to:
/// the same user, both SIDs as text, which Windows reads in any letter case. Another logged-on
/// user can work out the pipe's name and make it first, as SIDs aren't secret.
pub fn same_user(our_sid: &str, server_sid: &str) -> bool {
    !our_sid.is_empty() && our_sid.eq_ignore_ascii_case(server_sid)
}

/// `text`, from a client, as it goes into a log: without control characters, so it can't break a
/// line or style a terminal.
pub fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

/// A request on the pipe: `{id, request}`. The id is the sender's, unique among its requests
/// still unanswered.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PipeRequest {
    pub id: u64,
    pub request: AgentRequest,
}

/// The answer to a [`PipeRequest`] with the same id: `{id, ok}`, the tool's result, or
/// `{id, err}`, a sentence for the agent saying why it failed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PipeReply {
    pub id: u64,
    #[serde(flatten)]
    pub answer: Answer,
}

/// What a request came to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    Ok(serde_json::Value),
    Err(String),
}

impl From<Result<serde_json::Value, String>> for Answer {
    fn from(result: Result<serde_json::Value, String>) -> Self {
        match result {
            Ok(value) => Self::Ok(value),
            Err(message) => Self::Err(message),
        }
    }
}

impl From<Answer> for Result<serde_json::Value, String> {
    fn from(answer: Answer) -> Self {
        match answer {
            Answer::Ok(value) => Ok(value),
            Answer::Err(message) => Err(message),
        }
    }
}
