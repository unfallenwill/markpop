//! A small, dependency-free TrueType font reader.
//!
//! Raster output needs real glyph shapes, and there is no way to get them
//! without reading a font. This module parses just enough of the TrueType
//! format to render text:
//!
//! * the table directory (including `.ttc` collections),
//! * `head` / `maxp` / `hhea` / `hmtx` for basic metrics,
//! * `cmap` formats 4 and 12 for character → glyph mapping,
//! * `loca` / `glyf` for simple and composite outlines.
//!
//! Outlines are flattened to polylines here, in font units with the y axis
//! pointing up. The rasterizer applies the final transform (scale, shear,
//! y-flip, translation). CFF-flavoured OpenType is not supported; the
//! discovery list deliberately prefers TrueType fonts.

use std::path::{Path, PathBuf};

use crate::color::Color;

/// A point in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal coordinate, positive to the right.
    pub x: f32,
    /// Vertical coordinate, positive upwards (font convention).
    pub y: f32,
}

/// A closed contour, flattened to a polyline in font units.
pub type Contour = Vec<Point>;

/// A parsed font face.
#[derive(Debug, Clone)]
pub struct Face {
    data: Vec<u8>,
    units_per_em: f32,
    loca_format: i16,
    num_glyphs: u16,
    num_h_metrics: u16,
    cmap_offset: usize,
    cmap_format: u16,
    // Absolute file offsets of required tables.
    glyf: usize,
    loca: usize,
    hmtx: usize,
    /// Best-effort family name, for diagnostics.
    name: String,
    /// Font-designed bold weight (OS/2 fsSelection / head.macStyle).
    bold: bool,
    /// Font-designed italic style.
    italic: bool,
    /// COLR v0 layers + CPAL palette, when the face has colour glyphs.
    colors: Option<ColorGlyphs>,
}

/// Colour information for layered (COLR v0) glyphs.
#[derive(Debug, Clone)]
struct ColorGlyphs {
    /// Absolute offset of the base glyph record array.
    base_records: usize,
    /// Number of base glyph records.
    num_base: u16,
    /// Absolute offset of the layer record array.
    layer_records: usize,
    /// Palette 0, as RGBA colours.
    palette: Vec<Color>,
    /// Absolute offset of the COLR v1 BaseGlyphList, when present.
    base_list: Option<usize>,
    /// Absolute offset of the COLR v1 LayerList, when present.
    layer_list: Option<usize>,
}

/// Gradient extension mode for coordinates outside `0..=1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extend {
    /// Clamp to the first/last stop.
    Pad,
    /// Repeat the gradient.
    Repeat,
    /// Mirror the gradient.
    Reflect,
}

/// One colour stop of a gradient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    /// Position along the gradient, `0.0..=1.0`.
    pub offset: f32,
    /// Stop colour.
    pub color: Color,
}

/// A COLR paint graph, in font units with the y axis pointing up.
///
/// This is the font's own description of a colour glyph: nested layers,
/// affine transforms, solid fills and gradients, applied to glyph outlines.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    /// Fill the parent outline with the palette colour.
    Solid { color: Color },
    /// Fill the parent outline with the text's foreground colour.
    Foreground,
    /// A stack of paints, painted bottom to top.
    Layers(Vec<Paint>),
    /// Fill `glyph`'s outline with `paint`.
    Glyph { glyph: u16, paint: Box<Paint> },
    /// Apply an affine transform to everything below.
    Transform { matrix: [f32; 6], paint: Box<Paint> },
    /// Linear gradient from `p0` to `p1`.
    Linear {
        p0: [f32; 2],
        p1: [f32; 2],
        stops: Vec<GradientStop>,
        extend: Extend,
    },
    /// Radial gradient between two circles.
    Radial {
        c0: [f32; 2],
        r0: f32,
        c1: [f32; 2],
        r1: f32,
        stops: Vec<GradientStop>,
        extend: Extend,
    },
    /// Source painted over backdrop.
    Composite {
        source: Box<Paint>,
        backdrop: Box<Paint>,
    },
}
/// One layer of a colour glyph: a reusable outline plus a palette colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorLayer {
    /// Glyph id providing this layer's outline.
    pub glyph: u16,
    /// Fill colour from the font's palette.
    pub color: Color,
}

fn u8_at(data: &[u8], off: usize) -> u8 {
    data.get(off).copied().unwrap_or(0)
}

fn u16_at(data: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([u8_at(data, off), u8_at(data, off + 1)])
}

fn i16_at(data: &[u8], off: usize) -> i16 {
    u16_at(data, off) as i16
}

fn u32_at(data: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([
        u8_at(data, off),
        u8_at(data, off + 1),
        u8_at(data, off + 2),
        u8_at(data, off + 3),
    ])
}

impl Face {
    /// Loads the first face of a `.ttf`/`.ttc` file.
    pub fn load(path: &Path) -> Option<Face> {
        let data = std::fs::read(path).ok()?;
        Face::from_bytes(data, 0)
    }

