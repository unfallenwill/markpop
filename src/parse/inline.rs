//! Inline parsing: text spans to [`Inline`] trees.
//!
//! Two phases keep the tricky part isolated and testable:
//!
//! 1. **Tokenize** into text, code spans, hard breaks, links/images and
//!    *delimiter runs* (`*`, `_`, `~~`) tagged with whether they can open
//!    or close emphasis (flanking rules).
//! 2. **Resolve** delimiter runs with a stack-like pass: repeatedly pair
//!    the leftmost closer with the nearest matching opener.
//!
//! The subset is deliberate: emphasis runs of equal length nest correctly
//! (`*a **b** c*`, `***x***`), and unmatched runs degrade to literal
//! characters rather than being dropped.

use crate::ast::{ImageData, Inline};

/// Parses the inline content of a block (paragraph, heading, table cell).
pub(crate) fn parse_inlines(text: &str) -> Vec<Inline> {
    let tokens = tokenize(text);
    normalize(resolve(tokens))
}

/// One lexical unit before emphasis resolution.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Text(String),
    Code(String),
    Break,
    Delim {
        ch: char,
        count: usize,
        can_open: bool,
        can_close: bool,
    },
    Node(Inline),
}

fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<Tok> = Vec::new();
    let mut i = 0usize;
    let mut buf = String::new();

    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Tok::Text(std::mem::take(&mut buf)));
            }
        };
    }

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                match chars.get(i + 1) {
                    Some(&'\n') => {
                        // Backslash before a newline is a hard break.
                        flush!();
                        out.push(Tok::Break);
                        i += 2;
                    }
                    Some(&next) if is_escapable(next) => {
                        buf.push(next);
                        i += 2;
                    }
                    _ => {
                        buf.push('\\');
                        i += 1;
                    }
                }
            }
            '`' => {
                let run = count_run(&chars, i, '`');
                if let Some((content, next)) = scan_code_span(&chars, i + run, run) {
                    flush!();
                    out.push(Tok::Code(content));
                    i = next;
                } else {
                    for _ in 0..run {
                        buf.push('`');
                    }
                    i += run;
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((alt, url, next)) = scan_link_like(&chars, i + 2, false) {
                    flush!();
                    out.push(Tok::Node(Inline::Image(ImageData { alt, url })));
                    i = next;
                } else {
                    buf.push('!');
                    i += 1;
                }
            }
            '[' => {
                if let Some((label, url, next)) = scan_link_like(&chars, i + 1, true) {
                    flush!();
                    let text = parse_inlines(&label);
                    out.push(Tok::Node(Inline::Link { text, url }));
                    i = next;
                } else {
                    buf.push('[');
                    i += 1;
                }
            }
            '<' => {
                if let Some((display, url, next)) = scan_autolink(&chars, i) {
                    flush!();
                    out.push(Tok::Node(Inline::Link {
                        text: vec![Inline::Text(display)],
                        url,
                    }));
                    i = next;
                } else {
                    buf.push('<');
                    i += 1;
                }
            }
            '&' => {
                if let Some((ch, next)) = scan_entity(&chars, i) {
                    buf.push(ch);
                    i = next;
                } else {
                    buf.push('&');
                    i += 1;
                }
            }
            '\n' => {
                flush!();
                out.push(Tok::Text(" ".into()));
                i += 1;
            }
            ' ' => {
                // Two or more trailing spaces before a newline: hard break.
                let run = count_run(&chars, i, ' ');
                if run >= 2 && chars.get(i + run) == Some(&'\n') {
                    flush!();
                    out.push(Tok::Break);
                    i += run + 1;
                } else {
                    for _ in 0..run {
                        buf.push(' ');
                    }
                    i += run;
                }
            }
            '*' | '_' | '~' => {
                let run = count_run(&chars, i, c);
                let prev = i.checked_sub(1).map(|p| chars[p]);
                let next = chars.get(i + run).copied();
                let (can_open, can_close) = flanking(c, prev, next);
                flush!();
                push_delims(&mut out, c, run, can_open, can_close);
                i += run;
            }
            _ => {
                buf.push(c);
                i += 1;
            }
        }
    }
    flush!();
    out
}

