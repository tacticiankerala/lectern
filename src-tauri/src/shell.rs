//! Handing files, folders and links to Windows: what may be opened with the shell, what goes to
//! the editor, and what is only ever revealed in Explorer.
//!
//! Targets come from document HTML, which a note can forge (`data-kind`, `data-target`), so
//! nothing here trusts them. Only absolute local paths are accepted. The shell opens only viewable
//! types, code and text go to the editor, and anything else (programs, scripts, shortcuts, unknown
//! types) is revealed in Explorer, never run.

use std::env;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use lectern_core::ipc::EditorPref;
use lectern_core::library::MARKDOWN_EXTENSIONS;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;
use windows::Win32::UI::Shell::{
    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Opened with the shell's default app: images, documents, office files, media, web pages.
const VIEWABLE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "pdf", "txt", "csv", "log", "docx",
    "xlsx", "pptx", "odt", "ods", "mp4", "mov", "mp3", "wav", "html", "htm",
];

/// Opened in the editor, at line 1 when no line is cited. Never shell-opened: `.js` and `.py`
/// would run under Windows Script Host or the Python launcher.
const CODE: &[&str] = &[
    "rb", "js", "jsx", "ts", "tsx", "py", "go", "rs", "java", "kt", "cs", "css", "scss", "json",
    "jsonc", "yml", "yaml", "toml", "sh", "sql", "vim", "lua", "xml", "ini", "cfg", "conf", "env",
    "erb", "haml", "slim",
];

/// `raw` when it is an absolute local path: `X:\…` (or `X:/…`) or `\\server\share\…`. URLs and
/// other schemes, device paths (`\\.\…`, `\\?\…`), relative paths, bare names, a second `:` (an
/// alternate data stream) and characters Windows forbids in paths are all refused.
pub fn local_path(raw: &str) -> Option<&str> {
    let forbidden = |c: char| c.is_control() || matches!(c, '"' | '<' | '>' | '|' | '?' | '*');
    if raw.chars().any(forbidden) {
        return None;
    }
    let b = raw.as_bytes();
    let rest = if b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'\\' | b'/')
    {
        &raw[2..]
    } else {
        let unc = raw.strip_prefix(r"\\")?;
        let mut parts = unc.split(['\\', '/']);
        let (server, share) = (parts.next()?, parts.next()?);
        if server.is_empty() || share.is_empty() || server == "." {
            return None;
        }
        unc
    };
    (!rest.contains(':')).then_some(raw)
}

/// What a local file is, by extension, for deciding how to open it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Markdown,
    /// Code and text: the editor.
    Code,
    /// The shell's default app.
    Viewable,
    /// Programs, scripts, shortcuts and anything unknown: only revealed in Explorer.
    Other,
}

pub fn file_kind(path: &str) -> FileKind {
    let Some(ext) = extension(path) else {
        return FileKind::Other;
    };
    let is = |list: &[&str]| list.iter().any(|e| ext.eq_ignore_ascii_case(e));
    if is(MARKDOWN_EXTENSIONS) {
        FileKind::Markdown
    } else if is(CODE) {
        FileKind::Code
    } else if is(VIEWABLE) {
        FileKind::Viewable
    } else {
        FileKind::Other
    }
}

/// The extension, counting a dotfile's name (`.env`) as one.
fn extension(path: &str) -> Option<&str> {
    let path = Path::new(path);
    match path.extension() {
        Some(ext) => ext.to_str(),
        None => path.file_name()?.to_str()?.strip_prefix('.'),
    }
}

/// `url` when it is an http, https or mailto link; a protocol-relative `//host/…` becomes https.
pub fn web_link(url: &str) -> Option<String> {
    let url = url.trim();
    if url.starts_with("//") {
        return Some(format!("https:{url}"));
    }
    let (scheme, _) = url.split_once(':')?;
    ["http", "https", "mailto"]
        .iter()
        .any(|s| scheme.eq_ignore_ascii_case(s))
        .then(|| url.to_owned())
}

/// Opens a viewable local file (an image, a PDF, …) with its default app.
pub fn open_file(path: &str) -> Result<(), String> {
    let path = local_path(path).ok_or_else(|| not_local(path))?;
    if file_kind(path) != FileKind::Viewable {
        return Err(format!("Lectern doesn't open {path} with other apps"));
    }
    shell_execute(path)
}

