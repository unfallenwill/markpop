//! Markdown parsing: source text to [`Document`].
//!
//! Block parsing is line-based and recursive: containers (quotes, list
//! items) strip their markers and re-parse their content, which keeps every
//! nesting level on the same code path. Inline parsing happens after the
//! whole document is read, because link reference definitions are resolved
//! document-wide.
//!
//! Parsing is infallible by design — anything unrecognized degrades into
//! paragraph text, exactly like browsers tolerate broken HTML.

mod block;
mod entities;
mod inline;

use crate::ast::Document;

/// Syntax extensions layered on top of CommonMark.
///
/// The default output (and the CLI) targets GitHub Flavored Markdown, which
/// is CommonMark plus these extensions. Pure CommonMark is still available
/// for conformance testing and for callers that want the strict dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extensions {
    /// GFM autolink literals (`www.`, `http://`, bare emails).
    pub autolinks: bool,
    /// GFM task list items (`- [ ]`, `- [x]`).
    pub task_lists: bool,
}

impl Extensions {
    /// Strict CommonMark: no GFM extensions.
    pub const fn commonmark() -> Extensions {
        Extensions {
            autolinks: false,
            task_lists: false,
        }
    }

    /// GitHub Flavored Markdown: every extension enabled.
    pub const fn gfm() -> Extensions {
        Extensions {
            autolinks: true,
            task_lists: true,
        }
    }
}

impl Default for Extensions {
    fn default() -> Extensions {
        Extensions::gfm()
    }
}

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
    parse_with(src, Extensions::gfm())
}

/// Parses strict CommonMark, without the GFM extensions.
pub fn parse_commonmark(src: &str) -> Document {
    parse_with(src, Extensions::commonmark())
}

/// Parses Markdown with the given extensions enabled.
pub fn parse_with(src: &str, extensions: Extensions) -> Document {
    let normalized = src.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(expand_tabs).collect();
    // A trailing newline ends the last line; it does not start an empty one.
    if normalized.ends_with('\n') {
        lines.pop();
    }
    // Definitions can appear anywhere, so inline parsing waits until the
    // whole document has been read.
    let mut raw = block::parse_blocks(&lines, extensions);
    let refs = block::collect_definitions(&mut raw);
    Document {
        blocks: block::finalize(raw, &refs, extensions),
    }
}

/// Expands tabs in the leading whitespace run only.
///
/// CommonMark treats a tab as advancing to the next four-column stop, but
/// only "in contexts where whitespace helps define block structure": tabs
/// inside code and text are preserved. Expanding just the indentation is
/// exactly that rule for a line-at-a-time parser.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0usize;
    let mut leading = true;
    for c in line.chars() {
        if leading && c == '\t' {
            let spaces = 4 - col % 4;
            for _ in 0..spaces {
                out.push(' ');
            }
            col += spaces;
            continue;
        }
        if leading && c == ' ' {
            out.push(' ');
            col += 1;
            continue;
        }
        leading = false;
        out.push(c);
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
        // Tabs after the first non-space character are preserved.
        assert_eq!(expand_tabs("a\tb"), "a\tb");
        assert_eq!(expand_tabs(" \tb"), "    b");
        assert_eq!(expand_tabs("    a\tb"), "    a\tb");
        assert_eq!(expand_tabs("\tx"), "    x");

        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }
}
