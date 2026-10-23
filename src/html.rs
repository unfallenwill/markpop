//! Minimal HTML interpretation for raw Markdown HTML.
//!
//! The Markdown specification (CommonMark 0.31.2 §4.6 "HTML blocks" and
//! §6.6 "Raw HTML") defines exactly which HTML reaches this crate: block
//! chunks captured verbatim and inline tags matching the raw-HTML grammar.
//! This module turns that captured markup into something layout can draw.
//!
//! The boundary is the spec's: anything the spec passes through is at most
//! a tag with attributes and text. There is no CSS, no scripting, and no
//! resource fetching. Unknown tag names still parse (the spec explicitly
//! allows custom tags) and act as transparent containers; comments,
//! processing instructions, declarations and CDATA are dropped, matching
//! how a browser renders them (nothing).

use crate::parse::decode_entities;

/// One parsed HTML node: an element or text.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HtmlNode {
    /// An element (known or unknown tag name).
    Element(HtmlElement),
    /// Text content with entities already decoded.
    Text(String),
}

/// An HTML element: lowercased tag name, attributes and children.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HtmlElement {
    /// Tag name, lowercased (`div`, `my-custom-tag`, …).
    pub tag: String,
    /// Attributes in source order; duplicates keep the first value.
    pub attrs: Vec<(String, String)>,
    /// Child nodes (elements and text).
    pub children: Vec<HtmlNode>,
}

impl HtmlElement {
    /// Value of an attribute (case-insensitive name lookup).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A single tag as tokenized from raw HTML.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct HtmlTag {
    /// Lowercased tag name.
    pub name: String,
    /// Attributes in source order.
    pub attrs: Vec<(String, String)>,
    /// Written as `<tag …/>` (or is a void element).
    pub self_closing: bool,
}

impl HtmlTag {
    /// Value of an attribute (case-insensitive name lookup).
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A tokenizer item. Comments, processing instructions, declarations and
/// CDATA are dropped during tokenization, exactly like a browser drops
/// them from rendered output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum HtmlToken {
    /// `<tag …>` or `<tag …/>`.
    Start(HtmlTag),
    /// `</tag>`.
    End(String),
    /// Text between tags, entities decoded.
    Text(String),
}

/// Elements that never have closing tags.
const VOID_TAGS: [&str; 12] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track",
];

/// Whether a tag name is a void element (never has a close tag).
pub(crate) fn is_void(tag: &str) -> bool {
    VOID_TAGS.contains(&tag)
}

/// Block-level names that auto-close an open `<p>` (HTML semantics: a new
/// block ends the paragraph).
const BLOCK_TAGS: [&str; 28] = [
    "address", "article", "aside", "blockquote", "center", "details", "div", "dl", "fieldset",
    "figcaption", "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header",
    "hr", "li", "main", "nav", "ol", "p", "pre", "table",
];

/// Names whose sibling occurrence closes the previous open one
/// (`<li>` closes `<li>`, `<tr>` closes `<tr>`, …).
const SIBLING_TAGS: [&str; 7] = ["li", "dt", "dd", "tr", "td", "th", "option"];

/// Tokenizes a chunk of raw HTML into tags and text.
///
/// Malformed input degrades to text: a `<` that does not start anything
/// tag-like stays literal, exactly like the Markdown inline scanner that
/// produced this markup in the first place.
pub(crate) fn tokenize_html(raw: &str) -> Vec<HtmlToken> {
    let chars: Vec<char> = raw.chars().collect();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut i = 0;

    let flush = |text: &mut String, tokens: &mut Vec<HtmlToken>| {
        if !text.is_empty() {
            tokens.push(HtmlToken::Text(decode_entities(text)));
            text.clear();
        }
    };

    while i < chars.len() {
        if chars[i] != '<' {
            text.push(chars[i]);
            i += 1;
            continue;
        }
        // A markup declaration: comments, processing instructions,
        // declarations, CDATA. All dropped.
        if raw_like_construct(&chars, i) {
            match skip_raw_construct(&chars, i) {
                Some(next) => {
                    i = next;
                    continue;
                }
                // Unterminated construct: treat the `<` as text.
                None => {
                    text.push('<');
                    i += 1;
                    continue;
                }
            }
        }
        // A closing tag.
        if chars[i] == '<' && chars.get(i + 1) == Some(&'/') {
            if let Some((name, next)) = scan_tag_name(&chars, i + 2) {
                let next = skip_to_gt(&chars, next);
                flush(&mut text, &mut tokens);
                tokens.push(HtmlToken::End(name));
                i = next;
                continue;
            }
            text.push('<');
            i += 1;
            continue;
        }
        // An opening tag.
        if chars.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic())
            && let Some((name, mut next)) = scan_tag_name(&chars, i + 1)
        {
            let mut attrs = Vec::new();
            let mut self_closing = false;
            loop {
                next = skip_ws(&chars, next);
                match chars.get(next) {
                    Some('>') => {
                        next += 1;
                        break;
                    }
                    Some('/') if chars.get(next + 1) == Some(&'>') => {
                        self_closing = true;
                        next += 2;
                        break;
                    }
                    Some(_) => {
                        if let Some(((key, value), after)) = scan_attr(&chars, next) {
                            // First declaration of a name wins.
                            if !attrs.iter().any(|(k, _): &(String, String)| {
                                k.eq_ignore_ascii_case(&key)
                            }) {
                                attrs.push((key, value));
                            }
                            next = after;
                        } else {
                            next += 1;
                        }
                    }
                    None => break,
                }
            }
            flush(&mut text, &mut tokens);
            let void = is_void(&name);
            tokens.push(HtmlToken::Start(HtmlTag {
                name,
                attrs,
                self_closing: self_closing || void,
            }));
            i = next;
            continue;
        }
        text.push('<');
        i += 1;
    }
    flush(&mut text, &mut tokens);
    tokens
}

