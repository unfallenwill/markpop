# markdown2svg

Render Markdown **directly to SVG, PNG or lossless WebP** — no HTML, no headless browser, no
screenshots, no image libraries. Pure Rust, zero dependencies.

```bash
cat README.md | markdown2svg > README.svg          # vector (default)
markdown2svg doc.md --format png  -o doc.png       # bitmap, wide compatibility
markdown2svg doc.md --format webp -o doc.webp      # bitmap, smaller files
```

## Why

The usual pipeline is `Markdown → HTML → headless Chrome → screenshot`. It is slow, heavy and
produces a raster image you cannot restyle. This crate replaces it with a single-pass native
pipeline plus a pluggable output engine:

```
Markdown text
  → parse      (src/parse)   → Document  (src/ast.rs)
  → layout     (src/layout)  → Scene     (src/scene.rs)
  → engine     (src/engine)  → SVG | PNG | WebP
```

Consequences:

- **Three engines, one layout.** SVG is serialized directly; PNG and WebP are produced by a
  built-in software rasterizer (TrueType parser + anti-aliased scanline filler) and our own
  deflate/VP8L encoders. No browser, no system APIs, no C libraries.
- **Fast and tiny.** One binary, zero dependencies, no JS runtime.
- **Vector by default.** Text stays text — selectable, searchable, crisp at any zoom.
- **Deterministic.** Same input ⇒ same bytes, on every platform. Layout advances come from a
  built-in table instead of a system font, so a document cannot reflow because of a missing font
  (and CJK is exact: one em per glyph).
- **Self-contained.** Remote images are never fetched; they render as labeled placeholders.

## Output formats

| Format | Engine | Use when |
|---|---|---|
| `svg` (default) | `SvgEngine` — direct serializer | You want vectors: crisp at any zoom, selectable text, tiny files |
| `png` | `BitmapEngine` + PNG encoder | You need the widest compatibility (docs, chat, office tools) |
| `webp` | `BitmapEngine` + VP8L lossless encoder | You need a bitmap with the smallest size |

Bitmap engines parse a TrueType font at runtime (discovered from the system, or supplied with
`--font <FILE>`); SVG needs no font because the viewer resolves the family stack. For the
sample document, WebP lossless is ~38% smaller than the equivalent PNG, and both decode to
pixel-identical images.

The `Engine` trait (`src/engine.rs`) is the extension point: a new format only needs a
serializer over `Scene`, or an encoder over the raster `Bitmap`.
- **Self-contained.** Remote images are never fetched; they render as labeled placeholders.

## Install

```bash
cargo install --path .
```

## CLI

```
USAGE:
    markdown2svg [OPTIONS] [INPUT]

ARGS:
    <INPUT>    Markdown file to render. Omit or use `-` to read stdin.

OPTIONS:
    -o, --output <FILE>       Write the output to FILE instead of stdout
    -f, --format <NAME>       Output format: svg (default), png or webp
    -z, --zoom <PERCENT>      Output scale as a percentage (default: 100)
        --theme <NAME>        Color theme: light (default), dark or print
        --canvas <NAME>       Canvas: web (default), a4, a4-large or mobile
        --width <PX>          Canvas width in pixels (default: 800)
        --font-size <PX>      Base font size in pixels (default: 16)
        --line-height <MULT>  Line height multiplier (default: 1.6)
        --letter-spacing <EM> Extra tracking in em units (default: 0)
        --font-family <STACK> Font stack for body text (default: system sans)
        --mono-family <STACK> Font stack for code (default: system mono)
        --columns <N>         Text columns per page (default: 1)
        --column-gap <PX>     Gap between columns (default: 24)
        --page-height <PX>    Split into pages of this height; writes one
                              file per page (requires --output)
        --font <FILE>         TrueType font used for png/webp output
        --toc                 Prepend a table of contents
    -h, --help                Print this help
    -V, --version             Print version information
```

Examples:

