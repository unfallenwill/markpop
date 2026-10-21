# `markpop::render`

Render a Markdown document to an SVG string.

## Signature

```rust
pub fn render(markdown: &str) -> String
```

Renders with the default theme (`Theme::light()`) at 100% zoom.

## Parameters

| Parameter | Type | Description |
|---|---|---|
| `markdown` | `&str` | The document source. Unrecognized input degrades to text rather than erroring. |

## Returns

The SVG document as a `String`. The markup is plain SVG 1.1: `<svg>`, `<rect>`, `<line>`
and `<text>` only — no CSS, no filters, no `<foreignObject>`, no external references.

## Example

```rust
let svg = markpop::render("# Hello\n\nWorld");
assert!(svg.starts_with("<svg"));
```

## Errors

This function does not fail. Parsing is total: every byte sequence produces a document.

## See also

- [`render_with`] — control the theme, zoom, canvas and output format
- [`to_scene`] — get geometry without serializing it

[`render_with`]: https://docs.rs/markpop
[`to_scene`]: https://docs.rs/markpop
