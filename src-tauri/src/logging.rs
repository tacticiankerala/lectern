//! The log file: `lectern.log` in the app's log folder, at most 1 MB and rotated once, with
//! panics logged too.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Metadata, Record};

/// The log file is moved to `lectern.log.1` once it reaches this size.
const LOG_LIMIT: u64 = 1024 * 1024;

/// Sends the `log` records of Lectern (and Tauri's warnings) to `dir\lectern.log`, and logs
/// panics. Without a writable folder, logging is off.
pub fn init_logging(dir: &Path) {
    let logger = match FileLog::open(dir) {
        Ok(logger) => logger,
        Err(e) => {
            eprintln!("lectern: can't write the log in {}: {e}", dir.display());
            return;
        }
    };
    if log::set_boxed_logger(Box::new(logger)).is_err() {
        return;
    }
    log::set_max_level(if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    });
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("{info}");
        default_hook(info);
    }));
}

/// A log file of at most 1 MB that is rotated once: when full it becomes `lectern.log.1`,
/// replacing the previous one, and a new file starts.
struct FileLog {
    path: PathBuf,
    inner: Mutex<LogFile>,
}

struct LogFile {
    file: Option<File>,
    written: u64,
}

impl FileLog {
    fn open(dir: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join("lectern.log");
        let log = Self {
            inner: Mutex::new(LogFile {
                file: None,
                written: fs::metadata(&path).map_or(0, |m| m.len()),
            }),
            path,
        };
        {
            let mut inner = log.inner.lock().unwrap_or_else(PoisonError::into_inner);
            if inner.written >= LOG_LIMIT {
                log.rotate(&mut inner);
            } else {
                inner.file = Some(log.append()?);
            }
        }
        Ok(log)
    }

    fn append(&self) -> std::io::Result<File> {
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
    }

    fn rotate(&self, inner: &mut LogFile) {
        inner.file = None;
        let _ = fs::rename(&self.path, self.path.with_extension("log.1"));
        inner.written = 0;
        inner.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .ok();
    }
}

impl log::Log for FileLog {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info || metadata.target().starts_with("lectern")
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} {:<5} {}: {}\n",
            timestamp(SystemTime::now()),
            record.level(),
            record.target(),
            record.args()
        );
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.written + line.len() as u64 > LOG_LIMIT {
            self.rotate(&mut inner);
        }
        let inner = &mut *inner;
        if let Some(file) = inner.file.as_mut() {
            if file.write_all(line.as_bytes()).is_ok() {
                inner.written += line.len() as u64;
            }
        }
    }

    fn flush(&self) {}
}

/// `t` in UTC as `2026-10-01 14:05:09.123Z`.
fn timestamp(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (year, month, day) = civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let in_day = secs % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}.{:03}Z",
        in_day / 3600,
        in_day % 3600 / 60,
        in_day % 60,
        since.subsec_millis()
    )
}

/// The date `days` after 1970-01-01, by Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn log_timestamps_are_utc_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        let t = UNIX_EPOCH + Duration::from_millis(1_727_791_509_123);
        assert_eq!(timestamp(t), "2024-10-01 14:05:09.123Z");
    }
}
