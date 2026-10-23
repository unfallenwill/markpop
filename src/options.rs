//! Render options and error type for the public API.

use crate::engine::OutputFormat;
use crate::theme::Theme;

/// Tunable inputs for a render, beyond the Markdown itself.
///
/// Zoom is expressed as a multiplier where `1.0` means 100%:
///
/// ```
/// use markpop::Options;
///
/// // 150% output
/// let opts = Options::default().with_zoom(1.5);
/// assert_eq!(opts.zoom, 1.5);
/// ```
///
/// Pagination and a table of contents are opt-in:
///
/// ```
/// use markpop::Options;
///
/// let opts = Options::default()
///     .with_page_height(1123.0) // A4 at 96 dpi, roughly
///     .with_toc(true);
/// let pages = markpop::render_pages("# Title\n\nBody", &opts).unwrap();
/// assert!(!pages.is_empty());
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    /// The visual theme (canvas metrics, fonts, colors).
    pub theme: Theme,
    /// Output scale relative to 100% (`1.0` = 100%, `1.5` = 150%).
    ///
    /// Zoom is implemented with the SVG `viewBox`: geometry stays in base
    /// units and only the output `width`/`height` scale, so zooming is
    /// lossless.
    pub zoom: f32,
    /// When set, the document is split into pages of this height (base
    /// units) instead of producing one tall image.
    ///
    /// Cuts only happen between blocks, so paragraphs, tables and code
    /// boxes are never sliced in half. Code blocks taller than a page are
    /// split between lines.
    pub page_height: Option<f32>,
    /// Prepend a table of contents built from the document's headings.
    pub toc: bool,
    /// Heading text used for the table of contents.
    pub toc_title: String,
    /// Output format: SVG (default), PNG or lossless WebP.
    pub format: OutputFormat,
    /// Optional `.ttf`/`.ttc` file used for bitmap output. When `None`, a
    /// font is discovered from the system font directories.
    pub font_file: Option<String>,
    /// Variable-font axis settings as `(tag, value)` pairs, e.g.
    /// `("wght", 700.0)`. Ignored by static faces.
    pub variations: Vec<(String, f32)>,
    /// Directory relative image paths resolve against (the source
    /// file's directory for the CLI). `None` means the working directory.
    pub base_dir: Option<String>,
    /// Whether local images referenced by `![alt](path)` are read from
    /// disk and rendered. `false` restores the all-placeholder behavior —
    /// use it when rendering untrusted documents.
    pub local_images: bool,
}

impl Options {
    /// Returns these options with a different zoom (builder style).
    pub fn with_zoom(mut self, zoom: f32) -> Options {
        self.zoom = zoom;
        self
    }

    /// Returns these options with a different theme (builder style).
    pub fn with_theme(mut self, theme: Theme) -> Options {
        self.theme = theme;
        self
    }

    /// Enables pagination at the given page height in base units
    /// (builder style).
    pub fn with_page_height(mut self, page_height: f32) -> Options {
        self.page_height = Some(page_height);
        self
    }

    /// Enables or disables the table of contents (builder style).
    pub fn with_toc(mut self, toc: bool) -> Options {
        self.toc = toc;
        self
    }

    /// Sets the table-of-contents heading text (builder style).
    pub fn with_toc_title(mut self, title: impl Into<String>) -> Options {
        self.toc_title = title.into();
        self
    }

    /// Selects the output format (builder style).
    pub fn with_format(mut self, format: OutputFormat) -> Options {
        self.format = format;
        self
    }

    /// Uses a specific font file for bitmap output (builder style).
    pub fn with_font_file(mut self, path: impl Into<String>) -> Options {
        self.font_file = Some(path.into());
        self
    }

    /// Selects a variable-font instance (builder style).
    ///
    /// ```
    /// use markpop::Options;
    ///
    /// let opts = Options::default().with_variation("wght", 700.0);
    /// assert_eq!(opts.variations, vec![("wght".to_string(), 700.0)]);
    /// ```
    pub fn with_variation(mut self, tag: impl Into<String>, value: f32) -> Options {
        self.variations.push((tag.into(), value));
        self
    }

    /// Sets the directory relative image paths resolve against
    /// (builder style).
    pub fn with_base_dir(mut self, dir: impl Into<String>) -> Options {
        self.base_dir = Some(dir.into());
        self
    }

    /// Enables or disables local image rendering (builder style).
    pub fn with_local_images(mut self, enabled: bool) -> Options {
        self.local_images = enabled;
        self
    }

    /// Checks that the options describe a renderable picture.
    ///
    /// Zoom must be a finite, positive number; the canvas must be wide
    /// enough to fit its padding plus a minimum text column; page height,
    /// if any, must leave room for at least one line of content.
    pub fn validate(&self) -> Result<(), RenderError> {
        if !self.zoom.is_finite() || self.zoom <= 0.0 {
            return Err(RenderError::InvalidZoom(self.zoom));
        }
        let min_width = self.theme.padding * 2.0 + 64.0;
        if !self.theme.width.is_finite() || self.theme.width < min_width {
            return Err(RenderError::InvalidCanvasWidth(self.theme.width));
        }
        if let Some(h) = self.page_height {
            let min_height = self.theme.padding * 2.0 + 80.0;
            if !h.is_finite() || h < min_height {
                return Err(RenderError::InvalidPageHeight(h));
            }
        }
        let columns = self.theme.columns;
        if columns == 0 || columns > 12 || !self.theme.column_gap.is_finite() {
            return Err(RenderError::InvalidColumns(columns));
        }
        // Every column must still hold a readable amount of text.
        if self.theme.column_width() < 96.0 {
            return Err(RenderError::InvalidColumns(columns));
        }
        Ok(())
    }
}

