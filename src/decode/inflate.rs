//! DEFLATE (RFC 1951) and zlib (RFC 1950) decompression.
//!
//! This is the decompression half of the crate's own PNG support: the
//! encoder in [`crate::encode::deflate`] produces streams, this module
//! reads them (and every real-world PNG, which may use dynamic Huffman
//! or stored blocks).
//!
//! Everything is bounds-checked and panic-free by construction: a
//! truncated, corrupt or hostile stream decompresses to `None`, never to
//! a crash. An output cap guards against zip bombs.

/// Hard cap on decompressed size, in bytes. 100 MP of RGBA is 400 MB —
/// far above any sane document image, and the ceiling a zip bomb hits.
pub(crate) const MAX_OUTPUT: usize = 400_000_000;

/// Inflates a zlib stream (RFC 1950 wrapper + RFC 1951 payload).
///
/// The Adler-32 checksum is verified; a mismatch means the stream is
/// corrupt and `None` is returned.
pub(crate) fn inflate_zlib(data: &[u8]) -> Option<Vec<u8>> {
    // Wrapper: CMF/FLG. We require plain deflate (method 8), no
    // preset dictionary, at most a 32 KiB window.
    let cmf = *data.first()?;
    let flg = *data.get(1)?;
    if cmf & 0x0f != 0x08 || cmf >> 4 > 7 || flg & 0x20 != 0 {
        return None;
    }
    // FDICT uses bits of the header that must make (CMF*256+FLG) a
    // multiple of 31; verify as a corruption check.
    if (u16::from(cmf) * 256 + u16::from(flg)) % 31 != 0 {
        return None;
    }
    let body = &data[2..];
    let (out, consumed) = inflate(body, MAX_OUTPUT)?;
    // Trailing Adler-32, big-endian.
    let tail = body.get(consumed..consumed + 4)?;
    let expect = u32::from_be_bytes(tail.try_into().ok()?);
    if adler32(&out) != expect {
        return None;
    }
    Some(out)
}

/// Inflates a bare DEFLATE stream with an output cap.
pub(crate) fn inflate(data: &[u8], max_output: usize) -> Option<(Vec<u8>, usize)> {
    let mut br = BitReader::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let final_block = br.bits(1)? == 1;
        match br.bits(2)? {
            0 => inflate_stored(&mut br, &mut out, max_output)?,
            1 => {
                let (lit, dist) = fixed_tables()?;
                inflate_block(&mut br, &mut out, &lit, &dist, max_output)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut br)?;
                inflate_block(&mut br, &mut out, &lit, &dist, max_output)?;
            }
            _ => return None,
        }
        if final_block {
            return Some((out, br.consumed()));
        }
    }
}

fn inflate_stored(br: &mut BitReader, out: &mut Vec<u8>, max_output: usize) -> Option<()> {
    br.align();
    let len = br.bits(16)? as usize;
    let nlen = br.bits(16)? as usize;
    if len != (!nlen & 0xffff) {
        return None;
    }
    for _ in 0..len {
        if out.len() >= max_output {
            return None;
        }
        out.push(br.byte()?);
    }
    Some(())
}

fn inflate_block(
    br: &mut BitReader,
    out: &mut Vec<u8>,
    lit: &Huffman,
    dist: &Huffman,
    max_output: usize,
) -> Option<()> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => {
                if out.len() >= max_output {
                    return None;
                }
                out.push(sym as u8);
            }
            256 => return Some(()),
            257..=285 => {
                let length = usize::from(LENGTH_BASE[(sym - 257) as usize])
                    + br.bits(u32::from(LENGTH_EXTRA[(sym - 257) as usize]))? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym >= 30 {
                    return None;
                }
                let distance = usize::from(DIST_BASE[dsym])
                    + br.bits(u32::from(DIST_EXTRA[dsym]))? as usize;
                if distance == 0 || distance > out.len() {
                    return None;
                }
                // Overlapping copies must proceed byte by byte.
                for _ in 0..length {
                    if out.len() >= max_output {
                        return None;
                    }
                    let byte = out[out.len() - distance];
                    out.push(byte);
                }
            }
            _ => return None,
        }
    }
}

fn fixed_tables() -> Option<(Huffman, Huffman)> {
    let mut lengths = [0u8; 288];
    for (i, len) in lengths.iter_mut().enumerate() {
        *len = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let lit = Huffman::new(&lengths)?;
    let dist = Huffman::new(&[5u8; 30])?;
    Some((lit, dist))
}

fn dynamic_tables(br: &mut BitReader) -> Option<(Huffman, Huffman)> {
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    let mut cl_lengths = [0u8; 19];
    for slot in ORDER.iter().take(hclen) {
        cl_lengths[*slot] = br.bits(3)? as u8;
    }
    let cl = Huffman::new(&cl_lengths)?;

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0usize;
    while i < lengths.len() {
        let sym = cl.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return None;
                }
                let prev = lengths[i - 1];
                let repeat = 3 + br.bits(2)? as usize;
                if i + repeat > lengths.len() {
                    return None;
                }
                for _ in 0..repeat {
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => {
                let repeat = 3 + br.bits(3)? as usize;
                if i + repeat > lengths.len() {
                    return None;
                }
                i += repeat;
            }
            18 => {
                let repeat = 11 + br.bits(7)? as usize;
                if i + repeat > lengths.len() {
                    return None;
                }
                i += repeat;
            }
            _ => return None,
        }
    }
    if lengths[256] == 0 {
        // No end-of-block symbol: the block cannot terminate.
        return None;
    }
    let lit = Huffman::new(&lengths[..hlit])?;
    let dist = Huffman::new(&lengths[hlit..])?;
    Some((lit, dist))
}

