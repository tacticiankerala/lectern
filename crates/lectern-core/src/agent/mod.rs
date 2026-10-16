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

pub mod lines;
pub mod mcp;
pub mod paths;
pub mod request;
pub mod status;
pub mod tools;

use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use self::request::AgentRequest;
pub use self::status::{AgentCall, AgentReply};

/// The longest line either end of the pipe sends, and the longest MCP message read from stdin, in
/// bytes.
pub const MAX_LINE: usize = 1024 * 1024;

/// How long Lectern takes at most to answer a request on the pipe; past it, the answer is an
/// error.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(10);

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
