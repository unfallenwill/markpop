//! PNG decoding (RFC 2083), scoped to what document images actually use.
//!
//! Supported: 8/16-bit gray, RGB, palette (+tRNS), gray+alpha, RGBA;
//! non-interlaced only. Adam7 files decode to `None` and callers fall
//! back to the image placeholder, which keeps the decoder small without
//! ever failing loudly.

use super::inflate::inflate_zlib;

/// Hard cap on total pixels; a corrupt or hostile header must not make
/// us allocate gigabytes.
const MAX_PIXELS: u64 = 100_000_000;

pub(crate) const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];


/// One decoded PNG image as straight-alpha RGBA8.
pub(crate) struct DecodedPng {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, straight (non-premultiplied) alpha.
    pub pixels: Vec<u8>,
}

/// Decodes a PNG file. Any structural problem returns `None`.
pub(crate) fn decode_png(data: &[u8]) -> Option<DecodedPng> {
    if data.len() < 8 || data[..8] != SIGNATURE {
        return None;
    }
    let mut width = 0u32;
    let mut height = 0u32;
    let mut depth = 0u8;
    let mut color = 0u8;
    let mut interlace = 0u8;
    let mut palette: Vec<[u8; 3]> = Vec::new();
    let mut trns: Vec<u8> = Vec::new();
    let mut trns_gray: Option<u16> = None;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_ihdr = false;

    let mut pos = 8usize;
    loop {
        let chunk_len = u32::from_be_bytes(data.get(pos..pos + 4)?.try_into().ok()?) as usize;
        let kind = data.get(pos + 4..pos + 8)?;
        let body = data.get(pos + 8..pos + 8 + chunk_len)?;
        let crc = u32::from_be_bytes(data.get(pos + 8 + chunk_len..pos + 12 + chunk_len)?.try_into().ok()?);
        if crc32(&data[pos + 4..pos + 8 + chunk_len]) != crc {
            return None;
        }
        match kind {
            b"IHDR" => {
                if body.len() != 13 {
                    return None;
                }
                width = u32::from_be_bytes(body[0..4].try_into().ok()?);
                height = u32::from_be_bytes(body[4..8].try_into().ok()?);
                depth = body[8];
                color = body[9];
                // Compression and filter method must both be 0.
                if body[10] != 0 || body[11] != 0 {
                    return None;
                }
                interlace = body[12];
                seen_ihdr = true;
            }
            b"PLTE" => {
                if body.is_empty() || body.len() % 3 != 0 || body.len() > 256 * 3 {
                    return None;
                }
                palette = body.as_chunks::<3>().0.iter().map(|c| [c[0], c[1], c[2]]).collect();
            }
            b"tRNS" => match color {
                3 => trns = body.to_vec(),
                0 if body.len() == 2 => {
                    trns_gray = Some(u16::from_be_bytes(body.try_into().ok()?))
                }
                _ => {}
            },
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + chunk_len;
        if pos + 12 > data.len() && kind != b"IEND" {
            return None;
        }
    }
    if !seen_ihdr || width == 0 || height == 0 || idat.is_empty() {
        return None;
    }
    if u64::from(width) * u64::from(height) > MAX_PIXELS || interlace != 0 {
        return None;
    }
    let channels = match color {
        0 => 1usize,
        2 => 3,
        3 => 1,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    let depth_ok = match color {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        _ => matches!(depth, 8 | 16),
    };
    if !depth_ok {
        return None;
    }

    let raw = inflate_zlib(&idat)?;
    let stride = (width as usize * channels * depth as usize).div_ceil(8);
    if raw.len() < height as usize * (stride + 1) {
        return None;
    }
    let bytes = unfilter(height as usize, stride, &raw)?;

    let pixels = assemble(width, height, &bytes, channels, depth, color, &palette, trns.as_slice(), trns_gray)?;
    Some(DecodedPng { width, height, pixels })
}

/// Reverses the per-row PNG filters into a buffer of packed scanlines.
fn unfilter(h: usize, stride: usize, raw: &[u8]) -> Option<Vec<u8>> {
    let bpp = 4usize; // filtering works on bytes; 4 covers every depth ≥ 8
    let mut out = vec![0u8; h * stride];
    let mut pos = 0usize;
    for row in 0..h {
        let filter = *raw.get(pos)?;
        pos += 1;
        let line = &raw[pos..pos + stride];
        let this = row * stride;
        let prev = this.saturating_sub(stride);
        for col in 0..stride {
            let x = line[col] as i32;
            let a = if col >= bpp { out[this + col - bpp] as i32 } else { 0 };
            let b = if row > 0 { out[prev + col] as i32 } else { 0 };
            let c = if col >= bpp && row > 0 { out[prev + col - bpp] as i32 } else { 0 };
            let value = match filter {
                0 => x,
                1 => x + a,
                2 => x + b,
                3 => x + (a + b) / 2,
                4 => {
                    let p = a + b - c;
                    let pa = (p - a).abs();
                    let pb = (p - b).abs();
                    let pc = (p - c).abs();
                    x + if pa <= pb && pa <= pc { a } else if pb <= pc { b } else { c }
                }
                _ => return None,
            };
            out[this + col] = (value as u32 & 0xff) as u8;
        }
        pos += stride;
    }
    Some(out)
}

/// Expands packed sub-byte samples of one scanline into one byte each.
fn unpack_sub_byte(row: &[u8], w: usize, depth: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(w);
    let per_byte = 8 / depth as usize;
    let mask = (1u16 << depth) as u8 - 1;
    for byte in row.iter().take(w.div_ceil(per_byte)) {
        for shift in (0..per_byte).rev() {
            if out.len() == w {
                break;
            }
            out.push((byte >> (shift * depth as usize)) & mask);
        }
    }
    out
}

fn sample(row: &[u8], index: usize, depth: u8, channels: usize) -> u8 {
    if depth == 16 {
        // Take the high byte of the big-endian sample.
        row[index * channels * 2]
    } else if depth == 8 {
        row[index * channels]
    } else {
        row[index]
    }
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    w: u32,
    h: u32,
    bytes: &[u8],
    channels: usize,
    depth: u8,
    color: u8,
    palette: &[[u8; 3]],
    trns: &[u8],
    trns_gray: Option<u16>,
) -> Option<Vec<u8>> {
    let w = w as usize;
    let h = h as usize;
    // Bytes per packed scanline (the filter byte was already stripped).
    let row_len = if depth >= 8 {
        w * channels * (depth as usize / 8)
    } else {
        w.div_ceil(8 / depth as usize)
    };
    let mut pixels = vec![0u8; w * h * 4];
    let mut packed = Vec::new();
    for (row_index, row_bytes) in bytes.chunks(row_len.max(1)).take(h).enumerate() {
        let row = if depth < 8 {
            packed.clear();
            packed.extend(unpack_sub_byte(row_bytes, w, depth));
            &packed
        } else {
            row_bytes
        };
        for col in 0..w {
            let out = &mut pixels[(row_index * w + col) * 4..(row_index * w + col) * 4 + 4];
            match color {
                0 => {
                    let g = sample(row, col, depth, channels);
                    let transparent = trns_gray.is_some_and(|t| raw_sample(row, col, depth) == t);
                    let [r, gg, b] = scale_gray(g, depth);
                    out[0] = r;
                    out[1] = gg;
                    out[2] = b;
                    out[3] = if transparent { 0 } else { 255 };
                }
                2 => {
                    for slot in 0..3 {
                        out[slot] = if depth == 16 {
                            row[col * 6 + slot * 2]
                        } else {
                            row[col * 3 + slot]
                        };
                    }
                    out[3] = 255;
                }
                3 => {
                    let idx = sample(row, col, depth, channels) as usize;
                    let entry = *palette.get(idx)?;
                    out[0] = entry[0];
                    out[1] = entry[1];
                    out[2] = entry[2];
                    out[3] = *trns.get(idx).unwrap_or(&255);
                }
                4 => {
                    let [r, gg, b] = scale_gray(sample(row, col, depth, channels), depth);
                    out[0] = r;
                    out[1] = gg;
                    out[2] = b;
                    out[3] = if depth == 16 { row[col * 4] } else { row[col * 2 + 1] };
                }
                6 => {
                    let step = if depth == 16 { 2 } else { 1 };
                    for slot in 0..4 {
                        out[slot] = row[col * 4 + slot * step];
                    }
                }
                _ => return None,
            }
        }
    }
    Some(pixels)
}

/// Full-width raw sample (used for 16-bit grayscale tRNS matching).
fn raw_sample(row: &[u8], col: usize, depth: u8) -> u16 {
    if depth == 16 {
        u16::from_be_bytes([row[col * 2], row[col * 2 + 1]])
    } else {
        u16::from(row[col])
    }
}

/// Scales a gray sample of any depth to 8 bits.
fn scale_gray(g: u8, depth: u8) -> [u8; 3] {
    let v = match depth {
            1 => g * 255,
            2 => g * 85,
            4 => g * 17,
            _ => g,
    };
    [v, v, v]
}

/// CRC-32 (ISO 3309), as required for every PNG chunk.
fn crc32(data: &[u8]) -> u32 {
    // Called once per chunk; a table is unnecessary at this frequency.
    data.iter().fold(0xffff_ffffu32, |crc, &byte| {
        let crc = crc ^ u32::from(byte);
        (0..8).fold(crc, |crc, _| if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 })
    }) ^ 0xffff_ffff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::png as png_enc;

    #[test]
    fn roundtrips_own_encoder_output() {
        // 3x2 RGBA, one transparent corner pixel.
        let w = 3u32;
        let h = 2u32;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px[0] = (i * 37 % 256) as u8;
            px[1] = (i * 91 % 256) as u8;
            px[2] = (i * 13 % 256) as u8;
            px[3] = if i == 4 { 0 } else { 255 };
        }
        let file = png_enc::encode(w, h, &rgba);
        let decoded = decode_png(&file).expect("decodes");
        assert_eq!((decoded.width, decoded.height), (w, h));
        assert_eq!(decoded.pixels, rgba);
    }

    #[test]
    fn rejects_truncated_and_wrong_magic() {
        assert!(decode_png(b"not a png").is_none());
        let file = png_enc::encode(2, 2, &[128u8; 16]);
        assert!(decode_png(&file[..file.len() / 2]).is_none());
    }

    #[test]
    fn rejects_bad_crc() {
        let mut file = png_enc::encode(2, 2, &[128u8; 16]);
        let last = file.len() - 1;
        file[last] ^= 0xff;
        assert!(decode_png(&file).is_none());
    }

    #[test]
    fn decodes_8bpp_gray() {
        // Hand-built 2x1 grayscale PNG: IHDR (color 0, depth 8), IDAT, IEND.
        let raw = [0u8, 0x40, 0x80]; // filter 0, two gray samples
        let idat = crate::encode::deflate::zlib(&raw);
        let mut file = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
        push_chunk(&mut file, b"IHDR", &ihdr);
        push_chunk(&mut file, b"IDAT", &idat);
        push_chunk(&mut file, b"IEND", &[]);
        let decoded = decode_png(&file).expect("decodes");
        assert_eq!(decoded.pixels, vec![64, 64, 64, 255, 128, 128, 128, 255]);
    }

    fn push_chunk(file: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
        file.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let start = file.len();
        file.extend_from_slice(kind);
        file.extend_from_slice(body);
        let crc = crc32(&file[start..]);
        file.extend_from_slice(&crc.to_be_bytes());
    }
}
