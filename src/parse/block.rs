//! Block-level parsing: lines to a tree of raw blocks.
//!
//! The parser is a cursor over lines. At each position it tries block
//! starts in a fixed order; paragraphs then apply the "what can interrupt
//! a paragraph" rules. Container blocks (quotes, list items) recursively
//! parse their stripped content, which keeps every nesting level on the
//! same code path.
//!
//! Paragraphs, headings, table cells and figures keep *raw text* until the
//! whole document has been read: link reference definitions can appear
//! anywhere, and the specification resolves them document-wide. `finalize`
//! turns raw blocks into [`Block`]s once every definition is known.

use super::Extensions;
use super::inline::{LinkRef, Refs, normalize_label, parse_inlines, parse_with_refs};
use crate::ast::{Align, Block, CodeBlock, ImageData, Inline, ListData, ListItem, TableData};

/// A block whose inline content has not been parsed yet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum RawBlock {
    /// Paragraph text, lines joined with `\n`.
    Paragraph(String),
    /// Heading with raw inline text.
    Heading { level: u8, text: String },
    /// Code block, already final.
    Code(CodeBlock),
    /// Block quote with raw children.
    Quote(Vec<RawBlock>),
    /// List, with raw item contents.
    List {
        ordered: bool,
        start: u32,
        tight: bool,
        items: Vec<Vec<RawBlock>>,
    },
    /// GFM table with raw cell text.
    Table {
        aligns: Vec<Align>,
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Raw HTML block, kept verbatim.
    Html(String),
    /// Thematic break.
    ThematicBreak,
}

/// Parses a sequence of lines into raw blocks.
pub(crate) fn parse_blocks(lines: &[String], extensions: Extensions) -> Vec<RawBlock> {
    BlockParser {
        lines,
        pos: 0,
        pending: std::collections::VecDeque::new(),
        extensions,
    }
    .run()
}

/// Cursor over the lines of one nesting level.
struct BlockParser<'a> {
    lines: &'a [String],
    pos: usize,
    /// Blocks produced ahead of the current one (e.g. definitions that
    /// precede a setext heading).
    pending: std::collections::VecDeque<RawBlock>,
    /// Enabled syntax extensions.
    extensions: Extensions,
}

