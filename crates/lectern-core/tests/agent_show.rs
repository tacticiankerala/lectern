//! Agents showing files and workspaces, in core: the name an agent goes by, where a `show` goes
//! (as tables), how long Lectern has to answer one, what a window's UI answers about it, and what
//! `show` and `workspace` answer.

use std::path::Path;
use std::time::Duration;

use lectern_core::agent::request::{ClientInfo, ShowArgs, Target};
use lectern_core::agent::show::{
    route_show, AgentRoute, FocusStep, ShowAnswer, ShowState, WorkspaceAnswer,
};
use lectern_core::agent::status::{
    window_status, AgentCall, AgentReply, AskAnswer, AskOutcome, WindowView, ANSWERS_KEPT,
};
use lectern_core::agent::{agent_name, answer_within, AgentRequest, ANSWER_WITHIN};
use lectern_core::ipc::PathForms;
use lectern_core::library::pathmap::PathMapper;
use lectern_core::workspace::{OpenWindow, Workspace, Workspaces};
use serde_json::json;

const PLAN: &str = r"S:\Notes\My Vault\plan.md";
const PLAN_WSL: &str = "/mnt/s/Notes/My Vault/plan.md";
const NOTES: &str = r"S:\Notes\My Vault\notes.md";
const NOTES_WSL: &str = "/mnt/s/Notes/My Vault/notes.md";

fn wsl_mapper() -> PathMapper {
    PathMapper {
        mappings: Vec::new(),
        wsl_distro: Some("Ubuntu".to_owned()),
    }
}

// The agent's name.

fn named(client: &str) -> String {
    agent_name(Some(&ClientInfo {
        name: client.to_owned(),
        version: "1.0.0".to_owned(),
    }))
}

#[test]
fn claude_and_codex_go_by_their_own_names() {
    for (client, name) in [
        ("claude-code", "Claude"),
        ("claude", "Claude"),
        ("Claude Desktop", "Claude"),
        ("claude-ai", "Claude"),
        ("codex", "Codex"),
        ("Codex", "Codex"),
    ] {
        assert_eq!(named(client), name, "{client}");
    }
}

#[test]
fn another_agent_goes_by_its_name_cut_to_safe_characters() {
    assert_eq!(named("Gemini CLI"), "Gemini CLI");
    assert_eq!(named("  zed-agent_2.1  "), "zed-agent_2.1");
    assert_eq!(named("codex-cli"), "codex-cli");
    // Only letters, digits, spaces and `._-`: nothing that could style a bar or break a line.
    assert_eq!(named("agent<b>\u{202e}one\n"), "agentbone");
    assert_eq!(named("Ünïcode Agent"), "ncode Agent");
    // At most 32 characters, trimmed again where the cut falls.
    let long = format!("{} {}", "a".repeat(31), "b".repeat(10));
    assert_eq!(named(&long), "a".repeat(31));
    assert_eq!(named(&"x".repeat(40)), "x".repeat(32));
}

#[test]
fn an_agent_with_no_usable_name_is_an_agent() {
    assert_eq!(agent_name(None), "An agent");
    for client in ["", "   ", "***", "\u{1F916}"] {
        assert_eq!(named(client), "An agent", "{client:?}");
    }
    // "You" is the reader's name in a review sidecar, never an agent's.
    for client in ["You", "you", " YOU "] {
        assert_eq!(named(client), "An agent", "{client:?}");
    }
}

// How long Lectern has.

fn show(wait: u32) -> AgentRequest {
    AgentRequest::Show(ShowArgs {
        path: PLAN.to_owned(),
        workspace: None,
        target: None,
        new_window: false,
        wait,
    })
}

#[test]
fn a_show_that_waits_for_the_reader_has_that_much_longer() {
    assert_eq!(answer_within(&AgentRequest::Status), ANSWER_WITHIN);
    assert_eq!(answer_within(&show(0)), ANSWER_WITHIN);
    assert_eq!(
        answer_within(&show(60)),
        ANSWER_WITHIN + Duration::from_secs(60)
    );
}

