//! DEFLATE (RFC 1951) and zlib (RFC 1950) encoding.
//!
//! Only the encoder is needed: PNG stores its pixels as a zlib stream. The
//! implementation is a classic greedy LZ77 match finder followed by a
//! *fixed* Huffman code — no dynamic tree, which keeps the encoder small
//! while still compressing our flat backgrounds and text well.

/// LSB-first bit writer used by DEFLATE and WebP.
#[derive(Debug, Default)]
pub struct BitWriter {
    out: Vec<u8>,
    bit_buf: u64,
    bit_count: u32,
}

impl BitWriter {
    /// Creates an empty writer.
    pub fn new() -> BitWriter {
        BitWriter::default()
    }

    /// Appends `count` bits of `value`, least significant bit first.
    pub fn write_bits(&mut self, value: u32, count: u32) {
        debug_assert!(count <= 32);
        if count == 0 {
            return;
        }
        self.bit_buf |= ((value as u64) & ((1u64 << count) - 1)) << self.bit_count;
        self.bit_count += count;
        while self.bit_count >= 8 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    /// Appends a Huffman code, most significant bit first.
    ///
    /// DEFLATE and WebP both store prefix codes so that the decoder reads
    /// them starting from the most significant bit of the canonical code.
    pub fn write_code(&mut self, code: u32, count: u32) {
        for i in (0..count).rev() {
            self.write_bits((code >> i) & 1, 1);
        }
    }

    /// Pads the final byte with zero bits and returns the buffer.
    pub fn finish(mut self) -> Vec<u8> {
        if self.bit_count > 0 {
            self.out.push((self.bit_buf & 0xFF) as u8);
            self.bit_buf = 0;
            self.bit_count = 0;
        }
        self.out
    }
}

/// Length codes 257..=285: base lengths and extra bits.
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Distance codes 0..=29: base distances and extra bits.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Writes a literal byte or length symbol with the fixed Huffman code.
fn write_fixed_symbol(w: &mut BitWriter, symbol: u16) {
    match symbol {
        0..=143 => w.write_code(0x30 + symbol as u32, 8),
        144..=255 => w.write_code(0x190 + (symbol as u32 - 144), 9),
        256..=279 => w.write_code(symbol as u32 - 256, 7),
        _ => w.write_code(0xC0 + (symbol as u32 - 280), 8),
    }
}

/// Compresses `data` into a raw DEFLATE stream using fixed Huffman codes.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.write_bits(1, 1); // BFINAL
    w.write_bits(1, 2); // BTYPE = 01 (fixed Huffman)

    let mut finder = MatchFinder::new();
    let mut pos = 0usize;
    while pos < data.len() {
        let (len, dist) = finder.find(data, pos);
        if len >= 3 {
            let (code_idx, extra_bits, extra_val) = length_code(len as u16);
            write_fixed_symbol(&mut w, 257 + code_idx as u16);
            if extra_bits > 0 {
                w.write_bits(extra_val, extra_bits as u32);
            }
            let (d_idx, d_bits, d_val) = distance_code(dist as u16);
            w.write_code(d_idx as u32, 5);
            if d_bits > 0 {
                w.write_bits(d_val, d_bits as u32);
            }
            finder.insert_range(data, pos, len);
            pos += len;
        } else {
            write_fixed_symbol(&mut w, data[pos] as u16);
            finder.insert(data, pos);
            pos += 1;
        }
    }
    write_fixed_symbol(&mut w, 256);
    w.finish()
}

/// Maps a match length (3..=258) to (code index, extra bits, extra value).
fn length_code(len: u16) -> (usize, u8, u32) {
    for i in (0..LENGTH_BASE.len()).rev() {
        let base = LENGTH_BASE[i];
        let bits = LENGTH_EXTRA[i];
        let max = base + ((1u16 << bits) - 1);
        if len >= base && (len <= max || i == LENGTH_BASE.len() - 1) {
            return (i, bits, (len - base) as u32);
        }
    }
    (0, 0, 0)
}

