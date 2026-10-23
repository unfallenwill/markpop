//! MarkPop renders Markdown directly to SVG, PNG or lossless WebP — no
//! HTML, no headless browser.
//!
//! The pipeline is a straight line of pure stages followed by a pluggable
//! output engine:
//!
//! ```text
//! Markdown text
//!   → parse      (src/parse)   → Document  (src/ast.rs)
//!   → layout     (src/layout)  → Scene     (src/scene.rs)
//!   → engine     (src/engine)  → SVG | PNG | WebP bytes
//! ```
//!
//! SVG is serialized directly. Bitmap formats go through a software
//! rasterizer (`src/raster.rs`) fed by our own TrueType parser
//! (`src/font.rs`), and are compressed by our own PNG and VP8L encoders —
//! nothing shells out, nothing fetches the network, and the layout is always
//! computed by this crate rather than by a browser.
//!
//! Every stage is independently testable: parsing is asserted on the AST,
//! layout on coordinates and primitives, serialization on markup, and the
//! encoders round-trip their pixel buffers.
//!
//! # Example
//!
//! ```
//! let svg = markpop::render("# Hello\n\nWorld with **bold** text.");
//! assert!(svg.starts_with("<svg"));
//! assert!(svg.contains("Hello"));
//! ```
//!
//! With zoom and a theme:
//!
//! ```
//! use markpop::{Options, Theme};
//!
//! let opts = Options::default()
//!     .with_theme(Theme::dark())
//!     .with_zoom(1.5); // 150%
//! let svg = markpop::render_with("# Hi", &opts).unwrap();
//! assert!(svg.contains(r#"width="1200""#));
//! ```
//!
//! Long documents can be paginated and given a table of contents:
//!
//! ```
//! use markpop::Options;
//!
//! let opts = Options::default()
//!     .with_page_height(1123.0) // roughly A4 at 96 dpi
//!     .with_toc(true);
//! let pages = markpop::render_pages("# One\n\nBody", &opts).unwrap();
//! assert_eq!(pages.len(), 1);
//! ```
//!
//! Bitmap output selects a different engine on the same scene:
//!
//! ```no_run
//! use markpop::{Options, OutputFormat};
//!
//! let opts = Options::default().with_format(OutputFormat::Webp);
//! let bytes = markpop::render_bytes("# Hi", &opts).unwrap();
//! assert_eq!(&bytes[..4], b"RIFF");
//! ```
//!
//! Typography is configuration. [`Theme`] carries font size, line height,
//! tracking, font stacks and multi-column flow, each with a builder, and
//! [`CanvasPreset`] bundles a ready-made page geometry:
//!
//! ```
//! use markpop::{CanvasPreset, Options, Theme};
//!
//! let theme = Theme::light()
//!     .with_font_size(17.0)
//!     .with_line_height(1.75)
//!     .with_letter_spacing(0.01)
//!     .with_font_family("Georgia, serif")
//!     .with_columns(2)
//!     .with_column_gap(28.0);
//! assert_eq!(theme.columns, 2);
//! assert!(theme.column_width() < theme.width / 2.0);
//!
//! // A4 at 96 dpi, paginated automatically.
//! let opts = Options::default().with_canvas(CanvasPreset::A4);
//! assert_eq!(opts.page_height, Some(1123.0));
//! ```
//!
//! # Extending
//!
//! * **Looks** — [`Theme`] is plain data; start from [`Theme::light`],
//!   [`Theme::dark`] or [`Theme::print`] and override fields.
//! * **Syntax** — add a variant to [`ast::Block`] or [`ast::Inline`], handle
//!   it in `parse`, then handle it in `layout`.
//! * **Output formats** — implement [`engine::Engine`]; a new encoder only
//!   needs [`scene::Scene`], which is pure geometry.
//! * **Font metrics** — [`text::Metrics`] is the single place that decides
//!   how wide a character is (used for layout; bitmap glyph placement uses
//!   the font's own advances, like a browser does for SVG).
//! * **Pagination** — `paginate` cuts only between top-level blocks; any new
//!   block type automatically participates.

pub mod ast;
pub mod canvas;
pub mod color;
pub mod encode;
pub mod engine;
pub mod font;
pub mod options;
pub mod outline;
pub mod parse;
pub mod raster;
pub mod scene;
pub mod svg;
pub mod text;
pub mod theme;

