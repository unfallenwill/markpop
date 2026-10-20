//! Inline parsing, following the CommonMark algorithm.
//!
//! The pipeline is the one the specification describes:
//!
//! 1. **Tokenize** the text into literal text, code spans, raw HTML,
//!    autolinks, entities and *delimiter runs* (`*`, `_`, `~~`) tagged with
//!    the left/right-flanking rules.
//! 2. **Resolve emphasis** by repeatedly matching the first closer with the
//!    nearest opener of the same character, honouring the "multiple of 3"
//!    rule, and wrapping everything in between.
//! 3. Links and images are resolved during tokenization, because their text
//!    is parsed recursively (a link may not contain another link, which is
//!    exactly what the recursion detects).

use std::collections::HashMap;

use super::Extensions;
use super::entities::NAMED;
use crate::ast::{ImageData, Inline, plain_text_of};

/// One link reference definition: destination plus optional title.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct LinkRef {
    /// Destination URL.
    pub url: String,
    /// Optional title.
    pub title: Option<String>,
}

/// Link reference definitions, keyed by normalised label.
pub(crate) type Refs = HashMap<String, LinkRef>;

/// Parses inline content with no reference definitions available.
pub(crate) fn parse_inlines(text: &str) -> Vec<Inline> {
    parse_with_refs(text, &Refs::new(), Extensions::gfm())
}

/// Parses inline content, resolving reference links through `refs`.
pub(crate) fn parse_with_refs(text: &str, refs: &Refs, ext: Extensions) -> Vec<Inline> {
    let chars: Vec<char> = text.chars().collect();
    let tokens = tokenize(&chars, refs, true, ext);
    resolve(tokens)
}

/// Collapses a link label the way the specification compares them:
/// leading/trailing whitespace removed, internal runs of whitespace turned
/// into one space, and case folded.
pub(crate) fn normalize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        if ch == '\u{1e9e}' || ch == 'ß' {
            // Case folding maps the sharp s to "ss".
            out.push_str("ss");
            continue;
        }
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A finished inline (code span, link, raw HTML, break, …).
    Node(Inline),
    /// Literal text.
    Text(String),
    /// A run of emphasis/strikethrough markers.
    Delim {
        ch: char,
        count: usize,
        can_open: bool,
        can_close: bool,
        /// Length of the original run, needed for the "multiple of 3" rule.
        orig: usize,
    },
}

