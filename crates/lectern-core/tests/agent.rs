//! Agent control in core: the pipe's name and messages, the capped line reader, agents' paths,
//! the tools and their arguments, `status`'s windows, and MCP over stdio in both eras.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use lectern_core::agent::lines::{Line, LineReader};
use lectern_core::agent::mcp::{code, Handled, Mcp, INSTRUCTIONS, LEGACY, MODERN};
use lectern_core::agent::paths::{agent_path, AgentPath};
use lectern_core::agent::request::{
    ClientInfo, CommentArgs, CommentsArgs, EntryKind, HighlightArgs, ReadArgs, ReplyArgs,
    ResolveArgs, ShowArgs, Target, ThreadFilter, WorkspaceAction, WorkspaceArgs,
};
use lectern_core::agent::status::{
    window_status, AgentCall, AgentReply, ViewSelection, WindowView, SELECTION_MAX,
};
use lectern_core::agent::tools::{definitions, parse_call, CallError};
use lectern_core::agent::{
    pipe_name, printable, same_user, AgentRequest, Answer, PipeReply, PipeRequest, FOREIGN_PIPE,
};
use lectern_core::ipc::PathForms;
use lectern_core::library::pathmap::PathMapper;
use lectern_core::library::DocKind;
use serde_json::{json, Map, Value};

const PLAN: &str = r"S:\Notes\My Vault\plan.md";
const PLAN_WSL: &str = "/mnt/s/Notes/My Vault/plan.md";

fn version() -> &'static str {
    lectern_core::version()
}

// The pipe.

#[test]
fn the_pipe_is_named_for_the_app_and_the_user() {
    let name = pipe_name("io.github.example.lectern", "S-1-5-21-1000-2000-3000-1001");
    let hash = name
        .strip_prefix(r"\\.\pipe\lectern-io.github.example.lectern-")
        .unwrap_or_else(|| panic!("{name}"));
    assert_eq!(hash.len(), 16, "{name}");
    assert!(
        hash.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "{name}"
    );
    // The start of the SHA-256 of the SID as text.
    assert_eq!(
        pipe_name("x", "S-1-5-21-1000-2000-3000-1001"),
        pipe_name("x", "S-1-5-21-1000-2000-3000-1001")
    );
    assert_ne!(
        pipe_name("x", "S-1-5-21-1000-2000-3000-1001"),
        pipe_name("x", "S-1-5-21-1000-2000-3000-1002")
    );
    assert_ne!(
        pipe_name("x", "S-1-5-21-1000-2000-3000-1001"),
        pipe_name("y", "S-1-5-21-1000-2000-3000-1001")
    );
    // sha256("abc") starts ba7816bf8f01cfea.
    assert_eq!(
        pipe_name("x", "abc"),
        r"\\.\pipe\lectern-x-ba7816bf8f01cfea"
    );
}

#[test]
fn only_a_server_of_the_same_user_is_talked_to() {
    let ours = "S-1-5-21-1000-2000-3000-1001";
    assert!(same_user(ours, "S-1-5-21-1000-2000-3000-1001"));
    // SIDs as text compare as Windows parses them, in any letter case.
    assert!(same_user(ours, "s-1-5-21-1000-2000-3000-1001"));
    for other in [
        "S-1-5-21-1000-2000-3000-1002",
        "S-1-5-21-1000-2000-3000-10011",
        "S-1-5-21-1000-2000-3000-100",
        "S-1-5-18",
        "",
    ] {
        assert!(!same_user(ours, other), "{other}");
    }
    assert!(!same_user("", ""));
    assert_eq!(
        FOREIGN_PIPE,
        "Lectern's pipe is owned by another user; refusing to connect."
    );
}

#[test]
fn a_client_name_is_logged_without_control_characters() {
    assert_eq!(printable("claude-code"), "claude-code");
    assert_eq!(
        printable("claude\ncode\r\u{1b}[31m\u{7}\u{85}\t2.1"),
        "claudecode[31m2.1"
    );
    assert_eq!(printable("Ünïcode ok"), "Ünïcode ok");
}

