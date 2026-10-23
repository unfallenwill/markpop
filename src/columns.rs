//! Multi-column flow.
//!
//! The layout engine lays every top-level block out **once**, in a single
//! column of `theme.column_width()`, and records a [`BlockMark`] per block.
//! This module then distributes those finished blocks across columns — it
//! never re-lays-out content, so block geometry is identical to the
//! single-column case and only the offsets change.
//!
//! Two modes:
//!
//! * **Balanced** (no page height): find the smallest column height that
//!   still fits the document in `n` columns, then fill each column to that
//!   height. This is the newspaper/CSS `column-count` look.
//! * **Paginated**: columns are exactly the page's content height; blocks
//!   that do not fit move to the next column, and every `n` columns start a
//!   new page.

use crate::layout::BlockMark;
use crate::scene::{RectDraw, Scene};
use crate::theme::Theme;

/// Packs blocks into columns of at most `col_height`.
///
/// * a block that does not fit continues in the next column;
/// * a heading is kept with the block that follows it;
/// * a block taller than the column is placed alone and the next column is
///   started afterwards, so it can never overlap a neighbour.
pub(crate) fn pack(marks: &[BlockMark], col_height: f32) -> Vec<Vec<usize>> {
    let mut columns: Vec<Vec<usize>> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    let mut used = 0.0f32;

    for (i, mark) in marks.iter().enumerate() {
        let gap = if current.is_empty() { 0.0 } else { mark.gap };
        let mut needed = mark.height();
        if mark.keep_with_next && i + 1 < marks.len() {
            needed += marks[i + 1].gap + marks[i + 1].height();
        }
        if !current.is_empty() && used + gap + needed > col_height {
            columns.push(std::mem::take(&mut current));
            used = 0.0;
        }
        used += if current.is_empty() { 0.0 } else { gap } + mark.height();
        current.push(i);
        if used > col_height {
            // Oversized block: give it a column of its own.
            columns.push(std::mem::take(&mut current));
            used = 0.0;
        }
    }
    if !current.is_empty() {
        columns.push(current);
    }
    if columns.is_empty() {
        columns.push(Vec::new());
    }
    columns
}

/// Height actually occupied by one column.
pub(crate) fn used_height(marks: &[BlockMark], column: &[usize]) -> f32 {
    let mut height = 0.0f32;
    for (i, &block) in column.iter().enumerate() {
        if i > 0 {
            height += marks[block].gap;
        }
        height += marks[block].height();
    }
    height
}

/// Finds the shortest column height that fits the document in `ncols`
/// columns, and returns the columns plus the height they occupy.
///
/// The packing is monotone in the column height, so a binary search finds
/// the balanced height in ~48 iterations of a linear pass.
pub(crate) fn balanced(marks: &[BlockMark], ncols: usize) -> (Vec<Vec<usize>>, f32) {
    if marks.is_empty() {
        return (vec![Vec::new()], 0.0);
    }
    let ncols = ncols.max(1);
    let tallest = marks.iter().map(|m| m.height()).fold(1.0f32, f32::max);
    let mut lo = tallest;
    let mut hi = tallest;
    for _ in 0..64 {
        if pack(marks, hi).len() <= ncols {
            break;
        }
        hi *= 1.5;
    }
    let mut best = pack(marks, hi);
    for _ in 0..48 {
        let mid = (lo + hi) * 0.5;
        if mid <= lo || mid >= hi {
            break;
        }
        let columns = pack(marks, mid);
        if columns.len() <= ncols {
            hi = mid;
            best = columns;
        } else {
            lo = mid;
        }
    }
    let used = best
        .iter()
        .map(|c| used_height(marks, c))
        .fold(0.0f32, f32::max);
    (best, used)
}

