//! Cheap image metadata extraction.
//!
//! Reading pixel dimensions and format requires decoding only the file header,
//! not the full pixel data. The scanner uses this to decide whether generating
//! a thumbnail is worthwhile before ever decoding pixels.

use std::path::Path;

use crate::error::CoreResult;

/// Header-level metadata about an image file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageMetadata {
    pub width: u32,
    pub height: u32,
    /// Lowercase format name, e.g. "png", "jpeg". None if undetectable.
    pub format: Option<String>,
}

/// Read width/height/format from the image header without decoding pixels.
pub fn read_metadata(path: &Path) -> CoreResult<ImageMetadata> {
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let format = reader.format();
    let (width, height) = reader.into_dimensions()?;
    Ok(ImageMetadata {
        width,
        height,
        format: format.map(|f| f.extensions_str()[0].to_string()),
    })
}

/// Upper bound on pixels we are willing to decode for thumbnails.
/// Above this, we still index the file (metadata only) but skip pixel work.
pub const MAX_DECODE_PIXELS: u64 = 80_000_000; // ~80 MP

/// Whether decoding this image for thumbnails/phash is safe memory-wise.
pub fn safe_to_decode(width: u32, height: u32) -> bool {
    (width as u64) * (height as u64) <= MAX_DECODE_PIXELS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(path: &Path, w: u32, h: u32) {
        let img = image::DynamicImage::new_rgb8(w, h);
        img.save(path).unwrap();
    }

    #[test]
    fn reads_png_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("shot_2026-09-02.png");
        write_png(&p, 640, 480);
        let meta = read_metadata(&p).unwrap();
        assert_eq!(meta.width, 640);
        assert_eq!(meta.height, 480);
        assert_eq!(meta.format.as_deref(), Some("png"));
    }

    #[test]
    fn reads_jpeg_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("photo.jpg");
        let img = image::DynamicImage::new_rgb8(320, 200);
        img.save(&p).unwrap();
        let meta = read_metadata(&p).unwrap();
        assert_eq!(meta.format.as_deref(), Some("jpg"));
        assert_eq!((meta.width, meta.height), (320, 200));
    }

    #[test]
    fn unicode_and_space_paths() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Screenshot 2026-09-02 (final) — نسخة.png");
        write_png(&p, 10, 10);
        assert_eq!(read_metadata(&p).unwrap().height, 10);
    }

    #[test]
    fn corrupted_image_is_error() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("broken.png");
        std::fs::write(&p, b"this is not an image at all").unwrap();
        assert!(read_metadata(&p).is_err());
    }

    #[test]
    fn decode_guard() {
        assert!(safe_to_decode(8000, 6000));
        assert!(!safe_to_decode(20000, 20000));
    }
}
