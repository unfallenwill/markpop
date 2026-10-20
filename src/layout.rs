//! Layout: turns the AST into a [`Scene`] of positioned primitives.
//!
//! The engine is a recursive vertical flow. Each block reports its height
//! and appends primitives; containers (blockquotes, list items, table
//! cells) simply re-enter the flow with a narrower column.
//!
//! Two ideas keep this small and testable:
//!
//! * **Margin collapsing**: a block declares `(before, after)` margins, and
//!   the flow inserts `max(prev_after, next_before)` between neighbours.
//! * **Atoms**: inline content is flattened into style-tagged words and
//!   wide characters, which are then greedily wrapped. All measurement goes
//!   through [`Metrics`], so layout is deterministic.

use crate::ast::{Align, Block, CodeBlock, Document, ImageData, Inline, ListData, TableData};
use crate::color::Color;
use crate::scene::{Draw, FontKind, LineDraw, RectDraw, Scene, TextDraw};
use crate::text::{Metrics, TextStyle, atomize};
use crate::theme::Theme;

/// Vertical rhythm constants, expressed in multiples of the base font size.
mod space {
    /// Margin above and below a paragraph.
    pub const PARAGRAPH: f32 = 0.75;
    /// Margin above a heading, by level (h1 has the most air).
    pub const HEADING_BEFORE: [f32; 6] = [1.6, 1.4, 1.2, 1.0, 1.0, 1.0];
    /// Margin below a heading (smaller than above: headings bind downward).
    pub const HEADING_AFTER: f32 = 0.55;
    /// Margin around code blocks, quotes, tables and figures.
    pub const BLOCK: f32 = 1.0;
    /// Margin around thematic breaks.
    pub const RULE: f32 = 1.4;
    /// Gap between items of a loose list.
    pub const LOOSE_ITEM: f32 = 0.45;
}

/// Internal code-block metrics, in base units.
mod code {
    pub const PAD_X: f32 = 12.0;
    pub const PAD_Y: f32 = 10.0;
    pub const RADIUS: f32 = 6.0;
    pub const LABEL_GAP: f32 = 6.0;
}

/// Width available to code lines (content column minus the code box's
/// horizontal padding).
pub(crate) fn code_wrap_width(theme: &Theme) -> f32 {
    (theme.column_width() - 2.0 * code::PAD_X).max(1.0)
}

/// How many *rendered* code lines fit on one page of `page_height`.
///
/// Used when paginating long code blocks; the label row and the box
/// padding are subtracted so a chunk never spills onto the next page.
pub(crate) fn code_lines_per_page(theme: &Theme, page_height: f32) -> usize {
    let content_h = (page_height - 2.0 * theme.padding).max(1.0);
    let line_h = (theme.code_size() * theme.code_line_height).max(1.0);
    let chrome = 2.0 * code::PAD_Y + theme.font_size * 0.75 + code::LABEL_GAP;
    ((content_h - chrome).max(line_h) / line_h).floor() as usize
}

/// Blockquote metrics, in base units.
mod quote {
    pub const BAR_WIDTH: f32 = 4.0;
    pub const PAD_LEFT: f32 = 16.0;
    pub const PAD_RIGHT: f32 = 12.0;
    pub const PAD_Y: f32 = 8.0;
    /// Vertical padding for quotes nested inside another quote.
    pub const PAD_Y_NESTED: f32 = 2.0;
}

/// Table metrics, in base units.
mod table {
    pub const PAD_X: f32 = 10.0;
    pub const PAD_Y: f32 = 7.0;
    pub const MIN_COL: f32 = 36.0;
    pub const BORDER: f32 = 1.0;
}

/// Properties that apply to a whole inline flow (a paragraph or heading).
#[derive(Debug, Clone, Copy)]
struct InlineStyle {
    size: f32,
    bold: bool,
    italic: bool,
    color: Color,
    underline: bool,
    strike: bool,
}

impl InlineStyle {
    fn new(size: f32, color: Color) -> InlineStyle {
        InlineStyle {
            size,
            bold: false,
            italic: false,
            color,
            underline: false,
            strike: false,
        }
    }
}

/// A styled span of text, after inline flattening.
#[derive(Debug, Clone)]
struct Frag {
    text: String,
    style: TextStyle,
    color: Color,
    underline: bool,
    strike: bool,
    /// Rendered as an inline-code chip (background + padding).
    code: bool,
}

impl Frag {
    fn new(text: impl Into<String>, style: TextStyle, color: Color) -> Frag {
        Frag {
            text: text.into(),
            style,
            color,
            underline: false,
            strike: false,
            code: false,
        }
    }
}

/// An unbreakable piece of a line.
#[derive(Debug, Clone)]
struct Atom {
    frag: Frag,
    text: String,
    /// Advance width including chip padding.
    width: f32,
    /// Horizontal padding on each side (inline code only).
    pad: f32,
    space: bool,
    hard_break: bool,
}

/// Safety margin applied to text wrapping.
///
/// Our advance table cannot know which font the viewer will pick from the
/// stack, and the widest common faces are a few percent wider than the
/// estimate. Wrapping slightly early keeps real output inside the canvas
/// without visibly shortening lines.
const FONT_SLACK: f32 = 0.98;

/// One laid-out line: its atoms.
struct Line {
    atoms: Vec<Atom>,
    width: f32,
}

/// Arguments for emitting a group of lines.
struct LineBox<'b> {
    lines: &'b [Line],
    x: f32,
    top: f32,
    line_h: f32,
    size: f32,
    align: Align,
    /// Width the lines may be aligned within.
    avail: f32,
    /// Force bold on every run (table headers).
    bold: bool,
}

/// Arguments shared by every cell of one table row.
struct RowBox<'r> {
    cols: &'r [f32],
    aligns: &'r [Align],
    x: f32,
    top: f32,
    line_h: f32,
    size: f32,
    bold: bool,
}

/// Where a top-level block landed in the draw list and on the canvas.
///
/// Pagination uses these records to move whole blocks between pages, so a
/// cut never lands in the middle of a paragraph, table or code box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BlockMark {
    /// First index into [`Layout::draws`] belonging to the block.
    pub draw_start: usize,
    /// One past the last draw index belonging to the block.
    pub draw_end: usize,
    /// Top edge, in base units.
    pub top: f32,
    /// Bottom edge, in base units.
    pub bottom: f32,
    /// Collapsed margin between the previous block and this one.
    pub gap: f32,
    /// Whether this block must stay on the same page as the next one
    /// (headings must not be stranded at the bottom of a page).
    pub keep_with_next: bool,
}

impl BlockMark {
    /// Height of the block's content.
    pub fn height(&self) -> f32 {
        self.bottom - self.top
    }
}

/// The flow engine.
pub(crate) struct Layout<'a> {
    theme: &'a Theme,
    metrics: Metrics,
    draws: Vec<Draw>,
    /// Top of the next block.
    y: f32,
    /// How many blockquotes enclose the current position (0 = top level).
    quote_depth: u32,
}

