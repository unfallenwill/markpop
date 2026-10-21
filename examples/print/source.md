# Deterministic Raster Output

**A note on reproducing bitmaps exactly**

## 1. Introduction

A rasterizer that produces a different image on every machine is not a rasterizer, it is a
lottery. This document sets out what it takes to render the same pixels everywhere: a
font whose outlines are read from the file rather than guessed, a metric table that does
not depend on what happens to be installed, and an encoder that never consults the
operating system.

## 2. The problem of hinted outlines

TrueType ships coordinates that describe the shape at the design size. Renderers usually
apply grid fitting at small sizes to keep stems aligned to the pixel grid. Grid fitting is
a rounding decision, and rounding decisions differ between implementations.

The rasterizer in this project deliberately does not apply grid fitting. Outlines are
flattened as they are stored and filled directly. The consequence is that a glyph may not
look quite as crisp at 12 px as it would in a system renderer. The other consequence, which
matters far more, is that it looks the same everywhere.

### 2.1 Synthetic bold

When a family ships only one weight, a synthetic bold is produced by drawing the outline a
second time, offset by a fraction of the stem width. Real bold faces are always preferred
when the system provides them.

### 2.2 Synthetic italic

Italic is produced by shearing the outline horizontally by twelve degrees. As with bold,
a real italic face is preferred and the synthetic form is only a fallback.

## 3. Colour fonts

Colour requires a font that carries a palette. The COLR table describes a paint graph:
layers, glyph references, transforms, composites, and gradient brushes. Each is resolved
during rasterization, in document order, into a single flat bitmap.

| Table | Purpose | Supported |
|---|---|---|
| `COLR` | Paint graph for colour glyphs | yes |
| `CPAL` | Palette of colours referenced by `COLR` | yes |
| `CBDT` | Bitmap colour strikes | monochrome only |
| `sbix` | Bitmap strikes | monochrome only |

Variable fields inside a `COLR` paint graph are resolved against the axis coordinates of
the variable font instance in use.

## 4. Encoding

The PNG encoder performs adaptive per-row filtering and writes its own deflate stream with
fixed Huffman codes. The WebP encoder writes a lossless VP8L stream with canonical prefix
codes and an LZ77 token stream. Neither encoder calls into a system library.

The practical consequence is that the two decoders agree byte for byte:

> 8,092,800 bytes compared across a full rendered page, zero differing pixels.

## 5. Verification

The test suite builds fonts byte by byte inside the test process, so the fixtures do not
depend on any font file being installed. It then renders, decodes with a decoder written
for the purpose, and compares.

- [x] Chunk CRCs validated
- [x] Adaptive filter round-trip
- [x] Exact pixel recovery with an independent inflater
- [x] VP8L canonical prefix codes verified against a Kraft sum
- [x] LZ77 token replay and distance-range invariants