fn tokenize(chars: &[char], refs: &Refs, allow_links: bool, ext: Extensions) -> Vec<Tok> {
    let mut out: Vec<Tok> = Vec::new();
    let mut buf = String::new();
    let mut i = 0usize;

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
            '\\' => match chars.get(i + 1) {
                Some(&'\n') => {
                    flush!();
                    out.push(Tok::Node(Inline::Break));
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
            },
            '`' => {
                let run = count_run(chars, i, '`');
                if let Some((content, next)) = scan_code_span(chars, i + run, run) {
                    flush!();
                    out.push(Tok::Node(Inline::Code(content)));
                    i = next;
                } else {
                    for _ in 0..run {
                        buf.push('`');
                    }
                    i += run;
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                if let Some((node, next)) = scan_link_like(chars, i + 1, true, refs, ext) {
                    flush!();
                    out.push(Tok::Node(node));
                    i = next;
                } else {
                    buf.push('!');
                    i += 1;
                }
            }
            '[' if allow_links => {
                if let Some((node, next)) = scan_link_like(chars, i, false, refs, ext) {
                    flush!();
                    out.push(Tok::Node(node));
                    i = next;
                } else {
                    buf.push('[');
                    i += 1;
                }
            }
            '!' if chars.get(i + 1) == Some(&'[') => {
                buf.push('!');
                i += 1;
            }
            '<' => {
                if let Some((display, url, next)) = scan_autolink(chars, i) {
                    flush!();
                    out.push(Tok::Node(Inline::Link {
                        text: vec![Inline::Text(display)],
                        url,
                        title: None,
                    }));
                    i = next;
                } else if let Some((raw, next)) = scan_html(chars, i) {
                    flush!();
                    out.push(Tok::Node(Inline::Html(raw)));
                    i = next;
                } else {
                    buf.push('<');
                    i += 1;
                }
            }
            '&' => {
                if let Some((text, next)) = scan_entity(chars, i) {
                    buf.push_str(&text);
                    i = next;
                } else {
                    buf.push('&');
                    i += 1;
                }
            }
            '\n' => {
                // Trailing spaces: two or more make a hard break, otherwise
                // they are stripped before the soft break.
                let mut hard = false;
                if buf.ends_with("  ") {
                    hard = true;
                }
                while buf.ends_with(' ') {
                    buf.pop();
                }
                flush!();
                out.push(Tok::Node(if hard {
                    Inline::Break
                } else {
                    Inline::SoftBreak
                }));
                i += 1;
            }
            _ if allow_links
                && ext.autolinks
                && (is_autolink_literal_start(chars, i) || is_email_literal_start(chars, i)) =>
            {
                if let Some((display, url, next)) = scan_autolink_literal(chars, i) {
                    flush!();
                    out.push(Tok::Node(Inline::Link {
                        text: vec![Inline::Text(display)],
                        url,
                        title: None,
                    }));
                    i = next;
                } else {
                    buf.push(c);
                    i += 1;
                }
            }
            '*' | '_' | '~' => {
                let run = count_run(chars, i, c);
                let prev = i.checked_sub(1).map(|p| chars[p]);
                let next = chars.get(i + run).copied();
                let (can_open, can_close) = flanking(c, prev, next);
                flush!();
                out.push(Tok::Delim {
                    ch: c,
                    count: run,
                    can_open,
                    can_close,
                    orig: run,
                });
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

/// CommonMark left/right-flanking rules.
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
    let next_punct = is_punctuation(next);
    let prev_punct = is_punctuation(prev);

    let left_flanking = !next_ws && (!next_punct || prev_ws || prev_punct);
    let right_flanking = !prev_ws && (!prev_punct || next_ws || next_punct);

    if ch == '_' {
        (
            left_flanking && (!right_flanking || prev_punct),
            right_flanking && (!left_flanking || next_punct),
        )
    } else {
        (left_flanking, right_flanking)
    }
}

/// Unicode punctuation per the specification's definition.
fn is_punctuation(c: char) -> bool {
    // The specification counts both punctuation and symbols, so currency
    // signs, arrows and emoji make `*€*` literal text.
    let symbol = matches!(
        c,
        '\u{20a0}'..='\u{20cf}'
            | '\u{2100}'..='\u{214f}'
            | '\u{2190}'..='\u{2bff}'
            | '\u{1f000}'..='\u{1faff}'
    );
    symbol
        || c.is_ascii_punctuation()
        || matches!(
            c,
            '\u{00a1}'..='\u{00bf}'
                | '\u{2010}'..='\u{2027}'
                | '\u{2030}'..='\u{205e}'
                | '\u{3001}'..='\u{3003}'
                | '\u{3008}'..='\u{3011}'
                | '\u{3014}'..='\u{301f}'
                | '\u{ff01}'..='\u{ff03}'
                | '\u{ff05}'..='\u{ff0a}'
                | '\u{ff0c}'..='\u{ff0f}'
                | '\u{ff1a}'..='\u{ff1b}'
                | '\u{ff1f}'..='\u{ff20}'
                | '\u{ff3b}'..='\u{ff3d}'
                | '\u{ff5b}'..='\u{ff5d}'
                | '\u{ff5f}'..='\u{ff65}'
        )
}

fn is_escapable(c: char) -> bool {
    c.is_ascii_punctuation()
}

fn count_run(chars: &[char], start: usize, c: char) -> usize {
    chars[start..].iter().take_while(|ch| **ch == c).count()
}

// ---------------------------------------------------------------------------
// Scanners
// ---------------------------------------------------------------------------

/// Scans a code span starting just past the opening run.
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

/// Code-span content normalization: newlines become spaces, and one
/// surrounding space is stripped unless the content is all spaces.
fn normalize_code_span(raw: &str) -> String {
    let replaced = raw.replace(['\n', '\r'], " ");
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

/// Scans `<scheme:...>` or `<user@host>`; returns display text, URL and the
/// index just past `>`.
fn scan_autolink(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    while i < chars.len() && chars[i] != '>' {
        if chars[i].is_whitespace() || chars[i] == '<' {
            return None;
        }
        if (chars[i] as u32) < 0x20 {
            return None;
        }
        i += 1;
    }
    if i >= chars.len() || i == start + 1 {
        return None;
    }
    let inner: String = chars[start + 1..i].iter().collect();
    if is_uri_autolink(&inner) {
        return Some((inner.clone(), inner, i + 1));
    }
    if is_email_autolink(&inner) {
        return Some((inner.clone(), format!("mailto:{inner}"), i + 1));
    }
    None
}

/// `scheme:` where scheme is 2..=32 letters/digits/`+.-`, then no spaces.
fn is_uri_autolink(inner: &str) -> bool {
    let Some((scheme, rest)) = inner.split_once(':') else {
        return false;
    };
    if !(2..=32).contains(&scheme.len()) {
        return false;
    }
    let mut scheme_chars = scheme.chars();
    match scheme_chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    if !scheme_chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
        return false;
    }
    rest.chars()
        .all(|c| !c.is_whitespace() && c != '<' && c != '>' && (c as u32) > 0x1f)
}

/// `local@domain` with the character classes the specification lists.
fn is_email_autolink(inner: &str) -> bool {
    let Some((local, domain)) = inner.rsplit_once('@') else {
        return false;
    };
    if local.is_empty() || domain.is_empty() {
        return false;
    }
    let local_ok = local.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '.' | '!'
                    | '#'
                    | '$'
                    | '%'
                    | '&'
                    | '\''
                    | '*'
                    | '+'
                    | '/'
                    | '='
                    | '?'
                    | '^'
                    | '_'
                    | '`'
                    | '{'
                    | '|'
                    | '}'
                    | '~'
                    | '-'
            )
    });
    if !local_ok {
        return false;
    }
    domain.split('.').all(|part| {
        !part.is_empty()
            && part.len() <= 63
            && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            && !part.starts_with('-')
            && !part.ends_with('-')
    })
}

