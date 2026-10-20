//! Deterministic, font-file-free text measurement and line breaking.
//!
//! Real glyph advances require a font. Embedding one would make the crate
//! huge and rendering non-deterministic, so instead this module uses a
//! conservative advance table: each character maps to a fraction of the em
//! box. That is accurate enough for proportional Latin text, exact for
//! monospace ASCII, and correct in the one place it matters most — CJK,
//! where every glyph is a full em square.
//!
//! Everything here is pure: same input, same output, on every platform.

use crate::scene::FontKind;

/// The style attributes that affect text metrics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Font size in base units.
    pub size: f32,
    /// Font family slot.
    pub family: FontKind,
    /// Bold weight.
    pub bold: bool,
    /// Italic slant.
    pub italic: bool,
    /// Extra advance between characters, in em units (`0.0` = none).
    pub letter_spacing: f32,
}

impl TextStyle {
    /// Creates a style for the given size and family.
    pub fn new(size: f32, family: FontKind) -> TextStyle {
        TextStyle {
            size,
            family,
            bold: false,
            italic: false,
            letter_spacing: 0.0,
        }
    }

    /// Returns this style with bold enabled.
    pub fn bold(mut self) -> TextStyle {
        self.bold = true;
        self
    }

    /// Returns this style with italics enabled.
    pub fn italic(mut self) -> TextStyle {
        self.italic = true;
        self
    }

    /// Returns this style with tracking (letter spacing) in em units.
    pub fn with_letter_spacing(mut self, em: f32) -> TextStyle {
        self.letter_spacing = em;
        self
    }
}

/// Text measuring and wrapping. Stateless; `Copy` so it can be passed by
/// value into layout helpers.
#[derive(Debug, Clone, Copy, Default)]
pub struct Metrics;

impl Metrics {
    /// Creates a metrics instance.
    pub fn new() -> Metrics {
        Metrics
    }

    /// Advance width of a single character, in base units.
    ///
    /// Monospace families use a fixed 0.6 em advance for every non-CJK
    /// character, which is what typical coding fonts measure. Letter
    /// spacing is added to every character, including spaces, matching how
    /// SVG and CSS apply `letter-spacing`.
    pub fn advance(&self, c: char, style: &TextStyle) -> f32 {
        if style.size <= 0.0 {
            return 0.0;
        }
        let em = match style.family {
            FontKind::Mono => {
                if is_wide(c) {
                    1.0
                } else if c == '\t' {
                    2.4
                } else {
                    0.6
                }
            }
            FontKind::Sans => sans_em(c),
        };
        // Bold glyphs are marginally wider in most sans designs; italics
        // are not. Keeping this small avoids surprising line-break changes
        // between `**bold**` and plain text.
        let weight = if style.bold && style.family == FontKind::Sans {
            1.02
        } else {
            1.0
        };
        em * style.size * weight + style.letter_spacing * style.size
    }

    /// Total advance width of a string, in base units.
    pub fn measure(&self, s: &str, style: &TextStyle) -> f32 {
        s.chars()
            .map(|c| self.advance(c, style))
            .fold(0.0, |a, b| a + b)
    }

    /// Greedy line breaking to `max_width`.
    ///
    /// Break opportunities are spaces (which are consumed) and between
    /// wide (CJK) characters. Words longer than `max_width` are broken
    /// character by character rather than overflowing. Trailing spaces are
    /// removed from the returned lines.
    ///
    /// ```
    /// use markdown2svg::text::{Metrics, TextStyle};
    /// use markdown2svg::scene::FontKind;
    ///
    /// let style = TextStyle::new(10.0, FontKind::Mono);
    /// let m = Metrics::new();
    /// // 0.6em * 10px = 6px per char: "aa bb" is 30px, too wide for 25px.
    /// assert_eq!(m.wrap("aa bb", 25.0, &style), vec!["aa", "bb"]);
    /// ```
    pub fn wrap(&self, s: &str, max_width: f32, style: &TextStyle) -> Vec<String> {
        self.wrap_impl(s, max_width, style, false)
    }

    /// Wraps **preformatted** text, keeping leading indentation.
    ///
    /// Code blocks are content, not prose: their leading spaces align
    /// columns and must survive wrapping. The only whitespace dropped is a
    /// single space at a line-break point.
    ///
    /// ```
    /// use markdown2svg::text::{Metrics, TextStyle};
    /// use markdown2svg::scene::FontKind;
    ///
    /// let style = TextStyle::new(10.0, FontKind::Mono);
    /// let m = Metrics::new();
    /// assert_eq!(m.wrap_preformatted("  x", 100.0, &style), vec!["  x"]);
    /// assert_eq!(m.wrap("  x", 100.0, &style), vec!["x"]);
    /// ```
    pub fn wrap_preformatted(&self, s: &str, max_width: f32, style: &TextStyle) -> Vec<String> {
        self.wrap_impl(s, max_width, style, true)
    }