/// Parses raw HTML into a node tree.
///
/// Tag soup is tolerated the way browsers tolerate it: a close tag without
/// a matching open element is ignored, unclosed elements are closed at the
/// end of input, `<li>`/`<tr>`/`<td>`-style elements close their previous
/// open sibling, and a block-level start tag closes an open `<p>`.
pub(crate) fn parse_html(raw: &str) -> Vec<HtmlNode> {
    let mut root: Vec<HtmlNode> = Vec::new();
    let mut stack: Vec<HtmlElement> = Vec::new();

    macro_rules! append {
        ($node:expr) => {
            match stack.last_mut() {
                Some(top) => top.children.push($node),
                None => root.push($node),
            }
        };
    }

    // Closes elements down to depth `pos`, handing each back to its
    // parent (or the root). This is where implicitly-closed elements —
    // by an unmatched end tag, an auto-close rule, or end of input —
    // finally land in the tree.
    macro_rules! close_to {
        ($pos:expr) => {
            while stack.len() > $pos {
                let el = stack.pop().expect("depth checked above");
                append!(HtmlNode::Element(el));
            }
        };
    }

    for token in tokenize_html(raw) {
        match token {
            HtmlToken::Text(text) => {
                if !text.is_empty() {
                    append!(HtmlNode::Text(text));
                }
            }
            HtmlToken::End(name) => {
                if let Some(pos) = stack.iter().rposition(|el| el.tag == name) {
                    close_to!(pos);
                }
            }
            HtmlToken::Start(tag) => {
                if SIBLING_TAGS.contains(&tag.name.as_str())
                    && let Some(pos) = stack.iter().rposition(|el| el.tag == tag.name)
                {
                    close_to!(pos);
                }
                if BLOCK_TAGS.contains(&tag.name.as_str())
                    && stack.last().is_some_and(|el| el.tag == "p")
                {
                    let keep = stack.len() - 1;
                    close_to!(keep);
                }
                let element = HtmlElement {
                    tag: tag.name.clone(),
                    attrs: tag.attrs,
                    children: Vec::new(),
                };
                if tag.self_closing {
                    append!(HtmlNode::Element(element));
                } else {
                    stack.push(element);
                }
            }
        }
    }
    close_to!(0);
    root
}

/// Whether `chars[i..]` begins a comment, PI, declaration or CDATA section.
fn raw_like_construct(chars: &[char], i: usize) -> bool {
    match chars.get(i + 1) {
        Some('!') => {
            starts_with(chars, i, "<!--") || starts_with(chars, i, "<![CDATA[")
                || chars.get(i + 2).is_some_and(|c| c.is_ascii_alphabetic())
        }
        Some('?') => true,
        _ => false,
    }
}

/// Skips a comment / PI / declaration / CDATA construct; returns the index
/// after it, or `None` when unterminated (the caller keeps the `<` literal).
fn skip_raw_construct(chars: &[char], i: usize) -> Option<usize> {
    let rest: String = chars[i..].iter().collect();
    if starts_with(chars, i, "<!--") {
        return rest.find("-->").map(|pos| i + pos + 3);
    }
    if starts_with(chars, i, "<?") {
        return rest.find("?>").map(|pos| i + pos + 2);
    }
    if starts_with(chars, i, "<![CDATA[") {
        return rest.find("]]>").map(|pos| i + pos + 3);
    }
    // Declaration (`<!DOCTYPE …>`): ends at the first `>`.
    rest.find('>').map(|pos| i + pos + 1)
}

