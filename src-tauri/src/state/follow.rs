//! Following a link the user clicked. Targets come from document HTML, so `plan_follow` never
//! trusts them: see `shell.rs` for what may be opened, edited or only revealed.

use std::fs;
use std::path::Path;
use std::time::Duration;

use lectern_core::ipc::{EditorPref, FollowKind, FollowResult, FollowTarget};
use lectern_core::library::pathmap::Mapped;
use lectern_core::library::scan::probe_with;

use super::paths::path_string;
use super::sync::read;
use super::{trust, WindowState};
use crate::shell::{self, FileKind};

/// What following a link comes to, before anything is launched.
#[derive(Debug, PartialEq, Eq)]
pub enum FollowPlan {
    OpenDoc {
        path: String,
        anchor: Option<String>,
        line: Option<u32>,
    },
    /// A viewable file (image, PDF, …), with its default app.
    OpenFile(String),
    /// An http, https or mailto link.
    OpenUrl(String),
    /// Code or text, in the editor.
    Editor {
        path: String,
        line: u32,
    },
    /// Anything else, shown in Explorer and never run.
    Reveal(String),
    NotFound(String),
}

/// Decides how to follow `target`, which comes from document HTML and so is never trusted. `map`
/// maps an absolute path from a note to this machine; `trusted` says whether a path's network
/// host may be reached; `exists` checks a mapped path that the index doesn't vouch for. The
/// anchor is passed through as written.
///
/// Docs, files and paths must come to an absolute local path, on a trusted host when it is a UNC
/// path (checked before anything touches it: reaching an SMB host hands it the user's
/// credentials). Markdown opens in the reader, code and text in the editor (line 1 without a
/// cited line), viewable files with their default app, and anything else is only revealed.
pub fn plan_follow(
    target: &FollowTarget,
    map: impl FnOnce(&str) -> Mapped,
    trusted: impl FnOnce(&str) -> bool,
    exists: impl FnOnce(&Path) -> bool,
) -> FollowPlan {
    let FollowTarget {
        kind,
        target: raw,
        line,
        anchor,
    } = target;
    let local = |path: &str| shell::local_path(path).map(str::to_owned);
    let (path, unverified) = match kind {
        FollowKind::Doc | FollowKind::File => (local(raw), false),
        FollowKind::Path => match map(raw) {
            Mapped::Verified(path) => (local(&path_string(&path)), false),
            Mapped::Unverified(path) => (local(&path_string(&path)), true),
            Mapped::Unresolved => (None, false),
        },
        FollowKind::External => {
            return match shell::web_link(raw) {
                Some(url) => FollowPlan::OpenUrl(url),
                None => FollowPlan::NotFound(format!(
                    "Lectern only opens http, https and mailto links, not {raw}"
                )),
            };
        }
        FollowKind::Broken => return FollowPlan::NotFound(format!("Couldn't resolve {raw}")),
    };
    let Some(path) = path else {
        return FollowPlan::NotFound(format!("Couldn't find {raw}"));
    };
    if !trusted(&path) {
        return FollowPlan::NotFound(trust::refusal(&path));
    }
    if unverified && !exists(Path::new(&path)) {
        return FollowPlan::NotFound(format!("Couldn't find {raw}"));
    }
    match (kind, shell::file_kind(&path)) {
        (FollowKind::Doc, _) | (_, FileKind::Markdown) => FollowPlan::OpenDoc {
            path,
            anchor: anchor.clone(),
            line: *line,
        },
        (_, FileKind::Code) => FollowPlan::Editor {
            path,
            line: line.unwrap_or(1),
        },
        (_, FileKind::Viewable) => FollowPlan::OpenFile(path),
        (_, FileKind::Other) => FollowPlan::Reveal(path),
    }
}

impl WindowState {
    pub fn follow(&self, target: &FollowTarget) -> FollowResult {
        let index = self.index();
        let mapper = self.mapper();
        let index = (!index.roots.is_empty()).then_some(&*index);
        let exists_timeout = self.app.timings.exists;
        let plan = plan_follow(
            target,
            |raw| mapper.map(raw, index),
            |path| self.trusts(path),
            |path| exists_within(path, exists_timeout),
        );
        let launched = match plan {
            FollowPlan::OpenDoc { path, anchor, line } => {
                return FollowResult::OpenDoc { path, anchor, line };
            }
            FollowPlan::NotFound(message) => return FollowResult::NotFound { message },
            FollowPlan::OpenFile(path) => shell::open_file(&path),
            FollowPlan::OpenUrl(url) => shell::open_url(&url),
            FollowPlan::Editor { path, line } => self.open_in_editor(&path, Some(line)),
            FollowPlan::Reveal(path) => self.reveal_in_explorer(&path),
        };
        match launched {
            Ok(()) => FollowResult::Opened,
            Err(message) => FollowResult::NotFound { message },
        }
    }

