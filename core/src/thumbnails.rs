//! Thumbnail generation and disk cache.
//!
//! Thumbnails are cached on disk keyed by the file's *content hash* and target
//! size, so the same image never gets thumbnailed twice even if it is renamed
//! or moved. Cache layout:
//!
//! ```text
//! <cache_dir>/<h[0..2]>/<h>_<size>.png
//! ```
//!
//! The grid UI must never decode full-resolution screenshots — only these.

use std::path::{Path, PathBuf};

use crate::error::CoreResult;

/// Thumbnail edge length presets exposed in the UI (small/medium/large grids).
pub const THUMB_SMALL: u32 = 256;
pub const THUMB_MEDIUM: u32 = 512;
pub const THUMB_LARGE: u32 = 1024;

/// Determine the cache path for a thumbnail without generating it.
pub fn cache_path(cache_dir: &Path, content_hash_hex: &str, size: u32) -> PathBuf {
    let prefix = content_hash_hex
        .get(..2)
        .unwrap_or("00")
        .to_string();
    cache_dir
        .join(prefix)
        .join(format!("{content_hash_hex}_{size}.png"))
}

/// Generate (or reuse) a thumbnail for the image at `path`.
///
/// `content_hash_hex` should be the file's SHA-256 as computed by the scanner.
/// Returns the thumbnail file path. If a cached thumbnail already exists it is
/// returned without re-decoding.
pub fn generate_thumbnail(
    path: &Path,
    content_hash_hex: &str,
    size: u32,
    cache_dir: &Path,
) -> CoreResult<PathBuf> {
    let dest = cache_path(cache_dir, content_hash_hex, size);
    if dest.exists() {
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    // `thumbnail()` preserves aspect ratio, fitting within `size x size`.
    let thumb = img.thumbnail(size, size);
    thumb.save_with_format(&dest, image::ImageFormat::Png)?;
    Ok(dest)
}

/// Thumbnail width/height for a source image, preserving aspect ratio.
pub fn thumbnail_dimensions(width: u32, height: u32, size: u32) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (size, size);
    }
    let scale = size as f64 / width.max(height) as f64;
    let w = ((width as f64 * scale).round() as u32).max(1);
    let h = ((height as f64 * scale).round() as u32).max(1);
    (w, h)
}

/// Count files currently in the thumbnail cache (for the maintenance screen).
pub fn cache_file_count(cache_dir: &Path) -> CoreResult<u64> {
    let mut count = 0u64;
    if !cache_dir.exists() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(cache_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            count += std::fs::read_dir(entry.path())?.count() as u64;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashing;

    fn make_png(path: &Path, w: u32, h: u32) {
        image::DynamicImage::new_rgb8(w, h).save(path).unwrap();
    }

    #[test]
    fn generates_and_reuses_thumbnail() {
        let src_dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("screenshot.png");
        make_png(&src, 1000, 500);

        let hash = hashing::content_hash(&src).unwrap();
        let t1 = generate_thumbnail(&src, &hash, THUMB_MEDIUM, cache_dir.path()).unwrap();
        assert!(t1.exists());
        let m = image::ImageReader::open(&t1).unwrap().into_dimensions().unwrap();
        assert_eq!(m, (THUMB_MEDIUM, THUMB_MEDIUM / 2));

        // Second call must hit the cache (same path)
        let t2 = generate_thumbnail(&src, &hash, THUMB_MEDIUM, cache_dir.path()).unwrap();
        assert_eq!(t1, t2);
    }

    #[test]
    fn cache_path_sharding() {
        let p = cache_path(Path::new("/tmp/c"), "abcdef1234567890", 512);
        assert!(p.starts_with("/tmp/c/ab"));
        assert!(p.ends_with("abcdef1234567890_512.png"));
    }

    #[test]
    fn thumbnail_dimensions_preserve_aspect() {
        assert_eq!(thumbnail_dimensions(1000, 500, 256), (256, 128));
        assert_eq!(thumbnail_dimensions(500, 1000, 512), (256, 512));
        assert_eq!(thumbnail_dimensions(10, 10, 512), (512, 512));
    }

    #[test]
    fn cache_file_count_works() {
        let cache_dir = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let src = src_dir.path().join("a.png");
        make_png(&src, 40, 40);
        let hash = hashing::content_hash(&src).unwrap();
        generate_thumbnail(&src, &hash, THUMB_SMALL, cache_dir.path()).unwrap();
        assert_eq!(cache_file_count(cache_dir.path()).unwrap(), 1);
    }
}