/// LSB-first bit reader; every accessor is bounds-checked.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> BitReader<'a> {
        BitReader { data, pos: 0, bit: 0 }
    }

    fn bits(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for i in 0..n {
            let byte = *self.data.get(self.pos)?;
            let b = (byte >> self.bit) & 1;
            value |= u32::from(b) << i;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Some(value)
    }

    fn align(&mut self) {
        if self.bit > 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }

    fn byte(&mut self) -> Option<u8> {
        self.align();
        let byte = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(byte)
    }

    /// Bytes consumed so far, counting a partially-read byte (the
    /// deflate stream ends mid-byte and the zlib checksum follows on
    /// the next byte boundary).
    fn consumed(&self) -> usize {
        self.pos + usize::from(self.bit > 0)
    }
}

/// A canonical Huffman decoding table (code lengths in, symbols out).
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    /// Builds the canonical table from code lengths. Returns `None` for
    /// over-subscribed length sets (an incomplete set is tolerated, the
    /// way real encoders produce single-symbol distance trees).
    fn new(lengths: &[u8]) -> Option<Huffman> {
        let mut counts = [0u16; 16];
        for &len in lengths {
            if len as usize >= 16 {
                return None;
            }
            counts[len as usize] += 1;
        }
        counts[0] = 0;
        // Over-subscription check.
        let mut left = 1i32;
        for &count in counts.iter().take(15).skip(1) {
            left <<= 1;
            left -= i32::from(count);
            if left < 0 {
                return None;
            }
        }
        let mut offsets = [0u16; 16];
        for len in 1..15 {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (symbol, &len) in lengths.iter().enumerate() {
            if len != 0 {
                symbols[offsets[len as usize] as usize] = symbol as u16;
                offsets[len as usize] += 1;
            }
        }
        Some(Huffman { counts, symbols })
    }

    /// Decodes one symbol, one bit at a time (puff-style canonical walk).
    fn decode(&self, br: &mut BitReader) -> Option<u16> {
        let mut code = 0i32;
        let mut first = 0i32;
        let mut index = 0i32;
        for len in 1..=15 {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return self.symbols.get((index + (code - first)) as usize).copied();
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        None
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

/// Adler-32 (RFC 1950), the zlib wrapper checksum.
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The crate's own deflate compressor produces fixed-Huffman streams;
    /// round-tripping through them is the strongest fixture we have.
    fn deflate_fixed(data: &[u8]) -> Vec<u8> {
        crate::encode::deflate::deflate(data)
    }

    #[test]
    fn roundtrips_own_deflate_output() {
        let cases: [Vec<u8>; 6] = [
            Vec::new(),
            b"a".to_vec(),
            b"hello hello hello hello".to_vec(),
            (0..=255u8).collect(),
            vec![7u8; 10_000],
            b"abcabcabcabcabcabcabcabcabcabcXYZ".to_vec(),
        ];
        for case in cases {
            let compressed = deflate_fixed(&case);
            let (out, consumed) = inflate(&compressed, MAX_OUTPUT).expect("inflates");
            assert_eq!(consumed, compressed.len(), "whole stream consumed");
            assert_eq!(out, case);
        }
    }

    #[test]
    fn stored_blocks_roundtrip() {
        // Hand-built stored block: header 0x01, LEN, ~LEN, payload.
        let payload = b"stored payload";
        let mut stream = vec![0x01, 14, 0, (!14 & 0xff) as u8, ((!14) >> 8) as u8];
        stream.extend_from_slice(payload);
        assert_eq!(inflate(&stream, MAX_OUTPUT).unwrap().0, payload);
    }

    #[test]
    fn output_cap_stops_zip_bombs() {
        // 100 stored zeros via a fixed-Huffman run would be longer; a
        // stored block of 40 zeros with a 10-byte cap must fail.
        let mut stream = vec![0x01, 40, 0, (!40 & 0xff) as u8, ((!40) >> 8) as u8];
        stream.extend_from_slice(&[0u8; 40]);
        assert!(inflate(&stream, 10).is_none());
    }

    #[test]
    fn corrupt_streams_return_none() {
        assert!(inflate(&[], MAX_OUTPUT).is_none());
        assert!(inflate(&[0x03], MAX_OUTPUT).is_none()); // btype 3
        let mut stream = vec![0x01, 14, 0, 0xff, 0xff]; // broken ~LEN
        stream.extend_from_slice(b"stored payload");
        assert!(inflate(&stream, MAX_OUTPUT).is_none());
    }

    #[test]
    fn zlib_wrapper_is_validated() {
        let good = crate::encode::deflate::zlib(b"data");
        assert_eq!(inflate_zlib(&good).unwrap(), b"data");

        // Corrupt checksum.
        let mut bad = good.clone();
        let last = bad.len() - 1;
        bad[last] ^= 0xff;
        assert!(inflate_zlib(&bad).is_none());
        // Preset dictionary flag set.
        assert!(inflate_zlib(&[0x78, 0x21]).is_none());
    }
}

