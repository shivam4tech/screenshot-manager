//! Tauri IPC commands. Thin adapters between the UI and shotmemory-core.
//!
//! The scan runs on a dedicated thread with its own DB connection (WAL lets
//! it write while the UI reads), emitting throttled progress events.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use shotmemory_core::db::{Database, LibraryStats, ScreenshotDetail, ScreenshotRow};
use shotmemory_core::ocr::{OcrConfig, OcrPipeline, OcrProgress, OcrSummary, TesseractEngine};
use shotmemory_core::platform;
use shotmemory_core::scanner::{ScanProgress, ScanSummary, Scanner};
use shotmemory_core::search::{SearchOutcome, Searcher};

use crate::AppState;

const SCAN_PROGRESS_EVENT: &str = "scan://progress";
const SCAN_COMPLETE_EVENT: &str = "scan://complete";
const OCR_PROGRESS_EVENT: &str = "ocr://progress";
const OCR_COMPLETE_EVENT: &str = "ocr://complete";
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(120);
/// Max OCR attempts per screenshot (mirrors OcrConfig::default).
const OCR_MAX_ATTEMPTS: i64 = 3;

/// Overall state the UI needs on startup to decide which screen to show.
#[derive(Serialize)]
pub struct AppStateDto {
    pub onboarded: bool,
    pub total_screenshots: i64,
    pub problem_count: i64,
    pub indexing: bool,
}

#[derive(Serialize)]
pub struct DirectoryDto {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
}

fn to_dto(d: shotmemory_core::db::Directory) -> DirectoryDto {
    DirectoryDto {
        id: d.id,
        path: d.path,
        enabled: d.enabled,
    }
}

#[tauri::command]
pub fn get_app_state(state: State<AppState>) -> Result<AppStateDto, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let stats = db.library_stats().map_err(|e| e.to_string())?;
    let onboarded = db
        .get_setting("onboarded")
        .map_err(|e| e.to_string())?
        .map(|v| v == "1")
        .unwrap_or(false);
    Ok(AppStateDto {
        onboarded,
        total_screenshots: stats.total,
        problem_count: stats.problem_count,
        indexing: state.scan_running.load(Ordering::Relaxed),
    })
}

/// Platform-appropriate default screenshot locations that currently exist.
#[tauri::command]
pub fn get_default_directories() -> Vec<String> {
    platform::default_screenshot_dirs()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub fn list_directories(state: State<AppState>) -> Result<Vec<DirectoryDto>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    Ok(db.list_directories().map_err(|e| e.to_string())?
        .into_iter()
        .map(to_dto)
        .collect())
}

#[tauri::command]
pub fn add_directory(
    state: State<AppState>,
    path: String,
) -> Result<DirectoryDto, String> {
    let p = std::path::PathBuf::from(&path);
    if !p.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let dir = to_dto(db.add_directory(&p).map_err(|e| e.to_string())?);
    drop(db);
    // The live watcher picks up the new folder without an app restart.
    if let Some(w) = state.watcher.lock().map_err(|e| e.to_string())?.as_ref() {
        w.reconfigure();
    }
    Ok(dir)
}

#[tauri::command]
pub fn remove_directory(state: State<AppState>, id: i64) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.remove_directory(id).map_err(|e| e.to_string())?;
    drop(db);
    if let Some(w) = state.watcher.lock().map_err(|e| e.to_string())?.as_ref() {
        w.reconfigure();
    }
    Ok(())
}

/// Open a native folder picker. Returns the chosen path, if any.
#[tauri::command]
pub fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picked = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .map(|p| p.to_string());
    Ok(picked)
}