#[test]
fn pipe_messages_are_id_and_request_then_id_and_ok_or_err() {
    let request = PipeRequest {
        id: 7,
        request: AgentRequest::Status,
    };
    let line = serde_json::to_string(&request).unwrap();
    assert_eq!(line, r#"{"id":7,"request":{"tool":"status"}}"#);
    assert_eq!(serde_json::from_str::<PipeRequest>(&line).unwrap(), request);

    let ok = PipeReply {
        id: 7,
        answer: Answer::Ok(json!({"windows": []})),
    };
    let line = serde_json::to_string(&ok).unwrap();
    assert_eq!(line, r#"{"id":7,"ok":{"windows":[]}}"#);
    assert_eq!(serde_json::from_str::<PipeReply>(&line).unwrap(), ok);

    let err = PipeReply {
        id: 8,
        answer: Answer::Err("Not yet available.".to_owned()),
    };
    let line = serde_json::to_string(&err).unwrap();
    assert_eq!(line, r#"{"id":8,"err":"Not yet available."}"#);
    assert_eq!(serde_json::from_str::<PipeReply>(&line).unwrap(), err);
}

#[test]
fn hello_names_the_client() {
    let hello = AgentRequest::Hello {
        client: ClientInfo {
            name: "claude-code".to_owned(),
            version: "2.1.0".to_owned(),
        },
    };
    assert_eq!(
        serde_json::to_value(&hello).unwrap(),
        json!({"tool": "hello", "client": {"name": "claude-code", "version": "2.1.0"}})
    );
}

#[test]
fn requests_are_tagged_by_tool_with_their_arguments_beside() {
    let show = AgentRequest::Show(ShowArgs {
        path: PLAN_WSL.to_owned(),
        workspace: None,
        target: Some(Target::Heading {
            heading: "Plan › Rollout".to_owned(),
        }),
        new_window: false,
        wait: 5,
    });
    let value = serde_json::to_value(&show).unwrap();
    assert_eq!(
        value,
        json!({
            "tool": "show",
            "path": PLAN_WSL,
            "target": {"heading": "Plan › Rollout"},
            "newWindow": false,
            "wait": 5,
        })
    );
    assert_eq!(serde_json::from_value::<AgentRequest>(value).unwrap(), show);
    for target in [
        Target::Line { line: 12 },
        Target::Quote {
            quote: "roll back".to_owned(),
            prefix: Some("we ".to_owned()),
            suffix: None,
        },
        Target::Page { page: 3 },
        Target::Comment {
            comment: "C3".to_owned(),
        },
    ] {
        let value = serde_json::to_value(&target).unwrap();
        assert_eq!(serde_json::from_value::<Target>(value).unwrap(), target);
    }
}

#[test]
fn a_window_is_asked_with_an_id_and_answers_by_kind() {
    let call = AgentCall {
        id: 3,
        request: AgentRequest::Status,
        agent: "Claude".to_owned(),
        places: Vec::new(),
        within_ms: None,
    };
    assert_eq!(
        serde_json::to_value(&call).unwrap(),
        json!({"id": 3, "request": {"tool": "status"}, "agent": "Claude"})
    );
    let view: AgentReply = serde_json::from_value(json!({
        "kind": "view",
        "path": PLAN,
        "heading": ["Plan", "Rollout"],
        "line": 12,
        "selection": {"quote": "roll back", "startLine": 14, "endLine": 14},
    }))
    .unwrap();
    assert_eq!(
        view,
        AgentReply::View(WindowView {
            path: Some(PLAN.to_owned()),
            heading: vec!["Plan".to_owned(), "Rollout".to_owned()],
            line: Some(12),
            selection: Some(ViewSelection {
                quote: "roll back".to_owned(),
                start_line: Some(14),
                end_line: Some(14),
            }),
            ..WindowView::default()
        })
    );
    let failed: AgentReply =
        serde_json::from_value(json!({"kind": "failed", "message": "Show it first."})).unwrap();
    assert_eq!(
        failed,
        AgentReply::Failed {
            message: "Show it first.".to_owned()
        }
    );
}

// Lines.

/// A stream that hands out at most `step` bytes per read, to split lines across reads.
struct Trickle<'a> {
    data: &'a [u8],
    step: usize,
}

impl Read for Trickle<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.step.min(buf.len()).min(self.data.len());
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

fn lines_of(data: &[u8], cap: usize, step: usize) -> Vec<Line> {
    let mut reader = LineReader::new(Trickle { data, step }, cap);
    let mut out = Vec::new();
    loop {
        let line = reader.next_line().unwrap();
        let end = line == Line::End;
        out.push(line);
        if end {
            return out;
        }
    }
}

fn text(s: &str) -> Line {
    Line::Text(s.to_owned())
}

#[test]
fn lines_end_at_newlines_with_or_without_a_carriage_return() {
    for step in [1, 3, 4096] {
        assert_eq!(
            lines_of(b"one\ntwo\r\n\nthree", 64, step),
            vec![text("one"), text("two"), text(""), text("three"), Line::End],
            "step {step}"
        );
    }
    assert_eq!(lines_of(b"", 64, 8), vec![Line::End]);
    assert_eq!(lines_of(b"a\n", 64, 8), vec![text("a"), Line::End]);
}

#[test]
fn a_line_over_the_cap_is_skipped_and_the_next_one_read() {
    let mut data = vec![b'x'; 100];
    data.extend_from_slice(b"\nnext\n");
    for step in [1, 7, 4096] {
        assert_eq!(
            lines_of(&data, 64, step),
            vec![Line::TooLong, text("next"), Line::End],
            "step {step}"
        );
    }
    // Exactly the cap fits; an unfinished line over it at the end is too long too.
    assert_eq!(
        lines_of(b"abcd\nabcde", 4, 2),
        vec![text("abcd"), Line::TooLong, Line::End]
    );
}

#[test]
fn bytes_that_are_not_utf8_are_replaced() {
    assert_eq!(
        lines_of(b"a\xffb\n", 64, 64),
        vec![text("a\u{fffd}b"), Line::End]
    );
}

// Paths.

fn wsl_mapper() -> PathMapper {
    PathMapper {
        mappings: Vec::new(),
        wsl_distro: Some("Ubuntu".to_owned()),
    }
}

/// The hosts an agent may reach in these tests: a library's share, and WSL's.
fn reachable() -> Vec<String> {
    vec!["nas".to_owned(), "wsl.localhost".to_owned()]
}

fn mapped(raw: &str) -> AgentPath {
    agent_path(&wsl_mapper(), raw, &reachable()).unwrap_or_else(|e| panic!("{raw}: {e}"))
}

#[test]
fn a_wsl_path_opens_as_its_windows_form() {
    let path = mapped(PLAN_WSL);
    assert_eq!(path.windows, PLAN);
    assert_eq!(path.wsl.as_deref(), Some(PLAN_WSL));
    assert_eq!((path.line, path.anchor.as_deref()), (None, None));
    assert_eq!(
        path.forms(),
        PathForms {
            windows: Some(PLAN.to_owned()),
            wsl: Some(PLAN_WSL.to_owned()),
        }
    );
    let home = mapped("/home/me/notes/today.md");
    assert_eq!(
        home.windows,
        r"\\wsl.localhost\Ubuntu\home\me\notes\today.md"
    );
    assert_eq!(home.wsl.as_deref(), Some("/home/me/notes/today.md"));
}

#[test]
fn any_linux_path_goes_into_wsl_not_only_a_system_one() {
    // Not only a path under a Linux system folder such as `/home`: an agent's path is a file.
    let path = mapped("/workspace/notes/plan.md");
    assert_eq!(
        path.windows,
        r"\\wsl.localhost\Ubuntu\workspace\notes\plan.md"
    );
    assert_eq!(path.wsl.as_deref(), Some("/workspace/notes/plan.md"));
    // Without WSL it has no Windows form, rather than a path from the current drive's root.
    let err = agent_path(
        &PathMapper::default(),
        "/workspace/notes/plan.md",
        &reachable(),
    )
    .unwrap_err();
    assert!(err.contains("/workspace/notes/plan.md"), "{err}");
}

#[test]
fn a_windows_path_stays_as_it_is() {
    let path = mapped(PLAN);
    assert_eq!(path.windows, PLAN);
    assert_eq!(path.wsl.as_deref(), Some(PLAN_WSL));
    let share = mapped(r"\\nas\share\plan.md");
    assert_eq!(share.windows, r"\\nas\share\plan.md");
    assert_eq!(share.wsl, None);
}

#[test]
fn a_line_or_anchor_after_the_path_is_set_apart() {
    let cases = [
        (format!("{PLAN_WSL}:12"), Some(12), None),
        (format!("{PLAN_WSL}:12:5"), Some(12), None),
        (format!("{PLAN}#L7"), Some(7), None),
        (format!("{PLAN}#rollout"), None, Some("rollout")),
        (format!("\"{PLAN_WSL}\""), None, None),
        (format!("`{PLAN}:3`"), Some(3), None),
    ];
    for (raw, line, anchor) in cases {
        let path = mapped(&raw);
        assert_eq!(path.windows, PLAN, "{raw}");
        assert_eq!(path.line, line, "{raw}");
        assert_eq!(path.anchor.as_deref(), anchor, "{raw}");
    }
    // A `#` in a name that isn't after an extension is part of the name.
    let meeting = mapped(r"S:\Notes\Meeting #3.md");
    assert_eq!(meeting.windows, r"S:\Notes\Meeting #3.md");
    assert_eq!(meeting.anchor, None);
}

#[test]
fn a_path_that_is_not_a_full_one_or_not_reachable_is_refused() {
    for raw in ["plan.md", "notes/plan.md", "", "  "] {
        let err = agent_path(&wsl_mapper(), raw, &reachable()).unwrap_err();
        assert!(err.contains("full path"), "{raw}: {err}");
    }
    // Without WSL, a Linux path outside /mnt has no Windows form.
    let err = agent_path(
        &PathMapper::default(),
        "/home/me/notes/today.md",
        &reachable(),
    )
    .unwrap_err();
    assert!(err.contains("/home/me/notes/today.md"), "{err}");
}

#[test]
fn a_device_path_is_refused() {
    for raw in [
        r"\\.\pipe\lectern-x",
        r"\\.\C:\Notes\plan.md",
        r"\\?\S:\Notes\My Vault\plan.md",
        r"\\?\UNC\nas\share\plan.md",
        "//./pipe/lectern-x",
        "//?/S:/Notes/plan.md",
        r"\\./C:/Notes/plan.md",
        r#""\\.\PhysicalDrive0""#,
    ] {
        let err = agent_path(&wsl_mapper(), raw, &reachable()).unwrap_err();
        assert!(err.contains("device path"), "{raw}: {err}");
    }
    // A share is not a device.
    assert_eq!(
        mapped(r"\\nas\share\plan.md").windows,
        r"\\nas\share\plan.md"
    );
}

/// An agent's path is checked on its text alone, before anything touches it: a share is reached
/// only on a host Lectern already trusts, which the caller gives. A note can steer an agent, so an
/// agent never makes a host trusted, as the user's own open does.
#[test]
fn an_agent_reaches_a_share_only_on_a_host_already_trusted() {
    let refused = |raw: &str| agent_path(&wsl_mapper(), raw, &reachable()).unwrap_err();
    assert_eq!(
        refused(r"\\nas2\share\x.md"),
        r"Lectern doesn't open files on \\nas2 for an agent; add that share as a library folder first."
    );
    // Any letter case. (`//nas2/share` is a Linux path, which goes into WSL.)
    assert!(refused(r"\\NAS2\Share\x.md").contains(r"\\nas2 "));
    assert_eq!(
        mapped(r"\\NAS\share\notes\x.md").windows,
        r"\\NAS\share\notes\x.md"
    );
    let wsl = mapped(r"\\wsl.localhost\Ubuntu\home\me\x.md");
    assert_eq!(wsl.wsl.as_deref(), Some("/home/me/x.md"));
    // A WSL path maps onto WSL's share, which is reached as any share is.
    assert_eq!(
        mapped("/home/me/x.md").windows,
        r"\\wsl.localhost\Ubuntu\home\me\x.md"
    );
    assert!(agent_path(&wsl_mapper(), "/home/me/x.md", &[]).is_err());
    // A local path has no host to trust.
    assert_eq!(agent_path(&wsl_mapper(), PLAN, &[]).unwrap().windows, PLAN);
    assert_eq!(
        agent_path(&wsl_mapper(), PLAN_WSL, &[]).unwrap().windows,
        PLAN
    );
}

// Status.

#[test]
fn a_window_status_gives_both_forms_the_kind_and_the_heading_path() {
    let view = WindowView {
        path: Some(PLAN.to_owned()),
        heading: vec!["Plan".to_owned(), "Rollout".to_owned()],
        line: Some(12),
        selection: Some(ViewSelection {
            quote: "roll back".to_owned(),
            start_line: Some(14),
            end_line: Some(15),
        }),
        ..WindowView::default()
    };
    let status = window_status("main", Some("Main".to_owned()), view, &wsl_mapper());
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({
            "label": "main",
            "workspace": "Main",
            "file": {"windows": PLAN, "wsl": PLAN_WSL},
            "kind": "markdown",
            "heading": "Plan › Rollout",
            "line": 12,
            "selection": {"quote": "roll back", "startLine": 14, "endLine": 15},
            "ask": null,
            "answers": [],
        })
    );
}

#[test]
fn a_blank_window_status_is_all_nulls() {
    let status = window_status("win-2", None, WindowView::default(), &wsl_mapper());
    assert_eq!(
        serde_json::to_value(&status).unwrap(),
        json!({
            "label": "win-2",
            "workspace": null,
            "file": null,
            "kind": null,
            "heading": null,
            "line": null,
            "selection": null,
            "ask": null,
            "answers": [],
        })
    );
}

#[test]
fn a_page_has_a_kind_and_a_long_selection_is_cut() {
    let view = WindowView {
        path: Some(r"S:\Notes\My Vault\report.html".to_owned()),
        heading: vec!["Results".to_owned()],
        line: None,
        selection: Some(ViewSelection {
            quote: "é".repeat(SELECTION_MAX + 10),
            start_line: None,
            end_line: None,
        }),
        ..WindowView::default()
    };
    let status = window_status("main", None, view, &wsl_mapper());
    assert_eq!(status.kind, Some(DocKind::Html));
    assert_eq!(status.heading.as_deref(), Some("Results"));
    let quote = &status.selection.unwrap().quote;
    assert_eq!(quote.chars().count(), SELECTION_MAX);
}

// Tools.

const TOOLS: [&str; 9] = [
    "status",
    "show",
    "workspace",
    "highlight",
    "read",
    "comment",
    "comments",
    "reply",
    "resolve",
];

fn args(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

fn call(name: &str, value: Value) -> Result<AgentRequest, CallError> {
    parse_call(name, &args(value))
}

fn invalid(name: &str, value: Value) -> String {
    match call(name, value.clone()) {
        Err(CallError::Invalid(message)) => message,
        other => panic!("{name} {value}: {other:?}"),
    }
}

#[test]
fn there_are_nine_tools_with_closed_schemas_and_two_sentences() {
    let tools = definitions();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOLS);
    for tool in &tools {
        let name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        assert_eq!(schema["type"], "object", "{name}");
        assert_eq!(schema["additionalProperties"], false, "{name}");
        assert!(schema["properties"].is_object(), "{name}");
        let description = tool["description"].as_str().unwrap();
        assert!(description.ends_with('.'), "{name}: {description}");
        assert_eq!(description.split(". ").count(), 2, "{name}: {description}");
        assert!(name.len() <= 128 && name.bytes().all(|b| b.is_ascii_lowercase()));
        // Every property is described, and every nested object is closed too.
        for (prop, spec) in schema["properties"].as_object().unwrap() {
            assert!(spec["description"].is_string(), "{name}.{prop}");
        }
        assert_closed(schema, name);
    }
}

/// Every object schema inside `schema` lists its properties and allows no others.
fn assert_closed(schema: &Value, at: &str) {
    match schema {
        Value::Object(map) => {
            if map.get("type") == Some(&json!("object")) {
                assert_eq!(map.get("additionalProperties"), Some(&json!(false)), "{at}");
            }
            for (key, value) in map {
                assert_closed(value, &format!("{at}.{key}"));
            }
        }
        Value::Array(items) => {
            for item in items {
                assert_closed(item, at);
            }
        }
        _ => {}
    }
}

#[test]
fn every_tool_has_a_short_title() {
    let titles: Vec<(String, String)> = definitions()
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap().to_owned(),
                t["title"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    let expected = [
        ("status", "Lectern status"),
        ("show", "Show a file"),
        ("workspace", "Switch or create a workspace"),
        ("highlight", "Highlight passages"),
        ("read", "Read the rendered text"),
        ("comment", "Start a comment"),
        ("comments", "List comments"),
        ("reply", "Reply to a comment"),
        ("resolve", "Resolve a comment"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(name, title)| ((*name).to_owned(), (*title).to_owned()))
        .collect();
    assert_eq!(titles, expected);
}

#[test]
fn every_path_says_it_takes_a_windows_or_wsl_path() {
    let mut described = Vec::new();
    for tool in definitions() {
        let name = tool["name"].as_str().unwrap().to_owned();
        for prop in ["path", "folders"] {
            let Some(spec) = tool["inputSchema"]["properties"].get(prop) else {
                continue;
            };
            let description = spec["description"].as_str().unwrap();
            for needed in ["Windows path", "WSL path", r"S:\Notes", "/mnt/s/Notes"] {
                assert!(description.contains(needed), "{name}.{prop}: {description}");
            }
            described.push(format!("{name}.{prop}"));
        }
    }
    assert_eq!(
        described,
        [
            "show.path",
            "workspace.folders",
            "highlight.path",
            "read.path",
            "comment.path",
            "comments.path",
            "reply.path",
            "resolve.path",
        ]
    );
    // Each of a new workspace's folders is described as a folder, with folders for examples.
    let workspace = definitions()
        .into_iter()
        .find(|tool| tool["name"] == "workspace")
        .unwrap();
    assert_eq!(
        workspace["inputSchema"]["properties"]["folders"]["items"]["description"],
        r"A folder. A Windows path (S:\Notes\My Vault) or a WSL path (/mnt/s/Notes/My Vault)."
    );
}

#[test]
fn read_only_and_idempotent_tools_say_so() {
    for tool in definitions() {
        let name = tool["name"].as_str().unwrap();
        let read_only = ["status", "read", "comments"].contains(&name);
        let idempotent = ["highlight", "resolve"].contains(&name);
        let annotations = tool.get("annotations").cloned().unwrap_or(json!({}));
        assert_eq!(
            annotations.get("readOnlyHint") == Some(&json!(true)),
            read_only,
            "{name}"
        );
        assert_eq!(
            annotations.get("idempotentHint") == Some(&json!(true)),
            idempotent,
            "{name}"
        );
    }
}

#[test]
fn every_property_a_schema_lists_is_one_its_tool_takes() {
    for tool in definitions() {
        let name = tool["name"].as_str().unwrap();
        for prop in tool["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
        {
            let result = call(name, json!({ prop: true }));
            if let Err(CallError::Invalid(message)) = result {
                assert!(
                    !message.contains("doesn't take"),
                    "{name}.{prop}: {message}"
                );
            }
        }
        let message = invalid(name, json!({"bogus": 1}));
        assert!(message.contains("bogus"), "{name}: {message}");
    }
}

#[test]
fn an_unknown_tool_is_named() {
    assert_eq!(
        call("frobnicate", json!({})),
        Err(CallError::UnknownTool("frobnicate".to_owned()))
    );
}

#[test]
fn status_takes_nothing() {
    assert_eq!(call("status", json!({})), Ok(AgentRequest::Status));
}

#[test]
fn show_takes_a_path_workspace_target_new_window_and_wait() {
    assert_eq!(
        call(
            "show",
            json!({
                "path": PLAN_WSL,
                "workspace": "Notes",
                "target": {"quote": "roll back the migration", "prefix": "we ", "suffix": " first"},
                "newWindow": true,
                "wait": 30,
            })
        ),
        Ok(AgentRequest::Show(ShowArgs {
            path: PLAN_WSL.to_owned(),
            workspace: Some("Notes".to_owned()),
            target: Some(Target::Quote {
                quote: "roll back the migration".to_owned(),
                prefix: Some("we ".to_owned()),
                suffix: Some(" first".to_owned()),
            }),
            new_window: true,
            wait: 30,
        }))
    );
    // Nulls count as left out.
    assert_eq!(
        call(
            "show",
            json!({"path": PLAN, "workspace": null, "target": null})
        ),
        Ok(AgentRequest::Show(ShowArgs {
            path: PLAN.to_owned(),
            workspace: None,
            target: None,
            new_window: false,
            wait: 0,
        }))
    );
    for target in [
        json!({"heading": "Plan › Rollout"}),
        json!({"line": 1}),
        json!({"page": 3}),
        json!({"comment": "C12"}),
    ] {
        assert!(
            call("show", json!({"path": PLAN, "target": target})).is_ok(),
            "{target}"
        );
    }
}

#[test]
fn arguments_over_their_limits_are_tool_errors() {
    let quote = "q".repeat(2001);
    let message = invalid("show", json!({"path": PLAN, "target": {"quote": quote}}));
    assert!(message.contains("2,000"), "{message}");
    assert!(call(
        "show",
        json!({"path": PLAN, "target": {"quote": "q".repeat(2000)}})
    )
    .is_ok());

    let long = format!(r"S:\{}", "a".repeat(4094));
    assert_eq!(long.chars().count(), 4097);
    let message = invalid("show", json!({ "path": long }));
    assert!(message.contains("4,096"), "{message}");
    assert!(call("show", json!({"path": &long[..4096]})).is_ok());

    for wait in [json!(61), json!(-1), json!(1.5), json!("5")] {
        let message = invalid("show", json!({"path": PLAN, "wait": wait}));
        assert!(message.contains("wait"), "{wait}: {message}");
    }
    assert!(invalid("show", json!({"path": PLAN, "target": {"line": 0}})).contains("line"));
    assert!(invalid("show", json!({"path": PLAN, "target": {"page": 0}})).contains("page"));
    assert!(invalid("show", json!({"path": "S:\\a\u{0}b.md"})).contains("path"));
    assert!(invalid("show", json!({"path": ""})).contains("path"));
    assert!(invalid("show", json!({"path": 5})).contains("path"));
    assert!(invalid("show", json!({})).contains("path"));
    assert!(invalid("show", json!({"path": PLAN, "newWindow": "yes"})).contains("newWindow"));
}

#[test]
fn a_target_names_exactly_one_place() {
    for target in [
        json!({}),
        json!({"heading": "A", "line": 2}),
        json!({"prefix": "we "}),
        json!({"quote": "x", "page": 2}),
        json!({"bogus": 1}),
        json!("Rollout"),
        json!({"comment": "3"}),
        json!({"heading": ""}),
    ] {
        let message = invalid("show", json!({"path": PLAN, "target": target}));
        assert!(!message.is_empty(), "{target}");
    }
}

#[test]
fn workspace_switches_or_creates_with_folders() {
    assert_eq!(
        call("workspace", json!({"action": "switch", "name": "Garden"})),
        Ok(AgentRequest::Workspace(WorkspaceArgs {
            action: WorkspaceAction::Switch,
            name: "Garden".to_owned(),
            folders: Vec::new(),
        }))
    );
    assert_eq!(
        call(
            "workspace",
            json!({"action": "create", "name": "Garden", "folders": ["/mnt/s/Notes/Garden"]})
        ),
        Ok(AgentRequest::Workspace(WorkspaceArgs {
            action: WorkspaceAction::Create,
            name: "Garden".to_owned(),
            folders: vec!["/mnt/s/Notes/Garden".to_owned()],
        }))
    );
    assert!(invalid("workspace", json!({"action": "delete", "name": "Garden"})).contains("action"));
    assert!(invalid(
        "workspace",
        json!({"action": "switch", "name": "Garden", "folders": ["/mnt/s/x"]})
    )
    .contains("folders"));
    let many: Vec<String> = (0..21).map(|i| format!("/mnt/s/{i}")).collect();
    assert!(invalid(
        "workspace",
        json!({"action": "create", "name": "Garden", "folders": many})
    )
    .contains("folders"));
    assert!(invalid("workspace", json!({"action": "switch", "name": " "})).contains("name"));
    assert!(invalid(
        "workspace",
        json!({"action": "switch", "name": "n".repeat(61)})
    )
    .contains("name"));
}

#[test]
fn highlight_takes_up_to_twenty_targets_or_a_clear() {
    assert_eq!(
        call(
            "highlight",
            json!({"targets": [{"quote": "roll back"}, {"line": 3}], "clear": true})
        ),
        Ok(AgentRequest::Highlight(HighlightArgs {
            path: None,
            targets: vec![
                Target::Quote {
                    quote: "roll back".to_owned(),
                    prefix: None,
                    suffix: None,
                },
                Target::Line { line: 3 },
            ],
            clear: true,
        }))
    );
    assert!(call("highlight", json!({"targets": [], "clear": true})).is_ok());
    assert!(invalid("highlight", json!({"targets": []})).contains("targets"));
    let many: Vec<Value> = (1..=21).map(|line| json!({ "line": line })).collect();
    assert!(invalid("highlight", json!({ "targets": many })).contains("20"));
    assert!(invalid("highlight", json!({"targets": [{"comment": "C1"}]})).contains("comment"));
    // PDFs can't be highlighted: a page is no target for it.
    assert_eq!(
        invalid("highlight", json!({"targets": [{"page": 2}]})),
        "`targets` can't be a page here: give one of heading, line or quote."
    );
}

#[test]
fn read_pages_by_offset_up_to_sixteen_thousand() {
    assert_eq!(
        call("read", json!({"path": PLAN, "from": 100, "max": 16000})),
        Ok(AgentRequest::Read(ReadArgs {
            path: Some(PLAN.to_owned()),
            from: Some(100),
            max: Some(16000),
        }))
    );
    assert_eq!(
        call("read", json!({})),
        Ok(AgentRequest::Read(ReadArgs {
            path: None,
            from: None,
            max: None,
        }))
    );
    assert_eq!(
        invalid("read", json!({"max": 16001})),
        "`max` must be a whole number from 1 to 16,000."
    );
    assert!(invalid("read", json!({"max": 0})).contains("max"));
    assert!(invalid("read", json!({"from": -1})).contains("from"));
}

#[test]
fn the_review_tools_take_their_threads_and_text() {
    assert_eq!(
        call(
            "comment",
            json!({"path": PLAN, "target": {"line": 4}, "text": "Why now?", "kind": "pushback"})
        ),
        Ok(AgentRequest::Comment(CommentArgs {
            path: PLAN.to_owned(),
            target: Target::Line { line: 4 },
            text: "Why now?".to_owned(),
            kind: Some(EntryKind::Pushback),
        }))
    );
    assert_eq!(
        call("comments", json!({"path": PLAN, "status": "resolved"})),
        Ok(AgentRequest::Comments(CommentsArgs {
            path: PLAN.to_owned(),
            status: Some(ThreadFilter::Resolved),
            from: None,
        }))
    );
    // A later part of a long list.
    assert_eq!(
        call("comments", json!({"path": PLAN, "from": 12})),
        Ok(AgentRequest::Comments(CommentsArgs {
            path: PLAN.to_owned(),
            status: None,
            from: Some(12),
        }))
    );
    assert!(invalid("comments", json!({"path": PLAN, "from": -1})).contains("from"));
    assert_eq!(
        call("reply", json!({"path": PLAN, "id": "C2", "text": "Done."})),
        Ok(AgentRequest::Reply(ReplyArgs {
            path: PLAN.to_owned(),
            id: "C2".to_owned(),
            text: "Done.".to_owned(),
            kind: None,
        }))
    );
    assert_eq!(
        call("resolve", json!({"path": PLAN, "id": "C2"})),
        Ok(AgentRequest::Resolve(ResolveArgs {
            path: PLAN.to_owned(),
            id: "C2".to_owned(),
            text: None,
        }))
    );
    assert!(invalid(
        "comment",
        json!({"path": PLAN, "target": {"line": 4}, "text": ""})
    )
    .contains("text"));
    assert!(invalid(
        "comment",
        json!({"path": PLAN, "target": {"line": 4}, "text": "x", "kind": "resolved"})
    )
    .contains("kind"));
    assert!(invalid(
        "comment",
        json!({"path": PLAN, "target": {"comment": "C1"}, "text": "x"})
    )
    .contains("comment"));
    assert!(invalid("comments", json!({"path": PLAN, "status": "dismissed"})).contains("status"));
    assert!(invalid("reply", json!({"path": PLAN, "id": "2", "text": "x"})).contains("C3"));
    assert!(invalid(
        "reply",
        json!({"path": PLAN, "id": "C2", "text": "x".repeat(20_001)})
    )
    .contains("20,000"));
}

// MCP.

/// What `mcp` answers to `lines`, a tool call relayed to `relay`.
fn session(
    mcp: &Mcp,
    lines: &[&str],
    relay: impl Fn(&AgentRequest) -> Result<Value, String>,
) -> Vec<String> {
    let mut out = Vec::new();
    for line in lines {
        match mcp.handle(line) {
            Handled::Nothing => {}
            Handled::Reply(reply) => out.push(reply),
            Handled::Call(call) => {
                let answer = relay(&call.request);
                out.push(mcp.finish(call, answer));
            }
        }
    }
    for line in &out {
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
    }
    out
}

fn no_relay(request: &AgentRequest) -> Result<Value, String> {
    panic!("relayed {request:?}")
}

fn line(value: Value) -> String {
    serde_json::to_string(&value).unwrap()
}

fn answer_to(mcp: &Mcp, request: Value) -> Value {
    let out = session(mcp, &[&line(request)], no_relay);
    assert_eq!(out.len(), 1, "{out:?}");
    serde_json::from_str(&out[0]).unwrap()
}

fn server_info() -> Value {
    json!({"name": "lectern", "title": "Lectern", "version": version()})
}

fn status_answer() -> Value {
    json!({
        "windows": [{
            "label": "main",
            "workspace": "Main",
            "file": {"windows": PLAN, "wsl": PLAN_WSL},
            "kind": "markdown",
            "heading": "Plan › Rollout",
            "line": 12,
            "selection": null,
            "ask": null,
        }],
        "closedWorkspaces": ["Garden"],
        "settings": {"agentOpens": "ask", "agentFocus": "front"},
    })
}

#[test]
fn a_scripted_session_gives_the_exact_lines() {
    let mcp = Mcp::new(version());
    let out = session(
        &mcp,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"claude-code","version":"2.1.0"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status","arguments":{}}}"#,
        ],
        |request| {
            assert_eq!(request, &AgentRequest::Status);
            Ok(status_answer())
        },
    );
    assert_eq!(out.len(), 3, "{out:#?}");
    assert_eq!(
        out[0],
        line(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            },
        }))
    );
    assert_eq!(
        out[1],
        line(json!({"jsonrpc": "2.0", "id": 2, "result": {"tools": definitions()}}))
    );
    assert_eq!(
        out[2],
        line(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "result": {
                "content": [{"type": "text", "text": line(status_answer())}],
                "structuredContent": status_answer(),
                "isError": false,
            },
        }))
    );
    assert_eq!(
        mcp.client(),
        Some(ClientInfo {
            name: "claude-code".to_owned(),
            version: "2.1.0".to_owned(),
        })
    );
}

