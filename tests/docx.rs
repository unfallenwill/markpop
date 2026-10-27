//! Integrity tests for the Word output.
//!
//! A `.docx` is only useful if Word can open it, and Word refuses files
//! whose parts do not agree with each other. These tests assert that
//! agreement on the package itself: every part is typed, every
//! relationship resolves, every style and every numbering instance the
//! document names is actually defined.

use markpop::docx::zip::read_archive;
use markpop::{Options, render_docx};

/// A document exercising every construct the writer supports.
const KITCHEN_SINK: &str = r#"
# Heading one

A paragraph with **bold**, *italic*, ~~struck~~ and `code` text, plus a
[link](https://example.com/page?a=1&b=2).

## Heading two

> A quotation.

- bullet one
- bullet two
  - nested bullet

1. ordered one
2. ordered two

- [x] done
- [ ] todo

```rust
let x = 1;
```

| Name | Size |
|:-----|-----:|
| alpha | 10 |

---

A line with <u>underline</u> and <b>bold</b>.
"#;

/// The parts of a generated document.
fn parts(markdown: &str) -> Vec<(String, String)> {
    parts_with(markdown, &Options::default())
}

/// The parts of a generated document, with explicit options.
fn parts_with(markdown: &str, options: &Options) -> Vec<(String, String)> {
    raw_parts(markdown, options)
        .into_iter()
        .map(|(name, bytes)| (name, String::from_utf8_lossy(&bytes).into_owned()))
        .collect()
}

/// The raw bytes of the parts of a generated document.
fn raw_parts(markdown: &str, options: &Options) -> Vec<(String, Vec<u8>)> {
    let bytes = render_docx(markdown, options).expect("a rendered document");
    read_archive(&bytes).expect("a readable archive")
}

/// One part of a generated document.
fn part(markdown: &str, name: &str) -> String {
    parts(markdown)
        .into_iter()
        .find(|(part, _)| part == name)
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("missing part {name}"))
}

/// The text of a document, as the runs spell it.
fn document_text(xml: &str) -> String {
    let mut text = String::new();
    let mut rest = xml;
    while let Some(pos) = rest.find("<w:t") {
        rest = &rest[pos..];
        let Some(start) = rest.find('>') else { break };
        rest = &rest[start + 1..];
        let Some(end) = rest.find("</w:t>") else { break };
        text.push_str(&rest[..end]);
        rest = &rest[end..];
    }
    text
}

