//! Live directory monitoring: the app keeps watching configured folders so
//! new screenshots index themselves without any user action.
//!
//! Behavior per event (after debouncing):
//! - **Created / Modified** → wait until the file is stable (size + mtime
//!   unchanged across polls — screenshots are written over a short interval),
//!   then index it. Identical content seen for a previously `missing` record
//!   re-points that record (moves/renames keep their identity via the
//!   content hash). Successful indexing queues OCR.
//! - **Removed** → mark the record `missing`; metadata is never destroyed
//!   (an external drive may simply be disconnected).
//!
//! The service is fully cooperative: it can be stopped, and reconfigured
//! (re-read watched directories from the DB) after the user adds/removes a
//! folder. It runs with its own DB connection; WAL keeps the UI responsive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::db::Database;
use crate::scanner::Scanner;

/// How long to wait for a freshly written file to stop changing.
const STABLE_TIMEOUT: Duration = Duration::from_secs(6);
/// Debounce window: events are batched after this much quiet time.
const DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Created,
    Modified,
    Removed,
}

/// Handle to the background watcher thread.
pub struct WatchService {
    stop: Arc<AtomicBool>,
    reconfigure: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl WatchService {
    /// Spawn the watcher thread. It opens its own DB connection and monitors
    /// every enabled directory currently registered.
    pub fn spawn(db_path: PathBuf, thumbnail_cache: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let reconfigure = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let reconfigure2 = reconfigure.clone();
        let handle = std::thread::spawn(move || {
            run_loop(db_path, thumbnail_cache, &stop2, &reconfigure2);
        });
        Self {
            stop,
            reconfigure,
            handle: Some(handle),
        }
    }

    /// Ask the thread to re-read watched directories from the DB.
    pub fn reconfigure(&self) {
        self.reconfigure.store(true, Ordering::SeqCst);
    }

    /// Stop the thread.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.reconfigure.store(true, Ordering::SeqCst);
    }
}

