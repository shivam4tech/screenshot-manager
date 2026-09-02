//! Minimal structured file logger.
//!
//! Privacy rule: we log operational events (scan progress counts, errors,
//! subsystem lifecycle) — never screenshot contents, OCR text, or search
//! queries. Paths are logged only when needed to diagnose a specific file
//! problem already shown to the user in the Problems screen.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};

struct FileLogger {
    file: Mutex<Option<std::fs::File>>,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!(
            "{} [{:<5}] [{}] {}\n",
            chrono_like_now(),
            record.level().to_string(),
            record.target(),
            record.args()
        );
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(f) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = f.flush();
        }
    }
}

/// Timestamp without pulling in chrono: seconds-resolution UTC.
fn chrono_like_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Days since epoch -> civil date (Howard Hinnant's algorithm).
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Install the file logger writing to `<app_data>/logs/app.log`.
pub fn init(app_data_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let logs_dir = app_data_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("app.log"))?;
    let logger = FileLogger {
        file: Mutex::new(Some(file)),
    };
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    });
    Ok(())
}
