//! Built-in canvas presets.
//!
//! A preset is a coherent bundle of page geometry: width, padding, and (for
//! paper sizes) a page height that turns on automatic pagination. It fills
//! [`Theme::width`] / [`Theme::padding`] and [`Options::page_height`], so
//! explicit flags can still override individual values afterwards.
//!
//! All dimensions are in base units (CSS pixels at 96 dpi), which is what
//! SVG and the rasterizer use.

use crate::options::Options;
use crate::theme::Theme;

/// A ready-made canvas size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanvasPreset {
    /// Default web width, continuous height (`800px` wide).
    Web,
    /// A4 portrait at 96 dpi (`794 × 1123`), paginated automatically.
    A4,
    /// A4 portrait at 144 dpi (`1191 × 1684`), paginated automatically.
    A4Large,
    /// Long-image width for phones (`750px`), continuous height.
    Mobile,
}

/// A4 is 210 × 297 mm. At 96 dpi that is 793.7 × 1122.5 px.
pub const A4_WIDTH: f32 = 794.0;
/// A4 height at 96 dpi.
pub const A4_HEIGHT: f32 = 1123.0;
/// A4 at 144 dpi (1.5×) for crisper raster output.
pub const A4_LARGE_WIDTH: f32 = 1191.0;
/// A4 height at 144 dpi.
pub const A4_LARGE_HEIGHT: f32 = 1684.0;
/// Long-image width used by mobile reading apps (750 px design width).
pub const MOBILE_WIDTH: f32 = 750.0;

impl CanvasPreset {
    /// Every preset, in help-text order.
    pub const ALL: [CanvasPreset; 4] = [
        CanvasPreset::Web,
        CanvasPreset::A4,
        CanvasPreset::A4Large,
        CanvasPreset::Mobile,
    ];

    /// Parses a preset name (case-insensitive).
    ///
    /// `web`/`default`, `a4`, `a4-large`/`a4l`, `mobile`/`phone`.
    pub fn by_name(name: &str) -> Option<CanvasPreset> {
        match name.trim().to_ascii_lowercase().as_str() {
            "web" | "default" => Some(CanvasPreset::Web),
            "a4" | "paper" => Some(CanvasPreset::A4),
            "a4-large" | "a4l" | "a4-144" => Some(CanvasPreset::A4Large),
            "mobile" | "phone" | "long" => Some(CanvasPreset::Mobile),
            _ => None,
        }
    }

    /// Canonical name, as accepted by [`CanvasPreset::by_name`].
    pub fn name(self) -> &'static str {
        match self {
            CanvasPreset::Web => "web",
            CanvasPreset::A4 => "a4",
            CanvasPreset::A4Large => "a4-large",
            CanvasPreset::Mobile => "mobile",
        }
    }

    /// Canvas width in base units.
    pub fn width(self) -> f32 {
        match self {
            CanvasPreset::Web => 800.0,
            CanvasPreset::A4 => A4_WIDTH,
            CanvasPreset::A4Large => A4_LARGE_WIDTH,
            CanvasPreset::Mobile => MOBILE_WIDTH,
        }
    }

    /// Page height in base units, or `None` for a continuous long image.
    pub fn height(self) -> Option<f32> {
        match self {
            CanvasPreset::Web | CanvasPreset::Mobile => None,
            CanvasPreset::A4 => Some(A4_HEIGHT),
            CanvasPreset::A4Large => Some(A4_LARGE_HEIGHT),
        }
    }

    /// Page margin (padding) in base units.
    ///
    /// Paper presets use a printable margin; the mobile preset uses a
    /// smaller one to maximize the text column on a phone.
    pub fn padding(self) -> f32 {
        match self {
            CanvasPreset::Web => 48.0,
            CanvasPreset::A4 => 56.0,
            CanvasPreset::A4Large => 84.0,
            CanvasPreset::Mobile => 32.0,
        }
    }

    /// Base font size that suits the canvas, if the preset overrides it.
    ///
    /// A 750 px long image is viewed at roughly half its pixel size on a
    /// phone, so it needs a larger base size than a desktop-width canvas.
    pub fn font_size(self) -> Option<f32> {
        match self {
            CanvasPreset::Web | CanvasPreset::A4 | CanvasPreset::A4Large => None,
            CanvasPreset::Mobile => Some(24.0),
        }
    }

    /// Applies the preset to a theme and to the pagination setting.
    ///
    /// Any value the preset does not define is left untouched, and callers
    /// can override individual fields afterwards.
    pub fn apply(self, theme: &mut Theme, page_height: &mut Option<f32>) {
        theme.width = self.width();
        theme.padding = self.padding();
        if let Some(size) = self.font_size() {
            theme.font_size = size;
        }
        *page_height = self.height();
    }
}