// Where a show goes.

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|&s| s.to_owned()).collect()
}

fn window(label: &str, workspace: Option<&str>, roots: &[&str]) -> OpenWindow {
    OpenWindow {
        label: label.to_owned(),
        workspace: workspace.map(str::to_owned),
        roots: strings(roots),
    }
}

fn workspace(id: &str, name: &str, roots: &[&str], open: bool) -> Workspace {
    Workspace {
        id: id.to_owned(),
        name: name.to_owned(),
        roots: strings(roots),
        open,
        ..Workspace::default()
    }
}

/// Studio (w1) shown in `main` over the vault; Garden (w2) and Archive (w3) closed, Garden
/// focused more recently; `win-2` blank and the most recently focused window.
fn studio_garden_archive() -> (Vec<OpenWindow>, Workspaces, Vec<String>) {
    let windows = vec![
        window("main", Some("w1"), &[r"S:\Notes\My Vault"]),
        window("win-2", None, &[]),
    ];
    let workspaces = Workspaces {
        items: vec![
            workspace("w1", "Studio", &[r"S:\Notes\My Vault"], true),
            workspace("w2", "Garden", &[r"S:\Notes\Garden"], false),
            workspace("w3", "Archive", &[r"D:\Archive"], false),
        ],
        focus: strings(&["w1", "w2", "w3"]),
        next_id: 4,
        ..Workspaces::default()
    };
    (windows, workspaces, strings(&["win-2", "main"]))
}

fn to(label: &str) -> AgentRoute {
    AgentRoute::Window(label.to_owned())
}

fn reopen(id: &str) -> AgentRoute {
    AgentRoute::Reopen(id.to_owned())
}

#[test]
fn a_show_without_a_workspace_goes_as_a_launch_does() {
    let (windows, workspaces, focus) = studio_garden_archive();
    let table: [(&str, bool, AgentRoute); 6] = [
        // The window whose libraries hold it; a new window isn't made for a workspace shown.
        (r"S:\Notes\My Vault\plan.md", false, to("main")),
        (r"s:/notes/my vault/plan.md", true, to("main")),
        // A closed workspace that holds it is reopened, in a new window either way.
        (r"S:\Notes\Garden\seeds.md", false, reopen("w2")),
        (r"S:\Notes\Garden\seeds.md", true, reopen("w2")),
        // Held by none: the most recently focused window, or a new blank one when asked.
        (r"D:\Elsewhere\a.md", false, to("win-2")),
        (r"D:\Elsewhere\a.md", true, AgentRoute::NewWindow),
    ];
    for (path, new_window, route) in table {
        assert_eq!(
            route_show(
                Path::new(path),
                None,
                new_window,
                &windows,
                &workspaces,
                &focus
            ),
            Ok(route),
            "{path}, new window: {new_window}"
        );
    }
}

#[test]
fn a_show_in_a_named_workspace_goes_to_its_window_or_reopens_it() {
    let (windows, workspaces, focus) = studio_garden_archive();
    let table: [(&str, &str, bool, AgentRoute); 5] = [
        // Named, in any letter case, with the spaces around it ignored.
        (r"S:\Notes\Garden\seeds.md", "STUDIO", false, to("main")),
        (r"S:\Notes\Garden\seeds.md", " Studio ", true, to("main")),
        // Closed: reopened in a new window, wherever the file is.
        (r"S:\Notes\My Vault\plan.md", "garden", false, reopen("w2")),
        (r"D:\Elsewhere\a.md", "Archive", false, reopen("w3")),
        (r"D:\Elsewhere\a.md", "Archive", true, reopen("w3")),
    ];
    for (path, name, new_window, route) in table {
        assert_eq!(
            route_show(
                Path::new(path),
                Some(name),
                new_window,
                &windows,
                &workspaces,
                &focus
            ),
            Ok(route),
            "{path} in {name}, new window: {new_window}"
        );
    }
}

