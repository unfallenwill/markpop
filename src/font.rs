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
    /// Variation axes and the currently selected instance.
    vars: Variations,
}

/// Colour information for layered (COLR v0) glyphs and COLR v1 paint graphs.
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
    /// Absolute offset of the COLR v1 ClipList, when present.
    clip_list: Option<usize>,
    /// Absolute offset of the COLR ItemVariationStore, when present.
    var_store: Option<usize>,
    /// Absolute offset of the COLR DeltaSetIndexMap, when present.
    var_map: Option<usize>,
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
    ///
    /// `p2` is the rotation point: the colour line is projected along the
    /// direction perpendicular to `p0`–`p2`, not necessarily along `p0`–`p1`.
    Linear {
        p0: [f32; 2],
        p1: [f32; 2],
        p2: [f32; 2],
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
    /// Sweep (conic) gradient around `center`.
    ///
    /// `start` and `end` are counter-clockwise angles in radians from the
    /// positive x axis of the design grid.
    Sweep {
        center: [f32; 2],
        start: f32,
        end: f32,
        stops: Vec<GradientStop>,
        extend: Extend,
    },
    /// Source composited over backdrop using a COLR composite mode index.
    Composite {
        mode: u8,
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

/// One axis of a variable font, in design units.
///
/// ```
/// use markpop::font::Axis;
/// let axis = Axis {
///     tag: "wght".into(),
///     min: 100.0,
///     default: 400.0,
///     max: 900.0,
/// };
/// assert_eq!(axis.default, 400.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Axis {
    /// Four-character axis tag (`wght`, `opsz`, …).
    pub tag: String,
    /// Minimum design value.
    pub min: f32,
    /// Default design value (the static instance).
    pub default: f32,
    /// Maximum design value.
    pub max: f32,
}

impl Axis {
    /// Restricts a design value to the axis range.
    pub fn clamp(&self, value: f32) -> f32 {
        if self.min <= self.max {
            value.clamp(self.min, self.max)
        } else {
            self.default
        }
    }
}

/// Variation state of a face: axes, selected instance and table offsets.
#[derive(Debug, Clone, Default)]
struct Variations {
    /// Axes declared by `fvar`, in font order.
    axes: Vec<Axis>,
    /// Selected design-space values, one per axis.
    values: Vec<f32>,
    /// Normalized coordinates in `-1.0..=1.0` (after `avar` mapping).
    coords: Vec<f32>,
    /// Absolute offset of `avar`, when present.
    avar: Option<usize>,
    /// Absolute offset of `gvar`, when present.
    gvar: Option<usize>,
    /// Absolute offset of `HVAR`, when present.
    hvar: Option<usize>,
}

impl Variations {
    fn parse(
        data: &[u8],
        table: &mut impl FnMut(&[u8; 4]) -> Option<(usize, usize)>,
    ) -> Variations {
        let mut vars = Variations::default();
        let Some((fvar, _)) = table(b"fvar") else {
            return vars;
        };
        let axes_offset = u16_at(data, fvar + 4) as usize;
        let axis_count = u16_at(data, fvar + 8) as usize;
        let axis_size = u16_at(data, fvar + 10) as usize;
        for i in 0..axis_count {
            let p = fvar + axes_offset + i * axis_size;
            vars.axes.push(Axis {
                tag: tag_at(data, p),
                min: fixed(data, p + 4),
                default: fixed(data, p + 8),
                max: fixed(data, p + 12),
            });
        }
        vars.values = vars.axes.iter().map(|a| a.default).collect();
        vars.coords = vec![0.0; vars.axes.len()];
        vars.avar = table(b"avar").map(|(off, _)| off);
        vars.gvar = table(b"gvar").map(|(off, _)| off);
        vars.hvar = table(b"HVAR").map(|(off, _)| off);
        vars
    }

    /// Selects the instance described by `settings` (`tag = value` pairs)
    /// and recomputes the normalized coordinates.
    fn apply(&mut self, data: &[u8], settings: &[(String, f32)]) {
        for (i, axis) in self.axes.iter().enumerate() {
            if let Some((_, value)) = settings
                .iter()
                .rev()
                .find(|(tag, _)| tag.eq_ignore_ascii_case(&axis.tag))
            {
                self.values[i] = axis.clamp(*value);
            }
        }
        self.normalize(data);
    }

    fn normalize(&mut self, data: &[u8]) {
        self.coords = self
            .axes
            .iter()
            .zip(&self.values)
            .enumerate()
            .map(|(i, (axis, value))| {
                let value = *value;
                let normalized = if value < axis.default {
                    -(axis.default - value) / (axis.default - axis.min).max(1e-6)
                } else if value > axis.default {
                    (value - axis.default) / (axis.max - axis.default).max(1e-6)
                } else {
                    0.0
                }
                .clamp(-1.0, 1.0);
                match self.avar {
                    Some(avar) => avar_map(data, avar, i, normalized),
                    None => normalized,
                }
            })
            .collect();
    }

    /// Whether every axis sits at its default value.
    fn is_default(&self) -> bool {
        self.coords.iter().all(|c| *c == 0.0)
    }
}

/// Maps a normalized coordinate through the `avar` segment map of one axis.
fn avar_map(data: &[u8], avar: usize, axis: usize, coord: f32) -> f32 {
    let axes = u16_at(data, avar + 6) as usize;
    if axis >= axes {
        return coord;
    }
    // Segment maps are variable-length, so walk the axes before this one.
    let mut p = avar + 8;
    for i in 0..axes {
        let pairs = u16_at(data, p) as usize;
        if i == axis {
            if pairs == 0 {
                return coord;
            }
            let from = |k: usize| f2dot14(data, p + 2 + k * 4);
            let to = |k: usize| f2dot14(data, p + 4 + k * 4);
            if coord <= from(0) {
                return to(0);
            }
            for k in 1..pairs {
                if coord <= from(k) {
                    let span = from(k) - from(k - 1);
                    let f = if span.abs() < 1e-6 {
                        1.0
                    } else {
                        (coord - from(k - 1)) / span
                    };
                    return to(k - 1) + (to(k) - to(k - 1)) * f;
                }
            }
            return to(pairs - 1);
        }
        p += 2 + pairs * 4;
    }
    coord
}

/// Reads the variable fields of one COLR paint table.
///
/// Each variable field has a fixed position in the base/sequence scheme: the
/// `k`-th variable field of a table uses the mapping entry `base + k`.
struct VarFields<'a> {
    face: &'a Face,
    base: u32,
}