/// Scans a tag name: ASCII letter followed by letters, digits, hyphens,
/// underscores or colons (the spec's grammar is letters/digits/hyphens;
/// the colon keeps namespaced names like `docbook:para` usable, matching
/// the spec's prose about custom tags).
fn scan_tag_name(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut name = String::new();
    let mut i = start;
    if !chars.get(i).is_some_and(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':' {
            name.push(c.to_ascii_lowercase());
            i += 1;
        } else {
            break;
        }
    }
    Some((name, i))
}

/// Scans one attribute. Returns `None` when there is no attribute name at
/// `start` (plain whitespace or junk to skip); valueless attributes get an
/// empty value. Returns `(name, value)` with the value entity-decoded.
fn scan_attr(chars: &[char], start: usize) -> Option<((String, String), usize)> {
    let mut name = String::new();
    let mut i = start;
    // Attribute names are permissive in practice; take everything up to
    // `=`, whitespace, `>` or `/`.
    while let Some(&c) = chars.get(i) {
        if c.is_whitespace() || c == '=' || c == '>' || c == '/' {
            break;
        }
        name.push(c.to_ascii_lowercase());
        i += 1;
    }
    if name.is_empty() {
        return None;
    }
    let after_name = skip_ws(chars, i);
    if chars.get(after_name) != Some(&'=') {
        return Some(((name, String::new()), i));
    }
    let mut j = skip_ws(chars, after_name + 1);
    let mut value = String::new();
    match chars.get(j) {
        Some(quote @ ('"' | '\'')) => {
            j += 1;
            while let Some(&c) = chars.get(j) {
                if c == *quote {
                    j += 1;
                    break;
                }
                value.push(c);
                j += 1;
            }
        }
        Some(_) => {
            while let Some(&c) = chars.get(j) {
                if c.is_whitespace() || c == '>' {
                    break;
                }
                value.push(c);
                j += 1;
            }
        }
        None => {}
    }
    Some(((name, decode_entities(&value)), j))
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while chars.get(i).is_some_and(|c| c.is_whitespace()) {
        i += 1;
    }
    i
}

fn skip_to_gt(chars: &[char], mut i: usize) -> usize {
    while let Some(&c) = chars.get(i) {
        i += 1;
        if c == '>' {
            break;
        }
    }
    i
}

fn starts_with(chars: &[char], i: usize, pattern: &str) -> bool {
    pattern.chars().enumerate().all(|(offset, c)| chars.get(i + offset) == Some(&c))
}

