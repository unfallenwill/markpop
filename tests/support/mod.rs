//! Shared helpers for the conformance tests: corpus loading and a small
//! AST → HTML serializer that mirrors the shapes the specifications expect.

// Each test binary uses a different subset of these helpers.
#![allow(dead_code)]

use std::fmt::Write as _;

use markpop::ast::{Align, Block, Document, ImageData, Inline, ListData, TableData};
use markpop::parse::{parse, parse_commonmark};

/// One spec example: Markdown source plus the expected HTML.
pub struct Example {
    /// Markdown source.
    pub markdown: String,
    /// Expected HTML.
    pub html: String,
    /// Spec section the example belongs to.
    pub section: String,
    /// Example number in the specification.
    pub number: u32,
}

/// Reads a generated corpus: `== N section`, then two length-prefixed
/// payloads (Markdown and expected HTML).
pub fn examples(corpus: &str) -> Vec<Example> {
    let mut out = Vec::new();
    let mut rest = corpus;
    while let Some(start) = rest.find("== ") {
        rest = &rest[start + 3..];
        let header_end = rest.find('\n').expect("header line");
        let header = &rest[..header_end];
        rest = &rest[header_end + 1..];
        let (number, section) = header.split_once(' ').expect("number and section");
        let number: u32 = number.parse().expect("example number");
        let section = section.to_string();

        let payload = |rest: &mut &str| -> String {
            *rest = rest.trim_start_matches('\n');
            let key_end = rest.find('\n').expect("payload header");
            let header = &rest[..key_end];
            let len: usize = header
                .split_whitespace()
                .nth(1)
                .expect("length")
                .parse()
                .expect("length digits");
            let body = &rest[key_end + 1..];
            let value = body[..len].to_string();
            *rest = &body[len..];
            value
        };
        let markdown = payload(&mut rest);
        let html = payload(&mut rest);
        out.push(Example {
            markdown,
            html,
            section,
            number,
        });
    }
    out
}

