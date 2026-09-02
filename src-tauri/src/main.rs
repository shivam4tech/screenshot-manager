//! Screenshot Memory — Tauri application shell.
//!
//! This crate is a thin UI-process layer over `shotmemory-core`. All real
//! logic (database, scanner, hashing, thumbnails) lives in the core crate so
//! it stays UI-independent and testable.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod logger;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use shotmemory_core::db::Database;
use shotmemory_core::platform::PlatformPaths;

/// Shared application state managed by Tauri.
pub struct AppState {
    /// Primary DB connection for quick UI queries.
    pub db: Mutex<Database>,
    /// Path to the DB file; the scan thread opens its own connection so long
    /// scans never block UI queries (WAL allows concurrent readers/writer).
    pub db_path: PathBuf,
    /// Thumbnail cache directory.
    pub thumb_cache: PathBuf,
    /// Whether a scan is currently running.
    pub scan_running: AtomicBool,
    /// Cooperative cancellation flag for the running scan (swapped per scan).
    pub scan_cancel: Mutex<Arc<AtomicBool>>,
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
            commands::get_stats,
            commands::list_screenshots,
            commands::get_thumbnail_path,
        ])
        .run(tauri::generate_context!())
        .expect("error while running screenshot-memory");
}
