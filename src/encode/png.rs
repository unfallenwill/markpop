//! PNG encoder (RFC 2083).
//!
//! Writes a truecolor-with-alpha (8-bit RGBA) image: signature, `IHDR`,
//! one `IDAT`, `IEND`. Scanlines use the adaptive filter heuristic from
//! the PNG specification, and the compressed payload is our own zlib
//! implementation.

use std::io::Write;

use crate::encode::deflate;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Encodes an RGBA bitmap as PNG bytes.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    // PNG rejects zero-sized images; normalize and pad the pixel buffer so
    // the encoder is total for any caller-provided input.
    let width = width.max(1);
    let height = height.max(1);
    let expected = width as usize * height as usize * 4;
    let mut padded;
    let rgba: &[u8] = if rgba.len() == expected {
        rgba
    } else {
        padded = vec![0u8; expected];
        let n = rgba.len().min(expected);
        padded[..n].copy_from_slice(&rgba[..n]);
        &padded
    };

    let mut out = Vec::with_capacity(expected / 3 + 1024);
    out.extend_from_slice(&SIGNATURE);

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: truecolor + alpha
    ihdr.push(0); // compression
    ihdr.push(0); // filter method
    ihdr.push(0); // interlace
    write_chunk(&mut out, b"IHDR", &ihdr);

    let filtered = filter_scanlines(width, height, rgba);
    let compressed = deflate::zlib(&filtered);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

/// Applies the adaptive per-row filter and prefixes each row with its id.
fn filter_scanlines(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut out = Vec::with_capacity((stride + 1) * height as usize);
    let mut prev = vec![0u8; stride];
    let mut candidates: Vec<Vec<u8>> = (0..5).map(|_| vec![0u8; stride]).collect();
    let mut row = vec![0u8; stride];

    for y in 0..height as usize {
        row.copy_from_slice(&rgba[y * stride..(y + 1) * stride]);
        let mut best = 0usize;
        let mut best_score = u64::MAX;
        for (kind, buf) in candidates.iter_mut().enumerate() {
            filter_row(kind, &row, &prev, stride, buf);
            let score: u64 = buf.iter().map(|&b| (b as i8 as i64).unsigned_abs()).sum();
            if score < best_score {
                best_score = score;
                best = kind;
            }
            // Fast path: an all-zero row needs no further consideration.
            if score == 0 {
                best = kind;
                break;
            }
        }
        out.push(best as u8);
        out.extend_from_slice(&candidates[best]);
        prev.copy_from_slice(&row);
    }
    out
}

/// Writes one filtered scanline into `out`.
fn filter_row(kind: usize, row: &[u8], prev: &[u8], stride: usize, out: &mut [u8]) {
    const BPP: usize = 4;
    match kind {
        0 => out.copy_from_slice(row),
        1 => {
            for i in 0..stride {
                let left = if i >= BPP { row[i - BPP] } else { 0 };
                out[i] = row[i].wrapping_sub(left);
            }
        }
        2 => {
            for i in 0..stride {
                out[i] = row[i].wrapping_sub(prev[i]);
            }
        }
        3 => {
            for i in 0..stride {
                let left = if i >= BPP { row[i - BPP] } else { 0 };
                let avg = ((left as u16 + prev[i] as u16) / 2) as u8;
                out[i] = row[i].wrapping_sub(avg);
            }
        }
        _ => {
            for i in 0..stride {
                let left = if i >= BPP { row[i - BPP] } else { 0 };
                let up_left = if i >= BPP { prev[i - BPP] } else { 0 };
                out[i] = row[i].wrapping_sub(paeth(left, prev[i], up_left));
            }
        }
    }
}

/// The Paeth predictor from the PNG specification.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    let _ = out.write_all(&(data.len() as u32).to_be_bytes());
    let _ = out.write_all(kind);
    let _ = out.write_all(data);
    let mut crc = Crc32::new();
    crc.update(kind);
    crc.update(data);
    let _ = out.write_all(&crc.finish().to_be_bytes());
}

/// CRC-32 (IEEE) used by PNG chunks.
pub struct Crc32 {
    value: u32,
}

impl Crc32 {
    /// Starts a new checksum.
    pub fn new() -> Crc32 {
        Crc32 { value: 0xFFFF_FFFF }
    }

    /// Feeds bytes into the checksum.
    pub fn update(&mut self, data: &[u8]) {
        for &b in data {
            let idx = ((self.value ^ b as u32) & 0xFF) as usize;
            self.value = CRC_TABLE[idx] ^ (self.value >> 8);
        }
    }

    /// Finalizes the checksum.
    pub fn finish(self) -> u32 {
        self.value ^ 0xFFFF_FFFF
    }
}

/// CRC table, computed at first use.
static CRC_TABLE: std::sync::LazyLock<[u32; 256]> = std::sync::LazyLock::new(|| {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    table
});

