//! Turns raw HTML blocks into Markdown blocks for rendering.
//!
//! The Markdown specification passes recognized HTML through verbatim
//! (CommonMark 0.31.2 §4.6). Rendering it means interpreting that markup,
//! and the lowest-risk interpretation is one that reuses everything this
//! crate already draws: the HTML tree is converted *into* [`Block`] and
//! [`Inline`] nodes, and the ordinary block/inlay layout paths take over.
//!
//! The boundary stays the spec's: styling tags map onto existing text
//! styles, structural tags map onto existing blocks, unknown tags are
//! transparent containers, non-visual constructs (comments, `<script>`,
//! `<head>`, …) are dropped. There is no CSS, no scripting, and nothing
//! is fetched.

use crate::ast::{
    Align, Block, CodeBlock, ImageData, Inline, ListData, ListItem, TableData,
};
use crate::html::{HtmlElement, HtmlNode, parse_html};

/// Elements whose entire subtree is invisible in rendered output.
const INVISIBLE_TAGS: [&str; 30] = [
    "applet", "area", "audio", "base", "basefont", "canvas", "col", "colgroup", "datalist",
    "embed", "frame", "frameset", "head", "iframe", "input", "link", "map", "meta", "noframes",
    "noscript", "object", "param", "script", "select", "source", "style", "svg", "template",
    "textarea", "title",
];

/// Structural tags that lay out as blocks (mapped or transparent).
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "body"
            | "center"
            | "dd"
            | "details"
            | "dir"
            | "div"
            | "dl"
            | "dt"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hr"
            | "html"
            | "li"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "tbody"
            | "tfoot"
            | "thead"
            | "tr"
            | "ul"
    )
}

/// Converts a raw HTML block (as captured by the Markdown block scanner)
/// into a sequence of Markdown blocks ready for the normal layout paths.
pub(super) fn html_blocks(raw: &str) -> Vec<Block> {
    let mut out = Vec::new();
    push_nodes(&parse_html(raw), &mut out);
    out
}

/// Walks a node sequence, grouping consecutive inline content into
/// paragraphs and recursing into block-level elements.
fn push_nodes(nodes: &[HtmlNode], out: &mut Vec<Block>) {
    let mut para: Vec<Inline> = Vec::new();
    for node in nodes {
        match node {
            HtmlNode::Text(text) => push_text(text, &mut para),
            HtmlNode::Element(el) if invisible(&el.tag) => {}
            HtmlNode::Element(el) if is_block_tag(&el.tag) => {
                flush_paragraph(&mut para, out);
                push_block(el, out);
            }
            HtmlNode::Element(el) => para.extend(html_inline(el)),
        }
    }
    flush_paragraph(&mut para, out);
}

fn flush_paragraph(para: &mut Vec<Inline>, out: &mut Vec<Block>) {
    if !para.is_empty() {
        let mut owned = std::mem::take(para);
        trim_edges(&mut owned);
        if !owned.is_empty() {
            out.push(Block::Paragraph(owned));
        }
    }
}

/// Appends collapsed text to a paragraph, merging with a preceding run.
/// Edge spaces are kept here; [`trim_edges`] drops them at flush time.
fn push_text(text: &str, para: &mut Vec<Inline>) {
    let text = collapse_keep_edges(text);
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text(prev)) = para.last_mut() {
        prev.push_str(&text);
        return;
    }
    para.push(Inline::Text(text));
}

/// Lays out one block-level element into `out`.
fn push_block(el: &HtmlElement, out: &mut Vec<Block>) {
    match el.tag.as_str() {
        "p" => out.push(Block::Paragraph(inlines_of(el))),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = el.tag.as_bytes()[1] - b'0';
            out.push(Block::Heading {
                level,
                inlines: inlines_of(el),
            });
        }
        "blockquote" => {
            let mut inner = Vec::new();
            push_nodes(&el.children, &mut inner);
            if inner.is_empty() {
                inner.push(Block::Paragraph(Vec::new()));
            }
            out.push(Block::Quote(inner));
        }
        "pre" => out.push(Block::Code(pre_code(el))),
        "hr" => out.push(Block::ThematicBreak),
        "ul" | "ol" => out.push(Block::List(list_data(el, el.tag == "ol"))),
        "table" => {
            if let Some(table) = table_data(el) {
                out.push(Block::Table(table));
            }
        }
        // Transparent containers (div, section, unknown tags, …) and
        // stray table internals without a table ancestor: lay out their
        // content.
        _ => push_nodes(&el.children, out),
    }
}