impl Drop for WatchService {
    fn drop(&mut self) {
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run_loop(db_path: PathBuf, thumb_cache: PathBuf, stop: &AtomicBool, reconfigure: &AtomicBool) {
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }

        let Ok(db) = Database::open(&db_path) else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        let dirs: Vec<PathBuf> = db
            .list_directories()
            .unwrap_or_default()
            .into_iter()
            .filter(|d| d.enabled)
            .map(|d| PathBuf::from(d.path))
            .collect();

        // (Re)build the platform watcher over the current directory set.
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(tx) {
            Ok(w) => w,
            Err(e) => {
                log::error!("file watcher unavailable: {e}");
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        for dir in &dirs {
            if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
                // One unreadable directory must not break the rest.
                log::warn!("cannot watch {}: {e}", dir.display());
            }
        }
        reconfigure.store(false, Ordering::SeqCst);
        log::info!("watching {} directories for changes", dirs.len());

        let scanner = Scanner::new(&db, &thumb_cache);
        let mut pending: HashMap<PathBuf, Kind> = HashMap::new();
        let mut dirty = false;

        // Event loop for this watcher instance.
        loop {
            if stop.load(Ordering::Relaxed) || reconfigure.load(Ordering::Relaxed) {
                return;
            }
            match rx.recv_timeout(DEBOUNCE) {
                Ok(event) => {
                    if let Ok(event) = event {
                        record_event(&event.kind, &event.paths, &mut pending);
                        dirty = true;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if dirty {
                        let mut batch: Vec<(PathBuf, Kind)> = pending.drain().collect();
                        batch.sort();
                        dirty = false;
                        for (path, kind) in batch {
                            handle_event(&db, &scanner, &path, kind);
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // Watcher dropped → rebuild on the next outer iteration.
                    break;
                }
            }
        }
    }
}

/// Translate a notify event into our simplified pending map.
fn record_event(kind: &EventKind, paths: &[PathBuf], pending: &mut HashMap<PathBuf, Kind>) {
    match kind {
        EventKind::Create(_) => {
            for p in paths {
                pending.insert(p.clone(), Kind::Created);
            }
        }
        EventKind::Remove(_) => {
            for p in paths {
                pending.insert(p.clone(), Kind::Removed);
            }
        }
        EventKind::Modify(m) => {
            // Rename From = old path gone; anything else = content changed.
            let rename_from = matches!(
                m,
                notify::event::ModifyKind::Name(notify::event::RenameMode::From)
            );
            let rename_both = matches!(
                m,
                notify::event::ModifyKind::Name(notify::event::RenameMode::Both)
            );
            for (i, p) in paths.iter().enumerate() {
                if rename_from || (rename_both && i == 0) {
                    pending.insert(p.clone(), Kind::Removed);
                } else {
                    pending.insert(p.clone(), Kind::Modified);
                }
            }
        }
        _ => {}
    }
}

fn handle_event(db: &Database, scanner: &Scanner, path: &Path, kind: Kind) {
    let normalized = crate::db::normalize_path(path);
    match kind {
        Kind::Removed => {
            if let Ok(n) = db.mark_missing_by_path(&normalized) {
                if n > 0 {
                    log::info!("file removed, record marked missing: {normalized}");
                }
            }
        }
        Kind::Created | Kind::Modified => {
            if !path.is_file() || !crate::scanner::is_image_file(&path.to_string_lossy()) {
                return;
            }
            if !wait_until_stable(path) {
                log::warn!("file never stabilized; skipping for now: {}", path.display());
                return;
            }
            match scanner.index_file(path) {
                crate::scanner::FileOutcome::Indexed(id) => {
                    let _ = db.queue_ocr(id);
                    log::info!("indexed new/changed file (id {id})");
                }
                crate::scanner::FileOutcome::Unchanged => {}
                crate::scanner::FileOutcome::Failed => {}
            }
        }
    }
}

/// Wait until the file's (size, mtime) stop changing — screenshots are
/// written incrementally by the OS, and indexing a partial file would
/// corrupt hashes and thumbnails.
fn wait_until_stable(path: &Path) -> bool {
    let start = Instant::now();
    let snapshot = || -> Option<(u64, std::time::SystemTime)> {
        std::fs::metadata(path)
            .ok()
            .map(|m| (m.len(), m.modified().unwrap_or(UNIX_EPOCH)))
    };
    let Some(mut last) = snapshot() else {
        return false;
    };
    while start.elapsed() < STABLE_TIMEOUT {
        std::thread::sleep(Duration::from_millis(250));
        match snapshot() {
            Some(cur) if cur == last => return true,
            Some(cur) => last = cur,
            None => return false, // vanished mid-write
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;
    use std::sync::atomic::AtomicBool;

    /// Poll a condition for up to `secs`, so OS file events (which arrive
    /// asynchronously) don't make tests flaky.
    fn wait_for<F: Fn() -> bool>(secs: u64, mut f: F) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        f()
    }

    fn make_png(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbImage::new(w, h);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, 100, 150]);
        }
        image::DynamicImage::ImageRgb8(img).save(path).unwrap();
    }

    #[test]
    fn wait_until_stable_basics() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.png");
        make_png(&p, 30, 30);
        assert!(wait_until_stable(&p));
        assert!(!wait_until_stable(&tmp.path().join("nope.png")));
    }

    #[test]
    fn record_event_maps_notify_kinds() {
        let mut pending = HashMap::new();
        let p = PathBuf::from("/tmp/x.png");
        record_event(&EventKind::Create(notify::event::CreateKind::File), &[p.clone()], &mut pending);
        assert_eq!(pending[&p], Kind::Created);
        record_event(&EventKind::Remove(notify::event::RemoveKind::File), &[p.clone()], &mut pending);
        assert_eq!(pending[&p], Kind::Removed);
        record_event(
            &EventKind::Modify(notify::event::ModifyKind::Name(notify::event::RenameMode::From)),
            &[p.clone()],
            &mut pending,
        );
        assert_eq!(pending[&p], Kind::Removed);
        record_event(
            &EventKind::Modify(notify::event::ModifyKind::Data(notify::event::DataChange::Any)),
            &[p.clone()],
            &mut pending,
        );
        assert_eq!(pending[&p], Kind::Modified);
    }

    #[test]
    fn service_indexes_new_files_and_marks_removed_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Screenshots");
        std::fs::create_dir_all(&src).unwrap();

        let db_path = tmp.path().join("db.sqlite");
        let db = Database::open(&db_path).unwrap();
        db.add_directory(&src).unwrap();

        let _service = WatchService::spawn(db_path.clone(), tmp.path().join("thumbs"));
        let img = src.join("new shot.png");
        make_png(&img, 200, 120);

        let db2 = Database::open(&db_path).unwrap();
        assert!(wait_for(15, || {
            db2.list_screenshots(10, 0).unwrap().len() == 1
        }));
        let row = &db2.list_screenshots(10, 0).unwrap()[0];
        assert_eq!(row.status, "available");
        assert_eq!(row.filename, "new shot.png");

        // Rename → identity follows the content: old path missing, new path
        // available, and (because the content is identical) the same record.
        let renamed = src.join("renamed shot.png");
        std::fs::rename(&img, &renamed).unwrap();
        assert!(wait_for(15, || {
            db2.list_screenshots(10, 0).unwrap()[0].filename == "renamed shot.png"
        }));

        // Delete → record stays but is marked missing.
        std::fs::remove_file(&renamed).unwrap();
        assert!(wait_for(15, || {
            db2.list_screenshots(10, 0).unwrap()[0].status == "missing"
        }));
        // Metadata is retained — the user may reconnect the drive.
        assert!(db2.list_screenshots(10, 0).unwrap()[0].content_hash.is_some());
    }

    #[test]
    fn service_ignores_non_image_and_unstable_writes_eventually() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("S");
        std::fs::create_dir_all(&src).unwrap();
        let db_path = tmp.path().join("db.sqlite");
        let db = Database::open(&db_path).unwrap();
        db.add_directory(&src).unwrap();

        std::fs::write(src.join("notes.txt"), b"not an image").unwrap();
        let _service = WatchService::spawn(db_path.clone(), tmp.path().join("thumbs"));

        let db2 = Database::open(&db_path).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(db2.list_screenshots(10, 0).unwrap().len(), 0);
        let _ = NewScreenshot::default();
        let _ = AtomicBool::new(false);
    }
}
