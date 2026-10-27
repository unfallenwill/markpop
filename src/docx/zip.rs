//! A ZIP container writer — the envelope a `.docx` ships in.
//!
//! Only the two methods every reader supports are used: stored (0) and
//! deflate (8). Entries carry a fixed DOS timestamp, so the same input
//! always produces the same bytes.

use crate::decode::inflate;
use crate::encode::crc32::crc32;
use crate::encode::deflate::deflate;
use crate::options::RenderError;

/// Local file header signature.
const LOCAL_SIG: u32 = 0x0403_4b50;
/// Central directory header signature.
const CENTRAL_SIG: u32 = 0x0201_4b50;
/// End of central directory signature.
const EOCD_SIG: u32 = 0x0605_4b50;
/// DOS date for 1980-01-01 — the earliest a ZIP can express.
const DOS_DATE: u16 = 0x0021;
/// DOS time of midnight.
const DOS_TIME: u16 = 0;
/// The largest size a ZIP entry can describe without ZIP64.
const MAX_ENTRY: usize = u32::MAX as usize;
/// Cap on an inflated entry, guarding against a hostile archive.
const MAX_INFLATE: usize = 64 << 20;

/// One file in the archive.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The path inside the archive, using forward slashes.
    pub name: String,
    /// The uncompressed contents.
    pub data: Vec<u8>,
}

/// Builds a ZIP archive.
#[derive(Debug, Default)]
pub struct ZipWriter {
    entries: Vec<Entry>,
}

impl ZipWriter {
    /// Creates an empty archive.
    pub fn new() -> ZipWriter {
        ZipWriter::default()
    }

    /// Adds a file.
    pub fn add(&mut self, name: &str, data: Vec<u8>) {
        self.entries.push(Entry {
            name: name.to_string(),
            data,
        });
    }

    /// Writes the archive.
    ///
    /// Fails when a single entry is too large for a ZIP without ZIP64.
    pub fn finish(self) -> Result<Vec<u8>, RenderError> {
        if self.entries.iter().any(|e| e.data.len() > MAX_ENTRY) {
            return Err(RenderError::PackageTooLarge);
        }
        let mut out = Vec::new();
        let mut central = Vec::new();
        for entry in &self.entries {
            let offset = u32::try_from(out.len()).map_err(|_| RenderError::PackageTooLarge)?;
            let crc = crc32(&entry.data);
            let compressed = deflate(&entry.data);
            // Storing beats compressing when compression does not pay.
            let (method, payload) = if compressed.len() < entry.data.len() {
                (8u16, compressed)
            } else {
                (0u16, entry.data.clone())
            };
            let size = u32::try_from(payload.len()).map_err(|_| RenderError::PackageTooLarge)?;
            let raw = u32::try_from(entry.data.len()).map_err(|_| RenderError::PackageTooLarge)?;
            let name = entry.name.as_bytes();
            let name_len = u16::try_from(name.len()).map_err(|_| RenderError::PackageTooLarge)?;

            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&DOS_TIME.to_le_bytes());
            out.extend_from_slice(&DOS_DATE.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
            out.extend_from_slice(&raw.to_le_bytes());
            out.extend_from_slice(&name_len.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra length
            out.extend_from_slice(name);
            out.extend_from_slice(&payload);

            central.extend_from_slice(&CENTRAL_SIG.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes()); // version made by
            central.extend_from_slice(&20u16.to_le_bytes()); // version needed
            central.extend_from_slice(&0u16.to_le_bytes()); // flags
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&DOS_TIME.to_le_bytes());
            central.extend_from_slice(&DOS_DATE.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&size.to_le_bytes());
            central.extend_from_slice(&raw.to_le_bytes());
            central.extend_from_slice(&name_len.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra length
            central.extend_from_slice(&0u16.to_le_bytes()); // comment length
            central.extend_from_slice(&0u16.to_le_bytes()); // disk number
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
        }
        let cd_offset = u32::try_from(out.len()).map_err(|_| RenderError::PackageTooLarge)?;
        let cd_size = u32::try_from(central.len()).map_err(|_| RenderError::PackageTooLarge)?;
        let count = u16::try_from(self.entries.len()).map_err(|_| RenderError::PackageTooLarge)?;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        Ok(out)
    }
}

/// Reads an archive back into its entries.
///
/// The reader walks the central directory, so it accepts what the writer
/// produces and anything a standard reader would accept. It returns
/// `None` for an archive it cannot make sense of — never a panic.
///
/// ```
/// use markpop::docx::zip::{ZipWriter, read_archive};
///
/// let mut writer = ZipWriter::new();
/// writer.add("a.txt", b"hello".to_vec());
/// let bytes = writer.finish().unwrap();
/// let entries = read_archive(&bytes).unwrap();
/// assert_eq!(entries, vec![("a.txt".to_string(), b"hello".to_vec())]);
/// ```
pub fn read_archive(data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let eocd = find_eocd(data)?;
    let count = u16_at(data, eocd + 10)? as usize;
    let mut offset = u32_at(data, eocd + 16)? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(data, offset)? != CENTRAL_SIG {
            return None;
        }
        let method = u16_at(data, offset + 10)?;
        let compressed = u32_at(data, offset + 20)? as usize;
        let name_len = u16_at(data, offset + 28)? as usize;
        let extra_len = u16_at(data, offset + 30)? as usize;
        let comment_len = u16_at(data, offset + 32)? as usize;
        let local = u32_at(data, offset + 42)? as usize;
        let name = String::from_utf8(data.get(offset + 46..offset + 46 + name_len)?.to_vec()).ok()?;
        // The local header repeats the name and may carry its own extra
        // fields, so the payload starts after those, not after the name.
        let local_name = u16_at(data, local + 26)? as usize;
        let local_extra = u16_at(data, local + 28)? as usize;
        let start = local + 30 + local_name + local_extra;
        let payload = data.get(start..start + compressed)?;
        let content = match method {
            0 => payload.to_vec(),
            8 => inflate(payload, MAX_INFLATE)?.0,
            _ => return None,
        };
        out.push((name, content));
        offset += 46 + name_len + extra_len + comment_len;
    }
    Some(out)
}

