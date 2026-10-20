//! Markdown parsing: source text to [`Document`].
//!
//! The parser is a pragmatic CommonMark subset plus GFM extensions
//! (tables, strikethrough). It is line-based and recursive: block
//! constructs (quotes, list items) are stripped of their markers and
//! re-parsed, which keeps every level of nesting on the same code path.
//!
//! Parsing is infallible by design — anything unrecognized degrades into
//! paragraph text, exactly like browsers tolerate broken HTML.

mod block;
mod inline;

use crate::ast::Document;

/// Parses Markdown source into a [`Document`].
///
/// ```
/// use markdown2svg::parse;
///
/// let doc = parse("# Hello\n\nworld");
/// assert_eq!(doc.block_counts().headings, 1);
/// assert_eq!(doc.block_counts().paragraphs, 1);
/// ```
pub fn parse(src: &str) -> Document {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<String> = normalized.split('\n').map(expand_tabs).collect();
    Document {
        blocks: block::parse_blocks(&lines),
    }
}

/// Expands tabs to spaces at 4-column boundaries (CommonMark-style).
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            let spaces = 4 - col % 4;
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_and_cr_are_normalized() {
        let doc = parse("a\r\nb\rc");
        // All one paragraph with soft breaks; importantly, no panics and
        // the content survives.
        assert_eq!(doc.block_counts().paragraphs, 1);
    }

    #[test]
    fn tabs_expand_to_boundaries() {
        let doc = parse("#\tHeading");
        assert_eq!(doc.block_counts().headings, 1);
    }

    #[test]
    fn empty_input_yields_empty_document() {
        assert!(parse("").blocks.is_empty());
        assert!(parse("\n\n\n").blocks.is_empty());
    }

    #[test]
    fn expand_tabs_hits_four_column_boundaries() {
        assert_eq!(expand_tabs("a\tb"), "a   b");
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }
}