/// Collapses HTML whitespace runs to single spaces and trims the ends.
///
/// Text inside `<pre>`/`<code>` keeps its spacing; every other HTML text
/// context collapses whitespace per HTML semantics.
pub(crate) fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(raw: &str) -> Vec<HtmlToken> {
        tokenize_html(raw)
    }

    #[test]
    fn start_and_end_tags_tokenize() {
        let toks = tags("<div class=\"a\">x</div>");
        assert_eq!(
            toks,
            vec![
                HtmlToken::Start(HtmlTag {
                    name: "div".into(),
                    attrs: vec![("class".into(), "a".into())],
                    self_closing: false,
                }),
                HtmlToken::Text("x".into()),
                HtmlToken::End("div".into()),
            ]
        );
    }

    #[test]
    fn tag_names_are_lowercased() {
        assert_eq!(tags("<DIV>")[0], tags("<div>")[0]);
    }

    #[test]
    fn attribute_forms() {
        let toks = tags("<img src=pic.png width='40' height=\"24\" data-x>");
        let HtmlToken::Start(tag) = &toks[0] else {
            panic!("expected start tag");
        };
        assert_eq!(tag.name, "img");
        assert_eq!(tag.attrs[0], ("src".into(), "pic.png".into()));
        assert_eq!(tag.attrs[1], ("width".into(), "40".into()));
        assert_eq!(tag.attrs[2], ("height".into(), "24".into()));
        assert_eq!(tag.attrs[3], ("data-x".into(), String::new()));
        assert!(tag.self_closing, "img is void");
    }

    #[test]
    fn first_duplicate_attribute_wins() {
        let HtmlToken::Start(tag) = &tags("<a href=one href=two>")[0] else {
            panic!("expected start tag");
        };
        assert_eq!(tag.attr("href"), Some("one"));
    }

    #[test]
    fn comments_and_declarations_are_dropped() {
        // Dropping a comment merges the text around it, like a browser.
        assert_eq!(
            tags("a<!-- hidden -->b"),
            vec![HtmlToken::Text("ab".into())]
        );
        assert!(tags("<?php echo 1; ?>").is_empty());
        assert!(tags("<!DOCTYPE html>").is_empty());
        assert!(tags("<![CDATA[data]]>").is_empty());
    }

    #[test]
    fn unterminated_construct_keeps_literal_lt() {
        assert_eq!(tags("a<!-- open"), [HtmlToken::Text("a<!-- open".into())]);
        assert_eq!(tags("a< b"), [HtmlToken::Text("a< b".into())]);
    }

    #[test]
    fn entities_decode_in_text_and_attrs() {
        let toks = tags("<a title=\"&lt;x&gt;\">&amp; &copy;</a>");
        let HtmlToken::Start(tag) = &toks[0] else {
            panic!("expected start tag");
        };
        assert_eq!(tag.attr("title"), Some("<x>"));
        assert_eq!(toks[1], HtmlToken::Text("& ©".into()));
    }

    #[test]
    fn custom_tags_parse() {
        let tree = parse_html("<docbook:para>x</docbook:para>");
        assert_eq!(
            tree,
            vec![HtmlNode::Element(HtmlElement {
                tag: "docbook:para".into(),
                attrs: vec![],
                children: vec![HtmlNode::Text("x".into())],
            })]
        );
    }

    #[test]
    fn stray_end_tag_is_ignored() {
        let tree = parse_html("</b>text");
        assert_eq!(tree, vec![HtmlNode::Text("text".into())]);
    }

    #[test]
    fn unclosed_elements_close_at_end() {
        let HtmlNode::Element(el) = &parse_html("<b>x")[0] else {
            panic!("expected element");
        };
        assert_eq!(el.tag, "b");
        assert_eq!(el.children, vec![HtmlNode::Text("x".into())]);
    }

    #[test]
    fn li_closes_previous_li() {
        let HtmlNode::Element(ul) = &parse_html("<ul><li>a<li>b</ul>")[0] else {
            panic!("expected element");
        };
        assert_eq!(ul.tag, "ul");
        assert_eq!(ul.children.len(), 2);
        let HtmlNode::Element(first) = &ul.children[0] else {
            panic!("expected li");
        };
        assert_eq!(first.tag, "li");
        assert_eq!(first.children, vec![HtmlNode::Text("a".into())]);
    }

    #[test]
    fn block_tag_closes_open_p() {
        let tree = parse_html("<p>one<div>two</div>");
        assert_eq!(tree.len(), 2);
        let HtmlNode::Element(p) = &tree[0] else {
            panic!("expected p");
        };
        assert_eq!(p.children, vec![HtmlNode::Text("one".into())]);
        let HtmlNode::Element(div) = &tree[1] else {
            panic!("expected div");
        };
        assert_eq!(div.tag, "div");
    }

    #[test]
    fn void_elements_have_no_children() {
        let tree = parse_html("line<br>next");
        assert_eq!(
            tree,
            vec![
                HtmlNode::Text("line".into()),
                HtmlNode::Element(HtmlElement {
                    tag: "br".into(),
                    attrs: vec![],
                    children: vec![],
                }),
                HtmlNode::Text("next".into()),
            ]
        );
    }

    #[test]
    fn self_closing_syntax() {
        let HtmlNode::Element(el) = &parse_html("<br/>")[0] else {
            panic!("expected element");
        };
        assert_eq!(el.tag, "br");
        assert!(el.children.is_empty());
    }

    #[test]
    fn text_and_elements_nest() {
        let tree = parse_html("<div>a<b>c</b>d</div>");
        let HtmlNode::Element(div) = &tree[0] else {
            panic!("expected div");
        };
        assert_eq!(
            div.children,
            vec![
                HtmlNode::Text("a".into()),
                HtmlNode::Element(HtmlElement {
                    tag: "b".into(),
                    attrs: vec![],
                    children: vec![HtmlNode::Text("c".into())],
                }),
                HtmlNode::Text("d".into()),
            ]
        );
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(collapse_spaces("  a\n\n  b\t c "), "a b c");
        assert_eq!(collapse_spaces("   "), "");
    }

    #[test]
    fn close_tag_matches_deepest_open() {
        // `</div>` closes both `<i>` (implicitly) and `<div>`; trailing
        // text becomes a root-level sibling.
        let tree = parse_html("<div><i>x</div>y");
        assert_eq!(tree.len(), 2);
        let HtmlNode::Element(div) = &tree[0] else {
            panic!("expected div");
        };
        let HtmlNode::Element(inner) = &div.children[0] else {
            panic!("expected inner element");
        };
        assert_eq!(inner.children, vec![HtmlNode::Text("x".into())]);
        assert_eq!(tree[1], HtmlNode::Text("y".into()));
    }
}