impl<'a> BlockParser<'a> {
    fn peek(&self) -> Option<&'a str> {
        self.lines.get(self.pos).map(String::as_str)
    }

    fn run(&mut self) -> Vec<RawBlock> {
        let mut blocks = Vec::new();
        while let Some(line) = self.peek() {
            if is_blank(line) {
                self.pos += 1;
                continue;
            }
            let block = if is_thematic_break(line) {
                self.pos += 1;
                Some(RawBlock::ThematicBreak)
            } else if let Some((level, content)) = atx_heading(line) {
                self.pos += 1;
                Some(RawBlock::Heading {
                    level,
                    text: content,
                })
            } else if let Some((fc, flen, info)) = fence_start(line) {
                Some(self.take_fenced(fc, flen, info))
            } else if let Some(kind) = html_block_start(line) {
                Some(self.take_html(kind))
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
                blocks.extend(self.pending.drain(..));
                blocks.push(b);
            }
        }
        blocks
    }

    /// Consumes a fenced code block. The current line is the opening fence.
    fn take_fenced(&mut self, fc: char, flen: usize, info: String) -> RawBlock {
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
        RawBlock::Code(CodeBlock { info, lines })
    }

    /// Consumes an HTML block of the given kind.
    fn take_html(&mut self, kind: HtmlKind) -> RawBlock {
        let mut lines: Vec<String> = Vec::new();
        while let Some(line) = self.peek() {
            match &kind {
                HtmlKind::Until(end) => {
                    lines.push(line.to_string());
                    self.pos += 1;
                    if line.to_ascii_lowercase().contains(end) {
                        break;
                    }
                }
                HtmlKind::Blank | HtmlKind::BareTag => {
                    if is_blank(line) {
                        break;
                    }
                    lines.push(line.to_string());
                    self.pos += 1;
                }
            }
        }
        let mut text = lines.join("\n");
        text.push('\n');
        RawBlock::Html(text)
    }

    /// Consumes a blockquote, stripping `>` markers and accepting lazy
    /// paragraph continuation lines.
    fn take_quote(&mut self) -> RawBlock {
        let mut content: Vec<String> = Vec::new();
        let mut fence: Option<(char, usize)> = None;
        let mut lazy = false;
        while let Some(line) = self.peek() {
            if quote_start(line) {
                let inner = strip_quote_marker(line);
                update_fence(&mut fence, &inner);
                content.push(inner);
                lazy = false;
                self.pos += 1;
            } else if is_blank(line) {
                // A blank line that is not marked with `>` ends the quote.
                break;
            } else if fence.is_none() && can_be_lazy(&content, line) {
                // Lazy continuation of a paragraph that forgot the `>`.
                content.push(line.to_string());
                lazy = true;
                self.pos += 1;
            } else if fence.is_none() && lazy && is_setext_underline(line).is_some() {
                // A lazy line cannot underline a heading: escape the marker
                // so it stays literal text.
                content.push(format!("\\{line}"));
                self.pos += 1;
            } else {
                break;
            }
        }
        RawBlock::Quote(parse_blocks(&content, self.extensions))
    }

    /// Consumes an entire list (all consecutive items of the same type).
    fn take_list(&mut self) -> RawBlock {
        let first = list_marker(self.peek().unwrap()).expect("caller verified list start");
        let (ordered, delim, base_indent) = (first.ordered, first.ch, first.indent);
        let mut items: Vec<Vec<RawBlock>> = Vec::new();
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
                Some(line)
                    if !is_thematic_break(line) && same_list(line, ordered, delim, base_indent) =>
                {
                    if blank_between {
                        tight = false;
                    }
                }
                _ => break,
            }
        }
        let start = if ordered { first.number } else { 1 };
        RawBlock::List {
            ordered,
            start,
            tight,
            items,
        }
    }

    /// Consumes one list item. Returns the item and whether a blank line
    /// fell inside it (which makes the surrounding list loose).
    ///
    fn take_item(&mut self) -> Option<(Vec<RawBlock>, bool)> {
        let line = self.peek()?;
        let m = list_marker(line)?;
        let column = m.indent + m.len;
        let after = &line[column..];
        // Tabs after the marker advance to the next four-column stop.
        let (spaces, rest) = leading_whitespace_columns(after, column);

        let mut content: Vec<String> = Vec::new();
        let content_indent;
        if rest.is_empty() {
            // Nothing after the marker: content (if any) starts at the next
            // column past the marker.
            content_indent = column + 1;
            self.pos += 1;
        } else if spaces >= 5 {
            // Five or more spaces: the item starts with indented code.
            content_indent = column + 1;
            content.push(format!("{}{}", " ".repeat(spaces - 1), rest));
            self.pos += 1;
        } else if spaces >= 1 {
            content_indent = column + spaces;
            content.push(rest.to_string());
            self.pos += 1;
        } else {
            // No space after the marker: not a list item at all.
            return None;
        }

        let mut internal_blank = false;
        // A list item may begin with at most one blank line.
        if rest.is_empty() && self.peek().is_some_and(is_blank) {
            return Some((Vec::new(), false));
        }
        let mut fence: Option<(char, usize)> = None;
        update_fence(&mut fence, content.last().map(String::as_str).unwrap_or(""));
        while let Some(line) = self.peek() {
            if is_blank(line) {
                // Blank lines belong to this item only if the next
                // non-blank line is indented to the content column.
                let mut j = self.pos + 1;
                while j < self.lines.len() && is_blank(&self.lines[j]) {
                    j += 1;
                }
                if j < self.lines.len() && indent_of(&self.lines[j]) >= content_indent {
                    // The blank loosens the list when it separates two of
                    // this item's own blocks. If the previous line opened a
                    // container and the next line is indented inside it, the
                    // blank belongs to that nested block instead.
                    let previous = content.iter().rev().find(|l| !is_blank(l));
                    let opens_container =
                        previous.is_some_and(|l| list_marker(l).is_some() || quote_start(l));
                    let nested = opens_container
                        && previous.is_some_and(|l| {
                            indent_of(&self.lines[j]) - content_indent > indent_of(l)
                        });
                    if !nested && fence.is_none() {
                        internal_blank = true;
                    }
                    for _ in self.pos..j {
                        content.push(String::new());
                    }
                    self.pos = j;
                } else {
                    break;
                }
            } else if indent_of(line) >= content_indent {
                let inner = strip_indent(line, content_indent).to_string();
                update_fence(&mut fence, &inner);
                content.push(inner);
                self.pos += 1;
            } else if is_thematic_break(line) {
                break;
            } else if is_list_start(line) && indent_of(line) < content_indent {
                // A marker that does not reach this item's content column
                // belongs to the enclosing list, not to this item.
                break;
            } else if interrupts_paragraph(line) || is_setext_underline(line).is_some() {
                break;
            } else if content.last().is_some_and(|l| !is_blank(l)) {
                // Lazy paragraph continuation into the item.
                content.push(line.to_string());
                self.pos += 1;
            } else {
                break;
            }
        }
        Some((parse_blocks(&content, self.extensions), internal_blank))
    }

    /// Consumes an indented (4-space) code block.
    fn take_indented_code(&mut self) -> RawBlock {
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
        RawBlock::Code(CodeBlock {
            info: String::new(),
            lines,
        })
    }

    /// Attempts to parse a GFM table at the current position.
    fn try_table(&mut self) -> Option<RawBlock> {
        let header_line = self.peek()?;
        if indent_of(header_line) > 3 || !header_line.contains('|') {
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
            if !line.contains('|') && rows.is_empty() {
                break;
            }
            rows.push(split_row(line));
            self.pos += 1;
        }
        Some(RawBlock::Table { aligns, head, rows })
    }

    /// Consumes a paragraph, watching for Setext underlines and tables.
    fn take_paragraph(&mut self) -> Option<RawBlock> {
        let mut raw: Vec<&'a str> = Vec::new();
        let mut setext_level = 0u8;
        while let Some(line) = self.peek() {
            if !raw.is_empty() {
                if let Some(level) = is_setext_underline(line) {
                    setext_level = level;
                    self.pos += 1;
                    break;
                }
                // A table may start on the next line: stop before its header
                // so the outer loop can pick it up.
                if self.next_line_starts_table() {
                    break;
                }
            }
            if is_blank(line) || interrupts_paragraph(line) {
                break;
            }
            // Paragraph text is never indented: up to three spaces are
            // allowed before a block, and deeper indentation cannot start
            // code inside an open paragraph.
            raw.push(line.trim_start_matches([' ', '\t']));
            self.pos += 1;
        }
        if raw.is_empty() {
            return None;
        }
        let joined = raw.join("\n");
        if setext_level > 0 {
            // A definition can precede the heading; it is a block of its own.
            let mut rest = joined.as_str();
            let mut consumed = 0usize;
            while let Some((_, _, used)) = parse_definition(rest) {
                rest = &rest[used..];
                consumed += used;
            }
            if consumed > 0 {
                self.pending
                    .push_back(RawBlock::Paragraph(joined[..consumed].to_string()));
                if rest.trim().is_empty() {
                    // Nothing left to underline: rewind so the underline
                    // itself starts the next paragraph.
                    self.pos -= 1;
                    return None;
                }
                return Some(RawBlock::Heading {
                    level: setext_level,
                    text: rest.trim_start_matches('\n').to_string(),
                });
            }
            return Some(RawBlock::Heading {
                level: setext_level,
                text: joined,
            });
        }
        Some(RawBlock::Paragraph(joined))
    }

    /// Whether the line at the cursor is a table header followed by a
    /// delimiter row.
    fn next_line_starts_table(&self) -> bool {
        let Some(header) = self.peek() else {
            return false;
        };
        let Some(sep) = self.lines.get(self.pos + 1) else {
            return false;
        };
        if indent_of(header) > 3 || !header.contains('|') {
            return false;
        }
        match parse_delimiter_row(sep) {
            Some(aligns) => split_row(header).len() == aligns.len(),
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Link reference definitions
// ---------------------------------------------------------------------------

/// Removes link reference definitions from the raw tree and collects them.
///
/// Definitions may appear anywhere a paragraph can start; the first one for
/// a given label wins.
pub(crate) fn collect_definitions(blocks: &mut Vec<RawBlock>) -> Refs {
    let mut refs = Refs::new();
    collect_definitions_into(blocks, &mut refs);
    refs
}

fn collect_definitions_into(blocks: &mut Vec<RawBlock>, refs: &mut Refs) {
    let mut kept: Vec<RawBlock> = Vec::with_capacity(blocks.len());
    for mut block in std::mem::take(blocks) {
        match &mut block {
            RawBlock::Paragraph(text) => {
                let mut rest = text.as_str();
                let mut cut = 0usize;
                while let Some((label, definition, consumed)) = parse_definition(rest) {
                    let key = normalize_label(&label);
                    if !key.is_empty() {
                        refs.entry(key).or_insert(definition);
                    }
                    rest = &rest[consumed..];
                    cut += consumed;
                }
                if rest.trim().is_empty() {
                    continue;
                }
                if cut > 0 {
                    block = RawBlock::Paragraph(rest.trim_start_matches('\n').to_string());
                }
                kept.push(block);
            }
            RawBlock::Quote(children) => {
                collect_definitions_into(children, refs);
                kept.push(block);
            }
            RawBlock::List { items, .. } => {
                for item in items.iter_mut() {
                    collect_definitions_into(item, refs);
                }
                kept.push(block);
            }
            _ => kept.push(block),
        }
    }
    *blocks = kept;
}

/// Parses one link reference definition at the start of `text`.
///
/// Returns the label, the definition and the number of bytes consumed.
fn parse_definition(text: &str) -> Option<(String, LinkRef, usize)> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let mut i = 1usize;
    let mut raw_label = String::new();
    loop {
        match bytes.get(i)? {
            b'\\'
                if bytes
                    .get(i + 1)
                    .is_some_and(|b| (*b as char).is_ascii_punctuation()) =>
            {
                raw_label.push('\\');
                raw_label.push(bytes[i + 1] as char);
                i += 2;
            }
            b'[' => return None,
            b']' => {
                i += 1;
                break;
            }
            b'\n' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    return None;
                }
                raw_label.push('\n');
                i += 1;
            }
            _ => {
                let ch = text[i..].chars().next()?;
                raw_label.push(ch);
                i += ch.len_utf8();
            }
        }
        if raw_label.chars().count() > 999 {
            return None;
        }
    }
    if raw_label.trim().is_empty() {
        return None;
    }
    if bytes.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    // Optional spaces, at most one newline, then the destination.
    i = skip_definition_space(text, i);
    let (url, next) = scan_definition_destination(text, i)?;
    i = next;
    let after_url = i;

    // The title must be separated by whitespace, and anything else on the
    // line makes the definition invalid.
    let mut title = None;
    let mut end_of_line = skip_definition_space(text, i);
    if end_of_line > i
        && text[end_of_line..].starts_with(['"', '\'', '('])
        && let Some((t, next)) = scan_definition_title(text, end_of_line)
    {
        title = Some(t);
        end_of_line = skip_spaces(text, next);
        i = next;
    }
    let _ = i;
    if end_of_line > 0 {
        // Consume the rest of the line: only spaces may follow.
        let rest = &text[end_of_line..];
        let line_end = rest
            .find('\n')
            .map(|n| end_of_line + n + 1)
            .unwrap_or(text.len());
        let tail = &text[end_of_line..line_end.min(text.len())];
        if !tail.trim().is_empty() {
            // Trailing junk: the definition is not valid. Fall back to a
            // destination-only definition ending at the URL's line.
            let line_end = text[after_url..]
                .find('\n')
                .map(|n| after_url + n + 1)
                .unwrap_or(text.len());
            if !text[after_url..line_end].trim().is_empty() {
                return None;
            }
            return Some((raw_label, LinkRef { url, title: None }, line_end));
        }
        return Some((raw_label, LinkRef { url, title }, line_end.min(text.len())));
    }
    None
}

