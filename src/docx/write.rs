//! Writing the OOXML parts of a Word document.
//!
//! The parts follow Word's own conventions: built-in style IDs with
//! Word's default look, hybrid multilevel numbering, A4 pages with
//! one-inch margins. Element order inside each part follows the
//! WordprocessingML schema, which Word checks strictly.

use crate::ast::Align;

use super::content::{Content, ImagePart, NumberingPlan};
use super::model::{Block, Paragraph, Run, RunProps, Table};
use super::xml::Element;

/// The XML declaration every part starts with.
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
/// The WordprocessingML namespace.
const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
/// The relationship namespace.
const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
/// The wordprocessing drawing namespace.
const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
/// The DrawingML namespace.
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
/// The picture namespace.
const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
/// The package content-types namespace.
const NS_CT: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
/// The package relationships namespace.
const NS_PKG_REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
/// The relationship type of the main document part.
const REL_OFFICE_DOCUMENT: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
/// The relationship type of a styles part.
const REL_STYLES: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";
/// The relationship type of a numbering part.
const REL_NUMBERING: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering";
/// The relationship type of a settings part.
const REL_SETTINGS: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings";
/// The relationship type of an image part.
const REL_IMAGE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
/// The relationship type of an external hyperlink.
const REL_HYPERLINK: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink";
/// A4 portrait, in twips.
const PAGE_WIDTH: u32 = 11906;
/// A4 portrait, in twips.
const PAGE_HEIGHT: u32 = 16838;
/// The margin on every side, in twips.
const MARGIN: u32 = 1440;
/// The number of relationship IDs reserved for the parts Word needs
/// before the document's own images and links.
const RESERVED_RELS: u32 = 4;

/// The relationship ID of an image part.
pub fn image_rel_id(index: u32) -> String {
    format!("rId{}", RESERVED_RELS + index)
}

/// The relationship ID of an external hyperlink.
pub fn link_rel_id(images: usize, index: usize) -> String {
    format!("rId{}", RESERVED_RELS as usize + images + index)
}

/// The `[Content_Types].xml` part.
///
/// Every part in the package must be covered here, or Word offers to
/// repair the file.
pub fn content_types(images: usize) -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<Types xmlns=\"{NS_CT}\">"));
    out.push_str(
        "<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>",
    );
    out.push_str("<Default Extension=\"xml\" ContentType=\"application/xml\"/>");
    if images > 0 {
        out.push_str("<Default Extension=\"png\" ContentType=\"image/png\"/>");
    }
    out.push_str(
        "<Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>",
    );
    out.push_str(
        "<Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/>",
    );
    out.push_str(
        "<Override PartName=\"/word/numbering.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml\"/>",
    );
    out.push_str(
        "<Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/>",
    );
    out.push_str("</Types>");
    out
}

/// The package relationships: the document part is the only one.
pub fn package_rels() -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<Relationships xmlns=\"{NS_PKG_REL}\">"));
    out.push_str(&format!(
        "<Relationship Id=\"rId1\" Type=\"{REL_OFFICE_DOCUMENT}\" Target=\"word/document.xml\"/>"
    ));
    out.push_str("</Relationships>");
    out
}

/// The document's relationships: the parts Word needs, then the images
/// and the links the document itself uses.
pub fn document_rels(images: &[ImagePart], links: &[String]) -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<Relationships xmlns=\"{NS_PKG_REL}\">"));
    out.push_str(&format!(
        "<Relationship Id=\"rId1\" Type=\"{REL_STYLES}\" Target=\"styles.xml\"/>"
    ));
    out.push_str(&format!(
        "<Relationship Id=\"rId2\" Type=\"{REL_NUMBERING}\" Target=\"numbering.xml\"/>"
    ));
    out.push_str(&format!(
        "<Relationship Id=\"rId3\" Type=\"{REL_SETTINGS}\" Target=\"settings.xml\"/>"
    ));
    for (index, image) in images.iter().enumerate() {
        out.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{REL_IMAGE}\" Target=\"media/{}\"/>",
            image_rel_id(index as u32),
            image.name
        ));
    }
    for (index, link) in links.iter().enumerate() {
        out.push_str(&format!(
            "<Relationship Id=\"{}\" Type=\"{REL_HYPERLINK}\" Target=\"{}\" TargetMode=\"External\"/>",
            link_rel_id(images.len(), index),
            super::xml::escape_attr(link)
        ));
    }
    out.push_str("</Relationships>");
    out
}

