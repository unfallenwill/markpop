//! Mapping the Markdown AST onto the Word model.
//!
//! Every Markdown construct gets a Word counterpart: headings become
//! styled paragraphs, lists become numbered paragraphs, tables become
//! real tables, images become embedded parts. A construct that cannot be
//! represented degrades to its text rather than failing the render.
//!
//! Raw HTML is interpreted exactly as the layout engine interprets it:
//! an HTML block becomes ordinary blocks, and inline tags open and close
//! styles around the text between them.

use std::path::{Path, PathBuf};

use crate::ast::{
    Align, Block as AstBlock, Document, ImageData, Inline as AstInline, ListData, TableData,
};
use crate::html::{HtmlToken, tokenize_html};
use crate::layout::html::html_blocks;
use crate::options::Options;

use super::media;
use super::model::{
    Block, Cell, Document as WordDocument, Image, Numbering, Paragraph, ParagraphProps, Row, Run,
    RunProps, Table, TextRun,
};

/// The content width of an A4 page, in twips.
const CONTENT_WIDTH_TWIPS: u32 = 9026;
/// The indent one quote level adds.
const QUOTE_INDENT_TWIPS: u32 = 720;
/// The mark an unfinished task item carries.
const TASK_OPEN: &str = "☐ ";
/// The mark a finished task item carries.
const TASK_DONE: &str = "☒ ";
/// The deepest list level Word's numbering defines.
const MAX_LEVEL: u8 = 8;

/// An image part inside the package.
#[derive(Debug, Clone, PartialEq)]
pub struct ImagePart {
    /// The part name, e.g. `image1.png`.
    pub name: String,
    /// The PNG bytes.
    pub bytes: Vec<u8>,
}

/// One ordered list instance and the number it starts at.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderedList {
    /// The `w:num` identifier.
    pub num_id: u32,
    /// The first number of the list.
    pub start: u32,
}

/// The numbering definitions a document needs.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingPlan {
    /// The `w:num` identifier shared by every bullet list.
    pub bullet_num_id: u32,
    /// One entry per ordered list, in document order.
    pub ordered: Vec<OrderedList>,
}

/// Everything the writer needs to produce the package.
#[derive(Debug, Clone, PartialEq)]
pub struct Content {
    /// The document body.
    pub document: WordDocument,
    /// The image parts, in the order their relationships are numbered.
    pub images: Vec<ImagePart>,
    /// The distinct link targets, in first-seen order.
    pub links: Vec<String>,
    /// The numbering definitions.
    pub numbering: NumberingPlan,
    /// Whether a table of contents was requested.
    pub toc: bool,
    /// The heading above the table of contents.
    pub toc_title: String,
}

/// Maps a parsed document onto the Word model.
pub fn build(doc: &Document, options: &Options) -> Content {
    let mut builder = Builder {
        options,
        body: Vec::new(),
        images: Vec::new(),
        links: Vec::new(),
        numbering: NumberingPlan {
            bullet_num_id: 1,
            ordered: Vec::new(),
        },
        next_num_id: 2,
        html_stack: Vec::new(),
    };
    builder.blocks(&doc.blocks, 0, 0);
    Content {
        document: WordDocument { body: builder.body },
        images: builder.images,
        links: builder.links,
        numbering: builder.numbering,
        toc: options.toc,
        toc_title: options.toc_title.clone(),
    }
}

/// Walks the AST once, producing the Word model and the package's
/// relationship state.
struct Builder<'a> {
    options: &'a Options,
    body: Vec<Block>,
    images: Vec<ImagePart>,
    links: Vec<String>,
    numbering: NumberingPlan,
    next_num_id: u32,
    /// The HTML tags currently open, with the style each one adds.
    html_stack: Vec<(String, RunProps)>,
}