/// Maps a distance (1..=32768) to (code, extra bits, extra value).
fn distance_code(dist: u16) -> (usize, u8, u32) {
    for i in (0..DIST_BASE.len()).rev() {
        let base = DIST_BASE[i];
        let bits = DIST_EXTRA[i];
        let max = base as u32 + ((1u32 << bits) - 1);
        if dist as u32 >= base as u32 && (dist as u32 <= max || i == DIST_BASE.len() - 1) {
            return (i, bits, dist as u32 - base as u32);
        }
    }
    (0, 0, 0)
}

const WINDOW: usize = 32768;
const MAX_MATCH: usize = 258;
const HASH_SIZE: usize = 1 << 15;

/// Greedy LZ77 match finder with hash chains.
struct MatchFinder {
    head: Vec<i32>,
    prev: Vec<i32>,
    inserted: usize,
}

impl MatchFinder {
    fn new() -> MatchFinder {
        MatchFinder {
            head: vec![-1; HASH_SIZE],
            prev: vec![-1; WINDOW],
            inserted: 0,
        }
    }

    fn hash(data: &[u8], pos: usize) -> usize {
        let a = data[pos] as usize;
        let b = data.get(pos + 1).copied().unwrap_or(0) as usize;
        let c = data.get(pos + 2).copied().unwrap_or(0) as usize;
        ((a << 10) ^ (b << 5) ^ c) & (HASH_SIZE - 1)
    }

    fn insert(&mut self, data: &[u8], pos: usize) {
        if pos + 2 >= data.len() {
            return;
        }
        let h = Self::hash(data, pos);
        self.prev[pos % WINDOW] = self.head[h];
        self.head[h] = pos as i32;
        self.inserted += 1;
    }

    fn insert_range(&mut self, data: &[u8], start: usize, len: usize) {
        for p in start..start + len {
            self.insert(data, p);
        }
    }

    fn find(&mut self, data: &[u8], pos: usize) -> (usize, usize) {
        if pos + 3 > data.len() {
            return (0, 0);
        }
        let h = Self::hash(data, pos);
        let mut candidate = self.head[h];
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let max_len = MAX_MATCH.min(data.len() - pos);
        let mut chain = 0;
        while candidate >= 0 && chain < 24 {
            let c = candidate as usize;
            let dist = pos - c;
            if dist == 0 || dist > WINDOW {
                break;
            }
            // Quick check on the byte after the current best match.
            if best_len < max_len
                && data.get(c + best_len) == data.get(pos + best_len)
                && data[c] == data[pos]
            {
                let mut l = 0usize;
                while l < max_len && data[c + l] == data[pos + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = dist;
                    if l == max_len {
                        break;
                    }
                }
            }
            candidate = self.prev[c % WINDOW];
            chain += 1;
        }
        if best_len >= 3 {
            (best_len, best_dist)
        } else {
            (0, 0)
        }
    }
}

/// Wraps `data` in a zlib container (header + deflate + Adler-32).
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    out.push(0x78);
    out.push(0x01); // fastest compression, no dictionary
    out.extend(deflate(data));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

