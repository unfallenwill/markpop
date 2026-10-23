# MarkPop Demo

A short document exercising the main block kinds.

## The pipeline

```
Markdown text
  → parse      (src/parse)   → Document  (src/ast.rs)
  → layout     (src/layout)  → Scene     (src/scene.rs)
  → engine     (src/engine)  → SVG | PNG | WebP
```

## Usage

```rust
let options = Options::default().with_theme(Theme::print());
let svg = markpop::render("doc.md contents", &options)?;
```

## Features

- Pure Rust, zero dependencies
- SVG, PNG and lossless WebP output
- Deterministic layout

## Table

| Format | Engine | Use    |
| ------ | ------ | ------ |
| svg    | Svg    | vector |
| png    | Bitmap | compat |

> Everything is drawn by the crate itself.

Final paragraph with **bold**, *italic* and `code`.
