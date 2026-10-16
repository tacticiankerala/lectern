//! A window's UI's part in answering an agent ([`AgentCall`] → [`AgentReply`]), and what `status`
//! answers: each window as its UI reports it, with both forms of its file's path, the request it
//! is asking the reader about and what became of the earlier ones.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::request::AgentRequest;
use super::show::TargetState;
use super::target::TextPlace;
use crate::ipc::{AgentFocus, AgentOpens, PathForms};
use crate::library::pathmap::PathMapper;
use crate::library::{doc_kind, DocKind};
use crate::review::text::PageBlock;

/// The most characters of the reader's selection `status` gives.
pub const SELECTION_MAX: usize = 2000;

/// How the top-visible heading's path is written: `Plan › Rollout`, as a heading target takes it.
pub const HEADING_SEPARATOR: &str = " › ";

/// How many of the reader's answers to an agent's requests a window keeps for `status`, the latest.
pub const ANSWERS_KEPT: usize = 10;

/// Event payload (`agent-request`): a request for a window's UI. It answers with the command
/// `agent_reply(id, reply)`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentCall {
    pub id: u64,
    pub request: AgentRequest,
    /// The agent's name as the reader sees it ([`super::agent_name`]): "Claude".
    pub agent: String,
    /// In a note or a text file, where each of the request's targets is, as core found it, in
    /// order: `show`'s one, `highlight`'s each. None for any other file, whose window finds them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<TextPlace>>", optional)]
    pub places: Vec<TextPlace>,
    /// How long the window has to answer, in milliseconds, after which Lectern stops waiting: a
    /// `show`'s, whose window may wait for an HTML page to load to find its target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub within_ms: Option<u32>,
}

/// A window's UI's answer to an [`AgentCall`], tagged by `kind`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum AgentReply {
    /// What the window shows (`status`).
    View(WindowView),
    /// `show`: the file is on screen, opened or there already, at its target; or, with `target`
    /// `pending`, an HTML page that hasn't loaded yet, which goes to it once it has.
    Shown {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        target: Option<TargetState>,
    },
    /// `show`: the window shows another file, so it asks the reader first.
    Asked,
    /// `show`: a newer request took this one's place before its HTML page loaded, so the window
    /// doesn't go to its target.
    Replaced,
    /// What became of a `show` the window asked the reader about, sent once it is settled: the
    /// reader's answer, or replaced by a newer request. For an open that failed, why (`message`),
    /// a sentence for the agent.
    Answered {
        answer: AskAnswer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        message: Option<String>,
    },
    /// `highlight`: how many of the agent's highlights the window shows now.
    Highlighted { count: u32 },
    /// `read` of an HTML page: its blocks, as its bridge last reported them, and whether it stopped
    /// them short, at its limit on the text it reports.
    PageText {
        blocks: Vec<PageBlock>,
        #[serde(default, skip_serializing_if = "is_false")]
        #[ts(as = "Option<bool>", optional)]
        truncated: bool,
    },
    /// What was asked couldn't be done: a sentence for the agent.
    Failed { message: String },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// What became of a request to show a file that the window asked the reader about: they showed
/// it or said not now, or a newer request took its place. `failed`: they chose to show it, and it
/// didn't open.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AskAnswer {
    Shown,
    Dismissed,
    Replaced,
    Failed,
}

/// A request the window asked the reader about, and what became of it.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AskOutcome {
    /// The file the agent asked to show.
    pub path: String,
    pub answer: AskAnswer,
    /// Why it didn't open, when it `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
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
    /// The file an agent asked to show that the reader hasn't answered about yet.
    #[serde(default)]
    pub ask: Option<String>,
    /// What became of the earlier requests the window asked about, the oldest first.
    #[serde(default)]
    pub answers: Vec<AskOutcome>,
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
    /// What became of the earlier requests asked about: the latest [`ANSWERS_KEPT`], the oldest
    /// first.
    pub answers: Vec<AnsweredAsk>,
}

/// A request the window asked the reader about, as `status` gives it: the file, in both forms,
/// and what became of it, with why when it `failed`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AnsweredAsk {
    pub path: PathForms,
    pub answer: AskAnswer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
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
/// [`SELECTION_MAX`] characters, and the answers to the latest [`ANSWERS_KEPT`].
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
        ask,
        answers,
    } = view;
    let skipped = answers.len().saturating_sub(ANSWERS_KEPT);
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
        ask: ask.map(|path| mapper.literal_forms(&path)),
        answers: answers
            .into_iter()
            .skip(skipped)
            .map(|outcome| AnsweredAsk {
                path: mapper.literal_forms(&outcome.path),
                answer: outcome.answer,
                message: outcome.message,
            })
            .collect(),
    }
}