/// Standard Adler-32 checksum.
pub fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &byte in chunk {
            a += byte as u32;
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// A minimal fixed-Huffman inflater used by tests in this crate.
#[cfg(test)]
pub mod test_inflate {
    use super::{DIST_BASE, DIST_EXTRA, LENGTH_BASE, LENGTH_EXTRA};

    /// Minimal inflater for tests: fixed-Huffman blocks only, which is exactly
    /// what [`deflate`] emits.
    pub fn inflate(data: &[u8]) -> Vec<u8> {
        let mut r = BitReader {
            data,
            pos: 0,
            bit: 0,
        };
        let mut out = Vec::new();
        loop {
            let final_block = r.bits(1);
            let btype = r.bits(2);
            assert_eq!(btype, 1, "test inflater only handles fixed Huffman blocks");
            loop {
                let sym = r.huffman_symbol();
                match sym {
                    0..=255 => out.push(sym as u8),
                    256 => break,
                    257..=285 => {
                        let i = (sym - 257) as usize;
                        let len = LENGTH_BASE[i] as usize + r.bits(LENGTH_EXTRA[i] as u32) as usize;
                        let dcode = r.huffman_distance();
                        let dist =
                            DIST_BASE[dcode] as usize + r.bits(DIST_EXTRA[dcode] as u32) as usize;
                        let start = out.len() - dist;
                        for k in 0..len {
                            let b = out[start + k];
                            out.push(b);
                        }
                    }
                    other => panic!("unexpected symbol {other}"),
                }
            }
            if final_block == 1 {
                break;
            }
        }
        out
    }

    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl BitReader<'_> {
        fn bits(&mut self, n: u32) -> u32 {
            let mut v = 0u32;
            for i in 0..n {
                let byte = self.data[self.pos];
                let b = (byte >> self.bit) & 1;
                v |= (b as u32) << i;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            v
        }

        fn huffman_symbol(&mut self) -> u16 {
            // Read MSB-first canonical codes; the fixed code only resolves at
            // 7, 8 or 9 bits.
            let mut code = 0u32;
            for len in 1..=9 {
                code = (code << 1) | self.bits(1);
                match len {
                    7 if code <= 0x17 => return 256 + code as u16,
                    8 if (0x30..=0xBF).contains(&code) => return (code - 0x30) as u16,
                    8 if (0xC0..=0xC7).contains(&code) => return 280 + (code - 0xC0) as u16,
                    9 if (0x190..=0x1FF).contains(&code) => return 144 + (code - 0x190) as u16,
                    _ => {}
                }
            }
            panic!("invalid fixed Huffman code");
        }

        fn huffman_distance(&mut self) -> usize {
            let mut code = 0u32;
            for _ in 0..5 {
                code = (code << 1) | self.bits(1);
            }
            code as usize
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::deflate::test_inflate::inflate;

    #[test]
    fn empty_payload_roundtrips() {
        let packed = deflate(b"");
        assert_eq!(inflate(&packed), Vec::<u8>::new());
    }

    #[test]
    fn short_text_roundtrips() {
        let src = b"hello, deflate!";
        assert_eq!(inflate(&deflate(src)), src.to_vec());
    }

    #[test]
    fn repetitive_data_roundtrips_and_compresses() {
        let src = b"abcdefghij".repeat(500);
        let packed = deflate(&src);
        assert_eq!(inflate(&packed), src.to_vec());
        assert!(
            packed.len() < src.len() / 4,
            "compressed {} of {}",
            packed.len(),
            src.len()
        );
    }

    #[test]
    fn binary_noise_roundtrips() {
        let mut state = 0x12345678u32;
        let src: Vec<u8> = (0..4096)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 24) as u8
            })
            .collect();
        assert_eq!(inflate(&deflate(&src)), src);
    }

    #[test]
    fn long_matches_use_max_length() {
        let src = vec![7u8; 10_000];
        let packed = deflate(&src);
        assert_eq!(inflate(&packed), src);
        assert!(
            packed.len() < 100,
            "long run compressed to {}",
            packed.len()
        );
    }

    #[test]
    fn adler32_matches_known_values() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"abc"), 0x024d0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E60398);
    }

    #[test]
    fn zlib_header_and_checksum_are_present() {
        let z = zlib(b"hello");
        assert_eq!(&z[0..2], &[0x78, 0x01]);
        let stored = u32::from_be_bytes([
            z[z.len() - 4],
            z[z.len() - 3],
            z[z.len() - 2],
            z[z.len() - 1],
        ]);
        assert_eq!(stored, adler32(b"hello"));
    }

    #[test]
    fn bit_writer_packs_lsb_first() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        w.write_bits(0b11, 2);
        assert_eq!(w.finish(), vec![0b0001_1101]);
    }

    #[test]
    fn bit_writer_writes_codes_msb_first() {
        let mut w = BitWriter::new();
        w.write_code(0b110, 3);
        // First bit written lands in the lowest bit position.
        assert_eq!(w.finish(), vec![0b0000_0011]);
    }

    #[test]
    fn length_and_distance_code_tables_are_consistent() {
        for len in 3..=258u16 {
            let (i, bits, extra) = length_code(len);
            let base = LENGTH_BASE[i] as u32 + extra;
            assert!(base >= len as u32, "len {len} mapped below range");
            assert!(base <= len as u32, "len {len} should map exactly");
            assert!(bits <= 5);
        }
        for dist in [1u16, 2, 4, 5, 8, 100, 1000, 32768] {
            let (i, _, extra) = distance_code(dist);
            assert_eq!(DIST_BASE[i] as u32 + extra, dist as u32);
        }
    }
}