#[test]
fn the_tool_list_is_as_reviewed() {
    let tools = serde_json::to_string_pretty(&definitions()).unwrap();
    insta::assert_snapshot!(tools);
}

#[test]
fn initialize_echoes_a_revision_it_speaks_else_gives_its_latest() {
    let mcp = Mcp::new(version());
    for (asked, answered) in [
        ("2025-06-18", "2025-06-18"),
        ("2025-11-25", "2025-11-25"),
        ("2024-11-05", "2025-11-25"),
        ("2030-01-01", "2025-11-25"),
    ] {
        let reply = answer_to(
            &mcp,
            json!({"jsonrpc": "2.0", "id": "init", "method": "initialize",
                   "params": {"protocolVersion": asked, "capabilities": {},
                              "clientInfo": {"name": "codex", "version": "1"}}}),
        );
        assert_eq!(reply["id"], "init");
        assert_eq!(reply["result"]["protocolVersion"], answered, "{asked}");
    }
    assert_eq!(LEGACY, ["2025-11-25", "2025-06-18"]);
    assert!(INSTRUCTIONS.contains("WSL"), "{INSTRUCTIONS}");
    assert!(!INSTRUCTIONS.contains('\n'));
    // The first client to name itself stays the client.
    assert_eq!(mcp.client().unwrap().name, "codex");
}

