//! WebP **lossless** encoder (VP8L bitstream).
//!
//! The container is RIFF (`RIFF`/`WEBP`/`VP8L`), and the payload is the
//! lossless bitstream described by the WebP lossless specification:
//!
//! ```text
//! 14 bits width-1 | 14 bits height-1 | 1 bit alpha | 3 bits version
//! 1 bit "no transform"
//! 1 bit "no color cache" | 1 bit "no meta prefix image"
//! five prefix codes (green+length, red, blue, alpha, distance)
//! pixels: entropy-coded literals and LZ77 backward references
//! ```
//!
//! We use no transforms and no color cache — the encoder is deliberately
//! simple — but the pixel stream is real LZ77 plus length-limited Huffman
//! codes, which is what makes the output smaller than a naive literal
//! stream.

use crate::encode::deflate::BitWriter;

/// Green-channel alphabet: 256 literals + 24 length prefix codes.
const GREEN_ALPHABET: usize = 256 + 24;
/// Distance alphabet: 40 prefix codes.
const DIST_ALPHABET: usize = 40;
/// Maximum backward-reference length.
const MAX_LENGTH: usize = 4096;
/// Maximum backward-reference distance.
///
/// WebP encodes a distance as `distance_code = dist + 120`, and distance
/// codes max out at 1048576, so the largest usable distance is 120 less.
const MAX_DISTANCE: u32 = 1_048_576 - 120;
/// Huffman codes are limited to 15 bits by the format.
const MAX_CODE_LENGTH: usize = 15;

/// One unit of the entropy-coded pixel stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    /// A literal ARGB pixel.
    Literal { a: u8, r: u8, g: u8, b: u8 },
    /// A copy of `len` pixels starting `dist` pixels back.
    Copy { len: u16, dist: u32 },
}

/// Encodes an RGBA bitmap as a lossless WebP file (RIFF container).
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    // The VP8L header stores dimensions in 14 bits each.
    let width = width.clamp(1, 16384);
    let height = height.clamp(1, 16384);
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
    let pixels: Vec<u32> = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| ((p[3] as u32) << 24) | ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32)
        .collect();
    let payload = encode_vp8l(width, height, &pixels);

    let mut out = Vec::with_capacity(payload.len() + 32);
    out.extend_from_slice(b"RIFF");
    let riff_size = 4 + 8 + payload.len() + (payload.len() & 1);
    out.extend_from_slice(&(riff_size as u32).to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(b"VP8L");
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(&payload);
    if payload.len() & 1 == 1 {
        out.push(0);
    }
    out
}

/// Builds the VP8L bitstream (everything after the `VP8L` size field).
fn encode_vp8l(width: u32, height: u32, pixels: &[u32]) -> Vec<u8> {
    let tokens = tokenize(pixels);
    let alpha_used = pixels.iter().any(|p| (p >> 24) != 0xFF);

    // Histograms per alphabet.
    let mut green = vec![0u32; GREEN_ALPHABET];
    let mut red = vec![0u32; 256];
    let mut blue = vec![0u32; 256];
    let mut alpha = vec![0u32; 256];
    let mut dist = vec![0u32; DIST_ALPHABET];
    for token in &tokens {
        match *token {
            Token::Literal { a, r, g, b } => {
                green[g as usize] += 1;
                red[r as usize] += 1;
                blue[b as usize] += 1;
                alpha[a as usize] += 1;
            }
            Token::Copy { len, dist: d } => {
                let (lcode, _, _) = prefix_encode(len as u32);
                green[256 + lcode as usize] += 1;
                let (dcode, _, _) = prefix_encode(d + 120);
                dist[dcode as usize] += 1;
            }
        }
    }

    let h_green = Huffman::build(&green);
    let h_red = Huffman::build(&red);
    let h_blue = Huffman::build(&blue);
    let h_alpha = Huffman::build(&alpha);
    let h_dist = Huffman::build(&dist);

    let mut w = BitWriter::new();
    w.write_bits(width - 1, 14);
    w.write_bits(height - 1, 14);
    w.write_bits(alpha_used as u32, 1);
    w.write_bits(0, 3); // version
    w.write_bits(0, 1); // no transform
    w.write_bits(0, 1); // no color cache
    w.write_bits(0, 1); // no meta prefix image

    h_green.write_tree(&mut w);
    h_red.write_tree(&mut w);
    h_blue.write_tree(&mut w);
    h_alpha.write_tree(&mut w);
    h_dist.write_tree(&mut w);

    for token in &tokens {
        match *token {
            Token::Literal { a, r, g, b } => {
                h_green.write_symbol(&mut w, g as usize);
                h_red.write_symbol(&mut w, r as usize);
                h_blue.write_symbol(&mut w, b as usize);
                h_alpha.write_symbol(&mut w, a as usize);
            }
            Token::Copy { len, dist: d } => {
                let (lcode, lbits, lextra) = prefix_encode(len as u32);
                h_green.write_symbol(&mut w, 256 + lcode as usize);
                if lbits > 0 {
                    w.write_bits(lextra, lbits as u32);
                }
                debug_assert!(d + 120 <= 1_048_576, "distance out of range");
                let (dcode, dbits, dextra) = prefix_encode(d + 120);
                h_dist.write_symbol(&mut w, dcode as usize);
                if dbits > 0 {
                    w.write_bits(dextra, dbits as u32);
                }
            }
        }
    }
    let mut payload = vec![0x2F]; // VP8L signature
    payload.extend(w.finish());
    payload
}

