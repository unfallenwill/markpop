//! Geometry-first drawing primitives.
//!
//! [`Scene`] is the layout engine's output: a flat, ordered list of
//! positioned primitives plus a canvas size. Keeping this representation
//! independent of SVG means the layout is trivially testable (assert on
//! coordinates) and new output formats could be added later.

use crate::color::Color;

/// Which font slot a text primitive uses; resolved to a family by the
/// [`Theme`](crate::Theme) at serialization time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontKind {
    /// The theme's proportional sans-serif family.
    Sans,
    /// The theme's monospace family.
    Mono,
}

/// A single positioned drawing primitive.
///
/// Primitives are drawn in list order, so later entries paint on top of
/// earlier ones. This is how e.g. inline-code backgrounds sit behind their
/// text.
#[derive(Debug, Clone, PartialEq)]
pub enum Draw {
    /// A run of text sharing one style. `y` is the *baseline*.
    Text(TextDraw),
    /// A (optionally rounded) rectangle, filled and/or stroked.
    Rect(RectDraw),
    /// A straight line segment.
    Line(LineDraw),
    /// A decoded image placed at a rectangle.
    Image(ImageDraw),
}

/// Positioned text; `y` is the baseline of the first (only) line.
#[derive(Debug, Clone, PartialEq)]
pub struct TextDraw {
    /// Left edge of the text run (start of the first glyph's advance).
    pub x: f32,
    /// Baseline of the text run.
    pub y: f32,
    /// Raw (unescaped) text content.
    pub text: String,
    /// Font size in canvas units (pixels at 100% zoom).
    pub size: f32,
    /// Font slot to use.
    pub family: FontKind,
    /// Fill color.
    pub color: Color,
    /// Bold weight.
    pub bold: bool,
    /// Italic style.
    pub italic: bool,
    /// Underline decoration.
    pub underline: bool,
    /// Strikethrough decoration.
    pub strike: bool,
    /// Extra advance between characters, in canvas units (`0.0` = none).
    pub letter_spacing: f32,
}

impl TextDraw {
    /// Vertical distance from the baseline to the top of the em box,
    /// approximated as `0.8 * size` (typical sans ascent).
    pub fn ascent(&self) -> f32 {
        0.8 * self.size
    }

    /// Vertical distance from the baseline to the bottom of the em box,
    /// approximated as `0.2 * size` (typical sans descent).
    pub fn descent(&self) -> f32 {
        0.2 * self.size
    }
}

/// A rectangle primitive with optional rounded corners.
#[derive(Debug, Clone, PartialEq)]
pub struct RectDraw {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// Corner radius (`0` for square corners).
    pub radius: f32,
    /// Fill color, if filled.
    pub fill: Option<Color>,
    /// Stroke color, if stroked.
    pub stroke: Option<Color>,
    /// Stroke width (ignored without [`Self::stroke`]).
    pub stroke_width: f32,
}

impl RectDraw {
    /// Creates an axis-aligned rectangle: `x..x+w`, `y..y+h`.
    ///
    /// Negative dimensions are normalized so the rect is always well-formed.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> RectDraw {
        let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
        let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
        RectDraw {
            x,
            y,
            w,
            h,
            radius: 0.0,
            fill: None,
            stroke: None,
            stroke_width: 1.0,
        }
    }

    /// Sets the fill color (builder style).
    pub fn with_fill(mut self, fill: Color) -> RectDraw {
        self.fill = Some(fill);
        self
    }

    /// Sets the stroke color and width (builder style).
    pub fn with_stroke(mut self, color: Color, width: f32) -> RectDraw {
        self.stroke = Some(color);
        self.stroke_width = width;
        self
    }

    /// Sets the corner radius (builder style).
    pub fn with_radius(mut self, radius: f32) -> RectDraw {
        self.radius = radius.max(0.0);
        self
    }
}

/// A line segment from `(x1, y1)` to `(x2, y2)`.
#[derive(Debug, Clone, PartialEq)]
pub struct LineDraw {
    /// Start x.
    pub x1: f32,
    /// Start y.
    pub y1: f32,
    /// End x.
    pub x2: f32,
    /// End y.
    pub y2: f32,
    /// Stroke color.
    pub color: Color,
    /// Stroke width.
    pub width: f32,
}

/// A decoded image placed at a rectangle; `image` indexes
/// [`Scene::images`]. The rectangle may be a scaled version of the
/// source; serializers preserve the aspect the layout computed.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageDraw {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Rendered width.
    pub w: f32,
    /// Rendered height.
    pub h: f32,
    /// Index into [`Scene::images`].
    pub image: usize,
}

/// A complete, self-contained picture: canvas size plus ordered primitives.
///
/// Coordinates are in *base* canvas units (pixels at 100% zoom). Zoom is
/// applied only when serializing, which keeps scenes zoom-independent.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Canvas width in base units.
    pub width: f32,
    /// Canvas height in base units.
    pub height: f32,
    /// Page background rectangle covering `0..width`, `0..height`, if any.
    pub background: Option<RectDraw>,
    /// Ordered drawing primitives (bottom-to-top paint order).
    pub draws: Vec<Draw>,
    /// Image payloads referenced by [`Draw::Image`] indexes. Kept in a
    /// side table so primitives stay small and copyable.
    pub images: Vec<SceneImage>,
}