fn skip_spaces(text: &str, mut i: usize) -> usize {
    let bytes = text.as_bytes();
    while matches!(bytes.get(i), Some(b' ') | Some(b'\t')) {
        i += 1;
    }
    i
}

/// Skips spaces, tabs and at most one line ending.
fn skip_definition_space(text: &str, mut i: usize) -> usize {
    i = skip_spaces(text, i);
    if text.as_bytes().get(i) == Some(&b'\n') {
        i = skip_spaces(text, i + 1);
    }
    i
}

/// Scans a definition destination at `i`.
fn scan_definition_destination(text: &str, i: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    if bytes.get(i) == Some(&b'<') {
        let mut j = i + 1;
        let mut raw = String::new();
        while j < bytes.len() {
            match bytes[j] {
                b'>' => return Some((decode_inline_text(&raw), j + 1)),
                b'<' | b'\n' => return None,
                b'\\'
                    if bytes
                        .get(j + 1)
                        .is_some_and(|b| (*b as char).is_ascii_punctuation()) =>
                {
                    raw.push('\\');
                    raw.push(bytes[j + 1] as char);
                    j += 2;
                }
                _ => {
                    let ch = text[j..].chars().next()?;
                    raw.push(ch);
                    j += ch.len_utf8();
                }
            }
        }
        return None;
    }
    let mut j = i;
    let mut depth = 0usize;
    let mut raw = String::new();
    while j < bytes.len() {
        match bytes[j] {
            b'\\'
                if bytes
                    .get(j + 1)
                    .is_some_and(|b| (*b as char).is_ascii_punctuation()) =>
            {
                raw.push('\\');
                raw.push(bytes[j + 1] as char);
                j += 2;
            }
            b' ' | b'\t' | b'\n' | b'\r' => break,
            b'(' => {
                depth += 1;
                raw.push('(');
                j += 1;
            }
            b')' if depth == 0 => break,
            b')' => {
                depth -= 1;
                raw.push(')');
                j += 1;
            }
            _ => {
                let ch = text[j..].chars().next()?;
                raw.push(ch);
                j += ch.len_utf8();
            }
        }
    }
    if raw.is_empty() || depth != 0 {
        return None;
    }
    Some((decode_inline_text(&raw), j))
}