#[test]
fn a_workspace_that_does_not_exist_is_an_error_naming_those_that_do() {
    let (windows, workspaces, focus) = studio_garden_archive();
    assert_eq!(
        route_show(
            Path::new(PLAN),
            Some("Nowhere"),
            false,
            &windows,
            &workspaces,
            &focus
        ),
        Err(
            "There's no workspace named “Nowhere”. The workspaces are Studio, Garden and Archive."
                .to_owned()
        )
    );
    let one = Workspaces {
        items: vec![workspace("w1", "Main", &[], true)],
        focus: strings(&["w1"]),
        ..Workspaces::default()
    };
    assert_eq!(
        route_show(
            Path::new(PLAN),
            Some("Garden"),
            false,
            &[window("main", Some("w1"), &[])],
            &one,
            &strings(&["main"])
        ),
        Err("There's no workspace named “Garden”. The only workspace is Main.".to_owned())
    );
}

#[test]
fn a_workspace_is_found_by_its_name_as_names_are_kept() {
    let (_, workspaces, _) = studio_garden_archive();
    assert_eq!(
        workspaces.find_by_name("  garden ").map(|w| w.id.as_str()),
        Some("w2")
    );
    assert!(workspaces.find_by_name("Gard").is_none());
    assert!(workspaces.find_by_name("   ").is_none());
    // A name is kept to 60 characters, so a longer one finds the workspace it was cut to.
    let long = Workspaces {
        items: vec![workspace("w1", &"n".repeat(60), &[], true)],
        ..Workspaces::default()
    };
    assert!(long.find_by_name(&"N".repeat(70)).is_some());
}

// What a window's UI hears and answers.

#[test]
fn a_window_is_asked_to_show_with_the_agents_name() {
    let call = AgentCall {
        id: 4,
        request: AgentRequest::Show(ShowArgs {
            path: PLAN.to_owned(),
            workspace: None,
            target: Some(Target::Line { line: 12 }),
            new_window: false,
            wait: 0,
        }),
        agent: "Claude".to_owned(),
    };
    assert_eq!(
        serde_json::to_value(&call).unwrap(),
        json!({
            "id": 4,
            "request": {
                "tool": "show",
                "path": PLAN,
                "target": {"line": 12},
                "newWindow": false,
                "wait": 0,
            },
            "agent": "Claude",
        })
    );
}

#[test]
fn a_window_answers_a_show_shown_or_asked_then_with_the_readers_answer() {
    let answered = |answer| AgentReply::Answered {
        answer,
        message: None,
    };
    for (json, reply) in [
        (json!({"kind": "shown"}), AgentReply::Shown),
        (json!({"kind": "asked"}), AgentReply::Asked),
        (
            json!({"kind": "answered", "answer": "shown"}),
            answered(AskAnswer::Shown),
        ),
        (
            json!({"kind": "answered", "answer": "dismissed"}),
            answered(AskAnswer::Dismissed),
        ),
        (
            json!({"kind": "answered", "answer": "replaced"}),
            answered(AskAnswer::Replaced),
        ),
        // The reader chose Show, and the file didn't open: why, for the agent.
        (
            json!({
                "kind": "answered",
                "answer": "failed",
                "message": "plan.md didn't open: It's gone.",
            }),
            AgentReply::Answered {
                answer: AskAnswer::Failed,
                message: Some("plan.md didn't open: It's gone.".to_owned()),
            },
        ),
    ] {
        assert_eq!(serde_json::to_value(&reply).unwrap(), json);
        assert_eq!(serde_json::from_value::<AgentReply>(json).unwrap(), reply);
    }
}

