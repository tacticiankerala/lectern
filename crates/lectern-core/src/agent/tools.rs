//! The tools Lectern offers agents: their definitions, as `tools/list` gives them, and their
//! arguments, checked and turned into an [`AgentRequest`]. Arguments come from a model, so each
//! one is checked against its tool's schema and its limits, and a bad one answers with a sentence
//! the model can act on.

use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};

use super::request::{
    AgentRequest, CommentArgs, CommentsArgs, HighlightArgs, ReadArgs, ReplyArgs, ResolveArgs,
    ShowArgs, Target, WorkspaceAction, WorkspaceArgs,
};
use crate::workspace::MAX_NAME_CHARS;

/// The longest path an argument may hold, in characters.
pub const PATH_MAX: usize = 4096;
/// The longest quote, heading, prefix or suffix, in characters.
pub const QUOTE_MAX: usize = 2000;
/// The longest comment or reply, in characters.
pub const TEXT_MAX: usize = 20_000;
/// The longest a `show` waits for the reader's answer, in seconds.
pub const WAIT_MAX: u32 = 60;
/// The most characters one `read` returns.
pub const READ_MAX: u32 = 16_000;
/// The most targets one `highlight` paints.
pub const TARGETS_MAX: usize = 20;
/// The most folders a new workspace starts with.
pub const FOLDERS_MAX: usize = 20;

/// Why a tool call can't go to Lectern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// No tool has this name: a JSON-RPC error.
    UnknownTool(String),
    /// The arguments don't fit the tool: a sentence for the model, as a tool error.
    Invalid(String),
}

