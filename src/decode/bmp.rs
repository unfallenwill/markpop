//! BMP decoding, scoped to uncompressed files as produced by common
//! tools: 24/32-bit truecolor and 8-bit palette, BI_RGB only. Anything
//! else (RLE, bitfields, CMYK) decodes to `None` and callers fall back
//! to the image placeholder.

use super::Decoded;

/// Decodes an uncompressed BMP file. Any structural problem returns `None`.
pub(crate) fn decode_bmp(data: &[u8]) -> Option<Decoded> {
    if data.len() < 54 || &data[0..2] != b"BM" {
        return None;
    }
    let pixel_offset = u32::from_le_bytes(data[10..14].try_into().ok()?) as usize;
    let header_size = u32::from_le_bytes(data[14..18].try_into().ok()?) as usize;
    if header_size < 40 || data.len() < 18 + 36 {
        return None;
    }
    let width = i32::from_le_bytes(data[18..22].try_into().ok()?);
    let height_raw = i32::from_le_bytes(data[22..26].try_into().ok()?);
    let planes = u16::from_le_bytes(data[26..28].try_into().ok()?);
    let bpp = u16::from_le_bytes(data[28..30].try_into().ok()?);
    let compression = u32::from_le_bytes(data[30..34].try_into().ok()?);
    if planes != 1 || compression != 0 {
        return None;
    }
    let top_down = height_raw < 0;
    let height = height_raw.unsigned_abs();
    let width = u32::try_from(width).ok()?;
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > super::MAX_PIXELS {
        return None;
    }
    let palette_colors = u32::from_le_bytes(data[46..50].try_into().ok()?) as usize;

    let palette = if bpp == 8 {
        let count = if palette_colors == 0 { 256 } else { palette_colors };
        let start = 14 + header_size;
        let mut table = Vec::with_capacity(count);
        for i in 0..count {
            let at = start + i * 4;
            let entry = data.get(at..at + 4)?;
            table.push([entry[2], entry[1], entry[0], 255]); // stored BGRX
        }
        table
    } else {
        Vec::new()
    };

    let stride = (width as usize * bpp as usize).div_ceil(32) * 4;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for row in 0..height as usize {
        // BMP rows run bottom-up unless the height is negative.
        let source_row = if top_down { row } else { height as usize - 1 - row };
        let line = data
            .get(pixel_offset + source_row * stride..pixel_offset + source_row * stride + stride)?;
        for col in 0..width as usize {
            let out = &mut pixels[(row * width as usize + col) * 4..][..4];
            match bpp {
                24 => {
                    let at = col * 3;
                    out[0] = line[at + 2];
                    out[1] = line[at + 1];
                    out[2] = line[at];
                    out[3] = 255;
                }
                32 => {
                    let at = col * 4;
                    out[0] = line[at + 2];
                    out[1] = line[at + 1];
                    out[2] = line[at];
                    // BI_RGB gives the fourth byte no defined meaning, and
                    // most writers leave it zeroed: treat as opaque.
                    out[3] = 255;
                }
                8 => {
                    let entry = palette.get(line[col] as usize)?;
                    out.copy_from_slice(entry);
                }
                _ => return None,
            }
        }
    }
    Some(Decoded { width, height, pixels })
}
