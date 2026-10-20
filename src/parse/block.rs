//! Block-level parsing: a flat list of lines to a tree of [`Block`]s.
//!
//! The parser is a cursor ([`BlockParser`]) over lines. At each position
//! it tries block starts in a fixed order; paragraph gathering then
//! applies the "what can interrupt a paragraph" rules. Container blocks
//! (quotes, list items) recursively call [`parse_blocks`] on their
//! stripped content lines.

use super::inline::parse_inlines;
use crate::ast::{Align, Block, CodeBlock, ImageData, Inline, ListData, ListItem, TableData};

/// Parses a sequence of lines into blocks.
pub(crate) fn parse_blocks(lines: &[String]) -> Vec<Block> {
    BlockParser { lines, pos: 0 }.run()
}

/// Cursor over the lines of one nesting level.
struct BlockParser<'a> {
    lines: &'a [String],
    pos: usize,
}

impl<'a> BlockParser<'a> {
    /// The current line, if any. The borrow is tied to the borrowed slice,
    /// not to `self`, so callers may advance the cursor while holding it.
    fn peek(&self) -> Option<&'a str> {
        self.lines.get(self.pos).map(String::as_str)
    }

    fn run(&mut self) -> Vec<Block> {
        let mut blocks = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) {
                self.pos += 1;
                continue;
            }
            let block = if is_thematic_break(line) {
                self.pos += 1;
                Some(Block::ThematicBreak)
            } else if let Some((level, content)) = atx_heading(line) {
                self.pos += 1;
                Some(Block::Heading {
                    level,
                    inlines: parse_inlines(&content),
                })
            } else if let Some((fc, flen, info)) = fence_start(line) {
                Some(self.take_fenced(fc, flen, info))
            } else if quote_start(line) {
                Some(self.take_quote())
            } else if is_list_start(line) {
                Some(self.take_list())
            } else if let Some(table) = self.try_table() {
                Some(table)
            } else if indent_of(line) >= 4 {
                Some(self.take_indented_code())
            } else {
                self.take_paragraph()
            };
            if let Some(b) = block {
                blocks.push(b);
            }
        }
        blocks
    }

    /// Consumes a fenced code block. The current line is the opening fence.
    fn take_fenced(&mut self, fc: char, flen: usize, info: String) -> Block {
        let open_indent = indent_of(self.peek().unwrap());
        self.pos += 1;
        let mut lines = Vec::new();
        while let Some(line) = self.peek() {
            if is_fence_end(line, fc, flen) {
                self.pos += 1;
                break;
            }
            lines.push(strip_indent(line, open_indent).to_string());
            self.pos += 1;
        }
        Block::Code(CodeBlock { info, lines })
    }

    /// Consumes a blockquote, stripping `>` markers and accepting lazy
    /// paragraph continuation lines.
    fn take_quote(&mut self) -> Block {
        let mut content: Vec<String> = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) {
                break;
            }
            if quote_start(line) {
                content.push(strip_quote_marker(line));
                self.pos += 1;
            } else if !content.is_empty() && !interrupts_paragraph(line) {
                // Lazy continuation: a paragraph line that forgot the `>`.
                content.push(line.to_string());
                self.pos += 1;
            } else {
                break;
            }
        }
        Block::Quote(parse_blocks(&content))
    }

    /// Consumes an entire list (all consecutive items of the same type).
    fn take_list(&mut self) -> Block {
        let first = list_marker(self.peek().unwrap()).expect("caller verified list start");
        let (ordered, delim, base_indent) = (first.ordered, first.ch, first.indent);
        let mut items: Vec<ListItem> = Vec::new();
        let mut tight = true;
        loop {
            let (item, internal_blank) = self.take_item().expect("caller verified list start");
            if internal_blank {
                tight = false;
            }
            items.push(item);

            let mut blank_between = false;
            while self.peek().is_some_and(is_blank) {
                self.pos += 1;
                blank_between = true;
            }
            match self.peek() {
                Some(line) if same_list(line, ordered, delim, base_indent) => {
                    if blank_between {
                        tight = false;
                    }
                }
                _ => break,
            }
        }
        let start = if ordered { first.number.max(1) } else { 1 };
        Block::List(ListData {
            ordered,
            start,
            tight,
            items,
        })
    }

    /// Consumes one list item. Returns the item and whether a blank line
    /// fell inside it (which makes the surrounding list loose).
    fn take_item(&mut self) -> Option<(ListItem, bool)> {
        let line = self.peek()?;
        let m = list_marker(line)?;
        let after = &line[m.indent + m.len..];
        let after_spaces = leading_spaces(after);

        let mut content: Vec<String> = Vec::new();
        let content_indent;
        if after.is_empty() {
            // Blank line right after the marker: content (if any) is
            // indented one column past the marker.
            content_indent = m.indent + m.len + 1;
            self.pos += 1;
        } else if after_spaces >= 5 {
            // Five or more spaces: the item starts with indented code.
            content_indent = m.indent + m.len + 1;
            content.push(format!(
                "{}{}",
                " ".repeat(after_spaces - 1),
                after.trim_start()
            ));
            self.pos += 1;
        } else if after_spaces >= 1 {
            content_indent = m.indent + m.len + after_spaces;
            content.push(after[after_spaces..].to_string());
            self.pos += 1;
        } else {
            // No space after the marker: not a list item at all.
            return None;
        }

        let mut internal_blank = false;
        while let Some(line) = self.peek() {
            if is_blank(line) {
                // Blank lines belong to this item only if the next
                // non-blank line is indented to the content column.
                let mut j = self.pos + 1;
                while j < self.lines.len() && is_blank(&self.lines[j]) {
                    j += 1;
                }
                if j < self.lines.len() && indent_of(&self.lines[j]) >= content_indent {
                    for _ in self.pos..j {
                        content.push(String::new());
                    }
                    internal_blank = true;
                    self.pos = j;
                } else {
                    break;
                }
            } else if indent_of(line) >= content_indent {
                content.push(strip_indent(line, content_indent).to_string());
                self.pos += 1;
            } else if interrupts_paragraph(line) {
                break;
            } else if content.last().is_some_and(|l| !is_blank(l)) {
                // Lazy paragraph continuation into the item.
                content.push(line.to_string());
                self.pos += 1;
            } else {
                break;
            }
        }
        Some((
            ListItem {
                blocks: parse_blocks(&content),
            },
            internal_blank,
        ))
    }

    /// Consumes an indented (4-space) code block.
    fn take_indented_code(&mut self) -> Block {
        let mut lines: Vec<String> = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) {
                let mut j = self.pos + 1;
                while j < self.lines.len() && is_blank(&self.lines[j]) {
                    j += 1;
                }
                if j < self.lines.len() && indent_of(&self.lines[j]) >= 4 {
                    for _ in self.pos..j {
                        lines.push(String::new());
                    }
                    self.pos = j;
                } else {
                    break;
                }
            } else if indent_of(line) >= 4 {
                lines.push(strip_indent(line, 4).to_string());
                self.pos += 1;
            } else {
                break;
            }
        }
        while lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.pop();
        }
        Block::Code(CodeBlock {
            info: String::new(),
            lines,
        })
    }

    /// Attempts to parse a GFM table at the current position.
    fn try_table(&mut self) -> Option<Block> {
        let header_line = self.peek()?;
        if !header_line.contains('|') {
            return None;
        }
        let sep_line = self.lines.get(self.pos + 1)?;
        let aligns = parse_delimiter_row(sep_line)?;
        let head = split_row(header_line);
        if head.len() != aligns.len() {
            return None;
        }

        self.pos += 2;
        let mut rows: Vec<Vec<String>> = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) || interrupts_paragraph(line) {
                break;
            }
            rows.push(split_row(line));
            self.pos += 1;
        }

        let ncols = aligns.len();
        let head_cells: Vec<Vec<Inline>> = head.into_iter().map(|c| parse_inlines(&c)).collect();
        let body_cells: Vec<Vec<Vec<Inline>>> = rows
            .into_iter()
            .map(|row| {
                let mut cells: Vec<Vec<Inline>> =
                    row.into_iter().map(|c| parse_inlines(&c)).collect();
                cells.resize(ncols, Vec::new());
                cells
            })
            .collect();

        Some(Block::Table(TableData {
            aligns,
            head: head_cells,
            rows: body_cells,
        }))
    }

    /// Consumes a paragraph, watching for Setext underlines.
    fn take_paragraph(&mut self) -> Option<Block> {
        let mut raw: Vec<&'a str> = Vec::new();
        let mut setext_level = 0u8;
        while let Some(line) = self.peek() {
            if !raw.is_empty()
                && let Some(level) = is_setext_underline(line)
            {
                setext_level = level;
                self.pos += 1;
                break;
            }
            if is_blank(line) || interrupts_paragraph(line) {
                break;
            }
            raw.push(line);
            self.pos += 1;
        }
        if raw.is_empty() {
            return None;
        }
        let joined = raw.join("\n");
        if setext_level > 0 {
            return Some(Block::Heading {
                level: setext_level,
                inlines: parse_inlines(&joined),
            });
        }
        let inlines = parse_inlines(&joined);
        let block = if inlines.len() == 1 && matches!(inlines[0], Inline::Image(_)) {
            // A paragraph containing only an image is promoted to a figure.
            match inlines.into_iter().next() {
                Some(Inline::Image(ImageData { alt, url })) => {
                    Block::Figure(ImageData { alt, url })
                }
                _ => unreachable!("checked above"),
            }
        } else {
            Block::Paragraph(inlines)
        };
        Some(block)
    }
}

