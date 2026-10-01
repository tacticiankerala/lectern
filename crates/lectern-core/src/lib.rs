//! Lectern's core logic. Pure Rust with no Tauri dependency, so it builds and tests on Linux.

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