/// Every tool's name, in the order `tools/list` gives them.
pub const TOOL_NAMES: [&str; 9] = [
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

/// Every tool's definition, in [`TOOL_NAMES`] order, as `tools/list` gives them.
pub fn definitions() -> Vec<Value> {
    TOOL_NAMES
        .iter()
        .filter_map(|name| definition(name))
        .collect()
}

/// The request a call of the tool `name` with `arguments` makes. Each argument is checked against
/// the tool's schema (the properties it lists, and no others) and its limits.
pub fn parse_call(name: &str, arguments: &Map<String, Value>) -> Result<AgentRequest, CallError> {
    let definition = definition(name).ok_or_else(|| CallError::UnknownTool(name.to_owned()))?;
    let args = Args {
        tool: name,
        map: arguments,
    };
    args.only(&definition["inputSchema"]["properties"])
        .and_then(|()| args.request())
        .map_err(CallError::Invalid)
}

/// The places a target can name, in the order a message lists them.
const PLACES: [&str; 5] = ["heading", "line", "quote", "page", "comment"];

/// The tool `name`'s definition: its title, its JSON Schema (closed, every property described),
/// its two sentences, and its annotations.
fn definition(name: &str) -> Option<Value> {
    let (title, description, properties, required, annotations) = match name {
        "status" => (
            "Lectern status",
            "Tells what each Lectern window shows: its workspace, the file and its kind, the \
             heading and line at the top of the view, the user's selection and any request \
             waiting on their answer, with the closed workspaces and the agent settings. Call it \
             first to see what the user is reading, before showing or highlighting anything.",
            json!({}),
            &[][..],
            Some(json!({"readOnlyHint": true})),
        ),
        "show" => (
            "Show a file",
            "Opens a file in Lectern, in the window of the workspace that holds it or of the one \
             named, scrolled to a heading, line, quote, PDF page or comment thread when given. \
             When that window already shows another file, Lectern may ask the user first and \
             answer `asked`, and `wait` waits that many seconds for their answer.",
            json!({
                "path": path_schema("The file to show."),
                "workspace": text_schema(
                    "The workspace to show it in, by name; by default the one whose folders \
                     hold it.",
                    MAX_NAME_CHARS,
                ),
                "target": target_schema(
                    "Where to scroll to: a heading, a line, a quote, a PDF page or a comment \
                     thread.",
                    &PLACES,
                ),
                "newWindow": {
                    "type": "boolean",
                    "description": "Show it in a new window, unless its workspace's window is \
                                    open already.",
                },
                "wait": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": WAIT_MAX,
                    "description": "Seconds to wait for the user's answer when Lectern asks them \
                                    first, up to 60; 0, the default, answers at once.",
                },
            }),
            &["path"][..],
            None,
        ),
        "workspace" => (
            "Switch or create a workspace",
            "Switches to a Lectern workspace by name, bringing its window forward or reopening \
             it, or creates a new one holding the given folders in a new window. It never \
             changes the workspace an open window shows.",
            json!({
                "action": {
                    "type": "string",
                    "enum": ["switch", "create"],
                    "description": "switch brings the workspace's window forward, reopening it \
                                    if it is closed; create makes a new workspace.",
                },
                "name": text_schema(
                    "The workspace's name: one that exists, to switch to it, or a new one.",
                    MAX_NAME_CHARS,
                ),
                "folders": {
                    "type": "array",
                    "items": located_schema(
                        "A folder.",
                        r"S:\Notes\My Vault",
                        "/mnt/s/Notes/My Vault",
                    ),
                    "maxItems": FOLDERS_MAX,
                    "description": "The folders a new workspace holds, for create only: each a \
                                    Windows path (S:\\Notes\\Garden) or a WSL path \
                                    (/mnt/s/Notes/Garden).",
                },
            }),
            &["action", "name"][..],
            None,
        ),
        "highlight" => (
            "Highlight passages",
            "Highlights passages, by heading, line or quote, in a note, text file or HTML page \
             a Lectern window shows, after removing your earlier highlights when `clear` is set. \
             Highlights are temporary: they are never saved and go when the user leaves the \
             file.",
            json!({
                "path": path_schema("The file to highlight in, which a window must show; by \
                                     default the file shown."),
                "targets": {
                    "type": "array",
                    "items": target_schema("A passage.", &PLACES[..3]),
                    "maxItems": TARGETS_MAX,
                    "description": "The passages to highlight, at most 20.",
                },
                "clear": {
                    "type": "boolean",
                    "description": "Remove your earlier highlights first; with no targets, only \
                                    that.",
                },
            }),
            &["targets"][..],
            Some(json!({"idempotentHint": true})),
        ),
        "read" => (
            "Read the rendered text",
            "Reads the text of a file a Lectern window shows, as rendered (an HTML page's after \
             its scripts ran), with where each block starts in it, its line, and its headings as \
             a place in the answer's `headings`. A long text comes in parts: when the answer is \
             `truncated`, call again with `from` set to its `next`.",
            json!({
                "path": path_schema("The file to read, which a window must show; by default \
                                     the file shown."),
                "from": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "The character to start from; 0 by default.",
                },
                "max": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": READ_MAX,
                    "description": "The most characters to return, up to 16,000; 8,000 by \
                                    default. A page of many small blocks holds fewer, to keep \
                                    the answer under 48 KB.",
                },
            }),
            &[][..],
            Some(json!({"readOnlyHint": true})),
        ),
        "comment" => (
            "Start a comment",
            "Starts a review comment thread on a passage of a note, text file or HTML page, \
             saved in its review sidecar under your name. Its kind is question (the default), \
             reply or pushback.",
            json!({
                "path": path_schema("The note, text file or HTML page to comment on."),
                "target": target_schema(
                    "The passage the comment is about: a quote, a line or a heading.",
                    &PLACES[..3],
                ),
                "text": text_schema("What you have to say, in Markdown.", TEXT_MAX),
                "kind": kind_schema(["question", "reply", "pushback"]),
            }),
            &["path", "target", "text"][..],
            None,
        ),
        "comments" => (
            "List comments",
            "Lists a file's review comment threads as Lectern's Comments tab shows them, each \
             with its id, status, where it is anchored now, its quote and its entries, leaving \
             out resolved and dismissed threads by default. A long list comes in parts: when the \
             answer has `next`, call again with `from` set to it.",
            json!({
                "path": path_schema("The file whose threads to list."),
                "status": {
                    "type": "string",
                    "enum": ["open", "question", "pushback", "resolved", "all"],
                    "description": "Which threads to list; by default every thread that isn't \
                                    resolved or dismissed.",
                },
                "from": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "The thread to start from, by its place in the list: 0, the \
                                    default, or the `next` of an answer that stopped short.",
                },
            }),
            &["path"][..],
            Some(json!({"readOnlyHint": true})),
        ),
        "reply" => (
            "Reply to a comment",
            "Adds your reply to a review comment thread, given by its id (C1, C2 and so on). \
             Its kind is reply (the default), question or pushback.",
            json!({
                "path": path_schema("The file the thread is on."),
                "id": id_schema(),
                "text": text_schema("Your reply, in Markdown.", TEXT_MAX),
                "kind": kind_schema(["reply", "question", "pushback"]),
            }),
            &["path", "id", "text"][..],
            None,
        ),
        "resolve" => (
            "Resolve a comment",
            "Resolves a review comment thread, given by its id, with an optional closing note. \
             A later reply from the user opens it again.",
            json!({
                "path": path_schema("The file the thread is on."),
                "id": id_schema(),
                "text": text_schema("A closing note; \"Resolved.\" by default.", TEXT_MAX),
            }),
            &["path", "id"][..],
            Some(json!({"idempotentHint": true})),
        ),
        _ => return None,
    };
    let mut tool = json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        },
    });
    if let Some(annotations) = annotations {
        tool["annotations"] = annotations;
    }
    Some(tool)
}

