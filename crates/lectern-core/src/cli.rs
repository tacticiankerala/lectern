//! Command-line arguments: a document path, plus the flags the perf harness passes.

use std::path::PathBuf;

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    /// The first argument that isn't a flag.
    pub path: Option<PathBuf>,
    /// `--perf-log <file>`: append perf marks to this file as JSON lines.
    pub perf_log: Option<PathBuf>,
    /// `--exit-after-paint`: quit once the first document has painted.
    pub exit_after_paint: bool,
    /// `--perf-t0 <unix ms>`: when the harness launched the process.
    pub perf_t0_ms: Option<f64>,
}

impl Args {
    /// Parses `argv`, skipping the program name. Value flags take `--flag value` or
    /// `--flag=value`. Unknown flags (Tauri and WebView2 pass their own) are skipped on their own,
    /// so `--unknown C:\a.md` still opens `C:\a.md`; an unknown flag's value must use `=`.
    pub fn parse<I: IntoIterator<Item = String>>(argv: I) -> Args {
        let mut args = Args::default();
        let mut rest = argv.into_iter().skip(1).peekable();
        while let Some(arg) = rest.next() {
            if !arg.starts_with('-') {
                if args.path.is_none() && !arg.is_empty() {
                    args.path = Some(PathBuf::from(arg));
                }
                continue;
            }
            let (flag, inline) = match arg.split_once('=') {
                Some((flag, value)) => (flag, Some(value.to_owned())),
                None => (arg.as_str(), None),
            };
            // A value flag's value is the inline `=value`, else the next argument unless that is
            // a flag itself.
            let value = || inline.or_else(|| rest.next_if(|next| !next.starts_with("--")));
            match flag {
                "--perf-log" => args.perf_log = value().map(PathBuf::from),
                "--perf-t0" => args.perf_t0_ms = value().and_then(|v| v.parse().ok()),
                "--exit-after-paint" => args.exit_after_paint = true,
                _ => {}
            }
        }
        args
    }
}