    fn wrap_impl(
        &self,
        s: &str,
        max_width: f32,
        style: &TextStyle,
        preformatted: bool,
    ) -> Vec<String> {
        let atoms = atomize(s);
        let mut lines: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w = 0.0f32;

        for atom in atoms {
            let w = self.measure(atom, style);
            if atom == " " {
                if cur.is_empty() && !preformatted {
                    continue; // prose never starts a line with a space
                }
                cur.push(' ');
                cur_w += w;
                continue;
            }
            // Break mid-atom when it is wider than a whole line, or when the
            // line so far is only indentation and the atom would not fit:
            // emitting a whitespace-only line would lose the code's shape.
            let only_indent = preformatted && !cur.is_empty() && cur.chars().all(|c| c == ' ');
            let must_split =
                max_width > 0.0 && (w > max_width || (only_indent && cur_w + w > max_width));
            if !must_split && !cur.is_empty() && cur_w + w > max_width {
                let line = if preformatted {
                    cur.clone()
                } else {
                    cur.trim_end().to_string()
                };
                if !line.is_empty() {
                    lines.push(line);
                }
                cur.clear();
                cur_w = 0.0;
            }
            if must_split {
                // Fill the current line first so indentation is preserved,
                // then continue from column zero.
                for c in atom.chars() {
                    let cw = self.advance(c, style);
                    if !cur.is_empty() && cur_w + cw > max_width {
                        let line = if preformatted {
                            cur.clone()
                        } else {
                            cur.trim_end().to_string()
                        };
                        lines.push(line);
                        cur.clear();
                        cur_w = 0.0;
                    }
                    cur.push(c);
                    cur_w += cw;
                }
            } else {
                cur.push_str(atom);
                cur_w += w;
            }
        }
        let last = if preformatted {
            cur.clone()
        } else {
            cur.trim_end().to_string()
        };
        if !last.is_empty() || lines.is_empty() {
            lines.push(last);
        }
        lines
    }

    /// Measures the widest line of a multi-line string.
    pub fn measure_widest_line(&self, s: &str, style: &TextStyle) -> f32 {
        s.lines()
            .map(|l| self.measure(l, style))
            .fold(0.0, f32::max)
    }
}

/// Splits text into unbreakable atoms: runs of non-space non-wide chars,
/// single wide characters, and single spaces.
///
/// ```
/// use markdown2svg::text::atomize;
/// assert_eq!(atomize("hi 你 ok"), ["hi", " ", "你", " ", "ok"]);
/// ```
pub fn atomize(s: &str) -> Vec<&str> {
    let mut atoms = Vec::new();
    let mut start = 0usize;
    let bytes: Vec<(usize, char)> = s.char_indices().collect();
    let mut i = 0usize;
    while i < bytes.len() {
        let (idx, c) = bytes[i];
        if c == ' ' || is_wide(c) {
            if start < idx {
                atoms.push(&s[start..idx]);
            }
            let end = idx + c.len_utf8();
            atoms.push(&s[idx..end]);
            start = end;
        }
        i += 1;
    }
    if start < s.len() {
        atoms.push(&s[start..]);
    }
    atoms
}

/// Whether a character occupies a full em square (CJK, Hangul, Kana,
/// fullwidth forms, emoji).
pub fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F      // Hangul Jamo
        | 0x1F000..=0x1FAFF  // emoji, pictographs, symbols
        | 0x2600..=0x27BF    // miscellaneous symbols & dingbats (✅ ✔ ✖ …)
        | 0x2B00..=0x2BFF     // miscellaneous symbols and arrows
        | 0x2E80..=0x303E     // CJK radicals, Kangxi, CJK symbols
        | 0x3041..=0x33FF    // Hiragana..CJK compatibility
        | 0x3400..=0x4DBF    // CJK Extension A
        | 0x4E00..=0x9FFF    // CJK Unified Ideographs
        | 0xA000..=0xA4CF    // Yi
        | 0xAC00..=0xD7A3    // Hangul syllables
        | 0xF900..=0xFAFF    // CJK compatibility ideographs
        | 0xFE30..=0xFE4F    // CJK compatibility forms
        | 0xFF00..=0xFF60    // Fullwidth forms
        | 0xFFE0..=0xFFE6
    )
}

