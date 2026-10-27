//! Output engines: the same [`Scene`] can be serialized as vector SVG or
//! rasterized into PNG / lossless WebP.
//!
//! Layout is shared by every engine — only the last step differs:
//!
//! ```text
//!                        ┌─ SvgEngine  → SVG text
//! Scene ─────────────────┤
//!                        └─ BitmapEngine → raster → PNG / WebP
//! ```
//!
//! Raster engines need glyph outlines, so they load a TrueType face from the
//! system (or a file supplied by the caller). SVG output needs no font: the
//! viewer resolves the font stack.

use std::path::Path;

use crate::encode::{png, webp};
use crate::font::{self, Face, FontSet, LoadedFonts};
use crate::options::RenderError;
use crate::raster::Rasterizer;
use crate::scene::Scene;
use crate::svg;
use crate::theme::Theme;

/// Output format of a render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OutputFormat {
    /// Vector SVG (default): crisp at any size, text stays selectable.
    #[default]
    Svg,
    /// Raster PNG: widest compatibility with viewers and image tools.
    Png,
    /// Raster lossless WebP: smallest files for a bitmap.
    Webp,
    /// A Microsoft Word document (OOXML).
    ///
    /// Unlike the other formats this one is written from the document,
    /// not from a laid-out scene, so it never reaches the scene engines.
    Docx,
}

impl OutputFormat {
    /// Parses a format name (`svg`, `png`, `webp`; case-insensitive).
    pub fn by_name(name: &str) -> Option<OutputFormat> {
        match name.trim().to_ascii_lowercase().as_str() {
            "svg" => Some(OutputFormat::Svg),
            "png" => Some(OutputFormat::Png),
            "webp" | "webp-lossless" => Some(OutputFormat::Webp),
            "docx" => Some(OutputFormat::Docx),
            _ => None,
        }
    }

    /// Conventional file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Svg => "svg",
            OutputFormat::Png => "png",
            OutputFormat::Webp => "webp",
            OutputFormat::Docx => "docx",
        }
    }

    /// MIME type of the output.
    pub fn mime(self) -> &'static str {
        match self {
            OutputFormat::Svg => "image/svg+xml",
            OutputFormat::Png => "image/png",
            OutputFormat::Webp => "image/webp",
            OutputFormat::Docx => {
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            }
        }
    }

    /// Whether this format is raster (needs fonts).
    pub fn is_raster(self) -> bool {
        !matches!(self, OutputFormat::Svg | OutputFormat::Docx)
    }
}

/// A pluggable output stage.
///
/// Engines are stateless with respect to layout: they receive finished
/// scenes and produce bytes.
pub trait Engine {
    /// Format produced by this engine.
    fn format(&self) -> OutputFormat;

    /// Encodes one page.
    fn encode(&self, scene: &Scene, theme: &Theme, zoom: f32) -> Vec<u8>;
}

/// Vector SVG engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct SvgEngine;

impl Engine for SvgEngine {
    fn format(&self) -> OutputFormat {
        OutputFormat::Svg
    }

    fn encode(&self, scene: &Scene, theme: &Theme, zoom: f32) -> Vec<u8> {
        svg::render(scene, theme, zoom).into_bytes()
    }
}

/// Software-raster engine, used for PNG and lossless WebP.
#[derive(Debug, Clone)]
pub struct BitmapEngine {
    format: OutputFormat,
    fonts: LoadedFonts,
}

impl BitmapEngine {
    /// Creates a raster engine for `format`, loading fonts from the system
    /// or from `font_file`.
    pub fn new(format: OutputFormat, font_file: Option<&str>) -> Result<BitmapEngine, RenderError> {
        BitmapEngine::with_families(format, font_file, None, None)
    }

    /// Creates a raster engine that prefers faces matching the given font
    /// stacks (the first family name in each stack wins).
    pub fn with_families(
        format: OutputFormat,
        font_file: Option<&str>,
        sans_family: Option<&str>,
        mono_family: Option<&str>,
    ) -> Result<BitmapEngine, RenderError> {
        BitmapEngine::with_variations(format, font_file, sans_family, mono_family, &[])
    }

    /// Creates a raster engine with variable-font axis settings applied.
    pub fn with_variations(
        format: OutputFormat,
        font_file: Option<&str>,
        sans_family: Option<&str>,
        mono_family: Option<&str>,
        variations: &[(String, f32)],
    ) -> Result<BitmapEngine, RenderError> {
        if !format.is_raster() {
            return Err(RenderError::DocumentFormat(format));
        }
        Ok(BitmapEngine {
            format,
            fonts: load_fonts(font_file, sans_family, mono_family, variations)?,
        })
    }