/// The inline content of a block element's children.
fn inlines_of(el: &HtmlElement) -> Vec<Inline> {
    let mut out = Vec::new();
    push_inlines(&el.children, &mut out);
    trim_edges(&mut out);
    out
}

/// Converts child nodes into inline content. A block element met in
/// inline context (e.g. `<td><p>x</p></td>`) contributes its plain text.
fn push_inlines(nodes: &[HtmlNode], out: &mut Vec<Inline>) {
    for node in nodes {
        match node {
            HtmlNode::Text(text) => {
                let text = collapse_keep_edges(text);
                if !text.is_empty() {
                    out.push(Inline::Text(text));
                }
            }
            HtmlNode::Element(el) if invisible(&el.tag) => {}
            HtmlNode::Element(el) if is_block_tag(&el.tag) => {
                let text = plain_text_of(&el.children);
                if !text.is_empty() {
                    if let Some(Inline::Text(prev)) = out.last_mut() {
                        prev.push(' ');
                    }
                    out.push(Inline::Text(text));
                }
            }
            HtmlNode::Element(el) => out.extend(html_inline(el)),
        }
    }
}

/// Converts one inline element into Markdown inlines.
fn html_inline(el: &HtmlElement) -> Vec<Inline> {
    let children = inlines_of(el);
    match el.tag.as_str() {
        "b" | "strong" => vec![Inline::Strong(children)],
        "i" | "em" | "cite" | "var" | "dfn" => vec![Inline::Emph(children)],
        "del" | "s" | "strike" => vec![Inline::Strike(children)],
        "u" | "ins" => vec![Inline::Underline(children)],
        "a" => vec![Inline::Link {
            text: children,
            url: el.attr("href").unwrap_or_default().to_string(),
            title: el.attr("title").map(str::to_string),
        }],
        "img" => vec![Inline::Image(ImageData {
            alt: el.attr("alt").unwrap_or_default().to_string(),
            url: el.attr("src").unwrap_or_default().to_string(),
            title: None,
        })],
        "br" => vec![Inline::Break],
        "code" | "kbd" | "samp" | "tt" => {
            vec![Inline::Code(plain_text_of(&el.children))]
        }
        // Transparent: span, font, mark, small, sub, sup, unknown/custom
        // tags — content renders with no extra styling.
        _ => children,
    }
}

/// Plain text of a node sequence (entities already decoded), with HTML
/// whitespace collapsed and blocks joined by spaces.
pub(super) fn plain_text_of(nodes: &[HtmlNode]) -> String {
    let mut words: Vec<String> = Vec::new();
    fn collect(nodes: &[HtmlNode], words: &mut Vec<String>) {
        for node in nodes {
            match node {
                HtmlNode::Text(text) => {
                    for word in text.split_whitespace() {
                        words.push(word.to_string());
                    }
                }
                HtmlNode::Element(el) if invisible(&el.tag) => {}
                HtmlNode::Element(el) => collect(&el.children, words),
            }
        }
    }
    collect(nodes, &mut words);
    words.join(" ")
}

/// The code block for a `<pre>` element: raw text (whitespace preserved),
/// with the HTML-conventional single leading newline dropped and a
/// `language-*` class on an inner `<code>` promoted to the info string.
fn pre_code(el: &HtmlElement) -> CodeBlock {
    let mut info = String::new();
    let mut text = plain_text_raw(&el.children);
    // A single newline right after `<pre>` is ignored, per HTML.
    if let Some(stripped) = text.strip_prefix('\n') {
        text = stripped.to_string();
    }
    let code = el.children.iter().find_map(|node| match node {
        HtmlNode::Element(e) if e.tag == "code" => Some(e),
        _ => None,
    });
    if let Some(class) = code.and_then(|code| code.attr("class")) {
        for class in class.split_whitespace() {
            if let Some(lang) = class
                .strip_prefix("language-")
                .or_else(|| class.strip_prefix("lang-"))
            {
                info = lang.to_string();
            }
        }
    }
    CodeBlock {
        info,
        lines: text.lines().map(str::to_string).collect(),
    }
}

/// Raw text of a node sequence without whitespace collapsing (for `<pre>`).
fn plain_text_raw(nodes: &[HtmlNode]) -> String {
    let mut out = String::new();
    fn collect(nodes: &[HtmlNode], out: &mut String) {
        for node in nodes {
            match node {
                HtmlNode::Text(text) => out.push_str(text),
                HtmlNode::Element(el) if invisible(&el.tag) => {}
                HtmlNode::Element(el) => collect(&el.children, out),
            }
        }
    }
    collect(nodes, &mut out);
    out
}