/// Advance of a Latin character as a fraction of the em, for the default
/// proportional font. Values are approximate but monotone in visual width,
/// which is all line breaking needs.
fn sans_em(c: char) -> f32 {
    match c {
        ' ' => 0.28,
        '\t' => 1.12,
        '.' | ',' | ':' | ';' | '!' | '\'' | '|' | 'i' | 'l' | 'j' | 'I' => 0.30,
        '(' | ')' | '[' | ']' | '{' | '}' | '"' | '/' | '\\' | '-' | '`' => 0.36,
        'f' | 't' | 'r' | 'k' | 's' | 'z' | 'x' | 'v' | 'y' | 'J' | 'L' => 0.50,
        'm' | 'w' | 'M' | 'W' | '@' | '%' | '&' => 0.86,
        '0'..='9' => 0.58,
        'A'..='Z' => 0.68,
        'a'..='z' => 0.56,
        c if is_wide(c) => 1.0,
        _ => 0.58,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(size: f32) -> TextStyle {
        TextStyle::new(size, FontKind::Mono)
    }

    fn sans(size: f32) -> TextStyle {
        TextStyle::new(size, FontKind::Sans)
    }

    #[test]
    fn mono_ascii_advance_is_six_tenths_em() {
        let m = Metrics::new();
        assert!((m.advance('a', &mono(10.0)) - 6.0).abs() < 1e-4);
        assert!((m.measure("abcd", &mono(10.0)) - 24.0).abs() < 1e-4);
    }

    #[test]
    fn mono_cjk_advance_is_one_em() {
        let m = Metrics::new();
        assert!((m.advance('中', &mono(10.0)) - 10.0).abs() < 1e-4);
        assert!((m.advance('ア', &mono(10.0)) - 10.0).abs() < 1e-4);
    }

    #[test]
    fn sans_wide_chars_are_full_em() {
        let m = Metrics::new();
        let style = sans(16.0);
        for c in ['中', '日', 'あ', '한', 'Ａ'] {
            assert!((m.advance(c, &style) - 16.0).abs() < 1e-4, "char {c}");
        }
    }

    #[test]
    fn sans_narrower_than_wide() {
        let m = Metrics::new();
        let style = sans(16.0);
        assert!(m.advance('i', &style) < m.advance('m', &style));
        assert!(m.advance('i', &style) < m.advance('中', &style));
    }

    #[test]
    fn zero_size_measures_zero() {
        let m = Metrics::new();
        assert_eq!(m.measure("anything", &mono(0.0)), 0.0);
    }

    #[test]
    fn empty_string_measures_zero() {
        assert_eq!(Metrics::new().measure("", &sans(12.0)), 0.0);
    }

    #[test]
    fn wraps_on_spaces() {
        let m = Metrics::new();
        let style = mono(10.0); // 6px per char
        let lines = m.wrap("aa bb cc", 36.0, &style);
        assert_eq!(lines, ["aa bb", "cc"]);
    }

    #[test]
    fn long_word_is_hard_broken() {
        let m = Metrics::new();
        let style = mono(10.0);
        let lines = m.wrap("abcdefgh", 30.0, &style);
        assert_eq!(lines, ["abcde", "fgh"]);
    }

    #[test]
    fn cjk_wraps_between_characters() {
        let m = Metrics::new();
        let style = sans(10.0); // one CJK char = 10px
        let lines = m.wrap("中文测试", 20.0, &style);
        assert_eq!(lines, ["中文", "测试"]);
    }

    #[test]
    fn mixed_cjk_and_latin() {
        let m = Metrics::new();
        let style = sans(10.0);
        let lines = m.wrap("ab中文", 20.0, &style);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn empty_input_yields_one_empty_line() {
        assert_eq!(Metrics::new().wrap("", 100.0, &sans(10.0)), vec![""]);
    }

    #[test]
    fn trailing_spaces_are_trimmed() {
        let m = Metrics::new();
        let lines = m.wrap("aa   ", 100.0, &mono(10.0));
        assert_eq!(lines, ["aa"]);
    }

    #[test]
    fn leading_spaces_are_dropped_at_line_start() {
        let m = Metrics::new();
        let lines = m.wrap("  aa", 100.0, &mono(10.0));
        assert_eq!(lines, ["aa"]);
    }

    #[test]
    fn exact_fit_does_not_break() {
        let m = Metrics::new();
        let lines = m.wrap("abcde", 30.0, &mono(10.0));
        assert_eq!(lines, ["abcde"]);
    }

    #[test]
    fn zero_width_still_terminates() {
        let m = Metrics::new();
        let lines = m.wrap("abc", 0.0, &mono(10.0));
        assert!(!lines.is_empty());
        assert_eq!(lines.concat(), "abc");
    }

    #[test]
    fn atomize_splits_words_wide_chars_and_spaces() {
        assert_eq!(atomize("hi 你 ok"), ["hi", " ", "你", " ", "ok"]);
        assert_eq!(atomize(""), Vec::<&str>::new());
        assert_eq!(atomize("中"), ["中"]);
        assert_eq!(atomize("hello"), ["hello"]);
    }

    #[test]
    fn atomize_never_loses_characters() {
        for s in ["a b", "中文字", "mix 中 文", "  spaced  "] {
            let joined: String = atomize(s).concat();
            assert_eq!(joined, s);
        }
    }

    #[test]
    fn widest_line() {
        let m = Metrics::new();
        let style = mono(10.0);
        assert!((m.measure_widest_line("a\nabc\nab", &style) - 18.0).abs() < 1e-4);
    }

    #[test]
    fn is_wide_classification() {
        assert!(is_wide('中'));
        assert!(is_wide('あ'));
        assert!(is_wide('한'));
        assert!(is_wide('😀'));
        // Emoji-presentation symbols occupy roughly one em, not a fraction.
        assert!(is_wide('✅'));
        assert!(is_wide('✔'));
        assert!(!is_wide('a'));
        assert!(!is_wide('1'));
        assert!(!is_wide('é'));
    }

    #[test]
    fn style_builders_set_flags() {
        let style = TextStyle::new(12.0, FontKind::Sans);
        assert!(!style.bold && !style.italic);
        let style = style.bold().italic();
        assert!(style.bold && style.italic);
        assert_eq!(style.size, 12.0);
        assert_eq!(style.family, FontKind::Sans);
    }

    #[test]
    fn letter_spacing_widens_every_character() {
        let m = Metrics::new();
        let plain = TextStyle::new(10.0, FontKind::Mono);
        let tracked = plain.with_letter_spacing(0.5);
        // 6px glyph + 5px tracking.
        assert!((m.advance('a', &tracked) - 11.0).abs() < 1e-4);
        assert!((m.measure("abc", &tracked) - 33.0).abs() < 1e-4);
        // Tracking applies to spaces too, so wrapping reacts to it.
        assert!(m.measure("a b", &tracked) > m.measure("a b", &plain));
    }

    #[test]
    fn letter_spacing_changes_wrapping() {
        let m = Metrics::new();
        let plain = TextStyle::new(10.0, FontKind::Mono);
        let tracked = plain.with_letter_spacing(0.5);
        assert_eq!(m.wrap("aa bb", 30.0, &plain), ["aa bb"]);
        assert_eq!(m.wrap("aa bb", 30.0, &tracked), ["aa", "bb"]);
    }

    #[test]
    fn bold_sans_is_slightly_wider() {
        let m = Metrics::new();
        let plain = TextStyle::new(10.0, FontKind::Sans);
        let bold = plain.bold();
        assert!(m.advance('m', &bold) > m.advance('m', &plain));
        // Monospace is unaffected: columns must stay aligned.
        let mono = TextStyle::new(10.0, FontKind::Mono);
        assert_eq!(m.advance('m', &mono.bold()), m.advance('m', &mono));
    }

    #[test]
    fn tabs_have_a_width() {
        let m = Metrics::new();
        assert!(m.advance('\t', &mono(10.0)) > 0.0);
        assert!(m.advance('\t', &sans(10.0)) > 0.0);
    }

    #[test]
    fn widest_line_of_empty_is_zero() {
        assert_eq!(Metrics::new().measure_widest_line("", &mono(10.0)), 0.0);
    }

    #[test]
    fn zero_width_wraps_by_character() {
        let m = Metrics::new();
        let lines = m.wrap("中文", 0.0, &sans(10.0));
        assert_eq!(lines.concat(), "中文");
        assert!(lines.len() >= 2);
    }

    #[test]
    fn preformatted_wrapping_keeps_leading_and_inner_spaces() {
        let m = Metrics::new();
        let style = mono(10.0);
        assert_eq!(
            m.wrap_preformatted("    indented", 200.0, &style),
            ["    indented"]
        );
        assert_eq!(m.wrap_preformatted("a    b", 200.0, &style), ["a    b"]);
        // Prose, by contrast, drops leading spaces.
        assert_eq!(m.wrap("    indented", 200.0, &style), ["indented"]);
    }

    #[test]
    fn preformatted_lines_still_wrap_when_too_long() {
        let m = Metrics::new();
        let style = mono(10.0); // 6px per char
        // 4 spaces of indent (24px) + 3 chars = 42px.
        let lines = m.wrap_preformatted("    abcdef", 42.0, &style);
        assert_eq!(lines, ["    abc", "def"]);
    }

    #[test]
    fn preformatted_empty_input_yields_one_empty_line() {
        assert_eq!(
            Metrics::new().wrap_preformatted("", 100.0, &mono(10.0)),
            vec![""]
        );
    }
}
