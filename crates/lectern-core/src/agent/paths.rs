//! Paths from agents. An agent in WSL writes `/mnt/s/Notes/plan.md` or `/home/me/notes/plan.md`,
//! one on Windows `S:\Notes\plan.md`; either may carry a `:12` or `#rollout` after it. Lectern
//! opens the Windows form, and answers with both.
//!
//! **Shares.** An agent's path on a network share is taken only when Lectern already trusts the
//! share's host (a library's, a path mapping's, WSL's, or one the user opened a file from), and
//! that is checked on the text alone, before anything touches the path. This differs from the
//! user's own opens: a file the user launches Lectern with, or opens from Explorer or the file
//! dialog, makes its host trusted for the session, because the user chose it. An agent's path may
//! come from text a note fed it (prompt injection), and Windows hands any SMB host the user's
//! credentials (NTLM), so an agent never makes a host trusted.

use crate::ipc::PathForms;
use crate::library::pathmap::{
    is_absolute, line_fragment, split_path_suffix, unc_host, Mapped, PathMapper,
};

/// A path an agent gave: both its forms, and the line or anchor written after it, kept for a
/// target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPath {
    /// The file on this machine, as Lectern opens it.
    pub windows: String,
    /// Where a WSL shell finds it; none for a share no mapping reaches from WSL.
    pub wsl: Option<String>,
    /// From a `:12`, `:12:5` or `#L12` after the path.
    pub line: Option<u32>,
    /// From a `#rollout` after the path.
    pub anchor: Option<String>,
}

impl AgentPath {
    /// Both forms, as an answer gives them.
    pub fn forms(&self) -> PathForms {
        PathForms {
            windows: Some(self.windows.clone()),
            wsl: self.wsl.clone(),
        }
    }
}

/// The path an agent wrote, `raw`, read as text (`split_path_suffix`), its quotes and the line or
/// anchor after it set apart, then mapped as a link is (`PathMapper::map`, without the index): any
/// Linux path goes into WSL, not only one under a Linux system folder such as `/home`.
/// Mapped onto a share, its host must be one of `trusted`, the hosts Lectern already trusts (in
/// lower case; see the top of this file). Nothing here touches the file system. The error is a
/// sentence for the agent: the path isn't a full one, Lectern can't reach it from Windows, or its
/// share isn't one Lectern trusts.
pub fn agent_path(mapper: &PathMapper, raw: &str, trusted: &[String]) -> Result<AgentPath, String> {
    let (path, suffix) = split_path_suffix(raw);
    if is_device(path) {
        return Err(format!(
            "“{}” is a device path, which Lectern doesn't open. Give a Windows path \
             (S:\\Notes\\plan.md) or a WSL path (/mnt/s/Notes/plan.md).",
            raw.trim()
        ));
    }
    if !is_absolute(path) {
        return Err(format!(
            "“{}” isn't a full path. Give a Windows path (S:\\Notes\\plan.md) or a WSL path \
             (/mnt/s/Notes/plan.md).",
            raw.trim()
        ));
    }
    let windows = match mapper.map(path, None) {
        Mapped::Verified(windows) | Mapped::Unverified(windows) => windows,
        Mapped::Unresolved => return Err(format!("Lectern can't reach {path} from Windows.")),
    };
    let wsl = mapper.to_wsl(&windows);
    let windows = windows.to_string_lossy().into_owned();
    if let Some(host) = unc_host(&windows).filter(|host| !trusted.contains(host)) {
        return Err(untrusted_share(&host));
    }
    let (line, anchor) = match suffix.strip_prefix('#') {
        Some(fragment) => match line_fragment(fragment) {
            Some(line) => (Some(line), None),
            None => (None, (!fragment.is_empty()).then(|| fragment.to_owned())),
        },
        None => (
            suffix
                .strip_prefix(':')
                .and_then(|rest| rest.split(':').next())
                .and_then(|line| line.parse().ok()),
            None,
        ),
    };
    Ok(AgentPath {
        windows,
        wsl,
        line,
        anchor,
    })
}

/// The error for an agent's path on the share of `host`, which Lectern doesn't trust.
fn untrusted_share(host: &str) -> String {
    format!(
        r"Lectern doesn't open files on \\{host} for an agent; add that share as a library folder first."
    )
}

/// Whether `path` is in one of Windows' device namespaces, `\\.\…` or `\\?\…`, written with
/// either slash: a pipe, a drive, or a path that skips Windows' checks, none of them a file to
/// read.
fn is_device(path: &str) -> bool {
    let start: String = path
        .chars()
        .take(4)
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    start == r"\\.\" || start == r"\\?\"
}
