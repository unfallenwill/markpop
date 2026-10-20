//! Splitting blocks that are taller than the space available for them.
//!
//! Pagination and multi-column flow cut **between** top-level blocks, which
//! keeps paragraphs, tables and code boxes intact. A block that is taller
//! than a whole page/column would still be clipped, so before layout this
//! module breaks the splittable ones into pieces that fit:
//!
//! * **code blocks** — at source-line boundaries (indentation and the info
//!   string are preserved, the language label repeats);
//! * **lists** — between items, with ordered numbering continuing across
//!   chunks;
//! * **blockquotes** — between child blocks, each piece keeping its bar;
//! * **tables** — between rows, repeating the header row in every piece.
//!
//! Only a single oversized paragraph or figure is left intact: there is no
//! meaningful place to cut those without re-flowing lines.

use crate::ast::{Block, CodeBlock, Document, ListData, ListItem, TableData};
use crate::layout::{self, Layout};
use crate::scene::FontKind;
use crate::text::{Metrics, TextStyle};
use crate::theme::Theme;

/// Height of a block sequence, measured with the real layout engine.
///
/// The first block's top margin and the last block's bottom margin are
/// excluded, matching how [`crate::layout::BlockMark`] reports heights.
pub(crate) fn measure(blocks: &[Block], theme: &Theme) -> f32 {
    if blocks.is_empty() {
        return 0.0;
    }
    let doc = Document {
        blocks: blocks.to_vec(),
    };
    let (_, marks) = Layout::new(theme).render_marked(&doc);
    match (marks.first(), marks.last()) {
        (Some(first), Some(last)) => (last.bottom - first.top).max(0.0),
        _ => 0.0,
    }
}

/// Splits top-level blocks so that none is taller than `available`.
pub(crate) fn for_height(blocks: Vec<Block>, theme: &Theme, available: f32) -> Vec<Block> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        out.extend(split_block(block, theme, available, 6));
    }
    out
}

fn split_block(block: Block, theme: &Theme, available: f32, depth: u32) -> Vec<Block> {
    if depth == 0 || available <= 0.0 {
        return vec![block];
    }
    if measure(std::slice::from_ref(&block), theme) <= available {
        return vec![block];
    }
    match block {
        Block::Code(code) => split_code(code, theme, available),
        Block::List(list) => split_list(list, theme, available, depth),
        Block::Quote(children) => split_quote(children, theme, available, depth),
        Block::Table(table) => split_table(table, theme, available, depth),
        other => vec![other],
    }
}

// ---------------------------------------------------------------------------
// Code blocks
// ---------------------------------------------------------------------------

fn split_code(code: CodeBlock, theme: &Theme, available: f32) -> Vec<Block> {
    let line_height = (theme.code_size() * theme.code_line_height).max(1.0);
    let chrome = layout::code_chrome_height(theme);
    let max_lines = ((available - chrome).max(line_height) / line_height).floor() as usize;
    let wrap_width = layout::code_wrap_width(theme);
    let style =
        TextStyle::new(theme.code_size(), FontKind::Mono).with_letter_spacing(theme.letter_spacing);
    let metrics = Metrics::new();

    let mut chunks: Vec<CodeBlock> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0usize;

    for raw in &code.lines {
        let rendered = if raw.is_empty() {
            1
        } else {
            metrics.wrap_preformatted(raw, wrap_width, &style).len()
        };
        // Breaks only happen at source-line boundaries; a single line taller
        // than the page stays where it is.
        if used + rendered > max_lines && !current.is_empty() {
            chunks.push(CodeBlock {
                info: code.info.clone(),
                lines: std::mem::take(&mut current),
            });
            used = 0;
        }
        current.push(raw.clone());
        used += rendered;
    }
    if !current.is_empty() || chunks.is_empty() {
        chunks.push(CodeBlock {
            info: code.info.clone(),
            lines: current,
        });
    }
    chunks.into_iter().map(Block::Code).collect()
}

// ---------------------------------------------------------------------------
// Lists
// ---------------------------------------------------------------------------

