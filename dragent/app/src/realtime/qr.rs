//! QR-code bitmap generation for the share sheet.
//!
//! Produces a raw RGBA buffer (black modules on white, with a quiet zone) that
//! the workspace registers into the asset cache and renders with an `Image`.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use qrcode::{Color, QrCode};

/// Pixels drawn per QR module. Higher = crisper when scaled up.
const SCALE: usize = 8;
/// Quiet-zone width, in modules, required around a QR code for reliable scanning.
const QUIET: usize = 4;

/// A rendered QR code as an RGBA bitmap.
pub struct QrBitmap {
    /// Stable per-URL asset id (same URL → same id, so the cache dedupes).
    pub id: String,
    /// Row-major RGBA pixels, `size * size * 4` bytes.
    pub rgba: Vec<u8>,
    /// Side length in pixels.
    pub size: u32,
}

/// Encode `data` (typically a join URL) into a QR bitmap. Returns `None` if the
/// data is too large to encode.
pub fn encode(data: &str) -> Option<QrBitmap> {
    let code = QrCode::new(data.as_bytes()).ok()?;
    let modules = code.width();
    let dim = modules + QUIET * 2;
    let size = dim * SCALE;

    // Start all-white, then paint the dark modules black.
    let mut rgba = vec![255u8; size * size * 4];
    for my in 0..modules {
        for mx in 0..modules {
            if code[(mx, my)] != Color::Dark {
                continue;
            }
            let px0 = (QUIET + mx) * SCALE;
            let py0 = (QUIET + my) * SCALE;
            for dy in 0..SCALE {
                let row = (py0 + dy) * size;
                for dx in 0..SCALE {
                    let idx = (row + px0 + dx) * 4;
                    rgba[idx] = 0;
                    rgba[idx + 1] = 0;
                    rgba[idx + 2] = 0;
                    rgba[idx + 3] = 255;
                }
            }
        }
    }

    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    let id = format!("qr:{:x}", hasher.finish());

    Some(QrBitmap {
        id,
        rgba,
        size: size as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_a_valid_rgba_bitmap() {
        let qr = encode("https://app.example.test/join?session=abc&secret=xyz").expect("encodes");
        let size = qr.size as usize;
        assert!(size > 0);
        assert_eq!(
            qr.rgba.len(),
            size * size * 4,
            "RGBA buffer must be size*size*4"
        );

        // A real QR has both black and white pixels; a blank buffer would not.
        let has_black = qr.rgba.chunks_exact(4).any(|p| p == [0, 0, 0, 255]);
        let has_white = qr.rgba.chunks_exact(4).any(|p| p == [255, 255, 255, 255]);
        assert!(
            has_black && has_white,
            "expected both dark and light modules"
        );

        // Every pixel is fully opaque.
        assert!(qr.rgba.chunks_exact(4).all(|p| p[3] == 255));
    }

    #[test]
    fn same_url_same_id() {
        let a = encode("https://x/y").unwrap();
        let b = encode("https://x/y").unwrap();
        assert_eq!(a.id, b.id);
        assert_ne!(a.id, encode("https://x/z").unwrap().id);
    }
}