#[test]
fn ping_answers_empty_and_notifications_answer_nothing() {
    let mcp = Mcp::new(version());
    assert_eq!(
        answer_to(&mcp, json!({"jsonrpc": "2.0", "id": 5, "method": "ping"})),
        json!({"jsonrpc": "2.0", "id": 5, "result": {}})
    );
    let quiet = session(
        &mcp,
        &[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":3}}"#,
            r#"{"jsonrpc":"2.0","method":"no/such/notification"}"#,
            r#"{"jsonrpc":"2.0","id":9,"result":{}}"#,
            "",
            "   ",
        ],
        no_relay,
    );
    assert!(quiet.is_empty(), "{quiet:?}");
}

#[test]
fn a_malformed_line_is_a_parse_error_and_the_next_line_still_works() {
    let mcp = Mcp::new(version());
    let out = session(
        &mcp,
        &[
            r#"{"jsonrpc":"2.0","id":1,"method":"ping""#,
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
        ],
        no_relay,
    );
    let first: Value = serde_json::from_str(&out[0]).unwrap();
    assert_eq!(first["id"], Value::Null);
    assert_eq!(first["error"]["code"], code::PARSE_ERROR);
    assert_eq!(
        serde_json::from_str::<Value>(&out[1]).unwrap(),
        json!({"jsonrpc": "2.0", "id": 2, "result": {}})
    );
}