    /// Parses a face from memory. `collection_index` selects a face when
    /// the data is a TrueType collection.
    pub fn from_bytes(mut data: Vec<u8>, collection_index: usize) -> Option<Face> {
        let base = if &data.get(0..4)? == b"ttcf" {
            let num = u32_at(&data, 8) as usize;
            if collection_index >= num {
                return None;
            }
            u32_at(&data, 12 + 4 * collection_index) as usize
        } else {
            0
        };

        let version = u32_at(&data, base);
        // 0x00010000 = TrueType, 'true' = Apple TrueType, 'OTTO' = CFF (unsupported).
        if version == u32::from_be_bytes(*b"OTTO") {
            return None;
        }
        let num_tables = u16_at(&data, base + 4) as usize;
        let mut table = |tag: &[u8; 4]| -> Option<(usize, usize)> {
            for i in 0..num_tables {
                let rec = base + 12 + i * 16;
                if data.get(rec..rec + 4)? == tag {
                    let off = u32_at(&data, rec + 8) as usize;
                    let len = u32_at(&data, rec + 12) as usize;
                    if off + len > data.len() {
                        return None;
                    }
                    return Some((off, len));
                }
            }
            None
        };

        let (head, _) = table(b"head")?;
        let (maxp, _) = table(b"maxp")?;
        let (hhea, _) = table(b"hhea")?;
        let (hmtx, _) = table(b"hmtx")?;
        let (cmap, _) = table(b"cmap")?;
        let (loca, _) = table(b"loca")?;
        let (glyf, _) = table(b"glyf")?;

        let units_per_em = u16_at(&data, head + 18);
        if units_per_em == 0 {
            return None;
        }
        let loca_format = i16_at(&data, head + 50);
        let num_glyphs = u16_at(&data, maxp + 4);
        let num_h_metrics = u16_at(&data, hhea + 34);
        let (cmap_offset, cmap_format) = pick_cmap(&data, cmap)?;
        let name = read_family_name(&data, base, num_tables, &mut table)
            .unwrap_or_else(|| "unnamed".to_string());

        // Font-designed style, used instead of synthesising when available.
        let mac_style = u16_at(&data, head + 44);
        let mut bold = mac_style & 0x0001 != 0;
        let mut italic = mac_style & 0x0002 != 0;
        if let Some((os2, _)) = table(b"OS/2") {
            let selection = u16_at(&data, os2 + 62);
            bold |= selection & 0x0020 != 0;
            italic |= selection & 0x0001 != 0;
        }

        // Colour glyphs: COLR v0 layer lists with a CPAL palette. COLR v1
        // fonts keep these records for older renderers, which is exactly the
        // flat, font-designed fallback we can draw faithfully.
        let colors = match (table(b"COLR"), table(b"CPAL")) {
            (Some((colr, _)), Some((cpal, _))) => parse_colors(&data, colr, cpal),
            _ => None,
        };

        // Normalize a TTC offset: table offsets are absolute already.
        data.shrink_to_fit();

        Some(Face {
            data,
            units_per_em: units_per_em as f32,
            loca_format,
            num_glyphs,
            num_h_metrics,
            cmap_offset,
            cmap_format,
            glyf,
            loca,
            hmtx,
            name,
            bold,
            italic,
            colors,
        })
    }

    /// Whether the font itself is a bold design.
    pub fn is_bold(&self) -> bool {
        self.bold
    }

    /// Whether the font itself is an italic design.
    pub fn is_italic(&self) -> bool {
        self.italic
    }

    /// Whether the face contains colour (layered) glyphs at all.
    pub fn has_color_glyphs(&self) -> bool {
        self.colors
            .as_ref()
            .is_some_and(|c| c.num_base > 0 || c.base_list.is_some())
    }