/// The styles part: Word's built-in styles with Word's own default look.
pub fn styles() -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<w:styles xmlns:w=\"{NS_W}\">"));
    out.push_str(
        "<w:docDefaults><w:rPrDefault><w:rPr>\
         <w:rFonts w:ascii=\"Calibri\" w:hAnsi=\"Calibri\" w:cs=\"Times New Roman\"/>\
         <w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/>\
         <w:lang w:val=\"en-US\" w:eastAsia=\"zh-CN\" w:bidi=\"ar-SA\"/>\
         </w:rPr></w:rPrDefault>\
         <w:pPrDefault><w:pPr><w:spacing w:after=\"160\" w:line=\"259\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault>\
         </w:docDefaults>",
    );
    out.push_str(
        "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">\
         <w:name w:val=\"Normal\"/><w:qFormat/></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"character\" w:default=\"1\" w:styleId=\"DefaultParagraphFont\">\
         <w:name w:val=\"Default Paragraph Font\"/><w:uiPriority w:val=\"1\"/>\
         <w:semiHidden/><w:unhideWhenUsed/></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\">\
         <w:name w:val=\"Normal Table\"/><w:uiPriority w:val=\"99\"/>\
         <w:semiHidden/><w:unhideWhenUsed/>\
         <w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/>\
         <w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>\
         <w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar>\
         </w:tblPr></w:style>",
    );
    for (level, size, color, before) in HEADINGS {
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"Heading{level}\">\
             <w:name w:val=\"heading {level}\"/><w:basedOn w:val=\"Normal\"/>\
             <w:next w:val=\"Normal\"/><w:uiPriority w:val=\"9\"/><w:qFormat/>\
             <w:pPr><w:keepNext/><w:keepLines/>\
             <w:spacing w:before=\"{before}\" w:after=\"0\" w:line=\"259\" w:lineRule=\"auto\"/>\
             <w:outlineLvl w:val=\"{}\"/></w:pPr>\
             <w:rPr><w:rFonts w:ascii=\"Calibri Light\" w:hAnsi=\"Calibri Light\"/>\
             <w:color w:val=\"{color}\"/><w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/></w:rPr>\
             </w:style>",
            level - 1
        ));
    }
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"Quote\">\
         <w:name w:val=\"Quote\"/><w:basedOn w:val=\"Normal\"/><w:next w:val=\"Normal\"/>\
         <w:uiPriority w:val=\"29\"/><w:qFormat/>\
         <w:pPr><w:spacing w:before=\"200\" w:after=\"160\"/>\
         <w:ind w:left=\"720\" w:right=\"720\"/></w:pPr>\
         <w:rPr><w:i/><w:iCs/><w:color w:val=\"404040\"/></w:rPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"ListParagraph\">\
         <w:name w:val=\"List Paragraph\"/><w:basedOn w:val=\"Normal\"/>\
         <w:uiPriority w:val=\"34\"/><w:qFormat/>\
         <w:pPr><w:ind w:left=\"720\"/><w:contextualSpacing/></w:pPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"HTMLPreformatted\">\
         <w:name w:val=\"HTML Preformatted\"/><w:basedOn w:val=\"Normal\"/>\
         <w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/>\
         <w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
         <w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/>\
         <w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/></w:rPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"character\" w:styleId=\"HTMLCode\">\
         <w:name w:val=\"HTML Code\"/><w:basedOn w:val=\"DefaultParagraphFont\"/>\
         <w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/>\
         <w:rPr><w:rFonts w:ascii=\"Consolas\" w:hAnsi=\"Consolas\" w:cs=\"Consolas\"/>\
         <w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/></w:rPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"character\" w:styleId=\"Hyperlink\">\
         <w:name w:val=\"Hyperlink\"/><w:basedOn w:val=\"DefaultParagraphFont\"/>\
         <w:uiPriority w:val=\"99\"/><w:unhideWhenUsed/>\
         <w:rPr><w:color w:val=\"0563C1\"/><w:u w:val=\"single\"/></w:rPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"table\" w:styleId=\"TableGrid\">\
         <w:name w:val=\"Table Grid\"/><w:basedOn w:val=\"TableNormal\"/>\
         <w:uiPriority w:val=\"39\"/>\
         <w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr>\
         <w:tblPr><w:tblBorders>\
         <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"auto\"/>\
         </w:tblBorders></w:tblPr></w:style>",
    );
    out.push_str(
        "<w:style w:type=\"paragraph\" w:styleId=\"TOCHeading\">\
         <w:name w:val=\"TOC Heading\"/><w:basedOn w:val=\"Heading1\"/>\
         <w:next w:val=\"Normal\"/><w:uiPriority w:val=\"39\"/>\
         <w:semiHidden/><w:unhideWhenUsed/><w:qFormat/>\
         <w:pPr><w:outlineLvl w:val=\"9\"/></w:pPr></w:style>",
    );
    for (level, indent) in [(1, 0), (2, 220), (3, 440)] {
        out.push_str(&format!(
            "<w:style w:type=\"paragraph\" w:styleId=\"TOC{level}\">\
             <w:name w:val=\"toc {level}\"/><w:basedOn w:val=\"Normal\"/>\
             <w:next w:val=\"Normal\"/><w:autoRedefine/><w:uiPriority w:val=\"39\"/>\
             <w:unhideWhenUsed/>\
             <w:pPr><w:spacing w:after=\"100\"/><w:ind w:left=\"{indent}\"/></w:pPr></w:style>"
        ));
    }
    out.push_str("</w:styles>");
    out
}