/// One decoded image attached to a [`Scene`].
///
/// `pixels` is straight-alpha RGBA8 for the rasterizer; `embed` holds a
/// complete PNG file (the original bytes when the source was PNG, a
/// re-encode otherwise) for self-contained SVG data URIs.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneImage {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// `width * height * 4` bytes, straight alpha.
    pub pixels: Vec<u8>,
    /// Complete PNG file bytes for the SVG data URI.
    pub embed: Vec<u8>,
}

impl Draw {
    /// Returns a copy of this primitive translated by `(dx, dy)`.
    ///
    /// Used by pagination and multi-column flow to move whole blocks
    /// without re-laying them out.
    pub fn translated(&self, dx: f32, dy: f32) -> Draw {
        match self {
            Draw::Text(t) => Draw::Text(TextDraw {
                x: t.x + dx,
                y: t.y + dy,
                ..t.clone()
            }),
            Draw::Rect(r) => Draw::Rect(RectDraw {
                x: r.x + dx,
                y: r.y + dy,
                ..r.clone()
            }),
            Draw::Line(l) => Draw::Line(LineDraw {
                x1: l.x1 + dx,
                y1: l.y1 + dy,
                x2: l.x2 + dx,
                y2: l.y2 + dy,
                ..l.clone()
            }),
            Draw::Image(i) => Draw::Image(ImageDraw {
                x: i.x + dx,
                y: i.y + dy,
                ..i.clone()
            }),
        }
    }
}

impl Scene {
    /// Creates an empty scene with the given canvas size.
    pub fn new(width: f32, height: f32) -> Scene {
        Scene {
            width,
            height,
            background: None,
            draws: Vec::new(),
            images: Vec::new(),
        }
    }

    /// Number of primitives in the scene.
    pub fn len(&self) -> usize {
        self.draws.len()
    }

    /// Whether the scene contains no primitives.
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    /// Finds the first text primitive containing `needle`, if any.
    ///
    /// A convenience for tests and downstream tooling.
    pub fn find_text(&self, needle: &str) -> Option<&TextDraw> {
        self.draws.iter().find_map(|d| match d {
            Draw::Text(t) if t.text.contains(needle) => Some(t),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_text(x: f32, y: f32) -> TextDraw {
        TextDraw {
            x,
            y,
            text: "hello".into(),
            size: 16.0,
            family: FontKind::Sans,
            color: Color::BLACK,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
        }
    }

    #[test]
    fn rect_normalizes_negative_sizes() {
        let r = RectDraw::new(10.0, 20.0, -30.0, -40.0);
        assert_eq!((r.x, r.y, r.w, r.h), (-20.0, -20.0, 30.0, 40.0));
    }

    #[test]
    fn rect_builders_chain() {
        let r = RectDraw::new(0.0, 0.0, 5.0, 5.0)
            .with_fill(Color::WHITE)
            .with_stroke(Color::BLACK, 2.0)
            .with_radius(3.0);
        assert_eq!(r.fill, Some(Color::WHITE));
        assert_eq!(r.stroke, Some(Color::BLACK));
        assert_eq!(r.stroke_width, 2.0);
        assert_eq!(r.radius, 3.0);
    }

    #[test]
    fn rect_radius_never_negative() {
        let r = RectDraw::new(0.0, 0.0, 1.0, 1.0).with_radius(-5.0);
        assert_eq!(r.radius, 0.0);
    }

    #[test]
    fn scene_find_text_matches_substring() {
        let mut scene = Scene::new(100.0, 100.0);
        scene.draws.push(Draw::Text(sample_text(1.0, 2.0)));
        let found = scene.find_text("ell").expect("should match");
        assert_eq!(found.x, 1.0);
        assert!(scene.find_text("nope").is_none());
    }

    #[test]
    fn scene_len_and_emptiness() {
        let mut scene = Scene::new(10.0, 10.0);
        assert!(scene.is_empty());
        scene.draws.push(Draw::Text(sample_text(0.0, 0.0)));
        assert_eq!(scene.len(), 1);
        assert!(!scene.is_empty());
    }

    #[test]
    fn text_ascent_descent_scale_with_size() {
        let t = sample_text(0.0, 0.0);
        assert_eq!(t.ascent(), 12.8);
        assert_eq!(t.descent(), 3.2);
    }

    #[test]
    fn draws_can_be_translated() {
        let text = Draw::Text(sample_text(10.0, 20.0));
        match text.translated(5.0, -3.0) {
            Draw::Text(t) => assert_eq!((t.x, t.y), (15.0, 17.0)),
            _ => panic!("expected text"),
        }
        let line = Draw::Line(LineDraw {
            x1: 1.0,
            y1: 2.0,
            x2: 3.0,
            y2: 4.0,
            color: Color::BLACK,
            width: 1.0,
        });
        match line.translated(10.0, 20.0) {
            Draw::Line(l) => {
                assert_eq!((l.x1, l.y1, l.x2, l.y2), (11.0, 22.0, 13.0, 24.0))
            }
            _ => panic!("expected line"),
        }
        let rect = Draw::Rect(RectDraw::new(1.0, 2.0, 3.0, 4.0));
        match rect.translated(-1.0, -2.0) {
            Draw::Rect(r) => assert_eq!((r.x, r.y), (0.0, 0.0)),
            _ => panic!("expected rect"),
        }
    }
}
