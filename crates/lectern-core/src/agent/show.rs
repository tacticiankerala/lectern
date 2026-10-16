//! `show` and `workspace`: which window an agent's `show` goes to ([`route_show`]), and what the
//! two tools answer ([`ShowAnswer`], [`WorkspaceAnswer`]).
//!
//! A workspace is shown in at most one window, and an agent never changes the workspace a window
//! shows: a closed workspace is reopened in a new window instead.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::status::AskAnswer;
use super::tools::listed;
use crate::ipc::PathForms;
use crate::library::{key_under, path_key};
use crate::workspace::{route_open, OpenWindow, Route, Workspaces};

/// Where an agent's `show` goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentRoute {
    /// To the window with this label.
    Window(String),
    /// To this closed workspace, reopened in a new window.
    Reopen(String),
    /// To a new blank window.
    NewWindow,
}

/// Where an agent's `show` of the file `path` goes, in the workspace named `named` when there is
/// one, and in a new window when `new_window`.
///
/// - **A named workspace** (its name in any letter case): its window, or a new one reopening it
///   when it is closed, wherever the file is. A name no workspace has is an error naming the
///   workspaces there are.
/// - **Otherwise,** as a launch goes (`route_open`): the window whose libraries hold the file,
///   else the closed workspace that holds it, reopened, else the most recently focused window.
///   With `new_window`, a file no open window's libraries hold goes to a new blank window instead
///   of the most recently focused one.
///
/// `windows` are the open windows, `workspaces` every workspace, and `focus` the windows' labels,
/// the most recently focused first.
pub fn route_show(
    path: &Path,
    named: Option<&str>,
    new_window: bool,
    windows: &[OpenWindow],
    workspaces: &Workspaces,
    focus: &[String],
) -> Result<AgentRoute, String> {
    if let Some(name) = named {
        let ws = workspaces
            .find_by_name(name)
            .ok_or_else(|| no_such_workspace(name, workspaces))?;
        return Ok(
            match windows
                .iter()
                .find(|w| w.workspace.as_deref() == Some(&ws.id))
            {
                Some(window) => AgentRoute::Window(window.label.clone()),
                None => AgentRoute::Reopen(ws.id.clone()),
            },
        );
    }
    let closed: Vec<(String, Vec<String>)> = workspaces
        .focus
        .iter()
        .filter(|id| !windows.iter().any(|w| w.workspace.as_ref() == Some(*id)))
        .filter_map(|id| workspaces.get(id))
        .map(|ws| (ws.id.clone(), ws.roots.clone()))
        .collect();
    Ok(match route_open(path, false, windows, &closed, focus) {
        Route::Window(label) if label.is_empty() => {
            return Err("Lectern has no window open.".to_owned())
        }
        Route::Window(label) => {
            let holds = windows
                .iter()
                .find(|w| w.label == label)
                .is_some_and(|w| holds(&w.roots, path));
            if new_window && !holds {
                AgentRoute::NewWindow
            } else {
                AgentRoute::Window(label)
            }
        }
        Route::Reopen(id) => AgentRoute::Reopen(id),
    })
}

/// Whether one of `roots` holds `path`, compared as Windows compares paths.
fn holds(roots: &[String], path: &Path) -> bool {
    let key = path_key(path);
    roots
        .iter()
        .any(|root| key_under(&key, &path_key(Path::new(root))).is_some())
}

/// The error for a workspace `name` that none is called, naming those there are.
pub fn no_such_workspace(name: &str, workspaces: &Workspaces) -> String {
    let names: Vec<&str> = workspaces.items.iter().map(|w| w.name.as_str()).collect();
    let there = match names[..] {
        [only] => format!("The only workspace is {only}."),
        _ => format!("The workspaces are {}.", listed(&names, "and")),
    };
    format!("There's no workspace named “{}”. {there}", name.trim())
}

/// What became of a `show`: the file is on screen, the reader is asked first, or (when it waited
/// for them) what they answered, or that they haven't yet.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShowState {
    Shown,
    Asked,
    Dismissed,
    Replaced,
    Waiting,
}

impl ShowState {
    /// What a `show` that waited answers once the reader's window settled it as `answer`: an
    /// open the reader chose that failed is the tool's error, why it failed (`message`).
    pub fn settled(answer: AskAnswer, message: Option<String>) -> Result<Self, String> {
        match answer {
            AskAnswer::Shown => Ok(Self::Shown),
            AskAnswer::Dismissed => Ok(Self::Dismissed),
            AskAnswer::Replaced => Ok(Self::Replaced),
            AskAnswer::Failed => Err(message.unwrap_or_else(|| "The file didn't open.".to_owned())),
        }
    }
}

/// How a window came forward for an agent (the `agentFocus` setting): it took the foreground, came
/// to the top without the focus, or only flashed on the taskbar.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FocusStep {
    Foreground,
    Raised,
    Flashed,
}

/// `show`'s answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShowAnswer {
    pub state: ShowState,
    /// The file, in both forms.
    pub file: PathForms,
    /// The window's label: `main` or `win-<n>`.
    pub window: String,
    /// The workspace the window shows; none for a blank window.
    pub workspace: Option<String>,
    /// How the window came forward; none when it hadn't shown yet.
    pub focus: Option<FocusStep>,
}

/// `workspace`'s answer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceAnswer {
    /// The workspace's name.
    pub workspace: String,
    /// The label of the window showing it.
    pub window: String,
    /// It was closed (or new) and opened in a new window.
    pub opened: bool,
    /// How the window came forward; none when it hadn't shown yet.
    pub focus: Option<FocusStep>,
}
