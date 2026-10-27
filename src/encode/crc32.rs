//! CRC-32 (IEEE), the checksum PNG chunks and ZIP entries share.

/// Incremental CRC-32 (IEEE) checksum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl Default for Crc32 {
    fn default() -> Crc32 {
        Crc32::new()
    }
}

/// CRC-32 (IEEE) of a byte slice.
///
/// ```
/// assert_eq!(markpop::encode::crc32::crc32(b"123456789"), 0xCBF4_3926);
/// ```
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        // The check value every CRC-32 implementation must produce.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn empty_input_has_the_seed_value() {
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn incremental_updates_match_the_one_shot_value() {
        let mut c = Crc32::new();
        c.update(b"1234");
        c.update(b"56789");
        assert_eq!(c.finish(), 0xCBF4_3926);
    }

    #[test]
    fn a_single_bit_change_changes_the_checksum() {
        assert_ne!(crc32(b"hello"), crc32(b"hellp"));
    }
}
