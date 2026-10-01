use std::fs;
use std::sync::Arc;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use lectern_core::perf::PerfLog;

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
        * 1000.0
}

fn lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn mark_appends_one_json_line_per_event() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("perf.jsonl");
    let start = now_ms() - 250.0;
    let log = PerfLog::new(Some(path.clone()), start);
    log.mark("window-created", None);
    log.mark("first-paint", Some(12.5));

    let events = lines(&path);
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["event"], "window-created");
    assert!(events[0]["extraMs"].is_null());
    assert_eq!(events[1]["event"], "first-paint");
    assert_eq!(events[1]["extraMs"], 12.5);
    for e in &events {
        let since = e["sinceStartMs"].as_f64().unwrap();
        let unix = e["unixMs"].as_f64().unwrap();
        assert!((250.0..60_000.0).contains(&since), "{e}");
        assert!((unix - start - since).abs() < 1.0, "{e}");
    }
}

#[test]
fn mark_appends_to_an_existing_log() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("perf.jsonl");
    PerfLog::new(Some(path.clone()), now_ms()).mark("run-1", None);
    PerfLog::new(Some(path.clone()), now_ms()).mark("run-2", None);
    let events = lines(&path);
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["event"], "run-2");
}

#[test]
fn marks_from_many_threads_stay_whole_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("perf.jsonl");
    let log = Arc::new(PerfLog::new(Some(path.clone()), now_ms()));
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let log = Arc::clone(&log);
            thread::spawn(move || {
                for i in 0..50 {
                    log.mark(&format!("t{t}-{i}"), Some(f64::from(i)));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(lines(&path).len(), 400);
}

#[test]
fn without_a_path_mark_is_a_no_op() {
    let log = PerfLog::new(None, now_ms());
    log.mark("ignored", None);
}

#[test]
fn since_start_counts_from_process_start() {
    let log = PerfLog::new(None, now_ms() - 1000.0);
    let since = log.since_start_ms();
    assert!((1000.0..2000.0).contains(&since), "{since}");
}