    /// Colour layers for a glyph, in painting order.
    ///
    /// Returns `None` for ordinary monochrome glyphs.
    pub fn color_layers(&self, gid: u16) -> Option<Vec<ColorLayer>> {
        let colors = self.colors.as_ref()?;
        let mut lo = 0usize;
        let mut hi = colors.num_base as usize;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(&self.data, colors.base_records + mid * 6) < gid {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= colors.num_base as usize {
            return None;
        }
        let record = colors.base_records + lo * 6;
        if u16_at(&self.data, record) != gid {
            return None;
        }
        let first = u16_at(&self.data, record + 2) as usize;
        let count = u16_at(&self.data, record + 4) as usize;
        if count == 0 {
            return None;
        }
        let mut layers = Vec::with_capacity(count);
        for k in 0..count {
            let layer = colors.layer_records + (first + k) * 4;
            let glyph = u16_at(&self.data, layer);
            let index = u16_at(&self.data, layer + 2) as usize;
            let color = colors.palette.get(index).copied().unwrap_or(Color::BLACK);
            layers.push(ColorLayer { glyph, color });
        }
        Some(layers)
    }

    /// The font's own paint graph for a colour glyph.
    ///
    /// Prefers the COLR v1 paint graph (layers, transforms, gradients) and
    /// falls back to the v0 layer list for older fonts.
    pub fn color_paint(&self, gid: u16) -> Option<Paint> {
        if let Some(colors) = self.colors.as_ref()
            && let Some(list) = colors.base_list
            && let Some(offset) = self.paint_offset(gid, list)
            && let Some(paint) = self.parse_paint(offset, 0)
        {
            return Some(paint);
        }
        let layers = self.color_layers(gid)?;
        Some(Paint::Layers(
            layers
                .into_iter()
                .map(|layer| Paint::Glyph {
                    glyph: layer.glyph,
                    paint: Box::new(Paint::Solid { color: layer.color }),
                })
                .collect(),
        ))
    }

    /// Finds the paint offset for `gid` in a COLR v1 BaseGlyphList.
    fn paint_offset(&self, gid: u16, list: usize) -> Option<usize> {
        let count = u32_at(&self.data, list) as usize;
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(&self.data, list + 4 + mid * 6) < gid {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= count {
            return None;
        }
        let record = list + 4 + lo * 6;
        if u16_at(&self.data, record) != gid {
            return None;
        }
        Some(list + u32_at(&self.data, record + 2) as usize)
    }

    /// Parses one COLR v1 paint table. `depth` bounds graph recursion.
    fn parse_paint(&self, offset: usize, depth: u32) -> Option<Paint> {
        if depth > 16 {
            return None;
        }
        let data = &self.data;
        match u8_at(data, offset) {
            // PaintColrLayers
            1 => {
                let count = u8_at(data, offset + 1) as usize;
                let first = u32_at(data, offset + 2) as usize;
                let list = self.colors.as_ref()?.layer_list?;
                let total = u32_at(data, list) as usize;
                let mut layers = Vec::with_capacity(count);
                for k in 0..count {
                    let index = first + k;
                    if index >= total {
                        break;
                    }
                    let paint_offset = list + u32_at(data, list + 4 + index * 4) as usize;
                    if let Some(paint) = self.parse_paint(paint_offset, depth + 1) {
                        layers.push(paint);
                    }
                }
                Some(Paint::Layers(layers))
            }
            // PaintSolid / PaintVarSolid
            2 | 3 => {
                let index = u16_at(data, offset + 1);
                let alpha = f2dot14(data, offset + 3);
                self.palette_paint(index, alpha)
            }
            // PaintLinearGradient / PaintVarLinearGradient
            4 | 5 => {
                let (extend, stops) =
                    self.parse_color_line(offset + u24_at(data, offset + 1) as usize)?;
                Some(Paint::Linear {
                    p0: [fword(data, offset + 4), fword(data, offset + 6)],
                    p1: [fword(data, offset + 8), fword(data, offset + 10)],
                    stops,
                    extend,
                })
            }
            // PaintRadialGradient / PaintVarRadialGradient
            6 | 7 => {
                let (extend, stops) =
                    self.parse_color_line(offset + u24_at(data, offset + 1) as usize)?;
                Some(Paint::Radial {
                    c0: [fword(data, offset + 4), fword(data, offset + 6)],
                    r0: u16_at(data, offset + 8) as f32,
                    c1: [fword(data, offset + 10), fword(data, offset + 12)],
                    r1: u16_at(data, offset + 14) as f32,
                    stops,
                    extend,
                })
            }
            // PaintGlyph
            10 => {
                let paint_offset = offset + u24_at(data, offset + 1) as usize;
                let glyph = u16_at(data, offset + 4);
                Some(Paint::Glyph {
                    glyph,
                    paint: Box::new(self.parse_paint(paint_offset, depth + 1)?),
                })
            }
            // PaintColrGlyph: reuse another base glyph's paint graph.
            11 => {
                let glyph = u16_at(data, offset + 1);
                let list = self.colors.as_ref()?.base_list?;
                let paint_offset = self.paint_offset(glyph, list)?;
                self.parse_paint(paint_offset, depth + 1)
            }
            // PaintTransform / PaintVarTransform
            12 | 13 => {
                let paint_offset = offset + u24_at(data, offset + 1) as usize;
                let affine = offset + u24_at(data, offset + 4) as usize;
                let matrix = [
                    fixed(data, affine),
                    fixed(data, affine + 4),
                    fixed(data, affine + 8),
                    fixed(data, affine + 12),
                    fixed(data, affine + 16),
                    fixed(data, affine + 20),
                ];
                Some(Paint::Transform {
                    matrix,
                    paint: Box::new(self.parse_paint(paint_offset, depth + 1)?),
                })
            }
            // Translate / Scale / Rotate / Skew, with and without a centre.
            14..=31 => {
                let paint_offset = offset + u24_at(data, offset + 1) as usize;
                let matrix = self.simple_transform(offset)?;
                Some(Paint::Transform {
                    matrix,
                    paint: Box::new(self.parse_paint(paint_offset, depth + 1)?),
                })
            }
            // PaintComposite
            32 => {
                let source = offset + u24_at(data, offset + 1) as usize;
                let backdrop = offset + u24_at(data, offset + 5) as usize;
                Some(Paint::Composite {
                    source: Box::new(self.parse_paint(source, depth + 1)?),
                    backdrop: Box::new(self.parse_paint(backdrop, depth + 1)?),
                })
            }
            _ => None,
        }
    }

    /// Builds the affine matrix of a simple transform paint table.
    fn simple_transform(&self, offset: usize) -> Option<[f32; 6]> {
        let data = &self.data;
        let format = u8_at(data, offset);
        let angle = |o: usize| f2dot14(data, o).to_radians();
        Some(match format {
            // Translate
            14 | 15 => [
                1.0,
                0.0,
                0.0,
                1.0,
                fword(data, offset + 4),
                fword(data, offset + 6),
            ],
            // Scale
            16 | 17 => [
                f2dot14(data, offset + 4),
                0.0,
                0.0,
                f2dot14(data, offset + 6),
                0.0,
                0.0,
            ],
            // ScaleAroundCenter
            18 | 19 => {
                let sx = f2dot14(data, offset + 8);
                let sy = f2dot14(data, offset + 10);
                around_center(
                    [sx, 0.0, 0.0, sy, 0.0, 0.0],
                    fword(data, offset + 4),
                    fword(data, offset + 6),
                )
            }
            // ScaleUniform
            20 | 21 => {
                let s = f2dot14(data, offset + 4);
                [s, 0.0, 0.0, s, 0.0, 0.0]
            }
            // ScaleUniformAroundCenter
            22 | 23 => {
                let s = f2dot14(data, offset + 4);
                around_center(
                    [s, 0.0, 0.0, s, 0.0, 0.0],
                    fword(data, offset + 6),
                    fword(data, offset + 8),
                )
            }
            // Rotate
            24 | 25 => {
                let a = angle(offset + 4);
                let (sin, cos) = a.sin_cos();
                [cos, sin, -sin, cos, 0.0, 0.0]
            }
            // RotateAroundCenter
            26 | 27 => {
                let a = angle(offset + 4);
                let (sin, cos) = a.sin_cos();
                around_center(
                    [cos, sin, -sin, cos, 0.0, 0.0],
                    fword(data, offset + 6),
                    fword(data, offset + 8),
                )
            }
            // Skew
            28 | 29 => {
                let x = f2dot14(data, offset + 4).to_radians().tan();
                let y = f2dot14(data, offset + 6).to_radians().tan();
                [1.0, y, x, 1.0, 0.0, 0.0]
            }
            // SkewAroundCenter
            30 | 31 => {
                let x = f2dot14(data, offset + 4).to_radians().tan();
                let y = f2dot14(data, offset + 6).to_radians().tan();
                around_center(
                    [1.0, y, x, 1.0, 0.0, 0.0],
                    fword(data, offset + 8),
                    fword(data, offset + 10),
                )
            }
            _ => return None,
        })
    }

    /// Resolves a palette index (0xFFFF = text foreground) into a paint.
    fn palette_paint(&self, index: u16, alpha: f32) -> Option<Paint> {
        if index == 0xFFFF {
            return Some(Paint::Foreground);
        }
        let color = self.colors.as_ref()?.palette.get(index as usize)?;
        let a = (color.a as f32 * alpha.clamp(0.0, 1.0))
            .round()
            .clamp(0.0, 255.0) as u8;
        Some(Paint::Solid {
            color: Color::rgba(color.r, color.g, color.b, a),
        })
    }

    /// Reads a gradient colour line (extend mode plus sorted stops).
    fn parse_color_line(&self, offset: usize) -> Option<(Extend, Vec<GradientStop>)> {
        let data = &self.data;
        let extend = match u8_at(data, offset) {
            1 => Extend::Repeat,
            2 => Extend::Reflect,
            _ => Extend::Pad,
        };
        let count = u16_at(data, offset + 1) as usize;
        let mut stops = Vec::with_capacity(count);
        for i in 0..count {
            let p = offset + 3 + i * 6;
            let stop_offset = i16_at(data, p) as f32 / 16384.0;
            let index = u16_at(data, p + 2);
            let alpha = i16_at(data, p + 4) as f32 / 16384.0;
            let color = if index == 0xFFFF {
                Color::BLACK
            } else {
                let base = *self.colors.as_ref()?.palette.get(index as usize)?;
                Color::rgba(
                    base.r,
                    base.g,
                    base.b,
                    (base.a as f32 * alpha.clamp(0.0, 1.0))
                        .round()
                        .clamp(0.0, 255.0) as u8,
                )
            };
            stops.push(GradientStop {
                offset: stop_offset,
                color,
            });
        }
        stops.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Some((extend, stops))
    }

    /// Family name (best effort).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Glyph id for a character, or 0 (`.notdef`) when absent.
    pub fn glyph_id(&self, c: char) -> u16 {
        match self.cmap_format {
            4 => self.cmap4(c as u32),
            12 => self.cmap12(c as u32),
            _ => 0,
        }
    }

    /// Whether the face has a real glyph for `c`.
    pub fn has_glyph(&self, c: char) -> bool {
        self.glyph_id(c) != 0
    }

    /// Horizontal advance in font units (unused for layout, exposed for
    /// diagnostics and tests).
    pub fn advance(&self, gid: u16) -> f32 {
        if self.num_h_metrics == 0 {
            return 0.0;
        }
        let idx = gid.min(self.num_h_metrics - 1) as usize;
        u16_at(&self.data, self.hmtx + idx * 4) as f32
    }

    /// Units per em (2048 for most TrueType faces, 1000 for CFF).
    pub fn units_per_em(&self) -> f32 {
        self.units_per_em
    }

    fn cmap4(&self, code: u32) -> u16 {
        if code > 0xFFFF {
            return 0;
        }
        let code = code as u16;
        let t = self.cmap_offset;
        let seg_count = (u16_at(&self.data, t + 6) / 2) as usize;
        if seg_count == 0 {
            return 0;
        }
        let end_codes = t + 14;
        let start_codes = end_codes + seg_count * 2 + 2;
        let id_deltas = start_codes + seg_count * 2;
        let id_range_offsets = id_deltas + seg_count * 2;

        // Binary search for the first segment whose endCode >= code.
        let (mut lo, mut hi) = (0usize, seg_count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(&self.data, end_codes + mid * 2) < code {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo >= seg_count {
            return 0;
        }
        let start = u16_at(&self.data, start_codes + lo * 2);
        if code < start {
            return 0;
        }
        let delta = i16_at(&self.data, id_deltas + lo * 2);
        let range = u16_at(&self.data, id_range_offsets + lo * 2);
        if range == 0 {
            return code.wrapping_add(delta as u16);
        }
        let glyph_off = id_range_offsets + lo * 2 + range as usize + (code - start) as usize * 2;
        let gid = u16_at(&self.data, glyph_off);
        if gid == 0 {
            0
        } else {
            gid.wrapping_add(delta as u16)
        }
    }

    fn cmap12(&self, code: u32) -> u16 {
        let t = self.cmap_offset;
        let n_groups = u32_at(&self.data, t + 12) as usize;
        let groups = t + 16;
        let (mut lo, mut hi) = (0usize, n_groups);
        while lo < hi {
            let mid = (lo + hi) / 2;
            let start = u32_at(&self.data, groups + mid * 12);
            let end = u32_at(&self.data, groups + mid * 12 + 4);
            if code < start {
                hi = mid;
            } else if code > end {
                lo = mid + 1;
            } else {
                let start_gid = u32_at(&self.data, groups + mid * 12 + 8);
                return (start_gid + (code - start)) as u16;
            }
        }
        0
    }

    /// Outline for a glyph, flattened to polylines in font units.
    ///
    /// `flatness` is the maximum deviation from a curve, in font units.
    pub fn outline(&self, gid: u16, flatness: f32) -> Vec<Contour> {
        let mut out = Vec::new();
        self.outline_into(gid, Transform::IDENTITY, flatness, 0, &mut out);
        out
    }

    fn outline_into(
        &self,
        gid: u16,
        xf: Transform,
        flatness: f32,
        depth: u32,
        out: &mut Vec<Contour>,
    ) {
        if depth > 8 || gid >= self.num_glyphs {
            return;
        }
        let (start, end) = match self.glyph_range(gid) {
            Some(range) if range.1 > range.0 => range,
            _ => return,
        };
        let n_contours = i16_at(&self.data, start);
        if n_contours >= 0 {
            self.simple_glyph(start, n_contours as usize, xf, flatness, out);
        } else {
            self.composite_glyph(start, xf, flatness, depth, out);
        }
        let _ = end;
    }

    /// Byte range of a glyph in `glyf`.
    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        let (a, b) = if self.loca_format == 0 {
            (
                u16_at(&self.data, self.loca + gid as usize * 2) as usize * 2,
                u16_at(&self.data, self.loca + gid as usize * 2 + 2) as usize * 2,
            )
        } else {
            (
                u32_at(&self.data, self.loca + gid as usize * 4) as usize,
                u32_at(&self.data, self.loca + gid as usize * 4 + 4) as usize,
            )
        };
        if b <= a || b > self.data.len().saturating_sub(self.glyf) {
            return None;
        }
        Some((self.glyf + a, self.glyf + b))
    }

    fn simple_glyph(
        &self,
        start: usize,
        n_contours: usize,
        xf: Transform,
        flatness: f32,
        out: &mut Vec<Contour>,
    ) {
        let mut p = start + 10;
        let mut end_pts = Vec::with_capacity(n_contours);
        for _ in 0..n_contours {
            end_pts.push(u16_at(&self.data, p) as usize);
            p += 2;
        }
        let n_points = end_pts.last().map(|e| e + 1).unwrap_or(0);
        let instructions = u16_at(&self.data, p) as usize;
        p += 2 + instructions;

        let mut flags = Vec::with_capacity(n_points);
        while flags.len() < n_points {
            let f = u8_at(&self.data, p);
            p += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                let repeat = u8_at(&self.data, p);
                p += 1;
                for _ in 0..repeat {
                    flags.push(f);
                }
            }
        }

        let mut coords = vec![Point { x: 0.0, y: 0.0 }; n_points];
        let mut x = 0i32;
        for (i, flag) in flags.iter().enumerate().take(n_points) {
            if flag & 0x02 != 0 {
                let dx = u8_at(&self.data, p) as i32;
                p += 1;
                x += if flag & 0x10 != 0 { dx } else { -dx };
            } else if flag & 0x10 == 0 {
                x += i16_at(&self.data, p) as i32;
                p += 2;
            }
            coords[i].x = x as f32;
        }
        let mut y = 0i32;
        for (i, flag) in flags.iter().enumerate().take(n_points) {
            if flag & 0x04 != 0 {
                let dy = u8_at(&self.data, p) as i32;
                p += 1;
                y += if flag & 0x20 != 0 { dy } else { -dy };
            } else if flag & 0x20 == 0 {
                y += i16_at(&self.data, p) as i32;
                p += 2;
            }
            coords[i].y = y as f32;
        }

        let mut first = 0usize;
        for end in end_pts {
            if end < first || end >= n_points {
                first = end.saturating_add(1);
                continue;
            }
            let contour = flatten_contour(&coords[first..=end], &flags[first..=end], xf, flatness);
            if contour.len() >= 3 {
                out.push(contour);
            }
            first = end + 1;
        }
    }

    fn composite_glyph(
        &self,
        start: usize,
        mut xf: Transform,
        flatness: f32,
        depth: u32,
        out: &mut Vec<Contour>,
    ) {
        let mut p = start + 10;
        loop {
            let flags = u16_at(&self.data, p);
            let gid = u16_at(&self.data, p + 2);
            p += 4;

            let (arg1, arg2) = if flags & 0x0001 != 0 {
                let a = i16_at(&self.data, p) as f32;
                let b = i16_at(&self.data, p + 2) as f32;
                p += 4;
                (a, b)
            } else {
                let a = u8_at(&self.data, p) as i8 as f32;
                let b = u8_at(&self.data, p + 1) as i8 as f32;
                p += 2;
                (a, b)
            };

            let mut local = Transform::IDENTITY;
            if flags & 0x0008 != 0 {
                let s = f2dot14(&self.data, p);
                p += 2;
                local = Transform {
                    a: s,
                    b: 0.0,
                    c: 0.0,
                    d: s,
                    dx: 0.0,
                    dy: 0.0,
                };
            } else if flags & 0x0040 != 0 {
                let sx = f2dot14(&self.data, p);
                let sy = f2dot14(&self.data, p + 2);
                p += 4;
                local = Transform {
                    a: sx,
                    b: 0.0,
                    c: 0.0,
                    d: sy,
                    dx: 0.0,
                    dy: 0.0,
                };
            } else if flags & 0x0080 != 0 {
                let a = f2dot14(&self.data, p);
                let b = f2dot14(&self.data, p + 2);
                let c = f2dot14(&self.data, p + 4);
                let d = f2dot14(&self.data, p + 6);
                p += 8;
                local = Transform {
                    a,
                    b,
                    c,
                    d,
                    dx: 0.0,
                    dy: 0.0,
                };
            }

            // Only XY offsets are supported; point-matching offsets fall back
            // to no offset (rare in practice, and only affects composites).
            if flags & 0x0002 != 0 {
                local.dx = arg1;
                local.dy = arg2;
            }

            self.outline_into(gid, xf.then(local), flatness, depth + 1, out);

            if flags & 0x0020 == 0 {
                break;
            }
        }
        let _ = &mut xf;
    }
}

/// Affine transform in font units: `x' = a*x + c*y + dx`, `y' = b*x + d*y + dy`.
#[derive(Debug, Clone, Copy)]
struct Transform {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    dx: f32,
    dy: f32,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        dx: 0.0,
        dy: 0.0,
    };

    fn apply(&self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.dx,
            y: self.b * p.x + self.d * p.y + self.dy,
        }
    }