/// Greedy LZ77 over ARGB pixels, restricted to distances WebP can express.
fn tokenize(pixels: &[u32]) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(pixels.len() / 2 + 1);
    const HASH_BITS: usize = 16;
    const HASH_SIZE: usize = 1 << HASH_BITS;
    const WINDOW: usize = 1 << 20;
    const MAX_CHAIN: usize = 24;

    let mut head = vec![-1i32; HASH_SIZE];
    let mut prev = vec![-1i32; WINDOW];
    let hash = |p: &[u32], i: usize| -> usize {
        let a = p[i];
        let b = p.get(i + 1).copied().unwrap_or(0);
        let c = p.get(i + 2).copied().unwrap_or(0);
        let h =
            a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA6B) ^ c.wrapping_mul(0xC2B2_AE35);
        (h as usize >> (32 - HASH_BITS)) & (HASH_SIZE - 1)
    };
    let insert = |head: &mut Vec<i32>, prev: &mut Vec<i32>, p: &[u32], i: usize| {
        if i + 2 >= p.len() {
            return;
        }
        let h = hash(p, i);
        prev[i % WINDOW] = head[h];
        head[h] = i as i32;
    };

    let mut pos = 0usize;
    while pos < pixels.len() {
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        if pos + 3 <= pixels.len() {
            let h = hash(pixels, pos);
            let mut candidate = head[h];
            let mut chain = 0;
            let max_len = MAX_LENGTH.min(pixels.len() - pos);
            while candidate >= 0 && chain < MAX_CHAIN {
                let c = candidate as usize;
                let dist = pos - c;
                if dist == 0 || dist > MAX_DISTANCE as usize {
                    break;
                }
                if pixels[c] == pixels[pos]
                    && best_len < max_len
                    && pixels[c + best_len] == pixels[pos + best_len]
                {
                    let mut l = 1usize;
                    while l < max_len && pixels[c + l] == pixels[pos + l] {
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
                candidate = prev[c % WINDOW];
                chain += 1;
            }
        }

        if best_len >= 3 {
            let p = pixels[pos];
            let _ = p;
            tokens.push(Token::Copy {
                len: best_len as u16,
                dist: best_dist as u32,
            });
            for i in pos..pos + best_len {
                insert(&mut head, &mut prev, pixels, i);
            }
            pos += best_len;
        } else {
            let p = pixels[pos];
            tokens.push(Token::Literal {
                a: (p >> 24) as u8,
                r: (p >> 16) as u8,
                g: (p >> 8) as u8,
                b: p as u8,
            });
            insert(&mut head, &mut prev, pixels, pos);
            pos += 1;
        }
    }
    tokens
}

/// WebP's LZ77 prefix coding: maps a value to (code, extra bits, extra value).
/// Base value and extra-bit count of a prefix code.
fn prefix_base(code: u8) -> (u32, u8) {
    if code < 4 {
        (code as u32 + 1, 0)
    } else {
        let e = code / 2 - 1;
        ((1u32 << (e + 1)) + 1 + ((code as u32 & 1) << e), e)
    }
}

/// WebP's LZ77 prefix coding: maps a value to (code, extra bits, extra value).
fn prefix_encode(value: u32) -> (u8, u8, u32) {
    if value <= 4 {
        return (value.saturating_sub(1) as u8, 0, 0);
    }
    for code in 4u8..40 {
        let (base, e) = prefix_base(code);
        let size = 1u32 << e;
        if value >= base && value < base + size {
            return (code, e, value - base);
        }
    }
    // Maximum value (1048576) maps to code 39.
    let (base, e) = prefix_base(39);
    (39, e, value.saturating_sub(base))
}

/// A canonical Huffman code with per-symbol lengths.
#[derive(Debug, Clone)]
struct Huffman {
    lengths: Vec<u8>,
    codes: Vec<u32>,
    used: Vec<usize>,
}

impl Huffman {
    /// Builds a length-limited (15-bit) Huffman code from symbol counts.
    fn build(freqs: &[u32]) -> Huffman {
        let used: Vec<usize> = freqs
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f > 0)
            .map(|(i, _)| i)
            .collect();
        let lengths = if used.is_empty() {
            // An empty alphabet is represented as a single symbol 0.
            let mut l = vec![0u8; freqs.len()];
            l[0] = 1;
            l
        } else if used.len() == 1 {
            let mut l = vec![0u8; freqs.len()];
            l[used[0]] = 1;
            l
        } else {
            package_merge(freqs, MAX_CODE_LENGTH)
        };
        let codes = canonical_codes(&lengths);
        let used = lengths
            .iter()
            .enumerate()
            .filter(|&(_, &l)| l > 0)
            .map(|(i, _)| i)
            .collect();
        Huffman {
            lengths,
            codes,
            used,
        }
    }

    /// Number of symbols with a non-zero code length.
    fn symbol_count(&self) -> usize {
        self.used.len()
    }

    /// Writes the code description in the format's simple-or-normal form.
    fn write_tree(&self, w: &mut BitWriter) {
        let simple = self.symbol_count() <= 2 && self.used.iter().all(|&s| s <= 255);
        if simple {
            w.write_bits(1, 1);
            let n = self.symbol_count();
            w.write_bits((n - 1) as u32, 1);
            let first = self.used.first().copied().unwrap_or(0);
            let eight = first > 1;
            w.write_bits(eight as u32, 1);
            w.write_bits(first as u32, if eight { 8 } else { 1 });
            if n == 2 {
                w.write_bits(self.used[1] as u32, 8);
            }
            return;
        }

        w.write_bits(0, 1); // normal code
        w.write_bits(15, 4); // num_code_lengths - 4 = 15 -> 19 lengths
        // Code-length code: symbols 0..=12 get 4 bits, 13..=18 get 5 bits.
        const ORDER: [usize; 19] = [
            17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ];
        for &sym in ORDER.iter() {
            let len = if sym <= 12 { 4 } else { 5 };
            w.write_bits(len, 3);
        }
        w.write_bits(0, 1); // max_symbol = full alphabet

        let cl_lengths: Vec<u8> = (0..19).map(|s| if s <= 12 { 4 } else { 5 }).collect();
        let cl_codes = canonical_codes(&cl_lengths);
        let mut i = 0usize;
        while i < self.lengths.len() {
            let len = self.lengths[i];
            if len == 0 {
                let mut run = 0usize;
                while i + run < self.lengths.len() && self.lengths[i + run] == 0 {
                    run += 1;
                }
                let mut remaining = run;
                while remaining >= 11 {
                    let take = remaining.min(138);
                    write_canonical(w, &cl_lengths, &cl_codes, 18, None);
                    w.write_bits((take - 11) as u32, 7);
                    remaining -= take;
                }
                while remaining >= 3 {
                    let take = remaining.min(10);
                    write_canonical(w, &cl_lengths, &cl_codes, 17, None);
                    w.write_bits((take - 3) as u32, 3);
                    remaining -= take;
                }
                for _ in 0..remaining {
                    write_canonical(w, &cl_lengths, &cl_codes, 0, None);
                }
                i += run;
            } else {
                // Repeat the previous non-zero length with code 16 where useful.
                let mut run = 1usize;
                while i + run < self.lengths.len() && self.lengths[i + run] == len {
                    run += 1;
                }
                write_canonical(w, &cl_lengths, &cl_codes, len as usize, None);
                let mut left = run - 1;
                while left >= 3 {
                    let take = left.min(6);
                    write_canonical(w, &cl_lengths, &cl_codes, 16, None);
                    w.write_bits((take - 3) as u32, 2);
                    left -= take;
                }
                for _ in 0..left {
                    write_canonical(w, &cl_lengths, &cl_codes, len as usize, None);
                }
                i += run;
            }
        }
    }

    /// Writes one symbol; a single-leaf code consumes no bits at all.
    fn write_symbol(&self, w: &mut BitWriter, symbol: usize) {
        if self.symbol_count() <= 1 {
            return;
        }
        let len = self.lengths[symbol];
        if len == 0 {
            return;
        }
        w.write_code(self.codes[symbol], len as u32);
    }
}

