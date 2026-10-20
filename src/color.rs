//! 8-bit RGBA color type shared by themes and drawing primitives.

use std::fmt;
use std::str::FromStr;

/// An RGBA color with 8 bits per channel.
///
/// Colors are value types: [`Copy`], comparable, and cheap to construct.
/// [`Display`](std::fmt::Display) renders the shortest SVG hex form
/// (`#rrggbb`, or `#rrggbbaa` when the alpha channel is not fully opaque).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Color {
    /// Red channel, `0..=255`.
    pub r: u8,
    /// Green channel, `0..=255`.
    pub g: u8,
    /// Blue channel, `0..=255`.
    pub b: u8,
    /// Alpha channel, `0..=255` (`255` = opaque).
    pub a: u8,
}

impl Color {
    /// Fully opaque black.
    pub const BLACK: Color = Color::rgb(0x00, 0x00, 0x00);
    /// Fully opaque white.
    pub const WHITE: Color = Color::rgb(0xff, 0xff, 0xff);
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color::rgba(0, 0, 0, 0);

    /// Creates an opaque color from 8-bit channels.
    pub const fn rgb(r: u8, g: u8, b: u8) -> Color {
        Color { r, g, b, a: 255 }
    }

    /// Creates a color with alpha from 8-bit channels.
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color { r, g, b, a }
    }

    /// Parses a CSS-style hex color: `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
    ///
    /// The leading `#` is required; hex digits are case-insensitive.
    ///
    /// ```
    /// use markdown2svg::Color;
    /// assert_eq!(Color::from_hex("#fff"), Some(Color::WHITE));
    /// assert_eq!(Color::from_hex("#00000080"), Some(Color::rgba(0, 0, 0, 0x80)));
    /// assert_eq!(Color::from_hex("fff"), None);
    /// ```
    pub fn from_hex(s: &str) -> Option<Color> {
        let hex = s.strip_prefix('#')?;
        let (r, g, b, a) = match hex.as_bytes() {
            [r, g, b] => {
                let r = nibble(*r)?;
                let g = nibble(*g)?;
                let b = nibble(*b)?;
                (r * 17, g * 17, b * 17, 255)
            }
            [r, g, b, a] => {
                let r = nibble(*r)?;
                let g = nibble(*g)?;
                let b = nibble(*b)?;
                let a = nibble(*a)?;
                (r * 17, g * 17, b * 17, a * 17)
            }
            [r1, r2, g1, g2, b1, b2] => {
                let r = hex_byte(*r1, *r2)?;
                let g = hex_byte(*g1, *g2)?;
                let b = hex_byte(*b1, *b2)?;
                (r, g, b, 255)
            }
            [r1, r2, g1, g2, b1, b2, a1, a2] => {
                let r = hex_byte(*r1, *r2)?;
                let g = hex_byte(*g1, *g2)?;
                let b = hex_byte(*b1, *b2)?;
                let a = hex_byte(*a1, *a2)?;
                (r, g, b, a)
            }
            _ => return None,
        };
        Some(Color { r, g, b, a })
    }

    /// Returns this color with a different alpha channel.
    pub const fn with_alpha(self, a: u8) -> Color {
        Color { a, ..self }
    }

    /// Linearly interpolates between `self` (`t = 0.0`) and `other` (`t = 1.0`).
    ///
    /// Useful for deriving nested-quote tints or hover shades from a theme.
    pub fn blend(self, other: Color, t: f32) -> Color {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Color {
            r: mix(self.r, other.r),
            g: mix(self.g, other.g),
            b: mix(self.b, other.b),
            a: mix(self.a, other.a),
        }
    }
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn hex_byte(hi: u8, lo: u8) -> Option<u8> {
    Some(nibble(hi)? << 4 | nibble(lo)?)
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.a {
            255 => write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b),
            a => write!(f, "#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, a),
        }
    }
}