/// A file's path, described as `description`.
fn path_schema(description: &str) -> Value {
    located_schema(description, r"S:\Notes\plan.md", "/mnt/s/Notes/plan.md")
}

/// A path, described as `description`, with a Windows and a WSL example of it.
fn located_schema(description: &str, windows: &str, wsl: &str) -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "maxLength": PATH_MAX,
        "description": format!(
            "{description} A Windows path ({windows}) or a WSL path ({wsl})."
        ),
    })
}

fn text_schema(description: &str, max: usize) -> Value {
    json!({"type": "string", "minLength": 1, "maxLength": max, "description": description})
}

fn kind_schema(kinds: [&str; 3]) -> Value {
    json!({
        "type": "string",
        "enum": kinds,
        "description": format!("{} (the default), {} or {}.", kinds[0], kinds[1], kinds[2]),
    })
}

/// A review thread's id: `C` and its number.
const ID_PATTERN: &str = "^C[1-9][0-9]*$";

fn id_schema() -> Value {
    json!({"type": "string", "pattern": ID_PATTERN, "description": "The thread's id: C3."})
}

/// A target naming one of `places`, described as `description`.
fn target_schema(description: &str, places: &[&str]) -> Value {
    let quote = |description: &str| json!({"type": "string", "maxLength": QUOTE_MAX, "description": description});
    let place = |name: &str| match name {
        "heading" => json!({
            "type": "object",
            "properties": {"heading": {
                "type": "string",
                "minLength": 1,
                "maxLength": QUOTE_MAX,
                "description": "A heading's text, or its path: Plan › Rollout.",
            }},
            "required": ["heading"],
            "additionalProperties": false,
        }),
        "line" => json!({
            "type": "object",
            "properties": {"line": {
                "type": "integer",
                "minimum": 1,
                "description": "A line of the file's source, from 1.",
            }},
            "required": ["line"],
            "additionalProperties": false,
        }),
        "quote" => json!({
            "type": "object",
            "properties": {
                "quote": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": QUOTE_MAX,
                    "description": "The passage, word for word, at most 2,000 characters.",
                },
                "prefix": quote("Text just before the passage, to tell apart two places it \
                                 appears."),
                "suffix": quote("Text just after the passage."),
            },
            "required": ["quote"],
            "additionalProperties": false,
        }),
        "page" => json!({
            "type": "object",
            "properties": {"page": {
                "type": "integer",
                "minimum": 1,
                "description": "A PDF's page, from 1.",
            }},
            "required": ["page"],
            "additionalProperties": false,
        }),
        _ => json!({
            "type": "object",
            "properties": {"comment": id_schema()},
            "required": ["comment"],
            "additionalProperties": false,
        }),
    };
    json!({
        "description": description,
        "anyOf": places.iter().map(|name| place(name)).collect::<Vec<_>>(),
    })
}