mod columns;
mod decode;
mod html;
mod layout;
mod paginate;
mod split;

pub use ast::Document;
pub use canvas::CanvasPreset;
pub use color::Color;
pub use engine::{BitmapEngine, Engine, OutputFormat, SvgEngine};
pub use options::{Options, RenderError};
pub use outline::Heading;
pub use parse::parse;
pub use scene::{Draw, FontKind, Scene};
pub use theme::Theme;

/// Renders Markdown to SVG with the default options (light theme, 100%).
///
/// ```
/// let svg = markpop::render("# Title");
/// assert!(svg.contains("viewBox=\"0 0 800"));
/// ```
pub fn render(markdown: &str) -> String {
    render_with(markdown, &Options::default()).expect("default options are always valid")
}

/// Renders Markdown to SVG with explicit [`Options`].
///
/// Returns an error only if the options themselves are unusable (e.g. a
/// non-positive zoom); malformed Markdown never fails.
///
/// This always produces a single SVG image: [`Options::format`] is ignored
/// here (use [`render_bytes`]) and [`Options::page_height`] is ignored too
/// (use [`render_pages`]).
pub fn render_with(markdown: &str, options: &Options) -> Result<String, RenderError> {
    options.validate()?;
    let scene = to_scene_with_options(markdown, options);
    Ok(svg::render(&scene, &options.theme, options.zoom))
}

/// Renders Markdown to one SVG per page, honouring [`Options::page_height`].
///
/// Without a page height this returns a single-element vector. With one,
/// cuts happen only between blocks; code blocks taller than a page are
/// split between lines.
///
/// This is the SVG-text variant; [`render_bytes_pages`] honours
/// [`Options::format`] as well.
pub fn render_pages(markdown: &str, options: &Options) -> Result<Vec<String>, RenderError> {
    options.validate()?;
    Ok(to_pages_inner(markdown, options)
        .iter()
        .map(|scene| svg::render(scene, &options.theme, options.zoom))
        .collect())
}

