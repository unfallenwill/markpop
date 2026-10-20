//! The Markdown document model.
//!
//! This is a pragmatic CommonMark subset plus GFM tables and
//! strikethrough. Blocks and inlines are separate enums so layout can
//! recurse structurally (quotes and list items contain whole block
//! sequences) while text styling composes through inline nesting.

/// A parsed Markdown document: a sequence of top-level blocks.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// The top-level blocks, in document order.
    pub blocks: Vec<Block>,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// An ATX (`# Heading`) or Setext (underline) heading.
    Heading {
        /// Level `1..=6`.
        level: u8,
        /// Inline content of the heading.
        inlines: Vec<Inline>,
    },
    /// A paragraph of inline content.
    Paragraph(Vec<Inline>),
    /// A fenced or indented code block.
    Code(CodeBlock),
    /// A blockquote; contains block-level children (possibly nested quotes).
    Quote(Vec<Block>),
    /// A bullet or ordered list.
    List(ListData),
    /// A GFM pipe table.
    Table(TableData),
    /// A standalone image (a paragraph whose only child is an image).
    Figure(ImageData),
    /// A thematic break (`---`, `***`, `___`).
    ThematicBreak,
    /// A raw HTML block, kept verbatim as text (never executed).
    Html(String),
}

/// A code block: raw lines plus the info string from the opening fence.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeBlock {
    /// Info string (e.g. `rust` in ` ```rust `), possibly empty.
    pub info: String,
    /// The literal code lines, without the fences.
    pub lines: Vec<String>,
}

/// A list and its items.
#[derive(Debug, Clone, PartialEq)]
pub struct ListData {
    /// `true` for ordered lists (`1. …`), `false` for bullets.
    pub ordered: bool,
    /// Starting number for ordered lists (`<ol start>`).
    pub start: u32,
    /// Tight lists have no blank lines between items; layout renders them
    /// with reduced spacing.
    pub tight: bool,
    /// The list items.
    pub items: Vec<ListItem>,
}

/// One list item: a sequence of blocks (paragraphs, nested lists, …).
#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    /// Block content of the item.
    pub blocks: Vec<Block>,
    /// GFM task state: `Some(true)` when checked, `Some(false)` when not.
    pub task: Option<bool>,
}

/// Column alignment for table columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// No explicit alignment.
    #[default]
    None,
    /// `:---`
    Left,
    /// `:---:`
    Center,
    /// `---:`
    Right,
}

/// A GFM table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    /// Alignment for each column (length = column count).
    pub aligns: Vec<Align>,
    /// Header row: inline content per column.
    pub head: Vec<Vec<Inline>>,
    /// Body rows.
    pub rows: Vec<Vec<Vec<Inline>>>,
}

/// An image reference. Images cannot be fetched (SVG output stays
/// self-contained), so they render as labeled figures.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageData {
    /// Alt text.
    pub alt: String,
    /// Source URL, shown in the figure caption.
    pub url: String,
    /// Optional title from `![alt](url "title")` or a definition.
    pub title: Option<String>,
}

/// An inline (text-level) element.
#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// A run of literal text (already unescaped).
    Text(String),
    /// An inline code span.
    Code(String),
    /// Emphasis (`*x*` / `_x_`).
    Emph(Vec<Inline>),
    /// Strong emphasis (`**x**` / `__x__`).
    Strong(Vec<Inline>),
    /// Strikethrough (`~~x~~`).
    Strike(Vec<Inline>),
    /// A hyperlink.
    Link {
        /// The visible text (may itself contain styled inlines).
        text: Vec<Inline>,
        /// Target URL.
        url: String,
        /// Optional title from `[text](url "title")` or a definition.
        title: Option<String>,
    },
    /// An inline image; renders as a compact chip.
    Image(ImageData),
    /// A hard line break (two trailing spaces or a trailing backslash).
    Break,
    /// A soft line break; layout renders it as a space.
    SoftBreak,
    /// Raw inline HTML, kept verbatim as text (never executed).
    Html(String),
}

impl Inline {
    /// Collects the plain text of this inline subtree (no decorations).
    ///
    /// Useful for measuring, alt text and debug output.
    pub fn plain_text(&self) -> String {
        match self {
            Inline::Text(s) | Inline::Code(s) => s.clone(),
            Inline::Emph(c) | Inline::Strong(c) | Inline::Strike(c) => plain_text_of(c),
            Inline::Link { text, .. } => plain_text_of(text),
            Inline::Image(img) => img.alt.clone(),
            Inline::Break => String::new(),
            Inline::SoftBreak => " ".to_string(),
            Inline::Html(raw) => raw.clone(),
        }
    }
}

/// Collects the plain text of an inline sequence.
pub fn plain_text_of(inlines: &[Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        out.push_str(&inline.plain_text());
    }
    out
}