/// A tool call's arguments, read with a sentence for the model on each mistake. A `null` counts
/// as left out.
struct Args<'a> {
    tool: &'a str,
    map: &'a Map<String, Value>,
}

impl Args<'_> {
    /// The request the arguments make, for the tool they are for.
    fn request(&self) -> Result<AgentRequest, String> {
        Ok(match self.tool {
            "status" => AgentRequest::Status,
            "show" => AgentRequest::Show(ShowArgs {
                path: self.need("path", self.path("path")?)?,
                workspace: self.text("workspace", MAX_NAME_CHARS)?,
                target: self.target("target", &PLACES)?,
                new_window: self.flag("newWindow")?,
                wait: self.number("wait", 0, WAIT_MAX)?.unwrap_or(0),
            }),
            "workspace" => {
                let action = self.need("action", self.choice("action", &["switch", "create"])?)?;
                let folders = self.folders()?;
                if action == WorkspaceAction::Switch && !folders.is_empty() {
                    return Err("`folders` are for create only.".to_owned());
                }
                AgentRequest::Workspace(WorkspaceArgs {
                    action,
                    name: self.need("name", self.text("name", MAX_NAME_CHARS)?)?,
                    folders,
                })
            }
            "highlight" => {
                let clear = self.flag("clear")?;
                let targets = self.targets(clear)?;
                AgentRequest::Highlight(HighlightArgs {
                    path: self.path("path")?,
                    targets,
                    clear,
                })
            }
            "read" => AgentRequest::Read(ReadArgs {
                path: self.path("path")?,
                from: self.number("from", 0, u32::MAX)?,
                max: self.number("max", 1, READ_MAX)?,
            }),
            "comment" => AgentRequest::Comment(CommentArgs {
                path: self.need("path", self.path("path")?)?,
                target: self.need("target", self.target("target", &PLACES[..3])?)?,
                text: self.need("text", self.text("text", TEXT_MAX)?)?,
                kind: self.choice("kind", &["question", "reply", "pushback"])?,
            }),
            "comments" => AgentRequest::Comments(CommentsArgs {
                path: self.need("path", self.path("path")?)?,
                status: self.choice(
                    "status",
                    &["open", "question", "pushback", "resolved", "all"],
                )?,
                from: self.number("from", 0, u32::MAX)?,
            }),
            "reply" => AgentRequest::Reply(ReplyArgs {
                path: self.need("path", self.path("path")?)?,
                id: self.need("id", self.thread_id("id")?)?,
                text: self.need("text", self.text("text", TEXT_MAX)?)?,
                kind: self.choice("kind", &["reply", "question", "pushback"])?,
            }),
            _ => AgentRequest::Resolve(ResolveArgs {
                path: self.need("path", self.path("path")?)?,
                id: self.need("id", self.thread_id("id")?)?,
                text: self.text("text", TEXT_MAX)?,
            }),
        })
    }

    /// Refuses an argument the tool's schema doesn't list in `properties`.
    fn only(&self, properties: &Value) -> Result<(), String> {
        let takes: Vec<&str> = properties
            .as_object()
            .map(|props| props.keys().map(String::as_str).collect())
            .unwrap_or_default();
        match self.map.keys().find(|key| !takes.contains(&key.as_str())) {
            None => Ok(()),
            Some(key) if takes.is_empty() => Err(format!(
                "{} doesn't take `{key}`: it takes no arguments.",
                self.tool
            )),
            Some(key) => Err(format!(
                "{} doesn't take `{key}`. It takes {}.",
                self.tool,
                listed(&takes, "and")
            )),
        }
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key).filter(|value| !value.is_null())
    }

    /// `value`, or a sentence saying the tool needs `key`.
    fn need<T>(&self, key: &str, value: Option<T>) -> Result<T, String> {
        value.ok_or_else(|| format!("{} needs `{key}`.", self.tool))
    }

    fn text(&self, key: &str, max: usize) -> Result<Option<String>, String> {
        self.get(key).map(|value| text(value, key, max)).transpose()
    }

    fn path(&self, key: &str) -> Result<Option<String>, String> {
        self.get(key).map(|value| path(value, key)).transpose()
    }

    fn flag(&self, key: &str) -> Result<bool, String> {
        match self.get(key) {
            None => Ok(false),
            Some(Value::Bool(on)) => Ok(*on),
            Some(_) => Err(format!("`{key}` must be true or false.")),
        }
    }

    fn number(&self, key: &str, min: u32, max: u32) -> Result<Option<u32>, String> {
        self.get(key)
            .map(|value| number(value, key, min, max))
            .transpose()
    }

    /// One of `names`, as the enum `T` takes it.
    fn choice<T: DeserializeOwned>(&self, key: &str, names: &[&str]) -> Result<Option<T>, String> {
        let Some(value) = self.get(key) else {
            return Ok(None);
        };
        match value.as_str() {
            Some(name) if names.contains(&name) => serde_json::from_value(value.clone())
                .map(Some)
                .map_err(|e| e.to_string()),
            _ => Err(format!("`{key}` must be {}.", listed(names, "or"))),
        }
    }

    fn thread_id(&self, key: &str) -> Result<Option<String>, String> {
        self.get(key).map(|value| thread_id(value, key)).transpose()
    }

    fn target(&self, key: &str, places: &[&str]) -> Result<Option<Target>, String> {
        self.get(key)
            .map(|value| target(value, key, places))
            .transpose()
    }

    /// `targets`: up to [`TARGETS_MAX`] of them, highlights' places; none only with `clear`.
    fn targets(&self, clear: bool) -> Result<Vec<Target>, String> {
        let items = match self.get("targets") {
            None => return Err(format!("{} needs `targets`.", self.tool)),
            Some(Value::Array(items)) => items,
            Some(_) => return Err("`targets` must be a list of targets.".to_owned()),
        };
        if items.len() > TARGETS_MAX {
            return Err(format!(
                "`targets` holds {} targets; highlight at most {TARGETS_MAX} at once.",
                items.len()
            ));
        }
        if items.is_empty() && !clear {
            return Err(
                "`targets` is empty: give at least one, or set `clear` to remove your highlights."
                    .to_owned(),
            );
        }
        items
            .iter()
            .map(|item| target(item, "targets", &PLACES[..3]))
            .collect()
    }

    /// `folders`: up to [`FOLDERS_MAX`] paths.
    fn folders(&self) -> Result<Vec<String>, String> {
        let items = match self.get("folders") {
            None => return Ok(Vec::new()),
            Some(Value::Array(items)) => items,
            Some(_) => return Err("`folders` must be a list of paths.".to_owned()),
        };
        if items.len() > FOLDERS_MAX {
            return Err(format!(
                "`folders` holds {} folders; a workspace starts with at most {FOLDERS_MAX}.",
                items.len()
            ));
        }
        items.iter().map(|item| path(item, "folders")).collect()
    }
}

