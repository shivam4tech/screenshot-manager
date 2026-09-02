//! Content and perceptual hashing.
//!
//! - [`content_hash`]: SHA-256 of file bytes. Detects *exact* duplicates with
//!   cryptographic confidence.
//! - [`perceptual_hash`]: 64-bit difference hash (dHash) of the decoded image.
//!   Detects *visually similar* screenshots (same page captured twice, minor
//!   cursor moves, etc.) via Hamming distance.
//!
//! Hashes are stored as lowercase hex strings in the database. The content
//! hash is the stable identity anchor: paths change, content identity doesn't.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

/// Size of the streaming read buffer for content hashing (1 MiB).
const HASH_BUF_SIZE: usize = 1024 * 1024;

/// Compute the SHA-256 content hash of a file, streaming so that arbitrarily
/// large files never load fully into memory. Returns lowercase hex.
pub fn content_hash(path: &Path) -> CoreResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of an in-memory byte slice (used by tests and small payloads).
pub fn content_hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Compute the 64-bit dHash (difference hash) of a decoded image.
///
/// The image is downscaled to 9x8 grayscale; each of the 64 horizontal
/// neighbor pairs produces one bit (left pixel brighter than right).
/// Horizontal-gradient hashing is robust to rescaling and small overlays,
/// which is exactly the "same screenshot captured twice" case.
pub fn perceptual_hash(img: &image::DynamicImage) -> u64 {
    const W: u32 = 9;
    const H: u32 = 8;
    let gray = img.to_luma8();
    let small = image::imageops::resize(&gray, W, H, image::imageops::FilterType::Triangle);
    let mut hash: u64 = 0;
    for y in 0..H as usize {
        for x in 0..(W as usize - 1) {
            let left = small.get_pixel(x as u32, y as u32)[0] as u16;
            let right = small.get_pixel((x + 1) as u32, y as u32)[0] as u16;
            hash <<= 1;
            if left > right {
                hash |= 1;
            }
        }
    }
    hash
}

/// Perceptual hash of the image at `path`. Decodes the full image.
pub fn perceptual_hash_file(path: &Path) -> CoreResult<u64> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()?;
    Ok(perceptual_hash(&img))
}

/// Hamming distance between two 64-bit perceptual hashes.
/// Rough guide: 0 = identical, <= 8 = near-duplicate, > 16 = probably different.
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Format a u64 perceptual hash as 16-char lowercase hex.
pub fn phash_to_hex(hash: u64) -> String {
    format!("{:016x}", hash)
}

/// Parse a 16-char hex perceptual hash back to u64.
pub fn phash_from_hex(s: &str) -> CoreResult<u64> {
    u64::from_str_radix(s.trim(), 16).map_err(|e| CoreError::other(format!("bad phash '{s}': {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn sha256_known_vector() {
        // sha256("") and sha256("abc") reference vectors
        assert_eq!(
            content_hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            content_hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn content_hash_file_matches_bytes_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file with spaces & ünïcødé.bin");
        let data = "some screenshot bytes \u{1F5BC}".as_bytes();
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(data).unwrap();
        assert_eq!(content_hash(&path).unwrap(), content_hash_bytes(data));
    }

    #[test]
    fn content_hash_missing_file_is_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(content_hash(&dir.path().join("nope.png")).is_err());
    }

    fn gradient_image(width: u32, height: u32) -> image::DynamicImage {
        // Horizontal gradient: brightness increases left -> right
        let mut img = image::RgbImage::new(width, height);
        for x in 0..width {
            let v = ((x as f32 / width as f32) * 255.0) as u8;
            for y in 0..height {
                img.put_pixel(x, y, image::Rgb([v, v, v]));
            }
        }
        image::DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn dhash_is_deterministic() {
        let img = gradient_image(200, 100);
        assert_eq!(perceptual_hash(&img), perceptual_hash(&img));
    }

    #[test]
    fn dhash_is_resolution_invariant() {
        let big = perceptual_hash(&gradient_image(800, 400));
        let small = perceptual_hash(&gradient_image(160, 80));
        assert_eq!(big, small, "same gradient at different sizes should hash alike");
    }

    #[test]
    fn dhash_distinguishes_direction() {
        let ltr = perceptual_hash(&gradient_image(200, 100)); // bright on right
        let mut flipped = gradient_image(200, 100);
        flipped = image::DynamicImage::ImageRgb8(image::imageops::rotate180(&flipped.to_rgb8()));
        let rtl = perceptual_hash(&flipped);
        assert_ne!(ltr, rtl);
    }

    #[test]
    fn hamming_distance_basics() {
        assert_eq!(hamming_distance(0, 0), 0);
        assert_eq!(hamming_distance(0, u64::MAX), 64);
        assert_eq!(hamming_distance(0b1010, 0b0110), 2);
    }

    #[test]
    fn phash_hex_roundtrip() {
        let h: u64 = 0x0123_4567_89ab_cdef;
        assert_eq!(phash_from_hex(&phash_to_hex(h)).unwrap(), h);
        assert!(phash_from_hex("not-hex").is_err());
    }
}