```bash
markdown2svg doc.md -o doc.svg
markdown2svg doc.md --canvas a4 -o page.svg                          # A4, auto-paginated
markdown2svg doc.md --canvas mobile -o long.svg                     # 750px long image
markdown2svg doc.md --theme print --page-height 1123 -o page.svg   # page-1.svg, page-2.svg, …
markdown2svg doc.md --format webp --font myfont.ttf -o doc.webp
markdown2svg doc.md --columns 2 --column-gap 28 -o doc.svg          # balanced columns
markdown2svg doc.md --columns 3 --page-height 1123 -o page.svg      # 3 columns per page
markdown2svg doc.md --font-size 17 --line-height 1.75 --letter-spacing 0.01 \
    --font-family "Georgia, serif" -o doc.svg
cat doc.md | markdown2svg --theme dark --zoom 150 > doc.svg
```

## Canvas presets

| Preset | Size (base units) | Padding | Pagination |
|---|---|---|---|
| `web` (default) | 800 × auto | 48 | continuous long image |
| `a4` | 794 × 1123 (210 × 297 mm @ 96 dpi) | 56 | automatic, one file per page |
| `a4-large` | 1191 × 1684 (1.5×) | 84 | automatic, one file per page |
| `mobile` | 750 × auto, 24 px body | 32 | continuous long image |

```bash
markdown2svg doc.md --canvas a4 -o page.svg      # → page-1.svg, page-2.svg, …
markdown2svg doc.md --canvas a4 --format png -o page.png   # → page-1.png, …
markdown2svg doc.md --canvas mobile -o long.svg  # one tall image for phone reading
```

```rust
use markdown2svg::{CanvasPreset, Options};

let opts = Options::default().with_canvas(CanvasPreset::A4);
assert_eq!(opts.page_height, Some(1123.0));
```

A preset is applied first; explicit flags still win (`--canvas a4 --width 900`
keeps A4 pagination but a 900 px canvas).

### No clipping on paginated canvases

Before layout, blocks that are taller than the available space are split so
page cuts never truncate content:

- **code blocks** split at source-line boundaries, repeating the language label;
- **lists** split between items, with ordered numbering continuing across pages;
- **blockquotes** split between child blocks, each piece keeping its bar;
- **tables** split between rows, repeating the header row in every piece.

Everything else is placed whole between block boundaries. The only remaining
case is a single paragraph or figure taller than one page (thousands of
characters in one paragraph), which cannot be cut without re-flowing text.

## Typography configuration

The layout engine is driven entirely by configuration; nothing is hard-coded.

| Knob | CLI | API (`Theme`) | Default |
|---|---|---|---|
| Font size | `--font-size 18` | `with_font_size(18.0)` | 16 |
| Line height | `--line-height 1.8` | `with_line_height(1.8)` | 1.6 |
| Letter spacing | `--letter-spacing 0.02` | `with_letter_spacing(0.02)` | 0 |
| Font family | `--font-family "Georgia, serif"` | `with_font_family("Georgia, serif")` | system sans |
| Code font | `--mono-family "Consolas, monospace"` | `with_mono_family(...)` | system mono |
| Columns | `--columns 2` | `with_columns(2)` | 1 |
| Column gap | `--column-gap 28` | `with_column_gap(28.0)` | 24 |

```rust
let theme = markdown2svg::Theme::light()
    .with_font_size(17.0)
    .with_line_height(1.75)
    .with_letter_spacing(0.01)
    .with_font_family("Georgia, 'Times New Roman', serif")
    .with_columns(2);
let svg = markdown2svg::render_with("# Hi", &markdown2svg::Options::default().with_theme(theme))?;
```

Notes:

- Layout is measured with the theme's own metrics, so changing these values
  changes line breaks predictably. `--font-family` is what SVG viewers resolve;
  bitmap output uses the closest discovered TrueType face (or `--font`).
- For bitmap output the family name is also used to prefer a matching face
  among the discovered fonts.
- Columns are either **balanced** (no page height: the shortest height that fits
  the requested number of columns) or **paginated** (a page height fills each
  column; every N columns starts a new page).