/// The heading styles: level, size in half-points, colour, space before.
const HEADINGS: [(u8, u32, &str, u32); 6] = [
    (1, 32, "2F5496", 240),
    (2, 26, "2F5496", 40),
    (3, 24, "1F3763", 40),
    (4, 22, "1F3763", 40),
    (5, 22, "1F3763", 40),
    (6, 22, "1F3763", 40),
];

/// The bullet characters of Word's hybrid multilevel list.
const BULLETS: [(&str, &str); 3] = [
    ("\u{f0b7}", "Symbol"),
    ("o", "Courier New"),
    ("\u{f0a7}", "Wingdings"),
];

/// The numbering part: two abstract definitions and one instance per
/// list.
pub fn numbering(plan: &NumberingPlan) -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<w:numbering xmlns:w=\"{NS_W}\">"));

    out.push_str("<w:abstractNum w:abstractNumId=\"0\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
    for level in 0..9u32 {
        let (glyph, font) = BULLETS[(level % 3) as usize];
        out.push_str(&format!(
            "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"bullet\"/>\
             <w:lvlText w:val=\"{glyph}\"/><w:lvlJc w:val=\"left\"/>\
             <w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr>\
             <w:rPr><w:rFonts w:ascii=\"{font}\" w:hAnsi=\"{font}\" w:hint=\"default\"/></w:rPr></w:lvl>",
            720 * (level + 1)
        ));
    }
    out.push_str("</w:abstractNum>");

    out.push_str("<w:abstractNum w:abstractNumId=\"1\"><w:multiLevelType w:val=\"hybridMultilevel\"/>");
    for level in 0..9u32 {
        let pattern: String = (1..=level + 1)
            .map(|n| format!("%{n}."))
            .collect::<Vec<_>>()
            .join("");
        out.push_str(&format!(
            "<w:lvl w:ilvl=\"{level}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/>\
             <w:lvlText w:val=\"{pattern}\"/><w:lvlJc w:val=\"left\"/>\
             <w:pPr><w:ind w:left=\"{}\" w:hanging=\"360\"/></w:pPr></w:lvl>",
            720 * (level + 1)
        ));
    }
    out.push_str("</w:abstractNum>");

    out.push_str(&format!(
        "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"0\"/></w:num>",
        plan.bullet_num_id
    ));
    for list in &plan.ordered {
        out.push_str(&format!(
            "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"1\"/>\
             <w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"{}\"/></w:lvlOverride></w:num>",
            list.num_id, list.start
        ));
    }
    out.push_str("</w:numbering>");
    out
}

/// The settings part.
///
/// `update_fields` asks Word to refresh the fields — and therefore the
/// table of contents — when the document is opened.
pub fn settings(update_fields: bool) -> String {
    let mut out = String::from(DECLARATION);
    out.push_str(&format!("<w:settings xmlns:w=\"{NS_W}\">"));
    out.push_str("<w:defaultTabStop w:val=\"720\"/>");
    if update_fields {
        out.push_str("<w:updateFields w:val=\"true\"/>");
    }
    out.push_str("</w:settings>");
    out
}