/// Scans a definition title at `i`.
fn scan_definition_title(text: &str, i: usize) -> Option<(String, usize)> {
    let bytes = text.as_bytes();
    let open = *bytes.get(i)?;
    let close = match open {
        b'"' => b'"',
        b'\'' => b'\'',
        b'(' => b')',
        _ => return None,
    };
    let mut j = i + 1;
    let mut raw = String::new();
    while j < bytes.len() {
        match bytes[j] {
            b'\\'
                if bytes
                    .get(j + 1)
                    .is_some_and(|b| (*b as char).is_ascii_punctuation()) =>
            {
                raw.push('\\');
                raw.push(bytes[j + 1] as char);
                j += 2;
            }
            b if b == close => return Some((decode_inline_text(&raw), j + 1)),
            b'\n' => {
                // A blank line ends the definition.
                if bytes.get(j + 1) == Some(&b'\n') {
                    return None;
                }
                raw.push('\n');
                j += 1;
            }
            b if close == b')' && b == b'(' => return None,
            _ => {
                let ch = text[j..].chars().next()?;
                raw.push(ch);
                j += ch.len_utf8();
            }
        }
    }
    None
}

/// Decodes backslash escapes and entities inside a destination or title.
fn decode_inline_text(raw: &str) -> String {
    super::inline::decode_text_for_definitions(raw)
}

