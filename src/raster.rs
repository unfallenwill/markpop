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

    /// Stores one pixel.
    fn set_pixel(&mut self, x: u32, y: u32, px: [u8; 4]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.data[i..i + 4].copy_from_slice(&px);
    }

    /// Source-over composite of `src` at pixel `(x, y)`.
    fn draw_over(&self, dst: &mut Bitmap, x: u32, y: u32) {
        for py in 0..self.height {
            for px in 0..self.width {
                let (tx, ty) = (x + px, y + py);
                if tx >= dst.width || ty >= dst.height {
                    continue;
                }
                let src = self.pixel(px, py);
                let sa = src[3] as f32 / 255.0;
                if sa <= 0.0 {
                    continue;
                }
                let dst_px = dst.pixel(tx, ty);
                let da = dst_px[3] as f32 / 255.0;
                let out_a = sa + da * (1.0 - sa);
                let mut out = [0u8; 4];
                for c in 0..3 {
                    let v = (src[c] as f32 * sa + dst_px[c] as f32 * da * (1.0 - sa)) / out_a;
                    out[c] = v.round().clamp(0.0, 255.0) as u8;
                }
                out[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                dst.set_pixel(tx, ty, out);
            }
        }
    }

    /// Source-over composite of `color` at `coverage` (`0.0..=1.0`).
    fn blend(&mut self, x: usize, y: usize, color: Color, coverage: f32) {
        if x >= self.width as usize || y >= self.height as usize {
            return;
        }
        let sa = (color.a as f32 / 255.0) * aa_contrast(coverage);
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

/// Contrast applied to antialiasing coverage before blending.
///
/// A linear coverage→alpha mapping renders a half-covered edge pixel as
/// half-ink gray, which reads as soft compared to browser rasterizers
/// (DirectWrite/Skia apply a text-contrast stage of their own). Pushing
/// coverage away from 0.5 toward both ends narrows the visible
/// transition band without breaking either polarity: dark text gets
/// crisper on light backgrounds, light text on dark. The map is
/// monotonic and fixes 0.0, 0.5 and 1.0; `1.0` disables it.
const AA_CONTRAST: f32 = 2.0;

/// Applies the antialiasing contrast curve to one coverage value.
fn aa_contrast(coverage: f32) -> f32 {
    let a = coverage.clamp(0.0, 1.0);
    (0.5 + (a - 0.5) * AA_CONTRAST).clamp(0.0, 1.0)
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
        // Painted regions in draw order. Colour glyphs contribute their whole
        // paint graph (layers, transforms, composites); ordinary text
        // contributes one solid-filled shape.
        let mut shapes: Vec<Shape> = Vec::new();
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
                        // Clip boxes are declared in font units, so map them to
                        // page space once, with the same y flip as outlines.
                        let clip = face.color_clip(gid).map(|b| {
                            [
                                pen + b[0] * k,
                                baseline - b[3] * k,
                                pen + b[2] * k,
                                baseline - b[1] * k,
                            ]
                        });
                        // Draw the font's own paint graph: exact outlines, exact
                        // palette, gradients, transforms and blend modes.
                        if let Some(shape) = self.emit_paint(
                            face,
                            &paint,
                            Affine::IDENTITY,
                            pen,
                            baseline,
                            k,
                            flatness,
                            t.color,
                            clip,
                            0,
                        ) {
                            shapes.push(shape);
                        }
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
                            shapes.push(Shape::Fill {
                                polys,
                                fill: Fill::Solid(t.color),
                            });
                        }
                    }
                }
                if face.has_glyph(ch) {
                    // Use the font's own advance here: this is what a browser
                    // does for the SVG output, so raster and vector text end up
                    // with the same spacing. Tracking is added on top, exactly
                    // like the `letter-spacing` attribute the SVG carries (and
                    // like `Metrics` reserved during layout).
                    advance = Some(face.advance(gid) * k + t.letter_spacing * self.scale);
                }
            }
            pen += advance.unwrap_or_else(|| metrics.advance(ch, &style) * self.scale);
        }
        for shape in &shapes {
            shape.render(bmp, 0.0, 0.0);
        }
    }

    /// Walks a COLR paint graph and turns it into a [`Shape`].
    ///
    /// `xf` accumulates COLR transforms (in font units); glyph outlines are
    /// transformed before being mapped to pixels, so the font's geometry is
    /// preserved exactly. `clip` is an optional page-space clip box declared
    /// by the font.
    #[allow(clippy::too_many_arguments)]
    fn emit_paint(
        &self,
        face: &crate::font::Face,
        paint: &ColorPaint,
        xf: Affine,
        pen: f32,
        baseline: f32,
        k: f32,
        flatness: f32,
        foreground: Color,
        clip: Option<[f32; 4]>,
        depth: u32,
    ) -> Option<Shape> {
        if depth > 24 {
            return None;
        }
        match paint {
            ColorPaint::Layers(layers) => {
                let shapes: Vec<Shape> = layers
                    .iter()
                    .filter_map(|layer| {
                        self.emit_paint(
                            face,
                            layer,
                            xf,
                            pen,
                            baseline,
                            k,
                            flatness,
                            foreground,
                            clip,
                            depth + 1,
                        )
                    })
                    .collect();
                (!shapes.is_empty()).then_some(Shape::Group(shapes))
            }
            ColorPaint::Transform { matrix, paint } => self.emit_paint(
                face,
                paint,
                xf.compose(*matrix),
                pen,
                baseline,
                k,
                flatness,
                foreground,
                clip,
                depth + 1,
            ),
            ColorPaint::Composite {
                mode,
                source,
                backdrop,
            } => {
                let source = self.emit_paint(
                    face,
                    source,
                    xf,
                    pen,
                    baseline,
                    k,
                    flatness,
                    foreground,
                    clip,
                    depth + 1,
                )?;
                let backdrop = self.emit_paint(
                    face,
                    backdrop,
                    xf,
                    pen,
                    baseline,
                    k,
                    flatness,
                    foreground,
                    clip,
                    depth + 1,
                )?;
                Some(Shape::Composite {
                    mode: *mode,
                    source: Box::new(source),
                    backdrop: Box::new(backdrop),
                })
            }
            ColorPaint::Glyph { glyph, paint } => {
                let fill = resolve_fill(paint, xf, pen, baseline, k, foreground)?;
                let mut polys = Vec::new();
                for contour in face.outline(*glyph, flatness) {
                    let transformed: Contour = contour.iter().map(|p| xf.apply(*p)).collect();
                    push_contour(&mut polys, &transformed, pen, baseline, k, false, 0.0);
                }
                if let Some(rect) = clip {
                    polys = clip_contours(&polys, rect);
                }
                if polys.is_empty() {
                    return None;
                }
                Some(Shape::Fill { polys, fill })
            }
            // A solid or gradient alone has no shape to fill.
            ColorPaint::Solid { .. }
            | ColorPaint::Foreground
            | ColorPaint::Linear { .. }
            | ColorPaint::Radial { .. }
            | ColorPaint::Sweep { .. } => None,
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
            p2,
            stops,
            extend,
        } => Fill::Linear {
            p0: to_px(*p0),
            p1: to_px(*p1),
            p2: to_px(*p2),
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
        ColorPaint::Sweep {
            center,
            start,
            end,
            stops,
            extend,
        } => {
            // A mirroring transform flips the sweep direction, so the colour
            // line then runs the other way round.
            let (mut start, mut end) = (*start, *end);
            if xf.det() < 0.0 {
                std::mem::swap(&mut start, &mut end);
                start = -start;
                end = -end;
            }
            Fill::Sweep {
                center: to_px(*center),
                start,
                end,
                stops: stops.clone(),
                extend: *extend,
            }
        }
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

    /// Determinant: negative when the transform mirrors the geometry.
    fn det(self) -> f32 {
        self.a * self.d - self.b * self.c
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
    ///
    /// `p2` is the rotation point: iso-colour lines run parallel to `p0`–`p2`,
    /// so the colour parameter is the projection onto the normal of that line.
    Linear {
        p0: Point,
        p1: Point,
        p2: Point,
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
    /// Sweep (conic) gradient around a pixel-space centre.
    ///
    /// Angles are counter-clockwise radians in the design grid; because the
    /// raster y axis points down, sampling negates the y delta.
    Sweep {
        center: Point,
        start: f32,
        end: f32,
        stops: Vec<GradientStop>,
        extend: Extend,
    },
}

impl Fill {
    /// Colour of this paint at a pixel centre.
    ///
    /// Transparent means "nothing painted here", which the two-circle radial
    /// and ill-formed gradients return for uncovered pixels.
    fn color_at(&self, x: f32, y: f32) -> Color {
        match self {
            Fill::Solid(color) => *color,
            Fill::Linear {
                p0,
                p1,
                p2,
                stops,
                extend,
            } => {
                // Iso-colour lines are parallel to p0→p2, so project onto the
                // normal of that direction.
                let (nx, ny) = (-(p2.y - p0.y), p2.x - p0.x);
                let denom = (p1.x - p0.x) * nx + (p1.y - p0.y) * ny;
                if denom.abs() <= 1e-6 {
                    return Color::TRANSPARENT;
                }
                let t = ((x - p0.x) * nx + (y - p0.y) * ny) / denom;
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
                if c0.x == c1.x && c0.y == c1.y && r0 == r1 {
                    return Color::TRANSPARENT;
                }
                // Solve |p - c(t)|² = r(t)² for the two-circle gradient.
                let (dcx, dcy) = (c1.x - c0.x, c1.y - c0.y);
                let (vx, vy) = (x - c0.x, y - c0.y);
                let dr = r1 - r0;
                let a = dcx * dcx + dcy * dcy - dr * dr;
                let b = -2.0 * (vx * dcx + vy * dcy + r0 * dr);
                let c = vx * vx + vy * vy - r0 * r0;
                // Circles are painted from the largest ω down; a pixel keeps
                // the colour of the smallest ω whose radius is positive.
                let mut best: Option<f32> = None;
                let mut consider = |t: f32| {
                    // Only circles with a positive radius are painted; the
                    // degenerate r == 0 circle is the centre point itself.
                    if (r0 + (r1 - r0) * t) < -1e-6 {
                        return;
                    }
                    best = Some(match best {
                        Some(b) => b.min(t),
                        None => t,
                    });
                };
                if a.abs() < 1e-6 {
                    if b.abs() >= 1e-6 {
                        consider(-c / b);
                    }
                } else {
                    let disc = b * b - 4.0 * a * c;
                    if disc >= 0.0 {
                        let sq = disc.sqrt();
                        consider((-b - sq) / (2.0 * a));
                        consider((-b + sq) / (2.0 * a));
                    }
                }
                match best {
                    Some(t) => sample_stops(stops, t, *extend),
                    None => Color::TRANSPARENT,
                }
            }
            Fill::Sweep {
                center,
                start,
                end,
                stops,
                extend,
            } => {
                // Counter-clockwise angle in the design grid.
                let angle = (center.y - y).atan2(x - center.x);
                let angle = if angle < 0.0 {
                    angle + 2.0 * std::f32::consts::PI
                } else {
                    angle
                };
                let span = end - start;
                const TWO_PI: f32 = std::f32::consts::TAU;
                let t = if span.abs() <= 1e-6 {
                    if matches!(extend, Extend::Pad) {
                        0.0
                    } else {
                        // A zero-width repeated sweep is ill-formed.
                        return Color::TRANSPARENT;
                    }
                } else if span.abs() >= TWO_PI - 1e-4 {
                    // At most one full turn is drawn; wrap into 0..1.
                    let signed = if span > 0.0 { TWO_PI } else { -TWO_PI };
                    ((angle - start) / signed).rem_euclid(1.0)
                } else {
                    (angle - start) / span
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

/// Exact sRGB transfer function, sRGB value to linear light.
fn to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Inverse of [`to_linear`], linear light to an sRGB byte.
fn from_linear(v: f32) -> u8 {
    let c = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

/// Interpolates two stop colours the way COLR gradients require.
///
/// The specification asks for linear-light values with alpha premultiplied,
/// so a fade to transparent does not darken towards black.
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let (aa, ab) = (a.a as f32 / 255.0, b.a as f32 / 255.0);
    let out_a = aa + (ab - aa) * t;
    if out_a <= 0.0 {
        return Color::TRANSPARENT;
    }
    let mix = |x: u8, y: u8| {
        let x = to_linear(x) * aa;
        let y = to_linear(y) * ab;
        from_linear((x + (y - x) * t) / out_a)
    };
    Color::rgba(
        mix(a.r, b.r),
        mix(a.g, b.g),
        mix(a.b, b.b),
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    )
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
/// Fills polygons with the non-zero winding rule and anti-aliasing.
///
/// Horizontal coverage is exact (fractional spans), vertical coverage uses
/// four sub-scanlines.
pub fn fill_polygons(bmp: &mut Bitmap, polys: &[Contour], fill: &Fill) {
    fill_polygons_at(bmp, polys, fill, 0.0, 0.0);
}

/// Fills polygons whose coordinates are in page space into a bitmap whose
/// pixel `(0, 0)` sits at page `(ox, oy)`.
///
/// The offset lets a composite sub-graph render into a small scratch bitmap
/// while its geometry and gradients stay in absolute page coordinates.
fn fill_polygons_at(bmp: &mut Bitmap, polys: &[Contour], fill: &Fill, ox: f32, oy: f32) {
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
    let y_start = (ymin.floor() - oy).max(0.0) as i32;
    let y_end = ((ymax.ceil() - oy) as i32).min(h);
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
        let base_y = py as f32 + oy;

        for s in 0..SUBSAMPLES {
            let sy = base_y + (s as f32 + 0.5) * weight;
            xs.clear();
            for e in &edges {
                if sy >= e.y0 && sy < e.y1 {
                    let t = (sy - e.y0) / (e.y1 - e.y0);
                    xs.push((e.x0 + (e.x1 - e.x0) * t, e.dir));
                }
            }
            if xs.len() < 2 {
                continue;
            }
            xs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            let mut winding = 0;
            let mut span: Option<f32> = None;
            for (x, dir) in &xs {
                let was_inside = winding != 0;
                winding += dir;
                let inside = winding != 0;
                if !was_inside && inside {
                    span = Some(*x);
                } else if was_inside
                    && !inside
                    && let Some(x0) = span.take()
                {
                    let (x0, x1) = (x0 - ox, *x - ox);
                    let x0 = x0.max(0.0);
                    let x1 = x1.min(w as f32);
                    if x1 <= x0 {
                        continue;
                    }
                    add_span(&mut coverage, x0, x1, weight);
                    min_x = min_x.min(x0);
                    max_x = max_x.max(x1);
                    touched = true;
                }
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
                let color = fill.color_at((px_start + offset) as f32 + ox + 0.5, base_y + 0.5);
                bmp.blend(px_start + offset, py as usize, color, c);
            }
        }
    }
}

/// A painted region of a colour glyph's paint graph.
#[derive(Debug, Clone)]
enum Shape {
    /// Polygons filled with one paint.
    Fill {
        /// Page-space polygons.
        polys: Vec<Contour>,
        /// Paint applied to them.
        fill: Fill,
    },
    /// Layers stacked with plain source-over.
    Group(Vec<Shape>),
    /// Two sub-graphs combined with a COLR composite mode.
    Composite {
        /// `CompositeMode` index (0..=27).
        mode: u8,
        /// Source sub-graph.
        source: Box<Shape>,
        /// Backdrop sub-graph.
        backdrop: Box<Shape>,
    },
}

impl Shape {
    /// Bounding box of everything this shape can paint, in page space.
    fn bounds(&self) -> Option<[f32; 4]> {
        match self {
            Shape::Fill { polys, .. } => {
                let mut b = [
                    f32::INFINITY,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                    f32::NEG_INFINITY,
                ];
                for p in polys.iter().flatten() {
                    b[0] = b[0].min(p.x);
                    b[1] = b[1].min(p.y);
                    b[2] = b[2].max(p.x);
                    b[3] = b[3].max(p.y);
                }
                (b[0] <= b[2] && b[1] <= b[3]).then_some(b)
            }
            Shape::Group(items) => items.iter().filter_map(|s| s.bounds()).reduce(union),
            Shape::Composite {
                source, backdrop, ..
            } => match (source.bounds(), backdrop.bounds()) {
                (Some(a), Some(b)) => Some(union(a, b)),
                (a, b) => a.or(b),
            },
        }
    }

    /// Paints this shape into `bmp`, whose pixel `(0, 0)` is page `(ox, oy)`.
    fn render(&self, bmp: &mut Bitmap, ox: f32, oy: f32) {
        match self {
            Shape::Fill { polys, fill } => fill_polygons_at(bmp, polys, fill, ox, oy),
            Shape::Group(items) => {
                for shape in items {
                    shape.render(bmp, ox, oy);
                }
            }
            Shape::Composite {
                mode,
                source,
                backdrop,
            } => {
                let Some(bounds) = self.bounds() else {
                    return;
                };
                // One pixel of slack so anti-aliased edges are kept.
                let x0 = (bounds[0].floor() as i32 - 1).max(0);
                let y0 = (bounds[1].floor() as i32 - 1).max(0);
                let x1 = (bounds[2].ceil() as i32 + 1).min(bmp.width as i32);
                let y1 = (bounds[3].ceil() as i32 + 1).min(bmp.height as i32);
                if x1 <= x0 || y1 <= y0 {
                    return;
                }
                let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
                let mut src = Bitmap::new(w, h);
                let mut dst = Bitmap::new(w, h);
                // Scratch pixel (0, 0) sits at page (x0, y0).
                let (dx, dy) = (x0 as f32, y0 as f32);
                backdrop.render(&mut dst, dx, dy);
                source.render(&mut src, dx, dy);
                for py in 0..h {
                    for px in 0..w {
                        let blended = composite_pixel(*mode, src.pixel(px, py), dst.pixel(px, py));
                        dst.set_pixel(px, py, blended);
                    }
                }
                dst.draw_over(bmp, x0 as u32, y0 as u32);
            }
        }
    }
}

/// Smallest box containing both inputs.
fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
}

/// Clips polygons to an axis-aligned rectangle (Sutherland–Hodgman).
fn clip_contours(polys: &[Contour], rect: [f32; 4]) -> Vec<Contour> {
    let [x0, y0, x1, y1] = rect;
    let mut out = Vec::with_capacity(polys.len());
    for poly in polys {
        if poly.len() < 3 {
            continue;
        }
        // Each pass clips against one edge: left, right, top, bottom.
        let mut current = poly.clone();
        let edges = [(0, x0, true), (0, x1, false), (1, y0, true), (1, y1, false)];
        for (axis, limit, keep_greater) in edges {
            if current.is_empty() {
                break;
            }
            let inside = |p: &Point| {
                let v = if axis == 0 { p.x } else { p.y };
                if keep_greater { v >= limit } else { v <= limit }
            };
            let mut clipped: Contour = Vec::with_capacity(current.len() + 1);
            for i in 0..current.len() {
                let a = current[i];
                let b = current[(i + 1) % current.len()];
                let (a_in, b_in) = (inside(&a), inside(&b));
                if a_in {
                    clipped.push(a);
                }
                if a_in != b_in {
                    let (av, bv) = if axis == 0 { (a.x, b.x) } else { (a.y, b.y) };
                    let f = if (bv - av).abs() < 1e-9 {
                        0.0
                    } else {
                        (limit - av) / (bv - av)
                    };
                    clipped.push(Point {
                        x: a.x + (b.x - a.x) * f,
                        y: a.y + (b.y - a.y) * f,
                    });
                }
            }
            current = clipped;
        }
        if current.len() >= 3 {
            out.push(current);
        }
    }
    out
}

/// Composites `src` over `dst` with a COLR `CompositeMode`.
///
/// The colour math follows Compositing and Blending Level 1: blends are
/// evaluated in linear light and the result is returned as straight sRGB.
fn composite_pixel(mode: u8, src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    let mode = if mode > 27 { 0 } else { mode };
    let sa = src[3] as f32 / 255.0;
    let da = dst[3] as f32 / 255.0;
    let cs = [to_linear(src[0]), to_linear(src[1]), to_linear(src[2])];
    let cb = [to_linear(dst[0]), to_linear(dst[1]), to_linear(dst[2])];
    let (co, ao) = if mode < 13 {
        let (fa, fb) = porter_duff_factors(mode, sa, da);
        let mut ao = sa * fa + da * fb;
        let mut co = [0.0f32; 3];
        for i in 0..3 {
            co[i] = sa * fa * cs[i] + da * fb * cb[i];
        }
        if mode == 12 {
            // PLUS clamps in premultiplied space, not after un-premultiplying.
            ao = ao.min(1.0);
            for c in &mut co {
                *c = c.min(1.0);
            }
        }
        (co, ao)
    } else {
        let blended = blend_channels(mode, cb, cs);
        let ao = sa + da * (1.0 - sa);
        let mut co = [0.0f32; 3];
        for i in 0..3 {
            co[i] = sa * (1.0 - da) * cs[i] + sa * da * blended[i] + (1.0 - sa) * da * cb[i];
        }
        (co, ao)
    };
    if ao <= 0.0 {
        return [0, 0, 0, 0];
    }
    [
        from_linear(co[0] / ao),
        from_linear(co[1] / ao),
        from_linear(co[2] / ao),
        (ao * 255.0).round().clamp(0.0, 255.0) as u8,
    ]
}

/// Porter-Duff source and backdrop factors for the composite modes 0..=12.
fn porter_duff_factors(mode: u8, sa: f32, da: f32) -> (f32, f32) {
    match mode {
        // CLEAR
        0 => (0.0, 0.0),
        // SRC
        1 => (1.0, 0.0),
        // DST
        2 => (0.0, 1.0),
        // SRC_OVER
        3 => (1.0, 1.0 - sa),
        // DST_OVER
        4 => (1.0 - da, 1.0),
        // SRC_IN
        5 => (da, 0.0),
        // DST_IN
        6 => (0.0, sa),
        // SRC_OUT
        7 => (1.0 - da, 0.0),
        // DST_OUT
        8 => (0.0, 1.0 - sa),
        // SRC_ATOP
        9 => (da, 1.0 - sa),
        // DST_ATOP
        10 => (1.0 - da, sa),
        // XOR
        11 => (1.0 - da, 1.0 - sa),
        // PLUS
        _ => (1.0, 1.0),
    }
}

/// Blend functions of the separable and non-separable composite modes.
fn blend_channels(mode: u8, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    match mode {
        13..=23 => {
            for i in 0..3 {
                out[i] = blend_channel(mode, cb[i], cs[i]);
            }
        }
        24 => out = set_lum(set_sat(cs, sat(cb)), lum(cb)),
        25 => out = set_lum(set_sat(cb, sat(cs)), lum(cb)),
        26 => out = set_lum(cs, lum(cb)),
        27 => out = set_lum(cb, lum(cs)),
        _ => out = cs,
    }
    out
}

/// One separable blend mode, applied per colour channel.
fn blend_channel(mode: u8, cb: f32, cs: f32) -> f32 {
    match mode {
        // SCREEN
        13 => cb + cs - cb * cs,
        // OVERLAY
        14 => hard_light(cs, cb),
        // DARKEN
        15 => cb.min(cs),
        // LIGHTEN
        16 => cb.max(cs),
        // COLOR_DODGE
        17 => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        // COLOR_BURN
        18 => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        // HARD_LIGHT
        19 => hard_light(cb, cs),
        // SOFT_LIGHT
        20 => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 {
                    ((16.0 * cb - 12.0) * cb + 4.0) * cb
                } else {
                    cb.sqrt()
                };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        // DIFFERENCE
        21 => (cb - cs).abs(),
        // EXCLUSION
        22 => cb + cs - 2.0 * cb * cs,
        // MULTIPLY
        23 => cb * cs,
        _ => cs,
    }
}

/// The `hard-light` rule, shared by `hard-light` and `overlay`.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        2.0 * cb * cs
    } else {
        1.0 - 2.0 * (1.0 - cb) * (1.0 - cs)
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

/// Clamps an out-of-gamut colour back into `0..=1` while keeping its
/// luminance, as the compositing spec requires.
fn clip_color(mut c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    if n < 0.0 {
        for v in &mut c {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
        for v in &mut c {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    c
}

fn set_lum(mut c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    for v in &mut c {
        *v += d;
    }
    clip_color(c)
}

fn set_sat(mut c: [f32; 3], s: f32) -> [f32; 3] {
    let (mut min, mut mid, mut max) = (0, 1, 2);
    if c[min] > c[mid] {
        std::mem::swap(&mut min, &mut mid);
    }
    if c[mid] > c[max] {
        std::mem::swap(&mut mid, &mut max);
    }
    if c[min] > c[mid] {
        std::mem::swap(&mut min, &mut mid);
    }
    if c[max] > c[min] {
        c[mid] = (c[mid] - c[min]) * s / (c[max] - c[min]);
        c[max] = s;
    } else {
        c[mid] = 0.0;
        c[max] = 0.0;
    }
    c[min] = 0.0;
    c
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
    fn aa_contrast_preserves_landmarks() {
        // Endpoints and the midpoint are fixed points; the curve only
        // shapes the transition band between them.
        assert_eq!(aa_contrast(0.0), 0.0);
        assert_eq!(aa_contrast(0.5), 0.5);
        assert_eq!(aa_contrast(1.0), 1.0);
    }

    #[test]
    fn aa_contrast_is_monotonic_with_flat_toe() {
        // The contrast stage flattens the faintest coverages to zero —
        // they would render as near-invisible gray anyway, which is part
        // of why edges read as crisp. Everything above that toe must be
        // monotonic.
        assert_eq!(aa_contrast(0.0), 0.0);
        assert_eq!(aa_contrast(0.1), 0.0, "faint coverage flattens to zero");
        let mut prev = 0.0;
        for step in 1..=100 {
            let a = step as f32 / 100.0;
            let c = aa_contrast(a);
            assert!(c >= prev, "must not decrease at coverage {a}");
            prev = c;
        }
    }

    #[test]
    fn aa_contrast_sharpens_both_polarities() {
        // Coverage below half is pushed toward 0, above half toward 1,
        // so dark-on-light and light-on-dark edges both gain contrast.
        assert!(aa_contrast(0.3) < 0.3);
        assert!(aa_contrast(0.7) > 0.7);
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
            // Iso-colour lines are vertical, so colour varies along x.
            p2: Point { x: 0.0, y: 100.0 },
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
        // Interpolation happens in linear light, so the midpoint is much
        // lighter than the sRGB average (~128): 0.5 linear is ~188 sRGB.
        let mid = fill.color_at(50.0, 0.0);
        assert!(mid.r > 180 && mid.r < 195, "{mid:?}");
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
            p2: Point { x: 0.0, y: 100.0 },
            stops: black_to_white(),
            extend: Extend::Repeat,
        };
        // t = 1.25 wraps to 0.25 (sRGB ~137 for a linear-light ramp).
        let c = repeat.color_at(125.0, 0.0);
        assert!(c.r > 130 && c.r < 145, "{c:?}");

        let reflect = Fill::Linear {
            p0: Point { x: 0.0, y: 0.0 },
            p1: Point { x: 100.0, y: 0.0 },
            p2: Point { x: 0.0, y: 100.0 },
            stops: black_to_white(),
            extend: Extend::Reflect,
        };
        // t = 1.75 reflects to 0.25 as well.
        let c = reflect.color_at(175.0, 0.0);
        assert!(c.r > 130 && c.r < 145, "{c:?}");
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
        assert!(mid.r > 180 && mid.r < 195, "{mid:?}");
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
        // Linear-light ramp: sRGB 0 -> 255 still ends at the extremes.
        assert!(left[0] < 60, "left should be dark: {left:?}");
        assert!(right[0] > 245, "right should be light: {right:?}");
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

    fn sweep(start_deg: f32, end_deg: f32, extend: Extend) -> Fill {
        Fill::Sweep {
            center: Point { x: 0.0, y: 0.0 },
            start: start_deg.to_radians(),
            end: end_deg.to_radians(),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color::rgba(255, 0, 0, 255),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color::rgba(0, 0, 255, 255),
                },
            ],
            extend,
        }
    }

    /// Samples the sweep gradient in design orientation (y up).
    fn sweep_at(fill: &Fill, angle_deg: f32) -> Color {
        let a = angle_deg.to_radians();
        fill.color_at(10.0 * a.cos(), -10.0 * a.sin())
    }

    #[test]
    fn sweep_gradient_runs_counter_clockwise() {
        let fill = sweep(0.0, 360.0, Extend::Pad);
        // 0 degrees is the positive x axis: the first stop.
        assert_eq!(sweep_at(&fill, 0.0), Color::rgba(255, 0, 0, 255));
        // Half a turn is halfway along the colour line.
        let half = sweep_at(&fill, 180.0);
        assert!(half.r > 170 && half.b > 170, "{half:?}");
        // Just below a full turn is (almost) the last stop.
        let end = sweep_at(&fill, 359.0);
        assert!(end.b > 200 && end.r < 40, "{end:?}");
    }

    #[test]
    fn sweep_gradient_pads_outside_its_range() {
        // A 90..270 degree sweep: below 90 is the first stop, above 270 the
        // last one, whatever the angle wrap does.
        let fill = sweep(90.0, 270.0, Extend::Pad);
        assert_eq!(sweep_at(&fill, 0.0), Color::rgba(255, 0, 0, 255));
        assert_eq!(sweep_at(&fill, 350.0), Color::rgba(0, 0, 255, 255));
        let mid = sweep_at(&fill, 180.0);
        assert!(mid.r > 170 && mid.b > 170, "{mid:?}");
    }

    #[test]
    fn sweep_gradient_can_run_clockwise() {
        // 270 -> 90 is a negative span, so the colour line runs clockwise.
        let fill = sweep(270.0, 90.0, Extend::Pad);
        assert_eq!(sweep_at(&fill, 270.0), Color::rgba(255, 0, 0, 255));
        assert_eq!(sweep_at(&fill, 90.0), Color::rgba(0, 0, 255, 255));
        // Going on past the end (towards 0) stays at the last stop.
        assert_eq!(sweep_at(&fill, 0.0), Color::rgba(0, 0, 255, 255));
    }

    #[test]
    fn zero_width_sweep_only_makes_sense_when_padded() {
        let pad = sweep(90.0, 90.0, Extend::Pad);
        assert_eq!(sweep_at(&pad, 0.0), Color::rgba(255, 0, 0, 255));
        // A repeated zero-width sweep is ill-formed: nothing is painted.
        for extend in [Extend::Repeat, Extend::Reflect] {
            let fill = sweep(90.0, 90.0, extend);
            assert_eq!(sweep_at(&fill, 0.0), Color::TRANSPARENT, "{extend:?}");
        }
    }

    fn opaque(r: u8, g: u8, b: u8) -> [u8; 4] {
        [r, g, b, 255]
    }

    #[test]
    fn porter_duff_composite_modes_follow_the_spec() {
        let red = opaque(255, 0, 0);
        let blue = opaque(0, 0, 255);
        // CLEAR wipes everything.
        assert_eq!(composite_pixel(0, red, blue), [0, 0, 0, 0]);
        // SRC/DST pick one side wholesale.
        assert_eq!(composite_pixel(1, red, blue), red);
        assert_eq!(composite_pixel(2, red, blue), blue);
        // SRC_OVER keeps the source where it is opaque.
        assert_eq!(composite_pixel(3, red, blue), red);
        // SRC_IN / DST_IN are multiplicative on alpha.
        assert_eq!(composite_pixel(5, red, blue), red);
        assert_eq!(composite_pixel(6, red, blue), blue);
        // SRC_OUT / XOR remove the overlap, which is all of it here.
        assert_eq!(composite_pixel(7, red, blue)[3], 0);
        assert_eq!(composite_pixel(11, red, blue)[3], 0);
        // PLUS adds the channels and clamps.
        assert_eq!(composite_pixel(12, red, blue), opaque(255, 0, 255));
        // A transparent backdrop leaves SRC_IN with nothing.
        assert_eq!(composite_pixel(5, red, [0, 0, 0, 0])[3], 0);
    }

    #[test]
    fn separable_blend_modes_match_the_spec() {
        let red = opaque(255, 0, 0);
        let blue = opaque(0, 0, 255);
        // MULTIPLY of complementary channels is black.
        assert_eq!(composite_pixel(23, red, blue), opaque(0, 0, 0));
        // SCREEN is the inverse of multiply: full magenta.
        assert_eq!(composite_pixel(13, red, blue), opaque(255, 0, 255));
        // DARKEN / LIGHTEN take per-channel extremes: red and blue share no
        // channel, so one mode gives black and the other magenta.
        assert_eq!(composite_pixel(15, red, blue), opaque(0, 0, 0));
        assert_eq!(composite_pixel(16, red, blue), opaque(255, 0, 255));
        // DIFFERENCE of red over blue is magenta.
        let diff = composite_pixel(21, red, blue);
        assert_eq!(diff[3], 255);
        assert!(diff[0] > 250 && diff[2] > 250, "{diff:?}");
        // OVERLAY is hard-light with the backdrop deciding: a fully blue
        // backdrop stays blue.
        assert_eq!(composite_pixel(14, red, blue), opaque(0, 0, 255));
        // MULTIPLY with a mid grey darkens on every channel.
        let grey = opaque(128, 128, 128);
        let out = composite_pixel(23, grey, opaque(200, 200, 200));
        assert!(out[0] < 150, "{out:?}");
    }

    #[test]
    fn non_separable_blend_modes_keep_luminance() {
        // HUE / SATURATION / COLOR / LUMINOSITY must stay in gamut and keep
        // the alpha of a source-over composite.
        for mode in 24..=27 {
            let px = composite_pixel(mode, opaque(10, 200, 90), opaque(240, 30, 70));
            assert_eq!(px[3], 255, "mode {mode}");
        }
        // LUMINOSITY of the source over an opaque backdrop differs from the
        // backdrop itself, so the mode really did something.
        let src = opaque(255, 255, 255);
        let dst = opaque(0, 0, 255);
        assert_ne!(composite_pixel(27, src, dst), dst);
    }

    #[test]
    fn composite_with_transparent_source_leaves_the_backdrop() {
        // Source-over with no source is the backdrop.
        let dst = opaque(20, 200, 120);
        assert_eq!(composite_pixel(3, [0, 0, 0, 0], dst), dst);
    }

    #[test]
    fn clipping_a_square_in_half_keeps_half_its_area() {
        let square = vec![
            Point { x: 0.0, y: 0.0 },
            Point { x: 100.0, y: 0.0 },
            Point { x: 100.0, y: 100.0 },
            Point { x: 0.0, y: 100.0 },
        ];
        let clipped = clip_contours(std::slice::from_ref(&square), [0.0, 0.0, 50.0, 100.0]);
        assert_eq!(clipped.len(), 1);
        let xs: Vec<f32> = clipped[0].iter().map(|p| p.x).collect();
        assert!(xs.iter().all(|x| *x <= 50.0 + 1e-3), "{xs:?}");
        assert!(xs.iter().any(|x| (*x - 50.0).abs() < 1e-3), "{xs:?}");

        // A rectangle that misses the polygon removes it entirely.
        let none = clip_contours(&[square], [200.0, 200.0, 300.0, 300.0]);
        assert!(none.is_empty());
    }

    #[test]
    fn composite_shapes_punch_holes_with_xor() {
        // Two overlapping squares, combined with XOR: only the parts that
        // belong to exactly one square remain.
        let left = vec![
            Point { x: 10.0, y: 10.0 },
            Point { x: 60.0, y: 10.0 },
            Point { x: 60.0, y: 60.0 },
            Point { x: 10.0, y: 60.0 },
        ];
        let right = vec![
            Point { x: 40.0, y: 10.0 },
            Point { x: 90.0, y: 10.0 },
            Point { x: 90.0, y: 60.0 },
            Point { x: 40.0, y: 60.0 },
        ];
        let shape = Shape::Composite {
            mode: 11,
            source: Box::new(Shape::Fill {
                polys: vec![right],
                fill: Fill::Solid(Color::rgba(0, 0, 255, 255)),
            }),
            backdrop: Box::new(Shape::Fill {
                polys: vec![left],
                fill: Fill::Solid(Color::rgba(255, 0, 0, 255)),
            }),
        };
        let mut bmp = Bitmap::new(100, 80);
        shape.render(&mut bmp, 0.0, 0.0);
        // Left-only, right-only and overlap.
        assert_eq!(bmp.pixel(20, 35)[3], 255);
        assert_eq!(bmp.pixel(80, 35)[3], 255);
        assert_eq!(bmp.pixel(50, 35)[3], 0, "the overlap must be cleared");
        assert_eq!(bmp.pixel(5, 5)[3], 0);
    }

    #[test]
    fn composite_renders_into_a_clipped_scratch_area() {
        // A composite whose bounds lie partly outside the target must still
        // paint the visible remainder.
        let big = vec![
            Point { x: -50.0, y: 10.0 },
            Point { x: 50.0, y: 10.0 },
            Point { x: 50.0, y: 50.0 },
            Point { x: -50.0, y: 50.0 },
        ];
        let shape = Shape::Composite {
            mode: 3,
            source: Box::new(Shape::Fill {
                polys: vec![big.clone()],
                fill: Fill::Solid(Color::rgba(0, 128, 0, 255)),
            }),
            backdrop: Box::new(Shape::Fill {
                polys: vec![big],
                fill: Fill::Solid(Color::rgba(0, 0, 128, 255)),
            }),
        };
        let mut bmp = Bitmap::new(60, 60);
        shape.render(&mut bmp, 0.0, 0.0);
        // Inside the target, the source wins; outside it nothing leaks.
        assert_eq!(bmp.pixel(10, 30), [0, 128, 0, 255]);
        assert_eq!(bmp.pixel(55, 30)[3], 0);
    }

    /// Horizontal ink span (first and last inked column) of a text run.
    fn ink_span(fonts: &LoadedFonts, text: &str, size: f32, spacing: f32) -> Option<(u32, u32)> {
        let scene = Scene {
            width: 600.0,
            height: size * 2.0,
            background: None,
            draws: vec![Draw::Text(TextDraw {
                x: 20.0,
                y: size * 1.5,
                text: text.into(),
                size,
                family: FontKind::Sans,
                color: Color::BLACK,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: spacing,
            })],
        };
        let bmp = Rasterizer::new(1.0).rasterize(&scene, fonts);
        let mut span: Option<(u32, u32)> = None;
        for x in 0..bmp.width {
            let inked = (0..bmp.height).any(|y| bmp.pixel(x, y)[3] > 32);
            if inked {
                span = Some(match span {
                    Some((first, _)) => (first, x),
                    None => (x, x),
                });
            }
        }
        span
    }

    #[test]
    fn letter_spacing_widens_raster_text() {
        let Some(fonts) = test_fonts() else {
            eprintln!("skipping: no fonts");
            return;
        };
        // Four narrow glyphs: three gaps between them can grow.
        let Some((plain_first, plain_last)) = ink_span(&fonts, "iiii", 40.0, 0.0) else {
            eprintln!("skipping: no ink");
            return;
        };
        let Some((wide_first, wide_last)) = ink_span(&fonts, "iiii", 40.0, 20.0) else {
            eprintln!("skipping: no ink");
            return;
        };
        assert_eq!(plain_first, wide_first, "the run must start at the same x");
        let plain = plain_last - plain_first;
        let wide = wide_last - wide_first;
        // Three inter-glyph gaps of 20px each.
        let expected = plain as i64 + 60;
        assert!(
            (wide as i64 - expected).abs() <= 6,
            "letter spacing should add 3 x 20px: {plain} -> {wide} (expected ~{expected})"
        );
    }

    #[test]
    fn letter_spacing_matches_the_layout_metrics() {
        // The raster advance must agree with the width the layout reserved,
        // otherwise text overflows its box or leaves a gap.
        let Some(fonts) = test_fonts() else {
            eprintln!("skipping: no fonts");
            return;
        };
        let style = TextStyle {
            size: 20.0,
            family: FontKind::Sans,
            bold: false,
            italic: false,
            letter_spacing: 0.25,
        };
        let metrics = Metrics::new();
        let reserved: f32 = "MMMM".chars().map(|c| metrics.advance(c, &style)).sum();
        let Some((first, last)) = ink_span(&fonts, "MMMM", 20.0, 5.0) else {
            eprintln!("skipping: no ink");
            return;
        };
        let drawn = (last - first) as f32 + 5.0;
        assert!(
            drawn < reserved + 4.0,
            "raster text is wider than the layout reserved: {drawn} > {reserved}"
        );
    }
}