impl Default for Crc32 {
    fn default() -> Self {
        Crc32::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::deflate::test_inflate;

    /// Splits a PNG into (type, data) chunk pairs, verifying lengths and CRCs.
    fn chunks(png: &[u8]) -> Vec<(String, Vec<u8>)> {
        assert_eq!(&png[..8], &SIGNATURE, "PNG signature");
        let mut out = Vec::new();
        let mut pos = 8;
        while pos < png.len() {
            let len =
                u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
            let kind = &png[pos + 4..pos + 8];
            let data = &png[pos + 8..pos + 8 + len];
            let crc_at = pos + 8 + len;
            let stored = u32::from_be_bytes([
                png[crc_at],
                png[crc_at + 1],
                png[crc_at + 2],
                png[crc_at + 3],
            ]);
            let mut crc = Crc32::new();
            crc.update(kind);
            crc.update(data);
            assert_eq!(
                stored,
                crc.finish(),
                "bad CRC for chunk {:?}",
                std::str::from_utf8(kind)
            );
            out.push((String::from_utf8_lossy(kind).to_string(), data.to_vec()));
            pos = crc_at + 4;
        }
        out
    }

    fn sample(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[
                    (x * 7 % 256) as u8,
                    (y * 13 % 256) as u8,
                    ((x + y) % 256) as u8,
                    if (x + y) % 5 == 0 { 128 } else { 255 },
                ]);
            }
        }
        data
    }

    /// Reverses the per-row filters.
    fn unfilter(width: u32, height: u32, filtered: &[u8]) -> Vec<u8> {
        let stride = width as usize * 4;
        let mut out = vec![0u8; stride * height as usize];
        let mut prev = vec![0u8; stride];
        let mut pos = 0usize;
        for y in 0..height as usize {
            let kind = filtered[pos];
            pos += 1;
            let row = &mut out[y * stride..(y + 1) * stride];
            row.copy_from_slice(&filtered[pos..pos + stride]);
            pos += stride;
            for i in 0..stride {
                let left = if i >= 4 { row[i - 4] } else { 0 };
                let up = prev[i];
                let up_left = if i >= 4 { prev[i - 4] } else { 0 };
                let add = match kind {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((left as u16 + up as u16) / 2) as u8,
                    4 => paeth(left, up, up_left),
                    other => panic!("unknown filter {other}"),
                };
                row[i] = row[i].wrapping_add(add);
            }
            prev.copy_from_slice(row);
        }
        out
    }

    #[test]
    fn signature_and_chunk_order() {
        let png = encode(2, 2, &sample(2, 2));
        let kinds: Vec<String> = chunks(&png).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
    }

    #[test]
    fn header_describes_an_rgba_image() {
        let png = encode(3, 5, &sample(3, 5));
        let (kind, data) = &chunks(&png)[0];
        assert_eq!(kind, "IHDR");
        assert_eq!(u32::from_be_bytes([data[0], data[1], data[2], data[3]]), 3);
        assert_eq!(u32::from_be_bytes([data[4], data[5], data[6], data[7]]), 5);
        assert_eq!(data[8], 8, "bit depth");
        assert_eq!(data[9], 6, "color type RGBA");
        assert_eq!((data[10], data[11], data[12]), (0, 0, 0));
    }

    #[test]
    fn pixels_survive_the_roundtrip() {
        let (w, h) = (9u32, 7u32);
        let src = sample(w, h);
        let png = encode(w, h, &src);
        let idat = chunks(&png)
            .into_iter()
            .find(|(k, _)| k == "IDAT")
            .map(|(_, d)| d)
            .expect("IDAT chunk");
        // Skip the 2-byte zlib header and 4-byte Adler trailer.
        let raw = test_inflate::inflate(&idat[2..idat.len() - 4]);
        let restored = unfilter(w, h, &raw);
        assert_eq!(restored, src);
    }

    #[test]
    fn solid_images_compress_well() {
        let (w, h) = (200u32, 200u32);
        let src = vec![255u8; (w * h * 4) as usize];
        let png = encode(w, h, &src);
        assert!(png.len() < 2000, "solid image took {} bytes", png.len());
    }

    #[test]
    fn crc_matches_known_value() {
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }

    #[test]
    fn paeth_predictor_examples() {
        assert_eq!(paeth(0, 0, 0), 0);
        // p = 15, so the "up-left" predictor wins (distance 0).
        assert_eq!(paeth(10, 20, 15), 15);
        assert_eq!(paeth(10, 20, 25), 10);
        assert_eq!(paeth(1, 2, 3), 1);
        assert_eq!(paeth(100, 0, 0), 100);
    }

    #[test]
    fn every_filter_is_reversible() {
        let stride = 16usize;
        let row: Vec<u8> = (0..stride).map(|i| (i * 37 % 256) as u8).collect();
        let prev: Vec<u8> = (0..stride).map(|i| (i * 91 % 256) as u8).collect();
        for kind in 0..5 {
            let mut filtered = vec![0u8; stride];
            filter_row(kind, &row, &prev, stride, &mut filtered);
            // Reverse manually.
            let mut restored = filtered.clone();
            for i in 0..stride {
                let left = if i >= 4 { restored[i - 4] } else { 0 };
                let up = prev[i];
                let up_left = if i >= 4 { prev[i - 4] } else { 0 };
                let add = match kind {
                    0 => 0,
                    1 => left,
                    2 => up,
                    3 => ((left as u16 + up as u16) / 2) as u8,
                    _ => paeth(left, up, up_left),
                };
                restored[i] = restored[i].wrapping_add(add);
            }
            assert_eq!(restored, row, "filter {kind} did not roundtrip");
        }
    }

    #[test]
    fn empty_image_is_encodable() {
        let png = encode(0, 0, &[]);
        let kinds: Vec<String> = chunks(&png).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
    }
}
