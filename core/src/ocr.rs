//! OCR pipeline: fully local text extraction via a Tesseract sidecar binary.
//!
//! Design:
//! - [`OcrEngine`] is a trait so the pipeline is testable with a mock and can
//!   later swap in better local engines without touching anything else.
//! - [`TesseractEngine`] shells out to the `tesseract` binary (bundled with
//!   installers). Each recognition is an isolated process: a crash never
//!   takes the app down.
//! - [`OcrPipeline`] claims pending jobs atomically from the DB, runs them on
//!   a configurable worker pool (each worker with its own DB connection —
//!   WAL allows one writer + many readers), records status, and puts the
//!   extracted text into the FTS index immediately on success.
//! - Failures are retried up to `max_attempts`, then parked as `failed`
//!   (user-retryable). Missing files park the job without attempts.
//! - Resource-friendliness: worker count, per-item delay, and optional
//!   pause-on-battery keep OCR from hogging the machine.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;

use crate::db::Database;
use crate::error::{CoreError, CoreResult};

/// Output of one successful recognition.
#[derive(Debug, Clone)]
pub struct OcrOutput {
    pub text: String,
    pub confidence: Option<f32>,
}

/// A local OCR engine. Implementations must be Send + Sync and stateless
/// between calls (cheap to share across worker threads).
pub trait OcrEngine: Send + Sync {
    fn recognize(&self, image_path: &Path) -> CoreResult<OcrOutput>;
    /// Short engine/version identifier stored with results.
    fn version(&self) -> String;
}

/// Tesseract sidecar engine. Invokes `tesseract <image> stdout -l <lang>`.
pub struct TesseractEngine {
    bin: PathBuf,
    lang: String,
}

impl TesseractEngine {
    pub fn new(bin: impl Into<PathBuf>, lang: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            lang: lang.into(),
        }
    }

    /// Find a tesseract binary on PATH.
    pub fn discover() -> Option<Self> {
        let name = if cfg!(target_os = "windows") {
            "tesseract.exe"
        } else {
            "tesseract"
        };
        let path = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect::<Vec<_>>())?;
        for dir in path {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(Self::new(candidate, "eng"));
            }
        }
        None
    }
}

impl OcrEngine for TesseractEngine {
    fn recognize(&self, image_path: &Path) -> CoreResult<OcrOutput> {
        let output = std::process::Command::new(&self.bin)
            .arg(image_path)
            .arg("stdout")
            .arg("-l")
            .arg(&self.lang)
            .output()
            .map_err(|e| CoreError::other(format!("failed to launch tesseract: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut msg = stderr.lines().next().unwrap_or("tesseract failed").to_string();
            // Keep messages bounded and content-free where possible.
            msg.truncate(200);
            return Err(CoreError::other(msg));
        }
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        Ok(OcrOutput {
            text,
            confidence: None,
        })
    }

    fn version(&self) -> String {
        "tesseract".to_string()
    }
}

/// Resource/behavior configuration for the OCR pipeline.
#[derive(Debug, Clone)]
pub struct OcrConfig {
    /// Number of concurrent worker threads (1 = gentlest).
    pub workers: usize,
    /// Pause between items per worker, to keep laptops cool.
    pub delay_ms: u64,
    /// Skip processing entirely while the machine runs on battery.
    pub pause_on_battery: bool,
    /// Max attempts per screenshot before parking as `failed`.
    pub max_attempts: i64,
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            workers: 1,
            delay_ms: 0,
            pause_on_battery: false,
            max_attempts: 3,
        }
    }
}

/// Live progress for the UI.
#[derive(Debug, Clone, Serialize, Default)]
pub struct OcrProgress {
    pub total: u64,
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped_missing: u64,
    pub done: bool,
}

/// Final outcome of a pipeline run.
#[derive(Debug, Clone, Serialize, Default)]
pub struct OcrSummary {
    pub processed: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub skipped_missing: u64,
    pub cancelled: bool,
    pub paused_battery: bool,
}