/// `value` as text that isn't blank, of at most `max` characters.
fn text(value: &Value, key: &str, max: usize) -> Result<String, String> {
    let Some(text) = value.as_str() else {
        return Err(format!("`{key}` must be a string."));
    };
    if text.trim().is_empty() {
        return Err(format!("`{key}` is empty."));
    }
    if text.chars().count() > max {
        return Err(format!(
            "`{key}` is longer than {} characters.",
            thousands(max)
        ));
    }
    Ok(text.to_owned())
}

/// `value` as a path: text of at most [`PATH_MAX`] characters, without a NUL.
fn path(value: &Value, key: &str) -> Result<String, String> {
    let path = text(value, key, PATH_MAX)?;
    if path.contains('\0') {
        return Err(format!("`{key}` holds a NUL character, which no path can."));
    }
    Ok(path)
}

/// `value` as a whole number from `min` to `max`.
fn number(value: &Value, key: &str, min: u32, max: u32) -> Result<u32, String> {
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| match max {
            u32::MAX => format!("`{key}` must be a whole number from {min} up."),
            _ => format!(
                "`{key}` must be a whole number from {min} to {}.",
                thousands(max as usize)
            ),
        })
}

/// `value` as a review thread's id, `C3`.
fn thread_id(value: &Value, key: &str) -> Result<String, String> {
    let id = value.as_str().unwrap_or_default();
    let digits = id.strip_prefix('C').unwrap_or_default();
    let well_formed = !digits.is_empty()
        && digits.len() <= 9
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit());
    if well_formed {
        Ok(id.to_owned())
    } else {
        Err(format!("`{key}` must be a thread's id, like C3."))
    }
}