impl Builder<'_> {
    /// Emits a sequence of blocks.
    fn blocks(&mut self, blocks: &[AstBlock], quote_depth: u32, list_level: u8) {
        for block in blocks {
            self.block(block, quote_depth, list_level);
        }
    }

    /// Emits one block.
    fn block(&mut self, block: &AstBlock, quote_depth: u32, list_level: u8) {
        match block {
            AstBlock::Heading { level, inlines } => {
                let p = self.paragraph(inlines, Some(format!("Heading{level}")), quote_depth);
                self.body.push(Block::Paragraph(p));
            }
            AstBlock::Paragraph(inlines) => {
                let p = self.paragraph(inlines, None, quote_depth);
                self.body.push(Block::Paragraph(p));
            }
            AstBlock::Code(code) => {
                for line in &code.lines {
                    let run = Run::Text(TextRun {
                        text: line.clone(),
                        props: RunProps::default(),
                    });
                    self.body.push(Block::Paragraph(Paragraph {
                        style: Some("HTMLPreformatted".into()),
                        props: ParagraphProps {
                            indent_left: quote_depth * QUOTE_INDENT_TWIPS,
                            ..ParagraphProps::default()
                        },
                        runs: vec![run],
                    }));
                }
            }
            AstBlock::Quote(inner) => {
                for inner_block in inner {
                    self.quote_block(inner_block, quote_depth + 1, list_level);
                }
            }
            AstBlock::List(list) => self.list(list, list_level),
            AstBlock::Table(table) => {
                let table = self.table(table);
                self.body.push(Block::Table(table));
            }
            AstBlock::Figure(image) => {
                let runs = self.image_runs(image);
                self.body.push(Block::Paragraph(Paragraph {
                    style: None,
                    props: ParagraphProps::default(),
                    runs,
                }));
            }
            AstBlock::ThematicBreak => {
                self.body.push(Block::Paragraph(Paragraph {
                    style: None,
                    props: ParagraphProps {
                        bottom_border: true,
                        ..ParagraphProps::default()
                    },
                    runs: Vec::new(),
                }));
            }
            AstBlock::Html(raw) => {
                // Raw HTML is interpreted by the same mapper the layout
                // engine uses, so both outputs agree on what it means.
                let blocks = html_blocks(raw);
                self.blocks(&blocks, quote_depth, list_level);
            }
        }
    }

    /// Emits a block that sits inside a quote.
    fn quote_block(&mut self, block: &AstBlock, depth: u32, list_level: u8) {
        match block {
            AstBlock::Paragraph(inlines) => {
                let p = self.paragraph(inlines, Some("Quote".into()), depth);
                self.body.push(Block::Paragraph(p));
            }
            other => self.block(other, depth, list_level),
        }
    }

    /// Builds a paragraph from inline content.
    fn paragraph(&mut self, inlines: &[AstInline], style: Option<String>, depth: u32) -> Paragraph {
        let runs = self.inlines(inlines, &RunProps::default());
        Paragraph {
            style,
            props: ParagraphProps {
                indent_left: depth * QUOTE_INDENT_TWIPS,
                ..ParagraphProps::default()
            },
            runs,
        }
    }

    /// Maps a sequence of inlines to runs.
    fn inlines(&mut self, inlines: &[AstInline], props: &RunProps) -> Vec<Run> {
        let mut runs = Vec::new();
        for inline in inlines {
            runs.extend(self.inline(inline, props));
        }
        runs
    }

    /// Maps one inline to runs.
    fn inline(&mut self, inline: &AstInline, props: &RunProps) -> Vec<Run> {
        let base = self.effective_props(props);
        match inline {
            AstInline::Text(text) => vec![Run::Text(TextRun {
                text: text.clone(),
                props: base,
            })],
            AstInline::Code(text) => {
                let mut p = base;
                p.style = Some("HTMLCode".into());
                vec![Run::Text(TextRun {
                    text: text.clone(),
                    props: p,
                })]
            }
            AstInline::Emph(children) => {
                let mut p = base;
                p.italic = true;
                self.inlines(children, &p)
            }
            AstInline::Strong(children) => {
                let mut p = base;
                p.bold = true;
                self.inlines(children, &p)
            }
            AstInline::Strike(children) => {
                let mut p = base;
                p.strike = true;
                self.inlines(children, &p)
            }
            AstInline::Underline(children) => {
                let mut p = base;
                p.underline = true;
                self.inlines(children, &p)
            }
            AstInline::Link { text, url, .. } => {
                let mut p = base;
                self.register_link(url);
                p.link = Some(url.clone());
                if p.style.is_none() {
                    p.style = Some("Hyperlink".into());
                }
                self.inlines(text, &p)
            }
            AstInline::Image(image) => self.image_runs(image),
            AstInline::Break => vec![Run::Break],
            AstInline::SoftBreak => vec![Run::Text(TextRun {
                text: " ".into(),
                props: base,
            })],
            AstInline::Html(raw) => self.html_runs(raw, &base),
        }
    }

    /// Maps raw inline HTML to runs.
    ///
    /// The Markdown scanner hands over one tag at a time, so the tags are
    /// read as a stream: an open tag pushes a style, a close tag pops it,
    /// and the text between them carries the style. This is the same
    /// reading the layout engine gives raw HTML.
    fn html_runs(&mut self, raw: &str, props: &RunProps) -> Vec<Run> {
        let mut runs = Vec::new();
        for token in tokenize_html(raw) {
            match token {
                HtmlToken::Text(text) => {
                    let p = self.effective_props(props);
                    runs.push(Run::Text(TextRun { text, props: p }));
                }
                HtmlToken::End(name) => {
                    // A close tag restores the state saved when its open
                    // tag was seen; unmatched close tags are ignored.
                    if let Some(pos) = self.html_stack.iter().rposition(|(open, _)| open == &name) {
                        self.html_stack.truncate(pos);
                    }
                }
                HtmlToken::Start(tag) => {
                    let mut delta = RunProps::default();
                    match tag.name.as_str() {
                        "b" | "strong" => delta.bold = true,
                        "i" | "em" | "cite" | "var" | "dfn" => delta.italic = true,
                        "del" | "s" | "strike" => delta.strike = true,
                        "u" | "ins" => delta.underline = true,
                        "code" | "kbd" | "samp" | "tt" => delta.style = Some("HTMLCode".into()),
                        "a" => {
                            if let Some(href) = tag.attr("href") {
                                self.register_link(href);
                                delta.link = Some(href.to_string());
                                delta.style = Some("Hyperlink".into());
                            }
                        }
                        "br" => {
                            runs.push(Run::Break);
                            continue;
                        }
                        "img" => {
                            let alt = tag.attr("alt").unwrap_or_default().to_string();
                            let url = tag.attr("src").unwrap_or_default().to_string();
                            runs.extend(self.image_runs(&ImageData {
                                alt,
                                url,
                                title: None,
                            }));
                            continue;
                        }
                        // Transparent containers: no visual effect, but the
                        // tag is still scoped so its close tag behaves.
                        _ => {}
                    }
                    if !tag.self_closing {
                        self.html_stack.push((tag.name.clone(), delta));
                    }
                }
            }
        }
        runs
    }

    /// The character properties in force, including the styles of the
    /// HTML tags that are currently open.
    fn effective_props(&self, base: &RunProps) -> RunProps {
        let mut p = base.clone();
        for (_, delta) in &self.html_stack {
            if delta.bold {
                p.bold = true;
            }
            if delta.italic {
                p.italic = true;
            }
            if delta.strike {
                p.strike = true;
            }
            if delta.underline {
                p.underline = true;
            }
            if delta.style.is_some() {
                p.style = delta.style.clone();
            }
            if delta.link.is_some() {
                p.link = delta.link.clone();
            }
        }
        p
    }

    /// Emits a list: one numbering instance, one paragraph per item.
    fn list(&mut self, list: &ListData, level: u8) {
        let level = level.min(MAX_LEVEL);
        let num_id = if list.ordered {
            // Every ordered list gets its own instance, so two lists in a
            // document do not share a counter.
            let id = self.next_num_id;
            self.next_num_id += 1;
            self.numbering.ordered.push(OrderedList {
                num_id: id,
                start: list.start.max(1),
            });
            id
        } else {
            self.numbering.bullet_num_id
        };
        for item in &list.items {
            let mut prefix = match item.task {
                Some(true) => Some(TASK_DONE),
                Some(false) => Some(TASK_OPEN),
                None => None,
            };
            for block in &item.blocks {
                match block {
                    AstBlock::Paragraph(inlines) => {
                        let mut runs = Vec::new();
                        if let Some(mark) = prefix.take() {
                            runs.push(Run::Text(TextRun {
                                text: mark.to_string(),
                                props: RunProps::default(),
                            }));
                        }
                        runs.extend(self.inlines(inlines, &RunProps::default()));
                        self.body.push(Block::Paragraph(Paragraph {
                            style: Some("ListParagraph".into()),
                            props: ParagraphProps {
                                numbering: Some(Numbering { num_id, level }),
                                ..ParagraphProps::default()
                            },
                            runs,
                        }));
                    }
                    AstBlock::List(nested) => self.list(nested, level + 1),
                    other => self.block(other, 0, level),
                }
            }
        }
    }

    /// Builds a table from a GFM table.
    fn table(&mut self, table: &TableData) -> Table {
        let columns = table
            .head
            .len()
            .max(table.rows.iter().map(|r| r.len()).max().unwrap_or(0))
            .max(1);
        let widths = vec![CONTENT_WIDTH_TWIPS / columns as u32; columns];

        let mut rows = Vec::with_capacity(table.rows.len() + 1);
        let head = (0..columns)
            .map(|i| {
                let inlines = table.head.get(i).map(|c| c.as_slice()).unwrap_or(&[]);
                self.cell(inlines, table.aligns.get(i).copied().unwrap_or_default(), true)
            })
            .collect();
        rows.push(Row {
            cells: head,
            header: true,
        });
        for row in &table.rows {
            let cells = (0..columns)
                .map(|i| {
                    let inlines = row.get(i).map(|c| c.as_slice()).unwrap_or(&[]);
                    self.cell(inlines, table.aligns.get(i).copied().unwrap_or_default(), false)
                })
                .collect();
            rows.push(Row { cells, header: false });
        }
        Table { rows, widths }
    }

    /// Builds one table cell.
    fn cell(&mut self, inlines: &[AstInline], align: Align, header: bool) -> Cell {
        let props = RunProps {
            bold: header,
            ..RunProps::default()
        };
        let runs = self.inlines(inlines, &props);
        Cell {
            paragraphs: vec![Paragraph {
                style: None,
                props: ParagraphProps::default(),
                runs,
            }],
            align,
        }
    }

    /// Loads an image, or falls back to its alt text.
    fn image_runs(&mut self, image: &ImageData) -> Vec<Run> {
        match self.load_image(&image.url) {
            Some((bytes, width_px, height_px)) => {
                let index = self.images.len() as u32;
                let name = format!("image{}.png", index + 1);
                self.images.push(ImagePart {
                    name: name.clone(),
                    bytes,
                });
                let (width_emu, height_emu) = media::fit_emu(width_px, height_px);
                vec![Run::Image(Image {
                    index,
                    width_emu,
                    height_emu,
                    alt: image.alt.clone(),
                    name,
                })]
            }
            None => vec![Run::Text(TextRun {
                text: image.alt.clone(),
                props: RunProps::default(),
            })],
        }
    }

    /// Reads an image file, when local images are allowed.
    fn load_image(&mut self, url: &str) -> Option<(Vec<u8>, u32, u32)> {
        if !self.options.local_images {
            return None;
        }
        // Remote images are never fetched.
        let lowered = url.to_ascii_lowercase();
        if lowered.starts_with("http://")
            || lowered.starts_with("https://")
            || lowered.starts_with("data:")
        {
            return None;
        }
        media::load_png(&self.resolve(url))
    }

    /// Resolves a relative image path against the base directory.
    fn resolve(&self, url: &str) -> PathBuf {
        let decoded = percent_decode(url);
        let path = Path::new(&decoded);
        match self.options.base_dir.as_deref() {
            Some(dir) => Path::new(dir).join(path),
            None => path.to_path_buf(),
        }
    }

    /// Records a link target, keeping the first-seen order.
    fn register_link(&mut self, url: &str) {
        if !self.links.iter().any(|seen| seen == url) {
            self.links.push(url.to_string());
        }
    }
}