#[test]
fn requests_that_are_not_json_rpc_are_invalid() {
    let mcp = Mcp::new(version());
    for request in [
        json!([{"jsonrpc": "2.0", "id": 1, "method": "ping"}]),
        json!("ping"),
        json!({"id": 1, "method": "ping"}),
        json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 1}),
        json!({"jsonrpc": "2.0", "id": {"n": 1}, "method": "ping"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": 7}),
    ] {
        let reply = answer_to(&mcp, request.clone());
        assert_eq!(reply["error"]["code"], code::INVALID_REQUEST, "{request}");
    }
}

#[test]
fn an_unknown_method_is_not_found() {
    let mcp = Mcp::new(version());
    let reply = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"}),
    );
    assert_eq!(reply["id"], 4);
    assert_eq!(reply["error"]["code"], code::METHOD_NOT_FOUND);
}

#[test]
fn an_unknown_tool_or_a_malformed_call_is_invalid_params() {
    let mcp = Mcp::new(version());
    let unknown = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
               "params": {"name": "frobnicate", "arguments": {}}}),
    );
    assert_eq!(unknown["error"]["code"], code::INVALID_PARAMS);
    assert!(unknown["error"]["message"]
        .as_str()
        .unwrap()
        .contains("frobnicate"));
    for params in [
        json!(null),
        json!({}),
        json!({"name": 5}),
        json!({"name": "status", "arguments": [1]}),
        json!([1]),
    ] {
        let reply = answer_to(
            &mcp,
            json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": params}),
        );
        assert_eq!(reply["error"]["code"], code::INVALID_PARAMS, "{params}");
    }
    // Arguments left out are none.
    let out = session(
        &mcp,
        &[r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"status"}}"#],
        |_| Ok(json!({})),
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out[0]).unwrap()["result"]["isError"],
        false
    );
}

