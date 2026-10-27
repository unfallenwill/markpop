//! Embedding local images into a Word document.
//!
//! Word does not support BMP, so every image is decoded to RGBA8 and
//! re-encoded as PNG — the one raster format Word always accepts.

use std::path::Path;

use crate::decode::decode;
use crate::encode::png::encode;

/// EMU per pixel at 96 dpi (914400 EMU per inch).
pub const EMU_PER_PX: u32 = 9525;
/// Width of the A4 content box, in EMU (9026 twips at 635 EMU per twip).
pub const CONTENT_WIDTH_EMU: u32 = 5_731_510;
/// Height of the A4 content box, in EMU (13958 twips).
pub const CONTENT_HEIGHT_EMU: u32 = 8_864_330;

/// Converts a pixel size to EMU, scaled to fit the content box.
///
/// The aspect ratio is preserved; an image that already fits is untouched.
///
/// ```
/// use markpop::docx::media::{fit_emu, EMU_PER_PX};
/// assert_eq!(fit_emu(96, 48), (96 * EMU_PER_PX, 48 * EMU_PER_PX));
/// ```
pub fn fit_emu(width_px: u32, height_px: u32) -> (u32, u32) {
    let w = width_px.saturating_mul(EMU_PER_PX);
    let h = height_px.saturating_mul(EMU_PER_PX);
    if w <= CONTENT_WIDTH_EMU && h <= CONTENT_HEIGHT_EMU {
        return (w, h);
    }
    let scale = (CONTENT_WIDTH_EMU as f64 / w as f64).min(CONTENT_HEIGHT_EMU as f64 / h as f64);
    (
        (w as f64 * scale).round() as u32,
        (h as f64 * scale).round() as u32,
    )
}

/// Reads an image file and returns PNG bytes plus its pixel size.
///
/// Returns `None` when the file cannot be read or decoded; the caller
/// degrades to the image's alt text.
pub fn load_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let raw = std::fs::read(path).ok()?;
    let decoded = decode(&raw)?;
    let png = encode(decoded.width, decoded.height, &decoded.pixels);
    Some((png, decoded.width, decoded.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::SIGNATURE;

    /// Builds a 2×2 24-bit BMP, the format Word does not accept.
    fn bmp_bytes() -> Vec<u8> {
        let row = 8usize; // 2 pixels × 3 bytes, padded to 4
        let pixels = row * 2;
        let mut out = Vec::new();
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&((54 + pixels) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes()); // DIB header
        out.extend_from_slice(&2i32.to_le_bytes()); // width
        out.extend_from_slice(&2i32.to_le_bytes()); // height
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&0u32.to_le_bytes()); // compression
        out.extend_from_slice(&(pixels as u32).to_le_bytes());
        out.extend_from_slice(&0i32.to_le_bytes()); // x pixels per meter
        out.extend_from_slice(&0i32.to_le_bytes()); // y pixels per meter
        out.extend_from_slice(&0u32.to_le_bytes()); // colors used
        out.extend_from_slice(&0u32.to_le_bytes()); // important colors
        // Two rows of two blue pixels, stored bottom-up and padded to a
        // four-byte boundary.
        out.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00]);
        out.extend_from_slice(&[0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00]);
        out
    }

    /// Writes a file into the temporary directory.
    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("markpop-media-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write");
        path
    }

    #[test]
    fn pixels_become_emu_at_96_dpi() {
        assert_eq!(fit_emu(96, 48), (96 * EMU_PER_PX, 48 * EMU_PER_PX));
    }

    #[test]
    fn a_wide_image_is_scaled_down_to_the_content_box() {
        let (w, h) = fit_emu(4000, 1000);
        assert!((w as i64 - CONTENT_WIDTH_EMU as i64).abs() <= 2, "width {w}");
        // The aspect ratio survives the scaling.
        assert!((h as i64 - w as i64 / 4).abs() <= 2, "height {h}");
    }

    #[test]
    fn a_tall_image_is_scaled_down_to_the_content_box() {
        let (w, h) = fit_emu(100, 20000);
        assert!((h as i64 - CONTENT_HEIGHT_EMU as i64).abs() <= 2, "height {h}");
        assert!(w < CONTENT_WIDTH_EMU);
    }

    #[test]
    fn a_missing_file_loads_as_none() {
        assert!(load_png(std::path::Path::new("no-such-image.png")).is_none());
    }

    #[test]
    fn a_bmp_becomes_a_png() {
        let path = temp_file("image.bmp", &bmp_bytes());
        let (png, w, h) = load_png(&path).expect("a decodable image");
        let _ = std::fs::remove_file(&path);
        assert_eq!((w, h), (2, 2));
        assert_eq!(&png[..8], &SIGNATURE);
        // The blue pixels came through the conversion.
        let decoded = crate::decode::decode(&png).expect("decodes");
        assert_eq!(&decoded.pixels[..4], &[0, 0, 255, 255]);
    }

    #[test]
    fn a_png_survives_the_conversion() {
        let source = crate::encode::png::encode(2, 2, &[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 255]);
        let path = temp_file("image.png", &source);
        let (png, w, h) = load_png(&path).expect("a decodable image");
        let _ = std::fs::remove_file(&path);
        assert_eq!((w, h), (2, 2));
        assert_eq!(&png[..8], &SIGNATURE);
        // The pixels come back unchanged.
        let decoded = crate::decode::decode(&png).expect("decodes");
        assert_eq!(decoded.pixels[0], 255);
        assert_eq!(decoded.pixels[1], 0);
        assert_eq!(decoded.pixels[2], 0);
    }

    #[test]
    fn a_file_that_is_not_an_image_loads_as_none() {
        let path = temp_file("image.txt", b"not an image at all");
        assert!(load_png(&path).is_none());
        let _ = std::fs::remove_file(&path);
    }
}