/// The main document part.
pub fn document(content: &Content) -> String {
    let mut body: Vec<Element> = Vec::new();
    if content.toc {
        body.push(toc_heading(&content.toc_title));
        body.push(toc_field());
    }
    for block in &content.document.body {
        match block {
            Block::Paragraph(paragraph) => body.push(paragraph_element(paragraph, content)),
            Block::Table(table) => {
                body.push(table_element(table, content));
                // Word requires a paragraph after a table; without one,
                // two adjacent tables merge into one.
                body.push(Element::new("w:p"));
            }
        }
    }
    body.push(section_properties());

    let root = Element::new("w:document")
        .attr("xmlns:w", NS_W)
        .attr("xmlns:r", NS_R)
        .attr("xmlns:wp", NS_WP)
        .attr("xmlns:a", NS_A)
        .attr("xmlns:pic", NS_PIC)
        .child(Element::new("w:body").children(body));

    let mut out = String::from(DECLARATION);
    root.write(&mut out);
    out
}

/// The heading above the table of contents.
fn toc_heading(title: &str) -> Element {
    Element::new("w:p")
        .child(
            Element::new("w:pPr").child(Element::new("w:pStyle").attr("w:val", "TOCHeading")),
        )
        .child(
            Element::new("w:r").child(
                Element::new("w:t")
                    .attr("xml:space", "preserve")
                    .text(title.to_string()),
            ),
        )
}

/// The table-of-contents field.
///
/// Word builds the entries itself when the document is opened, which is
/// why the field is marked dirty.
fn toc_field() -> Element {
    let field_char = |kind: &str| {
        Element::new("w:r").child(Element::new("w:fldChar").attr("w:fldCharType", kind))
    };
    Element::new("w:p")
        .child(
            Element::new("w:r").child(
                Element::new("w:fldChar")
                    .attr("w:fldCharType", "begin")
                    .attr("w:dirty", "true"),
            ),
        )
        .child(
            Element::new("w:r").child(
                Element::new("w:instrText")
                    .attr("xml:space", "preserve")
                    .text(" TOC \\o \"1-3\" \\h \\z \\u "),
            ),
        )
        .child(field_char("separate"))
        .child(
            Element::new("w:r").child(
                Element::new("w:t").text(
                    "Right-click here and choose “Update Field” to build the table of contents.",
                ),
            ),
        )
        .child(field_char("end"))
}

/// The page setup: A4 portrait with one-inch margins.
fn section_properties() -> Element {
    Element::new("w:sectPr")
        .child(
            Element::new("w:pgSz")
                .attr("w:w", PAGE_WIDTH.to_string())
                .attr("w:h", PAGE_HEIGHT.to_string()),
        )
        .child(
            Element::new("w:pgMar")
                .attr("w:top", MARGIN.to_string())
                .attr("w:right", MARGIN.to_string())
                .attr("w:bottom", MARGIN.to_string())
                .attr("w:left", MARGIN.to_string())
                .attr("w:header", "708")
                .attr("w:footer", "708")
                .attr("w:gutter", "0"),
        )
        .child(Element::new("w:cols").attr("w:space", "708"))
        .child(Element::new("w:docGrid").attr("w:linePitch", "360"))
}

/// Builds a paragraph element.
fn paragraph_element(paragraph: &Paragraph, content: &Content) -> Element {
    let mut props: Vec<Element> = Vec::new();
    if let Some(style) = &paragraph.style {
        props.push(Element::new("w:pStyle").attr("w:val", style.clone()));
    }
    if let Some(numbering) = &paragraph.props.numbering {
        props.push(
            Element::new("w:numPr")
                .child(Element::new("w:ilvl").attr("w:val", numbering.level.to_string()))
                .child(Element::new("w:numId").attr("w:val", numbering.num_id.to_string())),
        );
    }
    if paragraph.props.bottom_border {
        props.push(
            Element::new("w:pBdr").child(
                Element::new("w:bottom")
                    .attr("w:val", "single")
                    .attr("w:sz", "6")
                    .attr("w:space", "1")
                    .attr("w:color", "auto"),
            ),
        );
    }
    if paragraph.props.indent_left > 0 {
        props.push(
            Element::new("w:ind").attr("w:left", paragraph.props.indent_left.to_string()),
        );
    }

    let mut element = Element::new("w:p");
    if !props.is_empty() {
        element = element.child(Element::new("w:pPr").children(props));
    }
    for run in &paragraph.runs {
        element = element.child(run_element(run, content));
    }
    element
}

