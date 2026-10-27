# MarkPop DOCX output — design

**Date:** 2026-10-27
**Status:** approved for implementation
**Scope:** Markdown → Word (`.docx`) as a new output format of MarkPop.

## Goal

MarkPop renders Markdown to SVG, PNG and lossless WebP. This adds a fourth
output format: a real Microsoft Word document, written as Office Open XML
(OOXML) inside a ZIP container, with no new dependencies.

```
markpop README.md --format docx -o README.docx
```

The resulting file must open in Microsoft Word as an ordinary, editable
document — native built-in styles, real tables, real list numbering, real
hyperlinks, embedded images — not a picture of a document.

## Decisions

| Question | Decision |
|---|---|
| Direction | Output. Markdown → DOCX. Reading `.docx` is out of scope. |
| Formatting model | Native Word styles. The document uses Word's built-in style IDs and Word's own default look (Calibri body, `#2F5496` headings). MarkPop's `Theme` is not consulted for styling. |
| Tables | Real `w:tbl` elements with the `TableGrid` style. |
| Images | Embedded as real image parts, drawn inline. |
| Lists | Real Word numbering (`numbering.xml`), including nesting and task items. |
| Table of contents | A real `TOC` field that Word builds and refreshes itself. |
| Dependencies | None. The crate stays dependency-free. |

## Non-goals

- Reading `.docx` (input direction), legacy `.doc`, RTF or OLE formats.
- Embedded fonts, charts, content controls, comments, tracked changes,
  footnotes, endnotes, or embedded OLE objects.
- A Word theme part (`word/theme/theme1.xml`). Styles carry Word's default
  look explicitly instead, so the file needs no theme resolution.
- `docProps/core.xml` and `docProps/app.xml`. They would require fabricating
  a creation date, and this crate prizes byte-for-byte determinism.
- Round-tripping a document back to Markdown.

## Architecture

The DOCX writer is a new subsystem alongside `parse` → `layout` → `engine`.
It reads the Markdown AST (`src/ast.rs`) — never the laid-out `Scene` — and
writes OOXML parts, which a ZIP writer packages.

```
src/docx/mod.rs      public API and part assembly
src/docx/model.rs    typed Word model: Body, Paragraph, Run, Table, Image, …
src/docx/content.rs  Markdown AST → Word model (the semantic mapping)
src/docx/write.rs    Word model → OOXML parts
src/docx/xml.rs      minimal XML writer
src/docx/media.rs    image embedding: decode → PNG bytes, EMU sizing
src/docx/zip.rs      ZIP container writer
```

Each unit has one job and is testable on its own:

- `xml.rs` knows nothing about Word.
- `zip.rs` knows nothing about Word.
- `media.rs` knows nothing about XML.
- `content.rs` knows the Markdown AST and the Word model, but not XML syntax.
- `write.rs` knows the Word model and XML, but not Markdown.

### Reuse of existing code

- `src/encode/deflate.rs::deflate` already emits a **raw** DEFLATE stream
  (no zlib wrapper), which is exactly ZIP compression method 8.
- `src/encode/png.rs::Crc32` already computes CRC-32 (IEEE). It moves to
  `src/encode/crc32.rs` so PNG and ZIP share one implementation.
- `src/decode/mod.rs::decode` already decodes PNG and BMP to straight-alpha
  RGBA8.
- `src/encode/png.rs::encode` already writes a PNG from RGBA8.
- `src/html.rs::parse_html` already parses raw HTML into a node tree, which
  the DOCX writer reuses to map inline HTML to Word runs.

### Why DOCX bypasses the `Engine` trait

`engine::Engine::encode` receives a `Scene` — laid-out geometry. A Word
document is not geometry; it is structure. By the time a scene exists, the
distinction between a heading and a paragraph is only a font size. So
`render_bytes` dispatches DOCX to `docx::render_bytes(markdown, options)`
before the engine path is taken.

`OutputFormat` gains a `Docx` variant with `extension() == "docx"` and
`mime() == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"`.
`is_raster()` stays false.

## OOXML package layout

```
[Content_Types].xml
_rels/.rels
word/document.xml
word/_rels/document.xml.rels
word/styles.xml
word/numbering.xml
word/settings.xml
word/media/image1.png
word/media/image2.png
…
```

### `[Content_Types].xml`

`Default` entries for `rels`, `xml` and `png`; `Override` entries for
`/word/document.xml`, `/word/styles.xml`, `/word/numbering.xml` and
`/word/settings.xml` with their WordprocessingML content types. Every part
in the package must be covered by a `Default` (by extension) or an
`Override` (by part name) — a missing entry is what makes Word offer to
"repair" a file.

### `_rels/.rels`

