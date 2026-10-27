# DOCX Output Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give MarkPop a fourth output format — `markpop doc.md --format docx -o doc.docx` writes a real Microsoft Word document (OOXML in a ZIP) with native styles, real tables, real list numbering, embedded images and a Word TOC field.

**Architecture:** A new `src/docx/` subsystem reads the Markdown AST (`src/ast.rs`) and writes OOXML parts that a ZIP writer packages. It never touches the laid-out `Scene`, because a Word document is structure, not geometry. Seven small units: `xml.rs` (XML writing), `zip.rs` (ZIP container), `model.rs` (typed Word model), `content.rs` (AST → model), `write.rs` (model → OOXML parts), `media.rs` (image embedding), `mod.rs` (assembly).

**Tech Stack:** Rust 2024 edition, no dependencies (`Cargo.toml` stays empty). Reuses `encode::deflate::deflate` (raw DEFLATE = ZIP method 8), `encode::png::encode` (PNG writer), `decode::decode` (PNG/BMP decoder), `html::parse_html` (raw HTML tree).

**Spec:** `docs/superpowers/specs/2026-10-27-docx-output-design.md`

## Global Constraints

- **No new dependencies.** `Cargo.toml`'s `[dependencies]` section stays exactly as it is (empty).
- **Determinism.** The same Markdown and options must always produce byte-identical output. No timestamps, no random IDs, no locale-dependent formatting. The ZIP DOS date is fixed at 1980-01-01 (`date = 0x0021`, `time = 0`).
- **Word-native styles only.** Built-in style IDs with Word's default look (Calibri body, `#2F5496` headings). MarkPop's `Theme` is never consulted for styling.
- **Degrade, never fail.** An unreadable or undecodable image becomes a paragraph with the alt text. Markdown parsing stays infallible.
- **No whitespace inside `w:p`.** OOXML is whitespace-sensitive between runs; the XML writer must never pretty-print.
- **Every part is typed by `[Content_Types].xml`**, every `r:id` resolves, every `w:pStyle` resolves, every `w:numId` resolves. These are the integrity rules the tests enforce.
- Rust edition 2024, rust-version 1.88, `cargo clippy --all-targets` must stay clean.
- Code style follows the crate: `//!` module docs, `///` item docs on public items, doc-comment examples that compile as doctests.

---

### Task 1: Share CRC-32 between the PNG and ZIP writers

**Files:**
- Create: `src/encode/crc32.rs`
- Modify: `src/encode/mod.rs`, `src/encode/png.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `encode::crc32::Crc32` (struct with `new()`, `update(&[u8])`, `finish() -> u32`, `Default`), and `encode::crc32::crc32(data: &[u8]) -> u32` (one-shot). Task 3 uses both.

- [ ] **Step 1: Write the failing test**

In `src/encode/crc32.rs`:

```rust
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
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib encode::crc32`
Expected: FAIL — `mod crc32` does not exist yet.

- [ ] **Step 3: Write the implementation**

`src/encode/crc32.rs`:

```rust
//! CRC-32 (IEEE), the checksum PNG chunks and ZIP entries share.

/// Incremental CRC-32 (IEEE) checksum.
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
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *entry = c;
    }
    table
});
```

- [ ] **Step 4: Point PNG at the shared implementation**

In `src/encode/mod.rs` add `pub mod crc32;` (keep the module list alphabetical: `crc32`, `deflate`, `png`, `webp`).

In `src/encode/png.rs` delete the `Crc32` struct, its `Default` impl and the local `CRC_TABLE`, and import instead:

```rust
use super::crc32::Crc32;
```

Every existing `Crc32::new()` call site in `png.rs` keeps working unchanged.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --lib encode`
Expected: PASS — the new CRC tests and the existing PNG tests both pass.

- [ ] **Step 6: Commit**

```bash
git add src/encode/crc32.rs src/encode/mod.rs src/encode/png.rs
git commit -m "refactor(encode): share the CRC-32 checksum between PNG and ZIP"
```

---

### Task 2: A minimal XML writer

**Files:**
- Create: `src/docx/xml.rs`
- Create: `src/docx/mod.rs` (stub for now, filled in Task 8)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `docx::xml::escape_text(&str) -> String`
  - `docx::xml::escape_attr(&str) -> String`
  - `docx::xml::Element` with `new(tag: &str)`, `attr(self, name: &str, value: impl Into<String>) -> Element`, `child(self, child: Element) -> Element`, `text(self, text: impl Into<String>) -> Element`, `is_empty(&self) -> bool`, `write(&self, out: &mut String)`, `to_string(&self) -> String`.

- [ ] **Step 1: Write the failing test**

In `src/docx/xml.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_the_five_entities() {
        assert_eq!(escape_text("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn attributes_also_escape_quotes() {
        assert_eq!(escape_attr("say \"hi\" & 'bye'"), "say &quot;hi&quot; &amp; &apos;bye&apos;");
    }

    #[test]
    fn control_characters_are_dropped() {
        // XML 1.0 forbids these; Word rejects documents containing them.
        assert_eq!(escape_text("a\u{0}b\u{8}c"), "abc");
        assert_eq!(escape_text("keep\ttab\nand\rreturn"), "keep\ttab\nand\rreturn");
    }

    #[test]
    fn elements_render_with_attributes_and_children() {
        let e = Element::new("w:p")
            .attr("xml:space", "preserve")
            .child(Element::new("w:t").text("hi"));
        assert_eq!(e.to_string(), r#"<w:p xml:space="preserve"><w:t>hi</w:t></w:p>"#);
    }

    #[test]
    fn empty_elements_are_self_closing() {
        assert_eq!(Element::new("w:b").to_string(), "<w:b/>");
    }

    #[test]
    fn text_only_elements_keep_their_text() {
        assert_eq!(Element::new("w:t").text("a & b").to_string(), "<w:t>a &amp; b</w:t>");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::xml`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/xml.rs`:

```rust
//! A minimal XML writer for OOXML parts.
//!
//! OOXML is whitespace-sensitive: a space between two runs inside a
//! paragraph is real text in Word. The writer therefore never indents and
//! never adds whitespace of its own.

/// Escapes text for an XML text node.
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {} // not allowed in XML 1.0
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for an XML attribute value.
pub fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// An XML element with attributes, child elements and text.
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
    text: Option<String>,
}

impl Element {
    /// Starts an element with the given tag name.
    pub fn new(tag: &str) -> Element {
        Element { tag: tag.to_string(), attrs: Vec::new(), children: Vec::new(), text: None }
    }

    /// Adds an attribute.
    pub fn attr(mut self, name: &str, value: impl Into<String>) -> Element {
        self.attrs.push((name.to_string(), value.into()));
        self
    }

    /// Adds a child element.
    pub fn child(mut self, child: Element) -> Element {
        self.children.push(child);
        self
    }

    /// Sets the element's text content.
    pub fn text(mut self, text: impl Into<String>) -> Element {
        self.text = Some(text.into());
        self
    }

