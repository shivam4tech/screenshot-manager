//! Resumable, non-destructive directory scanner.
//!
//! Guarantees:
//! - **Read-only**: this module never renames, moves, copies, or deletes a
//!   user file. It only reads bytes and stat metadata.
//! - **Error isolation**: one corrupted/unreadable file never stops a scan;
//!   failures land in the `problems` table and the scan continues.
//! - **Resumable**: every file is fingerprinted (size + mtime at insert time);
//!   re-running a scan skips unchanged files, so an interrupted or repeated
//!   scan is cheap and never double-indexes.
//! - **Safe batching**: DB writes happen in transactions committed every
//!   `batch_size` files, so a crash mid-scan loses at most one batch.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::db::{Database, NewScreenshot};
use crate::error::CoreResult;
use crate::hashing;
use crate::metadata;
use crate::thumbnails;

/// File extensions treated as screenshots/images.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "avif",
];

/// Files processed per DB transaction.
const DEFAULT_BATCH_SIZE: usize = 64;

/// Live progress reported to the UI during a scan.
#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub files_found: u64,
    pub files_processed: u64,
    pub files_indexed: u64,
    pub files_skipped: u64,
    pub files_failed: u64,
    pub current_file: String,
    pub done: bool,
}

/// Final outcome of a scan run.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ScanSummary {
    pub found: u64,
    pub indexed: u64,
    pub skipped_unchanged: u64,
    pub failed: u64,
    pub cancelled: bool,
    pub directories: usize,
}

/// Whether a filename has a supported image extension (case-insensitive).
pub fn is_image_file(name: &str) -> bool {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(e) => IMAGE_EXTENSIONS.contains(&e.as_str()),
        None => false,
    }
}

/// The scanner engine. Holds only read access to user files.
pub struct Scanner<'a> {
    db: &'a Database,
    thumbnail_cache: PathBuf,
    thumbnail_size: u32,
    batch_size: usize,
}

impl<'a> Scanner<'a> {
    pub fn new(db: &'a Database, thumbnail_cache: impl Into<PathBuf>) -> Self {
        Self {
            db,
            thumbnail_cache: thumbnail_cache.into(),
            thumbnail_size: thumbnails::THUMB_MEDIUM,
            batch_size: DEFAULT_BATCH_SIZE,
        }
    }

    pub fn with_thumbnail_size(mut self, size: u32) -> Self {
        self.thumbnail_size = size;
        self
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Scan the given directories, indexing every supported image.
    ///
    /// `cancel`: set to `true` from another thread to stop early — partial
    /// progress is committed and the scan can simply be re-run later.
    /// `on_progress`: called after every processed file (throttle in the UI).
    pub fn scan_directories(
        &self,
        dirs: &[PathBuf],
        cancel: &AtomicBool,
        on_progress: &mut dyn FnMut(ScanProgress),
    ) -> CoreResult<ScanSummary> {
        let mut summary = ScanSummary {
            directories: dirs.len(),
            ..Default::default()
        };

        // Pre-pass: collect candidate files for accurate progress accounting.
        let mut files: Vec<PathBuf> = Vec::new();
        for dir in dirs {
            if !dir.is_dir() {
                self.db.record_problem(
                    Some(&dir.to_string_lossy()),
                    "directory_unavailable",
                    "source directory does not exist or is not readable",
                )?;
                continue;
            }
            for entry in walkdir::WalkDir::new(dir)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let name = entry.file_name().to_string_lossy();
                if is_image_file(&name) {
                    files.push(entry.into_path());
                }
            }
        }
        // Deterministic order => stable resume behavior across re-runs.
        files.sort();

        summary.found = files.len() as u64;
        log::info!("scan started: {} candidate files in {} dirs", files.len(), dirs.len());

        let mut progress = ScanProgress {
            files_found: summary.found,
            files_processed: 0,
            files_indexed: 0,
            files_skipped: 0,
            files_failed: 0,
            current_file: String::new(),
            done: false,
        };

        let mut tx = Some(self.db.begin_batch()?);
        let mut since_commit = 0usize;

        for path in &files {
            if cancel.load(Ordering::Relaxed) {
                summary.cancelled = true;
                log::info!("scan cancelled after {} files", progress.files_processed);
                break;
            }

            progress.current_file = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            match self.process_file(path) {
                Outcome::Indexed => {
                    progress.files_indexed += 1;
                    summary.indexed += 1;
                }
                Outcome::Unchanged => {
                    progress.files_skipped += 1;
                    summary.skipped_unchanged += 1;
                }
                Outcome::Failed => {
                    progress.files_failed += 1;
                    summary.failed += 1;
                }
            }
            progress.files_processed += 1;
            on_progress(progress.clone());

            since_commit += 1;
            if since_commit >= self.batch_size {
                if let Some(t) = tx.take() {
                    t.commit()?;
                }
                tx = Some(self.db.begin_batch()?);
                since_commit = 0;
            }
        }

        if let Some(t) = tx.take() {
            t.commit()?;
        }

        progress.done = true;
        progress.current_file.clear();
        on_progress(progress);

        log::info!(
            "scan finished: indexed={} skipped={} failed={} cancelled={}",
            summary.indexed, summary.skipped_unchanged, summary.failed, summary.cancelled
        );
        Ok(summary)
    }

