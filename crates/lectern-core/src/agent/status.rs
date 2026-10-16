//! A window's UI's part in answering an agent ([`AgentCall`] → [`AgentReply`]), and what `status`
//! answers: each window as its UI reports it, with both forms of its file's path.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::request::AgentRequest;
use crate::ipc::{AgentFocus, AgentOpens, PathForms};
use crate::library::pathmap::PathMapper;
use crate::library::{doc_kind, DocKind};

/// The most characters of the reader's selection `status` gives.
pub const SELECTION_MAX: usize = 2000;

/// How the top-visible heading's path is written: `Plan › Rollout`, as a heading target takes it.
pub const HEADING_SEPARATOR: &str = " › ";

/// Event payload (`agent-request`): a request for a window's UI. It answers with the command
/// `agent_reply(id, reply)`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentCall {
    pub id: u64,
    pub request: AgentRequest,
}

/// A window's UI's answer to an [`AgentCall`], tagged by `kind`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentReply {
    /// What the window shows (`status`).
    View(WindowView),
    /// What was asked couldn't be done: a sentence for the agent.
    Failed { message: String },
}

/// What a window shows, as its UI sees it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowView {
    /// The file on screen, as Lectern opened it; none for a blank window, the welcome page or a
    /// file that couldn't open.
    pub path: Option<String>,
    /// The heading at the top of the view, after the headings it sits under, outermost first;
    /// empty above the first heading.
    pub heading: Vec<String>,
    /// The source line at the top of the view: notes and text files only.
    pub line: Option<u32>,
    pub selection: Option<ViewSelection>,
}

/// The text the reader has selected, and its source lines (notes and text files only).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ViewSelection {
    pub quote: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

/// `status`'s answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Every window, the most recently focused first.
    pub windows: Vec<WindowStatus>,
    /// The workspaces no window shows, by name.
    pub closed_workspaces: Vec<String>,
    pub settings: AgentSettings,
}

/// One window, as `status` gives it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowStatus {
    /// `main` or `win-<n>`.
    pub label: String,
    /// The workspace it shows; none for a blank window.
    pub workspace: Option<String>,
    /// The file it shows, in both forms.
    pub file: Option<PathForms>,
    pub kind: Option<DocKind>,
    /// The heading at the top of the view, as a path: `Plan › Rollout`.
    pub heading: Option<String>,
    pub line: Option<u32>,
    pub selection: Option<ViewSelection>,
    /// The file an agent asked to show that the reader hasn't answered yet.
    pub ask: Option<PathForms>,
}

/// The settings that say what agents may do.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentSettings {
    pub agent_opens: AgentOpens,
    pub agent_focus: AgentFocus,
}

/// The window `label`, showing the workspace `workspace`, as `status` gives it from what its UI
/// reported (`view`). Its file's kind comes from its name; its paths are given in both forms,
/// taken as they are (`PathMapper::literal_forms`). The selection is cut to its first
/// [`SELECTION_MAX`] characters.
pub fn window_status(
    label: &str,
    workspace: Option<String>,
    view: WindowView,
    mapper: &PathMapper,
) -> WindowStatus {
    let WindowView {
        path,
        heading,
        line,
        selection,
    } = view;
    WindowStatus {
        label: label.to_owned(),
        workspace,
        kind: path.as_deref().and_then(doc_kind),
        file: path.map(|path| mapper.literal_forms(&path)),
        heading: (!heading.is_empty()).then(|| heading.join(HEADING_SEPARATOR)),
        line,
        selection: selection.map(|s| ViewSelection {
            quote: s.quote.chars().take(SELECTION_MAX).collect(),
            ..s
        }),
        ask: None,
    }
}
