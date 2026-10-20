# markdown2svg

Render Markdown **directly to SVG** — no HTML, no headless browser, no screenshots, no font
files, no dependencies.

```bash
cat README.md | markdown2svg > README.svg
markdown2svg doc.md --theme dark --zoom 150 -o doc.svg
```

## Why

The usual pipeline is `Markdown → HTML → headless Chrome → screenshot`. It is slow, heavy and
produces a raster image. This crate replaces it with a single-pass native pipeline:

```
Markdown text
  → parse      (src/parse)   → Document  (src/ast.rs)
  → layout     (src/layout)  → Scene     (src/scene.rs)
  → serialize  (src/svg)     → SVG string
```

Consequences:

- **Fast and tiny.** One binary, zero dependencies, no JS runtime.
- **Vector output.** Text stays text — selectable, searchable, crisp at any zoom.
- **Deterministic.** Same input ⇒ same bytes, on every platform. Glyph advances come from a
  built-in table instead of a system font, so a document cannot reflow because of a missing
  font (and CJK is exact: one em per glyph).
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
    -o, --output <FILE>       Write SVG to FILE instead of stdout
    -z, --zoom <PERCENT>      Output scale as a percentage (default: 100)
        --theme <NAME>        Color theme: light (default), dark or print
        --width <PX>          Canvas width in pixels (default: 800)
        --font-size <PX>      Base font size in pixels (default: 16)
        --line-height <MULT>  Line height multiplier (default: 1.6)
        --letter-spacing <EM> Extra tracking in em units (default: 0)
        --page-height <PX>    Split into pages of this height; writes one
                              file per page (requires --output)
        --toc                 Prepend a table of contents
    -h, --help                Print this help
    -V, --version             Print version information
```

Examples:

```bash
markdown2svg doc.md -o doc.svg
markdown2svg doc.md --theme print --page-height 1123 -o page.svg   # page-1.svg, page-2.svg, …
markdown2svg doc.md --font-size 17 --line-height 1.75 --toc -o doc.svg
cat doc.md | markdown2svg --theme dark --zoom 150 > doc.svg
```

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

```
src/
  ast.rs          document model (blocks, inlines) + flattening helpers
  parse/
    mod.rs        entry point, line normalization, tab expansion
    block.rs      containers: headings, lists, quotes, tables, fences
    inline.rs     emphasis resolution, links, code spans, escapes
  text.rs         deterministic advance table + greedy line breaking
  layout.rs       flow engine: margins, atoms, wrapping, decorations
  paginate.rs     block-aligned page splitting + code chunking
  outline.rs      headings → table of contents
  scene.rs        geometry primitives (text/rect/line) + Scene
  svg.rs          XML serialization, escaping, zoom
  theme.rs        all visual constants, as plain data
  options.rs      zoom, pagination, TOC, validation, error type
```

### Extending

- **New look** — `Theme` is plain data: start from `light()`/`dark()`/`print()` and override.
- **New syntax** — add a variant to `ast::Block`/`ast::Inline`, emit it from `parse`, and handle
  it in `layout`. Both matches are exhaustive, so the compiler lists the work.
- **New output format** — `Scene` is pure geometry; write another serializer beside `svg`.
- **Better metrics** — `text::Metrics` is the only place that decides how wide a character is.

## Testing

```bash
cargo test                 # unit, integration, golden and doctests
cargo clippy --all-targets
cargo llvm-cov --summary-only
```

The suite covers each stage in isolation (parser fixtures, layout coordinates, serializer markup)
plus end-to-end properties:

- SVG well-formedness (also validated with an external XML parser)
- zoom invariance of geometry, byte-for-byte determinism (golden file)
- no-overflow guarantees for CJK, emoji, deep nesting and unbreakable tokens
- pagination: fixed page heights, whole-block placement, no stranded headings, no lost code lines
- 300 pseudo-random inputs that must never panic or emit broken markup

Current coverage: **97% of lines** across 311 tests (`cargo llvm-cov`).

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
# Rasterize with a browser, if you need a PNG:
msedge --headless --screenshot=out.png --window-size=800,1200 out.svg
```

## Sample

`demo.md` is rendered by the crate itself:

- `demo.svg` — light theme, 100%
- `demo-dark.svg` — dark theme, 150%
- `demo-print.svg` — print theme (serif, paper tone)

## License

MIT OR Apache-2.0