impl VarFields<'_> {
    fn delta(&self, k: usize) -> f32 {
        self.face.var_delta(self.base, k)
    }

    fn fword(&self, offset: usize, k: usize) -> f32 {
        fword(&self.face.data, offset) + self.delta(k)
    }

    fn f2dot14(&self, offset: usize, k: usize) -> f32 {
        f2dot14(&self.face.data, offset) + self.delta(k) / 16384.0
    }

    fn fixed(&self, offset: usize, k: usize) -> f32 {
        fixed(&self.face.data, offset) + self.delta(k) / 65536.0
    }
}

/// Base outline points of a glyph, as variation data sees them.
#[derive(Debug, Clone)]
enum GlyphPoints {
    /// A simple glyph: points plus the last index of each contour.
    Simple {
        /// On/off-curve points in font units.
        points: Vec<Point>,
        /// Inclusive end index of each contour.
        ends: Vec<usize>,
    },
    /// A composite glyph: gvar deltas refer to component offsets.
    Composite {
        /// Number of components.
        count: usize,
    },
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

        // Variable-font tables: axes, instance coordinates and the
        // per-glyph / per-metric variation data.
        let vars = Variations::parse(&data, &mut table);

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
            vars,
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

    /// Pre-declared clip box of a colour glyph, in font units as
    /// `[x_min, y_min, x_max, y_max]`.
    ///
    /// COLR v1 fonts may give a glyph an overall bounds; anything painted
    /// outside it must not render. Returns `None` when the font declares no
    /// box for this glyph.
    pub fn color_clip(&self, gid: u16) -> Option<[f32; 4]> {
        let colors = self.colors.as_ref()?;
        let list = colors.clip_list?;
        let count = u32_at(&self.data, list + 1) as usize;
        // Clip records are sorted by start glyph id.
        // Find the last record whose range starts at or before `gid`.
        let (mut lo, mut hi) = (0usize, count);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if u16_at(&self.data, list + 5 + mid * 7) <= gid {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            return None;
        }
        let record = list + 5 + (lo - 1) * 7;
        let (start, end) = (u16_at(&self.data, record), u16_at(&self.data, record + 2));
        if gid < start || gid > end {
            return None;
        }
        let box_offset = list + u24_at(&self.data, record + 4) as usize;
        let format = u8_at(&self.data, box_offset);
        if format != 1 && format != 2 {
            return None;
        }
        let var = self.var_fields(box_offset, 9, format == 2);
        // The spec asks for an expanded box: round the minimum down and the
        // maximum up, so no part of the glyph is clipped away.
        Some([
            var.fword(box_offset + 1, 0).floor(),
            var.fword(box_offset + 3, 1).floor(),
            var.fword(box_offset + 5, 2).ceil(),
            var.fword(box_offset + 7, 3).ceil(),
        ])
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
        let format = u8_at(data, offset);
        match format {
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
                // PaintVarSolid appends its varIndexBase after the alpha.
                let var = self.var_fields(offset, 5, format == 3);
                self.palette_paint(index, var.f2dot14(offset + 3, 0))
            }
            // PaintLinearGradient / PaintVarLinearGradient
            4 | 5 => {
                let is_var = format == 5;
                let var = self.var_fields(offset, 16, is_var);
                let (extend, stops) =
                    self.parse_color_line(offset + u24_at(data, offset + 1) as usize, is_var)?;
                Some(Paint::Linear {
                    p0: [var.fword(offset + 4, 0), var.fword(offset + 6, 1)],
                    p1: [var.fword(offset + 8, 2), var.fword(offset + 10, 3)],
                    p2: [var.fword(offset + 12, 4), var.fword(offset + 14, 5)],
                    stops,
                    extend,
                })
            }
            // PaintRadialGradient / PaintVarRadialGradient
            6 | 7 => {
                let is_var = format == 7;
                let var = self.var_fields(offset, 16, is_var);
                let (extend, stops) =
                    self.parse_color_line(offset + u24_at(data, offset + 1) as usize, is_var)?;
                let radius =
                    |at: usize, k: usize| (u16_at(data, at) as f32 + var.delta(k)).max(0.0);
                Some(Paint::Radial {
                    c0: [var.fword(offset + 4, 0), var.fword(offset + 6, 1)],
                    r0: radius(offset + 8, 2),
                    c1: [var.fword(offset + 10, 3), var.fword(offset + 12, 4)],
                    r1: radius(offset + 14, 5),
                    stops,
                    extend,
                })
            }
            // PaintSweepGradient / PaintVarSweepGradient
            8 | 9 => {
                let is_var = format == 9;
                let var = self.var_fields(offset, 12, is_var);
                let (extend, stops) =
                    self.parse_color_line(offset + u24_at(data, offset + 1) as usize, is_var)?;
                // Angles carry a +1.0 bias and count 180 degrees per unit.
                Some(Paint::Sweep {
                    center: [var.fword(offset + 4, 0), var.fword(offset + 6, 1)],
                    start: (var.f2dot14(offset + 8, 2) + 1.0) * std::f32::consts::PI,
                    end: (var.f2dot14(offset + 10, 3) + 1.0) * std::f32::consts::PI,
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
                let var = self.var_fields(affine, 24, format == 13);
                let matrix = [
                    var.fixed(affine, 0),
                    var.fixed(affine + 4, 1),
                    var.fixed(affine + 8, 2),
                    var.fixed(affine + 12, 3),
                    var.fixed(affine + 16, 4),
                    var.fixed(affine + 20, 5),
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
                let raw = u8_at(data, offset + 4);
                // Unrecognised modes must behave like COMPOSITE_CLEAR.
                let mode = if raw <= 27 { raw } else { 0 };
                let backdrop = offset + u24_at(data, offset + 5) as usize;
                Some(Paint::Composite {
                    mode,
                    source: Box::new(self.parse_paint(source, depth + 1)?),
                    backdrop: Box::new(self.parse_paint(backdrop, depth + 1)?),
                })
            }
            _ => None,
        }
    }

    /// Builds the affine matrix of a simple transform paint table.
    fn simple_transform(&self, offset: usize) -> Option<[f32; 6]> {
        use std::f32::consts::PI;
        let data = &self.data;
        let format = u8_at(data, offset);
        if !(14..=31).contains(&format) {
            return None;
        }
        // Even formats are the static version, odd ones carry a varIndexBase.
        let is_var = format % 2 == 1;
        let kind = (format - 14) / 2;
        // Offset of the varIndexBase field, per transform kind.
        let var_offset = [8, 8, 12, 6, 10, 6, 10, 8, 12][kind as usize];
        let var = self.var_fields(offset, var_offset, is_var);
        // Rotations and skews count 180 degrees per 1.0.
        let angle = |k: usize, at: usize| var.f2dot14(at, k) * PI;
        Some(match kind {
            // Translate
            0 => [
                1.0,
                0.0,
                0.0,
                1.0,
                var.fword(offset + 4, 0),
                var.fword(offset + 6, 1),
            ],
            // Scale
            1 => [
                var.f2dot14(offset + 4, 0),
                0.0,
                0.0,
                var.f2dot14(offset + 6, 1),
                0.0,
                0.0,
            ],
            // ScaleAroundCenter
            2 => around_center(
                [
                    var.f2dot14(offset + 8, 2),
                    0.0,
                    0.0,
                    var.f2dot14(offset + 10, 3),
                    0.0,
                    0.0,
                ],
                var.fword(offset + 4, 0),
                var.fword(offset + 6, 1),
            ),
            // ScaleUniform
            3 => {
                let s = var.f2dot14(offset + 4, 0);
                [s, 0.0, 0.0, s, 0.0, 0.0]
            }
            // ScaleUniformAroundCenter
            4 => {
                let s = var.f2dot14(offset + 8, 2);
                around_center(
                    [s, 0.0, 0.0, s, 0.0, 0.0],
                    var.fword(offset + 4, 0),
                    var.fword(offset + 6, 1),
                )
            }
            // Rotate
            5 => {
                let (sin, cos) = angle(0, offset + 4).sin_cos();
                [cos, sin, -sin, cos, 0.0, 0.0]
            }
            // RotateAroundCenter
            6 => {
                let (sin, cos) = angle(2, offset + 8).sin_cos();
                around_center(
                    [cos, sin, -sin, cos, 0.0, 0.0],
                    var.fword(offset + 4, 0),
                    var.fword(offset + 6, 1),
                )
            }
            // Skew
            7 => {
                let x = angle(0, offset + 4).tan();
                let y = angle(1, offset + 6).tan();
                [1.0, y, x, 1.0, 0.0, 0.0]
            }
            // SkewAroundCenter
            8 => {
                let x = angle(2, offset + 8).tan();
                let y = angle(3, offset + 10).tan();
                around_center(
                    [1.0, y, x, 1.0, 0.0, 0.0],
                    var.fword(offset + 4, 0),
                    var.fword(offset + 6, 1),
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
    fn parse_color_line(&self, offset: usize, is_var: bool) -> Option<(Extend, Vec<GradientStop>)> {
        let data = &self.data;
        let extend = match u8_at(data, offset) {
            1 => Extend::Repeat,
            2 => Extend::Reflect,
            _ => Extend::Pad,
        };
        let count = u16_at(data, offset + 1) as usize;
        // VarColorStop records carry a trailing varIndexBase.
        let stride = if is_var { 10 } else { 6 };
        let mut stops = Vec::with_capacity(count);
        for i in 0..count {
            let p = offset + 3 + i * stride;
            let (stop_offset, alpha) = if is_var {
                let var = self.var_fields(p, 6, true);
                (var.f2dot14(p, 0), var.f2dot14(p + 4, 1))
            } else {
                (f2dot14(data, p), f2dot14(data, p + 4))
            };
            let index = u16_at(data, p + 2);
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

    /// Reads the variable fields of one paint table, applying deltas.
    fn var_fields(&self, offset: usize, var_offset: usize, is_var: bool) -> VarFields<'_> {
        VarFields {
            face: self,
            base: if is_var {
                u32_at(&self.data, offset + var_offset)
            } else {
                u32::MAX
            },
        }
    }

    /// Variation delta for the item `k` places after `var_index_base`.
    ///
    /// Returns zero when the font has no variation data, when the index is
    /// out of range, or when the recorded index marks "no variation".
    fn var_delta(&self, var_index_base: u32, k: usize) -> f32 {
        let Some(colors) = self.colors.as_ref() else {
            return 0.0;
        };
        let Some(store) = colors.var_store else {
            return 0.0;
        };
        if var_index_base == u32::MAX {
            return 0.0;
        }
        let index = var_index_base.wrapping_add(k as u32);
        let (outer, inner) = match colors.var_map {
            Some(map) => self.map_delta_set(map, index),
            // Without a map, the value itself is an outer/inner index pair.
            None => ((index >> 16) as u16, index as u16),
        };
        if outer == 0xFFFF && inner == 0xFFFF {
            return 0.0;
        }
        self.item_delta(store, outer, inner)
    }

    /// Net `gvar` deltas for a glyph at the current instance.
    ///
    /// One entry per outline point, plus the four phantom points. For a
    /// composite glyph the entries refer to component offsets. Points the
    /// font does not mention are interpolated (IUP) from their neighbours, as
    /// the specification requires.
    fn glyph_deltas(&self, gid: u16) -> Option<Vec<[f32; 2]>> {
        let gvar = self.vars.gvar?;
        if self.vars.is_default() {
            return None;
        }
        let data = &self.data;
        let axis_count = u16_at(data, gvar + 4) as usize;
        if axis_count != self.vars.coords.len() {
            return None;
        }
        let shared_count = u16_at(data, gvar + 6) as usize;
        let shared_offset = gvar + u32_at(data, gvar + 8) as usize;
        let glyph_count = u16_at(data, gvar + 12) as usize;
        if gid as usize >= glyph_count {
            return None;
        }
        let flags = u16_at(data, gvar + 14);
        let array = gvar + u32_at(data, gvar + 16) as usize;
        let offsets = gvar + 20;
        let (start, end) = if flags & 1 != 0 {
            (
                u32_at(data, offsets + gid as usize * 4) as usize,
                u32_at(data, offsets + gid as usize * 4 + 4) as usize,
            )
        } else {
            (
                u16_at(data, offsets + gid as usize * 2) as usize * 2,
                u16_at(data, offsets + gid as usize * 2 + 2) as usize * 2,
            )
        };
        if end <= start {
            return None;
        }
        let table = array.checked_add(start)?;
        let shape = self.outline_points(gid)?;
        let point_count = match &shape {
            GlyphPoints::Simple { points, .. } => points.len(),
            GlyphPoints::Composite { count } => *count,
        };
        let total = point_count + 4;
        let tuple_header = u16_at(data, table);
        let count = (tuple_header & 0x0FFF) as usize;
        let mut header = table + 4;
        let mut data_pos = table + u16_at(data, table + 2) as usize;
        // Shared point numbers live at the start of the serialized data.
        let shared_points = if tuple_header & 0x8000 != 0 {
            let (points, next) = self.read_points(data, data_pos, total)?;
            data_pos = next;
            Some(points)
        } else {
            None
        };
        let mut net = vec![[0.0f32; 2]; total];
        let mut any = false;
        for _ in 0..count {
            let size = u16_at(data, header) as usize;
            let tuple_index = u16_at(data, header + 2);
            let mut p = header + 4;
            let has_peak = tuple_index & 0x8000 != 0;
            let intermediate = tuple_index & 0x4000 != 0;
            let private_points = tuple_index & 0x2000 != 0;
            let peak: Vec<f32> = if has_peak {
                let values = (0..axis_count).map(|a| f2dot14(data, p + a * 2)).collect();
                p += axis_count * 2;
                values
            } else {
                let index = (tuple_index & 0x0FFF) as usize;
                if index >= shared_count {
                    return None;
                }
                (0..axis_count)
                    .map(|a| f2dot14(data, shared_offset + (index * axis_count + a) * 2))
                    .collect()
            };
            let (start_tuple, end_tuple) = if intermediate {
                let start = (0..axis_count)
                    .map(|a| f2dot14(data, p + a * 2))
                    .collect::<Vec<f32>>();
                p += axis_count * 2;
                let end = (0..axis_count)
                    .map(|a| f2dot14(data, p + a * 2))
                    .collect::<Vec<f32>>();
                p += axis_count * 2;
                (start, end)
            } else {
                let start = peak.iter().map(|v| v.min(0.0)).collect();
                let end = peak.iter().map(|v| v.max(0.0)).collect();
                (start, end)
            };
            header = p;
            let scalar = tuple_scalar(&self.vars.coords, &peak, &start_tuple, &end_tuple);
            let tuple_data = data_pos;
            data_pos += size;
            if scalar == 0.0 {
                continue;
            }
            let mut q = tuple_data;
            let points = if private_points {
                let (points, next) = self.read_points(data, q, total)?;
                q = next;
                points
            } else {
                match &shared_points {
                    Some(points) => points.clone(),
                    None => (0..total).collect(),
                }
            };
            let (xs, next) = self.read_deltas(data, q, points.len())?;
            let (ys, _) = self.read_deltas(data, next, points.len())?;
            if xs.len() != points.len() || ys.len() != points.len() {
                return None;
            }
            let mut dx = vec![0.0f32; point_count];
            let mut dy = vec![0.0f32; point_count];
            let mut touched = vec![false; point_count];
            for (i, &point) in points.iter().enumerate() {
                if point < point_count {
                    dx[point] += xs[i] * scalar;
                    dy[point] += ys[i] * scalar;
                    touched[point] = true;
                } else if point < total {
                    // Phantom points carry the side bearings and the advance.
                    let k = point - point_count;
                    net[point_count + k][0] += xs[i] * scalar;
                    net[point_count + k][1] += ys[i] * scalar;
                }
            }
            match &shape {
                GlyphPoints::Simple { points, ends } => {
                    let base = points.clone();
                    interpolate_contours(&mut dx, &mut dy, &touched, &base, ends);
                }
                GlyphPoints::Composite { .. } => {}
            }
            for i in 0..point_count {
                net[i][0] += dx[i];
                net[i][1] += dy[i];
                any |= dx[i] != 0.0 || dy[i] != 0.0;
            }
        }
        if any { Some(net) } else { None }
    }

    /// Base points of a glyph, before variation deltas.
    fn outline_points(&self, gid: u16) -> Option<GlyphPoints> {
        let (start, end) = self.glyph_range(gid)?;
        let n_contours = i16_at(&self.data, start);
        if n_contours < 0 {
            let mut p = start + 10;
            let mut count = 0usize;
            loop {
                if p + 4 > end {
                    break;
                }
                let flags = u16_at(&self.data, p);
                p += 4;
                p += if flags & 0x0001 != 0 { 4 } else { 2 };
                if flags & 0x0008 != 0 {
                    p += 2;
                } else if flags & 0x0040 != 0 {
                    p += 4;
                } else if flags & 0x0080 != 0 {
                    p += 8;
                }
                count += 1;
                if flags & 0x0020 == 0 {
                    break;
                }
            }
            return Some(GlyphPoints::Composite { count });
        }
        let n_contours = n_contours as usize;
        let mut p = start + 10;
        let mut ends = Vec::with_capacity(n_contours);
        for _ in 0..n_contours {
            ends.push(u16_at(&self.data, p) as usize);
            p += 2;
        }
        let n_points = ends.last().map(|e| e + 1).unwrap_or(0);
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
        let mut points = vec![Point { x: 0.0, y: 0.0 }; n_points];
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
            points[i].x = x as f32;
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
            points[i].y = y as f32;
        }
        Some(GlyphPoints::Simple { points, ends })
    }

    /// Reads packed point numbers, returning them with the next offset.
    fn read_points(&self, data: &[u8], offset: usize, limit: usize) -> Option<(Vec<usize>, usize)> {
        let mut p = offset;
        let first = u8_at(data, p);
        p += 1;
        let count = if first == 0 {
            // Zero means "all points", which the caller supplies itself.
            return Some(((0..limit).collect(), p));
        } else if first & 0x80 != 0 {
            // Two-byte count: the high bit is a flag, not part of the count.
            (((first & 0x7F) as usize) << 8) | u8_at(data, p) as usize
        } else {
            first as usize
        };
        let mut points = Vec::with_capacity(count);
        let mut last = 0usize;
        while points.len() < count {
            let control = u8_at(data, p);
            p += 1;
            let words = control & 0x80 != 0;
            let run = (control & 0x7F) as usize + 1;
            for _ in 0..run {
                if points.len() >= count {
                    break;
                }
                if words {
                    last += u16_at(data, p) as usize;
                    p += 2;
                } else {
                    last += u8_at(data, p) as usize;
                    p += 1;
                }
                points.push(last);
            }
        }
        Some((points, p))
    }

    /// Reads packed deltas, returning them with the next offset.
    fn read_deltas(&self, data: &[u8], offset: usize, count: usize) -> Option<(Vec<f32>, usize)> {
        let mut p = offset;
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let control = u8_at(data, p);
            p += 1;
            let zero = control & 0x80 != 0;
            let words = control & 0x40 != 0;
            let run = (control & 0x3F) as usize + 1;
            for _ in 0..run {
                if out.len() >= count {
                    break;
                }
                if zero {
                    out.push(0.0);
                } else if words {
                    out.push(i16_at(data, p) as f32);
                    p += 2;
                } else {
                    out.push(u8_at(data, p) as i8 as f32);
                    p += 1;
                }
            }
        }
        Some((out, p))
    }

    /// Resolves a delta-set index through a DeltaSetIndexMap.
    fn map_delta_set(&self, map: usize, index: u32) -> (u16, u16) {
        let data = &self.data;
        let format = u8_at(data, map);
        let entry_format = u8_at(data, map + 1);
        let (count, data_offset) = if format == 0 {
            (u16_at(data, map + 2) as u32, map + 4)
        } else {
            (u32_at(data, map + 2), map + 6)
        };
        if count == 0 {
            return (0xFFFF, 0xFFFF);
        }
        // Out-of-range indices use the last entry.
        let index = index.min(count - 1) as usize;
        let entry_size = ((entry_format & 0x30) >> 4) as usize + 1;
        let inner_bits = ((entry_format & 0x0F) as u32) + 1;
        let mut entry = 0u32;
        for i in 0..entry_size {
            entry = (entry << 8) | u8_at(data, data_offset + index * entry_size + i) as u32;
        }
        let inner = (entry & ((1u32 << inner_bits) - 1)) as u16;
        let outer = (entry >> inner_bits) as u16;
        (outer, inner)
    }

    /// Sums the scaled deltas of one item variation store delta set.
    fn item_delta(&self, store: usize, outer: u16, inner: u16) -> f32 {
        let data = &self.data;
        let count = u16_at(data, store + 6);
        if outer >= count {
            return 0.0;
        }
        let sub = store + u32_at(data, store + 8 + outer as usize * 4) as usize;
        if sub == store {
            // NULL offset: this subtable has no data.
            return 0.0;
        }
        let item_count = u16_at(data, sub);
        if inner >= item_count {
            return 0.0;
        }
        let word_field = u16_at(data, sub + 2);
        let long_words = word_field & 0x8000 != 0;
        let word_count = (word_field & 0x7FFF) as usize;
        let region_count = u16_at(data, sub + 4) as usize;
        let regions = sub + 6;
        let row_size = if long_words {
            2 * (region_count + word_count)
        } else {
            region_count + word_count
        };
        let mut p = regions + region_count * 2 + inner as usize * row_size;
        let mut total = 0.0;
        for r in 0..region_count {
            let delta = if r < word_count && long_words {
                let v = i32::from_be_bytes([
                    u8_at(data, p),
                    u8_at(data, p + 1),
                    u8_at(data, p + 2),
                    u8_at(data, p + 3),
                ]) as f32;
                p += 4;
                v
            } else if r < word_count || long_words {
                let v = i16_at(data, p) as f32;
                p += 2;
                v
            } else {
                let v = u8_at(data, p) as i8 as f32;
                p += 1;
                v
            };
            if delta == 0.0 {
                continue;
            }
            let region = u16_at(data, regions + r * 2);
            total += delta * self.region_scalar(store, region);
        }
        total
    }

    /// Scalar weight of one variation region at the current instance.
    fn region_scalar(&self, store: usize, region: u16) -> f32 {
        let data = &self.data;
        let list = store + u32_at(data, store + 2) as usize;
        let axis_count = u16_at(data, list) as usize;
        let region_count = (u16_at(data, list + 2) & 0x7FFF) as usize;
        if region as usize >= region_count || axis_count > self.vars.coords.len() {
            return 0.0;
        }
        let base = list + 4 + region as usize * axis_count * 6;
        let mut scalar = 1.0;
        for axis in 0..axis_count {
            let p = base + axis * 6;
            let peak = f2dot14(data, p + 2);
            let coord = self.vars.coords[axis];
            if peak == 0.0 {
                continue;
            }
            if coord == peak {
                continue;
            }
            let start = f2dot14(data, p);
            let end = f2dot14(data, p + 4);
            if coord <= start || coord >= end {
                return 0.0;
            }
            scalar *= if coord < peak {
                (coord - start) / (peak - start)
            } else {
                (end - coord) / (end - peak)
            };
        }
        scalar
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

    /// Horizontal advance in font units at the current instance.
    ///
    /// Layout keeps using the deterministic metric table, but raster text is
    /// positioned with the font's own advances — including the `HVAR` delta
    /// when the font is interpolated.
    pub fn advance(&self, gid: u16) -> f32 {
        if self.num_h_metrics == 0 {
            return 0.0;
        }
        let idx = gid.min(self.num_h_metrics - 1) as usize;
        u16_at(&self.data, self.hmtx + idx * 4) as f32 + self.advance_delta(gid)
    }

    /// `HVAR` advance-width delta for `gid` at the current instance.
    fn advance_delta(&self, gid: u16) -> f32 {
        if self.vars.is_default() {
            return 0.0;
        }
        if let Some(hvar) = self.vars.hvar {
            let store = hvar + u32_at(&self.data, hvar + 4) as usize;
            let map = u32_at(&self.data, hvar + 8);
            // A missing map means the glyph id is the delta-set index itself.
            let (outer, inner) = if map == 0 {
                (0, gid)
            } else {
                self.map_delta_set(hvar + map as usize, gid as u32)
            };
            return self.item_delta(store, outer, inner);
        }
        // Without `HVAR`, the gvar phantom points give the same answer:
        // advance = right side bearing - left side bearing.
        let Some(deltas) = self.glyph_deltas(gid) else {
            return 0.0;
        };
        let Some(points) = deltas.len().checked_sub(4) else {
            return 0.0;
        };
        deltas[points + 1][0] - deltas[points][0]
    }

    /// Variation axes of the face, in font order.
    ///
    /// Empty for a static font.
    pub fn axes(&self) -> &[Axis] {
        &self.vars.axes
    }

    /// Design-space values currently selected, one per axis.
    pub fn axis_values(&self) -> &[f32] {
        &self.vars.values
    }

    /// Selects a variable-font instance.
    ///
    /// `settings` are `(tag, value)` pairs; unknown tags are ignored and
    /// values are clamped to the axis range.
    pub fn set_variations(&mut self, settings: &[(String, f32)]) {
        self.vars.apply(&self.data, settings);
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
        // Interpolated points move the outline; a static instance has none.
        let deltas = self.glyph_deltas(gid);
        if n_contours >= 0 {
            self.simple_glyph(
                start,
                n_contours as usize,
                deltas.as_deref(),
                xf,
                flatness,
                out,
            );
        } else {
            self.composite_glyph(start, deltas.as_deref(), xf, flatness, depth, out);
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
        deltas: Option<&[[f32; 2]]>,
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

        if let Some(deltas) = deltas {
            for (i, point) in coords.iter_mut().enumerate() {
                if let Some(delta) = deltas.get(i) {
                    point.x += delta[0];
                    point.y += delta[1];
                }
            }
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
        deltas: Option<&[[f32; 2]]>,
        mut xf: Transform,
        flatness: f32,
        depth: u32,
        out: &mut Vec<Contour>,
    ) {
        let mut p = start + 10;
        let mut component = 0usize;
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
                // gvar deltas for a composite move individual components.
                if let Some(delta) = deltas.and_then(|d| d.get(component)) {
                    local.dx += delta[0];
                    local.dy += delta[1];
                }
            }

            self.outline_into(gid, xf.then(local), flatness, depth + 1, out);
            component += 1;

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
    let version = u16_at(data, colr);
    if version > 1 {
        return None;
    }
    let num_base = u16_at(data, colr + 2);
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
    // COLR v1 adds a paint-graph list, clip boxes and variation data;
    // v0-only fonts leave these empty.
    let (base_list, layer_list, clip_list, var_store, var_map) = if version >= 1 {
        let base = u32_at(data, colr + 14) as usize;
        let layers = u32_at(data, colr + 18) as usize;
        let clips = u32_at(data, colr + 22) as usize;
        let map = u32_at(data, colr + 26) as usize;
        let store = u32_at(data, colr + 30) as usize;
        let base_list = if base != 0 { Some(colr + base) } else { None };
        (
            base_list,
            if layers != 0 {
                Some(colr + layers)
            } else {
                None
            },
            if clips != 0 { Some(colr + clips) } else { None },
            if store != 0 { Some(colr + store) } else { None },
            if map != 0 { Some(colr + map) } else { None },
        )
    } else {
        (None, None, None, None, None)
    };
    if num_base == 0 && base_list.is_none() {
        return None;
    }
    Some(ColorGlyphs {
        base_records,
        num_base,
        layer_records,
        palette,
        base_list,
        layer_list,
        clip_list,
        var_store,
        var_map,
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

/// Reads a four-character tag as a string.
fn tag_at(data: &[u8], offset: usize) -> String {
    let bytes = [
        u8_at(data, offset),
        u8_at(data, offset + 1),
        u8_at(data, offset + 2),
        u8_at(data, offset + 3),
    ];
    String::from_utf8_lossy(&bytes).to_string()
}

/// Scalar weight of one tuple variation region at `coords`.
fn tuple_scalar(coords: &[f32], peak: &[f32], start: &[f32], end: &[f32]) -> f32 {
    let mut scalar = 1.0;
    for i in 0..coords.len().min(peak.len()) {
        let coord = coords[i];
        let peak = peak[i];
        if peak == 0.0 {
            continue;
        }
        if coord == peak {
            continue;
        }
        let (lo, hi) = (start[i], end[i]);
        if coord <= lo || coord >= hi {
            return 0.0;
        }
        scalar *= if coord < peak {
            (coord - lo) / (peak - lo)
        } else {
            (hi - coord) / (hi - peak)
        };
    }
    scalar
}

/// Fills in deltas for points a tuple does not mention (IUP).
fn interpolate_contours(
    dx: &mut [f32],
    dy: &mut [f32],
    touched: &[bool],
    points: &[Point],
    ends: &[usize],
) {
    let mut first = 0usize;
    for &end in ends {
        if end < first || end >= points.len() {
            first = end.saturating_add(1);
            continue;
        }
        let range = first..=end;
        let touched_indices: Vec<usize> = range.clone().filter(|i| touched[*i]).collect();
        match touched_indices.len() {
            0 => {}
            1 => {
                let only = touched_indices[0];
                for i in range {
                    if i != only {
                        dx[i] = dx[only];
                        dy[i] = dy[only];
                    }
                }
            }
            _ => {
                let n = touched_indices.len();
                for i in range {
                    if touched[i] {
                        continue;
                    }
                    // Nearest touched points before and after, wrapping around.
                    let (mut prev, mut next) = (0usize, 0usize);
                    for (k, &t) in touched_indices.iter().enumerate() {
                        if t < i {
                            prev = k;
                        } else if t > i {
                            next = k;
                            break;
                        }
                    }
                    let p1 = touched_indices[prev];
                    let p2 = touched_indices[(next) % n];
                    if p1 == i || p2 == i || p1 == p2 {
                        continue;
                    }
                    dx[i] = iup_axis(points[i].x, points[p1].x, dx[p1], points[p2].x, dx[p2]);
                    dy[i] = iup_axis(points[i].y, points[p1].y, dy[p1], points[p2].y, dy[p2]);
                }
            }
        }
        first = end + 1;
    }
}

/// Interpolates one coordinate axis between two touched points.
///
/// `c1`/`c2` are the deltas of the reference points at coordinates `x1`/`x2`.
fn iup_axis(coord: f32, x1: f32, c1: f32, x2: f32, c2: f32) -> f32 {
    if x1 == x2 {
        // Coincident references cannot be interpolated: use the nearest.
        if c1 == c2 || (coord - x1).abs() <= (coord - x2).abs() {
            c1
        } else {
            c2
        }
    } else if coord <= x1.min(x2) || coord >= x1.max(x2) {
        if (coord - x1).abs() < (coord - x2).abs() {
            c1
        } else {
            c2
        }
    } else {
        c1 + (c2 - c1) * (coord - x1) / (x2 - x1)
    }
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
                Paint::Linear { .. } | Paint::Radial { .. } | Paint::Sweep { .. } => {
                    *gradients += 1
                }
                Paint::Solid { .. } => *solids += 1,
                Paint::Layers(items) => {
                    for item in items {
                        walk(item, gradients, solids, depth + 1);
                    }
                }
                Paint::Glyph { paint, .. } | Paint::Transform { paint, .. } => {
                    walk(paint, gradients, solids, depth + 1);
                }
                Paint::Composite {
                    source, backdrop, ..
                } => {
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

    #[test]
    fn tuple_scalar_interpolates_between_start_and_peak() {
        // Non-intermediate region: peak +1, start 0, end +1.
        let peak = [1.0];
        let start = [0.0];
        let end = [1.0];
        assert_eq!(tuple_scalar(&[0.0], &peak, &start, &end), 0.0);
        assert_eq!(tuple_scalar(&[1.0], &peak, &start, &end), 1.0);
        assert_eq!(tuple_scalar(&[0.5], &peak, &start, &end), 0.5);
        // Outside the region the tuple does not apply.
        assert_eq!(tuple_scalar(&[-0.5], &peak, &start, &end), 0.0);
        // An axis whose peak is zero does not restrict the region.
        assert_eq!(tuple_scalar(&[0.0], &[0.0], &[-1.0], &[1.0]), 1.0);
        // Two axes multiply their scalars.
        let scalar = tuple_scalar(&[0.5, 0.5], &[1.0, 1.0], &[0.0, 0.0], &[1.0, 1.0]);
        assert_eq!(scalar, 0.25);
        // Intermediate regions ramp on both sides of the peak.
        let scalar = tuple_scalar(&[0.2], &[0.5], &[0.0], &[1.0]);
        assert!((scalar - 0.4).abs() < 1e-6);
        let scalar = tuple_scalar(&[0.8], &[0.5], &[0.0], &[1.0]);
        assert!((scalar - 0.4).abs() < 1e-6);
    }

    #[test]
    fn iup_interpolates_between_touched_points() {
        // Two reference points at x=0 and x=100 with deltas 0 and 20.
        assert_eq!(iup_axis(50.0, 0.0, 0.0, 100.0, 20.0), 10.0);
        // Inside the span but past the reference: still linear.
        assert_eq!(iup_axis(0.0, 0.0, 0.0, 100.0, 20.0), 0.0);
        assert_eq!(iup_axis(100.0, 0.0, 0.0, 100.0, 20.0), 20.0);
        // Outside the span, the nearest reference wins.
        assert_eq!(iup_axis(-10.0, 0.0, 5.0, 100.0, 20.0), 5.0);
        assert_eq!(iup_axis(110.0, 0.0, 5.0, 100.0, 20.0), 20.0);
        // Coincident references cannot interpolate: the first wins.
        assert_eq!(iup_axis(3.0, 5.0, 1.0, 5.0, 2.0), 1.0);
        assert_eq!(iup_axis(5.0, 5.0, 1.0, 5.0, 1.0), 1.0);
    }

    #[test]
    fn iup_fills_unmentioned_points_of_a_contour() {
        // A pentagon with the left and right extremes touched: everything in
        // between is interpolated, per axis, from its neighbours.
        let points = vec![
            Point { x: 0.0, y: 5.0 },
            Point { x: 5.0, y: 0.0 },
            Point { x: 10.0, y: 5.0 },
            Point { x: 5.0, y: 10.0 },
        ];
        let mut dx = vec![0.0, 0.0, 20.0, 0.0];
        let mut dy = vec![0.0, 0.0, 0.0, 0.0];
        let touched = vec![true, false, true, false];
        interpolate_contours(&mut dx, &mut dy, &touched, &points, &[3]);
        // x = 5 sits halfway between the references, so it gets half of 20.
        assert_eq!(dx, vec![0.0, 10.0, 20.0, 10.0]);
        assert_eq!(dy, vec![0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_contour_with_one_touched_point_moves_as_a_whole() {
        let points = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 10.0, y: 0.0 },
            Point { x: 10.0, y: 10.0 },
        ];
        let mut dx = vec![7.0, 0.0, 0.0];
        let mut dy = vec![-3.0, 0.0, 0.0];
        let touched = vec![true, false, false];
        interpolate_contours(&mut dx, &mut dy, &touched, &points, &[2]);
        assert_eq!(dx, vec![7.0, 7.0, 7.0]);
        assert_eq!(dy, vec![-3.0, -3.0, -3.0]);
    }

    #[test]
    fn avar_maps_normalized_coordinates() {
        // One axis, with a +0.5 -> +0.75 segment mid-point.
        let mut data = vec![0u8; 6];
        data.extend_from_slice(&1u16.to_be_bytes()); // axisCount
        data.extend_from_slice(&4u16.to_be_bytes()); // positionMapCount
        for (from, to) in [(-1.0f32, -1.0f32), (0.0, 0.0), (0.5, 0.75), (1.0, 1.0)] {
            data.extend_from_slice(&((from * 16384.0) as i16).to_be_bytes());
            data.extend_from_slice(&((to * 16384.0) as i16).to_be_bytes());
        }
        let at = 0;
        assert_eq!(avar_map(&data, at, 0, 0.0), 0.0);
        assert_eq!(avar_map(&data, at, 0, 0.5), 0.75);
        assert!((avar_map(&data, at, 0, 0.25) - 0.375).abs() < 1e-4);
        // Between the last two pairs the map keeps interpolating.
        assert!((avar_map(&data, at, 0, 0.9) - 0.95).abs() < 1e-4);
        // Beyond the last pair it clamps.
        assert_eq!(avar_map(&data, at, 0, 1.5), 1.0);
        // Unknown axes are left alone.
        assert_eq!(avar_map(&data, at, 3, 0.4), 0.4);
    }

    #[test]
    fn axis_values_clamp_to_their_range() {
        let axis = Axis {
            tag: "wght".into(),
            min: 100.0,
            default: 400.0,
            max: 900.0,
        };
        assert_eq!(axis.clamp(50.0), 100.0);
        assert_eq!(axis.clamp(1200.0), 900.0);
        assert_eq!(axis.clamp(700.0), 700.0);
        // A nonsense range falls back to the default.
        let broken = Axis {
            min: 900.0,
            max: 100.0,
            ..axis
        };
        assert_eq!(broken.clamp(500.0), 400.0);
    }
}
