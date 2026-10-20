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
use crate::font::{Contour, LoadedFonts, Point};
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
            fill_polygons(bmp, std::slice::from_ref(&poly), fill);
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
        fill_polygons(bmp, &[quad], l.color);
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
        let mut polys: Vec<Contour> = Vec::new();
        // Faux-bold offset; real bold faces are not required.
        let bold_dx = (size_px * 0.055).max(0.45);

        for ch in t.text.chars() {
            let face = fonts
                .find(ch, mono)
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
                    for contour in face.outline(gid, flatness) {
                        push_contour(&mut polys, &contour, pen, baseline, k, t.italic, 0.0);
                        if t.bold {
                            push_contour(&mut polys, &contour, pen, baseline, k, t.italic, bold_dx);
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
        if !polys.is_empty() {
            fill_polygons(bmp, &polys, t.color);
        }
    }
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
        fill_polygons(bmp, &[quad], color);
    }
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
pub fn fill_polygons(bmp: &mut Bitmap, polys: &[Contour], color: Color) {
    if color.a == 0 {
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
}
