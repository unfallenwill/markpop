//! Themes: every tunable visual constant lives here.
//!
//! A [`Theme`] is plain data — no behavior — so new looks are just new
//! values. Two presets ship built in ([`Theme::light`] and
//! [`Theme::dark`]); override any field for custom styling:
//!
//! ```
//! use markpop::{Options, Theme};
//!
//! let branding = Theme {
//!     link: "#ff5500".parse().unwrap(),
//!     ..Theme::light()
//! };
//! let opts = Options::default().with_theme(branding);
//! ```

use crate::color::Color;

/// Visual constants for one look: canvas, typography and colors.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    // --- canvas ---
    /// Design canvas width in base units (pixels at 100% zoom).
    pub width: f32,
    /// Canvas padding on all sides.
    pub padding: f32,

    // --- typography ---
    /// Base body font size; all other sizes derive from it.
    pub font_size: f32,
    /// Body line height as a multiple of the font size.
    pub line_height: f32,
    /// Extra tracking between characters, in em units (`0.0` = none).
    pub letter_spacing: f32,
    /// Number of text columns (`1` = a single full-width column).
    ///
    /// With more than one column, content flows down a column and continues
    /// at the top of the next one. Without a page height the columns are
    /// balanced (CSS `column-count` style); with one, columns fill the page
    /// and new pages are started as needed.
    pub columns: u32,
    /// Horizontal gap between text columns, in base units.
    pub column_gap: f32,
    /// Font size of code text as a multiple of the base size.
    pub code_scale: f32,
    /// Line height inside code blocks as a multiple of the code font size.
    pub code_line_height: f32,
    /// Proportional font family stack (SVG `font-family` value).
    pub sans_family: String,
    /// Monospace font family stack (SVG `font-family` value).
    pub mono_family: String,
    /// Heading font-size multipliers for h1..=h6.
    pub heading_scales: [f32; 6],
    /// Heading font-size multiplier for h1..=h6 at small screens.
    /// Draw a rule under headings with level <= this value (GitHub style).
    pub heading_rule_below: u8,

    // --- colors ---
    /// Body text color.
    pub text: Color,
    /// Heading text color.
    pub heading: Color,
    /// Link text color.
    pub link: Color,
    /// Muted text (e.g. code language labels, image captions).
    pub muted: Color,
    /// Page background, or `None` for a transparent page.
    pub background: Option<Color>,
    /// Code block / inline-code background.
    pub code_bg: Color,
    /// Code block / inline-code text color.
    pub code_text: Color,
    /// Blockquote accent bar color.
    pub quote_bar: Color,
    /// Blockquote background color.
    pub quote_bg: Color,
    /// Hairline borders: heading rules, table row lines, thematic breaks.
    pub border: Color,
    /// Table header row background.
    pub table_header_bg: Color,
    /// List bullet color.
    pub bullet: Color,
}

impl Theme {
    /// A light, GitHub-inspired default look (green headings, blue links).
    pub fn light() -> Theme {
        Theme {
            width: 800.0,
            padding: 48.0,
            font_size: 16.0,
            line_height: 1.6,
            letter_spacing: 0.0,
            columns: 1,
            column_gap: 24.0,
            code_scale: 0.85,
            code_line_height: 1.5,
            sans_family: "Segoe UI, -apple-system, Helvetica, Arial, PingFang SC, \
                          Hiragino Sans GB, Microsoft YaHei, sans-serif"
                .into(),
            mono_family: "Cascadia Code, Consolas, Menlo, Monaco, 'Courier New', monospace".into(),
            heading_scales: [2.0, 1.5, 1.25, 1.0, 0.875, 0.85],
            heading_rule_below: 2,
            text: Color::rgb(0x1f, 0x23, 0x28),
            heading: Color::rgb(0x05, 0x50, 0x3d),
            link: Color::rgb(0x09, 0x69, 0xda),
            muted: Color::rgb(0x5b, 0x63, 0x6c),
            background: Some(Color::rgb(0xff, 0xff, 0xff)),
            code_bg: Color::rgb(0xf1, 0xf3, 0xf6),
            code_text: Color::rgb(0x24, 0x29, 0x2e),
            quote_bar: Color::rgb(0xd1, 0xd9, 0xe0),
            quote_bg: Color::rgb(0xf8, 0xf9, 0xfb),
            border: Color::rgb(0xd8, 0xde, 0xe4),
            table_header_bg: Color::rgb(0xf2, 0xf4, 0xf7),
            bullet: Color::rgb(0x33, 0x38, 0x3f),
        }
    }