- Cuts happen between blocks; headings stay with the block they introduce, and
  code blocks taller than a column are split between lines.
- A single block that cannot fit (e.g. a very wide table) is placed alone and
  is allowed to overflow, rather than overlapping its neighbour.

## Library

```rust
// Defaults: light theme, 100% zoom.
let svg: String = markdown2svg::render("# Hello\n\nWorld");

// Custom options, including zoom (1.0 = 100%).
let opts = markdown2svg::Options::default()
    .with_theme(markdown2svg::Theme::dark())
    .with_zoom(1.5);
let svg = markdown2svg::render_with("# Hi", &opts)?;

// Geometry without serialization: useful for tests or other output formats.
let scene = markdown2svg::to_scene("# Hi", &markdown2svg::Theme::light());
assert!(scene.find_text("Hi").is_some());

// Bitmap engines: PNG or lossless WebP, from the same layout.
use markdown2svg::OutputFormat;
let opts = markdown2svg::Options::default().with_format(OutputFormat::Webp);
let bytes: Vec<u8> = markdown2svg::render_bytes("# Hi", &opts)?;

// Long documents: pagination + table of contents.
let opts = markdown2svg::Options::default()
    .with_page_height(1123.0)
    .with_toc(true);
let pages: Vec<String> = markdown2svg::render_pages("# One\n\nBody", &opts)?;
```

### Zoom

Zoom is implemented with the SVG `viewBox`: geometry is written in base units and only the root
`width`/`height` scale. Nothing is re-laid-out, so 25% and 400% are pixel-identical after scaling
and the output stays vector-crisp.

```rust
use markdown2svg::{Color, Options, Theme};

let branding = Theme { link: "#ff5500".parse()?, ..Theme::light() };
let opts = Options::default().with_theme(branding).with_zoom(1.5);
```

### Themes

Three presets ship built in, and every visual constant is a field on `Theme`:

| Preset | Character |
|---|---|
| `Theme::light()` | Sans-serif, green headings, blue links (default) |
| `Theme::dark()` | Dark canvas, soft syntax colors |
| `Theme::print()` | Serif body, warm paper, loose leading, subtle tracking |

```rust
let print = markdown2svg::Theme {
    line_height: 1.9,
    letter_spacing: 0.01,
    ..markdown2svg::Theme::print()
};
```

Geometry is theme-independent: changing colors never moves text. Metrics (font size, line
height, tracking) are part of the theme, so a design change re-flows predictably.

### Long documents

`--page-height` (or `Options::page_height`) splits the layout into fixed-height pages:

- cuts only ever happen **between top-level blocks** — paragraphs, tables and code boxes are
  never sliced in half;
- code blocks taller than a page are split **between lines**, repeating the language label;
- headings are kept with the block they introduce (no stranded headings at a page foot);
- pages are written as `name-1.svg`, `name-2.svg`, … and each one is a self-contained SVG.

Limitation: a non-code block taller than a page (e.g. a 100-row table) is placed alone on its
page and may overflow. Split such tables in the source or raise `--page-height`.

`--toc` prepends a nested outline of the document's h1–h3 headings.


## Supported syntax

| Construct | Notes |
|---|---|
| Headings | ATX (`#`) and Setext (`===`, `---`); h1/h2 get a rule |
| Emphasis | `*em*`, `**strong**`, `***both***`, `_em_`, `__strong__` |
| Strikethrough | GFM `~~text~~` |
| Inline code | Background chip, monospace, unbreakable when it fits |
| Code blocks | Fenced (with language label) and indented; wrapped, indentation preserved |
| Links | Inline `[text](url "title")`, `<autolinks>`, angle destinations |
| Images | Rendered as a placeholder figure with the URL as caption |
| Blockquotes | Arbitrarily nested; deeper levels fade and tighten |
| Lists | Ordered/unordered, tight/loose, nested, lazy continuation |
| Tables | GFM pipe tables with `:---`, `:---:`, `---:` alignment; columns auto-fit |
| Thematic breaks | `---`, `***`, `___` |
| Escapes & entities | `\*`, `&amp;`, `&#65;`, `&#x41;` |
| CJK | Full-width glyphs, per-character wrapping, mixed CJK/Latin |
| Pagination | Opt-in fixed-height pages, block-aligned cuts |
| Table of contents | Opt-in nested outline from headings |