/// Splits a marker run into delimiter tokens.
///
/// Runs are emitted as `**` pairs plus a possible single `*`, ordered so
/// that nesting works out: openers are emitted widest-first, closers
/// narrowest-first. This makes `***x***` resolve to strong-wrapping-emph
/// and `*a **b** c*` keep the outer emphasis.
fn push_delims(out: &mut Vec<Tok>, ch: char, run: usize, can_open: bool, can_close: bool) {
    if ch == '~' {
        // GFM strikethrough needs a `~~` pair; stray tildes stay literal.
        let pairs = run / 2;
        for _ in 0..pairs {
            out.push(Tok::Delim {
                ch,
                count: 2,
                can_open,
                can_close,
            });
        }
        if run % 2 == 1 {
            out.push(Tok::Text("~".into()));
        }
        return;
    }

    let mut counts: Vec<usize> = Vec::new();
    let mut rem = run;
    while rem >= 2 {
        counts.push(2);
        rem -= 2;
    }
    if rem == 1 {
        counts.push(1);
    }
    if can_close && !can_open {
        counts.reverse();
    }
    for count in counts {
        out.push(Tok::Delim {
            ch,
            count,
            can_open,
            can_close,
        });
    }
}

/// CommonMark-ish flanking rules, simplified but predictable.
fn flanking(ch: char, prev: Option<char>, next: Option<char>) -> (bool, bool) {
    let next = match next {
        Some(c) => c,
        None => return (false, prev.is_some_and(|p| !p.is_whitespace())),
    };
    let prev = match prev {
        Some(c) => c,
        None => return (!next.is_whitespace(), false),
    };
    let next_ws = next.is_whitespace();
    let prev_ws = prev.is_whitespace();
    let next_punct = is_punct(next);
    let prev_punct = is_punct(prev);

    let left_flanking = !next_ws && (!next_punct || prev_ws || prev_punct);
    let right_flanking = !prev_ws && (!prev_punct || next_ws || next_punct);

    if ch == '_' {
        // Underscores do not work inside words (`snake_case_name`).
        let intra_word = prev.is_alphanumeric() && next.is_alphanumeric();
        let can_open = left_flanking && (!right_flanking || prev_punct);
        let can_close = right_flanking && (!left_flanking || next_punct);
        if intra_word {
            (false, false)
        } else {
            (can_open, can_close)
        }
    } else {
        (left_flanking, right_flanking)
    }
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '，' | '。' | '、' | '；' | '：' | '！' | '？' | '「' | '」' | '（' | '）'
        )
}

fn is_escapable(c: char) -> bool {
    c.is_ascii_punctuation()
}

fn count_run(chars: &[char], start: usize, c: char) -> usize {
    chars[start..].iter().take_while(|ch| **ch == c).count()
}

/// Scans a code span starting at `start` (just past the opening run).
/// Returns the normalized content and the index after the closing run.
fn scan_code_span(chars: &[char], start: usize, run: usize) -> Option<(String, usize)> {
    let mut i = start;
    while i < chars.len() {
        if chars[i] == '`' {
            let closing = count_run(chars, i, '`');
            if closing == run {
                let raw: String = chars[start..i].iter().collect();
                return Some((normalize_code_span(&raw), i + closing));
            }
            i += closing;
        } else {
            i += 1;
        }
    }
    None
}

/// CommonMark code-span normalization: newlines become spaces, and one
/// surrounding space is stripped unless the content is all spaces.
fn normalize_code_span(raw: &str) -> String {
    let replaced = raw.replace('\n', " ");
    if replaced.len() >= 2
        && replaced.starts_with(' ')
        && replaced.ends_with(' ')
        && !replaced.trim().is_empty()
    {
        replaced[1..replaced.len() - 1].to_string()
    } else {
        replaced
    }
}