#[test]
fn bad_arguments_and_lectern_errors_are_tool_errors() {
    let mcp = Mcp::new(version());
    let quote = "q".repeat(2001);
    let reply = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call",
               "params": {"name": "show", "arguments": {"path": PLAN, "target": {"quote": quote}}}}),
    );
    assert_eq!(reply["result"]["isError"], true);
    assert_eq!(reply["result"]["content"][0]["type"], "text");
    assert!(reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("2,000"));
    assert!(reply["result"].get("structuredContent").is_none());

    let out = session(
        &mcp,
        &[
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"read","arguments":{}}}"#,
        ],
        |_| Err("Not yet available.".to_owned()),
    );
    assert_eq!(
        serde_json::from_str::<Value>(&out[0]).unwrap(),
        json!({
            "jsonrpc": "2.0",
            "id": 10,
            "result": {
                "content": [{"type": "text", "text": "Not yet available."}],
                "isError": true,
            },
        })
    );
}

#[test]
fn only_an_object_answer_is_structured_content() {
    let mcp = Mcp::new(version());
    for answer in [json!([1, 2]), json!("plain"), json!(3), json!(null)] {
        let out = session(
            &mcp,
            &[r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"status"}}"#],
            |_| Ok(answer.clone()),
        );
        let reply: Value = serde_json::from_str(&out[0]).unwrap();
        assert_eq!(
            reply["result"],
            json!({"content": [{"type": "text", "text": line(answer.clone())}], "isError": false}),
            "{answer}"
        );
    }
}