/// Scans inline raw HTML (tags, comments, processing instructions,
/// declarations and CDATA).
fn scan_html(chars: &[char], start: usize) -> Option<(String, usize)> {
    let rest = &chars[start..];
    let text: String = rest.iter().collect();
    // Comments and CDATA first: they may contain characters tags cannot.
    // Degenerate empty comments are still passed through.
    for empty in ["<!-->", "<!--->"] {
        if text.starts_with(empty) {
            return Some((empty.to_string(), start + empty.len()));
        }
    }
    if let Some(body) = text.strip_prefix("<!--") {
        // Text must not start with ">" or "->"; the comment cannot end in "-".
        let rest = &text[4..];
        let starts_badly = rest.starts_with('>') || rest.starts_with("->");
        if let Some(end) = body.find("-->")
            && !starts_badly
            && end > 0
            && !body[..end].ends_with('-')
        {
            let len = 4 + end + 3;
            return Some((text[..len].to_string(), start + len));
        }
        return None;
    }
    if text.starts_with("<![CDATA[") {
        let end = text.find("]]>")?;
        let len = end + 3;
        return Some((text[..len].to_string(), start + len));
    }
    if text.starts_with("<?") {
        let end = text.find("?>")?;
        let len = end + 2;
        return Some((text[..len].to_string(), start + len));
    }
    if let Some(after) = text.strip_prefix("<!") {
        // Declaration: <! + ASCII letter + anything without ">"
        let first = after.chars().next()?;
        if !first.is_ascii_alphabetic() {
            return None;
        }
        let end = text.find('>')?;
        return Some((text[..=end].to_string(), start + end + 1));
    }

    // Open or closing tag.
    let mut idx = start + 1;
    let closing = chars.get(idx) == Some(&'/');
    if closing {
        idx += 1;
    }
    let name_start = idx;
    if !chars.get(idx).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    while chars
        .get(idx)
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '-')
    {
        idx += 1;
    }
    let _name: String = chars[name_start..idx].iter().collect();
    if closing {
        skip_whitespace(chars, &mut idx);
        if chars.get(idx) == Some(&'>') {
            let raw: String = chars[start..=idx].iter().collect();
            return Some((raw, idx + 1));
        }
        return None;
    }
    loop {
        let before = idx;
        skip_whitespace(chars, &mut idx);
        match chars.get(idx) {
            Some(&'>') => {
                let raw: String = chars[start..=idx].iter().collect();
                return Some((raw, idx + 1));
            }
            Some(&'/') if chars.get(idx + 1) == Some(&'>') => {
                let raw: String = chars[start..=idx + 1].iter().collect();
                return Some((raw, idx + 2));
            }
            Some(_) if idx > before => {
                // Attribute name.
                let first = chars[idx];
                if !(first.is_ascii_alphabetic() || matches!(first, '_' | ':')) {
                    return None;
                }
                while chars.get(idx).is_some_and(|c| {
                    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '-')
                }) {
                    idx += 1;
                }
                // Optional value.
                let save = idx;
                skip_whitespace(chars, &mut idx);
                if chars.get(idx) == Some(&'=') {
                    idx += 1;
                    skip_whitespace(chars, &mut idx);
                    match chars.get(idx) {
                        Some(&q @ ('"' | '\'')) => {
                            idx += 1;
                            while idx < chars.len() && chars[idx] != q {
                                idx += 1;
                            }
                            if idx >= chars.len() {
                                return None;
                            }
                            idx += 1;
                        }
                        Some(_) => {
                            let value_start = idx;
                            while chars.get(idx).is_some_and(|c| {
                                !c.is_whitespace()
                                    && !matches!(c, '"' | '\'' | '=' | '<' | '>' | '`')
                            }) {
                                idx += 1;
                            }
                            if idx == value_start {
                                return None;
                            }
                        }
                        None => return None,
                    }
                } else {
                    idx = save;
                }
            }
            _ => return None,
        }
    }
}

