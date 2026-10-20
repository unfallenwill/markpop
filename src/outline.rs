//! Table of contents: collect headings and build a nested outline.
//!
//! The TOC is an ordinary AST fragment (a heading plus nested lists), so it
//! lays out and paginates exactly like hand-written Markdown. That keeps
//! this module free of layout knowledge and easy to test.

use crate::ast::{Block, Document, Inline, ListData, ListItem, plain_text_of};

/// One heading in the outline.
#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    /// Heading level, `1..=6`.
    pub level: u8,
    /// Plain text of the heading (styling is flattened away).
    pub text: String,
}

/// Collects the document's top-level headings up to `max_level`.
///
/// Headings inside blockquotes or list items are not part of the outline:
/// they are not sections of the document.
pub fn headings(doc: &Document, max_level: u8) -> Vec<Heading> {
    doc.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Heading { level, inlines } if *level <= max_level => Some(Heading {
                level: *level,
                text: plain_text_of(inlines),
            }),
            _ => None,
        })
        .collect()
}

/// Returns a copy of `doc` with a table of contents inserted at the top.
///
/// If the document has no headings (within `max_level`) it is returned
/// unchanged. An empty `title` skips the "Contents" heading and emits only
/// the outline list.
pub fn with_toc(doc: &Document, max_level: u8, title: &str) -> Document {
    let entries = headings(doc, max_level);
    let Some(list) = toc_list(&entries) else {
        return doc.clone();
    };

    let mut blocks = Vec::with_capacity(doc.blocks.len() + 2);
    if !title.is_empty() {
        blocks.push(Block::Heading {
            level: 2,
            inlines: vec![Inline::Text(title.to_string())],
        });
    }
    blocks.push(list);
    blocks.extend(doc.blocks.iter().cloned());
    Document { blocks }
}

/// Builds nested lists from a flat heading sequence.
///
/// The first entry establishes the base level; deeper headings become
/// children of the previous item. Level jumps (h1 → h3) are allowed.
fn toc_list(entries: &[Heading]) -> Option<Block> {
    if entries.is_empty() {
        return None;
    }
    let mut items = Vec::new();
    let mut i = 0usize;
    while i < entries.len() {
        let level = entries[i].level;
        let mut blocks = vec![Block::Paragraph(vec![Inline::Text(
            entries[i].text.clone(),
        )])];
        i += 1;

        let start = i;
        while i < entries.len() && entries[i].level > level {
            i += 1;
        }
        if i > start
            && let Some(child) = toc_list(&entries[start..i])
        {
            blocks.push(child);
        }
        items.push(ListItem { blocks });
    }
    debug_assert!(!items.is_empty());
    Some(Block::List(ListData {
        ordered: false,
        start: 1,
        tight: true,
        items,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn doc(src: &str) -> Document {
        parse(src)
    }

    #[test]
    fn collects_top_level_headings() {
        let d = doc("# One\n\ntext\n\n## Two\n\n### Three\n\n#### Four");
        let hs = headings(&d, 3);
        assert_eq!(hs.len(), 3);
        assert_eq!(
            hs[0],
            Heading {
                level: 1,
                text: "One".into()
            }
        );
        assert_eq!(
            hs[2],
            Heading {
                level: 3,
                text: "Three".into()
            }
        );
    }

    #[test]
    fn heading_text_is_flattened() {
        let d = doc("# **Bold** and `code`");
        assert_eq!(headings(&d, 6)[0].text, "Bold and code");
    }

    #[test]
    fn headings_inside_containers_are_ignored() {
        let d = doc("> # Quoted\n\n- # Listed");
        assert!(headings(&d, 6).is_empty());
    }

    #[test]
    fn empty_document_gets_no_toc() {
        let d = doc("just text");
        assert_eq!(with_toc(&d, 3, "Contents"), d);
    }

    #[test]
    fn toc_is_prepended_with_a_title() {
        let d = doc("# A\n\n## B");
        let out = with_toc(&d, 3, "Contents");
        assert_eq!(
            out.blocks[0],
            Block::Heading {
                level: 2,
                inlines: vec![Inline::Text("Contents".into())]
            }
        );
        assert!(matches!(out.blocks[1], Block::List(_)));
        assert_eq!(out.blocks.len(), d.blocks.len() + 2);
    }

    #[test]
    fn empty_title_skips_the_heading() {
        let d = doc("# A");
        let out = with_toc(&d, 3, "");
        assert!(matches!(out.blocks[0], Block::List(_)));
    }

    #[test]
    fn toc_nests_by_heading_level() {
        let d = doc("# A\n\n## B\n\n### C\n\n## D\n\n# E");
        let out = with_toc(&d, 6, "");
        let Block::List(list) = &out.blocks[0] else {
            panic!("expected a list");
        };
        assert_eq!(list.items.len(), 2, "two top-level sections");
        let first = &list.items[0];
        assert_eq!(first.blocks.len(), 2, "item A has a nested list");
        let Block::List(children) = &first.blocks[1] else {
            panic!("expected nested list");
        };
        assert_eq!(children.items.len(), 2, "B and D");
        let b = &children.items[0];
        assert_eq!(b.blocks.len(), 2, "B has C nested under it");
        assert!(matches!(&b.blocks[1], Block::List(grand) if grand.items.len() == 1));
    }

    #[test]
    fn max_level_filters_deep_headings() {
        let d = doc("# A\n\n### C");
        let hs = headings(&d, 2);
        assert_eq!(hs.len(), 1);
        assert_eq!(hs[0].text, "A");
    }

    #[test]
    fn toc_survives_level_jumps() {
        let d = doc("# A\n\n#### Deep");
        let out = with_toc(&d, 6, "");
        let Block::List(list) = &out.blocks[0] else {
            panic!("expected a list");
        };
        assert_eq!(list.items.len(), 1);
        assert!(matches!(&list.items[0].blocks[1], Block::List(_)));
    }
}