    /// Number of loaded font faces (sans + mono).
    pub fn font_count(&self) -> usize {
        self.fonts.len()
    }

    /// Family name of the face that will render proportional text.
    pub fn sans_face_name(&self) -> &str {
        self.fonts.sans.first().map(|f| f.name()).unwrap_or("")
    }

    /// Rasterizes a scene with this engine's fonts.
    pub fn rasterize(&self, scene: &Scene, zoom: f32) -> crate::raster::Bitmap {
        Rasterizer::new(zoom).rasterize(scene, &self.fonts)
    }
}

impl Engine for BitmapEngine {
    fn format(&self) -> OutputFormat {
        self.format
    }

    fn encode(&self, scene: &Scene, theme: &Theme, zoom: f32) -> Vec<u8> {
        // The historical default: an engine built for SVG still produces SVG.
        if self.format == OutputFormat::Svg {
            return svg::render(scene, theme, zoom).into_bytes();
        }
        let bmp = self.rasterize(scene, zoom);
        match self.format {
            OutputFormat::Png => png::encode(bmp.width, bmp.height, &bmp.data),
            OutputFormat::Webp => webp::encode(bmp.width, bmp.height, &bmp.data),
            OutputFormat::Svg => unreachable!("handled above"),
            OutputFormat::Docx => unreachable!("a Word document is not written from a scene"),
        }
    }
}

/// Builds the engine for a format, loading fonts only when needed.
pub fn engine_for(
    format: OutputFormat,
    font_file: Option<&str>,
) -> Result<Box<dyn Engine>, RenderError> {
    engine_for_theme(format, font_file, None)
}

/// Builds the engine for a format, preferring fonts that match the theme's
/// family stacks when rasterizing.
pub fn engine_for_theme(
    format: OutputFormat,
    font_file: Option<&str>,
    theme: Option<&Theme>,
) -> Result<Box<dyn Engine>, RenderError> {
    engine_for_render(format, font_file, theme, &[])
}

/// Builds the engine for a format, with variable-font axis settings.
pub fn engine_for_render(
    format: OutputFormat,
    font_file: Option<&str>,
    theme: Option<&Theme>,
    variations: &[(String, f32)],
) -> Result<Box<dyn Engine>, RenderError> {
    match format {
        OutputFormat::Svg => Ok(Box::new(SvgEngine)),
        OutputFormat::Docx => Err(RenderError::DocumentFormat(format)),
        format => {
            let sans = theme.map(|t| t.sans_family.as_str());
            let mono = theme.map(|t| t.mono_family.as_str());
            Ok(Box::new(BitmapEngine::with_variations(
                format, font_file, sans, mono, variations,
            )?))
        }
    }
}

/// Loads glyph sources for raster output.
fn load_fonts(
    font_file: Option<&str>,
    sans_family: Option<&str>,
    mono_family: Option<&str>,
    variations: &[(String, f32)],
) -> Result<LoadedFonts, RenderError> {
    if let Some(path) = font_file {
        let mut face = Face::load(Path::new(path)).ok_or(RenderError::FontUnavailable)?;
        face.set_variations(variations);
        return Ok(LoadedFonts {
            sans: vec![face.clone()],
            mono: vec![face],
        });
    }
    let mut fonts = FontSet::load(&font::discover()).ok_or(RenderError::FontUnavailable)?;
    if !variations.is_empty() {
        for face in fonts.sans.iter_mut().chain(fonts.mono.iter_mut()) {
            face.set_variations(variations);
        }
    }
    if let Some(family) = sans_family {
        fonts.sans = prefer_family(fonts.sans, family);
    }
    if let Some(family) = mono_family {
        fonts.mono = prefer_family(fonts.mono, family);
    }
    Ok(fonts)
}

