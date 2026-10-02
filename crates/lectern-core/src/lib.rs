//! Lectern's core logic. Pure Rust with no Tauri dependency, so it builds and tests on Linux.

pub mod cache;
pub mod cli;
pub mod frontmatter;
pub mod ipc;
pub mod library;
pub mod perf;
pub mod render;
pub mod search;
pub mod store;
pub mod text;
pub mod watch;

/// The Lectern version, taken from the workspace manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::version;

    #[test]
    fn version_matches_the_workspace_manifest() {
        assert_eq!(version(), "0.1.0");
    }
}