#[test]
fn a_windows_view_names_its_waiting_request_and_the_answers_so_far() {
    let view: WindowView = serde_json::from_value(json!({
        "path": PLAN,
        "heading": [],
        "line": null,
        "selection": null,
        "ask": NOTES,
        "answers": [
            {"path": PLAN, "answer": "replaced"},
            {"path": PLAN, "answer": "failed", "message": "plan.md didn't open: It's gone."},
        ],
    }))
    .unwrap();
    assert_eq!(view.ask.as_deref(), Some(NOTES));
    assert_eq!(
        view.answers,
        [
            AskOutcome {
                path: PLAN.to_owned(),
                answer: AskAnswer::Replaced,
                message: None,
            },
            AskOutcome {
                path: PLAN.to_owned(),
                answer: AskAnswer::Failed,
                message: Some("plan.md didn't open: It's gone.".to_owned()),
            }
        ]
    );
    let status = window_status("main", Some("Studio".to_owned()), view, &wsl_mapper());
    let status = serde_json::to_value(&status).unwrap();
    assert_eq!(status["ask"], json!({"windows": NOTES, "wsl": NOTES_WSL}));
    let plan = json!({"windows": PLAN, "wsl": PLAN_WSL});
    assert_eq!(
        status["answers"],
        json!([
            {"path": plan, "answer": "replaced"},
            {"path": plan, "answer": "failed", "message": "plan.md didn't open: It's gone."},
        ])
    );
}

#[test]
fn a_view_without_asks_has_none_and_status_keeps_the_latest_answers() {
    let view: WindowView = serde_json::from_value(json!({
        "path": null, "heading": [], "line": null, "selection": null,
    }))
    .unwrap();
    assert_eq!((view.ask.clone(), view.answers.len()), (None, 0));
    let answers: Vec<AskOutcome> = (1..=ANSWERS_KEPT + 3)
        .map(|n| AskOutcome {
            path: format!(r"S:\Notes\My Vault\n{n}.md"),
            answer: AskAnswer::Dismissed,
            message: None,
        })
        .collect();
    let status = window_status(
        "main",
        None,
        WindowView {
            answers,
            ..WindowView::default()
        },
        &wsl_mapper(),
    );
    assert_eq!(status.answers.len(), ANSWERS_KEPT);
    assert_eq!(
        status.answers[0].path.windows.as_deref(),
        Some(r"S:\Notes\My Vault\n4.md")
    );
}

// What show and workspace answer.

#[test]
fn show_answers_its_state_the_file_in_both_forms_the_window_and_the_focus() {
    let answer = ShowAnswer {
        state: ShowState::Asked,
        file: PathForms {
            windows: Some(PLAN.to_owned()),
            wsl: Some(PLAN_WSL.to_owned()),
        },
        window: "main".to_owned(),
        workspace: Some("Studio".to_owned()),
        focus: Some(FocusStep::Foreground),
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({
            "state": "asked",
            "file": {"windows": PLAN, "wsl": PLAN_WSL},
            "window": "main",
            "workspace": "Studio",
            "focus": "foreground",
        })
    );
    for (answer, state) in [
        (AskAnswer::Shown, "shown"),
        (AskAnswer::Dismissed, "dismissed"),
        (AskAnswer::Replaced, "replaced"),
    ] {
        let settled = ShowState::settled(answer, None).unwrap();
        assert_eq!(serde_json::to_value(settled).unwrap(), json!(state));
    }
    assert_eq!(
        serde_json::to_value(ShowState::Waiting).unwrap(),
        json!("waiting")
    );
    // An open the reader chose that failed is the tool's error: why, as the window said.
    let why = "plan.md didn't open: It's gone.";
    assert_eq!(
        ShowState::settled(AskAnswer::Failed, Some(why.to_owned())),
        Err(why.to_owned())
    );
    assert_eq!(
        ShowState::settled(AskAnswer::Failed, None),
        Err("The file didn't open.".to_owned())
    );
}

#[test]
fn workspace_answers_the_workspace_its_window_and_the_focus() {
    let answer = WorkspaceAnswer {
        workspace: "Garden".to_owned(),
        window: "win-3".to_owned(),
        opened: true,
        focus: Some(FocusStep::Raised),
    };
    assert_eq!(
        serde_json::to_value(&answer).unwrap(),
        json!({"workspace": "Garden", "window": "win-3", "opened": true, "focus": "raised"})
    );
    assert_eq!(
        serde_json::to_value(FocusStep::Flashed).unwrap(),
        json!("flashed")
    );
}