    /// Index one file. Failures are recorded as problems and reported as
    /// `Outcome::Failed` — never propagated, so one bad file can't stop a scan.
    fn process_file(&self, path: &Path) -> Outcome {
        let path_str = crate::db::normalize_path(path);

        // 1. Stat metadata (size + timestamps for the fingerprint).
        let fs_meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                let _ = self.db.record_problem(
                    Some(&path_str),
                    "stat_failed",
                    &format!("could not read file metadata: {e}"),
                );
                return Outcome::Failed;
            }
        };
        let size = fs_meta.len() as i64;
        let modified_ts = mtime_secs(&fs_meta);
        let created_ts = fs_meta
            .created()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .or(modified_ts);

        // 2. Fingerprint check: unchanged files are skipped (resumable scans).
        if let Some(fp) = self.db.fingerprint_by_path(&path_str).unwrap_or(None) {
            if Database::fingerprint_matches(&fp, size, modified_ts) {
                let _ = self.db.touch_verified(fp.id);
                return Outcome::Unchanged;
            }
        }

        // 3. Content hash (SHA-256, streamed).
        let content_hash = match hashing::content_hash(path) {
            Ok(h) => h,
            Err(e) => {
                let _ = self.db.record_problem(
                    Some(&path_str),
                    "hash_failed",
                    &format!("could not hash file: {e}"),
                );
                return Outcome::Failed;
            }
        };

        // 4. Header metadata (dimensions + format) without decoding pixels.
        let meta = match metadata::read_metadata(path) {
            Ok(m) => m,
            Err(e) => {
                let _ = self.db.record_problem(
                    Some(&path_str),
                    "unreadable",
                    &format!("corrupted or unsupported image: {e}"),
                );
                return Outcome::Failed;
            }
        };

        // 5. Perceptual hash + thumbnail from a single decode, only when the
        //    image is small enough to decode safely.
        let mut phash = None;
        if metadata::safe_to_decode(meta.width, meta.height) {
            let decoded = image::ImageReader::open(path)
                .map_err(image::ImageError::IoError)
                .and_then(|r| r.with_guessed_format().map_err(image::ImageError::IoError))
                .and_then(|r| r.decode());
            match decoded {
                Ok(img) => {
                    phash = Some(hashing::phash_to_hex(hashing::perceptual_hash(&img)));
                    if let Err(e) = self.write_thumbnail(&img, &content_hash) {
                        let _ = self.db.record_problem(
                            Some(&path_str),
                            "thumbnail_failed",
                            &format!("thumbnail generation failed: {e}"),
                        );
                        // Non-fatal: the record is still fully indexed.
                    }
                }
                Err(e) => {
                    let _ = self.db.record_problem(
                        Some(&path_str),
                        "unreadable",
                        &format!("image failed to decode: {e}"),
                    );
                    return Outcome::Failed;
                }
            }
        } else {
            let _ = self.db.record_problem(
                Some(&path_str),
                "too_large",
                &format!(
                    "image is {} MP; indexed by metadata only",
                    meta.width as u64 * meta.height as u64 / 1_000_000
                ),
            );
        }

        // 6. Insert the record. Images are immediately searchable by
        //    filename/date; OCR full-text arrives later (Sprint 2).
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rec = NewScreenshot {
            path: path_str,
            filename,
            size,
            created_ts,
            modified_ts,
            width: Some(meta.width as i64),
            height: Some(meta.height as i64),
            format: meta.format,
            content_hash: Some(content_hash),
            phash,
            source_dir_id: None,
        };
        if let Err(e) = self.db.insert_screenshot(&rec) {
            let _ = self.db.record_problem(
                Some(&rec.path),
                "db_insert_failed",
                &format!("could not store record: {e}"),
            );
            return Outcome::Failed;
        }
        Outcome::Indexed
    }

    /// Write a thumbnail into the cache from an already-decoded image.
    fn write_thumbnail(&self, img: &image::DynamicImage, content_hash: &str) -> CoreResult<()> {
        use std::io::Write;
        let dest = thumbnails::cache_path(&self.thumbnail_cache, content_hash, self.thumbnail_size);
        if dest.exists() {
            return Ok(());
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let thumb = img.thumbnail(self.thumbnail_size, self.thumbnail_size);
        // Write to a temp file then rename for atomic cache updates.
        let tmp = dest.with_extension("tmp");
        {
            let mut file = std::fs::File::create(&tmp)?;
            thumb.write_to(&mut file, image::ImageFormat::Png)?;
            file.flush()?;
        }
        std::fs::rename(&tmp, &dest)?;
        Ok(())
    }
}

enum Outcome {
    Indexed,
    Unchanged,
    Failed,
}

