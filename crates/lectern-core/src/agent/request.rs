//! What an agent asks of Lectern: one [`AgentRequest`] per tool call, plus the `hello` that opens
//! a connection. `lectern.exe mcp` builds them from the tools' arguments ([`super::tools`]), which
//! it has validated, and sends them over the pipe; a window's UI gets the ones it answers.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use super::tools::{parse_call, CallError};

/// A request from an agent, tagged by `tool`: the nine tools, and `hello`, which names the client
/// and the WSL distribution it runs in (`lectern.exe mcp --distro`) once a connection opens.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "tool", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentRequest {
    Hello {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        client: Option<ClientInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        distro: Option<String>,
    },
    Status,
    Show(ShowArgs),
    Workspace(WorkspaceArgs),
    Highlight(HighlightArgs),
    Read(ReadArgs),
    Comment(CommentArgs),
    Comments(CommentsArgs),
    Reply(ReplyArgs),
    Resolve(ResolveArgs),
}

impl AgentRequest {
    /// Whether the request keeps to its tool's schema and limits, as `tools/call` checks the
    /// arguments `lectern.exe mcp` builds it from ([`parse_call`]): Lectern's pipe checks again
    /// what reaches it, as any of the user's processes may write to the pipe. A `hello` is the
    /// connection's own, whose distribution `Agent::hello` checks. The error is a sentence for the
    /// agent.
    pub fn check(&self) -> Result<(), String> {
        if matches!(self, Self::Hello { .. }) {
            return Ok(());
        }
        // The request as the tool's arguments: its fields, beside its tool's name.
        let Ok(Value::Object(mut arguments)) = serde_json::to_value(self) else {
            return Err("Lectern couldn't read the request.".to_owned());
        };
        let tool = match arguments.remove("tool") {
            Some(Value::String(tool)) => tool,
            _ => return Err("Lectern couldn't read the request.".to_owned()),
        };
        parse_call(&tool, &arguments)
            .map(drop)
            .map_err(|e| match e {
                CallError::Invalid(message) => message,
                CallError::UnknownTool(tool) => format!("Lectern has no tool {tool}."),
            })
    }
}

/// The MCP client, as its `clientInfo` names it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[ts(export)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// `show`: open `path` (a Windows or WSL path, mapped by Lectern), in the window of `workspace`
/// when one is named, at `target`, in a new window when `newWindow`, waiting up to `wait`
/// seconds for the reader's answer when it asks first.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShowArgs {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<Target>,
    #[serde(default)]
    pub new_window: bool,
    /// Seconds, 0 to 60.
    #[serde(default)]
    pub wait: u32,
}

/// `workspace`: switch to the workspace `name`, or create it with `folders` as its roots.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkspaceArgs {
    pub action: WorkspaceAction,
    pub name: String,
    /// Windows or WSL paths; only for `create`.
    #[serde(default)]
    pub folders: Vec<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum WorkspaceAction {
    Switch,
    Create,
}

/// `highlight`: paint `targets` in the file shown (`path`, the shown file by default), after
/// clearing the agent's earlier highlights when `clear`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HighlightArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub clear: bool,
}

/// `read`: the rendered text of the file shown (`path`, the shown file by default), from the
/// character offset `from`, at most `max` characters.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReadArgs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max: Option<u32>,
}

/// `comment`: start a thread on `target` in `path`, its first entry `text`, of `kind`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommentArgs {
    pub path: String,
    pub target: Target,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<EntryKind>,
}

/// `comments`: the threads of `path` with `status`, from the thread at `from` in that list.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommentsArgs {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<ThreadFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub from: Option<u32>,
}

/// `reply`: add `text`, of `kind`, to the thread `id` of `path`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplyArgs {
    pub path: String,
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<EntryKind>,
}

/// `resolve`: resolve the thread `id` of `path`, with `text` as the closing note.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolveArgs {
    pub path: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<String>,
}

/// Where in a file: a heading (its text, or an `A › B` path), a 1-based source line, a quoted
/// passage with the text just before and after it, a PDF page, a comment thread's id, or a heading
/// by the `#anchor` after the file's path, as Lectern's links name one. An anchor is Lectern's
/// own, an HTML page's found by its window, a note's by core: an agent's target is never one.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(untagged)]
#[ts(export)]
pub enum Target {
    Heading {
        heading: String,
    },
    Line {
        line: u32,
    },
    Quote {
        quote: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        prefix: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        suffix: Option<String>,
    },
    Page {
        page: u32,
    },
    Comment {
        comment: String,
    },
    Anchor {
        anchor: String,
    },
}

/// What an agent's entry in a thread is: `**<Agent> (<kind>):**`.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum EntryKind {
    Question,
    Reply,
    Pushback,
}

/// Which threads `comments` lists.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ThreadFilter {
    Open,
    Question,
    Pushback,
    Resolved,
    All,
}
