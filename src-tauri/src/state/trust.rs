//! Which network hosts Lectern may reach. Windows answers any SMB host with the user's NTLM
//! credentials, so a path that comes from a note may only lead to a host the user chose: a
//! library root's (its canonical form included), a path mapping's, WSL's, or one the user opened a
//! file from by launching Lectern with it. Local paths are always fine.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lectern_core::ipc::Settings;
use lectern_core::library::path_key;
use lectern_core::library::pathmap::unc_host;

/// Trusted whatever the settings say.
const ALWAYS: &[&str] = &["wsl.localhost", "wsl$"];

#[derive(Debug, Default)]
pub struct Trust {
    /// Hosts of the configured roots and mapping targets, and WSL's.
    configured: HashSet<String>,
    /// Each configured root's canonical host, by the root's path key, learnt once the root has
    /// answered (`S:\…` turns out to be `\\nas\share\…`).
    canonical: HashMap<String, String>,
    /// Hosts of files the user launched Lectern with, for this session.
    opened: HashSet<String>,
}

impl Trust {
    pub fn new(settings: &Settings) -> Self {
        let mut trust = Self::default();
        trust.configure(settings);
        trust
    }

    /// Takes the roots and mappings from `settings`, forgetting the canonical hosts of roots that
    /// are gone. True when the trusted hosts changed.
    pub fn configure(&mut self, settings: &Settings) -> bool {
        let before = self.hosts();
        self.configured = settings
            .library_roots
            .iter()
            .chain(settings.path_mappings.iter().map(|m| &m.to))
            .filter_map(|path| network_host(path))
            .chain(ALWAYS.iter().map(|host| (*host).to_owned()))
            .collect();
        let roots: HashSet<String> = settings
            .library_roots
            .iter()
            .map(|root| path_key(Path::new(root)))
            .collect();
        self.canonical.retain(|root, _| roots.contains(root));
        self.hosts() != before
    }

    /// Notes the host of `canonical`, the canonical form of the configured root `root`. True
    /// when that host wasn't trusted before.
    pub fn learn_root(&mut self, root: &Path, canonical: &Path) -> bool {
        let Some(host) = network_host(&canonical.to_string_lossy()) else {
            return false;
        };
        let fresh = !self.allows_host(&host);
        self.canonical.insert(path_key(root), host);
        fresh
    }

    /// Trusts the host of a file or folder the user launched Lectern with.
    pub fn opened_by_user(&mut self, path: &Path) {
        if let Some(host) = network_host(&path.to_string_lossy()) {
            self.opened.insert(host);
        }
    }

    fn allows_host(&self, host: &str) -> bool {
        self.configured.contains(host)
            || self.opened.contains(host)
            || self.canonical.values().any(|h| h == host)
    }

    /// Whether Lectern may touch `path`: a local path, or one on a trusted host.
    pub fn allows(&self, path: &str) -> bool {
        unc_host(path).is_none_or(|host| self.allows_host(&host))
    }

    /// Every trusted host, sorted, for rendering.
    pub fn hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .configured
            .iter()
            .chain(self.opened.iter())
            .chain(self.canonical.values())
            .cloned()
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
    }
}

/// The host of a UNC path, unless it is a verbatim or device path (`\\?\…`, `\\.\…`), whose
/// `?` or `.` never names a host to trust: `\\?\C:\x` is how Windows canonicalises a local
/// folder.
fn network_host(path: &str) -> Option<String> {
    unc_host(path).filter(|host| host != "?" && host != ".")
}

/// The message for refusing `path` because its host isn't trusted.
pub fn refusal(path: &str) -> String {
    let host = unc_host(path).unwrap_or_default();
    format!(
        r"Lectern doesn't open files on \\{host} because it isn't one of your library locations."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lectern_core::ipc::PathMapping;
    use std::path::PathBuf;

    fn settings(roots: &[&str], mapping_targets: &[&str]) -> Settings {
        Settings {
            library_roots: roots.iter().map(|r| (*r).to_owned()).collect(),
            path_mappings: mapping_targets
                .iter()
                .map(|to| PathMapping {
                    from: "/home/me/shared".to_owned(),
                    to: (*to).to_owned(),
                })
                .collect(),
            ..Settings::default()
        }
    }

    #[test]
    fn roots_mappings_and_wsl_are_trusted_and_nothing_else() {
        let trust = Trust::new(&settings(
            &[r"\\NAS\Share\dev", r"S:\Notes\My Vault\dev"],
            &[r"\\nas2\notes"],
        ));
        assert!(trust.allows(r"\\nas\share\dev\a.md"));
        assert!(trust.allows(r"\\NAS2\notes\b.png"));
        assert!(trust.allows(r"\\wsl.localhost\Ubuntu\home\a.md"));
        assert!(trust.allows(r"\\wsl$\Ubuntu\home\a.md"));
        assert!(trust.allows(r"C:\anything\x.exe"));
        assert!(trust.allows("/home/me/a.md"));
        assert!(!trust.allows(r"\\attacker.example\s\x.png"));
        assert!(!trust.allows(r"\\?\UNC\attacker\s\x.png"));
        assert!(!trust.allows(r"\\.\pipe\x"));
    }

    #[test]
    fn a_roots_canonical_host_is_trusted_until_the_root_goes() {
        let mut trust = Trust::new(&settings(&[r"S:\Notes\My Vault\dev"], &[]));
        let root = Path::new(r"S:\Notes\My Vault\dev");
        assert!(!trust.allows(r"\\nas\share\x.png"));
        let canonical = PathBuf::from(r"\\?\UNC\nas\share\Notes\My Vault\dev");
        assert!(trust.learn_root(root, &canonical));
        assert!(!trust.learn_root(root, &canonical));
        assert!(trust.allows(r"\\nas\share\x.png"));
        assert!(trust.configure(&settings(&[], &[])));
        assert!(!trust.allows(r"\\nas\share\x.png"));
    }

    #[test]
    fn a_local_roots_canonical_form_trusts_no_host() {
        let mut trust = Trust::new(&settings(&[r"C:\notes"], &[]));
        assert!(!trust.learn_root(Path::new(r"C:\notes"), Path::new(r"\\?\C:\notes")));
        assert!(!trust.allows(r"\\?\C:\x"));
        assert_eq!(trust.hosts(), ["wsl$", "wsl.localhost"]);
    }

    #[test]
    fn a_host_the_user_launched_a_file_from_is_trusted() {
        let mut trust = Trust::new(&settings(&[], &[]));
        trust.opened_by_user(Path::new(r"\\fileserver\docs\plan.md"));
        assert!(trust.allows(r"\\FileServer\docs\other.md"));
        assert!(!trust.allows(r"\\elsewhere\docs\other.md"));
    }

    #[test]
    fn the_refusal_names_the_host() {
        assert_eq!(
            refusal(r"\\Attacker.example\s\x.png"),
            r"Lectern doesn't open files on \\attacker.example because it isn't one of your library locations."
        );
    }
}