impl FromStr for Color {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Color::from_hex(s).ok_or(ParseColorError(s.to_string()))
    }
}

/// Returned when [`Color::from_str`] receives something that is not a hex color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseColorError(pub String);

impl fmt::Display for ParseColorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid hex color: `{}` (expected #rgb or #rrggbb)",
            self.0
        )
    }
}

impl std::error::Error for ParseColorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_digit_forms() {
        assert_eq!(Color::from_hex("#fff"), Some(Color::WHITE));
        assert_eq!(Color::from_hex("#FfF"), Some(Color::WHITE));
        assert_eq!(Color::from_hex("#000"), Some(Color::BLACK));
    }

    #[test]
    fn parses_six_digit_forms() {
        assert_eq!(
            Color::from_hex("#1f2328"),
            Some(Color::rgb(0x1f, 0x23, 0x28))
        );
        assert_eq!(
            Color::from_hex("#abcdef"),
            Some(Color::rgb(0xab, 0xcd, 0xef))
        );
    }

    #[test]
    fn parses_alpha_forms() {
        let c = Color::from_hex("#00000080").unwrap();
        assert_eq!(c, Color::rgba(0, 0, 0, 0x80));
        let c = Color::from_hex("#1234").unwrap();
        assert_eq!(c, Color::rgba(0x11, 0x22, 0x33, 0x44));
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(Color::from_hex(""), None);
        assert_eq!(Color::from_hex("fff"), None);
        assert_eq!(Color::from_hex("#ff"), None);
        assert_eq!(Color::from_hex("#fffff"), None);
        assert_eq!(Color::from_hex("#gggggg"), None);
        assert_eq!(Color::from_hex("#fffffffff"), None);
    }

    #[test]
    fn display_uses_shortest_form() {
        assert_eq!(Color::WHITE.to_string(), "#ffffff");
        assert_eq!(Color::rgba(0, 0, 0, 128).to_string(), "#00000080");
        assert_eq!(Color::TRANSPARENT.to_string(), "#00000000");
    }

    #[test]
    fn blend_interpolates() {
        let black = Color::BLACK;
        let white = Color::WHITE;
        assert_eq!(black.blend(white, 0.0), black);
        assert_eq!(black.blend(white, 1.0), white);
        let mid = black.blend(white, 0.5);
        assert_eq!(mid, Color::rgb(128, 128, 128));
    }

    #[test]
    fn blend_clamps_t() {
        let c = Color::rgb(0, 0, 0).blend(Color::rgb(100, 0, 0), 2.0);
        assert_eq!(c, Color::rgb(100, 0, 0));
    }

    #[test]
    fn from_str_roundtrips_display() {
        let c: Color = "#ff8800".parse().unwrap();
        assert_eq!(c.to_string(), "#ff8800");
        assert!("#nope".parse::<Color>().is_err());
    }

    #[test]
    fn with_alpha_keeps_rgb() {
        let c = Color::rgb(1, 2, 3).with_alpha(0x40);
        assert_eq!(c, Color::rgba(1, 2, 3, 0x40));
    }

    #[test]
    fn blend_interpolates_alpha_too() {
        let c = Color::rgba(0, 0, 0, 0).blend(Color::rgba(255, 255, 255, 255), 0.5);
        assert_eq!(c, Color::rgba(128, 128, 128, 128));
    }

    #[test]
    fn parse_error_explains_the_expected_format() {
        let err: ParseColorError = "blue".parse::<Color>().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("blue"), "{msg}");
        assert!(msg.contains("#rgb"), "{msg}");
        // It is a real std error, so it composes with `?` and `Box<dyn Error>`.
        let boxed: Box<dyn std::error::Error> = Box::new(err);
        assert!(!boxed.to_string().is_empty());
    }

    #[test]
    fn defaults_to_transparent_black() {
        assert_eq!(Color::default(), Color::TRANSPARENT);
    }
}