/// Every value of an attribute in a document.
fn attribute_values(xml: &str, attribute: &str) -> Vec<String> {
    let needle = format!("{attribute}=\"");
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(pos) = rest.find(&needle) {
        rest = &rest[pos + needle.len()..];
        let Some(end) = rest.find('"') else { break };
        out.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}

#[test]
fn the_package_has_every_required_part() {
    let names: Vec<String> = parts(KITCHEN_SINK).into_iter().map(|(n, _)| n).collect();
    for expected in [
        "[Content_Types].xml",
        "_rels/.rels",
        "word/document.xml",
        "word/_rels/document.xml.rels",
        "word/styles.xml",
        "word/numbering.xml",
        "word/settings.xml",
    ] {
        assert!(names.contains(&expected.to_string()), "missing {expected}");
    }
}

#[test]
fn every_part_is_declared_in_the_content_types() {
    let all = parts(KITCHEN_SINK);
    let types = all
        .iter()
        .find(|(name, _)| name == "[Content_Types].xml")
        .map(|(_, body)| body.clone())
        .expect("the content types");
    for (name, _) in &all {
        if name == "[Content_Types].xml" {
            continue;
        }
        let extension = name.rsplit('.').next().expect("an extension");
        let declared = types.contains(&format!(r#"Extension="{extension}""#))
            || types.contains(&format!(r#"PartName="/{name}""#));
        assert!(declared, "{name} is not declared in [Content_Types].xml");
    }
}

#[test]
fn every_relationship_the_document_uses_resolves() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    let rels = part(KITCHEN_SINK, "word/_rels/document.xml.rels");
    let mut ids = attribute_values(&document, "r:id");
    ids.extend(attribute_values(&document, "r:embed"));
    assert!(!ids.is_empty(), "the document should reference relationships");
    for id in ids {
        assert!(
            rels.contains(&format!(r#"Id="{id}""#)),
            "unresolved relationship {id}"
        );
    }
}

#[test]
fn every_style_the_document_uses_is_defined() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    let styles = part(KITCHEN_SINK, "word/styles.xml");
    let mut used = attribute_values(&document, "w:val");
    used.retain(|value| {
        document.contains(&format!(r#"<w:pStyle w:val="{value}"/>"#))
            || document.contains(&format!(r#"<w:rStyle w:val="{value}"/>"#))
    });
    assert!(!used.is_empty(), "the document should use styles");
    for style in used {
        assert!(
            styles.contains(&format!(r#"w:styleId="{style}""#)),
            "undefined style {style}"
        );
    }
}

#[test]
fn every_numbering_id_the_document_uses_is_defined() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    let numbering = part(KITCHEN_SINK, "word/numbering.xml");
    for id in attribute_values(&document, "w:numId") {
        assert!(
            numbering.contains(&format!(r#"<w:num w:numId="{id}">"#)),
            "undefined numbering {id}"
        );
    }
}

#[test]
fn the_text_of_the_document_matches_the_source() {
    let text = document_text(&part(KITCHEN_SINK, "word/document.xml"));
    for expected in [
        "Heading one",
        "A paragraph with ",
        "bold",
        "italic",
        "struck",
        "code",
        "link",
        "Heading two",
        "A quotation.",
        "bullet one",
        "nested bullet",
        "ordered one",
        "done",
        "todo",
        "let x = 1;",
        "alpha",
        "underline",
    ] {
        assert!(text.contains(expected), "missing text {expected:?} in {text:?}");
    }
}

#[test]
fn the_document_uses_the_native_word_styles() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    for style in [
        "Heading1",
        "Heading2",
        "Quote",
        "ListParagraph",
        "HTMLPreformatted",
        "TableGrid",
    ] {
        assert!(
            document.contains(&format!(r#"w:val="{style}""#)),
            "missing style {style}"
        );
    }
}

#[test]
fn a_table_is_a_real_table() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    assert!(document.contains("<w:tbl>"));
    assert!(document.contains("<w:gridCol"));
    assert!(document.contains("<w:tc>"));
    assert!(document.contains("<w:tblHeader/>"));
}

#[test]
fn a_list_is_a_real_list() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    assert!(document.contains("<w:numPr>"));
    assert!(document.contains("<w:ilvl w:val=\"0\"/>"));
    assert!(document.contains("<w:ilvl w:val=\"1\"/>"));
}

#[test]
fn a_link_is_an_external_relationship() {
    let rels = part(KITCHEN_SINK, "word/_rels/document.xml.rels");
    assert!(rels.contains(r#"Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink""#));
    assert!(rels.contains(r#"TargetMode="External""#));
    assert!(rels.contains("https://example.com/page?a=1&amp;b=2"));
}

#[test]
fn a_thematic_break_is_a_bordered_paragraph() {
    let document = part(KITCHEN_SINK, "word/document.xml");
    assert!(document.contains("<w:pBdr>"));
    assert!(document.contains(r#"<w:bottom w:val="single""#));
}

#[test]
fn the_toc_is_a_field_word_can_build() {
    let options = Options::default().with_toc(true);
    let document = part_with(&options, "word/document.xml");
    assert!(document.contains(r#"TOC \o "1-3""#));
    assert!(document.contains(r#"w:fldCharType="begin""#));
    assert!(document.contains(r#"w:fldCharType="end""#));
    // Word is asked to refresh the fields when the document opens.
    let settings = part_with(&options, "word/settings.xml");
    assert!(settings.contains(r#"<w:updateFields w:val="true"/>"#));
}

/// One part of a document rendered with explicit options.
fn part_with(options: &Options, name: &str) -> String {
    parts_with(KITCHEN_SINK, options)
        .into_iter()
        .find(|(part, _)| part == name)
        .map(|(_, body)| body)
        .unwrap_or_else(|| panic!("missing part {name}"))
}

#[test]
fn without_a_toc_no_field_is_asked_for() {
    let settings = part(KITCHEN_SINK, "word/settings.xml");
    assert!(!settings.contains("updateFields"));
}

#[test]
fn the_same_document_always_produces_the_same_bytes() {
    let a = render_docx(KITCHEN_SINK, &Options::default()).unwrap();
    let b = render_docx(KITCHEN_SINK, &Options::default()).unwrap();
    assert_eq!(a, b);
}

#[test]
fn an_embedded_image_round_trips_to_the_same_pixels() {
    // Write a small PNG the crate can encode, then embed it.
    let mut dir = std::env::temp_dir();
    dir.push(format!("markpop-docx-image-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let path = dir.join("image.png");
    let pixels: Vec<u8> = (0..4 * 4 * 4).map(|i| (i * 7 % 251) as u8).collect();
    let source = markpop::encode::png::encode(4, 4, &pixels);
    std::fs::write(&path, &source).expect("write");

    let markdown = "![a picture](image.png)\n";
    let options = Options::default().with_base_dir(dir.to_string_lossy().to_string());
    let all = raw_parts(markdown, &options);
    let image = all
        .iter()
        .find(|(name, _)| name.starts_with("word/media/"))
        .map(|(_, body)| body.clone())
        .expect("the image part");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(image.starts_with(&[0x89, b'P', b'N', b'G']));
    // The part is the same PNG, byte for byte: the pixels survived.
    assert_eq!(image, source);
}

#[test]
fn an_unreadable_image_degrades_to_its_alt_text() {
    let document = part("![the alt](no-such-image.png)\n", "word/document.xml");
    assert!(document_text(&document).contains("the alt"));
    assert!(!document.contains("<w:drawing>"));
}

#[test]
fn a_document_without_images_has_no_media_parts() {
    let names: Vec<String> = parts("# Title\n").into_iter().map(|(n, _)| n).collect();
    assert!(!names.iter().any(|n| n.starts_with("word/media/")));
}

#[test]
fn the_page_setup_is_a4_with_one_inch_margins() {
    let document = part("# Title\n", "word/document.xml");
    assert!(document.contains(r#"<w:pgSz w:w="11906" w:h="16838"/>"#));
    assert!(document.contains(r#"w:top="1440""#));
}

#[test]
fn a_table_is_followed_by_a_paragraph() {
    // Word merges two tables that are not separated by a paragraph.
    let document = part("| a |\n|---|\n| 1 |\n", "word/document.xml");
    let table_end = document.find("</w:tbl>").expect("a table");
    let rest = &document[table_end..];
    assert!(
        rest.contains("<w:p/>") || rest.contains("<w:p>"),
        "a paragraph must follow the table"
    );
}

#[test]
fn the_document_has_no_leading_whitespace_inside_a_paragraph() {
    // A space between two runs is real text in Word.
    let document = part("text **bold** text\n", "word/document.xml");
    let start = document.find("<w:p>").expect("a paragraph");
    let end = document.find("</w:p>").expect("the end of the paragraph");
    let paragraph = &document[start..end];
    assert!(!paragraph.contains("> <"), "unexpected whitespace: {paragraph}");
}