/// Scans `label](url)` given the index just past the opening `[`.
/// Returns (label, url, index after the closing paren).
fn scan_link_like(
    chars: &[char],
    start: usize,
    allow_emphasis: bool,
) -> Option<(String, String, usize)> {
    let mut depth = 1usize;
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            '[' => {
                depth += 1;
                i += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 || i >= chars.len() {
        return None;
    }
    let label: String = chars[start..i].iter().collect();
    if chars.get(i + 1) != Some(&'(') {
        return None;
    }
    let mut j = i + 2;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    let url_start = j;
    if chars.get(j) == Some(&'<') {
        j += 1;
        let inner_start = j;
        while j < chars.len() && chars[j] != '>' {
            j += 1;
        }
        if j >= chars.len() {
            return None;
        }
        let url: String = chars[inner_start..j].iter().collect();
        j += 1;
        let end = skip_title_and_close(chars, j)?;
        return Some((label, url, end));
    }
    let mut parens = 0usize;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 2,
            '(' => {
                parens += 1;
                j += 1;
            }
            ')' if parens == 0 => break,
            ')' => {
                parens -= 1;
                j += 1;
            }
            _ => j += 1,
        }
    }
    if j >= chars.len() {
        return None;
    }
    let url: String = chars[url_start..j]
        .iter()
        .collect::<String>()
        .trim()
        .to_string();
    let (url, tail) = split_url_and_tail(&url);
    if !tail.is_empty() && !is_title(tail) {
        return None;
    }
    if url.is_empty() && allow_emphasis {
        return None;
    }
    Some((label, url, j + 1))
}

/// Splits a raw destination like `url "title"` into the URL and the rest.
fn split_url_and_tail(raw: &str) -> (String, &str) {
    match raw.find(char::is_whitespace) {
        Some(k) => (raw[..k].to_string(), raw[k..].trim()),
        None => (raw.to_string(), ""),
    }
}

/// Whether `s` is a quoted link title (`"t"`, `'t'` or `(t)`).
fn is_title(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return false;
    }
    let close = match bytes[0] {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return false,
    };
    bytes[bytes.len() - 1] == close
}

/// Skips an optional `"title"`/`'title'`/`(title)` and the closing paren.
fn skip_title_and_close(chars: &[char], from: usize) -> Option<usize> {
    let mut j = from;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if chars.get(j) == Some(&')') {
        return Some(j + 1);
    }
    let open = *chars.get(j)?;
    let close = match open {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    j += 1;
    while j < chars.len() && chars[j] != close {
        j += 1;
    }
    j += 1;
    while j < chars.len() && chars[j] == ' ' {
        j += 1;
    }
    if chars.get(j) == Some(&')') {
        Some(j + 1)
    } else {
        None
    }
}

/// Scans `<https://example.com>` or `<user@example.com>`.
/// Returns (display text, destination URL, index after `>`).
fn scan_autolink(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    while i < chars.len() && chars[i] != '>' {
        if chars[i].is_whitespace() || chars[i] == '<' {
            return None;
        }
        i += 1;
    }
    if i >= chars.len() {
        return None;
    }
    let inner: String = chars[start + 1..i].iter().collect();
    let looks_like_url = inner.contains("://") || inner.starts_with("mailto:");
    let looks_like_email = inner.contains('@')
        && !inner.contains('/')
        && inner.split('@').count() == 2
        && !inner.starts_with('@')
        && !inner.ends_with('@');
    if !looks_like_url && !looks_like_email {
        return None;
    }
    let url = if looks_like_email && !inner.starts_with("mailto:") {
        format!("mailto:{inner}")
    } else {
        inner.clone()
    };
    Some((inner, url, i + 1))
}

/// Decodes a small set of HTML entities (`&amp;`, `&#39;`, …).
fn scan_entity(chars: &[char], start: usize) -> Option<(char, usize)> {
    let mut i = start + 1;
    let mut name = String::new();
    while i < chars.len() && chars[i] != ';' && name.len() < 10 {
        name.push(chars[i]);
        i += 1;
    }
    if chars.get(i) != Some(&';') {
        return None;
    }
    let ch = match name.as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        hex if hex.starts_with("#x") || hex.starts_with("#X") => u32::from_str_radix(&hex[2..], 16)
            .ok()
            .and_then(char::from_u32)?,
        dec if dec.starts_with('#') => dec[1..].parse::<u32>().ok().and_then(char::from_u32)?,
        _ => return None,
    };
    Some((ch, i + 1))
}