// ---------------------------------------------------------------------------
// Line predicates and helpers
// ---------------------------------------------------------------------------

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// Number of leading spaces (tabs are pre-expanded).
fn indent_of(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ').count()
}

fn leading_spaces(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ').count()
}

/// Removes up to `n` leading spaces.
fn strip_indent(s: &str, n: usize) -> &str {
    let have = indent_of(s).min(n);
    &s[have..]
}

/// `---`, `***`, `___` (3+ of one char, spaces allowed between).
fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    let Some(c @ ('-' | '_' | '*')) = t.chars().next() else {
        return false;
    };
    let compact: String = t.chars().filter(|ch| !ch.is_whitespace()).collect();
    compact.len() >= 3 && compact.chars().all(|ch| ch == c)
}

/// A run of `=` or `-` (with ≤3 leading spaces) that can underline a
/// paragraph as a Setext heading. Returns the heading level.
fn is_setext_underline(line: &str) -> Option<u8> {
    if indent_of(line) > 3 {
        return None;
    }
    let t = line.trim();
    let first = t.chars().next()?;
    if first != '=' && first != '-' {
        return None;
    }
    if t.chars().all(|c| c == first) {
        Some(if first == '=' { 1 } else { 2 })
    } else {
        None
    }
}

/// `#`-prefixed heading; returns (level, content) with closing hashes
/// stripped.
fn atx_heading(line: &str) -> Option<(u8, String)> {
    if indent_of(line) > 3 {
        return None;
    }
    let s = line.trim_start();
    let hashes = s.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &s[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    let mut content = rest.trim().to_string();
    // Strip an optional closing run of `#` (must be preceded by whitespace
    // or be the entire content).
    let without_hashes = content.trim_end_matches('#');
    if without_hashes.is_empty() || without_hashes.ends_with(|c: char| c.is_whitespace()) {
        content = without_hashes.trim_end().to_string();
    }
    Some((hashes as u8, content))
}

/// Opening code fence; returns (fence char, fence length, info string).
fn fence_start(line: &str) -> Option<(char, usize, String)> {
    if indent_of(line) > 3 {
        return None;
    }
    let s = line.trim_start();
    let c = s.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = s.chars().take_while(|ch| *ch == c).count();
    if n < 3 {
        return None;
    }
    let info = s[n..].trim().to_string();
    // Backtick fences may not carry backticks in their info string.
    if c == '`' && info.contains('`') {
        return None;
    }
    Some((c, n, info))
}

/// Whether `line` closes a fence of char `c` opened with length `>= min`.
fn is_fence_end(line: &str, c: char, min: usize) -> bool {
    if indent_of(line) > 3 {
        return false;
    }
    let t = line.trim();
    let n = t.chars().take_while(|ch| *ch == c).count();
    n >= min && n == t.chars().count()
}

/// Whether `line` opens a blockquote.
fn quote_start(line: &str) -> bool {
    indent_of(line) <= 3 && line[indent_of(line)..].starts_with('>')
}

/// Strips the `>` marker (and one following space) from a quote line.
fn strip_quote_marker(line: &str) -> String {
    let indent = indent_of(line);
    let mut s = &line[indent + 1..];
    if s.starts_with(' ') {
        s = &s[1..];
    }
    s.to_string()
}

/// A parsed list-item marker.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Marker {
    ordered: bool,
    /// Bullet char, or the ordered delimiter (`.` or `)`).
    ch: char,
    /// First number for ordered lists.
    number: u32,
    /// Total marker length in chars (indent excluded).
    len: usize,
    /// Marker indent in columns.
    indent: usize,
}