/// Builds the element of one run.
fn run_element(run: &Run, content: &Content) -> Element {
    match run {
        Run::Break => Element::new("w:r").child(Element::new("w:br")),
        Run::Image(image) => Element::new("w:r").child(drawing_element(image)),
        Run::Text(text) => {
            let element = Element::new("w:r")
                .child(run_properties(&text.props))
                .child(
                    Element::new("w:t")
                        .attr("xml:space", "preserve")
                        .text(text.text.clone()),
                );
            match text.props.link.as_deref().and_then(|url| link_id(url, content)) {
                Some(id) => Element::new("w:hyperlink")
                    .attr("r:id", id)
                    .attr("w:history", "1")
                    .child(element),
                None => element,
            }
        }
    }
}

/// The relationship ID of a link target.
///
/// Returns `None` for a target that was never registered, so a run can
/// fall back to plain text rather than point at a missing relationship.
fn link_id(url: &str, content: &Content) -> Option<String> {
    content
        .links
        .iter()
        .position(|link| link == url)
        .map(|index| link_rel_id(content.images.len(), index))
}

/// The run properties, in the order the schema requires.
fn run_properties(props: &RunProps) -> Element {
    let mut element = Element::new("w:rPr");
    if let Some(style) = &props.style {
        element = element.child(Element::new("w:rStyle").attr("w:val", style.clone()));
    }
    if props.bold {
        element = element.child(Element::new("w:b"));
    }
    if props.italic {
        element = element.child(Element::new("w:i"));
    }
    if props.strike {
        element = element.child(Element::new("w:strike"));
    }
    if props.underline {
        element = element.child(Element::new("w:u").attr("w:val", "single"));
    }
    element
}

/// Builds the drawing of an inline picture.
fn drawing_element(image: &super::model::Image) -> Element {
    let cx = image.width_emu.to_string();
    let cy = image.height_emu.to_string();
    let doc_id = image.index + 1;
    Element::new("w:r").child(
        Element::new("w:drawing").child(
            Element::new("wp:inline")
                .attr("distT", "0")
                .attr("distB", "0")
                .attr("distL", "0")
                .attr("distR", "0")
                .child(
                    Element::new("wp:extent")
                        .attr("cx", cx.clone())
                        .attr("cy", cy.clone()),
                )
                .child(
                    Element::new("wp:effectExtent")
                        .attr("l", "0")
                        .attr("t", "0")
                        .attr("r", "0")
                        .attr("b", "0"),
                )
                .child(
                    Element::new("wp:docPr")
                        .attr("id", doc_id.to_string())
                        .attr("name", image.name.clone())
                        .attr("descr", image.alt.clone()),
                )
                .child(
                    Element::new("wp:cNvGraphicFramePr").child(
                        Element::new("a:graphicFrameLocks").attr("noChangeAspect", "1"),
                    ),
                )
                .child(
                    Element::new("a:graphic").child(
                        Element::new("a:graphicData")
                            .attr(
                                "uri",
                                "http://schemas.openxmlformats.org/drawingml/2006/picture",
                            )
                            .child(
                                Element::new("pic:pic")
                                    .child(
                                        Element::new("pic:nvPicPr")
                                            .child(
                                                Element::new("pic:cNvPr")
                                                    .attr("id", doc_id.to_string())
                                                    .attr("name", image.name.clone())
                                                    .attr("descr", image.alt.clone()),
                                            )
                                            .child(Element::new("pic:cNvPicPr")),
                                    )
                                    .child(
                                        Element::new("pic:blipFill")
                                            .child(
                                                Element::new("a:blip")
                                                    .attr("r:embed", image_rel_id(image.index)),
                                            )
                                            .child(
                                                Element::new("a:stretch")
                                                    .child(Element::new("a:fillRect")),
                                            ),
                                    )
                                    .child(
                                        Element::new("pic:spPr")
                                            .child(
                                                Element::new("a:xfrm")
                                                    .child(
                                                        Element::new("a:off")
                                                            .attr("x", "0")
                                                            .attr("y", "0"),
                                                    )
                                                    .child(
                                                        Element::new("a:ext")
                                                            .attr("cx", cx)
                                                            .attr("cy", cy),
                                                    ),
                                            )
                                            .child(
                                                Element::new("a:prstGeom")
                                                    .attr("prst", "rect")
                                                    .child(Element::new("a:avLst")),
                                            ),
                                    ),
                            ),
                    ),
                ),
        ),
    )
}