/// Writes a canonical code, optionally with extra bits.
fn write_canonical(
    w: &mut BitWriter,
    lengths: &[u8],
    codes: &[u32],
    symbol: usize,
    extra: Option<(u32, u32)>,
) {
    let len = lengths[symbol];
    if len > 0 {
        w.write_code(codes[symbol], len as u32);
    }
    if let Some((value, bits)) = extra {
        w.write_bits(value, bits);
    }
}

/// Canonical Huffman code assignment (same ordering as DEFLATE/WebP).
fn canonical_codes(lengths: &[u8]) -> Vec<u32> {
    let max = lengths.iter().copied().max().unwrap_or(0) as usize;
    let mut bl_count = vec![0u32; max + 1];
    for &l in lengths {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next_code = vec![0u32; max + 1];
    let mut code = 0u32;
    for bits in 1..=max {
        code = (code + bl_count[bits - 1]) << 1;
        next_code[bits] = code;
    }
    let mut codes = vec![0u32; lengths.len()];
    for (sym, &len) in lengths.iter().enumerate() {
        if len > 0 {
            codes[sym] = next_code[len as usize];
            next_code[len as usize] += 1;
        }
    }
    codes
}

/// Package-merge: optimal length-limited Huffman code lengths.
fn package_merge(freqs: &[u32], limit: usize) -> Vec<u8> {
    let mut leaves: Vec<(u64, Vec<usize>)> = freqs
        .iter()
        .enumerate()
        .filter(|&(_, &f)| f > 0)
        .map(|(i, &f)| (f as u64, vec![i]))
        .collect();
    leaves.sort_by_key(|(w, _)| *w);
    let n = leaves.len();
    let mut lengths = vec![0u8; freqs.len()];
    if n == 0 {
        return lengths;
    }
    if n == 1 {
        lengths[leaves[0].1[0]] = 1;
        return lengths;
    }

    let mut lists: Vec<Vec<(u64, Vec<usize>)>> = vec![leaves.clone()];
    for _ in 1..limit {
        let prev = lists.last().expect("non-empty");
        let mut packages: Vec<(u64, Vec<usize>)> = Vec::with_capacity(prev.len() / 2);
        let mut i = 0usize;
        while i + 1 < prev.len() {
            let mut set = prev[i].1.clone();
            set.extend(prev[i + 1].1.iter().copied());
            packages.push((prev[i].0 + prev[i + 1].0, set));
            i += 2;
        }
        let mut merged = Vec::with_capacity(packages.len() + leaves.len());
        let (mut a, mut b) = (0usize, 0usize);
        while a < leaves.len() || b < packages.len() {
            let take_leaf =
                b >= packages.len() || (a < leaves.len() && leaves[a].0 <= packages[b].0);
            if take_leaf {
                merged.push(leaves[a].clone());
                a += 1;
            } else {
                merged.push(packages[b].clone());
                b += 1;
            }
        }
        lists.push(merged);
    }

    let take = 2 * n - 2;
    for item in lists.last().expect("non-empty").iter().take(take) {
        for &sym in &item.1 {
            lengths[sym] += 1;
        }
    }
    lengths
}

/// A minimal VP8L decoder used to verify the encoder.
///
/// Only the subset this crate emits is supported: no transforms, no color
/// cache and no meta prefix codes. It exists so `cargo test` can prove that
/// the bitstream decodes back to the exact raster the encoder was given.
#[cfg(test)]
pub mod test_decode {
    use super::{canonical_codes, prefix_base};

    /// Reads LSB-first bits, matching the format's bit order.
    pub struct Reader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl<'a> Reader<'a> {
        /// Starts reading at byte `pos`.
        pub fn new(data: &'a [u8], pos: usize) -> Reader<'a> {
            Reader { data, pos, bit: 0 }
        }

        fn bits(&mut self, n: u32) -> u32 {
            let mut v = 0u32;
            for i in 0..n {
                let b = (self.data[self.pos] >> self.bit) & 1;
                v |= (b as u32) << i;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            v
        }

        fn symbol(&mut self, h: &DecodeTable) -> usize {
            if h.entries.len() == 1 {
                // A single-leaf tree consumes no bits at all.
                return h.entries[0].2;
            }
            let mut code = 0u32;
            for len in 1..=15u8 {
                code = (code << 1) | self.bits(1);
                if let Some(s) = h.lookup(len, code) {
                    return s;
                }
            }
            panic!("invalid Huffman code");
        }
    }

    /// Canonical code → symbol lookup.
    pub struct DecodeTable {
        entries: Vec<(u8, u32, usize)>,
    }

    impl DecodeTable {
        /// Builds a decode table from code lengths.
        pub fn new(lengths: &[u8]) -> DecodeTable {
            let codes = canonical_codes(lengths);
            let mut entries = Vec::new();
            for (sym, &len) in lengths.iter().enumerate() {
                if len > 0 {
                    entries.push((len, codes[sym], sym));
                }
            }
            DecodeTable { entries }
        }

        fn lookup(&self, len: u8, code: u32) -> Option<usize> {
            self.entries
                .iter()
                .find(|(l, c, _)| *l == len && *c == code)
                .map(|(_, _, s)| *s)
        }
    }

    fn read_tree(r: &mut Reader<'_>, alphabet: usize) -> DecodeTable {
        if r.bits(1) == 1 {
            // Simple code: one or two symbols.
            let n = r.bits(1) + 1;
            let eight = r.bits(1) == 1;
            let first = r.bits(if eight { 8 } else { 1 }) as usize;
            let mut lengths = vec![0u8; alphabet];
            lengths[first] = 1;
            if n == 2 {
                let second = r.bits(8) as usize;
                lengths[second] = 1;
            }
            return DecodeTable::new(&lengths);
        }

        const ORDER: [usize; 19] = [
            17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
        ];
        let num_cl = 4 + r.bits(4) as usize;
        let mut cl = [0u8; 19];
        for &sym in ORDER.iter().take(num_cl) {
            cl[sym] = r.bits(3) as u8;
        }
        let cl_table = DecodeTable::new(&cl);

        let max_symbol = if r.bits(1) == 0 {
            alphabet
        } else {
            let nbits = 2 + 2 * r.bits(3);
            2 + r.bits(nbits) as usize
        };

        let mut lengths = vec![0u8; alphabet];
        let mut i = 0usize;
        let mut prev = 8u8;
        while i < max_symbol {
            let sym = r.symbol(&cl_table);
            match sym {
                0..=15 => {
                    lengths[i] = sym as u8;
                    if sym != 0 {
                        prev = sym as u8;
                    }
                    i += 1;
                }
                16 => {
                    let rep = 3 + r.bits(2) as usize;
                    for _ in 0..rep {
                        if i < max_symbol {
                            lengths[i] = prev;
                            i += 1;
                        }
                    }
                }
                17 => {
                    let rep = 3 + r.bits(3) as usize;
                    i = (i + rep).min(max_symbol);
                }
                18 => {
                    let rep = 11 + r.bits(7) as usize;
                    i = (i + rep).min(max_symbol);
                }
                _ => panic!("bad code-length symbol"),
            }
        }
        DecodeTable::new(&lengths)
    }

    /// Decodes a complete WebP file into `(width, height, RGBA)`.
    pub fn decode(data: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&data[0..4], b"RIFF");
        assert_eq!(&data[8..12], b"WEBP");
        assert_eq!(&data[12..16], b"VP8L");
        let size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
        let payload = &data[20..20 + size];
        assert_eq!(payload[0], 0x2F);

        let mut r = Reader::new(payload, 1);
        let width = r.bits(14) + 1;
        let height = r.bits(14) + 1;
        let _alpha = r.bits(1);
        assert_eq!(r.bits(3), 0, "version");
        assert_eq!(r.bits(1), 0, "unexpected transform");
        assert_eq!(r.bits(1), 0, "unexpected color cache");
        assert_eq!(r.bits(1), 0, "unexpected meta prefix codes");

        let green = read_tree(&mut r, 280);
        let red = read_tree(&mut r, 256);
        let blue = read_tree(&mut r, 256);
        let alpha = read_tree(&mut r, 256);
        let distance = read_tree(&mut r, 40);

        let total = (width * height) as usize;
        let mut pixels: Vec<u32> = Vec::with_capacity(total);
        while pixels.len() < total {
            let sym = r.symbol(&green);
            if sym < 256 {
                let rr = r.symbol(&red) as u8;
                let bb = r.symbol(&blue) as u8;
                let aa = r.symbol(&alpha) as u8;
                pixels.push(
                    ((aa as u32) << 24) | ((rr as u32) << 16) | ((sym as u32) << 8) | bb as u32,
                );
            } else if sym < 280 {
                let (lbase, lbits) = prefix_base((sym - 256) as u8);
                let len = lbase + r.bits(lbits as u32);
                let dcode = r.symbol(&distance) as u8;
                let (dbase, dbits) = prefix_base(dcode);
                // Distance codes above 120 are scan-line distances offset by 120.
                let dist = dbase + r.bits(dbits as u32) - 120;
                let start = pixels.len() - dist as usize;
                for k in 0..len as usize {
                    let v = pixels[start + k];
                    pixels.push(v);
                }
            } else {
                panic!("color cache codes are not emitted by this encoder");
            }
        }

        let mut rgba = Vec::with_capacity(total * 4);
        for p in pixels {
            rgba.push((p >> 16) as u8);
            rgba.push((p >> 8) as u8);
            rgba.push(p as u8);
            rgba.push((p >> 24) as u8);
        }
        (width, height, rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_riff(data: &[u8]) -> (u32, u32, &[u8]) {
        assert_eq!(&data[0..4], b"RIFF");
        let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
        assert_eq!(riff_size + 8, data.len(), "RIFF size covers the file");
        assert_eq!(&data[8..12], b"WEBP");
        assert_eq!(&data[12..16], b"VP8L");
        let size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
        let payload = &data[20..20 + size];
        (riff_size as u32, size as u32, payload)
    }

    /// Reads the fixed header of a VP8L payload.
    fn header(payload: &[u8]) -> (u32, u32, bool) {
        assert_eq!(payload[0], 0x2F, "VP8L signature");
        // The bitstream starts right after the 1-byte signature.
        let mut bits = BitReader {
            data: payload,
            pos: 1,
            bit: 0,
        };
        let w = bits.read(14) + 1;
        let h = bits.read(14) + 1;
        let alpha = bits.read(1) == 1;
        let version = bits.read(3);
        assert_eq!(version, 0, "version must be 0");
        (w, h, alpha)
    }

    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl BitReader<'_> {
        fn read(&mut self, n: u32) -> u32 {
            let mut v = 0u32;
            for i in 0..n {
                let b = (self.data[self.pos] >> self.bit) & 1;
                v |= (b as u32) << i;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            v
        }
    }

    fn sample(width: u32, height: u32) -> Vec<u8> {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.extend_from_slice(&[
                    (x * 5 % 256) as u8,
                    (y * 3 % 256) as u8,
                    ((x * y) % 256) as u8,
                    if (x + y) % 7 == 0 { 100 } else { 255 },
                ]);
            }
        }
        data
    }

    #[test]
    fn container_and_header_are_valid() {
        let png = encode(17, 9, &sample(17, 9));
        let (_, size, payload) = parse_riff(&png);
        assert!(size as usize + 20 <= png.len());
        let (w, h, alpha) = header(payload);
        assert_eq!((w, h), (17, 9));
        assert!(alpha, "sample has transparent pixels");
    }

    #[test]
    fn opaque_images_clear_the_alpha_flag() {
        let pixels = vec![255u8; 4 * 4 * 4];
        let data = encode(4, 4, &pixels);
        let (_, _, payload) = parse_riff(&data);
        let (_, _, alpha) = header(payload);
        assert!(!alpha);
    }

    #[test]
    fn prefix_coding_matches_the_spec_ranges() {
        let cases = [
            (1u32, 0u8, 0u8),
            (4, 3, 0),
            (5, 4, 1),
            (6, 4, 1),
            (7, 5, 1),
            (8, 5, 1),
            (9, 6, 2),
            (12, 6, 2),
            (13, 7, 2),
            (17, 8, 3),
            (32, 9, 3),
            (33, 10, 4),
            (4096, 23, 10),
        ];
        for (value, code, extra) in cases {
            let (c, e, _) = prefix_encode(value);
            assert_eq!((c, e), (code, extra), "value {value}");
        }
    }

    #[test]
    fn prefix_coding_is_self_consistent() {
        for value in [
            1u32, 2, 3, 4, 5, 6, 7, 8, 9, 12, 13, 16, 17, 100, 4095, 4096,
        ] {
            let (code, bits, extra) = prefix_encode(value);
            let e = if code < 4 { 0 } else { (code / 2 - 1) as u32 };
            assert_eq!(e, bits as u32, "value {value}");
            let base = if code < 4 {
                code as u32 + 1
            } else {
                (1u32 << (e + 1)) + 1 + ((code as u32 & 1) << e)
            };
            assert_eq!(base + extra, value, "value {value} code {code}");
        }
    }

    #[test]
    fn canonical_codes_are_prefix_free() {
        let lengths = vec![1u8, 2, 3, 3];
        let codes = canonical_codes(&lengths);
        assert_eq!(codes, vec![0, 2, 6, 7]);
        // Prefix-freeness: a shorter code must not be a prefix of a longer one.
        for (i, &a) in codes.iter().enumerate() {
            for (j, &b) in codes.iter().enumerate() {
                if i == j {
                    continue;
                }
                let (la, lb) = (lengths[i], lengths[j]);
                if la < lb {
                    assert_ne!(a, b >> (lb - la), "code {a} is a prefix of {b}");
                } else if la > lb {
                    assert_ne!(a >> (la - lb), b, "code {b} is a prefix of {a}");
                } else {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn package_merge_produces_a_full_tree() {
        let freqs = vec![5u32, 3, 2, 1, 1, 1, 1];
        let lengths = package_merge(&freqs, 15);
        assert!(lengths.iter().all(|&l| l > 0 && l <= 15));
        let kraft: f64 = lengths.iter().map(|&l| 2f64.powi(-(l as i32))).sum();
        assert!((kraft - 1.0).abs() < 1e-9, "Kraft sum {kraft}");
    }

    #[test]
    fn package_merge_respects_the_length_limit() {
        // Fibonacci-like weights are the worst case for deep trees.
        let mut freqs = vec![0u32; 40];
        let (mut a, mut b) = (1u32, 1u32);
        for slot in freqs.iter_mut().take(20) {
            *slot = a;
            let next = a.saturating_add(b);
            a = b;
            b = next;
        }
        let lengths = package_merge(&freqs, 8);
        assert!(lengths.iter().all(|&l| l <= 8));
        let kraft: f64 = lengths
            .iter()
            .filter(|&&l| l > 0)
            .map(|&l| 2f64.powi(-(l as i32)))
            .sum();
        assert!((kraft - 1.0).abs() < 1e-9, "Kraft sum {kraft}");
    }

    #[test]
    fn single_symbol_alphabet_is_valid() {
        let freqs = vec![0u32, 0, 100, 0];
        let h = Huffman::build(&freqs);
        assert_eq!(h.symbol_count(), 1);
        assert_eq!(h.lengths[2], 1);
        // A single-leaf code consumes no bits.
        let mut w = BitWriter::new();
        h.write_symbol(&mut w, 2);
        assert!(w.finish().is_empty());
    }

    #[test]
    fn tokens_reconstruct_the_pixels() {
        let pixels: Vec<u32> = (0..500)
            .map(|i| {
                if i < 250 {
                    0xFF00_00FF
                } else {
                    (i as u32).wrapping_mul(2654435761)
                }
            })
            .collect();
        let tokens = tokenize(&pixels);
        let mut rebuilt = Vec::with_capacity(pixels.len());
        for token in tokens {
            match token {
                Token::Literal { a, r, g, b } => rebuilt
                    .push(((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32),
                Token::Copy { len, dist } => {
                    let start = rebuilt.len() - dist as usize;
                    for k in 0..len as usize {
                        let v = rebuilt[start + k];
                        rebuilt.push(v);
                    }
                }
            }
        }
        assert_eq!(rebuilt, pixels);
    }

    #[test]
    fn flat_images_compress_to_far_less_than_raw() {
        let (w, h) = (120u32, 120u32);
        let pixels = vec![255u8; (w * h * 4) as usize];
        let data = encode(w, h, &pixels);
        assert!(
            data.len() < (w * h) as usize,
            "flat image took {} bytes for {} pixels",
            data.len(),
            w * h
        );
    }

    #[test]
    fn odd_payloads_are_padded() {
        // Whatever the content, the RIFF size must still cover the file.
        for n in 1..40u32 {
            let data = encode(n, 1, &sample(n, 1));
            parse_riff(&data);
        }
    }

    #[test]
    fn own_decoder_reproduces_sample_pixels() {
        for (w, h) in [(1u32, 1u32), (2, 3), (17, 9), (60, 40)] {
            let src = sample(w, h);
            let (dw, dh, decoded) = test_decode::decode(&encode(w, h, &src));
            assert_eq!((dw, dh), (w, h), "dimensions for {w}x{h}");
            assert_eq!(decoded, src, "pixels for {w}x{h}");
        }
    }

    #[test]
    fn own_decoder_reproduces_flat_and_noisy_images() {
        let flat = vec![0x80u8; 40 * 30 * 4];
        let (_, _, decoded) = test_decode::decode(&encode(40, 30, &flat));
        assert_eq!(decoded, flat);

        let mut state = 0x1234_5678u32;
        let noise: Vec<u8> = (0..32 * 32 * 4)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                (state >> 24) as u8
            })
            .collect();
        let (_, _, decoded) = test_decode::decode(&encode(32, 32, &noise));
        assert_eq!(decoded, noise);
    }

    #[test]
    fn own_decoder_reproduces_a_real_rendered_page() {
        let Some(fonts) = crate::font::FontSet::load(&crate::font::discover()) else {
            eprintln!("skipping: no system fonts");
            return;
        };
        let scene = crate::to_scene(include_str!("../../demo.md"), &crate::Theme::light());
        let bmp = crate::raster::Rasterizer::new(1.0).rasterize(&scene, &fonts);
        let data = encode(bmp.width, bmp.height, &bmp.data);
        let (w, h, decoded) = test_decode::decode(&data);
        assert_eq!((w, h), (bmp.width, bmp.height));
        let first_diff = decoded
            .iter()
            .zip(bmp.data.iter())
            .position(|(a, b)| a != b);
        assert_eq!(first_diff, None, "encoded stream does not round-trip");
    }

    #[test]
    fn distances_never_exceed_the_encodable_range() {
        // A long image whose repeated pixels sit beyond the 1 MiB distance
        // limit: such matches must be rejected, not truncated.
        let n = 1_100_000usize;
        let mut pixels: Vec<u32> = (0..n)
            .map(|i| (i as u32).wrapping_mul(2654435761))
            .collect();
        for i in (0..n).step_by(97) {
            pixels[i] = 0x1234_5678;
        }
        for token in tokenize(&pixels) {
            if let Token::Copy { dist, .. } = token {
                assert!(
                    dist <= 1_048_576 - 120,
                    "distance {dist} cannot be expressed by a WebP code"
                );
            }
        }
    }
}