fn list_marker(line: &str) -> Option<Marker> {
    let indent = indent_of(line);
    if indent > 3 {
        return None;
    }
    let s = &line[indent..];
    let first = s.chars().next()?;
    let (ordered, ch, len, number) = match first {
        '-' | '+' | '*' => (false, first, 1, 0),
        '0'..='9' => {
            let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() > 9 {
                return None;
            }
            let delim = s[digits.len()..].chars().next()?;
            if delim != '.' && delim != ')' {
                return None;
            }
            (true, delim, digits.len() + 1, digits.parse().ok()?)
        }
        _ => return None,
    };
    Some(Marker {
        ordered,
        ch,
        number,
        len,
        indent,
    })
}

/// Whether the line can start a list item (marker followed by a space or
/// end-of-line).
fn is_list_start(line: &str) -> bool {
    match list_marker(line) {
        Some(m) => {
            let after = &line[m.indent + m.len..];
            after.is_empty() || after.starts_with(' ')
        }
        None => false,
    }
}

/// Whether the line continues the same list: same type, same delimiter
/// char, and the same marker indent.
fn same_list(line: &str, ordered: bool, delim: char, base_indent: usize) -> bool {
    match list_marker(line) {
        Some(m) if m.ordered == ordered && m.ch == delim && m.indent == base_indent => {
            let after = &line[m.indent + m.len..];
            after.is_empty() || after.starts_with(' ')
        }
        _ => false,
    }
}