/// Builds a table element.
fn table_element(table: &Table, content: &Content) -> Element {
    let total: u32 = table.widths.iter().sum();
    let mut element = Element::new("w:tbl");
    element = element.child(
        Element::new("w:tblPr")
            .child(Element::new("w:tblStyle").attr("w:val", "TableGrid"))
            .child(
                Element::new("w:tblW")
                    .attr("w:w", total.to_string())
                    .attr("w:type", "dxa"),
            )
            .child(Element::new("w:tblLayout").attr("w:type", "fixed"))
            .child(
                Element::new("w:tblLook")
                    .attr("w:val", "04A0")
                    .attr("w:firstRow", "1")
                    .attr("w:lastRow", "0")
                    .attr("w:firstColumn", "1")
                    .attr("w:lastColumn", "0")
                    .attr("w:noHBand", "0")
                    .attr("w:noVBand", "1"),
            ),
    );
    element = element.child(
        Element::new("w:tblGrid").children(
            table
                .widths
                .iter()
                .map(|w| Element::new("w:gridCol").attr("w:w", w.to_string())),
        ),
    );
    for row in &table.rows {
        let mut row_element = Element::new("w:tr");
        if row.header {
            row_element = row_element
                .child(Element::new("w:trPr").child(Element::new("w:tblHeader")));
        }
        for (index, cell) in row.cells.iter().enumerate() {
            let width = table.widths.get(index).copied().unwrap_or(0);
            let mut cell_element = Element::new("w:tc").child(
                Element::new("w:tcPr")
                    .child(
                        Element::new("w:tcW")
                            .attr("w:w", width.to_string())
                            .attr("w:type", "dxa"),
                    ),
            );
            for paragraph in &cell.paragraphs {
                cell_element = cell_element.child(cell_paragraph(paragraph, cell.align, content));
            }
            row_element = row_element.child(cell_element);
        }
        element = element.child(row_element);
    }
    element
}