/// `value` as a target naming exactly one of `places`, with only that place's fields.
fn target(value: &Value, key: &str, places: &[&str]) -> Result<Target, String> {
    let fields = value.as_object().ok_or_else(|| {
        format!(
            "`{key}` must be an object naming one place: {}.",
            listed(places, "or")
        )
    })?;
    let named: Vec<&str> = PLACES
        .into_iter()
        .filter(|place| fields.get(*place).is_some_and(|v| !v.is_null()))
        .collect();
    let place = match named[..] {
        [place] => place,
        [] => {
            return Err(format!(
                "`{key}` names no place: give one of {}.",
                listed(places, "or")
            ))
        }
        _ => {
            return Err(format!(
                "`{key}` names {}; give one place.",
                listed(&named, "and")
            ))
        }
    };
    if !places.contains(&place) {
        return Err(format!(
            "`{key}` can't be a {place} here: give one of {}.",
            listed(places, "or")
        ));
    }
    let takes: &[&str] = match place {
        "quote" => &["quote", "prefix", "suffix"],
        _ => &[place],
    };
    if let Some(other) = fields.keys().find(|k| !takes.contains(&k.as_str())) {
        return Err(format!("A {place} target doesn't take `{other}`."));
    }
    let optional = |name: &str| -> Result<Option<String>, String> {
        match fields.get(name).filter(|v| !v.is_null()) {
            None => Ok(None),
            Some(Value::String(s)) if s.chars().count() <= QUOTE_MAX => Ok(Some(s.clone())),
            Some(Value::String(_)) => Err(format!(
                "`{name}` is longer than {} characters.",
                thousands(QUOTE_MAX)
            )),
            Some(_) => Err(format!("`{name}` must be a string.")),
        }
    };
    let value = &fields[place];
    Ok(match place {
        "heading" => Target::Heading {
            heading: text(value, "heading", QUOTE_MAX)?,
        },
        "line" => Target::Line {
            line: number(value, "line", 1, u32::MAX)?,
        },
        "quote" => Target::Quote {
            quote: text(value, "quote", QUOTE_MAX)?,
            prefix: optional("prefix")?,
            suffix: optional("suffix")?,
        },
        "page" => Target::Page {
            page: number(value, "page", 1, u32::MAX)?,
        },
        _ => Target::Comment {
            comment: thread_id(value, "comment")?,
        },
    })
}

/// `items` as a list in a sentence: `a`, `a or b`, `a, b or c`.
pub(crate) fn listed(items: &[&str], and: &str) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} {and} {last}", rest.join(", ")),
    }
}

/// `n` with its thousands separated by commas: 20,000.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}
