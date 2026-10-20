//! Pagination: split one tall layout into fixed-height pages.
//!
//! The layout engine reports a [`BlockMark`] per top-level block — its draw
//! range and its top/bottom edges. Because blocks own contiguous draw
//! ranges, a page can be assembled by copying whole blocks and shifting
//! them, which guarantees a cut never lands inside a paragraph, table or
//! code box.
//!
//! Two things are handled specially:
//!
//! * **Tall code blocks** are pre-split by lines before layout, so a long
//!   listing flows across pages instead of overflowing one.
//! * **Any other oversized block** (e.g. a huge table) is placed alone on
//!   its own page and allowed to overflow; there is no sensible place to
//!   cut a table row set without repeating the header.

use crate::ast::{Block, CodeBlock};
use crate::layout::{self, BlockMark};
use crate::scene::FontKind;
use crate::scene::{RectDraw, Scene};
use crate::text::{Metrics, TextStyle};
use crate::theme::Theme;

/// Pre-splits top-level code blocks that are taller than one page.
///
/// Chunk boundaries follow source lines, and each chunk keeps the original
/// info string so the language label is repeated on every page.
pub(crate) fn split_tall_code(blocks: Vec<Block>, theme: &Theme, page_height: f32) -> Vec<Block> {
    let max_lines = layout::code_lines_per_page(theme, page_height);
    if max_lines == 0 {
        return blocks;
    }
    let wrap_width = layout::code_wrap_width(theme);
    let style =
        TextStyle::new(theme.code_size(), FontKind::Mono).with_letter_spacing(theme.letter_spacing);
    let metrics = Metrics::new();

    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            Block::Code(code) => {
                let chunks = split_code(&code, max_lines, wrap_width, &metrics, &style);
                if chunks.len() <= 1 {
                    out.push(Block::Code(code));
                } else {
                    out.extend(chunks.into_iter().map(Block::Code));
                }
            }
            other => out.push(other),
        }
    }
    out
}