/// Background OCR pipeline.
pub struct OcrPipeline {
    /// Path to the DB; each worker opens its own connection.
    db_path: PathBuf,
    engine: Arc<dyn OcrEngine>,
    config: OcrConfig,
}

impl OcrPipeline {
    pub fn new(db_path: impl Into<PathBuf>, engine: Arc<dyn OcrEngine>, config: OcrConfig) -> Self {
        Self {
            db_path: db_path.into(),
            engine,
            config,
        }
    }

    /// Process everything currently pending until the queue drains, the
    /// cancel flag is set, or the battery pause applies. Blocks the calling
    /// thread — run it from a background thread.
    pub fn run(
        &self,
        cancel: &AtomicBool,
        mut on_progress: impl FnMut(OcrProgress),
    ) -> CoreResult<OcrSummary> {
        let mut summary = OcrSummary::default();

        if self.config.pause_on_battery && crate::platform::on_battery() {
            summary.paused_battery = true;
            log::info!("OCR paused: running on battery");
            return Ok(summary);
        }

        // Snapshot how much work exists (for progress accounting).
        let probe = Database::open(&self.db_path)?;
        let total = probe
            .ocr_claimable_count(self.config.max_attempts)?
            .max(0) as u64;
        drop(probe);

        let progress = Arc::new(Mutex::new(OcrProgress {
            total,
            ..Default::default()
        }));
        // Ids already attempted in this run: a re-queued failure must wait
        // for the next run rather than being re-claimed immediately.
        let attempted: Arc<Mutex<HashSet<i64>>> = Arc::new(Mutex::new(HashSet::new()));

        let workers = self.config.workers.max(1);
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let db_path = self.db_path.clone();
                let engine = self.engine.clone();
                let config = self.config.clone();
                let progress = progress.clone();
                let attempted = attempted.clone();
                let cancel = cancel;
                scope.spawn(move || {
                    let Ok(db) = Database::open(&db_path) else {
                        return;
                    };
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let exclude: Vec<i64> = attempted
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .iter()
                            .copied()
                            .collect();
                        let Some((id, path)) =
                            db.claim_ocr_job(config.max_attempts, &exclude).unwrap_or(None)
                        else {
                            return; // queue drained
                        };
                        attempted
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(id);
                        let outcome =
                            process_one(&db, engine.as_ref(), &path, id, config.max_attempts);
                        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner());
                        p.processed += 1;
                        match outcome {
                            OcrOutcome::Done => p.succeeded += 1,
                            OcrOutcome::Failed => p.failed += 1,
                            OcrOutcome::Missing => p.skipped_missing += 1,
                        }
                        drop(p);
                        if config.delay_ms > 0 {
                            std::thread::sleep(Duration::from_millis(config.delay_ms));
                        }
                    }
                });
            }

            // Progress reporting from the coordinator until the queue drains.
            loop {
                std::thread::sleep(Duration::from_millis(100));
                {
                    let p = progress.lock().unwrap_or_else(|e| e.into_inner());
                    if p.processed >= p.total || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                }
                on_progress(progress.lock().unwrap_or_else(|e| e.into_inner()).clone());
            }
        });

        let mut p = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
        summary.processed = p.processed;
        summary.succeeded = p.succeeded;
        summary.failed = p.failed;
        summary.skipped_missing = p.skipped_missing;
        summary.cancelled = cancel.load(Ordering::Relaxed);
        p.done = true;
        on_progress(p);

        log::info!(
            "OCR run finished: ok={} failed={} missing={} cancelled={}",
            summary.succeeded,
            summary.failed,
            summary.skipped_missing,
            summary.cancelled
        );
        Ok(summary)
    }
}

enum OcrOutcome {
    Done,
    Failed,
    Missing,
}