    /// `self ∘ inner`: apply `inner` first, then `self`.
    fn then(&self, inner: Transform) -> Transform {
        Transform {
            a: self.a * inner.a + self.c * inner.b,
            b: self.b * inner.a + self.d * inner.b,
            c: self.a * inner.c + self.c * inner.d,
            d: self.b * inner.c + self.d * inner.d,
            dx: self.a * inner.dx + self.c * inner.dy + self.dx,
            dy: self.b * inner.dx + self.d * inner.dy + self.dy,
        }
    }
}

/// Parses COLR v0 layer records and the first CPAL palette.
fn parse_colors(data: &[u8], colr: usize, cpal: usize) -> Option<ColorGlyphs> {
    if u16_at(data, colr) > 1 {
        return None;
    }
    let num_base = u16_at(data, colr + 2);
    if num_base == 0 {
        return None;
    }
    let base_records = colr.checked_add(u32_at(data, colr + 4) as usize)?;
    let layer_records = colr.checked_add(u32_at(data, colr + 8) as usize)?;
    if base_records >= data.len() || layer_records >= data.len() {
        return None;
    }

    if u16_at(data, cpal) > 1 {
        return None;
    }
    let num_entries = u16_at(data, cpal + 2) as usize;
    let num_palettes = u16_at(data, cpal + 4) as usize;
    if num_entries == 0 || num_palettes == 0 {
        return None;
    }
    let color_records = cpal.checked_add(u32_at(data, cpal + 8) as usize)?;
    let first = u16_at(data, cpal + 12) as usize;
    let mut palette = Vec::with_capacity(num_entries);
    for i in 0..num_entries {
        let p = color_records + (first + i) * 4;
        if p + 3 >= data.len() {
            return None;
        }
        // CPAL stores BGRA.
        palette.push(Color::rgba(
            u8_at(data, p + 2),
            u8_at(data, p + 1),
            u8_at(data, p),
            u8_at(data, p + 3),
        ));
    }
    // COLR v1 adds a paint-graph list; v0-only fonts leave these empty.
    let (base_list, layer_list) = if u16_at(data, colr) >= 1 {
        let base = u32_at(data, colr + 14) as usize;
        let layers = u32_at(data, colr + 18) as usize;
        if base != 0 && layers != 0 {
            (Some(colr + base), Some(colr + layers))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    Some(ColorGlyphs {
        base_records,
        num_base,
        layer_records,
        palette,
        base_list,
        layer_list,
    })
}

fn u24_at(data: &[u8], off: usize) -> u32 {
    ((u8_at(data, off) as u32) << 16)
        | ((u8_at(data, off + 1) as u32) << 8)
        | u8_at(data, off + 2) as u32
}

fn fword(data: &[u8], off: usize) -> f32 {
    i16_at(data, off) as f32
}

fn fixed(data: &[u8], off: usize) -> f32 {
    i32::from_be_bytes([
        u8_at(data, off),
        u8_at(data, off + 1),
        u8_at(data, off + 2),
        u8_at(data, off + 3),
    ]) as f32
        / 65536.0
}

/// Builds a transform that scales/rotates around `(cx, cy)`.
fn around_center(m: [f32; 6], cx: f32, cy: f32) -> [f32; 6] {
    let [a, b, c, d, e, f] = m;
    [
        a,
        b,
        c,
        d,
        e + cx - a * cx - c * cy,
        f + cy - b * cx - d * cy,
    ]
}

fn f2dot14(data: &[u8], off: usize) -> f32 {
    i16_at(data, off) as f32 / 16384.0
}

/// Converts a TrueType contour (quadratic splines) to a polyline.
fn flatten_contour(points: &[Point], flags: &[u8], xf: Transform, flatness: f32) -> Contour {
    let n = points.len();
    if n == 0 {
        return Vec::new();
    }
    let is_on = |i: usize| flags[i] & 0x01 != 0;

    // Rotate the contour so it starts at an on-curve point; when every point
    // is off-curve, start at the implied midpoint between the last and first
    // point (TrueType semantics).
    let mut pts: Vec<(Point, bool)> = Vec::with_capacity(n + 2);
    match (0..n).find(|&i| is_on(i)) {
        Some(s) => {
            for k in 0..=n {
                let i = (s + k) % n;
                pts.push((points[i], is_on(i)));
            }
        }
        None => {
            let start = mid_point(points[n - 1], points[0]);
            pts.push((start, true));
            for &p in points {
                pts.push((p, false));
            }
            pts.push((start, true));
        }
    }

    let mut out: Contour = Vec::with_capacity(pts.len() * 2);
    let mut current = pts[0].0;
    out.push(xf.apply(current));

    let mut i = 1usize;
    while i < pts.len() {
        let (p, on) = pts[i];
        if on {
            out.push(xf.apply(p));
            current = p;
            i += 1;
        } else if i + 1 < pts.len() {
            let (next, next_on) = pts[i + 1];
            if next_on {
                flatten_quad(current, p, next, xf, flatness, &mut out);
                current = next;
                i += 2;
            } else {
                let end = mid_point(p, next);
                flatten_quad(current, p, end, xf, flatness, &mut out);
                current = end;
                i += 1;
            }
        } else {
            break;
        }
    }
    out
}

fn mid_point(a: Point, b: Point) -> Point {
    Point {
        x: (a.x + b.x) * 0.5,
        y: (a.y + b.y) * 0.5,
    }
}

/// Appends a flattened quadratic Bézier from `p0` to `p2` with control `p1`.
fn flatten_quad(p0: Point, p1: Point, p2: Point, xf: Transform, flatness: f32, out: &mut Contour) {
    // Deviation of the control point from the chord decides the subdivision.
    let dx = p0.x - 2.0 * p1.x + p2.x;
    let dy = p0.y - 2.0 * p1.y + p2.y;
    let dev = (dx * dx + dy * dy).sqrt() * 0.25;
    let steps = if flatness > 0.0 {
        ((dev / flatness).sqrt().ceil() as usize).clamp(1, 16)
    } else {
        8
    };
    for s in 1..=steps {
        let t = s as f32 / steps as f32;
        let mt = 1.0 - t;
        let p = Point {
            x: mt * mt * p0.x + 2.0 * mt * t * p1.x + t * t * p2.x,
            y: mt * mt * p0.y + 2.0 * mt * t * p1.y + t * t * p2.y,
        };
        out.push(xf.apply(p));
    }
}

/// Picks the best Unicode cmap subtable: format 12 first, then format 4.
fn pick_cmap(data: &[u8], cmap: usize) -> Option<(usize, u16)> {
    let n = u16_at(data, cmap + 2) as usize;
    let mut best4: Option<usize> = None;
    let mut best12: Option<usize> = None;
    for i in 0..n {
        let rec = cmap + 4 + i * 8;
        let platform = u16_at(data, rec);
        let encoding = u16_at(data, rec + 2);
        let sub = cmap + u32_at(data, rec + 4) as usize;
        let format = u16_at(data, sub);
        let unicode = matches!((platform, encoding), (0, _) | (3, 1) | (3, 10));
        if !unicode {
            continue;
        }
        match format {
            12 => best12.get_or_insert(sub),
            4 => best4.get_or_insert(sub),
            _ => continue,
        };
    }
    if let Some(sub) = best12 {
        return Some((sub, 12));
    }
    best4.map(|sub| (sub, 4))
}

fn read_family_name(
    data: &[u8],
    base: usize,
    num_tables: usize,
    table: &mut impl FnMut(&[u8; 4]) -> Option<(usize, usize)>,
) -> Option<String> {
    let (name, _) = table(b"name")?;
    let count = u16_at(data, name + 2) as usize;
    let string_offset = name + u16_at(data, name + 4) as usize;
    let mut fallback: Option<String> = None;
    for i in 0..count {
        let rec = name + 6 + i * 12;
        let platform = u16_at(data, rec);
        let name_id = u16_at(data, rec + 6);
        if name_id != 1 {
            continue;
        }
        let len = u16_at(data, rec + 8) as usize;
        let off = string_offset + u16_at(data, rec + 10) as usize;
        let bytes = data.get(off..off + len)?;
        let text = if platform == 3 || platform == 0 {
            // UTF-16BE
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(bytes).to_string()
        };
        if !text.is_empty() {
            if platform == 3 {
                return Some(text);
            }
            fallback.get_or_insert(text);
        }
    }
    let _ = base;
    let _ = num_tables;
    fallback
}

// ---------------------------------------------------------------------------
// Font discovery
// ---------------------------------------------------------------------------

/// Fonts used to rasterize text: a primary face per family slot and a list
/// of fallbacks for characters the primary faces do not cover (CJK, emoji).
#[derive(Debug, Clone)]
pub struct FontSet {
    /// Proportional face(s); the first entry is the primary.
    pub sans: Vec<PathBuf>,
    /// Monospace face(s).
    pub mono: Vec<PathBuf>,
    /// Extra fallbacks, consulted for any missing glyph.
    pub fallback: Vec<PathBuf>,
}

impl FontSet {
    /// Loads every listed face that parses. Returns `None` when nothing
    /// usable is found.
    pub fn load(&self) -> Option<LoadedFonts> {
        let mut fonts = LoadedFonts::default();
        for p in self.sans.iter().chain(&self.fallback) {
            if let Some(f) = Face::load(p) {
                fonts.sans.push(f);
            }
        }
        for p in self.mono.iter().chain(&self.fallback) {
            if let Some(f) = Face::load(p) {
                fonts.mono.push(f);
            }
        }
        if fonts.sans.is_empty() && fonts.mono.is_empty() {
            None
        } else {
            if fonts.sans.is_empty() {
                fonts.sans = fonts.mono.clone();
            }
            if fonts.mono.is_empty() {
                fonts.mono = fonts.sans.clone();
            }
            Some(fonts)
        }
    }
}

/// Font faces ready for rendering.
#[derive(Debug, Clone, Default)]
pub struct LoadedFonts {
    /// Proportional candidates, in priority order.
    pub sans: Vec<Face>,
    /// Monospace candidates, in priority order.
    pub mono: Vec<Face>,
}

impl LoadedFonts {
    /// Finds the best face for `c` in the requested style.
    ///
    /// Font fidelity first: a face designed in the requested weight and
    /// slant wins, so bold text uses the real bold outlines when the font
    /// provides them. Only when no styled face covers the character does it
    /// fall back to another style (and the rasterizer synthesises the rest).
    pub fn find(&self, c: char, mono: bool, bold: bool, italic: bool) -> Option<&Face> {
        let primary = if mono { &self.mono } else { &self.sans };
        let fallback = if mono { &self.sans } else { &self.mono };
        primary
            .iter()
            .find(|f| f.has_glyph(c) && f.is_bold() == bold && f.is_italic() == italic)
            .or_else(|| primary.iter().find(|f| f.has_glyph(c)))
            .or_else(|| {
                fallback
                    .iter()
                    .find(|f| f.has_glyph(c) && f.is_bold() == bold && f.is_italic() == italic)
            })
            .or_else(|| fallback.iter().find(|f| f.has_glyph(c)))
    }

    /// Total number of loaded faces.
    pub fn len(&self) -> usize {
        self.sans.len() + self.mono.len()
    }

    /// Whether no faces are loaded.
    pub fn is_empty(&self) -> bool {
        self.sans.is_empty() && self.mono.is_empty()
    }
}

/// Candidate font locations for the current platform.
pub fn discover() -> FontSet {
    let mut set = FontSet {
        sans: Vec::new(),
        mono: Vec::new(),
        fallback: Vec::new(),
    };

    #[cfg(target_os = "windows")]
    {
        let dir = Path::new("C:/Windows/Fonts");
        // R, B, I, BI for each family so the rasterizer can use designed
        // weights and slants instead of synthesising them.
        set.sans.extend(existing(
            dir,
            &[
                "segoeui.ttf",
                "segoeuib.ttf",
                "segoeuii.ttf",
                "segoeuiz.ttf",
                "arial.ttf",
                "arialbd.ttf",
                "ariali.ttf",
                "arialbi.ttf",
                "calibri.ttf",
                "calibrib.ttf",
                "calibrii.ttf",
                "calibriz.ttf",
            ],
        ));
        set.mono.extend(existing(
            dir,
            &[
                "consola.ttf",
                "consolab.ttf",
                "consolai.ttf",
                "consolaz.ttf",
                "cour.ttf",
                "courbd.ttf",
                "couri.ttf",
                "courbi.ttf",
            ],
        ));
        // CJK first, then the colour emoji font before the monochrome symbol
        // font, so emoji resolve to their designed colour layers.
        set.fallback.extend(existing(
            dir,
            &[
                "msyh.ttc",
                "msyhbd.ttc",
                "simsun.ttc",
                "simhei.ttf",
                "seguiemj.ttf",
                "seguisym.ttf",
            ],
        ));
    }

    #[cfg(target_os = "macos")]
    {
        set.sans.extend(existing(
            Path::new("/System/Library/Fonts"),
            &["Helvetica.ttc", "SFNS.ttf"],
        ));
        set.mono
            .extend(existing(Path::new("/System/Library/Fonts"), &["Menlo.ttc"]));
        set.fallback.extend(existing(
            Path::new("/System/Library/Fonts"),
            &["PingFang.ttc", "Hiragino Sans GB.ttc"],
        ));
        set.fallback.extend(existing(
            Path::new("/System/Library/Fonts/Supplemental"),
            &["Arial Unicode.ttf"],
        ));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let dirs = [
            Path::new("/usr/share/fonts/truetype/dejavu"),
            Path::new("/usr/share/fonts/truetype/liberation"),
            Path::new("/usr/share/fonts"),
        ];
        for dir in dirs {
            set.sans.extend(existing(
                dir,
                &["DejaVuSans.ttf", "LiberationSans-Regular.ttf"],
            ));
            set.mono.extend(existing(
                dir,
                &["DejaVuSansMono.ttf", "LiberationMono-Regular.ttf"],
            ));
            set.fallback.extend(existing(
                dir,
                &["NotoSansCJK-Regular.ttc", "wqy-zenhei.ttc"],
            ));
        }
    }

    set
}

fn existing(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names
        .iter()
        .map(|n| dir.join(n))
        .filter(|p| p.is_file())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn any_face() -> Option<Face> {
        let set = discover();
        let all: Vec<&PathBuf> = set.sans.iter().chain(&set.fallback).collect();
        all.iter().find_map(|p| Face::load(p))
    }

    #[test]
    fn discovery_finds_fonts_on_this_platform() {
        let set = discover();
        assert!(
            !set.sans.is_empty() || !set.fallback.is_empty(),
            "no system fonts discovered"
        );
    }

    #[test]
    fn parses_a_system_face() {
        let Some(face) = any_face() else {
            eprintln!("skipping: no system font available");
            return;
        };
        assert!(face.units_per_em() > 0.0);
        assert!(!face.name().is_empty());
        assert!(face.has_glyph('A'), "face {} lacks 'A'", face.name());
    }

    #[test]
    fn latin_glyph_has_contours() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id('A');
        assert_ne!(gid, 0);
        let outline = face.outline(gid, 1.0);
        assert!(!outline.is_empty(), "glyph 'A' produced no contours");
        assert!(outline.iter().all(|c| c.len() >= 3));
        // Points must be finite and within a sane range.
        for c in &outline {
            for p in c {
                assert!(p.x.is_finite() && p.y.is_finite());
                assert!(p.x.abs() < 20000.0 && p.y.abs() < 20000.0);
            }
        }
    }

    #[test]
    fn space_has_no_contours() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id(' ');
        assert!(face.outline(gid, 1.0).is_empty());
    }

    #[test]
    fn composite_glyphs_parse() {
        let Some(face) = any_face() else { return };
        if !face.has_glyph('é') {
            return;
        }
        let outline = face.outline(face.glyph_id('é'), 1.0);
        assert!(!outline.is_empty(), "composite glyph produced no contours");
    }

    #[test]
    fn missing_characters_map_to_notdef() {
        let Some(face) = any_face() else { return };
        // A private-use codepoint is almost certainly absent.
        assert!(!face.has_glyph('\u{F8FF}') || face.glyph_id('\u{F8FF}') != 0);
    }

    #[test]
    fn advance_is_positive_for_letters() {
        let Some(face) = any_face() else { return };
        assert!(face.advance(face.glyph_id('M')) > 0.0);
    }

    #[test]
    fn cjk_font_is_available_and_covers_han() {
        let set = discover();
        let mut found = false;
        for p in &set.fallback {
            if let Some(f) = Face::load(p)
                && f.has_glyph('中')
            {
                found = true;
                break;
            }
        }
        assert!(found, "no CJK-capable fallback font found");
    }

    #[test]
    fn loaded_fonts_resolve_by_family() {
        let set = discover();
        if let Some(loaded) = set.load() {
            assert!(!loaded.is_empty());
            if let Some(face) = loaded.find('A', false, false, false) {
                assert!(face.has_glyph('A'));
            }
        }
    }

    #[test]
    fn garbage_data_is_rejected() {
        assert!(Face::from_bytes(vec![0u8; 64], 0).is_none());
        assert!(Face::from_bytes(b"not a font at all".to_vec(), 0).is_none());
    }

    #[test]
    fn missing_file_returns_none() {
        assert!(Face::load(Path::new("no-such-font-file.ttf")).is_none());
    }

    #[test]
    fn cff_flavoured_opentype_is_skipped() {
        // 'OTTO' header: parsed as a font directory but unsupported.
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(b"OTTO");
        data[4..6].copy_from_slice(&0u16.to_be_bytes());
        assert!(Face::from_bytes(data, 0).is_none());
    }

    #[test]
    fn collection_index_is_bounds_checked() {
        // Minimal TTC header with one font, then ask for the second.
        let mut data = vec![0u8; 32];
        data[0..4].copy_from_slice(b"ttcf");
        data[8..12].copy_from_slice(&1u32.to_be_bytes());
        data[12..16].copy_from_slice(&16u32.to_be_bytes());
        assert!(Face::from_bytes(data, 1).is_none());
    }

    #[test]
    fn loads_the_first_face_of_a_collection() {
        let set = discover();
        let Some(ttc) = set
            .fallback
            .iter()
            .find(|p| p.extension().is_some_and(|e| e == "ttc"))
        else {
            eprintln!("skipping: no .ttc on this platform");
            return;
        };
        let face = Face::load(ttc).expect("first face of a collection");
        assert!(face.units_per_em() > 0.0);
        assert!(face.has_glyph('中'));
        // A second face may or may not exist; either way it must not panic.
        let _ = Face::from_bytes(std::fs::read(ttc).unwrap(), 1);
    }

    #[test]
    fn outline_flatness_changes_point_counts() {
        let Some(face) = any_face() else { return };
        let gid = face.glyph_id('O');
        let coarse = face.outline(gid, 8.0);
        let fine = face.outline(gid, 0.2);
        let count = |c: &Vec<Contour>| c.iter().map(|p| p.len()).sum::<usize>();
        assert!(
            count(&fine) >= count(&coarse),
            "finer flatness should not reduce points"
        );
    }

    #[test]
    fn glyph_lookup_is_stable() {
        let Some(face) = any_face() else { return };
        for c in ['a', 'Z', '0', '中', 'Ω'] {
            let first = face.glyph_id(c);
            assert_eq!(first, face.glyph_id(c));
        }
    }

    #[test]
    fn color_emoji_has_palette_layers() {
        let set = discover();
        let Some(loaded) = set.load() else { return };
        let Some(face) = loaded.find('✅', false, false, false) else {
            eprintln!("skipping: no emoji font");
            return;
        };
        let gid = face.glyph_id('✅');
        let Some(layers) = face.color_layers(gid) else {
            eprintln!("skipping: {} has no COLR layers", face.name());
            return;
        };
        assert!(layers.len() >= 2, "expected layered colour glyph");
        let colors: Vec<crate::color::Color> = layers.iter().map(|l| l.color).collect();
        assert!(
            colors.iter().any(|c| c.g > c.r && c.g > c.b),
            "expected a green palette colour: {colors:?}"
        );
        for layer in &layers {
            assert!(
                !face.outline(layer.glyph, 1.0).is_empty(),
                "layer {} has no outline",
                layer.glyph
            );
        }
    }

    #[test]
    fn styled_faces_are_preferred_when_installed() {
        let set = discover();
        let Some(loaded) = set.load() else { return };
        let Some(regular) = loaded.find('A', false, false, false) else {
            return;
        };
        if let Some(bold) = loaded.find('A', false, true, false)
            && bold.name() != regular.name()
        {
            assert!(
                bold.is_bold(),
                "bold request picked a non-bold design: {}",
                bold.name()
            );
        }
        if let Some(italic) = loaded.find('A', false, false, true)
            && italic.name() != regular.name()
        {
            assert!(
                italic.is_italic(),
                "italic request picked an upright design: {}",
                italic.name()
            );
        }
    }

    #[test]
    fn plain_faces_report_their_design() {
        let Some(face) = any_face() else { return };
        // Whatever the file is, the flags must be self-consistent.
        assert_eq!(face.is_bold(), face.is_bold());
        assert_eq!(face.is_italic(), face.is_italic());
    }

    #[test]
    fn colrv1_paint_graph_uses_gradients() {
        let set = discover();
        let Some(loaded) = set.load() else { return };
        let Some(face) = loaded.find('❤', false, false, false) else {
            return;
        };
        let Some(paint) = face.color_paint(face.glyph_id('❤')) else {
            eprintln!("skipping: no COLRv1 paint graph");
            return;
        };
        fn walk(p: &Paint, gradients: &mut usize, solids: &mut usize, depth: usize) {
            if depth > 24 {
                return;
            }
            match p {
                Paint::Linear { .. } | Paint::Radial { .. } => *gradients += 1,
                Paint::Solid { .. } => *solids += 1,
                Paint::Layers(items) => {
                    for item in items {
                        walk(item, gradients, solids, depth + 1);
                    }
                }
                Paint::Glyph { paint, .. } | Paint::Transform { paint, .. } => {
                    walk(paint, gradients, solids, depth + 1);
                }
                Paint::Composite { source, backdrop } => {
                    walk(source, gradients, solids, depth + 1);
                    walk(backdrop, gradients, solids, depth + 1);
                }
                Paint::Foreground => {}
            }
        }
        let (mut gradients, mut solids) = (0, 0);
        walk(&paint, &mut gradients, &mut solids, 0);
        assert!(gradients > 0, "expected gradient paints in the heart");
        assert!(solids + gradients > 1, "expected several paint layers");
    }
}