/// Finds the end-of-central-directory record.
fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    (0..=data.len() - 22).rev().find(|&i| u32_at(data, i) == Some(EOCD_SIG))
}

/// Reads a little-endian `u16`.
fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// Reads a little-endian `u32`.
fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_starts_with_the_local_header_signature() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish().unwrap();
        assert_eq!(&out[..4], &[0x50, 0x4b, 0x03, 0x04]);
    }

    #[test]
    fn archive_ends_with_the_end_of_central_directory() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish().unwrap();
        assert_eq!(&out[out.len() - 22..out.len() - 18], &[0x50, 0x4b, 0x05, 0x06]);
    }

    #[test]
    fn entry_names_and_sizes_are_recorded() {
        let mut w = ZipWriter::new();
        w.add("word/document.xml", b"<w:document/>".to_vec());
        let out = w.finish().unwrap();
        // The name appears in both the local header and the central directory.
        let name = b"word/document.xml";
        assert_eq!(out.windows(name.len()).filter(|s| *s == name).count(), 2);
    }

    #[test]
    fn entries_survive_a_round_trip() {
        let mut w = ZipWriter::new();
        let payload = b"abcabcabcabcabcabc".repeat(20);
        w.add("a.txt", payload.clone());
        w.add("b.txt", b"short".to_vec());
        let out = w.finish().unwrap();
        let entries = read_archive(&out).expect("readable archive");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("a.txt".to_string(), payload));
        assert_eq!(entries[1], ("b.txt".to_string(), b"short".to_vec()));
    }

    #[test]
    fn binary_payloads_survive_a_round_trip() {
        let mut w = ZipWriter::new();
        let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        w.add("bin", payload.clone());
        let out = w.finish().unwrap();
        let entries = read_archive(&out).unwrap();
        assert_eq!(entries[0].1, payload);
    }

    #[test]
    fn an_empty_archive_is_still_valid() {
        let out = ZipWriter::new().finish().unwrap();
        assert_eq!(out.len(), 22);
        assert!(read_archive(&out).unwrap().is_empty());
    }

    #[test]
    fn an_empty_file_is_stored() {
        let mut w = ZipWriter::new();
        w.add("empty", Vec::new());
        let out = w.finish().unwrap();
        let entries = read_archive(&out).unwrap();
        assert_eq!(entries[0].1, Vec::<u8>::new());
    }

    #[test]
    fn the_crc_of_an_entry_is_correct() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish().unwrap();
        // The CRC sits at offset 14 of the local header.
        let crc = u32::from_le_bytes(out[14..18].try_into().unwrap());
        assert_eq!(crc, crc32(b"hello"));
    }

    #[test]
    fn timestamps_are_fixed() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"x".to_vec());
        let out = w.finish().unwrap();
        // The DOS time and date sit at offsets 10 and 12.
        assert_eq!(u16::from_le_bytes(out[10..12].try_into().unwrap()), DOS_TIME);
        assert_eq!(u16::from_le_bytes(out[12..14].try_into().unwrap()), DOS_DATE);
    }

    #[test]
    fn the_same_input_produces_the_same_bytes() {
        let build = || {
            let mut w = ZipWriter::new();
            w.add("a.txt", b"hello".to_vec());
            w.finish().unwrap()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn a_truncated_archive_reads_as_nothing() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish().unwrap();
        assert!(read_archive(&out[..10]).is_none());
    }

    #[test]
    fn garbage_reads_as_nothing() {
        assert!(read_archive(b"not a zip at all").is_none());
        assert!(read_archive(&[0u8; 100]).is_none());
    }
}