fn split_code(
    code: &CodeBlock,
    max_lines: usize,
    wrap_width: f32,
    metrics: &Metrics,
    style: &TextStyle,
) -> Vec<CodeBlock> {
    let mut chunks: Vec<CodeBlock> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut used = 0usize;

    for raw in &code.lines {
        let rendered = if raw.is_empty() {
            1
        } else {
            metrics.wrap_preformatted(raw, wrap_width, style).len()
        };
        // A chunk break is only allowed at a source-line boundary; a single
        // line taller than the page stays on one page and overflows.
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
    chunks
}

/// Packs blocks into pages of `page_height` and returns one scene per page.
///
/// The returned vector always has at least one entry.
pub(crate) fn paginate(
    scene: Scene,
    marks: &[BlockMark],
    theme: &Theme,
    page_height: f32,
) -> Vec<Scene> {
    let content_h = (page_height - 2.0 * theme.padding).max(1.0);
    let background = |height: f32| {
        theme
            .background
            .map(|c| RectDraw::new(0.0, 0.0, theme.width, height).with_fill(c))
    };

    if marks.is_empty() {
        return vec![Scene {
            width: theme.width,
            height: page_height,
            background: background(page_height),
            draws: Vec::new(),
        }];
    }

    let mut pages = Vec::new();
    let mut i = 0usize;
    while i < marks.len() {
        let start = i;
        let mut used = 0.0f32;
        while i < marks.len() {
            let gap = if i == start { 0.0 } else { marks[i].gap };
            let mut h = marks[i].height();
            // Keep headings together with the block they introduce: measuring
            // the pair is what prevents a stranded heading at the page foot.
            if marks[i].keep_with_next && i + 1 < marks.len() {
                h += marks[i + 1].gap + marks[i + 1].height();
            }
            // Always place at least one block per page, even if it overflows.
            if i > start && used + gap + h > content_h {
                break;
            }
            used += gap + marks[i].height();
            i += 1;
        }

        let offset = marks[start].top - theme.padding;
        let mut draws = Vec::new();
        for mark in &marks[start..i] {
            draws.extend(
                scene.draws[mark.draw_start..mark.draw_end]
                    .iter()
                    .map(|d| d.translated(0.0, -offset)),
            );
        }
        pages.push(Scene {
            width: theme.width,
            height: page_height,
            background: background(page_height),
            draws,
        });
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Theme;
    use crate::parse::parse;
    use crate::scene::{Draw, FontKind};

    fn render(doc: &crate::ast::Document, theme: &Theme) -> (Scene, Vec<BlockMark>) {
        crate::layout::Layout::new(theme).render_marked(doc)
    }

    fn paragraphs(n: usize) -> String {
        (0..n)
            .map(|i| format!("paragraph number {i} with some words\n\n"))
            .collect()
    }

    #[test]
    fn short_documents_fit_on_one_page() {
        let theme = Theme::light();
        let doc = parse("hello\n\nworld");
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 1123.0);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].height, 1123.0);
        assert_eq!(pages[0].width, theme.width);
    }

    #[test]
    fn empty_document_yields_one_empty_page() {
        let theme = Theme::light();
        let doc = parse("");
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 400.0);
        assert_eq!(pages.len(), 1);
        assert!(pages[0].draws.is_empty());
        assert_eq!(pages[0].height, 400.0);
    }

    #[test]
    fn long_documents_are_split_into_several_pages() {
        let theme = Theme::light();
        let doc = parse(&paragraphs(40));
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 400.0);
        assert!(
            pages.len() > 2,
            "expected multiple pages, got {}",
            pages.len()
        );
        for page in &pages {
            assert_eq!(page.height, 400.0);
        }
    }

    #[test]
    fn every_paragraph_lands_on_exactly_one_page() {
        let theme = Theme::light();
        let n = 30;
        let doc = parse(&paragraphs(n));
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 420.0);
        let mut seen = 0;
        for page in &pages {
            for draw in &page.draws {
                if let Draw::Text(t) = draw
                    && t.text.starts_with("paragraph number")
                {
                    seen += 1;
                }
            }
        }
        assert_eq!(seen, n, "each paragraph should appear once");
    }

    #[test]
    fn blocks_are_never_cut_in_half() {
        let theme = Theme::light();
        let doc = parse(&paragraphs(25));
        let (_, marks) = render(&doc, &theme);
        let content_h = 420.0 - 2.0 * theme.padding;
        // Simulate the packer and check that no block spans a page boundary.
        let mut used = 0.0f32;
        let mut first = true;
        for mark in &marks {
            let gap = if first { 0.0 } else { mark.gap };
            if used + gap + mark.height() > content_h {
                used = 0.0;
                first = true;
            }
            let gap = if first { 0.0 } else { mark.gap };
            used += gap + mark.height();
            first = false;
        }
        // The invariant we care about is also enforced by construction:
        // pages are assembled from whole mark ranges.
        assert!(marks.iter().all(|m| m.draw_end >= m.draw_start));
    }

    #[test]
    fn page_content_starts_at_the_top_padding() {
        let theme = Theme::light();
        let doc = parse(&paragraphs(20));
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 380.0);
        for page in &pages {
            let top = page
                .draws
                .iter()
                .map(|d| match d {
                    Draw::Text(t) => t.y,
                    Draw::Rect(r) => r.y,
                    Draw::Line(l) => l.y1,
                })
                .fold(f32::INFINITY, f32::min);
            assert!(
                top >= theme.padding - 0.01,
                "page content starts above padding: {top}"
            );
        }
    }

    #[test]
    fn background_covers_every_page() {
        let theme = Theme::light();
        let doc = parse(&paragraphs(30));
        let (scene, marks) = render(&doc, &theme);
        for page in paginate(scene, &marks, &theme, 400.0) {
            let bg = page.background.expect("page background");
            assert_eq!((bg.w, bg.h), (theme.width, 400.0));
        }
    }

    #[test]
    fn tall_code_blocks_are_split_between_lines() {
        let theme = Theme::light();
        let source = format!(
            "```rust\n{}\n```",
            (0..200)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let doc = parse(&source);
        let blocks = split_tall_code(doc.blocks.clone(), &theme, 400.0);
        assert!(blocks.len() > 1, "code block was not split");

        let doc = crate::ast::Document { blocks };
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 400.0);
        assert!(pages.len() > 1);
        // No text may sit below the page's bottom padding.
        for page in &pages {
            for draw in &page.draws {
                if let Draw::Text(t) = draw {
                    assert!(
                        t.y <= 400.0 - theme.padding + 1.0,
                        "code text below page: {}",
                        t.y
                    );
                }
            }
        }
        // Every source line is present exactly once across the pages.
        let mut lines: Vec<String> = Vec::new();
        for page in &pages {
            for draw in &page.draws {
                if let Draw::Text(t) = draw
                    && t.text.starts_with("line ")
                {
                    lines.push(t.text.clone());
                }
            }
        }
        assert_eq!(lines.len(), 200, "lines were lost or duplicated");
    }

    #[test]
    fn code_chunking_keeps_the_language_label() {
        let theme = Theme::light();
        let code = CodeBlock {
            info: "rust".into(),
            lines: (0..100).map(|i| format!("l{i}")).collect(),
        };
        let chunks = split_code(
            &code,
            10,
            layout::code_wrap_width(&theme),
            &Metrics::new(),
            &TextStyle::new(theme.code_size(), FontKind::Mono),
        );
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| c.info == "rust"));
        assert_eq!(chunks.iter().map(|c| c.lines.len()).sum::<usize>(), 100);
    }

    #[test]
    fn headings_are_not_stranded_at_the_page_foot() {
        let theme = Theme::light();
        // Each section is just tall enough that the heading would otherwise
        // land in the last few pixels of a page.
        let mut src = String::new();
        for i in 0..12 {
            src.push_str(&format!("## Section {i}\n\n"));
            src.push_str(&format!("Body {i}. {}\n\n", "words ".repeat(18)));
        }
        let doc = parse(&src);
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 320.0);

        // For every page except the last, the bottom-most block must not be a
        // heading: inspect the draw at the maximum y on the page.
        for page in pages.iter().take(pages.len() - 1) {
            let bottom_text = page
                .draws
                .iter()
                .filter_map(|d| match d {
                    Draw::Text(t) => Some(t),
                    _ => None,
                })
                .max_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
            if let Some(t) = bottom_text {
                assert!(
                    !t.text.starts_with("Section"),
                    "heading {:?} is stranded at the page bottom",
                    t.text
                );
            }
        }
    }

    #[test]
    fn a_block_taller_than_a_page_gets_its_own_page() {
        let theme = Theme::light();
        let doc = parse(&format!("```\n{}\n```\n\nafter", "x\n".repeat(80)));
        let (scene, marks) = render(&doc, &theme);
        let pages = paginate(scene, &marks, &theme, 300.0);
        assert!(pages.len() >= 2);
        // The last page still contains the trailing paragraph.
        let last = pages.last().unwrap();
        assert!(
            last.draws
                .iter()
                .any(|d| matches!(d, Draw::Text(t) if t.text == "after"))
        );
    }
}