/// Whether a line ends an open paragraph. Setext underlines do *not*
/// interrupt (they transform the paragraph); indented lines do not
/// interrupt (they continue it).
fn interrupts_paragraph(line: &str) -> bool {
    if is_blank(line) {
        return true;
    }
    if indent_of(line) >= 4 {
        return false;
    }
    if is_setext_underline(line).is_some() {
        return false;
    }
    if is_thematic_break(line) {
        return true;
    }
    if atx_heading(line).is_some() {
        return true;
    }
    if fence_start(line).is_some() {
        return true;
    }
    if quote_start(line) {
        return true;
    }
    if let Some(m) = list_marker(line) {
        let after = &line[m.indent + m.len..];
        // Only non-empty items can interrupt a paragraph.
        return after.starts_with(' ') && !after.trim().is_empty();
    }
    false
}

/// Parses a table delimiter row into per-column alignments.
fn parse_delimiter_row(line: &str) -> Option<Vec<Align>> {
    if indent_of(line) > 3 {
        return None;
    }
    let t = line.trim();
    let inner = t.strip_prefix('|').unwrap_or(t);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    if inner.trim().is_empty() {
        return None;
    }
    let mut aligns = Vec::new();
    for cell in inner.split('|') {
        let c = cell.trim();
        if c.is_empty() {
            return None;
        }
        let left = c.starts_with(':');
        let right = c.ends_with(':');
        let dashes = &c[usize::from(left)..c.len() - usize::from(right)];
        if dashes.is_empty() || !dashes.chars().all(|d| d == '-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            (true, false) => Align::Left,
            (false, false) => Align::None,
        });
    }
    Some(aligns)
}