    /// Whether the element has neither children nor text.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty() && self.text.is_none()
    }

    /// Appends the element's XML to `out`.
    pub fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.tag);
        for (name, value) in &self.attrs {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(&escape_attr(value));
            out.push('"');
        }
        if self.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        if let Some(text) = &self.text {
            out.push_str(&escape_text(text));
        }
        for child in &self.children {
            child.write(out);
        }
        out.push_str("</");
        out.push_str(&self.tag);
        out.push('>');
    }

    /// Renders the element to a string.
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}
```

`src/docx/mod.rs` (stub, replaced in Task 8):

```rust
//! Writing Markdown as Microsoft Word documents (OOXML).

pub mod xml;
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::xml`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): a minimal XML writer for OOXML parts"
```

---

### Task 3: A ZIP container writer

**Files:**
- Create: `src/docx/zip.rs`
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: `encode::deflate::deflate(&[u8]) -> Vec<u8>`, `encode::crc32::crc32(&[u8]) -> u32`.
- Produces:
  - `docx::zip::Entry { name: String, data: Vec<u8> }`
  - `docx::zip::ZipWriter::new()`, `.add(&mut self, name: &str, data: Vec<u8>)`, `.finish(self) -> Vec<u8>`
  - `docx::zip::read_archive(&[u8]) -> Vec<(String, Vec<u8>)>` — a test-only reader is added in Task 10; the writer itself exposes only `ZipWriter`.

- [ ] **Step 1: Write the failing test**

In `src/docx/zip.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_starts_with_the_local_header_signature() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish();
        assert_eq!(&out[..4], &[0x50, 0x4b, 0x03, 0x04]);
    }

    #[test]
    fn archive_ends_with_the_end_of_central_directory() {
        let mut w = ZipWriter::new();
        w.add("a.txt", b"hello".to_vec());
        let out = w.finish();
        assert_eq!(&out[out.len() - 22..out.len() - 18], &[0x50, 0x4b, 0x05, 0x06]);
    }

    #[test]
    fn entry_names_and_sizes_are_recorded() {
        let mut w = ZipWriter::new();
        w.add("word/document.xml", b"<w:document/>".to_vec());
        let out = w.finish();
        // The name appears in both the local header and the central directory.
        let name = b"word/document.xml";
        assert_eq!(out.windows(name.len()).filter(|s| *s == name).count(), 2);
    }

    #[test]
    fn compressed_entries_round_trip_through_the_inflater() {
        let mut w = ZipWriter::new();
        let payload = b"abcabcabcabcabcabc".repeat(20);
        w.add("a.txt", payload.clone());
        let out = w.finish();
        let (data, _) = crate::decode::inflate::inflate(&out[30 + 6..], 1 << 20).expect("inflate");
        assert_eq!(data, payload);
    }

    #[test]
    fn an_empty_archive_is_still_valid() {
        let out = ZipWriter::new().finish();
        assert_eq!(out.len(), 22);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::zip`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/zip.rs`:

```rust
//! A ZIP container writer — the envelope a `.docx` ships in.
//!
//! Only the two methods every reader supports are used: stored (0) and
//! deflate (8). Entries are written with a fixed DOS timestamp so the same
//! input always produces the same bytes.

use crate::encode::crc32::crc32;
use crate::encode::deflate::deflate;

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
        self.entries.push(Entry { name: name.to_string(), data });
    }

    /// Writes the archive.
    pub fn finish(self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for entry in &self.entries {
            let offset = out.len() as u32;
            let crc = crc32(&entry.data);
            let compressed = deflate(&entry.data);
            // Fall back to storing when compression does not pay.
            let (method, payload) = if compressed.len() < entry.data.len() {
                (8u16, compressed)
            } else {
                (0u16, entry.data.clone())
            };
            let name = entry.name.as_bytes();

            out.extend_from_slice(&LOCAL_SIG.to_le_bytes());
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&DOS_TIME.to_le_bytes());
            out.extend_from_slice(&DOS_DATE.to_le_bytes());
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            out.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
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
            central.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            central.extend_from_slice(&(entry.data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes()); // extra length
            central.extend_from_slice(&0u16.to_le_bytes()); // comment length
            central.extend_from_slice(&0u16.to_le_bytes()); // disk number
            central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
            central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name);
        }
        let cd_offset = out.len() as u32;
        let cd_size = central.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD_SIG.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // disk with the directory
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment length
        out
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::zip`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): a ZIP container writer for the OOXML package"
```

---

### Task 4: The typed Word model