/// Builds list data from `<li>` children.
fn list_data(el: &HtmlElement, ordered: bool) -> ListData {
    let start = el
        .attr("start")
        .and_then(|value| value.trim().parse::<u32>().ok())
        .unwrap_or(1);
    let mut items = Vec::new();
    for node in &el.children {
        let HtmlNode::Element(el) = node else {
            continue;
        };
        if el.tag != "li" {
            continue;
        }
        let mut blocks = Vec::new();
        push_nodes(&el.children, &mut blocks);
        if blocks.is_empty() {
            blocks.push(Block::Paragraph(Vec::new()));
        }
        items.push(ListItem {
            blocks,
            task: None,
        });
    }
    ListData {
        ordered,
        start,
        tight: true,
        items,
    }
}

/// Builds a table from `<tr>`/`<td>`/`<th>` markup, looking through
/// `thead`/`tbody`/`tfoot`. `None` when the table has no rows at all.
fn table_data(el: &HtmlElement) -> Option<TableData> {
    // Rows may sit directly under <table> or inside thead/tbody/tfoot.
    let mut trs: Vec<&HtmlElement> = Vec::new();
    let mut has_head_section = false;
    for node in &el.children {
        let HtmlNode::Element(el) = node else {
            continue;
        };
        match el.tag.as_str() {
            "tr" => trs.push(el),
            "thead" => {
                has_head_section = true;
                trs.extend(tr_children(el));
            }
            "tbody" | "tfoot" => trs.extend(tr_children(el)),
            _ => {}
        }
    }
    if trs.is_empty() {
        return None;
    }

    // A thead row wins; otherwise a row made entirely of `th` cells does;
    // otherwise the first row acts as the header, matching the crate's
    // table renderer, which always draws one.
    let head_index = if has_head_section {
        0
    } else {
        trs.iter()
            .position(|tr| {
                let cells = cells_of(tr);
                !cells.is_empty()
                    && tr.children.iter().filter_map(cell_element).all(|c| c.tag == "th")
            })
            .unwrap_or(0)
    };

    let mut aligns = Vec::new();
    let mut head = Vec::new();
    let mut rows = Vec::new();
    for (index, tr) in trs.iter().enumerate() {
        let cells = cells_of(tr);
        if cells.is_empty() {
            continue;
        }
        if index == head_index {
            head = cells;
            for cell in tr.children.iter().filter_map(cell_element) {
                aligns.push(cell_align(cell));
            }
        } else {
            rows.push(cells);
        }
    }
    while aligns.len() < head.len() {
        aligns.push(Align::None);
    }
    aligns.truncate(head.len());
    Some(TableData { aligns, head, rows })
}

fn tr_children(el: &HtmlElement) -> Vec<&HtmlElement> {
    el.children
        .iter()
        .filter_map(|node| match node {
            HtmlNode::Element(tr) if tr.tag == "tr" => Some(tr),
            _ => None,
        })
        .collect()
}

fn cell_element(node: &HtmlNode) -> Option<&HtmlElement> {
    match node {
        HtmlNode::Element(cell) if cell.tag == "td" || cell.tag == "th" => Some(cell),
        _ => None,
    }
}

fn cells_of(tr: &HtmlElement) -> Vec<Vec<Inline>> {
    tr.children
        .iter()
        .filter_map(cell_element)
        .map(inlines_of)
        .collect()
}

/// Cell alignment from an `align` attribute (`left`/`center`/`right`).
fn cell_align(cell: &HtmlElement) -> Align {
    match cell.attr("align").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "left" => Align::Left,
        "center" => Align::Center,
        "right" => Align::Right,
        _ => Align::None,
    }
}

fn invisible(tag: &str) -> bool {
    INVISIBLE_TAGS.contains(&tag)
}

/// Collapses whitespace runs to single spaces, keeping at most one space
/// at each end. Interior edge spaces matter: HTML renders the space in
/// `含 <strong>strong</strong>` like any other space.
fn collapse_keep_edges(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            space = true;
        } else {
            if space {
                out.push(' ');
                space = false;
            }
            out.push(ch);
        }
    }
    if space {
        out.push(' ');
    }
    out
}