// ---------------------------------------------------------------------------
// Finalization: raw blocks to AST blocks
// ---------------------------------------------------------------------------

/// Converts raw blocks into AST blocks, parsing inline content with `refs`.
pub(crate) fn finalize(blocks: Vec<RawBlock>, refs: &Refs, ext: Extensions) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            RawBlock::Paragraph(text) => {
                let inlines = parse_with_refs(text.trim_end(), refs, ext);
                if inlines.len() == 1
                    && let Inline::Image(ImageData { alt, url, title }) = &inlines[0]
                {
                    // A paragraph containing only an image is a figure.
                    out.push(Block::Figure(ImageData {
                        alt: alt.clone(),
                        url: url.clone(),
                        title: title.clone(),
                    }));
                    continue;
                }
                out.push(Block::Paragraph(inlines));
            }
            RawBlock::Heading { level, text } => out.push(Block::Heading {
                level,
                inlines: parse_with_refs(text.trim_end(), refs, ext),
            }),
            RawBlock::Code(code) => out.push(Block::Code(code)),
            RawBlock::Quote(children) => out.push(Block::Quote(finalize(children, refs, ext))),
            RawBlock::List {
                ordered,
                start,
                tight,
                items,
            } => out.push(Block::List(ListData {
                ordered,
                start,
                tight,
                items: items
                    .into_iter()
                    .map(|mut blocks| {
                        let task = if ext.task_lists {
                            take_task_marker(&mut blocks)
                        } else {
                            None
                        };
                        ListItem {
                            blocks: finalize(blocks, refs, ext),
                            task,
                        }
                    })
                    .collect(),
            })),
            RawBlock::Table { aligns, head, rows } => {
                let ncols = aligns.len();
                let head: Vec<Vec<Inline>> = head
                    .iter()
                    .map(|cell| parse_with_refs(cell, refs, ext))
                    .collect();
                let rows: Vec<Vec<Vec<Inline>>> = rows
                    .into_iter()
                    .map(|row| {
                        let mut cells: Vec<Vec<Inline>> = row
                            .iter()
                            .map(|cell| parse_with_refs(cell, refs, ext))
                            .collect();
                        cells.resize(ncols, Vec::new());
                        cells.truncate(ncols);
                        cells
                    })
                    .collect();
                out.push(Block::Table(TableData { aligns, head, rows }));
            }
            RawBlock::Html(text) => out.push(Block::Html(text)),
            RawBlock::ThematicBreak => out.push(Block::ThematicBreak),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// HTML blocks