**Files:**
- Create: `src/docx/model.rs`
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `docx::model::{Document, Block, Paragraph, ParagraphProps, Numbering, Run, TextRun, RunProps, Image, Table, Row, Cell, Alignment}` — the shapes Task 5 fills in and Task 7 serializes.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paragraph_carries_a_style_and_runs() {
        let p = Paragraph {
            style: Some("Heading1".into()),
            props: ParagraphProps::default(),
            runs: vec![Run::Text(TextRun { text: "Hi".into(), props: RunProps::default() })],
        };
        assert_eq!(p.style.as_deref(), Some("Heading1"));
        assert_eq!(p.runs.len(), 1);
    }

    #[test]
    fn run_props_default_to_plain_text() {
        let p = RunProps::default();
        assert!(!p.bold && !p.italic && !p.strike && !p.underline);
        assert!(p.style.is_none() && p.link.is_none());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::model`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/model.rs`:

```rust
//! The Word document model: what a `.docx` contains, before it becomes XML.

use crate::ast::Align;

/// A Word document body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// The body blocks, in document order.
    pub body: Vec<Block>,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A paragraph.
    Paragraph(Paragraph),
    /// A table.
    Table(Table),
}

/// A paragraph and its runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    /// The style ID (`Heading1`, `Quote`, …). `None` means `Normal`.
    pub style: Option<String>,
    /// Paragraph-level properties.
    pub props: ParagraphProps,
    /// The runs, in order.
    pub runs: Vec<Run>,
}

/// Paragraph-level properties.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParagraphProps {
    /// The list this paragraph belongs to.
    pub numbering: Option<Numbering>,
    /// Left indent in twips.
    pub indent_left: u32,
    /// Whether a horizontal rule is drawn below the paragraph.
    pub bottom_border: bool,
}

/// A reference to a numbering definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbering {
    /// The `w:num` identifier.
    pub num_id: u32,
    /// The nesting level, `0` for the outermost list.
    pub level: u8,
}

/// A run of inline content.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Text with character properties.
    Text(TextRun),
    /// A line break.
    Break,
    /// An inline picture.
    Image(Image),
}

/// A text run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextRun {
    /// The literal text.
    pub text: String,
    /// Character properties.
    pub props: RunProps,
}

/// Character properties of a run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunProps {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// Underlined.
    pub underline: bool,
    /// A character style ID (`HTMLCode`, `Hyperlink`).
    pub style: Option<String>,
    /// The URL this run links to.
    pub link: Option<String>,
}

/// An inline picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// The relationship ID of the image part.
    pub rel_id: String,
    /// Width in EMU.
    pub width_emu: u32,
    /// Height in EMU.
    pub height_emu: u32,
    /// Alternative text.
    pub alt: String,
    /// The part's file name.
    pub name: String,
}

/// A table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The rows.
    pub rows: Vec<Row>,
    /// Column widths in twips.
    pub widths: Vec<u32>,
}

/// A table row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The cells.
    pub cells: Vec<Cell>,
    /// Whether this row repeats as a header on every page.
    pub header: bool,
}

/// A table cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The cell's paragraphs.
    pub paragraphs: Vec<Paragraph>,
    /// Horizontal alignment.
    pub align: Align,
}
```

Add `pub mod model;` to `src/docx/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::model`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): the typed Word document model"
```

---

### Task 5: Embedding images

**Files:**
- Create: `src/docx/media.rs`
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: `decode::decode(&[u8]) -> Option<Decoded>` (`Decoded { width, height, pixels }`), `encode::png::encode(u32, u32, &[u8]) -> Vec<u8>`.
- Produces:
  - `docx::media::EMU_PER_PX: u32` (`9525`)
  - `docx::media::CONTENT_WIDTH_EMU: u32` (`5731510`)
  - `docx::media::CONTENT_HEIGHT_EMU: u32` (`8864330`)
  - `docx::media::fit_emu(width_px: u32, height_px: u32) -> (u32, u32)`
  - `docx::media::load_png(path: &Path) -> Option<(Vec<u8>, u32, u32)>` — PNG bytes plus the pixel size.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_become_emu_at_96_dpi() {
        assert_eq!(fit_emu(96, 48), (96 * EMU_PER_PX, 48 * EMU_PER_PX));
    }

    #[test]
    fn a_wide_image_is_scaled_down_to_the_content_box() {
        let (w, h) = fit_emu(4000, 1000);
        assert_eq!(w, CONTENT_WIDTH_EMU);
        assert!((h as f64 - CONTENT_WIDTH_EMU as f64 * 0.25).abs() < 1.0);
    }

    #[test]
    fn a_tall_image_is_scaled_down_to_the_content_box() {
        let (w, h) = fit_emu(100, 20000);
        assert_eq!(h, CONTENT_HEIGHT_EMU);
        assert!(w < CONTENT_WIDTH_EMU);
    }

    #[test]
    fn a_missing_file_loads_as_none() {
        assert!(load_png(std::path::Path::new("no-such-image.png")).is_none());
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::media`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/media.rs`:

```rust
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
pub fn fit_emu(width_px: u32, height_px: u32) -> (u32, u32) {
    let mut w = width_px.saturating_mul(EMU_PER_PX);
    let mut h = height_px.saturating_mul(EMU_PER_PX);
    if w > CONTENT_WIDTH_EMU || h > CONTENT_HEIGHT_EMU {
        let scale = (CONTENT_WIDTH_EMU as f64 / w as f64).min(CONTENT_HEIGHT_EMU as f64 / h as f64);
        w = (w as f64 * scale).round() as u32;
        h = (h as f64 * scale).round() as u32;
    }
    (w, h)
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
```

Add `pub mod media;` to `src/docx/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::media`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): embed local images as PNG parts"
```

---

### Task 6: Markdown AST → Word model

**Files:**
- Create: `src/docx/content.rs`
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: `ast::{Document, Block, Inline, ListData, TableData, ImageData, Align}`, `html::parse_html`, `docx::model::*`, `docx::media`.
- Produces:
  - `docx::content::ImagePart { name: String, bytes: Vec<u8> }`
  - `docx::content::NumberingPlan { bullet_num_id: u32, ordered: Vec<OrderedList> }` with `OrderedList { num_id: u32, start: u32 }`
  - `docx::content::Content { document: model::Document, images: Vec<ImagePart>, links: Vec<String>, numbering: NumberingPlan, toc: bool }`
  - `docx::content::build(doc: &ast::Document, options: &Options) -> Content`

**Mapping rules (the whole task):**

- `Heading { level, inlines }` → paragraph, style `Heading{level}`.
- `Paragraph(inlines)` → paragraph, style `None` (Normal).
- `Code(block)` → one paragraph per line, style `HTMLPreformatted`; an empty block yields one empty paragraph.
- `Quote(blocks)` → every block inside becomes a paragraph with style `Quote`; each nesting level adds 720 twips of `indent_left`.
- `List(list)` → every block of every item becomes a paragraph with `ListParagraph` and a `Numbering { num_id, level }`. Bullets share `num_id = 1`; each ordered list gets its own `num_id` (2, 3, …) with its own `start`, so lists restart independently. A nested list inside an item is one level deeper. A task item (`ListItem::task`) gets a `☐` or `☒` prefix in front of its first run.
- `Table(table)` → a `model::Table` with one row per GFM row (the header first, marked `header: true`), one cell per column, equal column widths (`9026 / columns` twips), and the cell's `Align` taken from the GFM alignment row.
- `Figure(image)` → a paragraph with an image run; if the image cannot be loaded, a text run with the alt text.
- `ThematicBreak` → an empty paragraph with `bottom_border: true`.
- `Html(raw)` → parse with `html::parse_html` and map the tree (see the inline rules below); block-level children become paragraphs.

**Inline rules:**

- `Text(s)` → a text run.
- `Code(s)` → a text run with character style `HTMLCode`.
- `Emph` → italic, `Strong` → bold, `Strike` → strike, `Underline` → underline.
- `Link { text, url }` → the runs of `text`, each carrying `props.link = Some(url)`. The URL is registered in `Content::links` (deduplicated, first-seen order) and the run keeps the URL itself; Task 7 resolves it to a relationship ID.
- `Image(image)` → an image run when the file loads, otherwise a text run with the alt text.
- `Break` → `Run::Break`; `SoftBreak` → a text run containing a single space.
- `Html(raw)` → parse and map: `b`/`strong` → bold, `i`/`em` → italic, `u`/`ins` → underline, `s`/`del`/`strike` → strike, `code` → character style `HTMLCode`, `a` → hyperlink (its `href` attribute), `br` → line break, `img` → image, any other tag → a transparent container whose children are inlined.

**Image loading:** paths resolve against `Options::base_dir` (the working directory when it is `None`) and only when `Options::local_images` is true. Each loaded image becomes an `ImagePart` named `image{N}.png` (N counting from 1) and its relationship ID is `rId{images.len() + 4}` — the first three relationship IDs are reserved for styles, numbering and settings (Task 7).

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn build(markdown: &str) -> Content {
        build(&parse(markdown), &Options::default())
    }

    #[test]
    fn a_heading_becomes_a_styled_paragraph() {
        let c = build("# Title\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!("expected a paragraph") };
        assert_eq!(p.style.as_deref(), Some("Heading1"));
    }

    #[test]
    fn emphasis_becomes_run_properties() {
        let c = build("**b** *i* ~~s~~\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        let props: Vec<_> = p.runs.iter().filter_map(|r| match r {
            Run::Text(t) => Some(&t.props),
            _ => None,
        }).collect();
        assert!(props[0].bold);
        assert!(props[1].italic);
        assert!(props[2].strike);
    }

    #[test]
    fn inline_code_uses_the_code_character_style() {
        let c = build("`x`\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        let Run::Text(t) = &p.runs[0] else { panic!() };
        assert_eq!(t.props.style.as_deref(), Some("HTMLCode"));
    }

    #[test]
    fn a_link_is_recorded_once() {
        let c = build("[a](https://x.y) [b](https://x.y)\n");
        assert_eq!(c.links, vec!["https://x.y".to_string()]);
    }

    #[test]
    fn a_bullet_list_uses_the_bullet_numbering() {
        let c = build("- a\n- b\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        assert_eq!(p.style.as_deref(), Some("ListParagraph"));
        assert_eq!(p.props.numbering.as_ref().unwrap().num_id, c.numbering.bullet_num_id);
        assert_eq!(p.props.numbering.as_ref().unwrap().level, 0);
    }

    #[test]
    fn each_ordered_list_gets_its_own_numbering() {
        let c = build("1. a\n\n1. b\n");
        assert_eq!(c.numbering.ordered.len(), 2);
        assert_ne!(c.numbering.ordered[0].num_id, c.numbering.ordered[1].num_id);
    }

    #[test]
    fn an_ordered_list_keeps_its_start_number() {
        let c = build("3. a\n");
        assert_eq!(c.numbering.ordered[0].start, 3);
    }

    #[test]
    fn nested_lists_deepen_the_level() {
        let c = build("- a\n  - b\n");
        let levels: Vec<u8> = c.document.body.iter().filter_map(|b| match b {
            Block::Paragraph(p) => p.props.numbering.as_ref().map(|n| n.level),
            _ => None,
        }).collect();
        assert_eq!(levels, vec![0, 1]);
    }

    #[test]
    fn a_task_item_gets_a_checkbox_prefix() {
        let c = build("- [x] done\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        let Run::Text(t) = &p.runs[0] else { panic!() };
        assert_eq!(t.text, "☒ ");
    }

    #[test]
    fn a_table_becomes_a_table_with_a_header_row() {
        let c = build("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let Block::Table(t) = &c.document.body[0] else { panic!("expected a table") };
        assert_eq!(t.rows.len(), 2);
        assert!(t.rows[0].header);
        assert_eq!(t.rows[0].cells.len(), 2);
        assert_eq!(t.widths.len(), 2);
    }

    #[test]
    fn a_thematic_break_becomes_a_bordered_paragraph() {
        let c = build("---\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        assert!(p.props.bottom_border);
    }

    #[test]
    fn a_quote_uses_the_quote_style() {
        let c = build("> q\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        assert_eq!(p.style.as_deref(), Some("Quote"));
    }

    #[test]
    fn a_code_block_becomes_one_paragraph_per_line() {
        let c = build("```\na\nb\n```\n");
        assert_eq!(c.document.body.len(), 2);
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        assert_eq!(p.style.as_deref(), Some("HTMLPreformatted"));
    }

    #[test]
    fn raw_html_is_mapped_by_tag() {
        let c = build("a <u>b</u> <b>c</b>\n");
        let Block::Paragraph(p) = &c.document.body[0] else { panic!() };
        let props: Vec<_> = p.runs.iter().filter_map(|r| match r {
            Run::Text(t) => Some(t.props.clone()),
            _ => None,
        }).collect();
        assert!(props.iter().any(|p| p.underline));
        assert!(props.iter().any(|p| p.bold));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::content`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/content.rs` — the builder holds the state that spans the whole document:

```rust
//! Mapping the Markdown AST onto the Word model.

use std::path::{Path, PathBuf};

use crate::ast::{Align, Block as AstBlock, Document, Inline as AstInline, ListItem};
use crate::html::{HtmlElement, HtmlNode, parse_html};
use crate::options::Options;

use super::media;
use super::model::{Block, Cell, Document as WordDocument, Image, Numbering, Paragraph, ParagraphProps, Row, Run, RunProps, Table, TextRun};

/// An image part inside the package.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePart {
    /// The part name, e.g. `image1.png`.
    pub name: String,
    /// The PNG bytes.
    pub bytes: Vec<u8>,
}

/// One ordered list instance and the number it starts at.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedList {
    /// The `w:num` identifier.
    pub num_id: u32,
    /// The first number of the list.
    pub start: u32,
}

/// The numbering definitions a document needs.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingPlan {
    /// The `w:num` identifier shared by every bullet list.
    pub bullet_num_id: u32,
    /// One entry per ordered list, in document order.
    pub ordered: Vec<OrderedList>,
}

/// Everything the writer needs to produce the package.
#[derive(Debug, Clone, PartialEq)]
pub struct Content {
    /// The document body.
    pub document: WordDocument,
    /// The image parts, in relationship order.
    pub images: Vec<ImagePart>,
    /// The distinct link targets, in first-seen order.
    pub links: Vec<String>,
    /// The numbering definitions.
    pub numbering: NumberingPlan,
    /// Whether a table of contents was requested.
    pub toc: bool,
}

/// Maps a parsed document onto the Word model.
pub fn build(doc: &Document, options: &Options) -> Content {
    let mut builder = Builder {
        options,
        body: Vec::new(),
        images: Vec::new(),
        links: Vec::new(),
        numbering: NumberingPlan { bullet_num_id: 1, ordered: Vec::new() },
        next_num_id: 2,
    };
    builder.blocks(&doc.blocks, 0, 0);
    Content {
        document: WordDocument { body: builder.body },
        images: builder.images,
        links: builder.links,
        numbering: builder.numbering,
        toc: options.toc,
    }
}
```

The builder is a private struct with the state that spans the whole document:

```rust
/// Walks the AST once, producing the Word model and the package's
/// relationship state.
struct Builder<'a> {
    options: &'a Options,
    body: Vec<Block>,
    images: Vec<ImagePart>,
    links: Vec<String>,
    numbering: NumberingPlan,
    next_num_id: u32,
}
```

Its methods, in the order they call each other:

```rust
impl<'a> Builder<'a> {
    /// Emits a sequence of blocks at a given quote depth and list level.
    fn blocks(&mut self, blocks: &[AstBlock], quote_depth: u32, list_level: u8) {
        for block in blocks {
            self.block(block, quote_depth, list_level);
        }
    }

    /// Emits one block.
    fn block(&mut self, block: &AstBlock, quote_depth: u32, list_level: u8) {
        match block {
            AstBlock::Heading { level, inlines } => {
                let p = self.paragraph(inlines, Some(format!("Heading{level}")), list_level);
                self.body.push(Block::Paragraph(p));
            }
            AstBlock::Paragraph(inlines) => {
                let p = self.paragraph(inlines, None, list_level);
                self.body.push(Block::Paragraph(p));
            }
            AstBlock::Code(code) => {
                for line in &code.lines {
                    let run = Run::Text(TextRun { text: line.clone(), props: RunProps::default() });
                    self.body.push(Block::Paragraph(Paragraph {
                        style: Some("HTMLPreformatted".into()),
                        props: ParagraphProps::default(),
                        runs: vec![run],
                    }));
                }
            }
            AstBlock::Quote(inner) => {
                for block in inner {
                    self.quote_block(block, quote_depth + 1, list_level);
                }
            }
            AstBlock::List(list) => self.list(list, list_level),
            AstBlock::Table(table) => self.body.push(Block::Table(self.table(table))),
            AstBlock::Figure(image) => {
                let runs = self.image_runs(image);
                self.body.push(Block::Paragraph(Paragraph {
                    style: None,
                    props: ParagraphProps::default(),
                    runs,
                }));
            }
            AstBlock::ThematicBreak => {
                self.body.push(Block::Paragraph(Paragraph {
                    style: None,
                    props: ParagraphProps { bottom_border: true, ..ParagraphProps::default() },
                    runs: Vec::new(),
                }));
            }
            AstBlock::Html(raw) => self.html_block(raw),
        }
    }
}
```

The remaining methods follow the same shape: `paragraph` builds a paragraph
from inlines with a style and a list level, `inlines` maps a sequence of
AST inlines to runs, `inline` maps one, `list` walks a list's items,
`quote_block` re-emits a block with the `Quote` style and 720 twips of
extra indent per level, `table` builds the rows and the equal column
widths, `image_runs` loads the image or falls back to the alt text, and
`html_block`/`html_nodes`/`html_element` map raw HTML.

Two rules are worth stating explicitly in the code:

- A list item's blocks are emitted at the item's level; a nested list inside the item is emitted one level deeper (`level + 1`).
- Every ordered list allocates a fresh `num_id` from `next_num_id`, so two lists never share a counter.

`Align` in `docx::model` is `crate::ast::Align` — the writer needs it for
cell alignment, and the AST already defines exactly the three cases Word
has (`left`, `center`, `right`). `model.rs` therefore starts with
`use crate::ast::Align;` and the writer imports `crate::ast::Align`
directly; there is no second alignment type.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::content`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): map the Markdown AST onto the Word model"
```

---

### Task 7: OOXML parts

**Files:**
- Create: `src/docx/write.rs`
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: `docx::model::*`, `docx::content::{Content, ImagePart, NumberingPlan}`, `docx::xml::Element`.
- Produces:
  - `docx::write::content_types(images: usize) -> String`
  - `docx::write::package_rels() -> String`
  - `docx::write::document_rels(images: &[ImagePart], links: &[String]) -> String`
  - `docx::write::styles() -> String`
  - `docx::write::numbering(plan: &NumberingPlan) -> String`
  - `docx::write::settings(update_fields: bool) -> String`
  - `docx::write::document(content: &Content) -> String`
  - `docx::write::image_rel_id(index: usize) -> String` — `rId{index + 4}`
  - `docx::write::link_rel_id(index: usize) -> String` — `rId{4 + images + index}`

**Part contents:**

`[Content_Types].xml` — `Default` for `rels`, `xml` and (when images exist) `png`; `Override` for `/word/document.xml`, `/word/styles.xml`, `/word/numbering.xml`, `/word/settings.xml`.

`_rels/.rels` — one relationship, `rId1` → `word/document.xml`.

`word/_rels/document.xml.rels` — `rId1` → `styles.xml`, `rId2` → `numbering.xml`, `rId3` → `settings.xml`, then one relationship per image (`rId4` upwards) and one per link.

`word/styles.xml` — `w:docDefaults` (Calibri, 11 pt, 160 twips after, 259 line) plus the style IDs `Normal`, `Heading1`…`Heading6`, `Quote`, `ListParagraph`, `HTMLPreformatted`, `HTMLCode`, `Hyperlink`, `TableGrid`, `TOCHeading`, `TOC1`, `TOC2`, `TOC3`. Headings carry `w:outlineLvl` 0…5, which is what makes the TOC field find them.

`word/numbering.xml` — two abstract definitions (`abstractNumId` 0 = bullet, 1 = decimal), each with nine levels, then one `w:num` per instance: `numId 1` → abstract 0, and one per ordered list → abstract 1 with `<w:lvlOverride w:ilvl="0"><w:startOverride w:val="{start}"/></w:lvlOverride>`.

`word/settings.xml` — `w:defaultTabStop` always; `w:updateFields` only when the document has a TOC field.

`word/document.xml` — the root `w:document` declares the namespaces `w`, `r`, `wp`, `a` and `pic`. The body holds the model's blocks and ends with one `w:sectPr` (A4: `w:pgSz w:w="11906" w:h="16838"`, 1440-twip margins).

Serialization rules:

- A paragraph is `<w:p><w:pPr>…</w:pPr>runs</w:p>`; `w:pPr` holds `w:pStyle`, `w:numPr` (`w:ilvl` + `w:numId`), `w:ind w:left`, and `w:pBdr/w:bottom`.
- A run is `<w:r><w:rPr>…</w:rPr><w:t xml:space="preserve">text</w:t></w:r>`; `w:rPr` holds `w:b`, `w:i`, `w:strike`, `w:u w:val="single"`, `w:rStyle`.
- A linked run is wrapped in `<w:hyperlink r:id="rIdN">…</w:hyperlink>`.
- A line break is `<w:r><w:br/></w:r>`.
- An image is `<w:r><w:drawing><wp:inline>…<a:graphic><a:graphicData><pic:pic>…<a:blip r:embed="rIdN"/>…` with `wp:extent cx cy`, `wp:docPr id name descr` and `pic:spPr/a:xfrm/a:ext`.
- A table is `<w:tbl><w:tblPr>…<w:tblGrid><w:gridCol w:w="…"/>…<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc><w:tcPr><w:tcW …/></w:tcPr><w:p>…</w:p></w:tc>…` and is always followed by an empty `<w:p/>`.
- The TOC is a `TOCHeading` paragraph with the title, then a paragraph holding the field: `<w:fldChar w:fldCharType="begin" w:dirty="true"/>`, `<w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText>`, `<w:fldChar w:fldCharType="separate"/>`, a placeholder run, `<w:fldChar w:fldCharType="end"/>`.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_define_every_style_the_document_uses() {
        let styles = styles();
        for id in ["Normal", "Heading1", "Heading6", "Quote", "ListParagraph",
                   "HTMLPreformatted", "HTMLCode", "Hyperlink", "TableGrid",
                   "TOCHeading", "TOC1", "TOC3"] {
            assert!(styles.contains(&format!(r#"w:styleId="{id}""#)), "missing {id}");
        }
    }

    #[test]
    fn headings_carry_an_outline_level() {
        assert!(styles().contains(r#"<w:outlineLvl w:val="0"/>"#));
    }

    #[test]
    fn numbering_defines_both_abstract_lists() {
        let plan = NumberingPlan { bullet_num_id: 1, ordered: vec![OrderedList { num_id: 2, start: 3 }] };
        let xml = numbering(&plan);
        assert!(xml.contains(r#"w:abstractNumId="0""#));
        assert!(xml.contains(r#"w:abstractNumId="1""#));
        assert!(xml.contains(r#"<w:startOverride w:val="3"/>"#));
    }

    #[test]
    fn settings_only_update_fields_when_a_toc_is_present() {
        assert!(!settings(false).contains("updateFields"));
        assert!(settings(true).contains(r#"<w:updateFields w:val="true"/>"#));
    }

    #[test]
    fn content_types_cover_every_part() {
        let xml = content_types(0);
        assert!(xml.contains(r#"PartName="/word/document.xml""#));
        assert!(xml.contains(r#"PartName="/word/styles.xml""#));
        assert!(xml.contains(r#"PartName="/word/numbering.xml""#));
        assert!(xml.contains(r#"PartName="/word/settings.xml""#));
        assert!(!xml.contains(r#"Extension="png""#));
        assert!(content_types(1).contains(r#"Extension="png""#));
    }

    #[test]
    fn package_relationships_point_at_the_document() {
        assert!(package_rels().contains(r#"Target="word/document.xml""#));
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::write`
Expected: FAIL — the module does not exist.

- [ ] **Step 3: Write the implementation**

Write `src/docx/write.rs` with the functions listed above. Two details that must be exact:

- The `w:document` root carries the namespaces the drawing markup needs:

```rust
let root = Element::new("w:document")
    .attr("xmlns:w", "http://schemas.openxmlformats.org/wordprocessingml/2006/main")
    .attr("xmlns:r", "http://schemas.openxmlformats.org/officeDocument/2006/relationships")
    .attr("xmlns:wp", "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing")
    .attr("xmlns:a", "http://schemas.openxmlformats.org/drawingml/2006/main")
    .attr("xmlns:pic", "http://schemas.openxmlformats.org/drawingml/2006/picture");
```

- Every XML part starts with the declaration
  `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>\r\n` — the
  carriage return is what Word itself writes.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::write`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): generate the OOXML parts"
```

---

### Task 8: Assemble the package

**Files:**
- Modify: `src/docx/mod.rs`

**Interfaces:**
- Consumes: everything from Tasks 2–7.
- Produces:
  - `docx::render_bytes(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError>`
  - `docx::build_package(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError>`

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_is_a_zip_with_the_expected_parts() {
        let bytes = render_bytes("# Hi\n", &Options::default()).unwrap();
        assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[Content_Types].xml"));
        assert!(text.contains("word/document.xml"));
    }

    #[test]
    fn the_same_input_produces_the_same_bytes() {
        let a = render_bytes("# Hi\n\ntext", &Options::default()).unwrap();
        let b = render_bytes("# Hi\n\ntext", &Options::default()).unwrap();
        assert_eq!(a, b);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx::`
Expected: FAIL — `render_bytes` does not exist.

- [ ] **Step 3: Write the implementation**

`src/docx/mod.rs`:

```rust
//! Writing Markdown as Microsoft Word documents.
//!
//! A `.docx` is a ZIP container holding OOXML parts. The Markdown AST is
//! mapped onto a Word model, the model is serialized into parts, and the
//! parts are zipped — three stages, each testable on its own.

pub mod content;
pub mod media;
pub mod model;
pub mod write;
pub mod xml;
pub mod zip;

use crate::options::{Options, RenderError};
use crate::parse::parse;

/// Renders Markdown as a Word document.
pub fn render_bytes(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError> {
    let doc = parse(markdown);
    let content = content::build(&doc, options);
    let mut package = zip::ZipWriter::new();
    package.add("[Content_Types].xml", write::content_types(content.images.len()).into_bytes());
    package.add("_rels/.rels", write::package_rels().into_bytes());
    package.add("word/document.xml", write::document(&content).into_bytes());
    package.add(
        "word/_rels/document.xml.rels",
        write::document_rels(&content.images, &content.links).into_bytes(),
    );
    package.add("word/styles.xml", write::styles().into_bytes());
    package.add("word/numbering.xml", write::numbering(&content.numbering).into_bytes());
    package.add("word/settings.xml", write::settings(content.toc).into_bytes());
    for image in &content.images {
        package.add(&format!("word/media/{}", image.name), image.bytes.clone());
    }
    Ok(package.finish())
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --lib docx::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/docx
git commit -m "feat(docx): assemble the Word package"
```

---

### Task 9: Wire DOCX into the public API and the CLI

**Files:**
- Modify: `src/lib.rs`, `src/engine.rs`, `src/main.rs`
- Test: `src/lib.rs` (unit tests), `src/main.rs` (unit tests)

**Interfaces:**
- Consumes: `docx::render_bytes`.
- Produces:
  - `markpop::render_docx(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError>`
  - `OutputFormat::Docx`, `OutputFormat::by_name("docx")`, `extension() == "docx"`, `mime() == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"`, `is_raster() == false`.

- [ ] **Step 1: Write the failing test**

In `src/engine.rs`'s test module:

```rust
#[test]
fn docx_is_a_known_format() {
    assert_eq!(OutputFormat::by_name("docx"), Some(OutputFormat::Docx));
    assert_eq!(OutputFormat::Docx.extension(), "docx");
    assert!(!OutputFormat::Docx.is_raster());
}
```

In `src/lib.rs`'s test module:

```rust
#[test]
fn render_docx_produces_a_zip() {
    let bytes = markpop::render_docx("# Hi", &Options::default()).unwrap();
    assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
}

#[test]
fn the_byte_api_honours_the_docx_format() {
    let opts = Options::default().with_format(OutputFormat::Docx);
    let bytes = markpop::render_bytes("# Hi", &opts).unwrap();
    assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib docx_is_a_known_format render_docx`
Expected: FAIL — `OutputFormat::Docx` does not exist.

- [ ] **Step 3: Write the implementation**

`src/engine.rs` — add the variant and its arms:

```rust
    /// A Microsoft Word document (OOXML).
    Docx,
```

```rust
            "docx" => Some(OutputFormat::Docx),
```

```rust
            OutputFormat::Docx => "docx",
```

```rust
            OutputFormat::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
```

`is_raster` stays `!matches!(self, OutputFormat::Svg | OutputFormat::Docx)`.

`src/lib.rs` — declare the module and the entry points:

```rust
pub mod docx;
```

```rust
/// Renders Markdown as a Microsoft Word document.
///
/// The document uses Word's built-in styles, so the theme does not affect
/// its appearance. `--toc` becomes a Word TOC field that Word refreshes
/// on open.
pub fn render_docx(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError> {
    options.validate()?;
    docx::render_bytes(markdown, options)
}
```

`render_bytes` dispatches before the engine path:

```rust
pub fn render_bytes(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError> {
    options.validate()?;
    if options.format == OutputFormat::Docx {
        return docx::render_bytes(markdown, options);
    }
    // … the existing engine path
}
```

`render_bytes_pages` returns a single document for DOCX, because Word
paginates the flow itself:

```rust
pub fn render_bytes_pages(markdown: &str, options: &Options) -> Result<Vec<Vec<u8>>, RenderError> {
    options.validate()?;
    if options.format == OutputFormat::Docx {
        return Ok(vec![docx::render_bytes(markdown, options)?]);
    }
    // … the existing engine path
}
```

`src/main.rs` — the format list in the usage text and in the error message
must mention `docx`:

```
    -f, --format <NAME>       Output format: svg (default), png, webp or docx
```

and the error becomes
`unknown format `{raw}` (expected svg, png, webp or docx)`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test`
Expected: PASS — all existing tests plus the new ones.

- [ ] **Step 5: Commit**

```bash
git add src/lib.rs src/engine.rs src/main.rs
git commit -m "feat(docx): a Word output format for the API and the CLI"
```

---

### Task 10: Package-integrity integration tests

**Files:**
- Create: `tests/docx.rs`
- Modify: `src/docx/mod.rs` (expose a test-only archive reader) or the test file itself

**Interfaces:**
- Consumes: `markpop::render_docx`, `markpop::Options`.
- Produces: a test-only ZIP reader and the integrity assertions.

- [ ] **Step 1: Write the archive reader**

The crate's inflater is `pub(crate)`, so the reader that puts a package
back together has to live inside the crate. It goes in `src/docx/zip.rs`
next to the writer:

```rust
// src/docx/zip.rs

/// Reads an archive back into its entries.
///
/// Used by the test-suite to inspect what the writer produced.
pub fn read_archive(data: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 30 <= data.len() && u32::from_le_bytes(data[pos..pos + 4].try_into().ok()?) == LOCAL_SIG {
        let method = u16::from_le_bytes(data[pos + 8..pos + 10].try_into().ok()?);
        let csize = u32::from_le_bytes(data[pos + 18..pos + 22].try_into().ok()?) as usize;
        let usize_ = u32::from_le_bytes(data[pos + 22..pos + 26].try_into().ok()?) as usize;
        let name_len = u16::from_le_bytes(data[pos + 26..pos + 28].try_into().ok()?) as usize;
        let extra_len = u16::from_le_bytes(data[pos + 28..pos + 30].try_into().ok()?) as usize;
        let name = String::from_utf8(data[pos + 30..pos + 30 + name_len].to_vec()).ok()?;
        let start = pos + 30 + name_len + extra_len;
        let payload = &data[start..start + csize];
        let content = match method {
            0 => payload.to_vec(),
            8 => crate::decode::inflate::inflate(payload, 64 << 20)?.0,
            _ => return None,
        };
        if content.len() != usize_ {
            return None;
        }
        out.push((name, content));
        pos = start + csize;
    }
    Some(out)
}
```

`crate::decode::inflate` is `pub(crate)`, so `read_archive` must live
inside the crate — it does, in `src/docx/zip.rs`. It is public API
(`markpop::docx::zip::read_archive`), which is useful in its own right: a
consumer can verify a document they did not write.

- [ ] **Step 2: Write the failing tests**

`tests/docx.rs`:

```rust
//! Integrity tests for the Word output.
//!
//! A `.docx` is only useful if Word can open it, and Word refuses files
//! whose parts do not agree with each other. These tests assert that
//! agreement on the package itself.

use markpop::docx::zip::read_archive;
use markpop::{Options, render_docx};

/// The parts of a generated document, by name.
fn parts(markdown: &str) -> Vec<(String, String)> {
    let bytes = render_docx(markdown, &Options::default()).expect("render");
    read_archive(&bytes)
        .expect("a readable archive")
        .into_iter()
        .map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned()))
        .collect()
}

/// The body of the generated document.
fn document_xml(markdown: &str) -> String {
    parts(markdown).into_iter().find(|(n, _)| n == "word/document.xml").map(|(_, x)| x).expect("document.xml")
}

#[test]
fn the_package_has_every_required_part() {
    let names: Vec<String> = parts("# Hi\n").into_iter().map(|(n, _)| n).collect();
    for expected in ["[Content_Types].xml", "_rels/.rels", "word/document.xml",
                     "word/_rels/document.xml.rels", "word/styles.xml",
                     "word/numbering.xml", "word/settings.xml"] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
}

#[test]
fn every_part_is_declared_in_the_content_types() {
    let all = parts("# Hi\n");
    let types = all.iter().find(|(n, _)| n == "[Content_Types].xml").map(|(_, x)| x.clone()).unwrap();
    for (name, _) in &all {
        if name == "[Content_Types].xml" {
            continue;
        }
        let ext = name.rsplit('.').next().unwrap();
        let declared = types.contains(&format!(r#"Extension="{ext}""#))
            || types.contains(&format!(r#"PartName="/{name}""#));
        assert!(declared, "{name} is not declared in [Content_Types].xml");
    }
}

#[test]
fn every_relationship_resolves() {
    let all = parts("# Hi\n\n[a](https://example.com)\n");
    let doc = all.iter().find(|(n, _)| n == "word/document.xml").map(|(_, x)| x.clone()).unwrap();
    let rels = all.iter().find(|(n, _)| n == "word/_rels/document.xml.rels").map(|(_, x)| x.clone()).unwrap();
    let mut ids = Vec::new();
    let mut rest = doc.as_str();
    while let Some(pos) = rest.find("r:id=\"") {
        rest = &rest[pos + 6..];
        let id = rest[..rest.find('"').unwrap()].to_string();
        ids.push(id);
    }
    let mut rest = doc.as_str();
    while let Some(pos) = rest.find("r:embed=\"") {
        rest = &rest[pos + 9..];
        let id = rest[..rest.find('"').unwrap()].to_string();
        ids.push(id);
    }
    assert!(!ids.is_empty(), "the document should reference relationships");
    for id in ids {
        assert!(rels.contains(&format!(r#"Id="{id}""#)), "unresolved relationship {id}");
    }
}

#[test]
fn every_style_the_document_uses_is_defined() {
    let all = parts("# Hi\n\n> q\n\n- item\n\n```\ncode\n```\n");
    let doc = all.iter().find(|(n, _)| n == "word/document.xml").map(|(_, x)| x.clone()).unwrap();
    let styles = all.iter().find(|(n, _)| n == "word/styles.xml").map(|(_, x)| x.clone()).unwrap();
    let mut rest = doc.as_str();
    while let Some(pos) = rest.find("<w:pStyle w:val=\"") {
        rest = &rest[pos + 17..];
        let id = rest[..rest.find('"').unwrap()].to_string();
        assert!(styles.contains(&format!(r#"w:styleId="{id}""#)), "undefined style {id}");
    }
}

#[test]
fn every_numbering_id_the_document_uses_is_defined() {
    let all = parts("- a\n\n1. b\n");
    let doc = all.iter().find(|(n, _)| n == "word/document.xml").map(|(_, x)| x.clone()).unwrap();
    let numbering = all.iter().find(|(n, _)| n == "word/numbering.xml").map(|(_, x)| x.clone()).unwrap();
    let mut rest = doc.as_str();
    while let Some(pos) = rest.find("<w:numId w:val=\"") {
        rest = &rest[pos + 17..];
        let id = rest[..rest.find('"').unwrap()].to_string();
        assert!(numbering.contains(&format!(r#"<w:num w:numId="{id}">"#)), "undefined numId {id}");
    }
}

#[test]
fn the_text_of_the_document_matches_the_source() {
    let xml = document_xml("# Title\n\nSome **bold** text.\n");
    let mut text = String::new();
    let mut rest = xml.as_str();
    while let Some(pos) = rest.find("<w:t") {
        rest = &rest[pos..];
        let Some(start) = rest.find('>') else { break };
        rest = &rest[start + 1..];
        let Some(end) = rest.find("</w:t>") else { break };
        text.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    assert!(text.contains("Title"));
    assert!(text.contains("Some "));
    assert!(text.contains("bold"));
}
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cargo test --test docx`
Expected: FAIL until `read_archive` exists and the parts are complete.

- [ ] **Step 4: Make it pass**

Add `read_archive` to `src/docx/zip.rs` (Step 1 above) and make sure the
document emits `r:id` for hyperlinks and `r:embed` for images.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test docx`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add tests/docx.rs src/docx/zip.rs
git commit -m "test(docx): assert the package's parts agree with each other"
```

---

### Task 11: Verify the file in Microsoft Word

**Files:**
- Create: `tests/docx_word.rs`

**Interfaces:**
- Consumes: `markpop::render_docx`, and Word through PowerShell COM.
- Produces: a test that opens the document in Word and checks its structure.

- [ ] **Step 1: Write the test**

`tests/docx_word.rs`:

```rust
//! Opens a generated document in Microsoft Word and checks its structure.
//!
//! Word is the oracle: a package our own reader accepts can still be one
//! Word refuses to open. The test skips itself when Word is not installed.

use std::path::PathBuf;
use std::process::Command;

/// The PowerShell script that opens the document and reports its structure.
const PROBE: &str = r#"
$ErrorActionPreference = "Stop"
$word = New-Object -ComObject Word.Application
$word.Visible = $false
$word.DisplayAlerts = 0
try {
    $doc = $word.Documents.Open($args[0], $false, $true, $false)
    $headings = @()
    foreach ($p in $doc.Paragraphs) {
        $name = $p.Style.NameLocal
        if ($name -like "Heading*") { $headings += $name }
    }
    $tables = $doc.Tables.Count
    $lists = $doc.Lists.Count
    $shapes = $doc.InlineShapes.Count
    $fields = $doc.Fields.Count
    $text = $doc.Content.Text
    Write-Output "PARAGRAPHS=$($doc.Paragraphs.Count)"
    Write-Output "TABLES=$tables"
    Write-Output "LISTS=$lists"
    Write-Output "SHAPES=$shapes"
    Write-Output "FIELDS=$fields"
    Write-Output "HEADINGS=$($headings -join ",")"
    Write-Output "TEXT=$($text.Substring(0, [Math]::Min(200, $text.Length)))"
    $doc.Close($false)
} finally {
    $word.Quit()
}
"#;

/// Whether Word can be driven on this machine.
fn word_available() -> bool {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", "try { $w = New-Object -ComObject Word.Application; $w.Quit(); exit 0 } catch { exit 1 }"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Runs the probe and returns its output.
fn probe(path: &PathBuf) -> String {
    let script = std::env::temp_dir().join(format!("markpop-probe-{}.ps1", std::process::id()));
    std::fs::write(&script, PROBE).expect("write probe");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(path)
        .output()
        .expect("run powershell");
    let _ = std::fs::remove_file(&script);
    assert!(out.status.success(), "Word could not open the document: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Reads a `KEY=VALUE` line out of the probe output.
fn field(output: &str, key: &str) -> String {
    output
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn word_opens_the_document_and_finds_its_structure() {
    if !word_available() {
        eprintln!("skipping: Microsoft Word is not installed");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("markpop-word-{}.docx", std::process::id()));
    let bytes = markpop::render_docx(
        "# Title\n\nSome text.\n\n## Section\n\n- one\n- two\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        &markpop::Options::default(),
    )
    .expect("render");
    std::fs::write(&path, bytes).expect("write");

    let output = probe(&path);
    let _ = std::fs::remove_file(&path);

    assert!(field(&output, "TABLES").parse::<u32>().unwrap_or(0) >= 1, "no table: {output}");
    assert!(field(&output, "LISTS").parse::<u32>().unwrap_or(0) >= 1, "no list: {output}");
    let headings = field(&output, "HEADINGS");
    assert!(headings.contains("Heading 1") || headings.contains("Heading1"), "no heading: {output}");
    assert!(field(&output, "TEXT").contains("Title"), "no text: {output}");
}
```

- [ ] **Step 2: Run it to verify it fails or passes**

Run: `cargo test --test docx_word -- --nocapture`
Expected: the test either passes (if Word opens the document) or fails with
Word's error message — which is exactly the signal that the package is not
yet valid. Iterate on `src/docx/write.rs` until it passes.

- [ ] **Step 3: Run the whole suite**

Run: `cargo test`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tests/docx_word.rs src/docx
git commit -m "test(docx): open the document in Microsoft Word and check its structure"
```

---

### Task 12: Cross-check the package with independent tools

**Files:**
- Create: `scripts/verify-docx.ps1`
- Test: `tests/docx_external.rs`

**Interfaces:**
- Consumes: a generated `.docx`; PowerShell's `[xml]` parser and
  `System.IO.Compression.ZipFile`.
- Produces: an independent check that the XML is well-formed and the ZIP
  is standard.

- [ ] **Step 1: Write the script**

`scripts/verify-docx.ps1`:

```powershell
# Verifies a .docx with tools that are not MarkPop's own:
# the .NET XML parser and the .NET ZIP reader.
param([Parameter(Mandatory = $true)][string]$Path)

$ErrorActionPreference = "Stop"

$zip = [System.IO.Compression.ZipFile]::OpenRead($Path)
try {
    $names = $zip.Entries | ForEach-Object { $_.FullName }
    Write-Output "ENTRIES=$($names -join ',')"

    foreach ($entry in $zip.Entries) {
        if ($entry.FullName -like "*.xml" -or $entry.FullName -like "*.rels") {
            $reader = New-Object System.IO.StreamReader($entry.Open())
            $text = $reader.ReadToEnd()
            $reader.Close()
            # Throws when the XML is not well-formed.
            $null = [xml]$text
            Write-Output "XML_OK=$($entry.FullName)"
        }
    }
} finally {
    $zip.Dispose()
}
```

- [ ] **Step 2: Write the test**

`tests/docx_external.rs`:

```rust
//! Checks the generated package with independent tools.
//!
//! MarkPop's own reader and Microsoft's Word are two witnesses; the .NET
//! XML parser and ZIP reader are two more, and they are not written by
//! us.

use std::process::Command;

#[test]
fn the_package_is_well_formed_for_dotnet() {
    if Command::new("powershell").args(["-NoProfile", "-Command", "exit 0"]).status().is_err() {
        eprintln!("skipping: no PowerShell");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("markpop-external-{}.docx", std::process::id()));
    let bytes = markpop::render_docx(
        "# Title\n\nText with a [link](https://example.com).\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        &markpop::Options::default(),
    )
    .expect("render");
    std::fs::write(&path, bytes).expect("write");

    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-docx.ps1");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(&path)
        .output()
        .expect("run powershell");
    let _ = std::fs::remove_file(&path);

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "verification failed: {stdout}{}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("XML_OK=word/document.xml"), "{stdout}");
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test --test docx_external`
Expected: PASS — every XML part parses and the ZIP opens.

- [ ] **Step 4: Commit**

```bash
git add scripts/verify-docx.ps1 tests/docx_external.rs
git commit -m "test(docx): verify the package with independent XML and ZIP tools"
```

---

### Task 13: Document the format and check coverage

**Files:**
- Modify: `README.md`

**Interfaces:**
- Consumes: the finished feature.
- Produces: the README's output-format table, usage and testing sections.

- [ ] **Step 1: Update the README**

Add a row to the output-format table:

```markdown
| DOCX | `src/docx` | A Microsoft Word document you can edit and track changes in |
```

Document the exceptions in a short subsection under the table:

```markdown
### Word documents

`--format docx` writes a real Word document: native built-in styles
(`Heading1`…`Heading6`, `Quote`, `ListParagraph`, `HTMLPreformatted`,
`HTMLCode`), real tables, real list numbering, embedded images and a TOC
field that Word refreshes on open.

Two things do not carry over, because they belong to the picture and not
to the document: the canvas (Word paginates the flow itself, on A4), and
the theme (the document uses Word's own styles, so it looks like a file
authored in Word).
```

Update the testing section with the new suites.

- [ ] **Step 2: Run the full verification**

Run: `cargo test && cargo clippy --all-targets && cargo llvm-cov --summary-only`
Expected: all green; the new module's line coverage is at or above the
crate's 94 % standard.

- [ ] **Step 3: Generate an example and look at it**

Run: `cargo run -- example.md --format docx -o /tmp/example.docx` then open
`/tmp/example.docx` in Word and confirm the structure.

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document the Word output format"
```