/// Decodes `%XX` escapes in a path.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            if let Some(byte) = hex {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    /// Builds the content of a Markdown snippet.
    fn build(markdown: &str) -> Content {
        super::build(&parse(markdown), &Options::default())
    }

    /// Builds the content of a Markdown snippet with explicit options.
    fn build_with(markdown: &str, options: &Options) -> Content {
        super::build(&parse(markdown), options)
    }

    /// The paragraphs of a document.
    fn paragraphs(content: &Content) -> Vec<&Paragraph> {
        content
            .document
            .body
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    /// The character properties of a paragraph's runs that carry text.
    fn props(paragraph: &Paragraph) -> Vec<RunProps> {
        paragraph
            .runs
            .iter()
            .filter_map(|r| match r {
                Run::Text(t) if !t.text.trim().is_empty() => Some(t.props.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_heading_becomes_a_styled_paragraph() {
        let c = build("# Title\n");
        assert_eq!(paragraphs(&c)[0].style.as_deref(), Some("Heading1"));
    }

    #[test]
    fn heading_levels_reach_six() {
        let c = build("###### Deep\n");
        assert_eq!(paragraphs(&c)[0].style.as_deref(), Some("Heading6"));
    }

    #[test]
    fn a_plain_paragraph_has_no_style() {
        let c = build("text\n");
        assert_eq!(paragraphs(&c)[0].style, None);
    }

    #[test]
    fn emphasis_becomes_run_properties() {
        let c = build("**b** *i* ~~s~~\n");
        let p = props(paragraphs(&c)[0]);
        assert!(p[0].bold, "bold");
        assert!(p[1].italic, "italic");
        assert!(p[2].strike, "strike");
    }

    #[test]
    fn nested_emphasis_combines_properties() {
        let c = build("***both***\n");
        let p = props(paragraphs(&c)[0]);
        assert!(p[0].bold && p[0].italic);
    }

    #[test]
    fn inline_code_uses_the_code_character_style() {
        let c = build("`x`\n");
        let Run::Text(t) = &paragraphs(&c)[0].runs[0] else {
            panic!("expected a text run");
        };
        assert_eq!(t.props.style.as_deref(), Some("HTMLCode"));
    }

    #[test]
    fn a_link_is_recorded_once() {
        let c = build("[a](https://x.y) [b](https://x.y)\n");
        assert_eq!(c.links, vec!["https://x.y".to_string()]);
    }

    #[test]
    fn a_link_marks_its_run() {
        let c = build("[a](https://x.y)\n");
        assert_eq!(props(paragraphs(&c)[0])[0].link.as_deref(), Some("https://x.y"));
    }

    #[test]
    fn a_bullet_list_uses_the_bullet_numbering() {
        let c = build("- a\n- b\n");
        let p = paragraphs(&c);
        assert_eq!(p[0].style.as_deref(), Some("ListParagraph"));
        let numbering = p[0].props.numbering.as_ref().expect("numbering");
        assert_eq!(numbering.num_id, c.numbering.bullet_num_id);
        assert_eq!(numbering.level, 0);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn each_ordered_list_gets_its_own_numbering() {
        // A paragraph between the lists ends the first one.
        let c = build("1. a\n\nbetween\n\n1. b\n");
        assert_eq!(c.numbering.ordered.len(), 2);
        assert_ne!(c.numbering.ordered[0].num_id, c.numbering.ordered[1].num_id);
    }

    #[test]
    fn an_ordered_list_keeps_its_start_number() {
        let c = build("3. a\n");
        assert_eq!(c.numbering.ordered[0].start, 3);
    }

    #[test]
    fn nested_lists_deepen_the_level() {
        let c = build("- a\n  - b\n");
        let levels: Vec<u8> = paragraphs(&c)
            .iter()
            .filter_map(|p| p.props.numbering.as_ref().map(|n| n.level))
            .collect();
        assert_eq!(levels, vec![0, 1]);
    }

    #[test]
    fn a_task_item_gets_a_checkbox_prefix() {
        let c = build("- [x] done\n");
        let Run::Text(t) = &paragraphs(&c)[0].runs[0] else {
            panic!("expected a text run");
        };
        assert_eq!(t.text, "☒ ");
    }

    #[test]
    fn an_open_task_item_gets_an_empty_box() {
        let c = build("- [ ] todo\n");
        let Run::Text(t) = &paragraphs(&c)[0].runs[0] else {
            panic!("expected a text run");
        };
        assert_eq!(t.text, "☐ ");
    }

    #[test]
    fn a_table_becomes_a_table_with_a_header_row() {
        let c = build("| a | b |\n|---|---|\n| 1 | 2 |\n");
        let Block::Table(t) = &c.document.body[0] else {
            panic!("expected a table");
        };
        assert_eq!(t.rows.len(), 2);
        assert!(t.rows[0].header);
        assert_eq!(t.rows[0].cells.len(), 2);
        assert_eq!(t.widths.len(), 2);
    }

    #[test]
    fn a_table_header_is_bold() {
        let c = build("| a |\n|---|\n| 1 |\n");
        let Block::Table(t) = &c.document.body[0] else {
            panic!("expected a table");
        };
        let Run::Text(text) = &t.rows[0].cells[0].paragraphs[0].runs[0] else {
            panic!("expected a text run");
        };
        assert!(text.props.bold);
    }

    #[test]
    fn a_table_keeps_the_column_alignment() {
        let c = build("| a | b |\n|:--|--:|\n| 1 | 2 |\n");
        let Block::Table(t) = &c.document.body[0] else {
            panic!("expected a table");
        };
        assert_eq!(t.rows[1].cells[0].align, Align::Left);
        assert_eq!(t.rows[1].cells[1].align, Align::Right);
    }

    #[test]
    fn a_table_pads_short_rows() {
        let c = build("| a | b |\n|---|---|\n| 1 |\n");
        let Block::Table(t) = &c.document.body[0] else {
            panic!("expected a table");
        };
        assert_eq!(t.rows[1].cells.len(), 2);
    }

    #[test]
    fn a_thematic_break_becomes_a_bordered_paragraph() {
        let c = build("---\n");
        assert!(paragraphs(&c)[0].props.bottom_border);
    }

    #[test]
    fn a_quote_uses_the_quote_style() {
        let c = build("> q\n");
        assert_eq!(paragraphs(&c)[0].style.as_deref(), Some("Quote"));
    }

    #[test]
    fn a_nested_quote_is_indented_further() {
        let c = build("> outer\n>\n> > inner\n");
        let p = paragraphs(&c);
        assert_eq!(p[0].props.indent_left, 720);
        assert_eq!(p[1].props.indent_left, 1440);
    }

    #[test]
    fn a_code_block_becomes_one_paragraph_per_line() {
        let c = build("```\na\nb\n```\n");
        assert_eq!(paragraphs(&c).len(), 2);
        assert_eq!(paragraphs(&c)[0].style.as_deref(), Some("HTMLPreformatted"));
    }

    #[test]
    fn a_code_block_keeps_its_language_out_of_the_text() {
        let c = build("```rust\nlet x = 1;\n```\n");
        let Run::Text(t) = &paragraphs(&c)[0].runs[0] else {
            panic!("expected a text run");
        };
        assert_eq!(t.text, "let x = 1;");
    }

    #[test]
    fn a_line_break_becomes_a_break_run() {
        let c = build("a  \nb\n");
        assert!(paragraphs(&c)[0]
            .runs
            .iter()
            .any(|r| matches!(r, Run::Break)));
    }

    #[test]
    fn raw_html_is_mapped_by_tag() {
        let c = build("a <u>b</u> <b>c</b> <i>d</i> <s>e</s>\n");
        let p = props(paragraphs(&c)[0]);
        assert!(p.iter().any(|p| p.underline), "underline");
        assert!(p.iter().any(|p| p.bold), "bold");
        assert!(p.iter().any(|p| p.italic), "italic");
        assert!(p.iter().any(|p| p.strike), "strike");
    }

    #[test]
    fn raw_html_links_are_recorded() {
        let c = build("a <a href=\"https://x.y\">b</a>\n");
        assert_eq!(c.links, vec!["https://x.y".to_string()]);
    }

    #[test]
    fn an_html_block_becomes_ordinary_blocks() {
        let c = build("<div><h2>Title</h2><p>Body</p></div>\n");
        let p = paragraphs(&c);
        assert_eq!(p[0].style.as_deref(), Some("Heading2"));
        assert_eq!(p[1].style, None);
    }

    #[test]
    fn a_missing_image_degrades_to_its_alt_text() {
        let c = build("![the alt](no-such-image.png)\n");
        let Run::Text(t) = &paragraphs(&c)[0].runs[0] else {
            panic!("expected a text run");
        };
        assert_eq!(t.text, "the alt");
        assert!(c.images.is_empty());
    }

    #[test]
    fn local_images_can_be_switched_off() {
        let options = Options::default().with_local_images(false);
        let c = build_with("![the alt](image.png)\n", &options);
        assert!(c.images.is_empty());
    }

    #[test]
    fn a_remote_image_is_never_fetched() {
        let c = build("![the alt](https://example.com/image.png)\n");
        assert!(c.images.is_empty());
    }

    #[test]
    fn the_toc_flag_is_carried_over() {
        let options = Options::default().with_toc(true);
        assert!(build_with("# Hi\n", &options).toc);
        assert!(!build("# Hi\n").toc);
    }

    #[test]
    fn an_empty_document_produces_an_empty_body() {
        assert!(build("").document.body.is_empty());
    }

    #[test]
    fn percent_escapes_are_decoded_in_paths() {
        assert_eq!(percent_decode("my%20image.png"), "my image.png");
        assert_eq!(percent_decode("plain.png"), "plain.png");
        assert_eq!(percent_decode("100%"), "100%");
    }

    #[test]
    fn an_unmatched_close_tag_is_ignored() {
        let c = build("a </b> b\n");
        assert_eq!(paragraphs(&c).len(), 1);
    }

    #[test]
    fn a_self_closing_tag_does_not_open_a_style() {
        let c = build("a <br/> b\n");
        assert!(paragraphs(&c)[0]
            .runs
            .iter()
            .any(|r| matches!(r, Run::Break)));
        // The text after it is not styled.
        let p = props(paragraphs(&c)[0]);
        assert!(p.iter().all(|p| !p.bold && !p.italic));
    }

    #[test]
    fn an_anchor_without_a_href_stays_plain_text() {
        let c = build("a <a>b</a>\n");
        assert!(c.links.is_empty());
    }

    #[test]
    fn a_data_url_is_never_fetched() {
        let c = build("![alt](data:image/png;base64,AAAA)\n");
        assert!(c.images.is_empty());
    }

    #[test]
    fn an_image_resolves_against_the_base_directory() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("markpop-docx-base-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let pixels: Vec<u8> = vec![0; 4 * 4 * 4];
        std::fs::write(
            dir.join("pic.png"),
            crate::encode::png::encode(2, 2, &pixels),
        )
        .expect("write");
        let options = Options::default().with_base_dir(dir.to_string_lossy().to_string());
        let c = build_with("![alt](pic.png)\n", &options);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(c.images.len(), 1);
        assert_eq!(c.images[0].name, "image1.png");
    }

    #[test]
    fn an_image_without_a_base_directory_resolves_against_the_working_one() {
        // The path is used as given; the file does not exist, so the image
        // degrades rather than failing.
        let c = build("![alt](pic.png)\n");
        assert!(c.images.is_empty());
    }

    #[test]
    fn two_images_get_two_parts() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("markpop-docx-two-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let pixels: Vec<u8> = vec![0; 4 * 4 * 4];
        let png = crate::encode::png::encode(2, 2, &pixels);
        std::fs::write(dir.join("a.png"), &png).expect("write");
        std::fs::write(dir.join("b.png"), &png).expect("write");
        let options = Options::default().with_base_dir(dir.to_string_lossy().to_string());
        let c = build_with("![a](a.png)\n\n![b](b.png)\n", &options);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(c.images.len(), 2);
        assert_eq!(c.images[0].name, "image1.png");
        assert_eq!(c.images[1].name, "image2.png");
    }
}