/// Opens an http, https or mailto link in the default browser or mail app.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = web_link(url)
        .ok_or_else(|| format!("Lectern only opens http, https and mailto links, not {url}"))?;
    shell_execute(&url)
}

/// Opens an Explorer window on the file's folder with the file selected.
pub fn reveal_in_explorer(path: &str) -> Result<(), String> {
    // `local_path` refuses `"`, which would end the quoted argument early.
    let path = local_path(path).ok_or_else(|| not_local(path))?;
    let path = path.replace('/', "\\");
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(drop)
        .map_err(|e| format!("Couldn't open Explorer: {e}"))
}

fn not_local(path: &str) -> String {
    format!("{path} isn't a full path to a file on this computer")
}

/// Hands `target` to its default handler through `ShellExecuteExW`, synchronously and without
/// error dialogs. Blocks while the shell resolves the target, so call it off the main thread.
fn shell_execute(target: &str) -> Result<(), String> {
    let _com = ComApartment::enter();
    let file = HSTRING::from(target);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // NOASYNC: COM is uninitialised as soon as this returns.
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialised and `file` outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        // The low word of a Win32-facility HRESULT is the Win32 error.
        match e.code().0 as u32 & 0xFFFF {
            2 | 3 => format!("Couldn't find {target}"),
            5 => format!("Windows denied access to {target}"),
            1155 => format!("No app is set up to open {target}"),
            _ => format!("Couldn't open {target}: {e}"),
        }
    })
}

/// Initialises COM for the current thread while alive: some shell handlers that
/// `ShellExecuteExW` delegates to are COM objects.
struct ComApartment(bool);

impl ComApartment {
    fn enter() -> Self {
        // SAFETY: balanced by `CoUninitialize` in `drop` whenever it succeeds (S_FALSE included).
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
        Self(hr.is_ok())
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: pairs with the successful `CoInitializeEx` in `enter`, on the same thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// How "Open in editor" opens a file.
#[derive(Debug, PartialEq, Eq)]
pub enum EditorLaunch {
    /// Run a program with arguments.
    Run { program: String, args: Vec<String> },
    /// No editor was found: show the file in Explorer rather than run it.
    Reveal(String),
}

/// How to open `path` at `line` in the editor the user chose. `path` must be an absolute local
/// path, so it can't be taken for an option.
///
/// - A custom command is split into words like a command line, `{path}` and `{line}` are filled
///   in, and the path is appended when the command has no `{path}`.
/// - Auto runs `code --goto "path:line"` when VS Code's CLI (`code`) was found, else reveals the
///   file in Explorer.
pub fn editor_launch(
    pref: &EditorPref,
    path: &str,
    line: u32,
    vscode: Option<&Path>,
) -> Result<EditorLaunch, String> {
    if path.starts_with('-') {
        return Err(format!("{path} looks like an option, not a file"));
    }
    let path = local_path(path).ok_or_else(|| not_local(path))?;
    match pref {
        EditorPref::Custom { command } => {
            let mut words = split_command(command);
            if words.is_empty() {
                return Err("The editor command in Preferences is empty".to_owned());
            }
            let line = line.to_string();
            let has_path = words.iter().any(|w| w.contains("{path}"));
            for word in &mut words {
                // `{line}` first: the path itself is never searched for placeholders.
                *word = word.replace("{line}", &line).replace("{path}", path);
            }
            if !has_path {
                words.push(path.to_owned());
            }
            let program = words.remove(0);
            Ok(EditorLaunch::Run {
                program,
                args: words,
            })
        }
        EditorPref::Auto => Ok(match vscode {
            Some(code) => EditorLaunch::Run {
                program: code.to_string_lossy().into_owned(),
                args: vec!["--goto".to_owned(), format!("{path}:{line}")],
            },
            None => EditorLaunch::Reveal(path.to_owned()),
        }),
    }
}

/// Splits a command line into words at whitespace outside double quotes. Quotes group and are
/// dropped; there are no escapes, so `"C:\Program Files\x.exe"` stays one word.
pub fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut in_word) = (false, false);
    for c in command.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                in_word = true;
            }
            c if c.is_whitespace() && !quoted => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// Starts the editor (or Explorer) without waiting for it. Programs run without a console