    /// A dark look with the same metrics as [`Theme::light`].
    pub fn dark() -> Theme {
        Theme {
            text: Color::rgb(0xe6, 0xed, 0xf3),
            heading: Color::rgb(0xea, 0xf3, 0xf0),
            link: Color::rgb(0x58, 0xa6, 0xff),
            muted: Color::rgb(0x8b, 0x94, 0x9e),
            background: Some(Color::rgb(0x0d, 0x11, 0x17)),
            code_bg: Color::rgb(0x16, 0x1b, 0x22),
            code_text: Color::rgb(0xdb, 0xdf, 0xe4),
            quote_bar: Color::rgb(0x3d, 0x44, 0x4d),
            quote_bg: Color::rgb(0x13, 0x17, 0x1d),
            border: Color::rgb(0x2d, 0x33, 0x3b),
            table_header_bg: Color::rgb(0x16, 0x1b, 0x22),
            bullet: Color::rgb(0xb6, 0xbd, 0xc5),
            ..Theme::light()
        }
    }

    /// A print-inspired look: serif body text on warm paper, no heading
    /// rules below h1, slightly looser line height and a touch of tracking.
    pub fn print() -> Theme {
        Theme {
            font_size: 17.0,
            line_height: 1.75,
            letter_spacing: 0.005,
            sans_family: "Georgia, 'Iowan Old Style', 'Times New Roman', \
                          'Songti SC', SimSun, serif"
                .into(),
            mono_family: "'SFMono-Regular', Consolas, 'Liberation Mono', monospace".into(),
            heading_scales: [1.9, 1.45, 1.2, 1.0, 0.9, 0.85],
            heading_rule_below: 1,
            text: Color::rgb(0x1a, 0x1a, 0x1a),
            heading: Color::BLACK,
            link: Color::rgb(0x1a, 0x4b, 0x7a),
            muted: Color::rgb(0x6b, 0x63, 0x57),
            background: Some(Color::rgb(0xff, 0xfd, 0xf7)),
            code_bg: Color::rgb(0xf3, 0xef, 0xe5),
            code_text: Color::rgb(0x2b, 0x28, 0x24),
            quote_bar: Color::rgb(0xc9, 0xbe, 0xa8),
            quote_bg: Color::rgb(0xfa, 0xf7, 0xef),
            border: Color::rgb(0xd6, 0xcc, 0xb8),
            table_header_bg: Color::rgb(0xf3, 0xef, 0xe5),
            bullet: Color::rgb(0x2b, 0x28, 0x24),
            ..Theme::light()
        }
    }

    /// A named preset: `"light"`, `"dark"` or `"print"` (case-insensitive).
    pub fn by_name(name: &str) -> Option<Theme> {
        match name.to_ascii_lowercase().as_str() {
            "light" | "default" => Some(Theme::light()),
            "dark" => Some(Theme::dark()),
            "print" | "paper" | "serif" => Some(Theme::print()),
            _ => None,
        }
    }

    /// The code font size in base units.
    pub fn code_size(&self) -> f32 {
        self.font_size * self.code_scale
    }

    /// The heading font size for a level (1-indexed), in base units.
    ///
    /// Levels outside `1..=6` are clamped.
    pub fn heading_size(&self, level: u8) -> f32 {
        let idx = level.saturating_sub(1).min(5) as usize;
        self.font_size * self.heading_scales[idx]
    }

    /// Width of one text column, in base units.
    ///
    /// With a single column this is the full content width; with `n`
    /// columns it subtracts the gaps and divides what is left.
    pub fn column_width(&self) -> f32 {
        let total = (self.width - 2.0 * self.padding).max(1.0);
        let n = self.columns.max(1) as f32;
        if n <= 1.0 {
            total
        } else {
            ((total - (n - 1.0) * self.column_gap) / n).max(1.0)
        }
    }

    /// Sets the base font size, in base units (builder style).
    pub fn with_font_size(mut self, size: f32) -> Theme {
        self.font_size = size;
        self
    }

    /// Sets the line-height multiplier (builder style).
    pub fn with_line_height(mut self, mult: f32) -> Theme {
        self.line_height = mult;
        self
    }