/// Builds a paragraph inside a table cell.
fn cell_paragraph(paragraph: &Paragraph, align: Align, content: &Content) -> Element {
    let mut element = Element::new("w:p");
    let mut props: Vec<Element> = Vec::new();
    if let Some(style) = &paragraph.style {
        props.push(Element::new("w:pStyle").attr("w:val", style.clone()));
    }
    if align != Align::None {
        let value = match align {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::None => "left",
        };
        props.push(Element::new("w:jc").attr("w:val", value));
    }
    if !props.is_empty() {
        element = element.child(Element::new("w:pPr").children(props));
    }
    for run in &paragraph.runs {
        element = element.child(run_element(run, content));
    }
    element
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::content::{Content, ImagePart, NumberingPlan, OrderedList};

    /// A content with no images and no links.
    fn plain() -> Content {
        Content {
            document: Default::default(),
            images: Vec::new(),
            links: Vec::new(),
            numbering: NumberingPlan {
                bullet_num_id: 1,
                ordered: Vec::new(),
            },
            toc: false,
            toc_title: "Contents".into(),
        }
    }

    #[test]
    fn styles_define_every_style_the_document_uses() {
        let styles = styles();
        for id in [
            "Normal",
            "Heading1",
            "Heading6",
            "Quote",
            "ListParagraph",
            "HTMLPreformatted",
            "HTMLCode",
            "Hyperlink",
            "TableGrid",
            "TOCHeading",
            "TOC1",
            "TOC3",
        ] {
            assert!(
                styles.contains(&format!(r#"w:styleId="{id}""#)),
                "missing {id}"
            );
        }
    }

    #[test]
    fn headings_carry_an_outline_level() {
        let styles = styles();
        for level in 0..6 {
            assert!(
                styles.contains(&format!(r#"<w:outlineLvl w:val="{level}"/>"#)),
                "missing outline level {level}"
            );
        }
    }

    #[test]
    fn every_part_declares_its_namespace() {
        assert!(styles().contains(NS_W));
        assert!(numbering(&plain().numbering).contains(NS_W));
        assert!(settings(false).contains(NS_W));
        assert!(content_types(0).contains(NS_CT));
        assert!(package_rels().contains(NS_PKG_REL));
        assert!(document_rels(&[], &[]).contains(NS_PKG_REL));
    }

    #[test]
    fn numbering_defines_both_abstract_lists() {
        let plan = NumberingPlan {
            bullet_num_id: 1,
            ordered: vec![OrderedList {
                num_id: 2,
                start: 3,
            }],
        };
        let xml = numbering(&plan);
        assert!(xml.contains(r#"w:abstractNumId="0""#));
        assert!(xml.contains(r#"w:abstractNumId="1""#));
        assert!(xml.contains(r#"<w:startOverride w:val="3"/>"#));
        assert!(xml.contains(r#"<w:num w:numId="2">"#));
    }

    #[test]
    fn every_ordered_list_gets_its_own_instance() {
        let plan = NumberingPlan {
            bullet_num_id: 1,
            ordered: vec![
                OrderedList {
                    num_id: 2,
                    start: 1,
                },
                OrderedList {
                    num_id: 3,
                    start: 5,
                },
            ],
        };
        let xml = numbering(&plan);
        assert!(xml.contains(r#"<w:num w:numId="2">"#));
        assert!(xml.contains(r#"<w:num w:numId="3">"#));
        assert!(xml.contains(r#"<w:startOverride w:val="5"/>"#));
    }

    #[test]
    fn settings_only_update_fields_when_a_toc_is_present() {
        assert!(!settings(false).contains("updateFields"));
        assert!(settings(true).contains(r#"<w:updateFields w:val="true"/>"#));
    }

    #[test]
    fn content_types_cover_every_part() {
        let xml = content_types(0);
        assert!(xml.contains(r#"PartName="/word/document.xml""#));
        assert!(xml.contains(r#"PartName="/word/styles.xml""#));
        assert!(xml.contains(r#"PartName="/word/numbering.xml""#));
        assert!(xml.contains(r#"PartName="/word/settings.xml""#));
        assert!(!xml.contains(r#"Extension="png""#));
        assert!(content_types(1).contains(r#"Extension="png""#));
    }

    #[test]
    fn package_relationships_point_at_the_document() {
        assert!(package_rels().contains(r#"Target="word/document.xml""#));
    }

    #[test]
    fn document_relationships_number_the_parts_and_the_targets() {
        let images = vec![ImagePart {
            name: "image1.png".into(),
            bytes: Vec::new(),
        }];
        let links = vec!["https://example.com".to_string()];
        let xml = document_rels(&images, &links);
        assert!(xml.contains(r#"Id="rId1" Type"#));
        assert!(xml.contains(r#"Target="styles.xml""#));
        assert!(xml.contains(r#"Target="numbering.xml""#));
        assert!(xml.contains(r#"Target="settings.xml""#));
        assert!(xml.contains(r#"Id="rId4" Type"#));
        assert!(xml.contains(r#"Target="media/image1.png""#));
        assert!(xml.contains(r#"Target="https://example.com" TargetMode="External""#));
        assert!(xml.contains(r#"Id="rId5" Type"#));
    }

    #[test]
    fn relationship_ids_do_not_collide() {
        assert_eq!(image_rel_id(0), "rId4");
        assert_eq!(link_rel_id(1, 0), "rId5");
        assert_eq!(link_rel_id(2, 3), "rId9");
    }

    #[test]
    fn the_document_declares_the_namespaces_the_drawings_need() {
        let xml = document(&plain());
        assert!(xml.contains(NS_W));
        assert!(xml.contains(NS_R));
        assert!(xml.contains(NS_WP));
        assert!(xml.contains(NS_A));
        assert!(xml.contains(NS_PIC));
    }

    #[test]
    fn the_document_ends_with_the_page_setup() {
        let xml = document(&plain());
        assert!(xml.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#));
        assert!(xml.contains(r#"w:left="1440""#));
        assert!(xml.trim_end().ends_with("</w:document>"));
    }

    #[test]
    fn a_toc_adds_a_field() {
        let mut content = plain();
        content.toc = true;
        let xml = document(&content);
        assert!(xml.contains(r#"TOC \o "1-3""#));
        assert!(xml.contains(r#"w:fldCharType="begin""#));
        assert!(xml.contains(r#"w:fldCharType="end""#));
        assert!(xml.contains("Contents"));
    }
}