/// Renders Markdown to bytes in the format selected by [`Options::format`].
///
/// SVG is produced by the vector engine; PNG and WebP are produced by the
/// software rasterizer and need a TrueType font (discovered automatically
/// or supplied via [`Options::font_file`]).
pub fn render_bytes(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError> {
    options.validate()?;
    let scene = to_scene_with_options(markdown, options);
    let engine = engine::engine_for_render(
        options.format,
        options.font_file.as_deref(),
        Some(&options.theme),
        &options.variations,
    )?;
    Ok(engine.encode(&scene, &options.theme, options.zoom))
}

/// Renders one image per page, in the format selected by [`Options::format`].
pub fn render_bytes_pages(markdown: &str, options: &Options) -> Result<Vec<Vec<u8>>, RenderError> {
    options.validate()?;
    let engine = engine::engine_for_render(
        options.format,
        options.font_file.as_deref(),
        Some(&options.theme),
        &options.variations,
    )?;
    Ok(to_pages_inner(markdown, options)
        .iter()
        .map(|scene| engine.encode(scene, &options.theme, options.zoom))
        .collect())
}

/// Lays Markdown out into scenes, one per page, without serializing.
///
/// Useful for tests and for alternate serializers.
pub fn to_pages(markdown: &str, options: &Options) -> Result<Vec<Scene>, RenderError> {
    options.validate()?;
    Ok(to_pages_inner(markdown, options))
}

/// Applies document-level transforms (TOC, code chunking) and paginates.
fn to_pages_inner(markdown: &str, options: &Options) -> Vec<Scene> {
    let theme = &options.theme;
    let mut doc = prepare(markdown, options);
    if let Some(page_height) = options.page_height {
        let available = (page_height - 2.0 * theme.padding).max(1.0);
        doc = Document {
            blocks: split::for_height(doc.blocks, theme, available),
        };
    }
    let env = layout::ImageEnv {
        base_dir: options.base_dir.as_ref().map(std::path::PathBuf::from),
        enabled: options.local_images,
        max_height: options
            .page_height
            .map(|h| (h - 2.0 * theme.padding).max(1.0)),
    };
    let (scene, marks) = layout::Layout::with_image_env(theme, env).render_marked(&doc);

    if theme.columns > 1 {
        return columns::layout_columns(scene, &marks, theme, options.page_height);
    }
    match options.page_height {
        Some(page_height) => paginate::paginate(scene, &marks, theme, page_height),
        None => vec![scene],
    }
}

/// Parses a document and applies the options that change the AST.
fn prepare(markdown: &str, options: &Options) -> Document {
    let doc = parse(markdown);
    if options.toc {
        outline::with_toc(&doc, 3, &options.toc_title)
    } else {
        doc
    }
}

/// Lays Markdown out into a [`Scene`] (geometry only), without serializing.
///
/// Useful for tests, alternate renderers, and inspecting positions.
pub fn to_scene(markdown: &str, theme: &Theme) -> Scene {
    let doc = parse(markdown);
    let (scene, marks) = layout::Layout::new(theme).render_marked(&doc);
    if theme.columns > 1 {
        // Balanced columns, since there is no page height to fill.
        columns::layout_columns(scene, &marks, theme, None).remove(0)
    } else {
        scene
    }
}

fn to_scene_with_options(markdown: &str, options: &Options) -> Scene {
    // A single image never paginates, but it does honour columns.
    let mut single = options.clone();
    single.page_height = None;
    to_pages_inner(markdown, &single).remove(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_produces_svg_document() {
        let out = render("hello");
        assert!(out.starts_with("<svg "));
        assert!(out.trim_end().ends_with("</svg>"));
    }

    #[test]
    fn render_with_rejects_bad_options() {
        let opts = Options::default().with_zoom(0.0);
        assert_eq!(render_with("x", &opts), Err(RenderError::InvalidZoom(0.0)));
    }

    #[test]
    fn zoom_scales_the_output_size() {
        let base = render_with("x", &Options::default()).unwrap();
        let zoomed = render_with("x", &Options::default().with_zoom(1.5)).unwrap();
        assert!(base.contains(r#"width="800""#));
        assert!(zoomed.contains(r#"width="1200""#));
        assert!(zoomed.contains(r#"viewBox="0 0 800"#));
    }

    #[test]
    fn dark_theme_changes_colors_not_layout() {
        let light = render_with("x", &Options::default()).unwrap();
        let dark = render_with("x", &Options::default().with_theme(Theme::dark())).unwrap();
        assert!(light.contains("#ffffff"));
        assert!(dark.contains("#0d1117"));
        assert_ne!(light, dark);
    }

    #[test]
    fn malformed_markdown_never_errors() {
        for src in ["***", "[[[[", "```", "|a|", "> > >", "- ", "#", "\\"] {
            let opts = Options::default();
            assert!(render_with(src, &opts).is_ok(), "failed on {src:?}");
        }
    }

    #[test]
    fn scene_exposes_geometry_for_inspection() {
        let scene = to_scene("hello", &Theme::light());
        assert!(scene.find_text("hello").is_some());
        assert_eq!(scene.width, 800.0);
    }

    #[test]
    fn format_is_only_honoured_by_the_byte_api() {
        // `render_with` is the SVG-text API and ignores the format field.
        let opts = Options::default().with_format(OutputFormat::Png);
        assert!(render_with("x", &opts).unwrap().starts_with("<svg"));
    }

    #[test]
    fn svg_bytes_are_utf8() {
        let bytes = render_bytes("x", &Options::default()).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            render_with("x", &Options::default()).unwrap()
        );
    }

    #[test]
    fn pages_api_returns_one_image_per_page() {
        let opts = Options::default()
            .with_format(OutputFormat::Png)
            .with_page_height(400.0);
        match render_bytes_pages(&"paragraph\n\n".repeat(40), &opts) {
            Ok(pages) => assert!(pages.len() > 1),
            Err(RenderError::FontUnavailable) => eprintln!("skipping: no system fonts"),
            Err(e) => panic!("{e}"),
        }
    }

    #[test]
    fn invalid_options_are_rejected_before_encoding() {
        let opts = Options::default()
            .with_format(OutputFormat::Png)
            .with_zoom(-2.0);
        assert_eq!(
            render_bytes("x", &opts),
            Err(RenderError::InvalidZoom(-2.0))
        );
    }
}