/// Builds one scene from a group of columns.
///
/// `height` is the page/canvas height; every column starts at the top
/// padding, and column `i` is shifted right by `i * (column width + gap)`.
pub(crate) fn build_scene(
    source: &Scene,
    marks: &[BlockMark],
    columns: &[Vec<usize>],
    theme: &Theme,
    height: f32,
) -> Scene {
    let column_width = theme.column_width();
    let mut draws = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let Some(&first) = column.first() else {
            continue;
        };
        let dx = index as f32 * (column_width + theme.column_gap);
        let dy = theme.padding - marks[first].top;
        for &block in column {
            let mark = &marks[block];
            for draw in &source.draws[mark.draw_start..mark.draw_end] {
                draws.push(draw.translated(dx, dy));
            }
        }
    }
    Scene {
        width: theme.width,
        height,
        background: theme
            .background
            .map(|color| RectDraw::new(0.0, 0.0, theme.width, height).with_fill(color)),
        draws,
        images: source.images.clone(),
    }
}

/// Lays a marked scene out into columns, optionally paginating them.
pub(crate) fn layout_columns(
    source: Scene,
    marks: &[BlockMark],
    theme: &Theme,
    page_height: Option<f32>,
) -> Vec<Scene> {
    let ncols = theme.columns.max(1) as usize;
    match page_height {
        Some(height) => {
            let column_height = (height - 2.0 * theme.padding).max(1.0);
            let columns = pack(marks, column_height);
            columns
                .chunks(ncols)
                .map(|group| build_scene(&source, marks, group, theme, height))
                .collect()
        }
        None => {
            let (columns, used) = balanced(marks, ncols);
            let height = used + 2.0 * theme.padding;
            vec![build_scene(&source, marks, &columns, theme, height)]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use crate::parse::parse;

    fn marked(theme: &Theme, src: &str) -> (Scene, Vec<BlockMark>) {
        crate::layout::Layout::new(theme).render_marked(&parse(src))
    }

    fn paragraphs(n: usize) -> String {
        (0..n)
            .map(|i| format!("Paragraph {i} with a few words in it.\n\n"))
            .collect()
    }

    #[test]
    fn packing_respects_the_column_height() {
        let theme = Theme::light();
        let (_, marks) = marked(&theme, &paragraphs(12));
        let columns = pack(&marks, 300.0);
        for column in &columns {
            // Individual oversized blocks are allowed, but every multi-block
            // column must fit.
            if column.len() > 1 {
                assert!(
                    used_height(&marks, column) <= 300.0 + 0.01,
                    "column of {} blocks is too tall",
                    column.len()
                );
            }
        }
        assert!(columns.len() > 1);
    }

    #[test]
    fn headings_travel_with_their_section() {
        let theme = Theme::light();
        let src = format!("{}## Section\n\n{}", paragraphs(3), paragraphs(3));
        let (_, marks) = marked(&theme, &src);
        let heading = marks
            .iter()
            .position(|m| m.keep_with_next)
            .expect("a heading");

        // The heading plus its first paragraph is about 80pt tall; use heights
        // that can hold the pair.
        for height in [90.0f32, 100.0, 120.0, 160.0] {
            let columns = pack(&marks, height);
            for column in &columns {
                if let Some(pos) = column.iter().position(|&i| i == heading) {
                    assert!(
                        pos + 1 < column.len(),
                        "heading stranded at the foot of a column (height {height})"
                    );
                }
            }
        }
    }

    #[test]
    fn packing_handles_oversized_blocks_without_overlap() {
        let theme = Theme::light();
        let src = format!(
            "{}\n{}{}",
            paragraphs(2),
            "# ".to_string() + &"big ".repeat(200),
            paragraphs(2)
        );
        let (_, marks) = marked(&theme, &src);
        let columns = pack(&marks, 100.0);
        // The huge heading gets a column of its own.
        let big = columns
            .iter()
            .find(|c| c.contains(&2))
            .expect("big block placed");
        assert_eq!(big.len(), 1, "oversized block must be alone");
    }

    #[test]
    fn balanced_columns_fit_the_requested_count() {
        let theme = Theme::light().with_columns(3);
        let (source, marks) = marked(&theme, &paragraphs(30));
        let (columns, _) = balanced(&marks, 3);
        assert!(columns.len() <= 3, "got {} columns", columns.len());
        let scenes = layout_columns(source, &marks, &theme, None);
        assert_eq!(scenes.len(), 1);
    }

    #[test]
    fn balanced_columns_are_roughly_equal() {
        let theme = Theme::light().with_columns(2);
        let (_, marks) = marked(&theme, &paragraphs(20));
        let (columns, _) = balanced(&marks, 2);
        let heights: Vec<f32> = columns.iter().map(|c| used_height(&marks, c)).collect();
        let max = heights.iter().cloned().fold(0.0f32, f32::max);
        let min = heights.iter().cloned().fold(f32::INFINITY, f32::min);
        assert!(min > 0.0);
        assert!(max - min < 120.0, "columns are unbalanced: {heights:?}");
    }

    #[test]
    fn every_block_lands_in_exactly_one_column() {
        let theme = Theme::light().with_columns(3);
        let (_, marks) = marked(&theme, &paragraphs(25));
        let (columns, _) = balanced(&marks, 3);
        let mut seen: Vec<usize> = columns.iter().flatten().copied().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..marks.len()).collect::<Vec<_>>());
    }

    #[test]
    fn column_two_starts_right_of_column_one() {
        let theme = Theme::light().with_columns(2);
        let (source, marks) = marked(&theme, &paragraphs(20));
        let scene = layout_columns(source, &marks, &theme, None).remove(0);
        let texts: Vec<&crate::scene::TextDraw> = scene
            .draws
            .iter()
            .filter_map(|d| match d {
                crate::scene::Draw::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        let mid = theme.padding + theme.column_width();
        assert!(texts.iter().any(|t| t.x < mid), "no text in column 1");
        assert!(texts.iter().any(|t| t.x >= mid), "no text in column 2");
    }

    #[test]
    fn columns_stay_inside_their_own_column() {
        let theme = Theme::light().with_columns(3);
        let src = (0..12)
            .map(|i| {
                format!("**Bold {i}** and `code {i}` and a [link](https://x.y) with words.\n\n")
            })
            .collect::<String>();
        let (source, marks) = marked(&theme, &src);
        let scene = layout_columns(source, &marks, &theme, None).remove(0);
        let width = theme.column_width();
        for draw in &scene.draws {
            let x = match draw {
                crate::scene::Draw::Text(t) => t.x,
                crate::scene::Draw::Rect(r) => r.x,
                crate::scene::Draw::Line(l) => l.x1,
                crate::scene::Draw::Image(_) => continue,
            };
            // x must fall inside one of the column slots.
            let mut inside = false;
            for c in 0..theme.columns as usize {
                let left = theme.padding + c as f32 * (width + theme.column_gap);
                if x >= left - 1.0 && x <= left + width + 1.0 {
                    inside = true;
                    break;
                }
            }
            assert!(inside, "draw at x={x} is between columns");
        }
    }

    #[test]
    fn paginated_columns_group_into_pages() {
        let theme = Theme::light().with_columns(2);
        let (source, marks) = marked(&theme, &paragraphs(40));
        let pages = layout_columns(source, &marks, &theme, Some(420.0));
        assert!(pages.len() > 1, "expected multiple pages");
        for page in &pages {
            assert_eq!(page.height, 420.0);
            assert_eq!(page.width, 800.0);
        }
    }

    #[test]
    fn empty_documents_still_produce_one_scene() {
        let theme = Theme::light().with_columns(2);
        let (source, marks) = marked(&theme, "");
        let scenes = layout_columns(source, &marks, &theme, None);
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0].draws.len(), 0);
        let pages = layout_columns(
            marked(&theme, "").0,
            &marked(&theme, "").1,
            &theme,
            Some(400.0),
        );
        assert_eq!(pages.len(), 1);
    }
}