impl<'a> Layout<'a> {
    /// Creates a layout positioned at the theme's top padding.
    pub(crate) fn new(theme: &'a Theme) -> Layout<'a> {
        Layout {
            theme,
            metrics: Metrics::new(),
            draws: Vec::new(),
            y: theme.padding,
            quote_depth: 0,
        }
    }

    /// Like [`Self::render`], but also reports where each top-level block
    /// ended up, which is what pagination needs in order to cut between
    /// blocks without slicing through them.
    pub(crate) fn render_marked(mut self, doc: &Document) -> (Scene, Vec<BlockMark>) {
        let marks = self.lay_top(&doc.blocks);
        let height = (self.y + self.theme.padding).max(self.theme.padding * 2.0);
        let scene = Scene {
            width: self.theme.width,
            height,
            background: self
                .theme
                .background
                .map(|c| RectDraw::new(0.0, 0.0, self.theme.width, height).with_fill(c)),
            draws: self.draws,
        };
        (scene, marks)
    }

    /// Lays out top-level blocks while recording their draw ranges.
    fn lay_top(&mut self, blocks: &[Block]) -> Vec<BlockMark> {
        let x = self.theme.padding;
        let width = self.content_width();
        let mut marks = Vec::with_capacity(blocks.len());
        let mut prev_after: Option<f32> = None;
        for block in blocks {
            let (before, after) = self.margins(block, false);
            let gap = prev_after.map_or(0.0, |pa| pa.max(before));
            let draw_start = self.draws.len();
            self.y += gap;
            let top = self.y;
            self.block(block, x, width);
            marks.push(BlockMark {
                draw_start,
                draw_end: self.draws.len(),
                top,
                bottom: self.y,
                gap,
                keep_with_next: matches!(block, Block::Heading { .. }),
            });
            prev_after = Some(after);
        }
        marks
    }

    /// Width available to top-level block content (one column).
    fn content_width(&self) -> f32 {
        self.theme.column_width()
    }

    // -----------------------------------------------------------------
    // Block flow
    // -----------------------------------------------------------------

    /// Lays out blocks vertically at `x` with the given content `width`.
    ///
    /// The first block's top margin and the last block's bottom margin are
    /// dropped: containers provide their own padding.
    fn blocks(&mut self, blocks: &[Block], x: f32, width: f32, tight: bool) {
        let mut prev_after: Option<f32> = None;
        for block in blocks {
            let (before, after) = self.margins(block, tight);
            if let Some(pa) = prev_after {
                self.y += pa.max(before);
            }
            self.block(block, x, width);
            prev_after = Some(after);
        }
    }

    /// `(margin_before, margin_after)` for a block.
    fn margins(&self, block: &Block, tight: bool) -> (f32, f32) {
        let em = self.theme.font_size;
        match block {
            Block::Heading { level, .. } => {
                let i = (*level).clamp(1, 6) as usize - 1;
                (em * space::HEADING_BEFORE[i], em * space::HEADING_AFTER)
            }
            Block::Paragraph(_) => {
                if tight {
                    (0.0, 0.0)
                } else {
                    (em * space::PARAGRAPH, em * space::PARAGRAPH)
                }
            }
            Block::ThematicBreak => (em * space::RULE, em * space::RULE),
            Block::List(_) if tight => (0.0, 0.0),
            Block::List(l) if !l.items.is_empty() => (em * space::BLOCK, em * space::BLOCK),
            Block::List(_) => (0.0, 0.0),
            _ => (em * space::BLOCK, em * space::BLOCK),
        }
    }

    /// Lays out a single block, advancing `self.y` to its bottom edge.
    fn block(&mut self, block: &Block, x: f32, width: f32) {
        match block {
            Block::Heading { level, inlines } => self.heading(*level, inlines, x, width),
            Block::Paragraph(inlines) => {
                let size = self.theme.font_size;
                let style = InlineStyle::new(size, self.theme.text);
                self.paragraph(inlines, x, width, style, Align::None);
            }
            Block::Code(code) => self.code_block(code, x, width),
            Block::Quote(inner) => self.quote(inner, x, width),
            Block::List(list) => self.list(list, x, width),
            Block::Table(t) => self.table(t, x, width),
            Block::Figure(img) => self.figure(img, x, width),
            Block::ThematicBreak => self.rule(x, width),
        }
    }

    fn heading(&mut self, level: u8, inlines: &[Inline], x: f32, width: f32) {
        let size = self.theme.heading_size(level);
        let mut style = InlineStyle::new(size, self.theme.heading);
        style.bold = true;
        let has_rule = level <= self.theme.heading_rule_below;
        let rule_gap = size * 0.35;
        let rule_space = if has_rule { rule_gap + 3.0 } else { 0.0 };

        let top = self.y;
        let lines = self.compose(inlines, width, style);
        let line_h = self.line_height(size, true);
        let text_h = self.emit(LineBox {
            lines: &lines,
            x,
            top,
            line_h,
            size,
            align: Align::None,
            avail: width,
            bold: false,
        });

        if has_rule {
            let rule_y = top + text_h + rule_gap;
            let thickness = if level <= 1 { 2.0 } else { 1.0 };
            self.draws.push(Draw::Line(LineDraw {
                x1: x,
                y1: rule_y,
                x2: x + width,
                y2: rule_y,
                color: self.theme.border,
                width: thickness,
            }));
        }
        self.y = top + text_h + rule_space;
    }

    fn paragraph(
        &mut self,
        inlines: &[Inline],
        x: f32,
        width: f32,
        style: InlineStyle,
        align: Align,
    ) -> f32 {
        let lines = self.compose(inlines, width, style);
        let line_h = self.line_height(style.size, false);
        let top = self.y;
        let height = self.emit(LineBox {
            lines: &lines,
            x,
            top,
            line_h,
            size: style.size,
            align,
            avail: width,
            bold: false,
        });
        self.y = top + height;
        height
    }

    fn code_block(&mut self, code: &CodeBlock, x: f32, width: f32) {
        let size = self.theme.code_size();
        let style = self.text_style(size, FontKind::Mono);
        let line_h = size * self.theme.code_line_height;
        let inner_w = (width - 2.0 * code::PAD_X).max(1.0);

        let mut lines: Vec<String> = Vec::new();
        for raw in &code.lines {
            if raw.is_empty() {
                lines.push(String::new());
            } else {
                lines.extend(self.metrics.wrap_preformatted(raw, inner_w, &style));
            }
        }
        if lines.is_empty() {
            lines.push(String::new());
        }

        let label_h = if code.info.is_empty() {
            0.0
        } else {
            self.theme.font_size * 0.75 + code::LABEL_GAP
        };
        let content_h = lines.len() as f32 * line_h;
        let height = content_h + 2.0 * code::PAD_Y + label_h;

        self.draws.push(Draw::Rect(
            RectDraw::new(x, self.y, width, height)
                .with_fill(self.theme.code_bg)
                .with_radius(code::RADIUS),
        ));

        if !code.info.is_empty() {
            let label_size = self.theme.font_size * 0.7;
            let label = code.info.clone();
            let label_w = self
                .metrics
                .measure(&label, &self.text_style(label_size, FontKind::Mono));
            self.draws.push(Draw::Text(TextDraw {
                x: x + width - code::PAD_X - label_w,
                y: self.y + code::PAD_Y * 0.5 + label_size * 0.8,
                text: label,
                size: label_size,
                family: FontKind::Mono,
                color: self.theme.muted,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: self.theme.letter_spacing * label_size,
            }));
        }

        let text_top = self.y + code::PAD_Y + label_h;
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            self.draws.push(Draw::Text(TextDraw {
                x: x + code::PAD_X,
                y: text_top + i as f32 * line_h + (line_h - size) * 0.5 + size * 0.8,
                text: line.clone(),
                size,
                family: FontKind::Mono,
                color: self.theme.code_text,
                bold: false,
                italic: false,
                underline: false,
                strike: false,
                letter_spacing: self.theme.letter_spacing * size,
            }));
        }
        self.y += height;
    }