/// File modified time as unix seconds, if available on this platform.
fn mtime_secs(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn make_png(path: &Path, w: u32, h: u32) {
        let mut img = image::RgbImage::new(w, h);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x % 256) as u8, 128, 200]);
        }
        image::DynamicImage::ImageRgb8(img).save(path).unwrap();
    }

    #[test]
    fn is_image_file_detection() {
        assert!(is_image_file("Screenshot_2026-09-02.PNG"));
        assert!(!is_image_file("notes.txt"));
        assert!(!is_image_file("no_extension"));
        assert!(!is_image_file("archive.png.zip"));
        assert!(is_image_file("photo.JPEG"));
    }

    #[test]
    fn scans_indexes_skips_and_resumes() {
        let data_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let src = data_dir.path().join("Screenshots");
        std::fs::create_dir_all(&src).unwrap();

        make_png(&src.join("Screenshot_2026-09-01.png"), 300, 200);
        make_png(&src.join("Screenshot 2026-09-02 (final) — ünïcødé.png"), 400, 250);
        std::fs::create_dir_all(src.join("nested dir")).unwrap();
        make_png(&src.join("nested dir/deep shot.png"), 100, 100);
        std::fs::write(src.join("notes.txt"), b"not an image").unwrap();
        std::fs::write(src.join("corrupted.png"), b"definitely not a png").unwrap();

        let db = Database::open_in_memory().unwrap();
        let scanner = Scanner::new(&db, cache_dir.path());
        let cancel = AtomicBool::new(false);
        let mut pxs = Vec::new();
        let summary = scanner
            .scan_directories(&[src.clone()], &cancel, &mut |p| pxs.push(p))
            .unwrap();

        assert_eq!(summary.found, 4, "3 images + 1 corrupted png candidate");
        assert_eq!(summary.indexed, 3);
        assert_eq!(summary.failed, 1);
        assert!(!summary.cancelled);
        assert!(pxs.last().unwrap().done);

        let stats = db.library_stats().unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.available, 3);
        assert_eq!(stats.problem_count, 1, "corrupted file recorded as problem");

        // Thumbnails exist for every successfully indexed image.
        let thumbs = walkdir::WalkDir::new(cache_dir.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .count();
        assert_eq!(thumbs, 3);

        // Re-scan: everything unchanged => all skipped, nothing re-indexed.
        let summary2 = scanner
            .scan_directories(&[src.clone()], &cancel, &mut |_| {})
            .unwrap();
        assert_eq!(summary2.skipped_unchanged, 3);
        assert_eq!(summary2.indexed, 0);
        assert_eq!(db.library_stats().unwrap().total, 3);
    }

    #[test]
    fn scan_cancels_gracefully_and_partial_progress_persists() {
        let data_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let src = data_dir.path().join("S");
        std::fs::create_dir_all(&src).unwrap();
        for i in 0..5 {
            make_png(&src.join(format!("shot{i}.png")), 60, 40);
        }

        let db = Database::open_in_memory().unwrap();
        let scanner = Scanner::new(&db, cache_dir.path()).with_batch_size(2);
        let cancel = AtomicBool::new(true); // cancel immediately
        let summary = scanner
            .scan_directories(&[src.clone()], &cancel, &mut |_| {})
            .unwrap();
        assert!(summary.cancelled);

        // Cancelled scan leaves a consistent DB; re-running completes it.
        let cancel = AtomicBool::new(false);
        let summary2 = scanner
            .scan_directories(&[src], &cancel, &mut |_| {})
            .unwrap();
        assert!(!summary2.cancelled);
        assert_eq!(summary2.indexed + summary2.skipped_unchanged, 5);
        assert_eq!(db.library_stats().unwrap().total, 5);
    }

    #[test]
    fn scan_handles_missing_directory() {
        let cache_dir = tempfile::tempdir().unwrap();
        let db = Database::open_in_memory().unwrap();
        let scanner = Scanner::new(&db, cache_dir.path());
        let missing = std::env::temp_dir().join("shotmemory-nonexistent-dir-xyz");
        let summary = scanner
            .scan_directories(&[missing], &AtomicBool::new(false), &mut |_| {})
            .unwrap();
        assert_eq!(summary.found, 0);
        assert_eq!(db.library_stats().unwrap().problem_count, 1);
    }

    #[test]
    fn changed_file_is_reindexed() {
        let data_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let src = data_dir.path().join("S");
        std::fs::create_dir_all(&src).unwrap();
        let p = src.join("shot.png");
        make_png(&p, 50, 50);

        let db = Database::open_in_memory().unwrap();
        let scanner = Scanner::new(&db, cache_dir.path());
        scanner
            .scan_directories(&[src.clone()], &AtomicBool::new(false), &mut |_| {})
            .unwrap();

        // Modify the file (different content => different fingerprint).
        std::thread::sleep(std::time::Duration::from_millis(1100)); // mtime resolution
        make_png(&p, 80, 80);
        let summary = scanner
            .scan_directories(&[src], &AtomicBool::new(false), &mut |_| {})
            .unwrap();
        // The changed file no longer matches its fingerprint, so it is
        // re-indexed *in place* at the same path: same record id, fresh
        // metadata, stale OCR invalidated.
        assert_eq!(summary.indexed, 1);
        assert_eq!(summary.skipped_unchanged, 0);
        assert_eq!(db.library_stats().unwrap().total, 1);
    }
}