/// Process a single claimed job. Never panics on bad input; every failure
/// path is recorded and reported.
fn process_one(
    db: &Database,
    engine: &dyn OcrEngine,
    path: &str,
    id: i64,
    max_attempts: i64,
) -> OcrOutcome {
    let file = Path::new(path);
    if !file.is_file() {
        // The file vanished between indexing and OCR: park the record and
        // re-queue OCR so text is extracted automatically if it comes back
        // (the claim query skips missing records meanwhile).
        let _ = db.mark_missing_by_path(path);
        let _ = db.queue_ocr(id);
        return OcrOutcome::Missing;
    }

    match engine.recognize(file) {
        Ok(out) => match db.save_ocr_text(id, &out.text, out.confidence, &engine.version()) {
            Ok(()) => OcrOutcome::Done,
            Err(e) => {
                log::error!("OCR result storage failed for id {id}: {e}");
                // Storage failures park as failed (user-retryable) instead
                // of burning attempts against a broken pipeline.
                let _ = db.record_ocr_failure(id, i64::MAX);
                OcrOutcome::Failed
            }
        },
        Err(e) => {
            log::warn!("OCR failed for id {id}: {e}");
            let _ = db.record_ocr_failure(id, max_attempts);
            OcrOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::NewScreenshot;
    use std::sync::atomic::AtomicBool;

    /// Deterministic mock engine: filename words become the "extracted text".
    struct MockEngine {
        fail_on: Mutex<Vec<String>>,
    }

    impl MockEngine {
        fn new() -> Self {
            Self {
                fail_on: Mutex::new(Vec::new()),
            }
        }
    }

    impl OcrEngine for MockEngine {
        fn recognize(&self, image_path: &Path) -> CoreResult<OcrOutput> {
            let name = image_path.file_name().unwrap().to_string_lossy().into_owned();
            if self.fail_on.lock().unwrap().contains(&name) {
                return Err(CoreError::other("mock engine failure"));
            }
            let text = name.trim_end_matches(".png").replace(['_', '-'], " ");
            Ok(OcrOutput {
                text,
                confidence: Some(0.95),
            })
        }
        fn version(&self) -> String {
            "mock-1.0".into()
        }
    }

    fn setup(tmp: &Path) -> (PathBuf, Database) {
        let db_path = tmp.join("test.db");
        let db = Database::open(&db_path).unwrap();
        (db_path, db)
    }

    fn add_pending(db: &Database, path: &str) -> i64 {
        let id = db
            .insert_screenshot(&NewScreenshot {
                path: path.into(),
                filename: Path::new(path)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                ..Default::default()
            })
            .unwrap();
        db.queue_ocr(id).unwrap();
        id
    }

    #[test]
    fn pipeline_processes_pending_and_makes_text_searchable() {
        let tmp = tempfile::tempdir().unwrap();
        let (db_path, db) = setup(tmp.path());
        let f1 = tmp.path().join("docker_error.png");
        std::fs::write(&f1, b"fake image").unwrap();
        let f2 = tmp.path().join("invoice_total.png");
        std::fs::write(&f2, b"fake image").unwrap();
        let id1 = add_pending(&db, &f1.to_string_lossy());
        let id2 = add_pending(&db, &f2.to_string_lossy());

        let pipeline =
            OcrPipeline::new(&db_path, Arc::new(MockEngine::new()), OcrConfig::default());
        let summary = pipeline.run(&AtomicBool::new(false), |_| {}).unwrap();

        assert_eq!(summary.processed, 2);
        assert_eq!(summary.succeeded, 2);
        assert_eq!(summary.failed, 0);
        assert!(!summary.cancelled);

        let stats = db.library_stats().unwrap();
        assert_eq!(stats.with_ocr, 2);
        assert_eq!(stats.ocr_pending, 0);

        let out = crate::search::Searcher::new(&db)
            .search("docker error", 10, 0)
            .unwrap();
        assert_eq!(out.total, 1);
        assert_eq!(out.rows[0].row.id, id1);

        let detail = db.get_screenshot_detail(id2).unwrap().unwrap();
        assert_eq!(detail.ocr_text.as_deref(), Some("invoice total"));
        assert_eq!(detail.ocr_confidence, Some(0.95));
    }

    #[test]
    fn pipeline_records_failures_and_parks_after_max_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let (db_path, db) = setup(tmp.path());
        let f = tmp.path().join("broken.png");
        std::fs::write(&f, b"fake image").unwrap();
        let id = add_pending(&db, &f.to_string_lossy());

        let engine = MockEngine::new();
        engine.fail_on.lock().unwrap().push("broken.png".into());

        let pipeline = OcrPipeline::new(
            &db_path,
            Arc::new(engine),
            OcrConfig {
                max_attempts: 2,
                ..Default::default()
            },
        );

        // Run 1: fails, re-queued (attempt 1 of 2).
        let s1 = pipeline.run(&AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(s1.processed, 1);
        let status: String = db
            .conn()
            .query_row(
                "SELECT ocr_status FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "queued", "first failure must remain retryable");

        // Run 2: fails again → parked as failed.
        let _ = pipeline.run(&AtomicBool::new(false), |_| {}).unwrap();
        let (status, attempts): (String, i64) = db
            .conn()
            .query_row(
                "SELECT ocr_status, ocr_attempts FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert_eq!(attempts, 2);

        // User-triggered retry re-queues it.
        assert_eq!(db.retry_failed_ocr().unwrap(), 1);
        let stats = db.library_stats().unwrap();
        assert_eq!(stats.ocr_pending, 1);
        assert_eq!(stats.ocr_failed, 0);
    }

    #[test]
    fn pipeline_parks_missing_files_without_attempts() {
        let tmp = tempfile::tempdir().unwrap();
        let (db_path, db) = setup(tmp.path());
        let id = add_pending(&db, "/nonexistent/vanished.png");

        let pipeline =
            OcrPipeline::new(&db_path, Arc::new(MockEngine::new()), OcrConfig::default());
        let summary = pipeline.run(&AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(summary.skipped_missing, 1);

        let (status, path_status): (String, String) = db
            .conn()
            .query_row(
                "SELECT ocr_status, status FROM screenshots WHERE id = ?1",
                rusqlite::params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(path_status, "missing");
        // Job was re-queued so it retries automatically when the file
        // comes back (e.g. drive reconnected and re-verified).
        assert_eq!(status, "queued");
    }

    #[test]
    fn pipeline_cancellation_stops_early() {
        let tmp = tempfile::tempdir().unwrap();
        let (db_path, db) = setup(tmp.path());
        for i in 0..5 {
            let f = tmp.path().join(format!("file{i}.png"));
            std::fs::write(&f, b"x").unwrap();
            add_pending(&db, &f.to_string_lossy());
        }
        let cancel = AtomicBool::new(true);
        let pipeline =
            OcrPipeline::new(&db_path, Arc::new(MockEngine::new()), OcrConfig::default());
        let summary = pipeline.run(&cancel, |_| {}).unwrap();
        assert!(summary.cancelled);
        assert_eq!(summary.processed, 0);
    }

    #[test]
    fn multi_worker_pool_processes_everything_exactly_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (db_path, db) = setup(tmp.path());
        for i in 0..12 {
            let f = tmp.path().join(format!("doc number {i}.png"));
            std::fs::write(&f, b"x").unwrap();
            add_pending(&db, &f.to_string_lossy());
        }
        let pipeline = OcrPipeline::new(
            &db_path,
            Arc::new(MockEngine::new()),
            OcrConfig {
                workers: 4,
                ..Default::default()
            },
        );
        let summary = pipeline.run(&AtomicBool::new(false), |_| {}).unwrap();
        assert_eq!(summary.processed, 12);
        assert_eq!(summary.succeeded, 12);
        assert_eq!(db.library_stats().unwrap().with_ocr, 12);
        // No double-processing: nothing stuck in 'processing'.
        let stuck: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM screenshots WHERE ocr_status = 'processing'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stuck, 0);
    }

    #[test]
    fn tesseract_engine_reports_missing_binary_gracefully() {
        let engine = TesseractEngine::new("/nonexistent/tesseract-binary", "eng");
        let tmp = tempfile::tempdir().unwrap();
        let img = tmp.path().join("x.png");
        std::fs::write(&img, b"x").unwrap();
        let err = engine.recognize(&img).unwrap_err();
        assert!(err.to_string().contains("failed to launch tesseract"));
    }
}