impl Default for Options {
    fn default() -> Options {
        Options {
            theme: Theme::default(),
            zoom: 1.0,
            page_height: None,
            toc: false,
            toc_title: "Contents".to_string(),
            format: OutputFormat::Svg,
            font_file: None,
            variations: Vec::new(),
            base_dir: None,
            local_images: true,
        }
    }

}

/// Ways a render can be rejected before any work is done.
///
/// Note that Markdown *parsing* is infallible by design: malformed input
/// degrades into paragraphs rather than erroring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RenderError {
    /// Zoom was zero, negative, NaN or infinite.
    InvalidZoom(f32),
    /// Canvas width was NaN/infinite or too narrow for its padding.
    InvalidCanvasWidth(f32),
    /// Page height was NaN/infinite or too small for its padding.
    InvalidPageHeight(f32),
    /// Column count was out of range or left too little width per column.
    InvalidColumns(u32),
    /// Bitmap output was requested but no usable TrueType font was found.
    FontUnavailable,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::InvalidZoom(z) => {
                write!(f, "invalid zoom `{z}`: must be finite and > 0 (1.0 = 100%)")
            }
            RenderError::InvalidCanvasWidth(w) => {
                write!(
                    f,
                    "invalid canvas width `{w}`: too narrow for the theme padding"
                )
            }
            RenderError::InvalidPageHeight(h) => {
                write!(
                    f,
                    "invalid page height `{h}`: too small for the theme padding"
                )
            }
            RenderError::InvalidColumns(n) => write!(
                f,
                "invalid column count `{n}`: must be 1..=12 and leave at least 96px per column"
            ),
            RenderError::FontUnavailable => write!(
                f,
                "no TrueType font available for bitmap output: install a system font or pass one explicitly"
            ),
        }
    }
}

impl std::error::Error for RenderError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_one_hundred_percent() {
        let opts = Options::default();
        assert_eq!(opts.zoom, 1.0);
        assert_eq!(opts.theme, Theme::light());
        assert_eq!(opts.page_height, None);
        assert!(!opts.toc);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn builders_chain() {
        let opts = Options::default()
            .with_zoom(2.0)
            .with_theme(Theme::dark())
            .with_page_height(1123.0)
            .with_toc(true)
            .with_toc_title("目录");
        assert_eq!(opts.zoom, 2.0);
        assert_eq!(opts.theme, Theme::dark());
        assert_eq!(opts.page_height, Some(1123.0));
        assert!(opts.toc);
        assert_eq!(opts.toc_title, "目录");
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn rejects_bad_zoom() {
        for z in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            match Options::default().with_zoom(z).validate() {
                Err(RenderError::InvalidZoom(v)) => {
                    if z.is_nan() {
                        assert!(v.is_nan());
                    } else {
                        assert_eq!(v, z);
                    }
                }
                other => panic!("expected InvalidZoom for {z}, got {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_bad_page_height() {
        for h in [0.0, -10.0, 50.0, f32::NAN, f32::INFINITY] {
            let opts = Options::default().with_page_height(h);
            assert!(
                matches!(opts.validate(), Err(RenderError::InvalidPageHeight(_))),
                "height {h} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_too_narrow_canvas() {
        let mut opts = Options::default();
        opts.theme.width = 100.0; // padding alone is 96
        assert_eq!(opts.validate(), Err(RenderError::InvalidCanvasWidth(100.0)));
    }

    #[test]
    fn error_messages_are_helpful() {
        let msg = RenderError::InvalidZoom(0.0).to_string();
        assert!(msg.contains("zoom"), "message should name the field: {msg}");
        let msg = RenderError::InvalidCanvasWidth(10.0).to_string();
        assert!(
            msg.contains("width"),
            "message should name the field: {msg}"
        );
        let msg = RenderError::InvalidPageHeight(10.0).to_string();
        assert!(
            msg.contains("height"),
            "message should name the field: {msg}"
        );
    }

    #[test]
    fn column_configuration_is_validated() {
        // Reasonable counts pass.
        for n in 1..=4u32 {
            let opts = Options::default().with_theme(Theme::light().with_columns(n));
            assert!(opts.validate().is_ok(), "columns={n} should be valid");
        }
        // Zero and absurd counts fail (the builder clamps, so set the field).
        for n in [0u32, 13, 40] {
            let theme = Theme {
                columns: n,
                ..Theme::light()
            };
            let opts = Options::default().with_theme(theme);
            assert!(
                matches!(opts.validate(), Err(RenderError::InvalidColumns(_))),
                "columns={n} should be rejected"
            );
        }
        // Too many columns for the canvas width fails.
        let narrow = Theme {
            width: 300.0,
            ..Theme::light()
        }
        .with_columns(3);
        let opts = Options::default().with_theme(narrow);
        assert!(matches!(
            opts.validate(),
            Err(RenderError::InvalidColumns(3))
        ));
    }

    #[test]
    fn column_error_message_explains_the_limit() {
        let msg = RenderError::InvalidColumns(99).to_string();
        assert!(msg.contains("column"), "{msg}");
        assert!(msg.contains("96"), "{msg}");
    }
}