    /// Opens a local file in the editor (VS Code, a custom command, or else Explorer), at `line`
    /// or line 1.
    pub fn open_in_editor(&self, path: &str, line: Option<u32>) -> Result<(), String> {
        if !self.trusts(path) {
            return Err(trust::refusal(path));
        }
        let pref = read(&self.app.settings).editor.clone();
        let vscode = match pref {
            EditorPref::Auto => shell::vscode_cli(),
            EditorPref::Custom { .. } => None,
        };
        let launch = shell::editor_launch(&pref, path, line.unwrap_or(1), vscode.as_deref())?;
        shell::launch_editor(launch)
    }

    /// Shows a local file in Explorer; one on an untrusted network host is refused.
    pub fn reveal_in_explorer(&self, path: &str) -> Result<(), String> {
        if !self.trusts(path) {
            return Err(trust::refusal(path));
        }
        shell::reveal_in_explorer(path)
    }
}

pub(super) fn exists_within(path: &Path, timeout: Duration) -> bool {
    let path = path.to_path_buf();
    probe_with(move || fs::metadata(&path).map(drop), timeout).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn target(
        kind: FollowKind,
        raw: &str,
        line: Option<u32>,
        anchor: Option<&str>,
    ) -> FollowTarget {
        FollowTarget {
            kind,
            target: raw.to_owned(),
            line,
            anchor: anchor.map(str::to_owned),
        }
    }

    fn unmapped(_: &str) -> Mapped {
        panic!("only paths are mapped")
    }

    fn unchecked(_: &Path) -> bool {
        panic!("existence was checked")
    }

    fn trust_all(_: &str) -> bool {
        true
    }

    #[test]
    fn follow_refuses_untrusted_network_hosts_before_touching_them() {
        let refusal = trust::refusal(r"\\attacker.example\s\x.png");
        let untrusted = |p: &str| !p.starts_with(r"\\attacker.example");
        for kind in [FollowKind::Doc, FollowKind::File] {
            let t = target(kind, r"\\attacker.example\s\x.png", None, None);
            assert_eq!(
                plan_follow(&t, unmapped, untrusted, unchecked),
                FollowPlan::NotFound(refusal.clone())
            );
        }
        // A mapped path on an untrusted host is refused before the existence check.
        let mapped = |_: &str| Mapped::Unverified(PathBuf::from(r"\\attacker.example\s\x.png"));
        let t = target(FollowKind::Path, "/home/me/x.png", None, None);
        assert_eq!(
            plan_follow(&t, mapped, untrusted, unchecked),
            FollowPlan::NotFound(refusal)
        );
        // A trusted host goes through.
        let t = target(FollowKind::File, r"\\nas\share\a.png", None, None);
        assert_eq!(
            plan_follow(&t, unmapped, untrusted, unchecked),
            FollowPlan::OpenFile(r"\\nas\share\a.png".to_owned())
        );
    }

    #[test]
    fn a_doc_opens_in_the_reader_with_its_anchor_as_written() {
        let t = target(
            FollowKind::Doc,
            r"S:\dev\a b.md",
            Some(4),
            Some("Step 2: Ship It"),
        );
        assert_eq!(
            plan_follow(&t, unmapped, trust_all, unchecked),
            FollowPlan::OpenDoc {
                path: r"S:\dev\a b.md".to_owned(),
                anchor: Some("Step 2: Ship It".to_owned()),
                line: Some(4),
            }
        );
    }

    #[test]
    fn follow_refuses_anything_but_an_absolute_local_path() {
        let bad = [
            "ms-msdt:/id PCWDiagnostic",
            "search-ms:query=x",
            "file:///C:/x",
            "calc",
            r"relative\x.png",
            "-x.js",
        ];
        for raw in bad {
            for kind in [FollowKind::Doc, FollowKind::File] {
                let plan = plan_follow(
                    &target(kind, raw, Some(3), None),
                    unmapped,
                    trust_all,
                    unchecked,
                );
                assert_eq!(
                    plan,
                    FollowPlan::NotFound(format!("Couldn't find {raw}")),
                    "{raw}"
                );
            }
            // Whatever a mapping hands back is checked too, before any existence check.
            let forged = |_: &str| Mapped::Unverified(PathBuf::from(raw));
            let plan = plan_follow(
                &target(FollowKind::Path, raw, None, None),
                forged,
                trust_all,
                unchecked,
            );
            assert!(matches!(plan, FollowPlan::NotFound(_)), "{raw}");
            let forged = |_: &str| Mapped::Verified(PathBuf::from(raw));
            let plan = plan_follow(
                &target(FollowKind::Path, raw, None, None),
                forged,
                trust_all,
                unchecked,
            );
            assert!(matches!(plan, FollowPlan::NotFound(_)), "{raw}");
        }
    }

    #[test]
    fn follow_opens_each_kind_of_file_the_safe_way() {
        let file = |raw: &str, line| {
            plan_follow(
                &target(FollowKind::File, raw, line, None),
                unmapped,
                trust_all,
                unchecked,
            )
        };
        assert_eq!(
            file(r"C:\x\a.exe", None),
            FollowPlan::Reveal(r"C:\x\a.exe".to_owned())
        );
        assert_eq!(
            file(r"C:\x\run.cmd", Some(2)),
            FollowPlan::Reveal(r"C:\x\run.cmd".to_owned())
        );
        assert_eq!(
            file(r"C:\x\a.js", None),
            FollowPlan::Editor {
                path: r"C:\x\a.js".to_owned(),
                line: 1
            }
        );
        assert_eq!(
            file(r"C:\x\app.rb", Some(17)),
            FollowPlan::Editor {
                path: r"C:\x\app.rb".to_owned(),
                line: 17
            }
        );
        assert_eq!(
            file(r"C:\x\a.png", Some(3)),
            FollowPlan::OpenFile(r"C:\x\a.png".to_owned())
        );
        assert_eq!(
            file(r"C:\x\a.md", Some(3)),
            FollowPlan::OpenDoc {
                path: r"C:\x\a.md".to_owned(),
                anchor: None,
                line: Some(3)
            }
        );
        let exe = |_: &str| Mapped::Verified(PathBuf::from(r"C:\tools\a.exe"));
        assert_eq!(
            plan_follow(
                &target(FollowKind::Path, "/opt/a.exe", None, None),
                exe,
                trust_all,
                unchecked
            ),
            FollowPlan::Reveal(r"C:\tools\a.exe".to_owned())
        );
    }

    #[test]
    fn a_verified_path_is_followed_without_an_existence_check() {
        let t = target(
            FollowKind::Path,
            "/home/me/shared/dev/work/a/README.md",
            None,
            Some("Status"),
        );
        let map = |raw: &str| {
            assert_eq!(raw, "/home/me/shared/dev/work/a/README.md");
            Mapped::Verified(PathBuf::from(r"S:\Notes\My Vault\dev\work\a\README.md"))
        };
        assert_eq!(
            plan_follow(&t, map, trust_all, unchecked),
            FollowPlan::OpenDoc {
                path: r"S:\Notes\My Vault\dev\work\a\README.md".to_owned(),
                anchor: Some("Status".to_owned()),
                line: None,
            }
        );
    }

    #[test]
    fn an_unverified_path_is_followed_only_when_it_exists() {
        let mapped = |_: &str| Mapped::Unverified(PathBuf::from(r"C:\code\x.py"));
        let t = target(FollowKind::Path, "/mnt/c/code/x.py", Some(8), None);
        assert_eq!(
            plan_follow(&t, mapped, trust_all, |_| true),
            FollowPlan::Editor {
                path: r"C:\code\x.py".to_owned(),
                line: 8
            }
        );
        let t = target(FollowKind::Path, "/mnt/c/code/x.py", None, None);
        assert_eq!(
            plan_follow(&t, mapped, trust_all, |p| {
                assert_eq!(p, Path::new(r"C:\code\x.py"));
                false
            }),
            FollowPlan::NotFound("Couldn't find /mnt/c/code/x.py".to_owned())
        );
    }

    #[test]
    fn an_unresolved_path_is_not_found() {
        let t = target(FollowKind::Path, "/opt/elsewhere/a.md", None, None);
        assert_eq!(
            plan_follow(&t, |_| Mapped::Unresolved, trust_all, unchecked),
            FollowPlan::NotFound("Couldn't find /opt/elsewhere/a.md".to_owned())
        );
    }

    #[test]
    fn only_web_and_mail_links_go_out() {
        let plan = |raw: &str| {
            plan_follow(
                &target(FollowKind::External, raw, None, None),
                unmapped,
                trust_all,
                unchecked,
            )
        };
        assert_eq!(
            plan("https://x.dev/a?b#c"),
            FollowPlan::OpenUrl("https://x.dev/a?b#c".to_owned())
        );
        assert_eq!(
            plan("mailto:a@b.c"),
            FollowPlan::OpenUrl("mailto:a@b.c".to_owned())
        );
        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "ms-msdt:x",
        ] {
            assert!(matches!(plan(bad), FollowPlan::NotFound(_)), "{bad}");
        }
    }

    #[test]
    fn a_broken_link_is_not_found() {
        let t = target(FollowKind::Broken, "Missing Note", None, None);
        assert_eq!(
            plan_follow(&t, unmapped, trust_all, unchecked),
            FollowPlan::NotFound("Couldn't resolve Missing Note".to_owned())
        );
    }
}