/// Start a full scan of all registered directories on a background thread.
///
/// - Emits `scan://progress` (throttled) and `scan://complete`.
/// - Refuses to start if a scan is already running.
/// - Uses its own DB connection so UI queries stay responsive during scans.
#[tauri::command]
pub fn start_scan(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    if state
        .scan_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("a scan is already running".into());
    }

    let cancel = Arc::new(AtomicBool::new(false));
    // Publish OUR cancel flag so cancel_scan() can reach this scan.
    *state
        .scan_cancel
        .lock()
        .map_err(|e| e.to_string())? = cancel.clone();
    let app_handle = app.clone();
    let db_path = state.db_path.clone();
    let thumb_cache = state.thumb_cache.clone();

    std::thread::spawn(move || {
        let result = run_scan(&app_handle, &db_path, &thumb_cache, &cancel);
        let app2 = app_handle.clone();
        if let Ok(stats_db) = Database::open(&db_path) {
            let _ = stats_db.set_setting("onboarded", "1");
        }
        match result {
            Ok(summary) => {
                log::info!(
                    "scan thread complete: indexed={} failed={} cancelled={}",
                    summary.indexed,
                    summary.failed,
                    summary.cancelled
                );
                let _ = app2.emit(SCAN_COMPLETE_EVENT, &summary);
            }
            Err(e) => {
                log::error!("scan thread failed: {e}");
                let _ = app2.emit(
                    SCAN_COMPLETE_EVENT,
                    &ScanSummary {
                        cancelled: true,
                        ..Default::default()
                    },
                );
            }
        }
        // Clear the running flag via managed state.
        if let Some(st) = app2.try_state::<AppState>() {
            st.scan_running.store(false, Ordering::SeqCst);
        }
        // Freshly indexed screenshots are OCR candidates: kick a run now so
        // text becomes searchable without user action.
        let _ = spawn_ocr_if_enabled(app_handle);
    });
    Ok(())
}

fn run_scan(
    app: &AppHandle,
    db_path: &std::path::Path,
    thumb_cache: &std::path::Path,
    cancel: &AtomicBool,
) -> shotmemory_core::CoreResult<ScanSummary> {
    let db = Database::open(db_path)?;
    let dirs: Vec<std::path::PathBuf> = db
        .list_directories()?
        .into_iter()
        .filter(|d| d.enabled)
        .map(|d| std::path::PathBuf::from(d.path))
        .collect();

    let scanner = Scanner::new(&db, thumb_cache);
    let mut last_emit = Instant::now() - PROGRESS_EMIT_INTERVAL;
    let app2 = app.clone();
    let summary = scanner.scan_directories(&dirs, cancel, &mut |p: ScanProgress| {
        // Throttle event traffic so the UI thread isn't flooded on huge scans.
        if p.done || last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
            last_emit = Instant::now();
            let _ = app2.emit(SCAN_PROGRESS_EVENT, &p);
        }
    })?;
    Ok(summary)
}

/// Ask the running scan to stop. Partial progress is kept (resumable).
#[tauri::command]
pub fn cancel_scan(state: State<AppState>) -> Result<(), String> {
    let flag = state.scan_cancel.lock().map_err(|e| e.to_string())?.clone();
    flag.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub fn get_stats(state: State<AppState>) -> Result<LibraryStats, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.library_stats().map_err(|e| e.to_string())
}

/// Paged newest-first listing for the library grid.
#[tauri::command]
pub fn list_screenshots(
    state: State<AppState>,
    limit: i64,
    offset: i64,
) -> Result<Vec<ScreenshotRow>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.list_screenshots(limit, offset).map_err(|e| e.to_string())
}

/// Resolve the cached thumbnail path for a screenshot's content hash.
/// The frontend loads it through the asset protocol (no pixel work in JS).
#[tauri::command]
pub fn get_thumbnail_path(
    state: State<AppState>,
    content_hash: String,
    size: u32,
) -> Result<Option<String>, String> {
    let p = shotmemory_core::thumbnails::cache_path(&state.thumb_cache, &content_hash, size);
    if p.exists() {
        Ok(Some(p.to_string_lossy().into_owned()))
    } else {
        Ok(None)
    }
}

// ---- Search ----------------------------------------------------------------

/// Ranked full-text search (filename + OCR text + tags/notes + filters).
#[tauri::command]
pub fn search(
    state: State<AppState>,
    query: String,
    limit: i64,
    offset: i64,
) -> Result<SearchOutcome, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    Searcher::new(&db)
        .search(&query, limit, offset)
        .map_err(|e| e.to_string())
}

/// Full record for the detail panel (metadata + OCR text + tags).
#[tauri::command]
pub fn get_screenshot(
    state: State<AppState>,
    id: i64,
) -> Result<Option<ScreenshotDetail>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_screenshot_detail(id).map_err(|e| e.to_string())
}

// ---- Settings ----------------------------------------------------------------

