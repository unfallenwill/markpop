//! SVG serialization: [`Scene`] in, well-formed SVG text out.
//!
//! This layer knows nothing about Markdown. It only needs a scene, a theme
//! (for font stacks) and a zoom factor, which makes it straightforward to
//! test by inspecting the emitted markup.
//!
//! Zoom is applied through the `viewBox`: geometry is written in base
//! units and only the root `width`/`height` scale. Output therefore stays
//! vector-crisp at any zoom and the coordinates never drift.

use std::fmt::Write;

use crate::scene::{Draw, Scene, TextDraw};

const B64: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64-encodes `data` (with padding, no line breaks).
fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// Serializes a scene to an SVG document.
///
/// `zoom` is a multiplier where `1.0` is 100%.
///
/// ```
/// use markpop::{scene::Scene, svg};
///
/// let scene = Scene::new(100.0, 50.0);
/// let out = svg::render(&scene, &markpop::Theme::light(), 2.0);
/// assert!(out.contains(r#"width="200""#));
/// assert!(out.contains(r#"viewBox="0 0 100 50""#));
/// ```
pub fn render(scene: &Scene, theme: &crate::theme::Theme, zoom: f32) -> String {
    let w = scene.width * zoom;
    let h = scene.height * zoom;
    let mut out = String::with_capacity(1024 + scene.draws.len() * 96);

    let _ = writeln!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" role=\"img\" \
         width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" \
         font-family=\"{}\">",
        num(w),
        num(h),
        num(scene.width),
        num(scene.height),
        escape(&theme.sans_family),
    );

    if let Some(bg) = &scene.background {
        let _ = writeln!(out, "  {}", rect(bg));
    }

    for draw in &scene.draws {
        match draw {
            Draw::Rect(r) => {
                let _ = writeln!(out, "  {}", rect(r));
            }
            Draw::Line(l) => {
                let _ = writeln!(
                    out,
                    "  <line x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\" stroke=\"{}\" stroke-width=\"{}\"/>",
                    num(l.x1),
                    num(l.y1),
                    num(l.x2),
                    num(l.y2),
                    l.color,
                    num(l.width),
                );
            }
            Draw::Text(t) => {
                let _ = writeln!(out, "  {}", text(t, theme));
            }
            Draw::Image(i) => {
                let Some(payload) = scene.images.get(i.image) else {
                    continue;
                };
                let _ = writeln!(
                    out,
                    "  <image x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
                     preserveAspectRatio=\"none\" href=\"data:image/png;base64,{}\"/>",
                    num(i.x),
                    num(i.y),
                    num(i.w),
                    num(i.h),
                    base64(&payload.embed),
                );
            }
        }
    }

    out.push_str("</svg>\n");
    out
}

fn rect(r: &crate::scene::RectDraw) -> String {
    let mut s = format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"",
        num(r.x),
        num(r.y),
        num(r.w),
        num(r.h)
    );
    if r.radius > 0.0 {
        let _ = write!(s, " rx=\"{}\"", num(r.radius));
    }
    let _ = write!(
        s,
        " fill=\"{}\"",
        r.fill
            .map(|c| c.to_string())
            .unwrap_or_else(|| "none".into())
    );
    if let Some(stroke) = r.stroke {
        let _ = write!(
            s,
            " stroke=\"{stroke}\" stroke-width=\"{}\"",
            num(r.stroke_width)
        );
    }
    s.push_str("/>");
    s
}

fn text(t: &TextDraw, theme: &crate::theme::Theme) -> String {
    let mut s = format!(
        "<text x=\"{}\" y=\"{}\" font-size=\"{}\" fill=\"{}\" xml:space=\"preserve\"",
        num(t.x),
        num(t.y),
        num(t.size),
        t.color
    );
    match t.family {
        crate::scene::FontKind::Mono => {
            let _ = write!(s, " font-family=\"{}\"", escape(&theme.mono_family));
        }
        crate::scene::FontKind::Sans => {}
    }
    if t.bold {
        s.push_str(" font-weight=\"bold\"");
    }
    if t.italic {
        s.push_str(" font-style=\"italic\"");
    }
    if t.letter_spacing > 0.0 {
        let _ = write!(s, " letter-spacing=\"{}\"", num(t.letter_spacing));
    }
    // Underline and strikethrough are drawn as explicit `<line>` primitives
    // by the layout engine. Emitting `text-decoration` as well would make
    // every renderer add its own second decoration at a slightly different
    // offset, so the attribute is deliberately omitted.
    let _ = write!(s, ">{}</text>", escape(&t.text));
    s
}