/// Pairs delimiter tokens into emphasis/strong/strike nodes.
fn resolve(tokens: Vec<Tok>) -> Vec<Inline> {
    let mut tokens = tokens;
    while let Some(close_idx) = tokens.iter().position(|t| {
        matches!(
            t,
            Tok::Delim {
                can_close: true,
                ..
            }
        )
    }) {
        let Tok::Delim {
            ch,
            count: close_count,
            ..
        } = tokens[close_idx]
        else {
            unreachable!("position() matched a delimiter");
        };

        let open_idx = (0..close_idx).rev().find(|&j| match tokens[j] {
            Tok::Delim {
                ch: c,
                count,
                can_open: true,
                ..
            } => c == ch && count > 0,
            _ => false,
        });

        let Some(open_idx) = open_idx else {
            // No opener: this run is literal text.
            tokens[close_idx] = Tok::Text(ch.to_string().repeat(close_count));
            continue;
        };

        let Tok::Delim {
            count: open_count, ..
        } = tokens[open_idx]
        else {
            unreachable!("index came from the wildcard above");
        };
        // Strikethrough only matches as a `~~` pair.
        // `~~` is the only tilde delimiter the tokenizer emits, so a tilde
        // run always closes as strikethrough; other markers close as strong
        // when both sides have at least two characters.
        let k = if ch == '~' || (open_count >= 2 && close_count >= 2) {
            2
        } else {
            1
        };

        // Consume the opener: either shrink it or blank it out so it can
        // never match again. A zero-count delimiter disappears in
        // `to_inlines`.
        tokens[open_idx] = Tok::Delim {
            ch,
            count: open_count - k,
            can_open: open_count > k,
            can_close: false,
        };

        let inner_tokens: Vec<Tok> = tokens.drain(open_idx + 1..close_idx).collect();
        let inner = to_inlines(inner_tokens);
        let node = match (ch, k) {
            ('~', _) => Inline::Strike(inner),
            (_, 2) => Inline::Strong(inner),
            _ => Inline::Emph(inner),
        };

        // `drain` shifted the closer down to `open_idx + 1`.
        let close_pos = open_idx + 1;
        tokens[close_pos] = Tok::Node(node);
        if close_count > k {
            tokens.insert(
                close_pos + 1,
                Tok::Delim {
                    ch,
                    count: close_count - k,
                    can_open: false,
                    can_close: true,
                },
            );
        }
    }
    to_inlines(tokens)
}

/// Converts leftover tokens to inlines (stray delimiters become literal).
fn to_inlines(tokens: Vec<Tok>) -> Vec<Inline> {
    let mut out = Vec::with_capacity(tokens.len());
    for tok in tokens {
        match tok {
            Tok::Text(s) => out.push(Inline::Text(s)),
            Tok::Code(s) => out.push(Inline::Code(s)),
            Tok::Break => out.push(Inline::Break),
            Tok::Node(node) => out.push(node),
            Tok::Delim { ch, count, .. } => out.push(Inline::Text(ch.to_string().repeat(count))),
        }
    }
    out
}