// ---------------------------------------------------------------------------

/// How an HTML block ends.
#[derive(Debug, Clone, PartialEq)]
enum HtmlKind {
    /// Ends on a line containing this marker.
    Until(&'static str),
    /// Ends at a blank line; a block-level tag (type 6).
    Blank,
    /// Ends at a blank line; a bare tag (type 7, cannot interrupt).
    BareTag,
}

/// Block-level tag names that start an HTML block of type 6.
const BLOCK_TAGS: &[&str] = &[
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
    "xmp",
];

/// Recognizes the seven kinds of HTML block start.
fn html_block_start(line: &str) -> Option<HtmlKind> {
    if indent_of(line) > 3 {
        return None;
    }
    let s = line.trim_start();
    let lower = s.to_ascii_lowercase();
    for (prefix, end) in [
        ("<script", "</script>"),
        ("<pre", "</pre>"),
        ("<style", "</style>"),
        ("<textarea", "</textarea>"),
    ] {
        if lower.starts_with(prefix)
            && lower[prefix.len()..]
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '>')
        {
            return Some(HtmlKind::Until(end));
        }
    }
    if s.starts_with("<!--") {
        return Some(HtmlKind::Until("-->"));
    }
    if s.starts_with("<?") {
        return Some(HtmlKind::Until("?>"));
    }
    if s.starts_with("<![CDATA[") {
        return Some(HtmlKind::Until("]]>"));
    }
    if let Some(rest) = s.strip_prefix("<!")
        && rest.starts_with(|c: char| c.is_ascii_alphabetic())
    {
        return Some(HtmlKind::Until(">"));
    }
    // Type 6: a block-level tag.
    let name = s
        .strip_prefix("</")
        .or_else(|| s.strip_prefix('<'))
        .map(tag_name)
        .unwrap_or_default();
    if !name.is_empty()
        && BLOCK_TAGS.iter().any(|t| t.eq_ignore_ascii_case(name))
        && s.get(1 + name.len() + usize::from(s.starts_with("</"))..)
            .is_none_or(|rest| {
                rest.starts_with('>')
                    || rest.starts_with("/>")
                    || rest.starts_with(|c: char| c.is_whitespace())
            })
    {
        return Some(HtmlKind::Blank);
    }
    // Type 7: a complete tag alone on the line.
    if is_complete_tag_line(s) {
        return Some(HtmlKind::BareTag);
    }
    None
}

