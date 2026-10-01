use std::path::PathBuf;

use lectern_core::cli::Args;

fn parse(argv: &[&str]) -> Args {
    Args::parse(argv.iter().map(|s| s.to_string()))
}

#[test]
fn args_parse() {
    let a = Args::parse(
        [
            "lectern.exe",
            "C:\\n\\a.md",
            "--perf-log",
            "C:\\t\\p.jsonl",
            "--exit-after-paint",
            "--perf-t0=123.5",
            "--unknown",
        ]
        .map(String::from),
    );
    assert_eq!(a.path.unwrap(), PathBuf::from("C:\\n\\a.md"));
    assert_eq!(a.perf_log, Some(PathBuf::from("C:\\t\\p.jsonl")));
    assert!(a.exit_after_paint);
    assert_eq!(a.perf_t0_ms, Some(123.5));
}

#[test]
fn no_args_is_default() {
    assert_eq!(parse(&["lectern.exe"]), Args::default());
    assert_eq!(Args::parse(Vec::<String>::new()), Args::default());
}

#[test]
fn equals_and_space_forms() {
    let a = parse(&[
        "lectern.exe",
        "--perf-log=C:\\t\\p.jsonl",
        "--perf-t0",
        "42",
        "S:\\My Vault\\notes.md",
    ]);
    assert_eq!(a.perf_log, Some(PathBuf::from("C:\\t\\p.jsonl")));
    assert_eq!(a.perf_t0_ms, Some(42.0));
    assert_eq!(a.path, Some(PathBuf::from("S:\\My Vault\\notes.md")));
    assert!(!a.exit_after_paint);
}

#[test]
fn bare_unknown_flag_does_not_swallow_the_path() {
    let a = parse(&["lectern.exe", "--unknown", "C:\\n\\a.md"]);
    assert_eq!(a.path, Some(PathBuf::from("C:\\n\\a.md")));
}

#[test]
fn unknown_flags_with_values_are_ignored() {
    let a = parse(&[
        "lectern.exe",
        "--remote-debugging-port=9222",
        "--enable-features=msWebView2",
        "\\\\nas\\Shared\\résumé.md",
    ]);
    assert_eq!(
        a,
        Args {
            path: Some(PathBuf::from("\\\\nas\\Shared\\résumé.md")),
            ..Args::default()
        }
    );
}

#[test]
fn first_non_flag_argument_is_the_path() {
    let a = parse(&["lectern.exe", "C:\\n\\a.md", "C:\\n\\b.md"]);
    assert_eq!(a.path, Some(PathBuf::from("C:\\n\\a.md")));
}

#[test]
fn a_value_flag_does_not_take_the_next_flag() {
    let a = parse(&["lectern.exe", "--perf-log", "--exit-after-paint"]);
    assert_eq!(a.perf_log, None);
    assert!(a.exit_after_paint);
}

#[test]
fn a_value_flag_at_the_end_is_ignored() {
    let a = parse(&["lectern.exe", "C:\\n\\a.md", "--perf-log"]);
    assert_eq!(a.perf_log, None);
    assert_eq!(a.path, Some(PathBuf::from("C:\\n\\a.md")));
}

#[test]
fn unparseable_t0_is_none() {
    assert_eq!(parse(&["lectern.exe", "--perf-t0=soon"]).perf_t0_ms, None);
    assert_eq!(parse(&["lectern.exe", "--perf-t0", "x"]).perf_t0_ms, None);
}