/// Formats a coordinate with at most two decimals, dropping a trailing
/// `.0` so the output is compact and stable across renders.
pub(crate) fn num(v: f32) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    if rounded == 0.0 {
        return "0".to_string();
    }
    if rounded.fract() == 0.0 && rounded.abs() < 1e9 {
        format!("{}", rounded as i64)
    } else {
        let s = format!("{rounded:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Escapes XML text content and attribute values.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{FontKind, LineDraw, RectDraw, TextDraw};
    use crate::theme::Theme;

    fn text_draw(text: &str) -> TextDraw {
        TextDraw {
            x: 10.0,
            y: 20.0,
            text: text.to_string(),
            size: 16.0,
            family: FontKind::Sans,
            color: crate::color::Color::BLACK,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
        }
    }

    #[test]
    fn empty_scene_is_valid_svg() {
        let out = render(&Scene::new(100.0, 50.0), &Theme::light(), 1.0);
        assert!(out.starts_with("<svg"));
        assert!(out.ends_with("</svg>\n"));
        assert!(out.contains(r#"viewBox="0 0 100 50""#));
        assert!(out.contains(r#"width="100""#));
        assert!(out.contains(r#"height="50""#));
    }

    #[test]
    fn zoom_one_is_default_scale() {
        let out = render(&Scene::new(800.0, 400.0), &Theme::light(), 1.0);
        assert!(out.contains(r#"width="800""#));
        assert!(out.contains(r#"height="400""#));
    }

    #[test]
    fn zoom_scales_output_size_not_viewbox() {
        let out = render(&Scene::new(800.0, 400.0), &Theme::light(), 1.5);
        assert!(out.contains(r#"width="1200""#));
        assert!(out.contains(r#"height="600""#));
        assert!(out.contains(r#"viewBox="0 0 800 400""#));
    }

    #[test]
    fn zoom_is_lossless_for_geometry() {
        let mut scene = Scene::new(100.0, 100.0);
        scene.draws.push(Draw::Text(text_draw("hi")));
        let a = render(&scene, &Theme::light(), 1.0);
        let b = render(&scene, &Theme::light(), 4.0);
        let coords = |s: &str| {
            s.lines()
                .filter(|l| l.trim_start().starts_with("<text"))
                .map(|l| l.split('>').next().unwrap_or("").to_string())
                .collect::<String>()
        };
        assert_eq!(coords(&a), coords(&b), "geometry must not change with zoom");
    }

    #[test]
    fn background_rect_is_emitted_first() {
        let mut scene = Scene::new(10.0, 10.0);
        scene.background =
            Some(RectDraw::new(0.0, 0.0, 10.0, 10.0).with_fill(crate::color::Color::WHITE));
        scene.draws.push(Draw::Text(text_draw("x")));
        let out = render(&scene, &Theme::light(), 1.0);
        let bg = out.find("<rect").unwrap();
        let txt = out.find("<text").unwrap();
        assert!(bg < txt);
    }

    #[test]
    fn rect_radius_becomes_rx() {
        let mut scene = Scene::new(10.0, 10.0);
        scene.draws.push(Draw::Rect(
            RectDraw::new(0.0, 0.0, 5.0, 5.0)
                .with_radius(2.0)
                .with_fill(crate::color::Color::WHITE),
        ));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(out.contains(r#"rx="2""#), "{out}");
    }

    #[test]
    fn rect_stroke_is_emitted() {
        let mut scene = Scene::new(10.0, 10.0);
        scene.draws.push(Draw::Rect(
            RectDraw::new(0.0, 0.0, 5.0, 5.0).with_stroke(crate::color::Color::BLACK, 2.0),
        ));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(out.contains(r##"stroke="#000000""##), "{out}");
        assert!(out.contains(r#"stroke-width="2""#), "{out}");
    }

    #[test]
    fn line_is_emitted() {
        let mut scene = Scene::new(10.0, 10.0);
        scene.draws.push(Draw::Line(LineDraw {
            x1: 1.0,
            y1: 2.0,
            x2: 3.0,
            y2: 4.0,
            color: crate::color::Color::BLACK,
            width: 1.5,
        }));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(
            out.contains(r#"<line x1="1" y1="2" x2="3" y2="4""#),
            "{out}"
        );
        assert!(out.contains(r#"stroke-width="1.5""#));
    }

    #[test]
    fn text_attributes() {
        let mut scene = Scene::new(10.0, 10.0);
        let mut t = text_draw("hello");
        t.bold = true;
        t.italic = true;
        t.underline = true;
        scene.draws.push(Draw::Text(t));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(out.contains("font-weight=\"bold\""), "{out}");
        assert!(out.contains("font-style=\"italic\""), "{out}");
        // Decoration primitives are drawn by layout, not by the renderer.
        assert!(!out.contains("text-decoration"), "{out}");
        assert!(out.contains(">hello</text>"));
    }

    #[test]
    fn strikethrough_does_not_rely_on_text_decoration() {
        let mut scene = Scene::new(10.0, 10.0);
        let mut t = text_draw("gone");
        t.strike = true;
        scene.draws.push(Draw::Text(t));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(!out.contains("line-through"), "{out}");
    }

    #[test]
    fn letter_spacing_is_emitted_only_when_set() {
        let mut scene = Scene::new(10.0, 10.0);
        let mut t = text_draw("spaced");
        t.letter_spacing = 1.5;
        scene.draws.push(Draw::Text(t));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(out.contains("letter-spacing=\"1.5\""), "{out}");

        let plain = render(&Scene::new(10.0, 10.0), &Theme::light(), 1.0);
        assert!(!plain.contains("letter-spacing"));
    }

    #[test]
    fn mono_text_gets_its_own_family() {
        let mut scene = Scene::new(10.0, 10.0);
        let mut t = text_draw("code");
        t.family = FontKind::Mono;
        scene.draws.push(Draw::Text(t));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(out.contains("monospace"), "{out}");
    }

    #[test]
    fn xml_special_characters_are_escaped() {
        let mut scene = Scene::new(10.0, 10.0);
        scene
            .draws
            .push(Draw::Text(text_draw("a < b & c > d \"q\" 'p'")));
        let out = render(&scene, &Theme::light(), 1.0);
        assert!(
            out.contains("a &lt; b &amp; c &gt; d &quot;q&quot; &apos;p&apos;"),
            "{out}"
        );
        assert!(!out.contains("a < b"), "raw markup leaked: {out}");
    }

    #[test]
    fn escape_helper_covers_all_five_entities() {
        assert_eq!(escape("&<>\"'"), "&amp;&lt;&gt;&quot;&apos;");
        assert_eq!(escape("plain"), "plain");
    }

    #[test]
    fn numbers_are_compact() {
        assert_eq!(num(0.0), "0");
        assert_eq!(num(-0.0), "0");
        assert_eq!(num(1.0), "1");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num(1.234), "1.23");
        assert_eq!(num(1.999), "2");
        assert_eq!(num(-3.25), "-3.25");
        assert_eq!(num(1000.0), "1000");
    }

    #[test]
    fn output_is_deterministic() {
        let mut scene = Scene::new(50.0, 50.0);
        scene.draws.push(Draw::Text(text_draw("same")));
        let a = render(&scene, &Theme::light(), 1.0);
        let b = render(&scene, &Theme::light(), 1.0);
        assert_eq!(a, b);
    }

    #[test]
    fn transparent_background_emits_no_rect() {
        let mut theme = Theme::light();
        theme.background = None;
        let out = render(&Scene::new(10.0, 10.0), &theme, 1.0);
        assert!(!out.contains("<rect"), "{out}");
    }

    #[test]
    fn font_family_is_declared_once_on_the_root() {
        let out = render(&Scene::new(10.0, 10.0), &Theme::light(), 1.0);
        assert_eq!(out.matches("Segoe UI").count(), 1, "{out}");
    }
}
