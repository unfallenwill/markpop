//! Render Markdown directly to SVG — no HTML, no headless browser.
//!
//! The pipeline is a straight line of pure stages:
//!
//! ```text
//! Markdown text
//!   → parse      (src/parse)   → Document  (src/ast.rs)
//!   → layout     (src/layout)  → Scene     (src/scene.rs)
//!   → serialize  (src/svg)     → SVG string
//! ```
//!
//! Every stage is independently testable: parsing is asserted on the AST,
//! layout on coordinates and primitives, serialization on markup. Nothing
//! shells out, nothing fetches the network, and the only "font" is a
//! deterministic advance table — so the same Markdown always produces the
//! same bytes.
//!
//! # Example
//!
//! ```
//! let svg = markdown2svg::render("# Hello\n\nWorld with **bold** text.");
//! assert!(svg.starts_with("<svg"));
//! assert!(svg.contains("Hello"));
//! ```
//!
//! With zoom and a theme:
//!
//! ```
//! use markdown2svg::{Options, Theme};
//!
//! let opts = Options::default()
//!     .with_theme(Theme::dark())
//!     .with_zoom(1.5); // 150%
//! let svg = markdown2svg::render_with("# Hi", &opts).unwrap();
//! assert!(svg.contains(r#"width="1200""#));
//! ```
//!
//! Long documents can be paginated and given a table of contents:
//!
//! ```
//! use markdown2svg::Options;
//!
//! let opts = Options::default()
//!     .with_page_height(1123.0) // roughly A4 at 96 dpi
//!     .with_toc(true);
//! let pages = markdown2svg::render_pages("# One\n\nBody", &opts).unwrap();
//! assert_eq!(pages.len(), 1);
//! ```
//!
//! # Extending
//!
//! * **Looks** — [`Theme`] is plain data; start from [`Theme::light`],
//!   [`Theme::dark`] or [`Theme::print`] and override fields.
//! * **Syntax** — add a variant to [`ast::Block`] or [`ast::Inline`], handle
//!   it in `parse`, then handle it in `layout`.
//! * **Output formats** — [`scene::Scene`] is geometry only; write another
//!   serializer next to [`svg`].
//! * **Font metrics** — [`text::Metrics`] is the single place that decides
//!   how wide a character is.
//! * **Pagination** — `paginate` cuts only between top-level blocks; any new
//!   block type automatically participates.

pub mod ast;
pub mod color;
pub mod options;
pub mod outline;
pub mod parse;
pub mod scene;
pub mod svg;
pub mod text;
pub mod theme;

mod layout;
mod paginate;

pub use ast::Document;
pub use color::Color;
pub use options::{Options, RenderError};
pub use outline::Heading;
pub use parse::parse;
pub use scene::{Draw, FontKind, Scene};
pub use theme::Theme;

/// Renders Markdown to SVG with the default options (light theme, 100%).
///
/// ```
/// let svg = markdown2svg::render("# Title");
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
/// This always produces a single image, even when
/// [`Options::page_height`] is set. Use [`render_pages`] for paginated
/// output.
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
pub fn render_pages(markdown: &str, options: &Options) -> Result<Vec<String>, RenderError> {
    options.validate()?;
    Ok(to_pages_inner(markdown, options)
        .iter()
        .map(|scene| svg::render(scene, &options.theme, options.zoom))
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
    let mut doc = prepare(markdown, options);
    if let Some(page_height) = options.page_height {
        doc = Document {
            blocks: paginate::split_tall_code(doc.blocks, &options.theme, page_height),
        };
    }
    let (scene, marks) = layout::Layout::new(&options.theme).render_marked(&doc);
    match options.page_height {
        Some(page_height) => paginate::paginate(scene, &marks, &options.theme, page_height),
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
    layout::Layout::new(theme).render(&parse(markdown))
}

fn to_scene_with_options(markdown: &str, options: &Options) -> Scene {
    layout::Layout::new(&options.theme).render(&prepare(markdown, options))
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
}
