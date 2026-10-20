//! Software rasterizer: [`Scene`] → RGBA pixels.
//!
//! This is the shared back end for the PNG and WebP engines. It is written
//! from scratch: a non-zero-winding scanline filler with analytic horizontal
//! coverage and 4× vertical supersampling, plus a glyph renderer that feeds
//! TrueType outlines through the same filler.
//!
//! Text is positioned with the *layout engine's* advances, so a raster page
//! and its SVG counterpart break lines at exactly the same places.

use crate::color::Color;
use crate::font::{Contour, Extend, GradientStop, LoadedFonts, Paint as ColorPaint, Point};
use crate::scene::{Draw, FontKind, LineDraw, RectDraw, Scene, TextDraw};
use crate::text::{Metrics, TextStyle};

/// Number of sub-scanlines per pixel row (vertical anti-aliasing).
const SUBSAMPLES: usize = 4;

/// An RGBA image with straight (non-premultiplied) alpha.
#[derive(Debug, Clone, PartialEq)]
pub struct Bitmap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Row-major RGBA bytes, `width * height * 4` long.
    pub data: Vec<u8>,
}

impl Bitmap {
    /// Allocates a transparent bitmap.
    pub fn new(width: u32, height: u32) -> Bitmap {
        Bitmap {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// Allocates a bitmap filled with one opaque color.
    pub fn filled(width: u32, height: u32, color: Color) -> Bitmap {
        let mut bmp = Bitmap::new(width, height);
        for px in bmp.data.as_chunks_mut::<4>().0 {
            *px = [color.r, color.g, color.b, color.a];
        }
        bmp
    }

    /// Returns the RGBA value at `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Source-over composite of `color` at `coverage` (`0.0..=1.0`).
    fn blend(&mut self, x: usize, y: usize, color: Color, coverage: f32) {
        if x >= self.width as usize || y >= self.height as usize {
            return;
        }
        let sa = (color.a as f32 / 255.0) * coverage.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let i = (y * self.width as usize + x) * 4;
        let da = self.data[i + 3] as f32 / 255.0;
        let out_a = sa + da * (1.0 - sa);
        if out_a <= 0.0 {
            return;
        }
        for (c, channel) in [color.r, color.g, color.b].iter().enumerate() {
            let src = *channel as f32;
            let dst = self.data[i + c] as f32;
            // Straight-alpha "source over".
            let value = (src * sa + dst * da * (1.0 - sa)) / out_a;
            self.data[i + c] = value.round().clamp(0.0, 255.0) as u8;
        }
        self.data[i + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

/// Rasterizes scenes at a given scale.
#[derive(Debug, Clone, Copy)]
pub struct Rasterizer {
    scale: f32,
}

impl Rasterizer {
    /// Creates a rasterizer; `scale` is the zoom factor (`1.0` = 100%).
    pub fn new(scale: f32) -> Rasterizer {
        Rasterizer {
            scale: scale.max(0.01),
        }
    }

    /// Renders a scene into a bitmap.
    pub fn rasterize(&self, scene: &Scene, fonts: &LoadedFonts) -> Bitmap {
        let width = (scene.width * self.scale).round().max(1.0) as u32;
        let height = (scene.height * self.scale).round().max(1.0) as u32;
        let mut bmp = match scene.background.as_ref().and_then(|r| r.fill) {
            Some(bg) => Bitmap::filled(width, height, bg),
            None => Bitmap::new(width, height),
        };
        for draw in &scene.draws {
            match draw {
                Draw::Rect(r) => self.draw_rect(&mut bmp, r),
                Draw::Line(l) => self.draw_line(&mut bmp, l),
                Draw::Text(t) => self.draw_text(&mut bmp, t, fonts),
            }
        }
        bmp
    }

    fn draw_rect(&self, bmp: &mut Bitmap, r: &RectDraw) {
        let x = r.x * self.scale;
        let y = r.y * self.scale;
        let w = r.w * self.scale;
        let h = r.h * self.scale;
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let radius = (r.radius * self.scale).min(w.min(h) * 0.5).max(0.0);
        let poly = rounded_rect(x, y, w, h, radius);
        if let Some(fill) = r.fill {
            fill_polygons(bmp, std::slice::from_ref(&poly), &Fill::Solid(fill));
        }
        if let Some(stroke) = r.stroke {
            let sw = r.stroke_width * self.scale;
            stroke_polyline(bmp, &poly, sw, stroke);
        }
    }

    fn draw_line(&self, bmp: &mut Bitmap, l: &LineDraw) {
        let p0 = Point {
            x: l.x1 * self.scale,
            y: l.y1 * self.scale,
        };
        let p1 = Point {
            x: l.x2 * self.scale,
            y: l.y2 * self.scale,
        };
        let width = (l.width * self.scale).max(0.35);
        let dx = p1.x - p0.x;
        let dy = p1.y - p0.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            return;
        }
        let nx = -dy / len * width * 0.5;
        let ny = dx / len * width * 0.5;
        let quad = vec![
            Point {
                x: p0.x + nx,
                y: p0.y + ny,
            },
            Point {
                x: p1.x + nx,
                y: p1.y + ny,
            },
            Point {
                x: p1.x - nx,
                y: p1.y - ny,
            },
            Point {
                x: p0.x - nx,
                y: p0.y - ny,
            },
        ];
        fill_polygons(bmp, &[quad], &Fill::Solid(l.color));
    }

    fn draw_text(&self, bmp: &mut Bitmap, t: &TextDraw, fonts: &LoadedFonts) {
        if fonts.is_empty() || t.text.is_empty() || t.size <= 0.0 {
            return;
        }
        let metrics = Metrics::new();
        let style = TextStyle {
            size: t.size,
            family: t.family,
            bold: t.bold,
            italic: t.italic,
            letter_spacing: if t.size > 0.0 {
                t.letter_spacing / t.size
            } else {
                0.0
            },
        };
        let mono = t.family == FontKind::Mono;
        let size_px = t.size * self.scale;
        let baseline = t.y * self.scale;
        let mut pen = t.x * self.scale;
        // Groups of outlines sharing one paint, drawn in order. Colour glyphs
        // contribute one group per COLR layer; ordinary text contributes one
        // group with the text colour.
        let mut groups: Vec<(Vec<Contour>, Fill)> = Vec::new();
        // Faux-bold offset, used only when the font has no bold design.
        let bold_dx = (size_px * 0.055).max(0.45);

        for ch in t.text.chars() {
            let face = fonts
                .find(ch, mono, t.bold, t.italic)
                // Fall back to .notdef so unsupported characters stay visible.
                .or_else(|| fonts.sans.first().or_else(|| fonts.mono.first()));
            let mut advance = None;
            if let Some(face) = face {
                let gid = if face.has_glyph(ch) {
                    face.glyph_id(ch)
                } else {
                    0
                };
                let k = size_px / face.units_per_em();
                if !ch.is_whitespace() && gid != 0 {
                    let flatness = (0.35 / k).max(0.5);
                    if let Some(paint) = face.color_paint(gid) {
                        // Draw the font's own paint graph: exact outlines, exact
                        // palette, gradients and transforms. No synthesis.
                        self.emit_paint(
                            &mut groups,
                            face,
                            &paint,
                            Affine::IDENTITY,
                            pen,
                            baseline,
                            k,
                            flatness,
                            t.color,
                            0,
                        );
                    } else {
                        // Synthesise weight/slant only when the face itself is
                        // not the requested design.
                        let shear = t.italic && !face.is_italic();
                        let bold = t.bold && !face.is_bold();
                        let mut polys = Vec::new();
                        for contour in face.outline(gid, flatness) {
                            push_contour(&mut polys, &contour, pen, baseline, k, shear, 0.0);
                            if bold {
                                push_contour(
                                    &mut polys, &contour, pen, baseline, k, shear, bold_dx,
                                );
                            }
                        }
                        if !polys.is_empty() {
                            groups.push((polys, Fill::Solid(t.color)));
                        }
                    }
                }
                if face.has_glyph(ch) {
                    // Use the font's own advance here: this is what a browser
                    // does for the SVG output, so raster and vector text end up
                    // with the same spacing. Layout still uses `Metrics`.
                    advance = Some(face.advance(gid) * k);
                }
            }
            pen += advance.unwrap_or_else(|| metrics.advance(ch, &style) * self.scale);
        }
        for (polys, fill) in groups {
            fill_polygons(bmp, &polys, &fill);
        }
    }

    /// Walks a COLR paint graph and appends filled regions to `out`.
    ///
    /// `xf` accumulates COLR transforms (in font units); glyph outlines are
    /// transformed before being mapped to pixels, so the font's geometry is
    /// preserved exactly.
    #[allow(clippy::too_many_arguments)]
    fn emit_paint(
        &self,
        out: &mut Vec<(Vec<Contour>, Fill)>,
        face: &crate::font::Face,
        paint: &ColorPaint,
        xf: Affine,
        pen: f32,
        baseline: f32,
        k: f32,
        flatness: f32,
        foreground: Color,
        depth: u32,
    ) {
        if depth > 24 {
            return;
        }
        match paint {
            ColorPaint::Layers(layers) => {
                for layer in layers {
                    self.emit_paint(
                        out,
                        face,
                        layer,
                        xf,
                        pen,
                        baseline,
                        k,
                        flatness,
                        foreground,
                        depth + 1,
                    );
                }
            }
            ColorPaint::Transform { matrix, paint } => {
                self.emit_paint(
                    out,
                    face,
                    paint,
                    xf.compose(*matrix),
                    pen,
                    baseline,
                    k,
                    flatness,
                    foreground,
                    depth + 1,
                );
            }
            ColorPaint::Composite { source, backdrop } => {
                self.emit_paint(
                    out,
                    face,
                    backdrop,
                    xf,
                    pen,
                    baseline,
                    k,
                    flatness,
                    foreground,
                    depth + 1,
                );
                self.emit_paint(
                    out,
                    face,
                    source,
                    xf,
                    pen,
                    baseline,
                    k,
                    flatness,
                    foreground,
                    depth + 1,
                );
            }
            ColorPaint::Glyph { glyph, paint } => {
                let Some(fill) = resolve_fill(paint, xf, pen, baseline, k, foreground) else {
                    return;
                };
                let mut polys = Vec::new();
                for contour in face.outline(*glyph, flatness) {
                    let transformed: Contour = contour.iter().map(|p| xf.apply(*p)).collect();
                    push_contour(&mut polys, &transformed, pen, baseline, k, false, 0.0);
                }
                if !polys.is_empty() {
                    out.push((polys, fill));
                }
            }
            // A solid or gradient alone has no shape to fill.
            ColorPaint::Solid { .. }
            | ColorPaint::Foreground
            | ColorPaint::Linear { .. }
            | ColorPaint::Radial { .. } => {}
        }
    }
}

/// Turns a paint into a fill, descending through transforms.
///
/// `xf` accumulates the transforms that wrap the paint, so both the glyph
/// outline and the gradient geometry go through the same affine.
fn resolve_fill(
    paint: &ColorPaint,
    xf: Affine,
    pen: f32,
    baseline: f32,
    k: f32,
    foreground: Color,
) -> Option<Fill> {
    let to_px = |p: [f32; 2]| {
        let q = xf.apply(Point { x: p[0], y: p[1] });
        Point {
            x: pen + q.x * k,
            y: baseline - q.y * k,
        }
    };
    Some(match paint {
        ColorPaint::Solid { color } => Fill::Solid(*color),
        ColorPaint::Foreground => Fill::Solid(foreground),
        ColorPaint::Transform { matrix, paint } => {
            return resolve_fill(paint, xf.compose(*matrix), pen, baseline, k, foreground);
        }
        ColorPaint::Linear {
            p0,
            p1,
            stops,
            extend,
        } => Fill::Linear {
            p0: to_px(*p0),
            p1: to_px(*p1),
            stops: stops.clone(),
            extend: *extend,
        },
        ColorPaint::Radial {
            c0,
            r0,
            c1,
            r1,
            stops,
            extend,
        } => Fill::Radial {
            c0: to_px(*c0),
            r0: r0 * xf.scale() * k,
            c1: to_px(*c1),
            r1: r1 * xf.scale() * k,
            stops: stops.clone(),
            extend: *extend,
        },
        _ => return None,
    })
}

/// Transforms one glyph contour from font units into pixel space.
///
/// `k` is pixels per font unit; italic applies a shear in font space so the
/// slant is proportional to the glyph height.
#[allow(clippy::too_many_arguments)]
fn push_contour(
    out: &mut Vec<Contour>,
    contour: &Contour,
    pen_x: f32,
    baseline: f32,
    k: f32,
    italic: bool,
    dx: f32,
) {
    let shear = if italic { 0.21 } else { 0.0 };
    let mut poly = Vec::with_capacity(contour.len());
    for p in contour {
        let gx = p.x + shear * p.y;
        poly.push(Point {
            x: pen_x + dx + gx * k,
            y: baseline - p.y * k,
        });
    }
    out.push(poly);
}

/// Builds a rounded rectangle as a polyline.
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Contour {
    if r <= 0.05 {
        return vec![
            Point { x, y },
            Point { x: x + w, y },
            Point { x: x + w, y: y + h },
            Point { x, y: y + h },
        ];
    }
    const SEGMENTS: usize = 8;
    let mut pts = Vec::with_capacity(4 * (SEGMENTS + 1));
    let corners = [
        (x + w - r, y + r, -std::f32::consts::FRAC_PI_2),
        (x + w - r, y + h - r, 0.0),
        (x + r, y + h - r, std::f32::consts::FRAC_PI_2),
        (x + r, y + r, std::f32::consts::PI),
    ];
    for (cx, cy, start) in corners {
        for i in 0..=SEGMENTS {
            let a = start + std::f32::consts::FRAC_PI_2 * (i as f32 / SEGMENTS as f32);
            pts.push(Point {
                x: cx + r * a.cos(),
                y: cy + r * a.sin(),
            });
        }
    }
    pts
}

/// Strokes an open polyline by filling quads between consecutive points.
fn stroke_polyline(bmp: &mut Bitmap, poly: &[Point], width: f32, color: Color) {
    if poly.len() < 2 || width <= 0.0 {
        return;
    }
    let hw = width * 0.5;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let dx = b.x - a.x;
        let dy = b.y - a.y;
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-4 {
            continue;
        }
        let nx = -dy / len * hw;
        let ny = dx / len * hw;
        let quad = vec![
            Point {
                x: a.x + nx,
                y: a.y + ny,
            },
            Point {
                x: b.x + nx,
                y: b.y + ny,
            },
            Point {
                x: b.x - nx,
                y: b.y - ny,
            },
            Point {
                x: a.x - nx,
                y: a.y - ny,
            },
        ];
        fill_polygons(bmp, &[quad], &Fill::Solid(color));
    }
}

/// An affine transform in font units: `x' = a·x + c·y + e`,
/// `y' = b·x + d·y + f`.
#[derive(Debug, Clone, Copy)]
struct Affine {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Affine {
    const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn apply(self, p: Point) -> Point {
        Point {
            x: self.a * p.x + self.c * p.y + self.e,
            y: self.b * p.x + self.d * p.y + self.f,
        }
    }

    /// Returns `self ∘ inner`: `inner` is applied first.
    fn compose(self, inner: [f32; 6]) -> Affine {
        Affine {
            a: self.a * inner[0] + self.c * inner[1],
            b: self.b * inner[0] + self.d * inner[1],
            c: self.a * inner[2] + self.c * inner[3],
            d: self.b * inner[2] + self.d * inner[3],
            e: self.a * inner[4] + self.c * inner[5] + self.e,
            f: self.b * inner[4] + self.d * inner[5] + self.f,
        }
    }

    /// Average scale factor, used to scale gradient radii.
    fn scale(self) -> f32 {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        ((sx + sy) * 0.5).max(0.01)
    }
}

/// How a filled region is painted: a solid colour or a gradient.
#[derive(Debug, Clone)]
pub enum Fill {
    /// A single colour.
    Solid(Color),
    /// Linear gradient between two pixel-space points.
    Linear {
        p0: Point,
        p1: Point,
        stops: Vec<GradientStop>,
        extend: Extend,
    },
    /// Radial gradient between two pixel-space circles.
    Radial {
        c0: Point,
        r0: f32,
        c1: Point,
        r1: f32,
        stops: Vec<GradientStop>,
        extend: Extend,
    },
}

impl Fill {
    /// Colour of this paint at a pixel centre.
    fn color_at(&self, x: f32, y: f32) -> Color {
        match self {
            Fill::Solid(color) => *color,
            Fill::Linear {
                p0,
                p1,
                stops,
                extend,
            } => {
                let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
                let denom = dx * dx + dy * dy;
                let t = if denom <= 1e-6 {
                    0.0
                } else {
                    ((x - p0.x) * dx + (y - p0.y) * dy) / denom
                };
                sample_stops(stops, t, *extend)
            }
            Fill::Radial {
                c0,
                r0,
                c1,
                r1,
                stops,
                extend,
            } => {
                // Solve |p - c(t)|² = r(t)² for the two-circle gradient.
                let (dcx, dcy) = (c1.x - c0.x, c1.y - c0.y);
                let (vx, vy) = (x - c0.x, y - c0.y);
                let dr = r1 - r0;
                let a = dcx * dcx + dcy * dcy - dr * dr;
                let b = -2.0 * (vx * dcx + vy * dcy + r0 * dr);
                let c = vx * vx + vy * vy - r0 * r0;
                let t = if a.abs() < 1e-6 {
                    if b.abs() < 1e-6 { 0.0 } else { -c / b }
                } else {
                    let disc = b * b - 4.0 * a * c;
                    if disc < 0.0 {
                        0.0
                    } else {
                        let sq = disc.sqrt();
                        let t1 = (-b - sq) / (2.0 * a);
                        let t2 = (-b + sq) / (2.0 * a);
                        if t1 >= 0.0 && t2 >= 0.0 {
                            t1.min(t2)
                        } else if t1 >= 0.0 {
                            t1
                        } else if t2 >= 0.0 {
                            t2
                        } else {
                            t1.max(t2)
                        }
                    }
                };
                sample_stops(stops, t, *extend)
            }
        }
    }
}

/// Samples a colour line, applying the gradient's extend mode.
fn sample_stops(stops: &[GradientStop], t: f32, extend: Extend) -> Color {
    let Some(first) = stops.first() else {
        return Color::TRANSPARENT;
    };
    let t = match extend {
        Extend::Pad => t.clamp(0.0, 1.0),
        Extend::Repeat => t.rem_euclid(1.0),
        Extend::Reflect => {
            let m = t.rem_euclid(2.0);
            if m > 1.0 { 2.0 - m } else { m }
        }
    };
    if t <= first.offset {
        return first.color;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.offset {
            let span = b.offset - a.offset;
            let f = if span > 1e-6 {
                (t - a.offset) / span
            } else {
                1.0
            };
            return lerp_color(a.color, b.color, f);
        }
    }
    stops[stops.len() - 1].color
}

fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let mix = |x: u8, y: u8| {
        (x as f32 + (y as f32 - x as f32) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color::rgba(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b), mix(a.a, b.a))
}

/// An edge of a filled polygon, oriented top-to-bottom with a winding sign.
struct Edge {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    dir: i32,
}

/// Fills polygons with the non-zero winding rule and anti-aliasing.
///
/// Horizontal coverage is exact (fractional spans), vertical coverage uses
/// four sub-scanlines.
pub fn fill_polygons(bmp: &mut Bitmap, polys: &[Contour], fill: &Fill) {
    if let Fill::Solid(color) = fill
        && color.a == 0
    {
        return;
    }
    let mut edges = Vec::new();
    let (mut ymin, mut ymax) = (f32::INFINITY, f32::NEG_INFINITY);
    for poly in polys {
        if poly.len() < 3 {
            continue;
        }
        for i in 0..poly.len() {
            let a = poly[i];
            let b = poly[(i + 1) % poly.len()];
            if !a.x.is_finite() || !a.y.is_finite() || !b.x.is_finite() || !b.y.is_finite() {
                continue;
            }
            if (a.y - b.y).abs() < 1e-6 {
                continue;
            }
            let (dir, top, bot) = if a.y < b.y { (1, a, b) } else { (-1, b, a) };
            ymin = ymin.min(top.y);
            ymax = ymax.max(bot.y);
            edges.push(Edge {
                x0: top.x,
                y0: top.y,
                x1: bot.x,
                y1: bot.y,
                dir,
            });
        }
    }
    if edges.is_empty() || !ymin.is_finite() {
        return;
    }

    let h = bmp.height as i32;
    let w = bmp.width as i32;
    let y_start = ymin.floor().max(0.0) as i32;
    let y_end = (ymax.ceil() as i32).min(h);
    if y_start >= y_end {
        return;
    }
    let mut coverage = vec![0.0f32; bmp.width as usize + 2];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SUBSAMPLES as f32;

    for py in y_start..y_end {
        coverage.fill(0.0);
        let mut touched = false;
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;

        for s in 0..SUBSAMPLES {
            let sy = py as f32 + (s as f32 + 0.5) * weight;
            xs.clear();
            for e in &edges {
                if sy >= e.y0 && sy < e.y1 {
                    let t = (sy - e.y0) / (e.y1 - e.y0);
                    xs.push((e.x0 + t * (e.x1 - e.x0), e.dir));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

            let mut winding = 0;
            for k in 0..xs.len() {
                winding += xs[k].1;
                if winding == 0 || k + 1 >= xs.len() {
                    continue;
                }
                let (mut x0, mut x1) = (xs[k].0, xs[k + 1].0);
                if x1 <= 0.0 || x0 >= w as f32 {
                    continue;
                }
                x0 = x0.max(0.0);
                x1 = x1.min(w as f32);
                if x1 <= x0 {
                    continue;
                }
                add_span(&mut coverage, x0, x1, weight);
                min_x = min_x.min(x0);
                max_x = max_x.max(x1);
                touched = true;
            }
        }

        if !touched || min_x.is_nan() || max_x.is_nan() {
            continue;
        }
        let px_start = (min_x.floor().max(0.0) as usize).min(bmp.width as usize);
        let px_end = (max_x.ceil().max(0.0) as usize).min(bmp.width as usize);
        for (offset, &c) in coverage[px_start..px_end].iter().enumerate() {
            let c = c.min(1.0);
            if c > 0.002 {
                let color = fill.color_at((px_start + offset) as f32 + 0.5, py as f32 + 0.5);
                bmp.blend(px_start + offset, py as usize, color, c);
            }
        }
    }
}

/// Adds horizontal coverage for the span `[x0, x1)` weighted by `weight`.
fn add_span(cov: &mut [f32], x0: f32, x1: f32, weight: f32) {
    let w = cov.len() as f32;
    if x1 <= 0.0 || x0 >= w || x1 <= x0 {
        return;
    }
    let x0 = x0.max(0.0);
    let x1 = x1.min(w);
    let a = x0.floor() as usize;
    let b = x1.floor() as usize;
    if a == b {
        if a < cov.len() {
            cov[a] += (x1 - x0) * weight;
        }
        return;
    }
    if a < cov.len() {
        cov[a] += (a as f32 + 1.0 - x0) * weight;
    }
    let end = b.min(cov.len());
    for c in cov.iter_mut().take(end).skip(a + 1) {
        *c += weight;
    }
    if b < cov.len() {
        cov[b] += (x1 - b as f32) * weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{self, FontSet};

    fn test_fonts() -> Option<LoadedFonts> {
        FontSet::load(&font::discover())
    }

    #[test]
    fn bitmap_starts_transparent() {
        let bmp = Bitmap::new(4, 3);
        assert_eq!(bmp.pixel(0, 0), [0, 0, 0, 0]);
        assert_eq!(bmp.data.len(), 4 * 3 * 4);
    }

    #[test]
    fn filled_bitmap_has_opaque_pixels() {
        let bmp = Bitmap::filled(2, 2, Color::WHITE);
        assert_eq!(bmp.pixel(1, 1), [255, 255, 255, 255]);
    }

    #[test]
    fn blending_respects_coverage() {
        let mut bmp = Bitmap::filled(1, 1, Color::WHITE);
        bmp.blend(0, 0, Color::BLACK, 0.5);
        let p = bmp.pixel(0, 0);
        assert!(
            p[0] > 120 && p[0] < 135,
            "half coverage should be mid grey: {p:?}"
        );
        assert_eq!(p[3], 255);
    }

    #[test]
    fn blending_out_of_bounds_is_ignored() {
        let mut bmp = Bitmap::new(1, 1);
        bmp.blend(5, 5, Color::BLACK, 1.0);
        assert_eq!(bmp.pixel(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    fn solid_rect_covers_its_area_only() {
        let scene = Scene {
            width: 10.0,
            height: 10.0,
            background: None,
            draws: vec![Draw::Rect(
                RectDraw::new(2.0, 2.0, 4.0, 4.0).with_fill(Color::BLACK),
            )],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &LoadedFonts::default());
        assert_eq!(bmp.pixel(3, 3), [0, 0, 0, 255]);
        assert_eq!(bmp.pixel(1, 3)[3], 0);
        assert_eq!(bmp.pixel(6, 6)[3], 0);
    }

    #[test]
    fn rounded_corners_are_not_filled() {
        let scene = Scene {
            width: 20.0,
            height: 20.0,
            background: None,
            draws: vec![Draw::Rect(
                RectDraw::new(2.0, 2.0, 16.0, 16.0)
                    .with_fill(Color::BLACK)
                    .with_radius(6.0),
            )],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &LoadedFonts::default());
        assert_eq!(bmp.pixel(10, 10), [0, 0, 0, 255]);
        // The exact corner pixel is outside the rounded shape.
        assert_eq!(bmp.pixel(2, 2)[3], 0);
    }

    #[test]
    fn anti_aliasing_produces_partial_coverage() {
        let scene = Scene {
            width: 10.0,
            height: 10.0,
            background: None,
            // A triangle: its diagonal edge must produce soft pixels.
            draws: vec![Draw::Line(LineDraw {
                x1: 1.0,
                y1: 9.0,
                x2: 9.0,
                y2: 1.0,
                color: Color::BLACK,
                width: 2.0,
            })],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &LoadedFonts::default());
        let partial = bmp
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 8 && p[3] < 247)
            .count();
        assert!(
            partial > 0,
            "expected anti-aliased pixels along the diagonal"
        );
    }

    #[test]
    fn zoom_scales_geometry() {
        let scene = Scene {
            width: 10.0,
            height: 10.0,
            background: None,
            draws: vec![Draw::Rect(
                RectDraw::new(2.0, 2.0, 4.0, 4.0).with_fill(Color::BLACK),
            )],
        };
        let bmp = Rasterizer::new(2.0).rasterize(&scene, &LoadedFonts::default());
        assert_eq!((bmp.width, bmp.height), (20, 20));
        // The rect now spans 4..12 in device pixels.
        assert_eq!(bmp.pixel(4, 4), [0, 0, 0, 255]);
        assert_eq!(bmp.pixel(11, 11), [0, 0, 0, 255]);
        assert_eq!(bmp.pixel(3, 3)[3], 0);
        assert_eq!(bmp.pixel(12, 12)[3], 0);
    }

    #[test]
    fn background_fills_the_canvas() {
        let scene = Scene {
            width: 8.0,
            height: 8.0,
            background: Some(RectDraw::new(0.0, 0.0, 8.0, 8.0).with_fill(Color::rgb(255, 0, 0))),
            draws: vec![],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &LoadedFonts::default());
        assert_eq!(bmp.pixel(0, 0), [255, 0, 0, 255]);
    }

    #[test]
    fn shapes_outside_the_canvas_are_clipped() {
        let scene = Scene {
            width: 8.0,
            height: 8.0,
            background: None,
            draws: vec![Draw::Rect(
                RectDraw::new(-20.0, -20.0, 100.0, 100.0).with_fill(Color::BLACK),
            )],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &LoadedFonts::default());
        assert_eq!(bmp.pixel(0, 0), [0, 0, 0, 255]);
        assert_eq!(bmp.pixel(7, 7), [0, 0, 0, 255]);
    }

    #[test]
    fn text_renders_ink_inside_its_box() {
        let Some(fonts) = test_fonts() else {
            eprintln!("skipping: no fonts");
            return;
        };
        let scene = Scene {
            width: 80.0,
            height: 40.0,
            background: Some(RectDraw::new(0.0, 0.0, 80.0, 40.0).with_fill(Color::WHITE)),
            draws: vec![Draw::Text(TextDraw {
                x: 10.0,
                y: 25.0,
                text: "H".into(),
                size: 20.0,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: 0.0,
            })],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &fonts);
        let dark = (0..bmp.height)
            .flat_map(|y| (0..bmp.width).map(move |x| (x, y)))
            .filter(|&(x, y)| bmp.pixel(x, y)[0] < 128)
            .count();
        assert!(dark > 20, "expected glyph ink, got {dark} dark pixels");
        // Nothing outside the text box.
        assert_eq!(bmp.pixel(0, 0), [255, 255, 255, 255]);
        assert_eq!(bmp.pixel(79, 39), [255, 255, 255, 255]);
    }

    #[test]
    fn cjk_text_renders() {
        let Some(fonts) = test_fonts() else { return };
        let scene = Scene {
            width: 60.0,
            height: 40.0,
            background: Some(RectDraw::new(0.0, 0.0, 80.0, 40.0).with_fill(Color::WHITE)),
            draws: vec![Draw::Text(TextDraw {
                x: 5.0,
                y: 28.0,
                text: "中".into(),
                size: 24.0,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: 0.0,
            })],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &fonts);
        let dark = bmp
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] < 128)
            .count();
        assert!(dark > 30, "expected CJK glyph ink, got {dark}");
    }

    #[test]
    fn bold_text_is_heavier_than_regular() {
        let Some(fonts) = test_fonts() else { return };
        let make = |bold: bool| Scene {
            width: 80.0,
            height: 40.0,
            background: Some(RectDraw::new(0.0, 0.0, 80.0, 40.0).with_fill(Color::WHITE)),
            draws: vec![Draw::Text(TextDraw {
                x: 10.0,
                y: 25.0,
                text: "HH".into(),
                size: 20.0,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: 0.0,
            })],
        };
        let regular: u32 = Rasterizer::new(1.0)
            .rasterize(&make(false), &fonts)
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| 255 - p[0] as u32)
            .sum();
        let bold: u32 = Rasterizer::new(1.0)
            .rasterize(&make(true), &fonts)
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| 255 - p[0] as u32)
            .sum();
        assert!(
            bold > regular,
            "bold {bold} should be heavier than {regular}"
        );
    }

    #[test]
    fn raster_text_stays_inside_the_canvas() {
        let Some(fonts) = test_fonts() else { return };
        let scene = crate::to_scene(
            &"中英混排 mixed text wraps here ".repeat(40),
            &crate::Theme::light(),
        );
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &fonts);
        // The right-most two columns must stay background.
        for y in 0..bmp.height {
            for x in (bmp.width - 2)..bmp.width {
                let p = bmp.pixel(x, y);
                assert_eq!(p, [255, 255, 255, 255], "ink at the canvas edge ({x},{y})");
            }
        }
    }

    #[test]
    fn color_emoji_renders_with_palette_colors() {
        let Some(fonts) = test_fonts() else { return };
        let scene = Scene {
            width: 60.0,
            height: 60.0,
            background: Some(RectDraw::new(0.0, 0.0, 60.0, 60.0).with_fill(Color::WHITE)),
            draws: vec![Draw::Text(TextDraw {
                x: 8.0,
                y: 44.0,
                text: "✅".into(),
                size: 40.0,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: 0.0,
            })],
        };
        // Only meaningful when the selected face really has colour layers.
        let face = fonts.find('✅', false, false, false);
        let layers = face
            .and_then(|f| f.color_layers(f.glyph_id('✅')))
            .unwrap_or_default();
        if layers.is_empty() {
            eprintln!("skipping: no colour emoji font installed");
            return;
        }
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &fonts);
        let mut colors = std::collections::HashSet::new();
        for px in bmp.data.as_chunks::<4>().0 {
            if px[3] > 200 {
                colors.insert((px[0], px[1], px[2]));
            }
        }
        assert!(
            colors
                .iter()
                .any(|(r, g, b)| (*g as i16) > (*r as i16) + 20 && (*g as i16) > (*b as i16) + 20),
            "expected the palette's green fill: {colors:?}"
        );
    }

    fn ramp(stops: Vec<GradientStop>) -> Fill {
        Fill::Linear {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: 100.0, y: 0.0 },
            stops,
            extend: Extend::Pad,
        }
    }

    fn black_to_white() -> Vec<GradientStop> {
        vec![
            GradientStop {
                offset: 0.0,
                color: Color::BLACK,
            },
            GradientStop {
                offset: 1.0,
                color: Color::WHITE,
            },
        ]
    }

    #[test]
    fn linear_gradient_samples_along_its_axis() {
        let fill = ramp(black_to_white());
        assert_eq!(fill.color_at(0.0, 0.0), Color::BLACK);
        assert_eq!(fill.color_at(100.0, 0.0), Color::WHITE);
        let mid = fill.color_at(50.0, 0.0);
        assert!(mid.r > 120 && mid.r < 135, "{mid:?}");
        // Perpendicular position does not change the colour.
        assert_eq!(fill.color_at(50.0, 500.0), mid);
    }

    #[test]
    fn gradient_extend_modes_wrap_the_colour_line() {
        let pad = ramp(black_to_white());
        assert_eq!(pad.color_at(150.0, 0.0), Color::WHITE);

        let repeat = Fill::Linear {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: 100.0, y: 0.0 },
            stops: black_to_white(),
            extend: Extend::Repeat,
        };
        // t = 1.25 wraps to 0.25.
        let c = repeat.color_at(125.0, 0.0);
        assert!(c.r > 55 && c.r < 70, "{c:?}");

        let reflect = Fill::Linear {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: 100.0, y: 0.0 },
            stops: black_to_white(),
            extend: Extend::Reflect,
        };
        // t = 1.75 reflects to 0.25 as well.
        let c = reflect.color_at(175.0, 0.0);
        assert!(c.r > 55 && c.r < 70, "{c:?}");
    }