#[tauri::command]
pub fn get_setting(state: State<AppState>, key: String) -> Result<Option<String>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.get_setting(&key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_setting(state: State<AppState>, key: String, value: String) -> Result<(), String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    db.set_setting(&key, &value).map_err(|e| e.to_string())
}

// ---- OCR ---------------------------------------------------------------------

/// Spawn a background OCR run over the pending queue.
///
/// Mirrors `start_scan`: dedicated thread, own DB connection, throttled
/// `ocr://progress` events and a final `ocr://complete`. Starting while a
/// run is already active is a no-op success (the UI keeps one set of
/// controls enabled via `ocr_running`).
fn spawn_ocr_run(app: AppHandle) -> Result<(), String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err("app state unavailable".into());
    };
    let engine: Arc<dyn shotmemory_core::ocr::OcrEngine> = match TesseractEngine::discover() {
        Some(e) => Arc::new(e),
        None => {
            return Err(
                "OCR unavailable: the tesseract binary was not found on PATH. \
                 Install tesseract to enable text extraction."
                    .into(),
            )
        }
    };
    if state
        .ocr_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(()); // already running
    }
    let cancel = Arc::new(AtomicBool::new(false));
    *state
        .ocr_cancel
        .lock()
        .map_err(|e| e.to_string())? = cancel.clone();
    let db_path = state.db_path.clone();

    std::thread::spawn(move || {
        let pipeline = OcrPipeline::new(db_path, engine, OcrConfig::default());
        let app2 = app.clone();
        let result = pipeline.run(&cancel, move |p: OcrProgress| {
            let _ = app2.emit(OCR_PROGRESS_EVENT, &p);
        });
        match result {
            Ok(summary) => {
                log::info!(
                    "OCR run complete: ok={} failed={} missing={} cancelled={}",
                    summary.succeeded,
                    summary.failed,
                    summary.skipped_missing,
                    summary.cancelled
                );
                let _ = app.emit(OCR_COMPLETE_EVENT, &summary);
            }
            Err(e) => {
                log::error!("OCR pipeline failed: {e}");
                let _ = app.emit(
                    OCR_COMPLETE_EVENT,
                    &OcrSummary {
                        cancelled: true,
                        ..Default::default()
                    },
                );
            }
        }
        if let Some(st) = app.try_state::<AppState>() {
            st.ocr_running.store(false, Ordering::SeqCst);
        }
    });
    Ok(())
}

/// Start an OCR run now (user-triggered from the UI).
#[tauri::command]
pub fn start_ocr(app: AppHandle) -> Result<(), String> {
    spawn_ocr_run(app)
}

/// Ask the running OCR pipeline to stop. Already-processed results are kept.
#[tauri::command]
pub fn cancel_ocr(state: State<AppState>) -> Result<(), String> {
    let flag = state.ocr_cancel.lock().map_err(|e| e.to_string())?.clone();
    flag.store(true, Ordering::SeqCst);
    Ok(())
}

/// Re-queue every failed OCR job, then start a run.
#[tauri::command]
pub fn retry_ocr(app: AppHandle, state: State<AppState>) -> Result<usize, String> {
    let n = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        db.retry_failed_ocr().map_err(|e| e.to_string())?
    };
    spawn_ocr_run(app)?;
    Ok(n)
}

/// Start an OCR run when it makes sense to do so without user action:
/// enabled in settings, pending work exists, tesseract is available, and
/// no run is active. Called at startup (resume leftovers) and after a
/// scan completes. Returns whether a run was started.
pub fn spawn_ocr_if_enabled(app: AppHandle) -> bool {
    let Some(state) = app.try_state::<AppState>() else {
        return false;
    };
    if state.ocr_running.load(Ordering::Relaxed) {
        return false;
    }
    // User kill switch (Settings toggles this; default on).
    let enabled = state
        .db
        .lock()
        .ok()
        .and_then(|db| db.get_setting("ocr_enabled").ok().flatten())
        .map(|v| v != "0")
        .unwrap_or(true);
    if !enabled {
        return false;
    }
    let pending = Database::open(&state.db_path)
        .ok()
        .and_then(|db| db.ocr_claimable_count(OCR_MAX_ATTEMPTS).ok())
        .unwrap_or(0);
    if pending <= 0 {
        return false;
    }
    matches!(spawn_ocr_run(app), Ok(()))
}