/// window; a bare name such as `code` is looked up on `PATH` with `PATHEXT`, so `.cmd` shims work.
pub fn launch_editor(launch: EditorLaunch) -> Result<(), String> {
    match launch {
        EditorLaunch::Reveal(path) => reveal_in_explorer(&path),
        EditorLaunch::Run { program, args } => {
            let program = resolve_program(&program);
            Command::new(&program)
                .args(&args)
                .creation_flags(CREATE_NO_WINDOW.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map(drop)
                .map_err(|e| format!("Couldn't run {}: {e}", program.display()))
        }
    }
}

fn resolve_program(program: &str) -> PathBuf {
    let path = Path::new(program);
    let bare = path.components().count() == 1 && path.extension().is_none();
    match bare.then(|| find_on_path(program)).flatten() {
        Some(found) => found,
        None => path.to_path_buf(),
    }
}

/// VS Code's command-line launcher, found on `PATH` once and cached.
pub fn vscode_cli() -> Option<PathBuf> {
    static CODE_CLI: OnceLock<Option<PathBuf>> = OnceLock::new();
    CODE_CLI.get_or_init(|| find_on_path("code")).clone()
}

/// The first `PATH` entry holding `name` with one of the `PATHEXT` extensions, as `where` finds
/// it. Touches the file system, so call it off the main thread.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let exts = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    let dirs = env::var_os("PATH")?;
    env::split_paths(&dirs).find_map(|dir| {
        exts.split(';')
            .filter(|ext| !ext.is_empty())
            .map(|ext| dir.join(format!("{name}{ext}")))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_absolute_local_paths_are_accepted() {
        for good in [
            r"C:\x\a.png",
            "C:/x/a.png",
            r"S:\Notes\My Vault\dev\a b.md",
            r"\\nas\share\a.pdf",
            r"\\wsl.localhost\Ubuntu\home\me\a.md",
            r"C:\notes\résumé notes.md",
        ] {
            assert_eq!(local_path(good), Some(good), "{good}");
        }
        for bad in [
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "file:///C:/x",
            "calc",
            r"relative\x.png",
            "-x.js",
            r"\x.png",
            r"\\.\pipe\x",
            r"\\?\C:\x.png",
            r"\\server",
            r"\\server\",
            r"C:\x\evil.exe:x.png",
            "C:\\x\\\"a.png",
            r"C:\x\a?.png",
            "C:\\x\\a\n.png",
            "C:",
            "",
        ] {
            assert_eq!(local_path(bad), None, "{bad}");
        }
    }

    #[test]
    fn files_are_sorted_into_markdown_code_viewable_and_other() {
        assert_eq!(file_kind(r"C:\x\a.md"), FileKind::Markdown);
        assert_eq!(file_kind(r"C:\x\a.MARKDOWN"), FileKind::Markdown);
        assert_eq!(file_kind(r"C:\x\a.mdown"), FileKind::Markdown);
        assert_eq!(file_kind(r"C:\x\a.MKD"), FileKind::Markdown);
        assert_eq!(file_kind(r"C:\x\a.js"), FileKind::Code);
        assert_eq!(file_kind(r"C:\x\a.py"), FileKind::Code);
        assert_eq!(file_kind(r"C:\x\.env"), FileKind::Code);
        assert_eq!(file_kind(r"C:\x\a.PNG"), FileKind::Viewable);
        assert_eq!(file_kind(r"C:\x\a.html"), FileKind::Viewable);
        for other in [
            "a.exe",
            "a.com",
            "a.bat",
            "a.cmd",
            "a.ps1",
            "a.vbs",
            "a.vbe",
            "a.wsf",
            "a.wsh",
            "a.msi",
            "a.msc",
            "a.scr",
            "a.lnk",
            "a.url",
            "a.hta",
            "a.cpl",
            "a.reg",
            "a.pif",
            "a.jar",
            "a.application",
            "a.appref-ms",
            "Makefile",
            "a.unknown",
        ] {
            assert_eq!(
                file_kind(&format!(r"C:\x\{other}")),
                FileKind::Other,
                "{other}"
            );
        }
    }

    #[test]
    fn only_web_and_mail_links_count_as_links() {
        assert_eq!(
            web_link("https://x.dev/a?b#c").as_deref(),
            Some("https://x.dev/a?b#c")
        );
        assert_eq!(web_link("HTTP://x.dev").as_deref(), Some("HTTP://x.dev"));
        assert_eq!(web_link("mailto:a@b.c").as_deref(), Some("mailto:a@b.c"));
        assert_eq!(web_link("//x.dev/a").as_deref(), Some("https://x.dev/a"));
        for bad in [
            "file:///C:/x",
            "javascript:alert(1)",
            "ms-settings:",
            "search-ms:x",
            "x",
        ] {
            assert_eq!(web_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_shell_refuses_anything_but_viewable_local_files_and_web_links() {
        // Each refusal happens before the shell is reached.
        assert!(open_file(r"C:\Windows\System32\calc.exe").is_err());
        assert!(open_file(r"C:\x\a.js").is_err());
        assert!(open_file("ms-msdt:x").is_err());
        assert!(open_url("search-ms:x").is_err());
        assert!(open_url(r"C:\x\a.png").is_err());
    }

    #[test]
    fn reveal_refuses_quotes_and_anything_not_local() {
        assert!(reveal_in_explorer("C:\\x\\a\" /e,C:\\Windows.png").is_err());
        assert!(reveal_in_explorer("calc").is_err());
        assert!(reveal_in_explorer("file:///C:/x").is_err());
    }

    #[test]
    fn split_command_keeps_quoted_words_together() {
        assert_eq!(
            split_command(r#""C:\Program Files\Notepad++\notepad++.exe" -n{line} "{path}""#),
            vec![
                r"C:\Program Files\Notepad++\notepad++.exe",
                "-n{line}",
                "{path}"
            ]
        );
        assert_eq!(split_command("  code   --goto  "), vec!["code", "--goto"]);
        assert_eq!(split_command(r#"a "" b"#), vec!["a", "", "b"]);
        assert!(split_command("   ").is_empty());
    }

    #[test]
    fn custom_editor_fills_in_path_and_line() {
        let pref = EditorPref::Custom {
            command: r#""C:\Tools\ed.exe" +{line} "{path}""#.to_owned(),
        };
        assert_eq!(
            editor_launch(&pref, r"S:\Notes\My Vault\a b.rb", 17, None).unwrap(),
            EditorLaunch::Run {
                program: r"C:\Tools\ed.exe".to_owned(),
                args: vec!["+17".to_owned(), r"S:\Notes\My Vault\a b.rb".to_owned()],
            }
        );
    }

    #[test]
    fn custom_editor_without_a_path_placeholder_gets_the_path_appended() {
        let pref = EditorPref::Custom {
            command: "subl".to_owned(),
        };
        assert_eq!(
            editor_launch(&pref, r"C:\{line}.md", 3, None).unwrap(),
            EditorLaunch::Run {
                program: "subl".to_owned(),
                args: vec![r"C:\{line}.md".to_owned()],
            }
        );
        let empty = EditorPref::Custom {
            command: "  ".to_owned(),
        };
        assert!(editor_launch(&empty, r"C:\a.md", 1, None).is_err());
    }

    #[test]
    fn auto_editor_uses_vscode_when_found_else_reveals() {
        let code = Path::new(r"C:\VS Code\bin\code.cmd");
        assert_eq!(
            editor_launch(&EditorPref::Auto, r"C:\a b\x.rs", 9, Some(code)).unwrap(),
            EditorLaunch::Run {
                program: r"C:\VS Code\bin\code.cmd".to_owned(),
                args: vec!["--goto".to_owned(), r"C:\a b\x.rs:9".to_owned()],
            }
        );
        assert_eq!(
            editor_launch(&EditorPref::Auto, r"C:\x.js", 1, None).unwrap(),
            EditorLaunch::Reveal(r"C:\x.js".to_owned())
        );
    }

    #[test]
    fn the_editor_refuses_options_and_anything_not_local() {
        let code = Some(Path::new(r"C:\VS Code\bin\code.cmd"));
        for bad in [
            "-x.js",
            "--disable-extensions",
            "calc",
            r"rel\x.rs",
            "ms-msdt:x",
        ] {
            assert!(
                editor_launch(&EditorPref::Auto, bad, 1, code).is_err(),
                "{bad}"
            );
            let custom = EditorPref::Custom {
                command: "ed {path}".to_owned(),
            };
            assert!(editor_launch(&custom, bad, 1, None).is_err(), "{bad}");
        }
    }
}