One relationship: `rId1` → `word/document.xml`, type
`…/officeDocument/2006/relationships/officeDocument`.

### `word/_rels/document.xml.rels`

- `rId1` → `styles.xml` (type `…/styles`)
- `rId2` → `numbering.xml` (type `…/numbering`)
- `rId3` → `settings.xml` (type `…/settings`)
- `rId4…` → external hyperlinks (type `…/hyperlink`, `TargetMode="External"`)
- `rId…` → `media/imageN.png` (type `…/image`)

Relationship IDs are allocated in document order and are unique across the
part. Hyperlink targets are deduplicated: the same URL reuses one
relationship.

### `word/document.xml`

Root element `w:document` with the namespaces `w`, `r`, `wp`, `a` and `pic`.
The body is a sequence of block elements followed by exactly one `w:sectPr`.

### `word/styles.xml`

`w:docDefaults` (Calibri 11 pt, 8 pt space after, 1.08 line spacing) plus
these built-in style IDs:

| Style ID | Type | Purpose |
|---|---|---|
| `Normal` | paragraph | body text, marked `w:default="1"` |
| `Heading1` … `Heading6` | paragraph | headings, each with `w:outlineLvl` 0…5 |
| `Quote` | paragraph | block quotes |
| `ListParagraph` | paragraph | list items |
| `HTMLPreformatted` | paragraph | code blocks ("HTML Preformatted") |
| `HTMLCode` | character | inline code ("HTML Code") |
| `Hyperlink` | character | link runs |
| `TableGrid` | table | table borders |
| `TOCHeading` | paragraph | the "Contents" heading |
| `TOC1`, `TOC2`, `TOC3` | paragraph | TOC entry levels |

Heading colours are `#2F5496` for levels 1–2 and `#1F3763` for levels 3–6,
sizes 16/13/12/11/11/11 pt, which is what Word itself writes for a new
document.

### `word/numbering.xml`

Two abstract numbering definitions:

- **Bullet** (`multiLevelType = hybridMultilevel`), nine levels using Word's
  default pattern: level 0 `•` in Symbol, level 1 `o` in Courier New,
  level 2 `▪` in Wingdings, repeating.
- **Decimal** (`hybridMultilevel`), nine levels with `lvlText` `%1.`,
  `%1.%2.`, … and left indents of 720 twips per level.

Concrete instances are `w:num` elements pointing at an abstract definition.
Every ordered list gets its **own** `w:num` with a `startOverride` so that
each list restarts at its own number (GFM allows a list to start at any
number, e.g. `3.`).

### `word/settings.xml`

`<w:defaultTabStop w:val="720"/>` always. `<w:updateFields w:val="true"/>`
is added only when the document contains a TOC field, so Word refreshes it
on open. Without a TOC, the setting is omitted so no unrelated prompt
appears.

### Page setup

`w:sectPr` sets A4 portrait — `w:pgSz w:w="11906" w:h="16838"` — with
1440-twip (1 inch) margins on all four sides, giving 9026 twips of content
width. MarkPop's canvas has no page equivalent, so the canvas presets do
not affect DOCX output.

## Markdown → Word mapping

| Markdown | Word construct |
|---|---|
| `# H1` … `###### H6` | paragraph with `Heading1`…`Heading6` |
| paragraph | paragraph with `Normal` |
| `**bold**` | run property `<w:b/>` |
| `*italic*` | run property `<w:i/>` |
| `~~strike~~` | run property `<w:strike/>` |
| `<u>`, `<ins>` | run property `<w:u w:val="single"/>` |
| `` `code` `` | run with character style `HTMLCode` |
| `[text](url)` | `w:hyperlink` run + external relationship |
| `![alt](path)` | `w:drawing` inline picture + embedded PNG |
| `---` | paragraph with a bottom border |
| `> quote` | paragraph with `Quote`; deeper levels add 720 twips of indent each |
| fenced code block | one `HTMLPreformatted` paragraph per line |
| `- item`, `1. item` | `ListParagraph` + `w:numPr` |
| nested list | `w:ilvl` = nesting depth (clamped to 9 levels) |
| `- [ ]`, `- [x]` | `ListParagraph` with a `☐` / `☒` prefix (Word has no native checkbox list) |
| GFM table | `w:tbl` with `TableGrid`, bold header row marked `w:tblHeader`, column widths equal, cell alignment from the GFM align row |
| raw inline HTML | tag-mapped runs: `b`/`strong` → bold, `i`/`em` → italic, `u`/`ins` → underline, `s`/`del`/`strike` → strike, `code` → `HTMLCode`, `a` → hyperlink, `br` → line break, `img` → image; other tags are transparent containers |
| raw HTML block | block-level children become paragraphs, inline children become runs |
| `--toc` | a `TOC \o "1-3" \h \z \u` field, preceded by a `TOCHeading` paragraph |

