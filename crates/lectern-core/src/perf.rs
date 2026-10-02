//! Perf marks for the startup benchmarks, appended as JSON lines to the `--perf-log` file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

pub struct PerfLog {
    /// `None` without `--perf-log`, or when the file couldn't be opened.
    file: Option<Mutex<File>>,
    process_start_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Mark<'a> {
    event: &'a str,
    since_start_ms: f64,
    extra_ms: Option<f64>,
    unix_ms: f64,
}

impl PerfLog {
    /// A log appending to `path`; marks are dropped when `path` is `None` or can't be opened.
    /// `process_start_unix_ms` is when the process started, in Unix milliseconds.
    pub fn new(path: Option<PathBuf>, process_start_unix_ms: f64) -> Self {
        let file =
            path.and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
        Self {
            file: file.map(Mutex::new),
            process_start_ms: process_start_unix_ms,
        }
    }

    /// Appends `{"event", "sinceStartMs", "extraMs", "unixMs"}` as one line. Safe to call from
    /// several threads: each line is written whole, under a lock.
    pub fn mark(&self, event: &str, extra_ms: Option<f64>) {
        let Some(file) = &self.file else {
            return;
        };
        let unix_ms = unix_ms();
        let mark = Mark {
            event,
            since_start_ms: unix_ms - self.process_start_ms,
            extra_ms,
            unix_ms,
        };
        let Ok(mut line) = serde_json::to_vec(&mark) else {
            return;
        };
        line.push(b'\n');
        // The file is unbuffered, so a mark is on disk even if the process exits right after.
        if let Ok(mut file) = file.lock() {
            let _ = file.write_all(&line);
        }
    }

    /// Milliseconds since the process started.
    pub fn since_start_ms(&self) -> f64 {
        unix_ms() - self.process_start_ms
    }
}

fn unix_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0
}