/// Splits a table row into cells on unescaped `|`, dropping the cells
/// produced by leading/trailing pipes.
fn split_row(line: &str) -> Vec<String> {
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cur.push('|');
                chars.next();
            }
            '|' => {
                cells.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    cells.push(cur.trim().to_string());
    if cells.first().is_some_and(|c| c.trim().is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| c.trim().is_empty()) {
        cells.pop();
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Block;

    /// Parses source and returns its blocks (top level only).
    fn blocks(src: &str) -> Vec<Block> {
        let lines: Vec<String> = src.split('\n').map(str::to_string).collect();
        parse_blocks(&lines)
    }

    #[test]
    fn atx_headings_levels_and_trailing_hashes() {
        let bs = blocks("# one\n## two\n###### six\n####### seven");
        assert_eq!(bs.len(), 4);
        for (b, (level, text)) in bs[..3].iter().zip([(1u8, "one"), (2, "two"), (6, "six")]) {
            assert_eq!(
                b,
                &Block::Heading {
                    level,
                    inlines: vec![Inline::Text(text.into())]
                }
            );
        }
        // 7 hashes is not a heading.
        assert!(matches!(&bs[3], Block::Paragraph(_)));
    }

    #[test]
    fn atx_closing_hash_run_is_stripped() {
        let bs = blocks("# Heading ##");
        assert_eq!(
            bs[0],
            Block::Heading {
                level: 1,
                inlines: vec![Inline::Text("Heading".into())]
            }
        );
    }

    #[test]
    fn hash_without_space_is_paragraph() {
        assert!(matches!(blocks("#hashtag")[0], Block::Paragraph(_)));
    }

    #[test]
    fn setext_headings() {
        let bs = blocks("Title\n=====\n\nSub\n---");
        assert_eq!(
            bs[0],
            Block::Heading {
                level: 1,
                inlines: vec![Inline::Text("Title".into())]
            }
        );
        assert_eq!(
            bs[1],
            Block::Heading {
                level: 2,
                inlines: vec![Inline::Text("Sub".into())]
            }
        );
    }

    #[test]
    fn thematic_break_variants() {
        for src in ["---", "***", "___", "- - -", "  ***  ", "----------"] {
            assert_eq!(
                blocks(src).as_slice(),
                &[Block::ThematicBreak],
                "failed: {src}"
            );
        }
        // Not breaks:
        for src in ["--", "**", "_ _", "- a", "*em*", "--x--"] {
            assert!(
                !matches!(blocks(src)[0], Block::ThematicBreak),
                "false positive: {src}"
            );
        }
    }

    #[test]
    fn fenced_code_with_info_and_close() {
        let bs = blocks("```rust\nfn main() {}\nlet x = 1;\n```\nafter");
        match &bs[0] {
            Block::Code(c) => {
                assert_eq!(c.info, "rust");
                assert_eq!(c.lines, ["fn main() {}", "let x = 1;"]);
            }
            other => panic!("expected code, got {other:?}"),
        }
        assert!(matches!(&bs[1], Block::Paragraph(_)));
    }

    #[test]
    fn fenced_code_unclosed_runs_to_end() {
        let bs = blocks("~~~\nline");
        match &bs[0] {
            Block::Code(c) => assert_eq!(c.lines, ["line"]),
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn fence_info_with_backtick_is_not_a_fence() {
        assert!(matches!(blocks("``` bad`info\n")[0], Block::Paragraph(_)));
    }

    #[test]
    fn indented_code_block() {
        let bs = blocks("    code line 1\n    code line 2\n\npara");
        match &bs[0] {
            Block::Code(c) => assert_eq!(c.lines, ["code line 1", "code line 2"]),
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn indented_code_does_not_interrupt_paragraph() {
        let bs = blocks("para\n    still para");
        assert_eq!(bs.len(), 1);
        assert!(matches!(&bs[0], Block::Paragraph(_)));
    }

    #[test]
    fn blockquote_strips_markers_and_recurses() {
        let bs = blocks("> quoted\n> more");
        match &bs[0] {
            Block::Quote(inner) => {
                assert_eq!(inner.len(), 1);
                assert!(matches!(&inner[0], Block::Paragraph(_)));
            }
            other => panic!("expected quote, got {other:?}"),
        }
    }

    #[test]
    fn nested_blockquotes() {
        let bs = blocks("> > deep");
        match &bs[0] {
            Block::Quote(inner) => {
                assert!(
                    matches!(&inner[0], Block::Quote(_)),
                    "expected nested quote"
                );
            }
            other => panic!("expected quote, got {other:?}"),
        }
    }

    #[test]
    fn blockquote_lazy_continuation() {
        let bs = blocks("> quoted\nlazy");
        match &bs[0] {
            Block::Quote(inner) => assert_eq!(inner.len(), 1),
            other => panic!("expected quote, got {other:?}"),
        }
    }

    #[test]
    fn blockquote_containing_other_blocks() {
        let bs = blocks("> # head\n>\n> - item");
        match &bs[0] {
            Block::Quote(inner) => {
                assert!(matches!(&inner[0], Block::Heading { .. }));
                assert!(matches!(&inner[1], Block::List(_)));
            }
            other => panic!("expected quote, got {other:?}"),
        }
    }

    #[test]
    fn simple_bullet_list_is_tight() {
        let bs = blocks("- a\n- b\n- c");
        match &bs[0] {
            Block::List(l) => {
                assert!(!l.ordered);
                assert!(l.tight);
                assert_eq!(l.items.len(), 3);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn ordered_list_with_start() {
        let bs = blocks("3. three\n4. four");
        match &bs[0] {
            Block::List(l) => {
                assert!(l.ordered);
                assert_eq!(l.start, 3);
                assert_eq!(l.items.len(), 2);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn blank_line_between_items_makes_list_loose() {
        let bs = blocks("- a\n\n- b");
        match &bs[0] {
            Block::List(l) => assert!(!l.tight),
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn blank_line_inside_item_makes_list_loose() {
        let bs = blocks("- a\n\n  continued\n- b");
        match &bs[0] {
            Block::List(l) => {
                assert!(!l.tight);
                assert_eq!(l.items.len(), 2);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn nested_list_inside_item() {
        let bs = blocks("- a\n  - b");
        match &bs[0] {
            Block::List(l) => {
                assert_eq!(l.items.len(), 1);
                let item_blocks = &l.items[0].blocks;
                assert!(matches!(&item_blocks[0], Block::Paragraph(_)));
                assert!(matches!(&item_blocks[1], Block::List(_)));
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn different_bullet_char_starts_new_list() {
        let bs = blocks("- a\n* b");
        assert_eq!(bs.len(), 2);
        assert!(matches!(&bs[0], Block::List(_)));
        assert!(matches!(&bs[1], Block::List(_)));
    }

    #[test]
    fn list_item_with_indented_code() {
        let bs = blocks("-\n      code");
        match &bs[0] {
            Block::List(l) => match &l.items[0].blocks[0] {
                Block::Code(c) => assert_eq!(c.lines, ["code"]),
                other => panic!("expected code in item, got {other:?}"),
            },
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn list_item_lazy_continuation() {
        let bs = blocks("- a\nb");
        match &bs[0] {
            Block::List(l) => {
                assert_eq!(l.items.len(), 1);
                assert_eq!(l.items[0].blocks.len(), 1);
            }
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn deep_list_nesting() {
        let bs = blocks("- a\n  - b\n    - c");
        fn assert_nested(b: &Block, depth: usize) {
            match b {
                Block::List(l) => {
                    assert_eq!(l.items.len(), 1);
                    assert!(matches!(&l.items[0].blocks[0], Block::Paragraph(_)));
                    if depth == 1 {
                        assert_eq!(
                            l.items[0].blocks.len(),
                            1,
                            "innermost item has no child list"
                        );
                    } else {
                        let child = l.items[0]
                            .blocks
                            .iter()
                            .find(|b| matches!(b, Block::List(_)))
                            .unwrap_or_else(|| panic!("expected a nested list at depth {depth}"));
                        assert_nested(child, depth - 1);
                    }
                }
                other => panic!("expected list, got {other:?}"),
            }
        }
        assert_nested(&bs[0], 3);
    }

    #[test]
    fn list_interrupts_paragraph_with_content() {
        let bs = blocks("para\n- item");
        assert_eq!(bs.len(), 2);
        assert!(matches!(&bs[1], Block::List(_)));
    }

    #[test]
    fn empty_list_item_cannot_interrupt() {
        let bs = blocks("para\n-");
        assert_eq!(bs.len(), 1, "empty item must not interrupt: {bs:?}");
    }

    #[test]
    fn basic_table() {
        let bs = blocks("| a | b |\n|---|:-:|\n| 1 | 2 |");
        match &bs[0] {
            Block::Table(t) => {
                assert_eq!(t.aligns, [Align::None, Align::Center]);
                assert_eq!(t.head.len(), 2);
                assert_eq!(t.rows.len(), 1);
                assert_eq!(t.rows[0].len(), 2);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn table_without_boundary_pipes() {
        let bs = blocks("a | b\n--- | ---:\n1 | 2");
        match &bs[0] {
            Block::Table(t) => assert_eq!(t.aligns, [Align::None, Align::Right]),
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn table_column_mismatch_is_paragraph() {
        let bs = blocks("| a | b |\n| --- |\n| 1 | 2 |");
        assert!(matches!(&bs[0], Block::Paragraph(_)), "got {bs:?}");
    }

    #[test]
    fn table_escapes_pipes() {
        let bs = blocks("| a\\|b | c |\n| --- | --- |");
        match &bs[0] {
            Block::Table(t) => {
                let cell = crate::ast::plain_text_of(&t.head[0]);
                assert_eq!(cell, "a|b");
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn table_rows_are_padded_and_truncated() {
        let bs = blocks("| a | b | c |\n|---|---|---|\n| 1 |\n| 1 | 2 | 3 | 4 |");
        match &bs[0] {
            Block::Table(t) => {
                assert_eq!(t.rows.len(), 2);
                assert_eq!(t.rows[0].len(), 3);
                assert_eq!(t.rows[1].len(), 3);
            }
            other => panic!("expected table, got {other:?}"),
        }
    }

    #[test]
    fn image_only_paragraph_becomes_figure() {
        let bs = blocks("![alt text](https://example.com/x.png)");
        assert_eq!(bs.len(), 1);
        match &bs[0] {
            Block::Figure(img) => {
                assert_eq!(img.alt, "alt text");
                assert_eq!(img.url, "https://example.com/x.png");
            }
            other => panic!("expected figure, got {other:?}"),
        }
    }

    #[test]
    fn image_with_text_stays_paragraph() {
        let bs = blocks("![alt](x.png) and words");
        assert!(matches!(&bs[0], Block::Paragraph(_)));
    }

    #[test]
    fn paragraph_runs_until_blank_or_interrupt() {
        let bs = blocks("one\ntwo\n\nthree\n# head");
        assert_eq!(bs.len(), 3);
        assert!(matches!(&bs[0], Block::Paragraph(_)));
        assert!(matches!(&bs[1], Block::Paragraph(_)));
        assert!(matches!(&bs[2], Block::Heading { .. }));
    }

    #[test]
    fn asterisk_without_space_is_emphasis_not_list() {
        let bs = blocks("*em* text");
        assert!(matches!(&bs[0], Block::Paragraph(_)));
    }

    #[test]
    fn list_item_may_start_with_indented_code() {
        let bs = blocks("-     code\n      more");
        match &bs[0] {
            Block::List(l) => match &l.items[0].blocks[0] {
                Block::Code(c) => assert_eq!(c.lines, ["code", "more"]),
                other => panic!("expected code, got {other:?}"),
            },
            other => panic!("expected list, got {other:?}"),
        }
    }

    #[test]
    fn indented_code_keeps_internal_blank_lines() {
        let bs = blocks("    a\n\n\n    b");
        match &bs[0] {
            Block::Code(c) => assert_eq!(c.lines, ["a", "", "", "b"]),
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn indented_code_ignores_trailing_blank_lines() {
        let bs = blocks("    a\n\n\n");
        match &bs[0] {
            Block::Code(c) => assert_eq!(c.lines, ["a"]),
            other => panic!("expected code, got {other:?}"),
        }
    }

    #[test]
    fn a_lone_pipe_line_is_a_paragraph() {
        assert!(matches!(&blocks("a | b")[0], Block::Paragraph(_)));
    }

    #[test]
    fn a_table_row_ends_at_a_heading() {
        let bs = blocks("| a |\n|---|\n| 1 |\n# h");
        match &bs[0] {
            Block::Table(t) => assert_eq!(t.rows.len(), 1),
            other => panic!("expected table, got {other:?}"),
        }
        assert!(matches!(&bs[1], Block::Heading { level: 1, .. }));
    }

    #[test]
    fn too_many_digits_is_not_an_ordered_list() {
        assert!(matches!(&blocks("1234567890. x")[0], Block::Paragraph(_)));
    }

    #[test]
    fn indented_delimiter_row_is_not_a_table() {
        let bs = blocks("| a | b |\n    |---|:-:|");
        assert!(matches!(&bs[0], Block::Paragraph(_)), "got {bs:?}");
    }

    #[test]
    fn blockquote_interrupts_a_paragraph() {
        let bs = blocks("text\n> quote");
        assert_eq!(bs.len(), 2);
        assert!(matches!(&bs[1], Block::Quote(_)));
    }
}
