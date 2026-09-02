//! Platform abstraction for default paths.
//!
//! Platform-dependent path discovery is isolated here so the rest of the code
//! never touches `#[cfg(target_os)]` directly. Per the product spec, default
//! screenshot locations differ per OS and must never be assumed identical.

use std::path::{Path, PathBuf};

use crate::error::CoreResult;

/// The application's name as used in OS data directories.
pub const APP_DIR_NAME: &str = "screenshot-memory";

/// Everything the app needs to know about "where things live" on this machine.
#[derive(Debug, Clone)]
pub struct PlatformPaths {
    /// Sensible default screenshot locations for this OS, filtered to those
    /// that currently exist. The user can add arbitrary directories later.
    pub default_screenshot_dirs: Vec<PathBuf>,
    /// Writable per-user application data directory (created on discovery).
    pub app_data_dir: PathBuf,
    /// SQLite database file path.
    pub db_path: PathBuf,
    /// Thumbnail cache directory (created on discovery).
    pub thumbnail_cache_dir: PathBuf,
}

impl PlatformPaths {
    /// Discover paths for the current platform, creating app-local directories.
    pub fn discover() -> CoreResult<Self> {
        let app_data_dir = app_data_dir_for(APP_DIR_NAME);
        std::fs::create_dir_all(&app_data_dir)?;
        let thumbnail_cache_dir = app_data_dir.join("thumbnails");
        std::fs::create_dir_all(&thumbnail_cache_dir)?;
        Ok(Self {
            default_screenshot_dirs: default_screenshot_dirs(),
            db_path: app_data_dir.join("screenshots.db"),
            app_data_dir,
            thumbnail_cache_dir,
        })
    }
}

/// Default per-user application data directory, per-OS conventions.
pub fn app_data_dir_for(app_name: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        dirs::data_dir()
            .unwrap_or_else(home_fallback)
            .join(app_name)
    }
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir()
            .unwrap_or_else(home_fallback)
            .join(app_name)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // XDG data home; fall back to ~/.local/share
        dirs::data_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
            .unwrap_or_else(home_fallback)
            .join(app_name)
    }
}

/// Default screenshot locations per OS.
///
/// - Linux:   ~/Pictures/Screenshots, ~/Pictures, ~/Desktop, ~/Downloads
/// - Windows: %USERPROFILE%\Pictures\Screenshots, Pictures, Desktop, Downloads
/// - macOS:   ~/Pictures/Screenshots, ~/Pictures, ~/Desktop, ~/Downloads
///
/// Only directories that currently exist are returned; the empty case is fine
/// because the user can pick arbitrary folders during onboarding.
pub fn default_screenshot_dirs() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let pictures = dirs::picture_dir();
    let desktop = dirs::desktop_dir();
    let downloads = dirs::download_dir();
    let home = dirs::home_dir();

    let push = |candidates: &mut Vec<PathBuf>, p: Option<PathBuf>| {
        if let Some(p) = p {
            if !candidates.contains(&p) {
                candidates.push(p);
            }
        }
    };

    #[cfg(target_os = "windows")]
    {
        // Windows Snipping Tool / Win+PrintScreen default to Pictures\Screenshots
        if let Some(pics) = &pictures {
            push(&mut candidates, Some(pics.join("Screenshots")));
        }
        push(&mut candidates, pictures);
        push(&mut candidates, desktop);
        push(&mut candidates, downloads);
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Some(pics) = &pictures {
            push(&mut candidates, Some(pics.join("Screenshots")));
        }
        push(&mut candidates, pictures);
        push(&mut candidates, desktop);
        push(&mut candidates, downloads);
        // Last-resort fallbacks if XDG user dirs are missing
        if let Some(h) = &home {
            push(&mut candidates, Some(h.join("Pictures")));
            push(&mut candidates, Some(h.join("Desktop")));
        }
    }

    candidates.retain(|p| p.is_dir());
    candidates
}

fn home_fallback() -> PathBuf {
    std::env::temp_dir().join("screenshot-memory-fallback-home")
}

/// Ensure a directory exists, creating parents as needed.
pub fn ensure_dir(path: &Path) -> CoreResult<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_creates_app_dirs() {
        let paths = PlatformPaths::discover().unwrap();
        assert!(paths.app_data_dir.exists());
        assert!(paths.thumbnail_cache_dir.exists());
        assert!(paths.db_path.starts_with(&paths.app_data_dir));
    }

    #[test]
    fn default_dirs_are_absolute_and_deduped() {
        let dirs = default_screenshot_dirs();
        for d in &dirs {
            assert!(d.is_absolute(), "not absolute: {d:?}");
            assert!(d.is_dir(), "returned non-existent dir: {d:?}");
        }
        let mut seen = std::collections::HashSet::new();
        for d in &dirs {
            assert!(seen.insert(d.clone()), "duplicate: {d:?}");
        }
    }
}