Not supported (by design, for now): reference links, raw HTML, task lists, footnotes, syntax
highlighting. Unrecognized input degrades to text rather than erroring — parsing is total.

### Fitting and overflow

- **Text** wraps against measured advances; unbreakable tokens (long URLs, identifiers) are
  split by character rather than allowed to escape the canvas; CJK wraps per character.
- **Code** wraps with preformatted rules, so leading indentation and column alignment survive.
- **Tables** start at their natural width and shrink proportionally above a per-column minimum,
  guaranteeing `sum(columns) <= content width` for any column count.
- Wrapping keeps a 2% safety margin. Measured against Chromium, Latin runs render at
  0.976–1.001× the predicted advance (mean 0.978), so the margin errs on the safe side only.

## Architecture

Every stage is a pure function with its own tests:

| Stage | Input → Output | Tested by asserting on |
|---|---|---|
| `parse` | `&str` → `Document` | AST shape |
| `outline` | `Document` → `Document` | TOC nesting |
| `layout` | `Document` + `Theme` → `Scene` | primitives and coordinates |
| `paginate` | `Scene` + block marks → `Vec<Scene>` | pages, no block cuts |
| `svg` | `Scene` + zoom → `String` | markup |
| `raster` | `Scene` + fonts + zoom → `Bitmap` | pixels, glyph ink, clipping |
| `encode` | `Bitmap` → PNG / WebP bytes | headers, CRCs, pixel round-trip |
| `engine` | `Scene` → bytes | format dispatch |

```
src/
  ast.rs          document model (blocks, inlines) + flattening helpers
  parse/
    mod.rs        entry point, line normalization, tab expansion
    block.rs      containers: headings, lists, quotes, tables, fences
    inline.rs     emphasis resolution, links, code spans, escapes
  text.rs         deterministic advance table + greedy line breaking
  layout.rs       flow engine: margins, atoms, wrapping, decorations
  columns.rs      multi-column flow (balanced or paginated)
  split.rs        splits oversized blocks (code/lists/quotes/tables)
  canvas.rs       canvas presets: web, A4, A4-large, mobile
  paginate.rs     block-aligned page splitting + code chunking
  outline.rs      headings → table of contents
  scene.rs        geometry primitives (text/rect/line) + Scene
  svg.rs          XML serialization, escaping, zoom
  engine.rs       Engine trait + SvgEngine / BitmapEngine factory
  font.rs         TrueType reader: cmap, loca/glyf, composite glyphs
  raster.rs       anti-aliased scanline filler + glyph compositing
  encode/
    deflate.rs    LZ77 + fixed-Huffman deflate, zlib, CRC/Adler
    png.rs        adaptive filtering + chunk writer
    webp.rs       VP8L: Huffman codes, LZ77 tokens, RIFF container
  theme.rs        all visual constants, as plain data
  options.rs      zoom, format, pagination, TOC, validation, errors
```

### Extending

- **New look** — `Theme` is plain data: start from `light()`/`dark()`/`print()` and override.
- **New syntax** — add a variant to `ast::Block`/`ast::Inline`, emit it from `parse`, and handle
  it in `layout`. Both matches are exhaustive, so the compiler lists the work.
- **New output format** — implement `engine::Engine`; it only needs `Scene` (vector) or
  `raster::Bitmap` (bitmap).
- **Better metrics** — `text::Metrics` is the only place that decides how wide a character is.

## Testing

```bash
cargo test                 # unit, integration, golden and doctests
cargo clippy --all-targets
cargo llvm-cov --summary-only
```

The suite covers each stage in isolation (parser fixtures, layout coordinates, serializer markup,
font parsing, rasterizer pixels, encoder containers) plus end-to-end properties:

- SVG well-formedness (also validated with an external XML parser)
- zoom invariance of geometry, byte-for-byte determinism (golden file)
- no-overflow guarantees for CJK, emoji, deep nesting and unbreakable tokens
- pagination: fixed page heights, whole-block placement, no stranded headings, no lost code lines
- PNG: chunk CRCs, adaptive-filter round-trip, exact pixel recovery (own inflater)
- WebP: RIFF/VP8L headers, canonical prefix codes, full-tree Kraft check, LZ77 token replay,
  distance-range invariants, and a built-in VP8L decoder that round-trips a full rendered page
- PNG vs WebP decode to **pixel-identical** images (verified in Chromium: 8,092,800 bytes
  compared, 0 differences)
- 300 pseudo-random inputs that must never panic or emit broken markup

Current coverage: **96% of lines** across 458 tests (`cargo llvm-cov`).

## SVG compatibility

The output is deliberately plain SVG 1.1: `<svg>`, `<rect>`, `<line>` and `<text>` only — no CSS,
no filters, no `<foreignObject>`, no external references. Text is real text (selectable and
searchable), decimals are rounded to two places, and every text node is XML-escaped.

It renders directly in browsers (verified in Chromium), can be embedded with `<img>`, and
converts cleanly to PNG/PDF with the usual tooling. All generated files (samples, golden fixture,
paginated pages) parse with an independent XML parser, and a real-browser measurement of every
text run in `demo.svg` put the widest right edge at 739.8px inside the 752px content column.

Underlines and strikethroughs are drawn as explicit `<line>` primitives instead of
`text-decoration`, so every renderer places them identically.

```bash
# Rasterize with a browser, if you need a PNG from SVG:
msedge --headless --screenshot=out.png --window-size=800,1200 out.svg
```

## Raster output notes

- PNG uses truecolor+alpha with adaptive per-row filters and our own fixed-Huffman deflate.
- WebP is **lossless** (VP8L): literal + LZ77 tokens, length-limited canonical Huffman codes,
  no transforms and no color cache (the simplest valid encoder).
- Both come from the same inline rasterizer, so their decoders agree byte for byte.

### Font fidelity

The rasterizer follows the font's own design rather than approximating it:

- **Unhinted outlines** are flattened and filled directly; no grid fitting distorts the shapes.
- **Real bold and italic faces** are used when the system provides them (`segoeuib.ttf`,
  `consolai.ttf`, …). Faux bold (a sub-pixel second pass) and faux italic (a shear) are only
  fallbacks for families that ship a single style.
- **Colour emoji** use the font's own `COLR` layer lists and `CPAL` palette (Segoe UI Emoji,
  Noto COLR builds): every layer is the designed outline filled with the designed colour.
  No tinting or re-colouring is synthesised.
- Glyphs are positioned with the font's own advances, like a browser does for SVG; line
  breaking uses the deterministic metric table so output stays stable.

Bitmap output needs a TrueType font. Discovery covers common Windows/macOS/Linux locations;
pass `--font <FILE>` to pin one. Colour is currently limited to `COLR` v0 layers: a COLRv1
font renders its backward-compatible flat layers (exact outlines, flat palette fills, no
gradients), and bitmap-only colour formats (`CBDT`, `sbix`) render monochrome.

## Sample

`demo.md` is rendered by the crate itself with every engine:

- `demo.svg` — light theme, 100%
- `demo-dark.svg` — dark theme, 150%
- `demo-print.svg` — print theme (serif, paper tone)
- `demo.png` — same document rasterized (800 × 2648, RGBA)
- `demo.webp` — lossless WebP of the same pixels, ~38% smaller
- `demo-2col.svg` — the same document balanced into two text columns
- `demo-mobile.svg` — phone-width long image (750 px, larger body text)
- `demo-a4-1.svg` — page 1 of the A4 render (794 × 1123, auto-paginated)

## License

MIT OR Apache-2.0