    /// Sets the tracking between characters, in em units (builder style).
    pub fn with_letter_spacing(mut self, em: f32) -> Theme {
        self.letter_spacing = em;
        self
    }

    /// Sets the proportional font stack, e.g. `"Georgia, serif"`
    /// (builder style).
    pub fn with_font_family(mut self, stack: impl Into<String>) -> Theme {
        self.sans_family = stack.into();
        self
    }

    /// Sets the monospace font stack (builder style).
    pub fn with_mono_family(mut self, stack: impl Into<String>) -> Theme {
        self.mono_family = stack.into();
        self
    }

    /// Sets the number of text columns; `0` is treated as `1` (builder style).
    pub fn with_columns(mut self, columns: u32) -> Theme {
        self.columns = columns.max(1);
        self
    }

    /// Sets the gap between columns, in base units (builder style).
    pub fn with_column_gap(mut self, gap: f32) -> Theme {
        self.column_gap = gap.max(0.0);
        self
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::light()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_is_default() {
        assert_eq!(Theme::default(), Theme::light());
    }

    #[test]
    fn dark_overrides_colors_but_keeps_metrics() {
        let (light, dark) = (Theme::light(), Theme::dark());
        assert_eq!(light.width, dark.width);
        assert_eq!(light.padding, dark.padding);
        assert_eq!(light.font_size, dark.font_size);
        assert_ne!(light.text, dark.text);
        assert_ne!(light.background, dark.background);
    }

    #[test]
    fn preset_lookup() {
        assert_eq!(Theme::by_name("light"), Some(Theme::light()));
        assert_eq!(Theme::by_name("DARK"), Some(Theme::dark()));
        assert_eq!(Theme::by_name("print"), Some(Theme::print()));
        assert_eq!(Theme::by_name("serif"), Some(Theme::print()));
        assert_eq!(Theme::by_name("solarized"), None);
    }

    #[test]
    fn print_preset_is_a_distinct_design() {
        let (light, print) = (Theme::light(), Theme::print());
        assert!(
            print.sans_family.contains("serif"),
            "print body should be serif"
        );
        assert!(print.font_size > light.font_size);
        assert!(print.line_height > light.line_height);
        assert!(print.letter_spacing > 0.0);
        assert_ne!(print.background, light.background);
        assert_ne!(print.heading_rule_below, light.heading_rule_below);
    }

    #[test]
    fn heading_sizes_clamp() {
        let t = Theme::light();
        assert_eq!(t.heading_size(0), t.font_size * 2.0);
        assert_eq!(t.heading_size(1), t.font_size * 2.0);
        assert_eq!(t.heading_size(7), t.font_size * 0.85);
    }

    #[test]
    fn code_size_derives_from_base() {
        let t = Theme::light();
        assert!((t.code_size() - 16.0 * 0.85).abs() < 1e-6);
    }

    #[test]
    fn typography_builders_are_chainable() {
        let theme = Theme::light()
            .with_font_size(18.0)
            .with_line_height(1.9)
            .with_letter_spacing(0.02)
            .with_font_family("Georgia, serif")
            .with_mono_family("Courier New, monospace")
            .with_columns(2)
            .with_column_gap(30.0);
        assert_eq!(theme.font_size, 18.0);
        assert_eq!(theme.line_height, 1.9);
        assert!((theme.letter_spacing - 0.02).abs() < 1e-6);
        assert_eq!(theme.sans_family, "Georgia, serif");
        assert_eq!(theme.mono_family, "Courier New, monospace");
        assert_eq!(theme.columns, 2);
        assert_eq!(theme.column_gap, 30.0);
    }

    #[test]
    fn columns_are_clamped_to_at_least_one() {
        assert_eq!(Theme::light().with_columns(0).columns, 1);
        assert_eq!(Theme::light().with_columns(3).columns, 3);
        assert_eq!(Theme::light().with_column_gap(-5.0).column_gap, 0.0);
    }

    #[test]
    fn column_width_matches_the_content_width_in_one_column() {
        let theme = Theme::light();
        assert_eq!(theme.column_width(), theme.width - 2.0 * theme.padding);
    }

    #[test]
    fn column_width_subtracts_gaps() {
        let theme = Theme::light().with_columns(3).with_column_gap(20.0);
        let expected = (800.0 - 96.0 - 2.0 * 20.0) / 3.0;
        assert!((theme.column_width() - expected).abs() < 1e-4);
    }
}