    fn quote(&mut self, inner: &[Block], x: f32, width: f32) {
        let top = self.y;
        let content_x = x + quote::PAD_LEFT;
        let content_w = (width - quote::PAD_LEFT - quote::PAD_RIGHT).max(1.0);
        // Nested quotes hug their content and fade toward the page color, so
        // depth reads at a glance without extra colors in the theme.
        let depth = self.quote_depth;
        let pad_y = if depth == 0 {
            quote::PAD_Y
        } else {
            quote::PAD_Y_NESTED
        };
        let backdrop = match self.theme.background {
            Some(bg) => self.theme.quote_bg.blend(bg, 0.4 * depth.min(3) as f32),
            None => self.theme.quote_bg,
        };

        // Reserve the backdrop before laying out the children: the scene is
        // painted in insertion order, so a background emitted afterwards
        // would cover the text it is supposed to sit behind. Heights are
        // patched in once the content has been measured.
        let bg_idx = self.draws.len();
        self.draws.push(Draw::Rect(
            RectDraw::new(x, top, width, 0.0).with_fill(backdrop),
        ));
        let bar_idx = self.draws.len();
        self.draws.push(Draw::Rect(
            RectDraw::new(x, top, quote::BAR_WIDTH, 0.0).with_fill(self.theme.quote_bar),
        ));

        self.quote_depth += 1;
        self.y = top + pad_y;
        self.blocks(inner, content_x, content_w, false);
        self.y += pad_y;
        self.quote_depth -= 1;

        let height = self.y - top;
        self.draws[bg_idx] = Draw::Rect(RectDraw::new(x, top, width, height).with_fill(backdrop));
        self.draws[bar_idx] = Draw::Rect(
            RectDraw::new(x, top, quote::BAR_WIDTH, height).with_fill(self.theme.quote_bar),
        );
    }

    fn list(&mut self, list: &ListData, x: f32, width: f32) {
        let size = self.theme.font_size;
        let marker_size = size;
        let marker_w = if list.ordered {
            let widest = (list.start + list.items.len().saturating_sub(1) as u32)
                .to_string()
                .len();
            let sample = format!("{}.", "0".repeat(widest));
            self.metrics
                .measure(&sample, &self.text_style(marker_size, FontKind::Sans))
                + 10.0
        } else {
            size * 1.5
        };
        // Never let the marker eat the whole column: with pathological
        // nesting the marker column collapses instead of pushing text past
        // the right edge.
        let marker_w = marker_w.min((width - size).max(0.0));
        let content_x = x + marker_w;
        let content_w = (width - marker_w).max(1.0);

        for (i, item) in list.items.iter().enumerate() {
            let item_top = self.y;
            let number = list.start + i as u32;
            let marker_baseline = item_top + size * 0.8;

            match list.ordered {
                true => {
                    let text = format!("{number}.");
                    let w = self
                        .metrics
                        .measure(&text, &self.text_style(marker_size, FontKind::Sans));
                    self.draws.push(Draw::Text(TextDraw {
                        x: content_x - 10.0 - w,
                        y: marker_baseline,
                        text,
                        size: marker_size,
                        family: FontKind::Sans,
                        color: self.theme.text,
                        bold: false,
                        italic: false,
                        underline: false,
                        strike: false,
                        letter_spacing: self.theme.letter_spacing * marker_size,
                    }));
                }
                false => {
                    let r = (size * 0.16).max(2.0);
                    let cx = x + marker_w * 0.5 - r;
                    let cy = marker_baseline - size * 0.3 - r;
                    self.draws.push(Draw::Rect(
                        RectDraw::new(cx, cy, r * 2.0, r * 2.0)
                            .with_fill(self.theme.bullet)
                            .with_radius(r),
                    ));
                }
            }

            self.blocks(&item.blocks, content_x, content_w, list.tight);
            if !list.tight && i + 1 < list.items.len() {
                self.y += size * space::LOOSE_ITEM;
            }
        }
    }

    fn table(&mut self, data: &TableData, x: f32, width: f32) {
        let size = self.theme.font_size * 0.92;
        let style = InlineStyle::new(size, self.theme.text);
        let ncols = data.aligns.len().max(1);

        // Natural width of each column = widest single-line cell.
        // Header cells are emitted bold, so they must be measured bold
        // as well, and the wrap safety margin has to be part of the
        // natural width: otherwise a column that exactly fits would wrap.
        let mut natural = vec![table::MIN_COL; ncols];
        let mut head_style = style;
        head_style.bold = true;
        for (c, cell) in data.head.iter().enumerate().take(ncols) {
            natural[c] = natural[c].max(self.column_width(cell, head_style));
        }
        for row in &data.rows {
            for (c, cell) in row.iter().enumerate().take(ncols) {
                natural[c] = natural[c].max(self.column_width(cell, style));
            }
        }

        let cols = fit_columns(&natural, width);
        let table_w: f32 = cols.iter().sum::<f32>().min(width);

        let line_h = self.line_height(size, false);
        let top = self.y;

        // Header background spans the table width.
        let head_lines = self.compose_table_row(&data.head, &cols, style, ncols);
        let head_rows = head_lines.iter().map(|c| c.len()).max().unwrap_or(1);
        let head_h = head_rows as f32 * line_h + 2.0 * table::PAD_Y;

        self.draws.push(Draw::Rect(
            RectDraw::new(x, top, table_w, head_h).with_fill(self.theme.table_header_bg),
        ));
        self.emit_row(
            &head_lines,
            RowBox {
                cols: &cols,
                aligns: &data.aligns,
                x,
                top,
                line_h,
                size,
                bold: true,
            },
        );
        self.draws.push(Draw::Line(LineDraw {
            x1: x,
            y1: top,
            x2: x + table_w,
            y2: top,
            color: self.theme.border,
            width: table::BORDER,
        }));

        let mut row_y = top + head_h;
        self.separator(x, table_w, row_y);

        for row in &data.rows {
            let cells = self.compose_table_row(row, &cols, style, ncols);
            let row_lines = cells.iter().map(|c| c.len()).max().unwrap_or(1);
            let h = row_lines as f32 * line_h + 2.0 * table::PAD_Y;
            self.emit_row(
                &cells,
                RowBox {
                    cols: &cols,
                    aligns: &data.aligns,
                    x,
                    top: row_y,
                    line_h,
                    size,
                    bold: false,
                },
            );
            row_y += h;
            self.separator(x, table_w, row_y);
        }

        self.y = row_y;
    }