    #[test]
    fn radial_gradient_maps_radius_to_the_colour_line() {
        let fill = Fill::Radial {
            c0: Point { x: 50.0, y: 50.0 },
            r0: 0.0,
            c1: Point { x: 50.0, y: 50.0 },
            r1: 50.0,
            stops: black_to_white(),
            extend: Extend::Pad,
        };
        assert_eq!(fill.color_at(50.0, 50.0), Color::BLACK);
        assert_eq!(fill.color_at(100.0, 50.0), Color::WHITE);
        let mid = fill.color_at(75.0, 50.0);
        assert!(mid.r > 120 && mid.r < 135, "{mid:?}");
    }

    #[test]
    fn gradients_paint_a_visible_ramp() {
        let mut scene = Scene {
            width: 100.0,
            height: 20.0,
            background: Some(RectDraw::new(0.0, 0.0, 100.0, 20.0).with_fill(Color::WHITE)),
            draws: vec![],
        };
        let mut bmp = Bitmap::new(100, 20);
        let square = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            Point { x: 100.0, y: 20.0 },
            Point { x: 0.0, y: 20.0 },
        ];
        fill_polygons(&mut bmp, &[square], &ramp(black_to_white()));
        let left = bmp.pixel(2, 10);
        let right = bmp.pixel(97, 10);
        assert!(left[0] < 30, "left should be dark: {left:?}");
        assert!(right[0] > 225, "right should be light: {right:?}");
        let middle = bmp.pixel(50, 10);
        assert!(middle[0] > left[0] && middle[0] < right[0]);
        scene.draws.clear();
    }

    #[test]
    fn gradient_emoji_renders_many_intermediate_colours() {
        let Some(fonts) = test_fonts() else { return };
        let scene = Scene {
            width: 160.0,
            height: 160.0,
            background: Some(RectDraw::new(0.0, 0.0, 160.0, 160.0).with_fill(Color::WHITE)),
            draws: vec![Draw::Text(TextDraw {
                x: 20.0,
                y: 130.0,
                text: "🔥".into(),
                size: 120.0,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: 0.0,
            })],
        };
        let has_color = fonts
            .find('🔥', false, false, false)
            .is_some_and(|f| f.has_color_glyphs());
        if !has_color {
            eprintln!("skipping: no colour emoji font installed");
            return;
        }
        let bmp = Rasterizer::new(1.0).rasterize(&scene, &fonts);
        let mut colors = std::collections::HashSet::new();
        let mut warm = 0usize;
        for px in bmp.data.as_chunks::<4>().0 {
            if px[3] > 200 {
                colors.insert((px[0], px[1], px[2]));
                if px[0] > 200 && px[1] > 100 && px[2] < 140 {
                    warm += 1;
                }
            }
        }
        if colors.len() < 10 {
            eprintln!("skipping: emoji rendered without colour");
            return;
        }
        assert!(
            colors.len() > 50,
            "gradients should produce many shades, got {}",
            colors.len()
        );
        assert!(warm > 100, "expected warm flame colours, got {warm} pixels");
    }
}