A `w:tbl` is always followed by an empty paragraph: Word requires a
paragraph after a table, and two adjacent tables would otherwise merge.

## Images

1. The path is resolved against `Options::base_dir` (the CLI sets this to
   the input file's directory).
2. The file is decoded with `decode::decode` (PNG or BMP → RGBA8).
3. It is re-encoded as an 8-bit RGBA PNG with `encode::png::encode`. Word
   does not support BMP natively, and re-encoding guarantees a PNG variant
   Word always accepts.
4. Size: natural pixels at 96 DPI (`1 px = 9525 EMU`), scaled down
   proportionally if the image exceeds the content box (5731510 × 8864330
   EMU — the 9026 × 13958 twip content area, at 635 EMU per twip).
5. A file that cannot be read or decoded degrades to a paragraph carrying
   the alt text. It never fails the render.

## Public API and CLI

```rust
pub fn render_docx(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError>
```

- `--format docx` writes exactly one file. `--page-height` is ignored:
  Word paginates the flow itself.
- `--width`, `--font-size`, `--line-height`, `--letter-spacing`,
  `--font-family`, `--mono-family`, `--columns` and `--column-gap` are
  ignored for DOCX output, because the document uses Word's native styles.
- `--toc` produces a Word TOC field rather than MarkPop's static outline.
- `--no-local-images` prevents images from being read from disk; images
  then degrade to their alt text.

No new `RenderError` variant is needed: Markdown parsing is infallible by
design, and unreadable images degrade instead of failing.

## Error handling

| Situation | Behaviour |
|---|---|
| Malformed Markdown | Parses into paragraphs, as everywhere else in MarkPop |
| Unreadable or undecodable image | Degrades to a paragraph with the alt text |
| Missing local image | Same degradation |
| A part larger than 4 GiB | The ZIP writer refuses it (ZIP64 is not implemented) |

## Testing

The goal is a suite that would catch a file Word refuses to open.

### Unit tests (in-module)

- `xml.rs`: escaping of `&`, `<`, `>`, `"`, `'`; empty elements; attribute
  quoting; nested elements.
- `zip.rs`: local header, central directory and EOCD layout; CRC-32
  values; a deflate round-trip through the crate's own inflater; stored vs
  deflated entries; empty archive; entry names with directories.
- `content.rs`: every AST block and inline variant maps to the expected
  model node, including nesting, task items, table alignment and raw HTML.
- `write.rs`: `styles.xml` defines every style the document references;
  `numbering.xml` defines every `numId` the document uses; the content
  types cover every part; relationships resolve.
- `media.rs`: BMP → PNG conversion yields a decodable PNG; oversized images
  are scaled; an undecodable file degrades.

### Integration tests (`tests/docx.rs`)

The product is unzipped by a test-only ZIP reader (central directory parse
plus the crate's inflater) and asserted for **cross-part integrity** — the
class of bug that produces a file Word offers to repair:

- every part named in `[Content_Types].xml` exists, and every part in the
  package is covered by it;
- every `r:id` / `r:embed` in `document.xml` resolves in
  `word/_rels/document.xml.rels`;
- every `w:pStyle w:val` resolves to a style defined in `styles.xml`;
- every `w:numId` resolves in `numbering.xml`;
- the embedded PNG decodes back to exactly the source pixels;
- the concatenation of all `<w:t>` text equals the document's plain text;
- the same input yields byte-identical output.

### Independent validators

PowerShell's `[xml]` parser and `System.IO.Compression` check
well-formedness and ZIP conformance with implementations that are not ours,
so a bug in our own reader cannot hide a bug in our own writer.

### Microsoft Word (`tests/docx_word.rs`)

Word 16 is available on the development machine, so the suite drives it
through COM: the generated document is opened with alerts disabled, and the
test asserts that it opens without a repair prompt and that its structure
matches — paragraph count, heading style names, table count, list count,
inline shape count, and a live TOC field. The test skips itself when Word
is not installed, so it never fails a build on another machine.

### Coverage

The new module is expected to meet the crate's existing standard of 94 %
line coverage, measured with `cargo llvm-cov`.

## Acceptance criteria

1. `markpop doc.md --format docx -o doc.docx` writes a file that Microsoft
   Word opens without a repair prompt.
2. The document contains every block and inline construct of the source
   Markdown, mapped per the table above.
3. Tables are real tables, lists are real lists, links are real hyperlinks,
   images are embedded parts, and `--toc` yields a Word TOC field.
4. Styles are Word's built-in IDs with Word's default look.
5. The crate gains no dependency.
6. The test suite covers the mapping, the package integrity, the ZIP
   container and the Word round trip, and the new module meets the crate's
   coverage standard.