fn skip_whitespace(chars: &[char], idx: &mut usize) {
    while chars.get(*idx).is_some_and(|c| c.is_whitespace()) {
        *idx += 1;
    }
}

/// Scans a character reference; returns the expansion and the next index.
fn scan_entity(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut i = start + 1;
    let mut name = String::new();
    while i < chars.len() && chars[i] != ';' && name.len() < 32 {
        name.push(chars[i]);
        i += 1;
    }
    if chars.get(i) != Some(&';') {
        return None;
    }
    if let Some(hex) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
        // At most six hex digits.
        if hex.is_empty() || hex.len() > 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let cp = u32::from_str_radix(hex, 16).ok()?;
        return Some((numeric_reference(cp), i + 1));
    }
    if let Some(dec) = name.strip_prefix('#') {
        // At most seven decimal digits.
        if dec.is_empty() || dec.len() > 7 || !dec.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let cp: u32 = dec.parse().ok()?;
        return Some((numeric_reference(cp), i + 1));
    }
    let found = NAMED
        .binary_search_by(|(candidate, _)| (*candidate).cmp(name.as_str()))
        .ok()
        .map(|index| NAMED[index].1)?;
    Some((found.to_string(), i + 1))
}

/// Scans a link destination starting at `i`.
fn scan_destination(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) == Some(&'<') {
        let mut j = i + 1;
        let mut raw = String::new();
        while j < chars.len() {
            match chars[j] {
                '>' => return Some((decode_text(&raw), j + 1)),
                '<' | '\n' | '\r' => return None,
                '\\' if chars.get(j + 1).is_some_and(|c| is_escapable(*c)) => {
                    raw.push('\\');
                    raw.push(chars[j + 1]);
                    j += 2;
                }
                c => {
                    raw.push(c);
                    j += 1;
                }
            }
        }
        return None;
    }
    let mut j = i;
    let mut depth = 0usize;
    let mut raw = String::new();
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' && chars.get(j + 1).is_some_and(|n| is_escapable(*n)) {
            raw.push('\\');
            raw.push(chars[j + 1]);
            j += 2;
            continue;
        }
        if matches!(c, ' ' | '\t' | '\n' | '\r') || (c as u32) < 0x20 || c == '\u{7f}' {
            break;
        }
        match c {
            '(' => {
                depth += 1;
                if depth > 32 {
                    return None;
                }
            }
            ')' if depth == 0 => break,
            ')' => depth -= 1,
            _ => {}
        }
        raw.push(c);
        j += 1;
    }
    if depth != 0 || raw.is_empty() {
        return None;
    }
    Some((decode_text(&raw), j))
}