fn split_list(list: ListData, theme: &Theme, available: f32, depth: u32) -> Vec<Block> {
    if list.items.len() <= 1 || depth == 0 {
        return vec![Block::List(list)];
    }
    // If even the smallest item does not fit, splitting only produces more
    // overflowing blocks: leave the list intact.
    let smallest = list
        .items
        .iter()
        .map(|item| {
            measure(
                std::slice::from_ref(&Block::List(ListData {
                    ordered: list.ordered,
                    start: 1,
                    tight: list.tight,
                    items: vec![item.clone()],
                })),
                theme,
            )
        })
        .fold(f32::INFINITY, f32::min);
    if smallest > available {
        return vec![Block::List(list)];
    }
    // A little headroom absorbs rounding and inter-item margins.
    let budget = (available - 2.0).max(1.0);
    let loose_gap = if list.tight {
        0.0
    } else {
        theme.font_size * 0.45
    };

    let mut chunks: Vec<ListData> = Vec::new();
    let mut current: Vec<ListItem> = Vec::new();
    let mut used = 0.0f32;
    let mut chunk_start = list.start;

    for (number, item) in (list.start..).zip(list.items.iter()) {
        let item_height = measure(
            std::slice::from_ref(&Block::List(ListData {
                ordered: list.ordered,
                start: 1,
                tight: list.tight,
                items: vec![item.clone()],
            })),
            theme,
        );
        let gap = if current.is_empty() { 0.0 } else { loose_gap };
        if !current.is_empty() && used + gap + item_height > budget {
            chunks.push(ListData {
                ordered: list.ordered,
                start: chunk_start,
                tight: list.tight,
                items: std::mem::take(&mut current),
            });
            used = 0.0;
            chunk_start = number;
        }
        used += gap + item_height;
        current.push(item.clone());
    }
    if !current.is_empty() {
        chunks.push(ListData {
            ordered: list.ordered,
            start: chunk_start,
            tight: list.tight,
            items: current,
        });
    }

    // Verify against the real layout; if a chunk is still too tall (a single
    // heavy item can skew the estimate), split it in half and retry.
    let mut out = Vec::new();
    for chunk in chunks {
        let chunk_block = Block::List(chunk.clone());
        if chunk.items.len() <= 1 || measure(std::slice::from_ref(&chunk_block), theme) <= available
        {
            out.push(chunk_block);
        } else {
            let mid = chunk.items.len() / 2;
            let left = ListData {
                ordered: chunk.ordered,
                start: chunk.start,
                tight: chunk.tight,
                items: chunk.items[..mid].to_vec(),
            };
            let right = ListData {
                ordered: chunk.ordered,
                start: chunk.start + mid as u32,
                tight: chunk.tight,
                items: chunk.items[mid..].to_vec(),
            };
            out.extend(split_list(left, theme, available, depth - 1));
            out.extend(split_list(right, theme, available, depth - 1));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Blockquotes
// ---------------------------------------------------------------------------

fn split_quote(children: Vec<Block>, theme: &Theme, available: f32, depth: u32) -> Vec<Block> {
    if children.len() <= 1 || depth == 0 {
        return vec![Block::Quote(children)];
    }
    // Splitting cannot help when a single child already overflows.
    let smallest = children
        .iter()
        .map(|child| {
            measure(
                std::slice::from_ref(&Block::Quote(vec![child.clone()])),
                theme,
            )
        })
        .fold(f32::INFINITY, f32::min);
    if smallest > available {
        return vec![Block::Quote(children)];
    }
    let quote = Block::Quote(children.clone());
    if measure(std::slice::from_ref(&quote), theme) <= available {
        return vec![quote];
    }
    // Child heights inside a quote are narrower than outside it, so measure
    // the candidate quote itself and split by child count.
    let mid = children.len() / 2;
    let mut out = split_quote(children[..mid].to_vec(), theme, available, depth - 1);
    out.extend(split_quote(
        children[mid..].to_vec(),
        theme,
        available,
        depth - 1,
    ));
    out
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

fn split_table(table: TableData, theme: &Theme, available: f32, depth: u32) -> Vec<Block> {
    if table.rows.len() <= 1 || depth == 0 {
        return vec![Block::Table(table)];
    }
    let whole = Block::Table(table.clone());
    if measure(std::slice::from_ref(&whole), theme) <= available {
        return vec![whole];
    }
    // If header plus a single row already overflows, splitting cannot help.
    let one_row = TableData {
        aligns: table.aligns.clone(),
        head: table.head.clone(),
        rows: vec![table.rows[0].clone()],
    };
    if measure(std::slice::from_ref(&Block::Table(one_row)), theme) > available {
        return vec![whole];
    }
    let mid = table.rows.len() / 2;
    let top = TableData {
        aligns: table.aligns.clone(),
        head: table.head.clone(),
        rows: table.rows[..mid].to_vec(),
    };
    let bottom = TableData {
        aligns: table.aligns,
        head: table.head,
        rows: table.rows[mid..].to_vec(),
    };
    let mut out = split_table(top, theme, available, depth - 1);
    out.extend(split_table(bottom, theme, available, depth - 1));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn blocks(src: &str) -> Vec<Block> {
        parse(src).blocks
    }

    #[test]
    fn short_blocks_are_untouched() {
        let src = "A short paragraph.\n\n- one\n- two";
        let out = for_height(blocks(src), &Theme::light(), 1000.0);
        assert_eq!(out, blocks(src));
    }

    #[test]
    fn tall_code_blocks_split_at_line_boundaries() {
        let src = format!(
            "```rust\n{}\n```",
            (0..300)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let out = for_height(blocks(&src), &Theme::light(), 400.0);
        assert!(out.len() > 1);
        let total: usize = out
            .iter()
            .map(|b| match b {
                Block::Code(c) => c.lines.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(total, 300, "no line may be lost");
        assert!(
            out.iter()
                .all(|b| matches!(b, Block::Code(c) if c.info == "rust"))
        );
    }

    #[test]
    fn tall_lists_split_between_items_and_keep_numbering() {
        let src = (1..=80)
            .map(|i| format!("{i}. item {i}\n"))
            .collect::<String>();
        let theme = Theme::light();
        let out = for_height(blocks(&src), &theme, 300.0);
        assert!(out.len() > 1, "list should be split");

        let mut numbers = Vec::new();
        let mut items = 0;
        for block in &out {
            match block {
                Block::List(list) => {
                    numbers.push(list.start);
                    items += list.items.len();
                }
                other => panic!("expected lists, got {other:?}"),
            }
        }
        assert_eq!(items, 80, "no item may be lost");
        assert_eq!(numbers[0], 1);
        for pair in numbers.windows(2) {
            assert!(pair[1] > pair[0], "numbering must continue: {numbers:?}");
        }
        // Every chunk fits.
        for block in &out {
            assert!(measure(std::slice::from_ref(block), &theme) <= 300.0 + 0.5);
        }
    }

    #[test]
    fn tall_quotes_split_between_children() {
        let src = (0..40)
            .map(|i| format!("> paragraph {i} in a long quote\n>\n"))
            .collect::<String>();
        let theme = Theme::light();
        let out = for_height(blocks(&src), &theme, 250.0);
        assert!(out.len() > 1, "quote should be split");
        let total: usize = out
            .iter()
            .map(|b| match b {
                Block::Quote(children) => children.len(),
                _ => 0,
            })
            .sum();
        assert!(total >= 40, "children must be preserved");
        for block in &out {
            assert!(measure(std::slice::from_ref(block), &theme) <= 250.0 + 0.5);
        }
    }

    #[test]
    fn a_single_oversized_item_is_left_alone() {
        let src = "- a\n- b";
        let theme = Theme::light();
        let out = for_height(blocks(src), &theme, 1.0);
        // Nothing can be split usefully; the blocks survive.
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Block::List(l) if l.items.len() == 2));
    }

    #[test]
    fn tables_are_not_split() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |";
        let out = for_height(blocks(src), &Theme::light(), 1.0);
        assert!(matches!(&out[0], Block::Table(_)));
    }

    #[test]
    fn measurement_matches_layout() {
        let theme = Theme::light();
        let src = "# Head\n\nBody text.\n\n- a\n- b";
        let bs = blocks(src);
        let total = measure(&bs, &theme);
        assert!(total > 0.0);
        assert!(total < 400.0);
    }

    #[test]
    fn tall_tables_split_by_rows_and_repeat_the_header() {
        let mut src = String::from("| id | value |\n|---:|---|\n");
        for i in 0..120 {
            src.push_str(&format!("| {i} | row {i} |\n"));
        }
        let theme = Theme::light();
        let out = for_height(blocks(&src), &theme, 400.0);
        assert!(out.len() > 1, "table should be split");

        let mut rows = 0;
        for block in &out {
            match block {
                Block::Table(t) => {
                    assert_eq!(t.head.len(), 2, "header repeated");
                    rows += t.rows.len();
                }
                other => panic!("expected tables, got {other:?}"),
            }
            assert!(measure(std::slice::from_ref(block), &theme) <= 400.0 + 0.5);
        }
        assert_eq!(rows, 120, "no row may be lost");
    }

    #[test]
    fn a_table_that_cannot_fit_one_row_stays_whole() {
        let src = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |";
        let out = for_height(blocks(src), &Theme::light(), 1.0);
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Block::Table(t) if t.rows.len() == 2));
    }
}