fn tag_name(s: &str) -> &str {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        if c.is_ascii_alphanumeric() || c == '-' {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    &s[..end]
}

/// Whether the line is a single complete open or closing tag.
///
/// This is HTML block type 7: a tag followed only by whitespace. Attribute
/// names and values are validated, so `<https://example.com>` is an
/// autolink rather than an HTML block.
fn is_complete_tag_line(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    if bytes.get(i) != Some(&b'<') {
        return false;
    }
    i += 1;
    if bytes.get(i) == Some(&b'/') {
        i += 1;
        let name_start = i;
        if !bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
            return false;
        }
        while bytes
            .get(i)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            i += 1;
        }
        if i == name_start {
            return false;
        }
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        return bytes.get(i) == Some(&b'>') && i + 1 == bytes.len();
    }
    if !bytes.get(i).is_some_and(|b| b.is_ascii_alphabetic()) {
        return false;
    }
    while bytes
        .get(i)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        i += 1;
    }
    loop {
        let before = i;
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        match bytes.get(i) {
            Some(b'>') => return i + 1 == bytes.len(),
            Some(b'/') if bytes.get(i + 1) == Some(&b'>') => return i + 2 == bytes.len(),
            Some(_) if i > before => {
                // Attribute name.
                if !bytes
                    .get(i)
                    .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_' || *b == b':')
                {
                    return false;
                }
                while bytes.get(i).is_some_and(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-')
                }) {
                    i += 1;
                }
                // Optional value.
                let save = i;
                while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                    i += 1;
                }
                if bytes.get(i) == Some(&b'=') {
                    i += 1;
                    while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
                        i += 1;
                    }
                    match bytes.get(i) {
                        Some(&q @ (b'"' | b'\'')) => {
                            i += 1;
                            while i < bytes.len() && bytes[i] != q {
                                i += 1;
                            }
                            if i >= bytes.len() {
                                return false;
                            }
                            i += 1;
                        }
                        Some(_) => {
                            let value_start = i;
                            while bytes.get(i).is_some_and(|b| {
                                !b.is_ascii_whitespace()
                                    && !matches!(b, b'"' | b'\'' | b'=' | b'<' | b'>' | b'`')
                            }) {
                                i += 1;
                            }
                            if i == value_start {
                                return false;
                            }
                        }
                        None => return false,
                    }
                } else {
                    i = save;
                }
            }
            _ => return false,
        }
    }
}

// ---------------------------------------------------------------------------

/// Detects a GFM task marker at the start of an item and removes it.
///
/// Returns `Some(true)` for `[x]`, `Some(false)` for `[ ]`.
fn take_task_marker(blocks: &mut Vec<RawBlock>) -> Option<bool> {
    let RawBlock::Paragraph(text) = blocks.first_mut()? else {
        return None;
    };
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let checked = match bytes.get(1) {
        Some(b' ') => false,
        Some(b'x') | Some(b'X') => true,
        _ => return None,
    };
    if bytes.get(2) != Some(&b']') {
        return None;
    }
    let rest = &text[3..];
    let stripped = match rest.strip_prefix([' ', '\t']) {
        Some(rest) => rest,
        None if rest.is_empty() => rest,
        None => return None,
    };
    if stripped.trim().is_empty() {
        blocks.remove(0);
    } else {
        *text = stripped.to_string();
    }
    Some(checked)
}

// Line predicates and helpers
// ---------------------------------------------------------------------------

/// Whether a fence opens or closes at `line`, tracking the open fence.
fn update_fence(state: &mut Option<(char, usize)>, line: &str) {
    match *state {
        Some((ch, len)) => {
            if is_fence_end(line, ch, len) {
                *state = None;
            }
        }
        None => {
            if let Some((ch, len, _)) = fence_start(line) {
                *state = Some((ch, len));
            }
        }
    }
}

/// Whether `line` may lazily continue the paragraph ending `content`.
///
/// Only paragraphs are lazy; a setext underline transforms the paragraph, so
/// it cannot continue a container, and indented lines belong to code.
fn can_be_lazy(content: &[String], line: &str) -> bool {
    if content.is_empty() || interrupts_paragraph(line) || is_setext_underline(line).is_some() {
        return false;
    }
    let Some(last) = content.last() else {
        return false;
    };
    if is_blank(last) {
        return false;
    }
    indent_of(last) < 4 && fence_start(last).is_none()
}

fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

/// Number of leading spaces (indentation is pre-expanded).
fn indent_of(s: &str) -> usize {
    s.bytes().take_while(|b| *b == b' ').count()
}

/// Removes up to `n` leading spaces.
fn strip_indent(s: &str, n: usize) -> &str {
    let have = indent_of(s).min(n);
    &s[have..]
}