pub fn escape_html(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Percent-encodes a URL the way the reference renderer does, then escapes
/// it for an HTML attribute.
pub fn escape_href(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for byte in url.bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_'
                    | b'.'
                    | b'+'
                    | b'!'
                    | b'*'
                    | b'\''
                    | b'('
                    | b')'
                    | b','
                    | b'%'
                    | b'#'
                    | b'@'
                    | b'?'
                    | b'='
                    | b';'
                    | b':'
                    | b'/'
                    | b'&'
                    | b'$'
                    | b'~'
            );
        if keep {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    escape_html(&out)
}

pub fn html_of_inlines(inlines: &[Inline], out: &mut String, gfm: bool) {
    for inline in inlines {
        match inline {
            Inline::Text(t) => out.push_str(&escape_html(t)),
            Inline::Code(c) => {
                out.push_str("<code>");
                out.push_str(&escape_html(c));
                out.push_str("</code>");
            }
            Inline::Emph(children) => {
                out.push_str("<em>");
                html_of_inlines(children, out, gfm);
                out.push_str("</em>");
            }
            Inline::Strong(children) => {
                out.push_str("<strong>");
                html_of_inlines(children, out, gfm);
                out.push_str("</strong>");
            }
            Inline::Strike(children) => {
                out.push_str("<del>");
                html_of_inlines(children, out, gfm);
                out.push_str("</del>");
            }
            // Only produced when rendering raw HTML, never by the parser.
            Inline::Underline(children) => {
                out.push_str("<u>");
                html_of_inlines(children, out, gfm);
                out.push_str("</u>");
            }
            Inline::Link { text, url, title } => {
                match title {
                    Some(title) => {
                        let _ = write!(
                            out,
                            "<a href=\"{}\" title=\"{}\">",
                            escape_href(url),
                            escape_html(title)
                        );
                    }
                    None => {
                        let _ = write!(out, "<a href=\"{}\">", escape_href(url));
                    }
                }
                html_of_inlines(text, out, gfm);
                out.push_str("</a>");
            }
            Inline::Image(ImageData { alt, url, title }) => match title {
                Some(title) => {
                    let _ = write!(
                        out,
                        "<img src=\"{}\" alt=\"{}\" title=\"{}\" />",
                        escape_href(url),
                        escape_html(alt),
                        escape_html(title)
                    );
                }
                None => {
                    let _ = write!(
                        out,
                        "<img src=\"{}\" alt=\"{}\" />",
                        escape_href(url),
                        escape_html(alt)
                    );
                }
            },
            Inline::Break => out.push_str("<br />\n"),
            Inline::SoftBreak => out.push('\n'),
            // Raw HTML is passed through verbatim, as the specifications ask,
            // except for the tags GFM filters out (only "<" is escaped).
            Inline::Html(raw) => {
                if gfm && is_filtered_tag(raw) {
                    out.push_str("&lt;");
                    out.push_str(&raw[1..]);
                } else {
                    out.push_str(raw);
                }
            }
        }
    }
}

fn align_attr(table: &TableData, index: usize) -> String {
    match table.aligns.get(index).copied().unwrap_or(Align::None) {
        Align::None => String::new(),
        Align::Left => " align=\"left\"".into(),
        Align::Center => " align=\"center\"".into(),
        Align::Right => " align=\"right\"".into(),
    }
}

fn html_of_table(table: &TableData, out: &mut String, gfm: bool) {
    out.push_str("<table>\n<thead>\n<tr>\n");
    for (i, cell) in table.head.iter().enumerate() {
        let _ = write!(out, "<th{}>", align_attr(table, i));
        html_of_inlines(cell, out, gfm);
        out.push_str("</th>\n");
    }
    out.push_str("</tr>\n</thead>\n");
    if !table.rows.is_empty() {
        out.push_str("<tbody>\n");
        for row in &table.rows {
            out.push_str("<tr>\n");
            for (i, cell) in row.iter().enumerate() {
                let _ = write!(out, "<td{}>", align_attr(table, i));
                html_of_inlines(cell, out, gfm);
                out.push_str("</td>\n");
            }
            out.push_str("</tr>\n");
        }
        out.push_str("</tbody>\n");
    }
    out.push_str("</table>\n");
}

fn html_of_list(list: &ListData, out: &mut String, gfm: bool) {
    if list.ordered {
        if list.start == 1 {
            out.push_str("<ol>\n");
        } else {
            let _ = writeln!(out, "<ol start=\"{}\">", list.start);
        }
    } else {
        out.push_str("<ul>\n");
    }
    for item in &list.items {
        out.push_str("<li>");
        match item.task {
            Some(true) => out.push_str("<input checked=\"\" disabled=\"\" type=\"checkbox\"> "),
            Some(false) => out.push_str("<input disabled=\"\" type=\"checkbox\"> "),
            None => {}
        }
        if item.blocks.is_empty() {
            out.push_str("</li>\n");
            continue;
        }
        if !list.tight {
            out.push('\n');
        }
        for (index, block) in item.blocks.iter().enumerate() {
            match block {
                Block::Paragraph(inlines) if list.tight => {
                    if index > 0 {
                        out.push('\n');
                    }
                    html_of_inlines(inlines, out, gfm);
                }
                other => {
                    if list.tight {
                        out.push('\n');
                    }
                    html_of_block(other, out, gfm);
                }
            }
        }
        out.push_str("</li>\n");
    }
    out.push_str(if list.ordered { "</ol>\n" } else { "</ul>\n" });
}

pub fn html_of_block(block: &Block, out: &mut String, gfm: bool) {
    match block {
        Block::Heading { level, inlines } => {
            let _ = write!(out, "<h{level}>");
            html_of_inlines(inlines, out, gfm);
            let _ = writeln!(out, "</h{level}>");
        }
        Block::Paragraph(inlines) => {
            out.push_str("<p>");
            html_of_inlines(inlines, out, gfm);
            out.push_str("</p>\n");
        }
        Block::Code(code) => {
            out.push_str("<pre><code");
            if let Some(lang) = code.info.split_whitespace().next()
                && !lang.is_empty()
            {
                let _ = write!(out, " class=\"language-{}\"", escape_html(lang));
            }
            out.push('>');
            out.push_str(&escape_html(&code.lines.join("\n")));
            if !code.lines.is_empty() {
                out.push('\n');
            }
            out.push_str("</code></pre>\n");
        }
        Block::Quote(children) => {
            out.push_str("<blockquote>\n");
            for child in children {
                html_of_block(child, out, gfm);
            }
            out.push_str("</blockquote>\n");
        }
        Block::List(list) => html_of_list(list, out, gfm),
        Block::Table(table) => html_of_table(table, out, gfm),
        Block::Figure(image) => {
            out.push_str("<p>");
            html_of_inlines(&[Inline::Image(image.clone())], out, gfm);
            out.push_str("</p>\n");
        }
        Block::ThematicBreak => out.push_str("<hr />\n"),
        Block::Html(raw) => {
            if gfm {
                out.push_str(&filter_raw_html(raw));
            } else {
                out.push_str(raw);
            }
            if !raw.ends_with('\n') {
                out.push('\n');
            }
        }
    }
}

pub fn html_of_document(doc: &Document, gfm: bool) -> String {
    let mut out = String::new();
    for block in &doc.blocks {
        html_of_block(block, &mut out, gfm);
    }
    out
}

/// Drops trailing spaces and blank lines so insignificant formatting is
/// ignored, but keeps the tag/text structure visible.
pub fn normalize(html: &str) -> String {
    html.lines()
        .map(|line| line.trim_end())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses an example as strict CommonMark and compares it with the spec.
pub fn compare(example: &Example) -> Result<(), String> {
    compare_with(parse_commonmark, false, example)
}

/// Parses an example as GitHub Flavored Markdown and compares it.
pub fn compare_gfm(example: &Example) -> Result<(), String> {
    compare_with(parse, true, example)
}

fn compare_with(parse: fn(&str) -> Document, gfm: bool, example: &Example) -> Result<(), String> {
    let doc = parse(&example.markdown);
    let actual = normalize(&html_of_document(&doc, gfm));
    let expected = normalize(&example.html);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "example {} ({}):\n  md:   {:?}\n  want: {:?}\n  got:  {:?}",
            example.number, example.section, example.markdown, expected, actual
        ))
    }
}

/// Tags GFM never passes through as raw HTML.
fn is_filtered_tag(text: &str) -> bool {
    const FILTERED: &[&str] = &[
        "title",
        "textarea",
        "style",
        "xmp",
        "iframe",
        "noembed",
        "noframes",
        "script",
        "plaintext",
    ];
    let lower = text.to_ascii_lowercase();
    let rest = lower.strip_prefix('<').unwrap_or(&lower);
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    FILTERED.iter().any(|tag| {
        rest.strip_prefix(tag)
            .is_some_and(|after| after.is_empty() || after.starts_with([' ', '\t', '\n', '>', '/']))
    })
}

/// Escapes `<` in front of the tags GFM filters out.
fn filter_raw_html(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let tail = &rest[open..];
        if is_filtered_tag(tail) {
            out.push_str("&lt;");
        } else {
            out.push('<');
        }
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}