    /// Lays out a table row's cells into wrapped lines (one `Vec<Line>`
    /// per column).
    fn compose_table_row(
        &self,
        cells: &[Vec<Inline>],
        cols: &[f32],
        style: InlineStyle,
        ncols: usize,
    ) -> Vec<Vec<Line>> {
        let mut out = Vec::with_capacity(ncols);
        for (c, col) in cols.iter().enumerate().take(ncols) {
            let empty: Vec<Inline> = Vec::new();
            let cell = cells.get(c).unwrap_or(&empty);
            let avail = (col - 2.0 * table::PAD_X).max(1.0);
            out.push(self.compose_padded(cell, avail, style));
        }
        out
    }

    fn emit_row(&mut self, cells: &[Vec<Line>], row: RowBox<'_>) {
        let mut cx = row.x;
        for (i, cell) in cells.iter().enumerate() {
            let avail = (row.cols[i] - 2.0 * table::PAD_X).max(1.0);
            self.emit(LineBox {
                lines: cell,
                x: cx + table::PAD_X,
                top: row.top + table::PAD_Y,
                line_h: row.line_h,
                size: row.size,
                align: row.aligns.get(i).copied().unwrap_or(Align::None),
                avail,
                bold: row.bold,
            });
            cx += row.cols[i];
        }
    }

    fn separator(&mut self, x: f32, width: f32, y: f32) {
        self.draws.push(Draw::Line(LineDraw {
            x1: x,
            y1: y,
            x2: x + width,
            y2: y,
            color: self.theme.border,
            width: table::BORDER,
        }));
    }

    /// The natural (unwrapped) width of a cell's inline content.
    fn natural_width(&self, inlines: &[Inline], style: InlineStyle) -> f32 {
        let frags = self.flatten(inlines, style);
        frags
            .iter()
            .map(|f| {
                self.metrics.measure(&f.text, &f.style)
                    + if f.code { self.code_pad() * 2.0 } else { 0.0 }
            })
            .sum()
    }

    /// Natural column width for a cell, including padding and the wrap
    /// safety margin.
    ///
    /// Cell text wraps at `(width - 2 * padding) * FONT_SLACK`, so the
    /// column must be wider than the raw measurement by `1 / FONT_SLACK`.
    fn column_width(&self, inlines: &[Inline], style: InlineStyle) -> f32 {
        self.natural_width(inlines, style) / FONT_SLACK + 2.0 * table::PAD_X + 0.5
    }

    fn figure(&mut self, img: &ImageData, x: f32, width: f32) {
        let size = self.theme.font_size;
        let box_h = size * 3.2;
        let top = self.y;
        self.draws.push(Draw::Rect(
            RectDraw::new(x, top, width, box_h)
                .with_fill(self.theme.code_bg)
                .with_stroke(self.theme.border, 1.0)
                .with_radius(6.0),
        ));

        // Alt text, centered in the placeholder box.
        let alt = if img.alt.is_empty() {
            "image"
        } else {
            &img.alt
        };
        let alt_style = self.text_style(size, FontKind::Sans);
        let alt_w = self.metrics.measure(alt, &alt_style);
        self.draws.push(Draw::Text(TextDraw {
            x: x + (width - alt_w) * 0.5,
            y: top + box_h * 0.5 + size * 0.3,
            text: alt.to_string(),
            size,
            family: FontKind::Sans,
            color: self.theme.muted,
            bold: false,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: self.theme.letter_spacing * size,
        }));

        self.y = top + box_h;

        if !img.url.is_empty() {
            let cap_size = size * 0.75;
            let cap_style = self.text_style(cap_size, FontKind::Mono);
            let lines = self.metrics.wrap(&img.url, width, &cap_style);
            for (i, line) in lines.iter().enumerate() {
                self.draws.push(Draw::Text(TextDraw {
                    x: x + (width - self.metrics.measure(line, &cap_style)) * 0.5,
                    y: self.y + size * 0.35 + i as f32 * cap_size * 1.35 + cap_size * 0.8,
                    text: line.clone(),
                    size: cap_size,
                    family: FontKind::Mono,
                    color: self.theme.muted,
                    bold: false,
                    italic: false,
                    underline: false,
                    strike: false,
                    letter_spacing: self.theme.letter_spacing * cap_size,
                }));
            }
            self.y += size * 0.35 + lines.len() as f32 * cap_size * 1.35;
        }
    }

    fn rule(&mut self, x: f32, width: f32) {
        let y = self.y + self.theme.font_size * 0.6;
        self.draws.push(Draw::Line(LineDraw {
            x1: x,
            y1: y,
            x2: x + width,
            y2: y,
            color: self.theme.border,
            width: 2.0,
        }));
        self.y = y + self.theme.font_size * 0.6;
    }

    // -----------------------------------------------------------------
    // Inline flow
    // -----------------------------------------------------------------

    /// Height of one line box at the given font size.
    fn line_height(&self, size: f32, heading: bool) -> f32 {
        let factor = if heading { 1.3 } else { self.theme.line_height };
        size * factor
    }

    /// Horizontal padding around inline code, per side.
    fn code_pad(&self) -> f32 {
        (self.theme.code_size() * 0.3).max(2.0)
    }

    /// A text style that carries the theme's tracking.
    fn text_style(&self, size: f32, family: FontKind) -> TextStyle {
        TextStyle::new(size, family).with_letter_spacing(self.theme.letter_spacing)
    }

    /// Flattens an inline tree into style-tagged fragments.
    fn flatten(&self, inlines: &[Inline], style: InlineStyle) -> Vec<Frag> {
        let mut out = Vec::new();
        self.flatten_into(inlines, style, &mut out);
        out
    }