/// Merges adjacent text nodes and drops empty ones.
fn normalize(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text(s) if s.is_empty() => {}
            Inline::Text(s) => match out.last_mut() {
                Some(Inline::Text(prev)) => prev.push_str(&s),
                _ => out.push(Inline::Text(s)),
            },
            Inline::Emph(inner) => {
                let inner = normalize(inner);
                if !inner.is_empty() {
                    out.push(Inline::Emph(inner));
                }
            }
            Inline::Strong(inner) => {
                let inner = normalize(inner);
                if !inner.is_empty() {
                    out.push(Inline::Strong(inner));
                }
            }
            Inline::Strike(inner) => {
                let inner = normalize(inner);
                if !inner.is_empty() {
                    out.push(Inline::Strike(inner));
                }
            }
            Inline::Link { text, url } => {
                let text = normalize(text);
                if !text.is_empty() {
                    out.push(Inline::Link { text, url });
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::plain_text_of;

    /// Parses inline content and returns the plain text.
    fn text(s: &str) -> String {
        plain_text_of(&parse_inlines(s))
    }

    #[test]
    fn plain_text_passes_through() {
        let inlines = parse_inlines("hello world");
        assert_eq!(inlines, vec![Inline::Text("hello world".into())]);
    }

    #[test]
    fn soft_break_becomes_space() {
        assert_eq!(text("a\nb"), "a b");
    }

    #[test]
    fn hard_break_from_two_spaces() {
        let inlines = parse_inlines("a  \nb");
        assert_eq!(
            inlines,
            vec![
                Inline::Text("a".into()),
                Inline::Break,
                Inline::Text("b".into())
            ]
        );
    }

    #[test]
    fn hard_break_from_backslash() {
        assert_eq!(
            parse_inlines("a\\\nb"),
            vec![
                Inline::Text("a".into()),
                Inline::Break,
                Inline::Text("b".into())
            ]
        );
    }

    #[test]
    fn single_trailing_space_is_not_a_break() {
        assert_eq!(text("a \nb"), "a  b");
    }

    #[test]
    fn escapes_are_literal() {
        assert_eq!(text("\\*not em\\*"), "*not em*");
        assert_eq!(text("\\\\"), "\\");
        assert_eq!(text("a\\`b"), "a`b");
    }

    #[test]
    fn backslash_before_letter_is_literal() {
        assert_eq!(text("\\n"), "\\n");
    }

    #[test]
    fn emphasis_basic() {
        let inlines = parse_inlines("*a*");
        assert_eq!(inlines, vec![Inline::Emph(vec![Inline::Text("a".into())])]);
    }

    #[test]
    fn strong_basic() {
        let inlines = parse_inlines("**a**");
        assert_eq!(
            inlines,
            vec![Inline::Strong(vec![Inline::Text("a".into())])]
        );
    }

    #[test]
    fn underscore_variants() {
        assert_eq!(
            parse_inlines("_a_"),
            vec![Inline::Emph(vec![Inline::Text("a".into())])]
        );
        assert_eq!(
            parse_inlines("__a__"),
            vec![Inline::Strong(vec![Inline::Text("a".into())])]
        );
    }

    #[test]
    fn intraword_underscore_stays_literal() {
        assert_eq!(text("snake_case_name"), "snake_case_name");
    }

    #[test]
    fn triple_marker_is_strong_and_emph() {
        let inlines = parse_inlines("***a***");
        let expected = vec![Inline::Strong(vec![Inline::Emph(vec![Inline::Text(
            "a".into(),
        )])])];
        assert_eq!(inlines, expected);
        assert_eq!(text("***a***"), "a");
    }

    #[test]
    fn nested_emphasis() {
        let inlines = parse_inlines("*a **b** c*");
        let expected = vec![Inline::Emph(vec![
            Inline::Text("a ".into()),
            Inline::Strong(vec![Inline::Text("b".into())]),
            Inline::Text(" c".into()),
        ])];
        assert_eq!(inlines, expected);
    }

    #[test]
    fn strikethrough() {
        assert_eq!(
            parse_inlines("~~x~~"),
            vec![Inline::Strike(vec![Inline::Text("x".into())])]
        );
    }

    #[test]
    fn single_tilde_is_literal() {
        assert_eq!(text("~x~"), "~x~");
    }

    #[test]
    fn unmatched_markers_stay_literal() {
        assert_eq!(text("*a"), "*a");
        assert_eq!(text("a*"), "a*");
        assert_eq!(text("**a"), "**a");
    }

    #[test]
    fn emphasis_does_not_span_surrounding_spaces() {
        // `* a *` has no left-flanking opener, so the stars are literal.
        assert_eq!(text("* a *"), "* a *");
    }

    #[test]
    fn code_span_is_verbatim() {
        assert_eq!(
            parse_inlines("`*not em*`"),
            vec![Inline::Code("*not em*".into())]
        );
    }

    #[test]
    fn code_span_with_backticks() {
        assert_eq!(
            parse_inlines("``a ` b``"),
            vec![Inline::Code("a ` b".into())]
        );
    }

    #[test]
    fn code_span_strips_one_padding_space() {
        assert_eq!(parse_inlines("` a `"), vec![Inline::Code("a".into())]);
        assert_eq!(parse_inlines("`  `"), vec![Inline::Code("  ".into())]);
    }

    #[test]
    fn code_span_joins_lines() {
        assert_eq!(parse_inlines("`a\nb`"), vec![Inline::Code("a b".into())]);
    }

    #[test]
    fn unclosed_code_span_is_literal() {
        assert_eq!(text("`a"), "`a");
    }

    #[test]
    fn links() {
        let inlines = parse_inlines("[docs](https://example.com)");
        assert_eq!(
            inlines,
            vec![Inline::Link {
                text: vec![Inline::Text("docs".into())],
                url: "https://example.com".into(),
            }]
        );
    }

    #[test]
    fn link_with_title() {
        let inlines = parse_inlines("[a](url \"the title\")");
        assert_eq!(
            inlines,
            vec![Inline::Link {
                text: vec![Inline::Text("a".into())],
                url: "url".into(),
            }]
        );
    }

    #[test]
    fn link_text_can_be_styled() {
        let inlines = parse_inlines("[**bold**](u)");
        assert_eq!(
            inlines,
            vec![Inline::Link {
                text: vec![Inline::Strong(vec![Inline::Text("bold".into())])],
                url: "u".into(),
            }]
        );
    }

    #[test]
    fn link_with_angle_bracket_destination() {
        let inlines = parse_inlines("[a](<https://x.y/a b>)");
        match &inlines[0] {
            Inline::Link { url, .. } => assert_eq!(url, "https://x.y/a b"),
            other => panic!("expected link, got {other:?}"),
        }
    }

    #[test]
    fn link_with_angle_destination_and_title() {
        let inlines = parse_inlines("[a](<u> \"t\")");
        match &inlines[0] {
            Inline::Link { url, .. } => assert_eq!(url, "u"),
            other => panic!("expected link, got {other:?}"),
        }
    }

    #[test]
    fn link_destination_may_contain_balanced_parens() {
        let inlines = parse_inlines("[a](https://x.y/a(b)c)");
        match &inlines[0] {
            Inline::Link { url, .. } => assert_eq!(url, "https://x.y/a(b)c"),
            other => panic!("expected link, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_angle_destination_is_literal() {
        assert_eq!(text("[a](<u)"), "[a](<u)");
    }

    #[test]
    fn link_with_nested_brackets() {
        assert_eq!(text("[a [b] c](u)"), "a [b] c");
    }

    #[test]
    fn image_inline() {
        assert_eq!(
            parse_inlines("![alt](x.png)"),
            vec![Inline::Image(ImageData {
                alt: "alt".into(),
                url: "x.png".into()
            })]
        );
    }

    #[test]
    fn image_without_url_is_literal() {
        assert_eq!(text("![alt]"), "![alt]");
    }

    #[test]
    fn angle_brackets_without_link_are_literal() {
        assert_eq!(text("<b>"), "<b>");
    }

    #[test]
    fn autolink_url() {
        let inlines = parse_inlines("<https://example.com/a?b=1>");
        assert_eq!(
            inlines,
            vec![Inline::Link {
                text: vec![Inline::Text("https://example.com/a?b=1".into())],
                url: "https://example.com/a?b=1".into(),
            }]
        );
    }

    #[test]
    fn autolink_email_gets_mailto() {
        let inlines = parse_inlines("<me@example.com>");
        match &inlines[0] {
            Inline::Link { url, text } => {
                assert_eq!(url, "mailto:me@example.com");
                assert_eq!(plain_text_of(text), "me@example.com");
            }
            other => panic!("expected link, got {other:?}"),
        }
    }

    #[test]
    fn link_url_stops_at_whitespace() {
        let inlines = parse_inlines("[a](https://x.y \"t\")");
        match &inlines[0] {
            Inline::Link { url, .. } => assert_eq!(url, "https://x.y"),
            other => panic!("expected link, got {other:?}"),
        }
        // A malformed tail means it is not a link at all.
        assert_eq!(text("[a](url garbage)"), "[a](url garbage)");
    }

    #[test]
    fn entities_decode() {
        assert_eq!(text("a &amp; b"), "a & b");
        assert_eq!(text("&lt;tag&gt;"), "<tag>");
        assert_eq!(text("&#65;&#x42;"), "AB");
        assert_eq!(text("&unknown;"), "&unknown;");
    }

    #[test]
    fn escaped_marker_is_not_emphasis() {
        assert_eq!(text("a\\*b*c"), "a*b*c");
    }

    #[test]
    fn adjacent_text_is_coalesced() {
        let inlines = parse_inlines("a\\*b");
        assert_eq!(inlines.len(), 1);
        assert_eq!(text("a\\*b"), "a*b");
    }

    #[test]
    fn marker_run_without_flanking_is_literal() {
        assert_eq!(parse_inlines("****"), vec![Inline::Text("****".into())]);
    }

    #[test]
    fn punctuation_flanking_allows_emphasis() {
        // A closing marker before punctuation and an opener after it both
        // count as valid flanking.
        assert_eq!(
            parse_inlines("(*a*)"),
            vec![
                Inline::Text("(".into()),
                Inline::Emph(vec![Inline::Text("a".into())]),
                Inline::Text(")".into()),
            ]
        );
    }
}