// The 2026-07-28 revision.

fn meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": MODERN,
        "io.modelcontextprotocol/clientInfo": {"name": "claude-code", "version": "3.0.0"},
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

fn fixture(name: &str) -> String {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/mcp")
        .join(name);
    fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .replace("{version}", version())
}

#[test]
fn server_discover_answers_as_the_fixture_has_it() {
    let mcp = Mcp::new(version());
    let request = fixture("discover-request.json");
    let request: Value = serde_json::from_str(&request).unwrap();
    let expected: Value = serde_json::from_str(&fixture("discover-answer.json")).unwrap();
    let out = session(&mcp, &[&line(request)], no_relay);
    assert_eq!(serde_json::from_str::<Value>(&out[0]).unwrap(), expected);
    assert_eq!(mcp.client().unwrap().name, "ExampleClient");
}

#[test]
fn modern_results_are_complete_and_name_the_server() {
    let mcp = Mcp::new(version());
    let listed = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta()}}),
    );
    assert_eq!(listed["result"]["resultType"], "complete");
    assert_eq!(listed["result"]["tools"], json!(definitions()));
    assert_eq!(
        listed["result"]["_meta"]["io.modelcontextprotocol/serverInfo"],
        server_info()
    );
    let out = session(
        &mcp,
        &[&line(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                       "params": {"name": "status", "arguments": {}, "_meta": meta()}}),
        )],
        |_| Ok(status_answer()),
    );
    let called: Value = serde_json::from_str(&out[0]).unwrap();
    assert_eq!(called["result"]["resultType"], "complete");
    assert_eq!(called["result"]["structuredContent"], status_answer());
    assert_eq!(
        called["result"]["_meta"]["io.modelcontextprotocol/serverInfo"],
        server_info()
    );
    let pinged = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 3, "method": "ping", "params": {"_meta": meta()}}),
    );
    assert_eq!(pinged["result"]["resultType"], "complete");
    assert_eq!(mcp.client().unwrap().version, "3.0.0");
}

#[test]
fn a_revision_lectern_does_not_speak_is_refused_with_those_it_does() {
    let mcp = Mcp::new(version());
    let mut meta = meta();
    meta["io.modelcontextprotocol/protocolVersion"] = json!("1900-01-01");
    let reply = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}),
    );
    assert_eq!(
        reply,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": code::UNSUPPORTED_VERSION,
                "message": "Unsupported protocol version",
                "data": {
                    "supported": ["2026-07-28", "2025-11-25", "2025-06-18"],
                    "requested": "1900-01-01",
                },
            },
        })
    );
}

#[test]
fn a_modern_request_without_its_meta_is_invalid_params() {
    let mcp = Mcp::new(version());
    let discover = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover"}),
    );
    assert_eq!(discover["error"]["code"], code::INVALID_PARAMS);
    let mut meta = meta();
    meta.as_object_mut()
        .unwrap()
        .remove("io.modelcontextprotocol/clientCapabilities");
    let listed = answer_to(
        &mcp,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
    );
    assert_eq!(listed["error"]["code"], code::INVALID_PARAMS);
}