    fn flatten_into(&self, inlines: &[Inline], style: InlineStyle, out: &mut Vec<Frag>) {
        for inline in inlines {
            match inline {
                Inline::Text(s) => {
                    if !s.is_empty() {
                        let mut frag = Frag::new(
                            s.clone(),
                            TextStyle {
                                size: style.size,
                                family: FontKind::Sans,
                                bold: style.bold,
                                italic: style.italic,
                                letter_spacing: self.theme.letter_spacing,
                            },
                            style.color,
                        );
                        frag.underline = style.underline;
                        frag.strike = style.strike;
                        out.push(frag);
                    }
                }
                Inline::Code(code) => {
                    let mut frag = Frag::new(
                        code.clone(),
                        self.text_style(self.theme.code_size(), FontKind::Mono),
                        self.theme.code_text,
                    );
                    frag.code = true;
                    out.push(frag);
                }
                Inline::Emph(inner) => {
                    let mut s = style;
                    s.italic = true;
                    self.flatten_into(inner, s, out);
                }
                Inline::Strong(inner) => {
                    let mut s = style;
                    s.bold = true;
                    self.flatten_into(inner, s, out);
                }
                Inline::Strike(inner) => {
                    let mut s = style;
                    s.strike = true;
                    self.flatten_into(inner, s, out);
                }
                Inline::Link { text, .. } => {
                    let mut s = style;
                    s.color = self.theme.link;
                    s.underline = true;
                    self.flatten_into(text, s, out);
                }
                Inline::Image(img) => {
                    let label = if img.alt.is_empty() {
                        "image"
                    } else {
                        &img.alt
                    };
                    let mut frag = Frag::new(
                        format!("[{label}]"),
                        self.text_style(self.theme.code_size(), FontKind::Mono),
                        self.theme.muted,
                    );
                    frag.code = true;
                    out.push(frag);
                }
                Inline::Break => out.push(Frag::new(
                    "\n",
                    self.text_style(style.size, FontKind::Sans),
                    style.color,
                )),
            }
        }
    }

    /// Breaks an inline sequence into wrapped lines.
    fn compose(&self, inlines: &[Inline], max_width: f32, style: InlineStyle) -> Vec<Line> {
        self.compose_padded(inlines, max_width, style)
    }

    /// Like [`Self::compose`], but with the `max_width` already excluding
    /// any horizontal padding.
    fn compose_padded(&self, inlines: &[Inline], max_width: f32, style: InlineStyle) -> Vec<Line> {
        let wrap_width = (max_width * FONT_SLACK).max(1.0);
        let frags = self.flatten(inlines, style);
        let atoms = self.atomize(&frags, wrap_width);
        self.wrap(&atoms, wrap_width)
    }

    /// Turns fragments into atoms, applying inline-code padding.
    fn atomize(&self, frags: &[Frag], max_width: f32) -> Vec<Atom> {
        let mut atoms = Vec::new();
        for frag in frags {
            if frag.text == "\n" {
                atoms.push(Atom {
                    frag: frag.clone(),
                    text: String::new(),
                    width: 0.0,
                    pad: 0.0,
                    space: false,
                    hard_break: true,
                });
                continue;
            }
            if frag.code {
                let pad = self.code_pad();
                let w = self.metrics.measure(&frag.text, &frag.style);
                if w + 2.0 * pad <= max_width || max_width <= 1.0 {
                    atoms.push(Atom {
                        frag: frag.clone(),
                        text: frag.text.clone(),
                        width: w + 2.0 * pad,
                        pad,
                        space: false,
                        hard_break: false,
                    });
                } else {
                    // Too wide for any line: break the chip by characters.
                    let mut first = true;
                    for c in frag.text.chars() {
                        let cw = self.metrics.advance(c, &frag.style);
                        let extra = if first { pad * 2.0 } else { 0.0 };
                        atoms.push(Atom {
                            frag: frag.clone(),
                            text: c.to_string(),
                            width: cw + extra,
                            pad: if first { pad } else { 0.0 },
                            space: c == ' ',
                            hard_break: false,
                        });
                        first = false;
                    }
                }
                continue;
            }
            for piece in atomize(&frag.text) {
                let w = self.metrics.measure(piece, &frag.style);
                atoms.push(Atom {
                    frag: frag.clone(),
                    text: piece.to_string(),
                    width: w,
                    pad: 0.0,
                    space: piece == " ",
                    hard_break: false,
                });
            }
        }
        atoms
    }

    /// Greedy wrapping over atoms.
    fn wrap(&self, atoms: &[Atom], max_width: f32) -> Vec<Line> {
        let mut lines: Vec<Line> = Vec::new();
        let mut cur: Vec<Atom> = Vec::new();
        let mut cur_w = 0.0f32;

        macro_rules! flush {
            () => {
                while cur.last().is_some_and(|a| a.space) {
                    cur.pop();
                }
                let width = std::mem::replace(&mut cur_w, 0.0);
                lines.push(Line {
                    atoms: std::mem::take(&mut cur),
                    width,
                });
            };
        }

        for atom in atoms {
            if atom.hard_break {
                flush!();
                continue;
            }
            if atom.space && cur.is_empty() {
                continue;
            }
            // An atom wider than the whole column (a long URL, an
            // identifier) is split by character rather than overflowing.
            if max_width > 1.0 && atom.width > max_width && atom.text.chars().count() > 1 {
                for c in atom.text.chars() {
                    let w = self.metrics.advance(c, &atom.frag.style);
                    if !cur.is_empty() && cur_w + w > max_width {
                        flush!();
                    }
                    if c == ' ' && cur.is_empty() {
                        continue;
                    }
                    let mut piece = atom.clone();
                    piece.text = c.to_string();
                    piece.width = w;
                    piece.pad = 0.0;
                    piece.space = c == ' ';
                    cur.push(piece);
                    cur_w += w;
                }
                continue;
            }
            if !cur.is_empty() && cur_w + atom.width > max_width {
                flush!();
                if atom.space {
                    continue;
                }
            }
            let w = atom.width;
            cur.push(atom.clone());
            cur_w += w;
        }
        if !cur.is_empty() || lines.is_empty() {
            flush!();
        }
        lines
    }

    /// Emits lines at `x`/`top`, returning the total height.
    fn emit(&mut self, b: LineBox<'_>) -> f32 {
        let mut y = b.top;
        for line in b.lines {
            let offset = match b.align {
                Align::Center => ((b.avail - line.width) * 0.5).max(0.0),
                Align::Right => (b.avail - line.width).max(0.0),
                _ => 0.0,
            };
            let baseline = y + (b.line_h - b.size) * 0.5 + b.size * 0.8;
            let mut run_x = b.x + offset;
            let mut i = 0usize;
            while i < line.atoms.len() {
                let frag = &line.atoms[i].frag;
                let start = run_x;
                let mut text = String::new();
                let mut width = 0.0;
                let mut pad: f32 = 0.0;
                while i < line.atoms.len() {
                    let atom = &line.atoms[i];
                    if !same_frag(&atom.frag, frag) {
                        break;
                    }
                    text.push_str(&atom.text);
                    width += atom.width;
                    pad = pad.max(atom.pad);
                    run_x += atom.width;
                    i += 1;
                }
                self.emit_run(RunBox {
                    text: &text,
                    x: start,
                    width,
                    pad,
                    baseline,
                    line_h: b.line_h,
                    line_top: y,
                    frag,
                    force_bold: b.bold,
                });
            }
            y += b.line_h;
        }
        b.lines.len() as f32 * b.line_h
    }