/// Trims the paragraph-leading space of the first inline and the
/// paragraph-trailing space of the last one, descending through styling
/// containers (`<b> x </b>` at a paragraph edge loses its padding).
fn trim_edges(inlines: &mut Vec<Inline>) {
    fn trim_start(v: &mut Vec<Inline>) {
        let Some(first) = v.first_mut() else {
            return;
        };
        match first {
            Inline::Text(t) => {
                let trimmed = t.trim_start().to_string();
                if trimmed.is_empty() {
                    v.remove(0);
                    trim_start(v);
                } else {
                    *t = trimmed;
                }
            }
            Inline::Emph(c)
            | Inline::Strong(c)
            | Inline::Strike(c)
            | Inline::Underline(c) => {
                trim_start(c);
                if c.is_empty() {
                    v.remove(0);
                    trim_start(v);
                }
            }
            Inline::Link { text, .. } => {
                trim_start(text);
                if text.is_empty() {
                    v.remove(0);
                    trim_start(v);
                }
            }
            _ => {}
        }
    }
    fn trim_end(v: &mut Vec<Inline>) {
        let Some(last) = v.last_mut() else {
            return;
        };
        match last {
            Inline::Text(t) => {
                let trimmed = t.trim_end().to_string();
                if trimmed.is_empty() {
                    v.pop();
                    trim_end(v);
                } else {
                    *t = trimmed;
                }
            }
            Inline::Emph(c)
            | Inline::Strong(c)
            | Inline::Strike(c)
            | Inline::Underline(c) => {
                trim_end(c);
                if c.is_empty() {
                    v.pop();
                    trim_end(v);
                }
            }
            Inline::Link { text, .. } => {
                trim_end(text);
                if text.is_empty() {
                    v.pop();
                    trim_end(v);
                }
            }
            _ => {}
        }
    }
    trim_start(inlines);
    trim_end(inlines);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocks(raw: &str) -> Vec<Block> {
        html_blocks(raw)
    }

    #[test]
    fn paragraph_and_div_content() {
        let out = blocks("<p>one</p><div><p>two</p></div>");
        assert_eq!(out.len(), 2);
        assert!(matches!(&out[0], Block::Paragraph(_)));
        assert!(matches!(&out[1], Block::Paragraph(_)));
    }

    #[test]
    fn loose_text_becomes_paragraph() {
        let out = blocks("just <b>text</b>");
        assert_eq!(out.len(), 1);
        let Block::Paragraph(inlines) = &out[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(&inlines[0], Inline::Text(t) if t.contains("just")));
        assert!(inlines.iter().any(|i| matches!(i, Inline::Strong(_))));
    }

    #[test]
    fn headings_map() {
        let out = blocks("<h2>Title</h2>");
        let Block::Heading { level, inlines } = &out[0] else {
            panic!("expected heading");
        };
        assert_eq!(*level, 2);
        assert!(!inlines.is_empty());
    }

    #[test]
    fn pre_preserves_whitespace_and_grabs_language() {
        let out = blocks("<pre><code class=\"language-rust\">fn a() {\n    b();\n}\n</code></pre>");
        let Block::Code(code) = &out[0] else {
            panic!("expected code");
        };
        assert_eq!(code.info, "rust");
        assert_eq!(code.lines, vec!["fn a() {", "    b();", "}"]);
    }

    #[test]
    fn pre_drops_single_leading_newline() {
        let out = blocks("<pre>\nx\n</pre>");
        let Block::Code(code) = &out[0] else {
            panic!("expected code");
        };
        assert_eq!(code.lines, vec!["x"]);
    }

    #[test]
    fn lists_map() {
        let out = blocks("<ol start=\"3\"><li>a</li><li>b</li></ol>");
        let Block::List(list) = &out[0] else {
            panic!("expected list");
        };
        assert!(list.ordered);
        assert_eq!(list.start, 3);
        assert_eq!(list.items.len(), 2);
    }

    #[test]
    fn nested_li_content() {
        let out = blocks("<ul><li>has <b>bold</b></li></ul>");
        let Block::List(list) = &out[0] else {
            panic!("expected list");
        };
        let Block::Paragraph(inlines) = &list.items[0].blocks[0] else {
            panic!("expected paragraph in li");
        };
        assert!(inlines.iter().any(|i| matches!(i, Inline::Strong(_))));
    }

    #[test]
    fn blockquote_maps() {
        let out = blocks("<blockquote><p>quoted</p></blockquote>");
        let Block::Quote(inner) = &out[0] else {
            panic!("expected quote");
        };
        assert!(matches!(&inner[0], Block::Paragraph(_)));
    }

    #[test]
    fn hr_maps() {
        let out = blocks("<hr>");
        assert!(matches!(&out[0], Block::ThematicBreak));
    }

    #[test]
    fn table_with_thead_maps() {
        let out = blocks(
            "<table><thead><tr><th align=\"left\">A</th><th>B</th></tr></thead>\
             <tbody><tr><td>1</td><td><b>2</b></td></tr></tbody></table>",
        );
        let Block::Table(table) = &out[0] else {
            panic!("expected table");
        };
        assert_eq!(table.aligns, vec![Align::Left, Align::None]);
        assert_eq!(table.head.len(), 2);
        assert_eq!(table.rows.len(), 1);
        let cell = &table.rows[0][1];
        assert!(cell.iter().any(|i| matches!(i, Inline::Strong(_))));
    }

    #[test]
    fn table_without_thead_first_row_is_head() {
        let out = blocks("<table><tr><td>a</td></tr><tr><td>b</td></tr></table>");
        let Block::Table(table) = &out[0] else {
            panic!("expected table");
        };
        assert_eq!(table.head.len(), 1);
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn img_and_a_convert() {
        let out = blocks(
            "<p><a href=\"https://x.y\" title=\"t\">link</a> <img src=\"p.png\" alt=\"pic\"></p>",
        );
        let Block::Paragraph(inlines) = &out[0] else {
            panic!("expected paragraph");
        };
        assert!(inlines.iter().any(|i| matches!(
            i,
            Inline::Link { url, title: Some(_), .. } if url == "https://x.y"
        )));
        assert!(
            inlines
                .iter()
                .any(|i| matches!(i, Inline::Image(img) if img.alt == "pic"))
        );
    }

    #[test]
    fn comments_and_invisible_tags_dropped() {
        let out = blocks("before<style>body{}</style><!-- c -->after");
        assert_eq!(out.len(), 1);
        let Block::Paragraph(inlines) = &out[0] else {
            panic!("expected paragraph");
        };
        let joined: String = inlines
            .iter()
            .map(|i| i.plain_text())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("before"), "{joined}");
        assert!(joined.contains("after"), "{joined}");
        assert!(!joined.contains("body"), "{joined}");
    }

    #[test]
    fn unknown_tag_is_transparent_container() {
        let out = blocks("<my-grid><p>inside</p></my-grid>");
        assert_eq!(out.len(), 1);
        assert!(matches!(&out[0], Block::Paragraph(_)));
    }

    #[test]
    fn underline_variant_from_u_tag() {
        let out = blocks("<p><u>kept</u></p>");
        let Block::Paragraph(inlines) = &out[0] else {
            panic!("expected paragraph");
        };
        assert!(inlines.iter().any(|i| matches!(i, Inline::Underline(_))));
    }

    #[test]
    fn boundary_spaces_around_inline_elements_are_kept() {
        // The HTML space before <strong> is significant.
        let Block::Paragraph(inlines) = &blocks("<p>含 <strong>strong</strong>。</p>")[0] else {
            panic!("expected paragraph");
        };
        let texts: Vec<&str> = inlines
            .iter()
            .filter_map(|i| match i {
                Inline::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.contains(&"含 "), "{texts:?}");
        assert!(texts.contains(&"。"), "{texts:?}");
    }

    #[test]
    fn paragraph_edges_are_trimmed_through_containers() {
        let Block::Paragraph(inlines) = &blocks("<p> <b> padded </b> </p>")[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.len(), 1, "edge whitespace vanishes: {inlines:?}");
        let Inline::Strong(children) = &inlines[0] else {
            panic!("expected strong");
        };
        assert_eq!(children, &vec![Inline::Text("padded".into())]);
    }

    #[test]
    fn glued_tags_have_no_invented_space() {
        // `a<b>b</b>c` — no whitespace in the source, none rendered.
        let Block::Paragraph(inlines) = &blocks("<p>a<b>b</b>c</p>")[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines[0], Inline::Text("a".into()));
        assert_eq!(inlines[2], Inline::Text("c".into()));
    }

    #[test]
    fn whitespace_only_text_between_inlines_becomes_a_space() {
        let Block::Paragraph(inlines) = &blocks("<p><b>x</b> <i>y</i></p>")[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(inlines.len(), 3);
        assert_eq!(inlines[1], Inline::Text(" ".into()));
    }
}