/// `---`, `***`, `___` (3+ of one char, spaces allowed between).
fn is_thematic_break(line: &str) -> bool {
    if indent_of(line) > 3 {
        return false;
    }
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
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
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
    let raw = s[n..].trim();
    // Backtick fences may not carry backticks in their info string.
    if c == '`' && raw.contains('`') {
        return None;
    }
    // The info string is inline content: escapes and entities are decoded.
    let info = parse_inlines(raw)
        .iter()
        .map(|inline| inline.plain_text())
        .collect::<String>();
    Some((c, n, info.trim().to_string()))
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
    let rest = &line[indent + 1..];
    // One optional space belongs to the marker; tabs after it advance to
    // four-column stops counted from the marker.
    let (spaces, rest) = leading_whitespace_columns(rest, indent + 1);
    if spaces == 0 {
        return rest.to_string();
    }
    let trimmed = rest;
    let consumed = if line[indent + 1..].starts_with(' ') {
        1
    } else {
        0
    };
    let _ = consumed;
    format!("{}{}", " ".repeat(spaces.saturating_sub(1)), trimmed)
}

/// Splits the leading run of spaces and tabs into columns, returning the
/// width and the remaining text.
fn leading_whitespace_columns(s: &str, start_col: usize) -> (usize, &str) {
    let mut col = start_col;
    let mut idx = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            ' ' => col += 1,
            '\t' => col += 4 - col % 4,
            _ => break,
        }
        idx = i + 1;
    }
    (col - start_col, &s[idx..])
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
            after.is_empty() || after.starts_with([' ', '\t'])
        }
        None => false,
    }
}

/// Whether the line continues the same list: same type, same delimiter
/// char, and the same marker indent.
fn same_list(line: &str, ordered: bool, delim: char, _base_indent: usize) -> bool {
    match list_marker(line) {
        // Up to three spaces of marker indentation still belong to the
        // same list: only the marker type and delimiter matter.
        Some(m) if m.ordered == ordered && m.ch == delim => {
            let after = &line[m.indent + m.len..];
            after.is_empty() || after.starts_with([' ', '\t'])
        }
        _ => false,
    }
}

/// Whether a line ends an open paragraph.
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
    // HTML block types 1-6 interrupt a paragraph; type 7 does not.
    if let Some(kind) = html_block_start(line) {
        match kind {
            HtmlKind::Until(_) | HtmlKind::Blank => return true,
            HtmlKind::BareTag => return false,
        }
    }
    if let Some(m) = list_marker(line) {
        let after = &line[m.indent + m.len..];
        if !after.starts_with([' ', '\t']) {
            return false;
        }
        // Only non-empty items interrupt a paragraph, and ordered lists
        // must start at 1.
        if after.trim().is_empty() {
            return false;
        }
        return !m.ordered || m.number == 1;
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

/// Splits a table row into cells on unescaped `|`, ignoring pipes inside
/// code spans and dropping the cells produced by leading/trailing pipes.
fn split_row(line: &str) -> Vec<String> {
    let mut cells: Vec<String> = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    let mut code_run: Option<usize> = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            let run = chars[i..].iter().take_while(|ch| **ch == '`').count();
            match code_run {
                Some(len) if len == run => code_run = None,
                Some(_) => {}
                None => code_run = Some(run),
            }
            for _ in 0..run {
                cur.push('`');
            }
            i += run;
            continue;
        }
        if c == '\\' && chars.get(i + 1) == Some(&'|') {
            // Escaped pipes are literal, including inside code spans.
            cur.push('|');
            i += 2;
            continue;
        }
        if code_run.is_none() && c == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
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

    fn blocks(src: &str) -> Vec<Block> {
        let lines: Vec<String> = src.split('\n').map(str::to_string).collect();
        let mut raw = parse_blocks(&lines, Extensions::gfm());
        let refs = collect_definitions(&mut raw);
        finalize(raw, &refs, Extensions::gfm())
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
        for src in ["--", "**", "_ _", "- a", "*em*", "--x--"] {
            assert!(
                !matches!(blocks(src)[0], Block::ThematicBreak),
                "false positive: {src}"
            );
        }
    }

    #[test]
    fn definitions_are_collected() {
        let bs = blocks("[foo]: /url \"title\"\n\n[foo]");
        assert_eq!(bs.len(), 1);
        assert_eq!(
            bs[0],
            Block::Paragraph(vec![Inline::Link {
                text: vec![Inline::Text("foo".into())],
                url: "/url".into(),
                title: Some("title".into())
            }])
        );
    }
}