    /// Emits one same-styled run: chip background, text, decorations.
    fn emit_run(&mut self, run: RunBox<'_>) {
        let RunBox {
            text,
            x,
            width,
            pad,
            baseline,
            line_h,
            line_top,
            frag,
            force_bold,
        } = run;
        let trimmed = text.trim_end();
        if trimmed.is_empty() {
            return;
        }
        let text_w = (width - 2.0 * pad).max(0.0);
        let visible_w = if text.len() != trimmed.len() {
            let ratio = trimmed.chars().count() as f32 / text.chars().count().max(1) as f32;
            (text_w * ratio).max(0.0)
        } else {
            text_w
        };

        if frag.code {
            let chip_h = (frag.style.size * 1.4).min(line_h);
            let chip_y = line_top + (line_h - chip_h) * 0.5;
            self.draws.push(Draw::Rect(
                RectDraw::new(
                    x + 0.5,
                    chip_y,
                    (visible_w + 2.0 * pad - 1.0).max(2.0),
                    chip_h,
                )
                .with_fill(self.theme.code_bg)
                .with_radius(3.0),
            ));
        }

        let bold = force_bold || frag.style.bold;
        self.draws.push(Draw::Text(TextDraw {
            x: x + pad,
            y: baseline,
            text: trimmed.to_string(),
            size: frag.style.size,
            family: frag.style.family,
            color: frag.color,
            bold,
            italic: frag.style.italic,
            underline: frag.underline,
            strike: frag.strike,
            letter_spacing: frag.style.letter_spacing * frag.style.size,
        }));

        if frag.underline {
            let uy = baseline + frag.style.size * 0.13;
            self.draws.push(Draw::Line(LineDraw {
                x1: x + pad,
                y1: uy,
                x2: x + pad + visible_w,
                y2: uy,
                color: frag.color,
                width: 1.0,
            }));
        }
        if frag.strike {
            let sy = baseline - frag.style.size * 0.28;
            self.draws.push(Draw::Line(LineDraw {
                x1: x + pad,
                y1: sy,
                x2: x + pad + visible_w,
                y2: sy,
                color: frag.color,
                width: 1.0,
            }));
        }
    }
}

/// Arguments for emitting one same-styled run.
struct RunBox<'r> {
    text: &'r str,
    x: f32,
    width: f32,
    pad: f32,
    baseline: f32,
    line_h: f32,
    line_top: f32,
    frag: &'r Frag,
    force_bold: bool,
}

/// Shrinks column widths so their sum never exceeds `width`.
///
/// Columns keep their natural width when everything fits. Otherwise they
/// give up width proportionally in the part above [`table::MIN_COL`], which
/// keeps narrow columns readable and guarantees the table never overflows
/// the content column, no matter how many columns or how wide the cells.
fn fit_columns(natural: &[f32], width: f32) -> Vec<f32> {
    let total: f32 = natural.iter().sum();
    if total <= width || natural.is_empty() {
        return natural.to_vec();
    }
    let n = natural.len();
    let min_total = table::MIN_COL * n as f32;
    if width <= min_total {
        return vec![width / n as f32; n];
    }
    let slack_natural: f32 = natural.iter().map(|w| (w - table::MIN_COL).max(0.0)).sum();
    if slack_natural <= 0.0 {
        return natural.to_vec();
    }
    let slack_available = width - min_total;
    natural
        .iter()
        .map(|w| table::MIN_COL + (w - table::MIN_COL).max(0.0) * slack_available / slack_natural)
        .collect()
}

