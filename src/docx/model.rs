//! The Word document model: what a `.docx` contains, before it becomes XML.

use crate::ast::Align;

/// A Word document body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// The body blocks, in document order.
    pub body: Vec<Block>,
}

/// A block-level element.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A paragraph.
    Paragraph(Paragraph),
    /// A table.
    Table(Table),
}

/// A paragraph and its runs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    /// The style ID (`Heading1`, `Quote`, …). `None` means `Normal`.
    pub style: Option<String>,
    /// Paragraph-level properties.
    pub props: ParagraphProps,
    /// The runs, in order.
    pub runs: Vec<Run>,
}

/// Paragraph-level properties.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParagraphProps {
    /// The list this paragraph belongs to.
    pub numbering: Option<Numbering>,
    /// Left indent in twips.
    pub indent_left: u32,
    /// Whether a horizontal rule is drawn below the paragraph.
    pub bottom_border: bool,
}

/// A reference to a numbering definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Numbering {
    /// The `w:num` identifier.
    pub num_id: u32,
    /// The nesting level, `0` for the outermost list.
    pub level: u8,
}

/// A run of inline content.
#[derive(Debug, Clone, PartialEq)]
pub enum Run {
    /// Text with character properties.
    Text(TextRun),
    /// A line break.
    Break,
    /// An inline picture.
    Image(Image),
}

/// A text run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextRun {
    /// The literal text.
    pub text: String,
    /// Character properties.
    pub props: RunProps,
}

/// Character properties of a run.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunProps {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Struck through.
    pub strike: bool,
    /// Underlined.
    pub underline: bool,
    /// A character style ID (`HTMLCode`, `Hyperlink`).
    pub style: Option<String>,
    /// The URL this run links to.
    pub link: Option<String>,
}

/// An inline picture.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// The image's position in the package's image list; the writer turns
    /// it into the relationship ID.
    pub index: u32,
    /// Width in EMU.
    pub width_emu: u32,
    /// Height in EMU.
    pub height_emu: u32,
    /// Alternative text.
    pub alt: String,
    /// The part's file name.
    pub name: String,
}

/// A table.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// The rows.
    pub rows: Vec<Row>,
    /// Column widths in twips.
    pub widths: Vec<u32>,
}

/// A table row.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The cells.
    pub cells: Vec<Cell>,
    /// Whether this row repeats as a header on every page.
    pub header: bool,
}

/// A table cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// The cell's paragraphs.
    pub paragraphs: Vec<Paragraph>,
    /// Horizontal alignment.
    pub align: Align,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_paragraph_carries_a_style_and_runs() {
        let p = Paragraph {
            style: Some("Heading1".into()),
            props: ParagraphProps::default(),
            runs: vec![Run::Text(TextRun {
                text: "Hi".into(),
                props: RunProps::default(),
            })],
        };
        assert_eq!(p.style.as_deref(), Some("Heading1"));
        assert_eq!(p.runs.len(), 1);
    }

    #[test]
    fn run_props_default_to_plain_text() {
        let p = RunProps::default();
        assert!(!p.bold && !p.italic && !p.strike && !p.underline);
        assert!(p.style.is_none() && p.link.is_none());
    }

    #[test]
    fn paragraph_props_default_to_an_ordinary_paragraph() {
        let p = ParagraphProps::default();
        assert!(p.numbering.is_none());
        assert_eq!(p.indent_left, 0);
        assert!(!p.bottom_border);
    }

    #[test]
    fn an_empty_document_has_no_body() {
        assert!(Document::default().body.is_empty());
    }
}