impl Document {
    /// Counts blocks of each kind, for a quick structural fingerprint.
    ///
    /// Recurses into quotes and list items. The returned vector is indexed
    /// by the block discriminant order in this file; tests use it to
    /// assert parse results without deep comparisons.
    pub fn block_counts(&self) -> BlockCounts {
        count_blocks(&self.blocks)
    }
}

/// Per-variant block counts. See [`Document::block_counts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockCounts {
    /// Number of [`Block::Heading`] nodes.
    pub headings: usize,
    /// Number of [`Block::Paragraph`] nodes.
    pub paragraphs: usize,
    /// Number of [`Block::Code`] nodes.
    pub code: usize,
    /// Number of [`Block::Quote`] nodes.
    pub quotes: usize,
    /// Number of [`Block::List`] nodes.
    pub lists: usize,
    /// Number of [`Block::Table`] nodes.
    pub tables: usize,
    /// Number of [`Block::Figure`] nodes.
    pub figures: usize,
    /// Number of [`Block::ThematicBreak`] nodes.
    pub thematic_breaks: usize,
    /// Number of [`Block::Html`] nodes.
    pub html_blocks: usize,
}

fn count_blocks(blocks: &[Block]) -> BlockCounts {
    let mut c = BlockCounts::default();
    for b in blocks {
        match b {
            Block::Heading { .. } => c.headings += 1,
            Block::Paragraph(_) => c.paragraphs += 1,
            Block::Code(_) => c.code += 1,
            Block::Quote(inner) => {
                c.quotes += 1;
                c.add(count_blocks(inner));
            }
            Block::List(l) => {
                c.lists += 1;
                for item in &l.items {
                    c.add(count_blocks(&item.blocks));
                }
            }
            Block::Table(_) => c.tables += 1,
            Block::Figure(_) => c.figures += 1,
            Block::ThematicBreak => c.thematic_breaks += 1,
            Block::Html(_) => c.html_blocks += 1,
        }
    }
    c
}

impl BlockCounts {
    fn add(&mut self, other: BlockCounts) {
        let o = other;
        self.headings += o.headings;
        self.paragraphs += o.paragraphs;
        self.code += o.code;
        self.quotes += o.quotes;
        self.lists += o.lists;
        self.tables += o.tables;
        self.figures += o.figures;
        self.thematic_breaks += o.thematic_breaks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_flattens_styling() {
        let inlines = vec![
            Inline::Text("hello ".into()),
            Inline::Strong(vec![
                Inline::Emph(vec![Inline::Text("big".into())]),
                Inline::Code("code".into()),
            ]),
            Inline::Link {
                text: vec![Inline::Text(" world".into())],
                url: "https://example.com".into(),
                title: None,
            },
        ];
        assert_eq!(plain_text_of(&inlines), "hello bigcode world");
    }

    #[test]
    fn block_counts_recurse() {
        let doc = Document {
            blocks: vec![
                Block::Heading {
                    level: 1,
                    inlines: vec![Inline::Text("t".into())],
                },
                Block::Quote(vec![
                    Block::Paragraph(vec![Inline::Text("q".into())]),
                    Block::List(ListData {
                        ordered: false,
                        start: 1,
                        tight: true,
                        items: vec![ListItem {
                            task: None,
                            blocks: vec![Block::Paragraph(vec![Inline::Text("i".into())])],
                        }],
                    }),
                ]),
                Block::ThematicBreak,
            ],
        };
        let c = doc.block_counts();
        assert_eq!(c.headings, 1);
        assert_eq!(c.quotes, 1);
        assert_eq!(c.paragraphs, 2);
        assert_eq!(c.lists, 1);
        assert_eq!(c.thematic_breaks, 1);
    }

    #[test]
    fn block_counts_cover_every_variant() {
        let doc = Document {
            blocks: vec![
                Block::Code(CodeBlock {
                    info: "rust".into(),
                    lines: vec!["x".into()],
                }),
                Block::Table(TableData {
                    aligns: vec![Align::Left],
                    head: vec![vec![Inline::Text("h".into())]],
                    rows: vec![],
                }),
                Block::Figure(ImageData {
                    alt: "a".into(),
                    url: "u".into(),
                    title: None,
                }),
                Block::Paragraph(vec![Inline::Break]),
            ],
        };
        let c = doc.block_counts();
        assert_eq!(c.code, 1);
        assert_eq!(c.tables, 1);
        assert_eq!(c.figures, 1);
        assert_eq!(c.paragraphs, 1);
        assert_eq!(c.headings + c.quotes + c.lists + c.thematic_breaks, 0);
    }

    #[test]
    fn plain_text_handles_images_and_breaks() {
        assert_eq!(
            Inline::Image(ImageData {
                alt: "pic".into(),
                url: "u".into(),
                title: None
            })
            .plain_text(),
            "pic"
        );
        assert_eq!(Inline::Break.plain_text(), "");
        assert_eq!(Inline::Code("c".into()).plain_text(), "c");
    }

    #[test]
    fn align_default_is_none() {
        assert_eq!(Align::default(), Align::None);
    }
}