/// Scans a link title (`"…"`, `'…'` or `(…)`).
fn scan_title(chars: &[char], i: usize) -> Option<(String, usize)> {
    let open = *chars.get(i)?;
    let close = match open {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        _ => return None,
    };
    let mut j = i + 1;
    let mut raw = String::new();
    let mut blank_line = false;
    while j < chars.len() {
        let c = chars[j];
        if c == '\\' && chars.get(j + 1).is_some_and(|n| is_escapable(*n)) {
            raw.push('\\');
            raw.push(chars[j + 1]);
            j += 2;
            continue;
        }
        if c == close {
            return Some((decode_text(&raw), j + 1));
        }
        if c == '\n' {
            // A blank line ends the paragraph and the title.
            let next = chars.get(j + 1).copied();
            if next.is_none_or(|n| n == '\n') {
                blank_line = true;
            }
        }
        if close == ')' && c == '(' {
            return None;
        }
        raw.push(c);
        j += 1;
    }
    let _ = blank_line;
    None
}

/// Decodes backslash escapes and character references in a raw string.
fn decode_text(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && chars.get(i + 1).is_some_and(|c| is_escapable(*c)) {
            out.push(chars[i + 1]);
            i += 2;
        } else if chars[i] == '&' {
            if let Some((text, next)) = scan_entity(&chars, i) {
                out.push_str(&text);
                i = next;
            } else {
                out.push('&');
                i += 1;
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Scans a reference label (`[…]`), returning the raw text and next index.
fn scan_label(chars: &[char], i: usize) -> Option<(String, usize)> {
    if chars.get(i) != Some(&'[') {
        return None;
    }
    let mut j = i + 1;
    let mut raw = String::new();
    while j < chars.len() {
        match chars[j] {
            '\\' if chars.get(j + 1).is_some_and(|c| is_escapable(*c)) => {
                raw.push('\\');
                raw.push(chars[j + 1]);
                j += 2;
            }
            '[' => return None,
            ']' => {
                if raw.chars().count() > 999 {
                    return None;
                }
                return Some((raw, j + 1));
            }
            c => {
                raw.push(c);
                j += 1;
            }
        }
    }
    None
}

/// Finds the `]` matching the `[` at `open`, honouring nesting and escapes.
fn matching_bracket(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = open;
    while i < chars.len() {
        match chars[i] {
            '\\' if chars.get(i + 1).is_some_and(|c| is_escapable(*c)) => i += 2,
            '`' => {
                // A bracket inside a code span does not count.
                let run = count_run(chars, i, '`');
                match scan_code_span(chars, i + run, run) {
                    Some((_, next)) => i = next,
                    None => i += run,
                }
            }
            '<' => {
                // Nor does one inside raw HTML or an autolink.
                if let Some((_, next)) = scan_html(chars, i) {
                    i = next;
                } else if let Some((_, _, next)) = scan_autolink(chars, i) {
                    i = next;
                } else {
                    i += 1;
                }
            }
            '[' => {
                depth += 1;
                i += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Numeric character references outside the Unicode ranges become U+FFFD.
fn numeric_reference(cp: u32) -> String {
    match char::from_u32(cp) {
        Some(c) if cp != 0 => c.to_string(),
        _ => '\u{fffd}'.to_string(),
    }
}

/// Parses `[text](dest "title")`, `[text][label]`, `[text][]` or `[text]`.
fn scan_link_like(
    chars: &[char],
    open: usize,
    image: bool,
    refs: &Refs,
    ext: Extensions,
) -> Option<(Inline, usize)> {
    let close = matching_bracket(chars, open)?;
    let label: Vec<char> = chars[open + 1..close].to_vec();
    let label_text: String = label.iter().collect();
    let after = close + 1;

    // The visible text (or alt text, for images) is parsed recursively.
    // Links inside a link make the outer link invalid: the inner one wins.
    // Images may not contain links either; the same recursion applies.
    let children = resolve(tokenize(&label, refs, true, ext));
    if !image && children.iter().any(contains_link) {
        return None;
    }

    // Inline link or image.
    if chars.get(after) == Some(&'(')
        && let Some((url, title, end)) = scan_inline_target(chars, after)
    {
        return Some((make_link(image, children, url, title, &label_text), end));
    }

    // Reference forms.
    let reference: String;
    let mut end = after;
    if chars.get(after) == Some(&'[') {
        // Full or collapsed reference; a failed lookup makes the whole
        // construct literal (the following bracket is parsed separately).
        let (raw, next) = scan_label(chars, after)?;
        reference = if raw.trim().is_empty() {
            label_text.clone()
        } else {
            raw
        };
        end = next;
    } else {
        // Shortcut reference.
        reference = label_text.clone();
    }
    let definition = refs.get(&normalize_label(&reference))?;
    Some((
        make_link(
            image,
            children,
            definition.url.clone(),
            definition.title.clone(),
            &label_text,
        ),
        end,
    ))
}

fn make_link(
    image: bool,
    children: Vec<Inline>,
    url: String,
    title: Option<String>,
    label_text: &str,
) -> Inline {
    if image {
        let mut alt = plain_text_of(&children);
        if alt.is_empty() {
            alt = label_text.to_string();
        }
        let _ = title;
        Inline::Image(ImageData { alt, url, title })
    } else {
        Inline::Link {
            text: children,
            url,
            title,
        }
    }
}

fn contains_link(inline: &Inline) -> bool {
    match inline {
        Inline::Link { .. } => true,
        Inline::Emph(inner) | Inline::Strong(inner) | Inline::Strike(inner) => {
            inner.iter().any(contains_link)
        }
        _ => false,
    }
}

/// Parses `(dest "title")` starting at the opening parenthesis.
fn scan_inline_target(chars: &[char], open: usize) -> Option<(String, Option<String>, usize)> {
    let mut j = open + 1;
    skip_spaces_and_newline(chars, &mut j);
    let (url, after_url) = if chars.get(j) == Some(&')') {
        (String::new(), j)
    } else {
        scan_destination(chars, j)?
    };
    j = after_url;
    let before_title = j;
    skip_spaces_and_newline(chars, &mut j);
    let mut title = None;
    if j > before_title
        && let Some((t, next)) = scan_title(chars, j)
    {
        title = Some(t);
        j = next;
        skip_spaces_and_newline(chars, &mut j);
    }
    if chars.get(j) != Some(&')') {
        return None;
    }
    Some((url, title, j + 1))
}

/// Skips spaces and tabs plus at most one line ending.
fn skip_spaces_and_newline(chars: &[char], i: &mut usize) {
    let mut newline = false;
    while let Some(&c) = chars.get(*i) {
        if c == ' ' || c == '\t' {
            *i += 1;
        } else if (c == '\n' || c == '\r') && !newline {
            newline = true;
            *i += 1;
        } else {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// Emphasis resolution
// ---------------------------------------------------------------------------

/// One delimiter run, with the index of its literal marker node in `out`.
struct Delim {
    ch: char,
    count: usize,
    can_open: bool,
    can_close: bool,
    orig: usize,
    node: usize,
}

fn resolve(tokens: Vec<Tok>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(tokens.len());
    let mut delims: Vec<Delim> = Vec::new();
    for token in tokens {
        match token {
            // Each token keeps its own node: delimiter indices must stay
            // exact until emphasis has been resolved.
            Tok::Text(text) => {
                if !text.is_empty() {
                    out.push(Inline::Text(text));
                }
            }
            Tok::Node(node) => out.push(node),
            Tok::Delim {
                ch,
                count,
                can_open,
                can_close,
                orig,
            } => {
                let node = out.len();
                out.push(Inline::Text(ch.to_string().repeat(count)));
                delims.push(Delim {
                    ch,
                    count,
                    can_open,
                    can_close,
                    orig,
                    node,
                });
            }
        }
    }

    // The specification's "process emphasis" loop, without the
    // openers_bottom optimisation (it only skips delimiters that cannot
    // match, so results are identical).
    let mut closer = 0usize;
    while closer < delims.len() {
        if !delims[closer].can_close || delims[closer].count == 0 {
            closer += 1;
            continue;
        }
        let ch = delims[closer].ch;
        let closer_count = delims[closer].count;
        let closer_orig = delims[closer].orig;
        let closer_can_open = delims[closer].can_open;

        let mut opener: Option<usize> = None;
        let mut probe = closer;
        while probe > 0 {
            probe -= 1;
            let candidate = &delims[probe];
            if candidate.ch != ch || !candidate.can_open || candidate.count == 0 {
                continue;
            }
            // "Multiple of 3" rule.
            if (candidate.can_close || closer_can_open)
                && (candidate.orig + closer_orig).is_multiple_of(3)
                && !(candidate.orig.is_multiple_of(3) && closer_orig.is_multiple_of(3))
            {
                continue;
            }
            opener = Some(probe);
            break;
        }

        let Some(opener) = opener else {
            if !closer_can_open {
                // This run can never match anything: drop it.
                delims.remove(closer);
            } else {
                closer += 1;
            }
            continue;
        };

        let use_count = if delims[opener].count >= 2 && closer_count >= 2 {
            2
        } else {
            1
        };
        let opener_node = delims[opener].node;
        let closer_node = delims[closer].node;
        let children: Vec<Inline> = out.drain(opener_node + 1..closer_node).collect();
        let child_count = children.len();
        delims[opener].count -= use_count;
        delims[closer].count -= use_count;
        let opener_left = delims[opener].count;
        let closer_left = delims[closer].count;

        let wrapper = if ch == '~' {
            Inline::Strike(children)
        } else if use_count == 2 {
            Inline::Strong(children)
        } else {
            Inline::Emph(children)
        };

        let mut replacement: Vec<Inline> = Vec::new();
        if opener_left > 0 {
            replacement.push(Inline::Text(ch.to_string().repeat(opener_left)));
        }
        replacement.push(wrapper);
        if closer_left > 0 {
            replacement.push(Inline::Text(ch.to_string().repeat(closer_left)));
        }
        // The drain above already removed everything between the markers,
        // so the opener and closer are now adjacent.
        let new_span = replacement.len();
        out.splice(opener_node..=opener_node + 1, replacement);

        // Rebuild the delimiter list: drop the ones that were wrapped, keep
        // the surviving markers, and shift the nodes that moved.
        let mut rebuilt: Vec<Delim> = Vec::with_capacity(delims.len());
        let opener_pos = if opener_left > 0 {
            Some(opener_node)
        } else {
            None
        };
        let closer_pos = if closer_left > 0 {
            Some(opener_node + 1 + usize::from(opener_left > 0))
        } else {
            None
        };
        // `child_count` nodes were drained and the two markers replaced by
        // `new_span` nodes: later nodes move by this much.
        let shift = new_span as isize - (child_count as isize + 2);
        for (index, mut delim) in std::mem::take(&mut delims).into_iter().enumerate() {
            if index == opener {
                if let Some(pos) = opener_pos {
                    delim.node = pos;
                    rebuilt.push(delim);
                }
            } else if index == closer {
                if let Some(pos) = closer_pos {
                    delim.node = pos;
                    rebuilt.push(delim);
                }
            } else if index > opener && index < closer {
                // Inside the wrapped range: now literal text.
            } else {
                if index > closer {
                    delim.node = (delim.node as isize + shift) as usize;
                }
                rebuilt.push(delim);
            }
        }
        delims = rebuilt;
        closer = opener + usize::from(opener_left > 0);
    }

    merge_text(out)
}

fn push_text(out: &mut Vec<Inline>, text: String) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(prev)) = out.last_mut() {
        prev.push_str(&text);
    } else {
        out.push(Inline::Text(text));
    }
}

/// Merges adjacent text nodes and drops empty ones.
fn merge_text(inlines: Vec<Inline>) -> Vec<Inline> {
    let mut out: Vec<Inline> = Vec::with_capacity(inlines.len());
    for inline in inlines {
        match inline {
            Inline::Text(s) if s.is_empty() => {}
            Inline::Text(s) => push_text(&mut out, s),
            Inline::Emph(inner) => {
                let inner = merge_text(inner);
                if !inner.is_empty() {
                    out.push(Inline::Emph(inner));
                }
            }
            Inline::Strong(inner) => {
                let inner = merge_text(inner);
                if !inner.is_empty() {
                    out.push(Inline::Strong(inner));
                }
            }
            Inline::Strike(inner) => {
                let inner = merge_text(inner);
                if !inner.is_empty() {
                    out.push(Inline::Strike(inner));
                }
            }
            Inline::Link { text, url, title } => {
                let text = merge_text(text);
                out.push(Inline::Link { text, url, title });
            }
            other => out.push(other),
        }
    }
    out
}

/// Decodes escapes and entities in text coming from a definition.
pub(crate) fn decode_text_for_definitions(raw: &str) -> String {
    decode_text(raw)
}

// ---------------------------------------------------------------------------
// GFM: autolink literals and the HTML tag filter
// ---------------------------------------------------------------------------

/// Whether a URL literal (`www.`, `http://`, …) starts here.
fn is_autolink_literal_start(chars: &[char], i: usize) -> bool {
    if !literal_boundary(chars, i) {
        return false;
    }
    let rest: String = chars[i..].iter().collect();
    rest.starts_with("www.")
        || rest.starts_with("http://")
        || rest.starts_with("https://")
        || rest.starts_with("ftp://")
}

/// Whether an email literal starts here.
fn is_email_literal_start(chars: &[char], i: usize) -> bool {
    literal_boundary(chars, i) && chars[i].is_ascii_alphanumeric()
}

/// Autolinks may not start in the middle of a word or URL.
fn literal_boundary(chars: &[char], i: usize) -> bool {
    match i.checked_sub(1).map(|p| chars[p]) {
        Some(prev) => !(prev.is_alphanumeric() || matches!(prev, '-' | '_' | '/')),
        None => true,
    }
}

/// Scans a GFM autolink literal at `i`.
fn scan_autolink_literal(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let rest: String = chars[i..].iter().collect();
    let (prefix, scheme) = if rest.starts_with("www.") {
        ("www.", "http://")
    } else if rest.starts_with("https://") {
        ("https://", "")
    } else if rest.starts_with("http://") {
        ("http://", "")
    } else if rest.starts_with("ftp://") {
        ("ftp://", "")
    } else {
        return scan_email_literal(chars, i);
    };
    let mut end = i + prefix.chars().count();
    while end < chars.len() && !chars[end].is_whitespace() && chars[end] != '<' {
        end += 1;
    }
    let text = trim_url_end(&chars[i..end].iter().collect::<String>());
    if text.len() <= prefix.len() {
        return None;
    }
    let host = text[prefix.len()..]
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    if host.is_empty() {
        return None;
    }
    if prefix == "www." && (!host.contains('.') || !host.chars().any(|c| c.is_ascii_alphabetic())) {
        return None;
    }
    let end = i + text.chars().count();
    Some((text.clone(), format!("{scheme}{text}"), end))
}

/// Scans an email literal at `i`.
fn scan_email_literal(chars: &[char], i: usize) -> Option<(String, String, usize)> {
    let mut end = i;
    while end < chars.len()
        && (chars[end].is_ascii_alphanumeric() || matches!(chars[end], '.' | '_' | '+' | '-'))
    {
        end += 1;
    }
    if chars.get(end) != Some(&'@') {
        return None;
    }
    let local: String = chars[i..end].iter().collect();
    if local.is_empty() || local.starts_with('.') || local.ends_with('.') {
        return None;
    }
    end += 1;
    let domain_start = end;
    while end < chars.len()
        && (chars[end].is_ascii_alphanumeric() || matches!(chars[end], '.' | '-'))
    {
        end += 1;
    }
    let mut domain: String = chars[domain_start..end].iter().collect();
    // A trailing dot is sentence punctuation, not part of the domain.
    while domain.ends_with('.') {
        domain.pop();
        end -= 1;
    }
    if chars
        .get(end)
        .is_some_and(|c| *c == '-' || *c == '_' || c.is_alphanumeric())
    {
        return None;
    }
    let valid = !domain.is_empty()
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
        });
    if !valid {
        return None;
    }
    let text = format!("{local}@{domain}");
    Some((text.clone(), format!("mailto:{text}"), end))
}

/// Trims the punctuation GFM allows to follow an autolink.
fn trim_url_end(url: &str) -> String {
    let mut text = url.to_string();
    while let Some(last) = text.chars().last() {
        // An entity-like tail ("&hl;") is not part of the link, and the
        // semicolon must be handled before the generic punctuation trim.
        if last == ';'
            && let Some(amp) = text.rfind('&')
            && text[amp + 1..text.len() - 1]
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
        {
            text.truncate(amp);
            continue;
        }
        let trim = match last {
            '?' | '!' | '.' | ',' | ':' | ';' | '"' | '\'' | '*' | '_' | '~' => true,
            ')' => {
                text.chars().filter(|c| *c == ')').count()
                    > text.chars().filter(|c| *c == '(').count()
            }
            ']' => {
                text.chars().filter(|c| *c == ']').count()
                    > text.chars().filter(|c| *c == '[').count()
            }
            _ => false,
        };
        if trim {
            text.pop();
            continue;
        }
        // An entity-like tail ("&hl;") is not part of the link.
        if last == ';'
            && let Some(amp) = text.rfind('&')
            && text[amp + 1..text.len() - 1]
                .chars()
                .all(|c| c.is_ascii_alphanumeric())
        {
            text.truncate(amp);
            continue;
        }
        break;
    }
    text
}
