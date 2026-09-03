//! Screenshot Memory — Tauri application shell.
//!
//! This crate is a thin UI-process layer over `shotmemory-core`. All real
//! logic (database, scanner, hashing, thumbnails) lives in the core crate so
//! it stays UI-independent and testable.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod logger;
mod tray;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use tauri::Manager;

use shotmemory_core::db::Database;
use shotmemory_core::platform::PlatformPaths;
use shotmemory_core::watcher::WatchService;

/// Shared application state managed by Tauri.
pub struct AppState {
    /// Primary DB connection for quick UI queries.
    pub db: Mutex<Database>,
    /// Path to the DB file; background threads open their own connections so
    /// long scans/OCR never block UI queries (WAL allows concurrency).
    pub db_path: PathBuf,
    /// Thumbnail cache directory.
    pub thumb_cache: PathBuf,
    /// Whether a scan is currently running.
    pub scan_running: AtomicBool,
    /// Cooperative cancellation flag for the running scan (swapped per scan).
    pub scan_cancel: Mutex<Arc<AtomicBool>>,
    /// Live directory monitor (Sprint 2).
    pub watcher: Mutex<Option<WatchService>>,
    /// Whether an OCR run is currently active.
    pub ocr_running: AtomicBool,
    /// Cooperative cancellation for the OCR pipeline (swapped per run).
    pub ocr_cancel: Mutex<Arc<AtomicBool>>,
}

fn main() {
    let paths = PlatformPaths::discover().expect("failed to initialize app directories");
    logger::init(&paths.app_data_dir).expect("failed to initialize logger");
    let db = Database::open(&paths.db_path).expect("failed to open database");
    log::info!("screenshot-memory starting; data dir: {}", paths.app_data_dir.display());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            db: Mutex::new(db),
            db_path: paths.db_path.clone(),
            thumb_cache: paths.thumbnail_cache_dir.clone(),
            scan_running: AtomicBool::new(false),
            scan_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
            watcher: Mutex::new(None),
            ocr_running: AtomicBool::new(false),
            ocr_cancel: Mutex::new(Arc::new(AtomicBool::new(false))),
        })
        .setup(|app| {
            // Start live directory monitoring so new screenshots index
            // themselves without any user action.
            let state = app.state::<AppState>();
            let service = WatchService::spawn(state.db_path.clone(), state.thumb_cache.clone());
            *state.watcher.lock().map_err(|e| e.to_string())? = Some(service);

            // Resume any OCR work left over from a previous session.
            let _ = commands::spawn_ocr_if_enabled(app.handle().clone());

            // Tray icon: show / scan / quit.
            tray::build_tray(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_app_state,
            commands::get_default_directories,
            commands::list_directories,
            commands::add_directory,
            commands::remove_directory,
            commands::pick_folder,
            commands::start_scan,
            commands::cancel_scan,
            commands::start_ocr,
            commands::cancel_ocr,
            commands::retry_ocr,
            commands::get_stats,
            commands::list_screenshots,
            commands::all_screenshot_ids,
            commands::search_ids,
            commands::collection_item_ids,
            commands::timeline_item_ids,
            commands::burst_range_ids,
            commands::get_thumbnail_path,
            commands::search,
            commands::get_screenshot,
            commands::get_setting,
            commands::set_setting,
            commands::add_tag,
            commands::remove_tag,
            commands::list_tags,
            commands::set_starred,
            commands::set_read_later,
            commands::set_note,
            commands::create_collection,
            commands::rename_collection,
            commands::delete_collection,
            commands::list_collections,
            commands::add_to_collection,
            commands::remove_from_collection,
            commands::list_collection_items,
            commands::list_screenshot_collections,
            commands::add_many_to_collection,
            commands::delete_screenshots,
            commands::restore_screenshots,
            commands::timeline_months,
            commands::timeline_days,
            commands::timeline_items,
            commands::exact_duplicate_groups,
            commands::similar_groups,
            commands::list_bursts,
            commands::burst_items,
            commands::list_problems,
            commands::clear_problems,
            commands::get_data_dir,
            commands::run_classification,
        ])
        .run(tauri::generate_context!())
        .expect("error while running screenshot-memory");
}