impl Options {
    /// Applies a canvas preset (builder style).
    ///
    /// ```
    /// use markpop::{CanvasPreset, Options};
    ///
    /// let opts = Options::default().with_canvas(CanvasPreset::A4);
    /// assert_eq!(opts.theme.width, 794.0);
    /// assert_eq!(opts.page_height, Some(1123.0));
    /// ```
    pub fn with_canvas(mut self, preset: CanvasPreset) -> Options {
        preset.apply(&mut self.theme, &mut self.page_height);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for preset in CanvasPreset::ALL {
            assert_eq!(CanvasPreset::by_name(preset.name()), Some(preset));
        }
        assert_eq!(CanvasPreset::by_name("A4"), Some(CanvasPreset::A4));
        assert_eq!(CanvasPreset::by_name("phone"), Some(CanvasPreset::Mobile));
        assert_eq!(CanvasPreset::by_name("letter"), None);
    }

    #[test]
    fn a4_is_210_by_297_millimetres_at_96_dpi() {
        // 1 mm = 96 / 25.4 px = 3.7795 px.
        let mm = 96.0 / 25.4;
        assert!((CanvasPreset::A4.width() - 210.0 * mm).abs() < 1.0);
        assert!((CanvasPreset::A4.height().unwrap() - 297.0 * mm).abs() < 1.0);
    }

    #[test]
    fn a4_turns_on_pagination() {
        let mut theme = Theme::light();
        let mut page_height = None;
        CanvasPreset::A4.apply(&mut theme, &mut page_height);
        assert_eq!(theme.width, A4_WIDTH);
        assert_eq!(page_height, Some(A4_HEIGHT));
        // Content must fit between the margins.
        assert!(theme.padding * 2.0 < theme.width);
        assert!(theme.padding * 2.0 < page_height.unwrap());
    }

    #[test]
    fn mobile_is_a_long_image_with_a_phone_width() {
        let mut theme = Theme::light();
        let mut page_height = Some(123.0);
        CanvasPreset::Mobile.apply(&mut theme, &mut page_height);
        assert_eq!(theme.width, MOBILE_WIDTH);
        assert_eq!(page_height, None, "long images do not paginate");
        assert_eq!(theme.font_size, 24.0);
    }

    #[test]
    fn web_keeps_the_default_geometry() {
        let mut theme = Theme::light();
        let mut page_height = Some(1.0);
        CanvasPreset::Web.apply(&mut theme, &mut page_height);
        assert_eq!(theme.width, 800.0);
        assert_eq!(theme.padding, 48.0);
        assert_eq!(page_height, None);
        assert_eq!(theme.font_size, 16.0);
    }

    #[test]
    fn options_builder_applies_the_preset() {
        let opts = Options::default().with_canvas(CanvasPreset::A4);
        assert_eq!(opts.theme.width, A4_WIDTH);
        assert_eq!(opts.page_height, Some(A4_HEIGHT));
        assert!(opts.validate().is_ok());

        let opts = Options::default().with_canvas(CanvasPreset::Mobile);
        assert_eq!(opts.theme.width, MOBILE_WIDTH);
        assert_eq!(opts.page_height, None);
        assert!(opts.validate().is_ok());
    }

    #[test]
    fn large_a4_keeps_the_same_aspect_ratio() {
        let ratio = A4_LARGE_WIDTH / A4_LARGE_HEIGHT;
        let small = A4_WIDTH / A4_HEIGHT;
        assert!((ratio - small).abs() < 0.001);
    }
}