/// Moves faces whose family name matches `stack` to the front.
fn prefer_family(faces: Vec<Face>, stack: &str) -> Vec<Face> {
    let wanted = stack
        .split(',')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches(|c| c == '\x27' || c == '"')
        .to_lowercase();
    if wanted.is_empty() {
        return faces;
    }
    let mut ranked: Vec<(u8, Face)> = faces
        .into_iter()
        .map(|f| {
            let name = f.name().to_lowercase();
            let rank = if name == wanted {
                0
            } else if name.contains(&wanted) {
                1
            } else {
                2
            };
            (rank, f)
        })
        .collect();
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, f)| f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_roundtrip() {
        for (name, format) in [
            ("svg", OutputFormat::Svg),
            ("SVG", OutputFormat::Svg),
            ("png", OutputFormat::Png),
            ("WebP", OutputFormat::Webp),
        ] {
            assert_eq!(OutputFormat::by_name(name), Some(format));
            assert_eq!(name.to_ascii_lowercase(), format.extension());
        }
        assert_eq!(OutputFormat::by_name("jpeg"), None);
    }

    #[test]
    fn docx_is_a_known_format() {
        assert_eq!(OutputFormat::by_name("docx"), Some(OutputFormat::Docx));
        assert_eq!(OutputFormat::Docx.extension(), "docx");
        assert!(!OutputFormat::Docx.is_raster());
        assert!(OutputFormat::Docx.mime().contains("wordprocessingml"));
    }

    #[test]
    fn a_word_document_is_not_written_from_a_scene() {
        let Err(err) = engine_for(OutputFormat::Docx, None) else {
            panic!("a Word document must not come from the scene engines");
        };
        assert_eq!(err, RenderError::DocumentFormat(OutputFormat::Docx));
        let Err(err) = BitmapEngine::new(OutputFormat::Docx, None) else {
            panic!("a Word document must not come from the scene engines");
        };
        assert_eq!(err, RenderError::DocumentFormat(OutputFormat::Docx));
    }

    #[test]
    fn mime_types_are_correct() {
        assert_eq!(OutputFormat::Svg.mime(), "image/svg+xml");
        assert_eq!(OutputFormat::Png.mime(), "image/png");
        assert_eq!(OutputFormat::Webp.mime(), "image/webp");
        assert!(!OutputFormat::Svg.is_raster());
        assert!(OutputFormat::Png.is_raster());
    }

    #[test]
    fn svg_engine_matches_the_serializer() {
        let scene = crate::to_scene("# Hi", &Theme::light());
        let via_engine = SvgEngine.encode(&scene, &Theme::light(), 1.0);
        let direct = svg::render(&scene, &Theme::light(), 1.0).into_bytes();
        assert_eq!(via_engine, direct);
    }

    #[test]
    fn factory_picks_the_right_engine() {
        let engine = engine_for(OutputFormat::Svg, None).unwrap();
        assert_eq!(engine.format(), OutputFormat::Svg);
    }

    #[test]
    fn raster_engine_produces_png_and_webp() {
        let scene = crate::to_scene("# Hi\n\nWorld", &Theme::light());
        for format in [OutputFormat::Png, OutputFormat::Webp] {
            let Ok(engine) = engine_for(format, None) else {
                eprintln!("skipping: no system fonts");
                return;
            };
            let bytes = engine.encode(&scene, &Theme::light(), 1.0);
            let magic: &[u8] = match format {
                OutputFormat::Png => &[0x89, b'P', b'N', b'G'],
                _ => b"RIFF",
            };
            assert_eq!(&bytes[..magic.len()], magic);
            assert!(bytes.len() > 100);
        }
    }

    #[test]
    fn missing_font_file_is_reported() {
        let err = BitmapEngine::new(OutputFormat::Png, Some("no-such-font.ttf")).unwrap_err();
        assert_eq!(err, RenderError::FontUnavailable);
        assert!(err.to_string().contains("font"));
    }
    #[test]
    fn theme_family_is_preferred_for_raster_fonts() {
        use crate::font::discover;
        let set = discover();
        // Pick a CJK family that is actually installed and ask for it.
        let mut wanted = None;
        for path in &set.fallback {
            if let Some(face) = Face::load(path)
                && face.has_glyph('中')
            {
                wanted = Some(face.name().to_string());
                break;
            }
        }
        let Some(family) = wanted else {
            eprintln!("skipping: no CJK font");
            return;
        };
        let stack = format!("\"{family}\", serif");
        let engine = BitmapEngine::with_families(OutputFormat::Png, None, Some(&stack), None)
            .expect("engine with fonts");
        assert!(
            engine
                .sans_face_name()
                .to_lowercase()
                .contains(&family.to_lowercase()),
            "expected {family}, got {}",
            engine.sans_face_name()
        );
    }
}