/// Whether two fragments can share one emitted text run.
fn same_frag(a: &Frag, b: &Frag) -> bool {
    a.style == b.style
        && a.color == b.color
        && a.underline == b.underline
        && a.strike == b.strike
        && a.code == b.code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::theme::Theme;

    fn scene(src: &str) -> Scene {
        let theme = Theme::light();
        let doc = parse(src);
        Layout::new(&theme).render_marked(&doc).0
    }

    fn texts(scene: &Scene) -> Vec<String> {
        scene
            .draws
            .iter()
            .filter_map(|d| match d {
                Draw::Text(t) => Some(t.text.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn empty_document_has_padding_height() {
        let s = scene("");
        assert_eq!(s.width, 800.0);
        assert!((s.height - 96.0).abs() < 0.01, "height was {}", s.height);
        assert!(s.draws.is_empty());
        assert!(s.background.is_some());
    }

    #[test]
    fn paragraph_starts_at_padding() {
        let s = scene("hello");
        let t = s.find_text("hello").expect("paragraph text");
        assert!((t.x - 48.0).abs() < 0.01);
        assert!(t.y > 48.0);
    }

    #[test]
    fn long_paragraph_wraps_into_multiple_text_runs() {
        let src = "word ".repeat(120);
        let s = scene(&src);
        let runs = texts(&s);
        assert!(runs.len() > 3, "expected wrapping, got {runs:?}");
        for line in &runs {
            assert!(line.len() <= 80, "line too long: {line}");
        }
    }

    #[test]
    fn heading_uses_larger_font_and_bold() {
        let s = scene("# Title");
        let t = s.find_text("Title").expect("heading text");
        assert_eq!(t.size, 32.0);
        assert!(t.bold);
        assert_eq!(t.color, Theme::light().heading);
    }

    #[test]
    fn heading_levels_shrink() {
        let size_of = |s: &Scene| {
            s.draws
                .iter()
                .find_map(|d| match d {
                    Draw::Text(t) => Some(t.size),
                    _ => None,
                })
                .expect("heading text")
        };
        let h1 = size_of(&scene("# a"));
        let h2 = size_of(&scene("## b"));
        let h3 = size_of(&scene("### c"));
        assert!(h1 > h2 && h2 > h3, "{h1} {h2} {h3}");
    }

    #[test]
    fn h1_and_h2_get_a_rule_but_h3_does_not() {
        for (src, expected) in [("# a", 1), ("## b", 1), ("### c", 0)] {
            let lines = scene(src)
                .draws
                .iter()
                .filter(|d| matches!(d, Draw::Line(_)))
                .count();
            assert_eq!(lines, expected, "source {src}");
        }
    }

    #[test]
    fn thematic_break_draws_a_line() {
        let s = scene("a\n\n---\n\nb");
        assert_eq!(
            s.draws
                .iter()
                .filter(|d| matches!(d, Draw::Line(_)))
                .count(),
            1
        );
    }

    #[test]
    fn code_block_has_background_and_mono_text() {
        let s = scene("```rust\nlet x = 1;\n```");
        let bg = s.draws.iter().find_map(|d| match d {
            Draw::Rect(r) if r.fill == Some(Theme::light().code_bg) => Some(r.clone()),
            _ => None,
        });
        assert!(bg.is_some(), "expected a code background rect");
        let t = s.find_text("let x = 1;").expect("code text");
        assert_eq!(t.family, FontKind::Mono);
    }

    #[test]
    fn code_blocks_keep_their_indentation() {
        let s = scene("```\nMarkdown\n  → parse\n    deeper\n```");
        let runs: Vec<&TextDraw> = s
            .draws
            .iter()
            .filter_map(|d| match d {
                Draw::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(runs.len(), 3, "{runs:?}");
        assert_eq!(runs[1].text, "  → parse");
        assert_eq!(runs[2].text, "    deeper");
    }

    #[test]
    fn code_block_shows_language_label() {
        let s = scene("```rust\nx\n```");
        assert!(s.find_text("rust").is_some());
    }

    #[test]
    fn code_block_without_info_has_no_label() {
        let s = scene("```\nx\n```");
        assert_eq!(
            s.draws
                .iter()
                .filter(|d| matches!(d, Draw::Text(t) if t.text == "rust"))
                .count(),
            0
        );
    }

    #[test]
    fn quote_draws_bar_and_indents_text() {
        let s = scene("> quoted");
        let bar = s.draws.iter().find_map(|d| match d {
            Draw::Rect(r) if r.fill == Some(Theme::light().quote_bar) => Some(r.clone()),
            _ => None,
        });
        let bar = bar.expect("quote bar");
        assert_eq!(bar.w, 4.0);
        let t = s.find_text("quoted").expect("quote text");
        assert!(t.x >= 48.0 + 16.0);
    }

    #[test]
    fn quote_text_is_painted_above_its_background() {
        let s = scene("> quoted");
        let bg = s
            .draws
            .iter()
            .position(|d| matches!(d, Draw::Rect(r) if r.fill == Some(Theme::light().quote_bg)))
            .expect("quote background");
        let text = s
            .draws
            .iter()
            .position(|d| matches!(d, Draw::Text(t) if t.text.contains("quoted")))
            .expect("quote text");
        assert!(
            bg < text,
            "background must be emitted before the text it sits behind"
        );
    }

    #[test]
    fn nested_quote_content_is_not_covered() {
        let s = scene("> outer\n>\n> > inner");
        let outer_bg = s
            .draws
            .iter()
            .position(|d| matches!(d, Draw::Rect(r) if r.fill == Some(Theme::light().quote_bg)))
            .expect("quote background");
        for needle in ["outer", "inner"] {
            let idx = s
                .draws
                .iter()
                .position(|d| matches!(d, Draw::Text(t) if t.text.contains(needle)))
                .unwrap_or_else(|| panic!("missing text {needle}"));
            assert!(
                idx > outer_bg,
                "{needle} must be painted above the outer backdrop"
            );
        }
    }

    #[test]
    fn nested_quote_backdrops_fade_with_depth() {
        let fills: Vec<Color> = scene("> a\n>\n> > b")
            .draws
            .iter()
            .filter_map(|d| match d {
                Draw::Rect(r) => r.fill,
                _ => None,
            })
            .filter(|c| *c != Theme::light().background.unwrap())
            .collect();
        assert!(
            fills.len() >= 2,
            "expected two quote backdrops, got {fills:?}"
        );
        assert_ne!(
            fills[0], fills[1],
            "nested quotes should be tinted differently"
        );
    }

    #[test]
    fn nested_quotes_indent_further() {
        let s = scene("> > deep");
        let t = s.find_text("deep").expect("deep text");
        assert!(t.x >= 48.0 + 2.0 * 16.0, "x = {}", t.x);
    }

    #[test]
    fn bullet_list_draws_dots_and_indents_content() {
        let s = scene("- a\n- b");
        let dots = s
            .draws
            .iter()
            .filter(|d| matches!(d, Draw::Rect(r) if r.radius > 0.0 && r.fill == Some(Theme::light().bullet)))
            .count();
        assert_eq!(dots, 2);
        let t = s.find_text("a").expect("item text");
        assert!(t.x > 48.0);
    }

    #[test]
    fn ordered_list_draws_numbers() {
        let s = scene("1. one\n2. two");
        assert!(s.find_text("1.").is_some());
        assert!(s.find_text("2.").is_some());
    }

    #[test]
    fn ordered_list_respects_start() {
        let s = scene("5. five");
        assert!(s.find_text("5.").is_some());
    }

    #[test]
    fn nested_list_increases_indent() {
        let s = scene("- a\n  - b");
        let a = s.find_text("a").expect("outer");
        let b = s.find_text("b").expect("inner");
        assert!(b.x > a.x, "inner {} should be right of outer {}", b.x, a.x);
    }

    #[test]
    fn table_draws_header_background_and_lines() {
        let s = scene("| a | b |\n|---|---|\n| 1 | 2 |");
        assert!(
            s.draws.iter().any(
                |d| matches!(d, Draw::Rect(r) if r.fill == Some(Theme::light().table_header_bg))
            )
        );
        assert_eq!(
            s.draws
                .iter()
                .filter(|d| matches!(d, Draw::Line(_)))
                .count(),
            3
        );
        assert!(s.find_text("a").is_some());
        assert!(s.find_text("1").is_some());
    }

    #[test]
    fn table_alignment_right_pushes_cell_text_right() {
        let s = scene("| left | right |\n|---|---:|\n| a | b |");
        let a = s.find_text("a").expect("left cell");
        let b = s.find_text("b").expect("right cell");
        assert!(b.x > a.x);
    }

    #[test]
    fn alignment_enum_is_used_for_headers() {
        // Center alignment centres the run within the content column.
        let s = scene("| a |\n|:-:|\n| b |");
        let a = s.find_text("a").expect("head");
        assert!(a.x > 48.0);
    }

    #[test]
    fn figure_draws_placeholder_box_and_url() {
        let s = scene("![a cat](https://example.com/cat.png)");
        assert!(
            s.draws
                .iter()
                .any(|d| matches!(d, Draw::Rect(r) if r.stroke.is_some()))
        );
        assert!(s.find_text("a cat").is_some());
        assert!(s.find_text("example.com").is_some());
    }

    #[test]
    fn inline_code_gets_a_background_chip() {
        let s = scene("use `cargo` here");
        let t = s.find_text("cargo").expect("code text");
        assert_eq!(t.family, FontKind::Mono);
        assert!(
            s.draws
                .iter()
                .any(|d| matches!(d, Draw::Rect(r) if r.fill == Some(Theme::light().code_bg)))
        );
    }

    #[test]
    fn links_are_colored_and_underlined() {
        let s = scene("[x](https://example.com)");
        let t = s.find_text("x").expect("link text");
        assert_eq!(t.color, Theme::light().link);
        assert!(t.underline);
        assert!(
            s.draws.iter().any(|d| matches!(d, Draw::Line(_))),
            "underline line"
        );
    }

    #[test]
    fn strong_and_emph_set_flags() {
        let s = scene("**b** *i*");
        let b = s.find_text("b").expect("bold");
        let i = s.find_text("i").expect("italic");
        assert!(b.bold);
        assert!(i.italic);
    }

    #[test]
    fn strikethrough_draws_a_line() {
        let s = scene("~~gone~~");
        assert!(s.find_text("gone").is_some_and(|t| t.strike));
        assert!(s.draws.iter().any(|d| matches!(d, Draw::Line(_))));
    }

    #[test]
    fn hard_break_starts_a_new_line() {
        let s = scene("a  \nb");
        let a = s.find_text("a").expect("a");
        let b = s.find_text("b").expect("b");
        assert!(b.y > a.y);
    }

    #[test]
    fn cjk_text_wraps_without_overflow() {
        let s = scene(&"中文测试内容".repeat(20));
        let overs: Vec<&TextDraw> = s
            .draws
            .iter()
            .filter_map(|d| match d {
                Draw::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(!overs.is_empty());
        for t in overs {
            let m = Metrics::new();
            let w = m.measure(&t.text, &TextStyle::new(t.size, t.family));
            assert!(
                t.x + w <= 800.0 - 48.0 + 1.0,
                "text overflows: {:?} x={} w={}",
                t.text,
                t.x,
                w
            );
        }
    }

    #[test]
    fn scene_height_grows_with_content() {
        let short = scene("a").height;
        let long = scene(&"paragraph\n\n".repeat(10)).height;
        assert!(long > short);
    }

    #[test]
    fn deeper_quotes_nest_visually() {
        let s = scene("> level one\n>\n> > level two");
        let one = s.find_text("level one").expect("one");
        let two = s.find_text("level two").expect("two");
        assert!(two.x > one.x);
    }

    #[test]
    fn tight_and_loose_lists_differ_in_height() {
        let tight = scene("- a\n- b\n- c").height;
        let loose = scene("- a\n\n- b\n\n- c").height;
        assert!(loose > tight, "loose {loose} vs tight {tight}");
    }

    #[test]
    fn everything_fits_inside_the_canvas() {
        let src = "# H1\n\nParagraph with **bold**, *italic*, `code`, and [a link](https://x.y).\n\n\
                   > quote\n> > nested\n\n- item one\n- item two\n  - nested item\n\n\
                   1. first\n2. second\n\n```rust\nfn main() { println!(\"hi\"); }\n```\n\n\
                   | a | b |\n|---|---|\n| 1 | 2 |\n\n---\n\n![img](u.png)";
        let s = scene(src);
        assert!(s.width > 0.0 && s.height > 0.0);
        for d in &s.draws {
            if let Draw::Text(t) = d {
                assert!(t.x >= 0.0, "negative x");
                assert!(t.y <= s.height, "text below canvas");
            }
        }
    }

    #[test]
    fn quote_on_a_transparent_theme_uses_the_plain_backdrop() {
        let theme = Theme {
            background: None,
            ..Theme::light()
        };
        let s = Layout::new(&theme).render_marked(&parse("> q")).0;
        assert!(
            s.draws
                .iter()
                .any(|d| matches!(d, Draw::Rect(r) if r.fill == Some(theme.quote_bg)))
        );
    }

    #[test]
    fn wide_tables_shrink_to_the_content_column() {
        let src = format!(
            "| {} | {} |\n|---|---|\n| {} | {} |",
            "a".repeat(120),
            "b".repeat(120),
            "c".repeat(120),
            "d".repeat(120)
        );
        let s = scene(&src);
        let header = s
            .draws
            .iter()
            .find_map(|d| match d {
                Draw::Rect(r) if r.fill == Some(Theme::light().table_header_bg) => Some(r.clone()),
                _ => None,
            })
            .expect("table header background");
        assert!(
            header.w <= 800.0 - 2.0 * 48.0 + 0.01,
            "table too wide: {}",
            header.w
        );
    }

    #[test]
    fn figure_without_alt_or_url_still_renders() {
        let s = scene("![]()");
        assert!(
            s.find_text("image").is_some(),
            "falls back to a generic label"
        );
        assert!(s.height > 0.0);
    }

    #[test]
    fn inline_images_render_as_chips() {
        let s = scene("see ![a cat](x.png) here");
        assert!(
            s.find_text("[a cat]").is_some(),
            "inline image chip missing"
        );
    }

    #[test]
    fn empty_image_url_skips_the_caption() {
        let s = scene("![]()");
        let captions = s
            .draws
            .iter()
            .filter(|d| matches!(d, Draw::Text(t) if t.family == FontKind::Mono))
            .count();
        assert_eq!(captions, 0, "no URL means no caption");
    }

    #[test]
    fn many_wide_columns_still_fit_the_column() {
        // 12 columns of text that would each need ~200px naturally.
        let header: Vec<String> = (0..12).map(|i| format!("h{i}")).collect();
        let row: Vec<String> = (0..12)
            .map(|i| format!("long cell {i} with words"))
            .collect();
        let src = format!(
            "| {} |\n|{}|\n| {} |",
            header.join(" | "),
            ["---"; 12].join("|"),
            row.join(" | ")
        );
        let s = scene(&src);
        let header_bg = s
            .draws
            .iter()
            .find_map(|d| match d {
                Draw::Rect(r) if r.fill == Some(Theme::light().table_header_bg) => Some(r.clone()),
                _ => None,
            })
            .expect("table header");
        assert!(header_bg.w <= 704.0 + 0.01, "table width {}", header_bg.w);
        let m = Metrics::new();
        for d in &s.draws {
            if let Draw::Text(t) = d {
                let w = m.measure(&t.text, &TextStyle::new(t.size, t.family));
                assert!(t.x + w <= 752.0 + 1.0, "cell text overflows: {:?}", t.text);
            }
        }
    }

    #[test]
    fn pathologically_nested_lists_do_not_overflow() {
        let mut src = String::new();
        for depth in 0..40 {
            src.push_str(&"  ".repeat(depth));
            src.push_str("- item\n");
        }
        let s = scene(&src);
        let m = Metrics::new();
        for d in &s.draws {
            if let Draw::Text(t) = d {
                let w = m.measure(&t.text, &TextStyle::new(t.size, t.family));
                assert!(
                    t.x + w <= 752.0 + 1.0,
                    "list text overflows: {:?} at {}",
                    t.text,
                    t.x
                );
            }
        }
    }

    #[test]
    fn listing_metrics_account_for_tracking() {
        let theme = Theme {
            letter_spacing: 0.1,
            ..Theme::light()
        };
        let s = Layout::new(&theme).render_marked(&parse("spaced text")).0;
        let t = s.find_text("spaced").expect("text");
        assert!(t.letter_spacing > 0.0);
        assert!((t.letter_spacing - 0.1 * t.size).abs() < 1e-4);
    }
}
