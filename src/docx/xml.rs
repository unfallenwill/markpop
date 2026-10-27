//! A minimal XML writer for OOXML parts.
//!
//! OOXML is whitespace-sensitive: a space between two runs inside a
//! paragraph is real text in Word. The writer therefore never indents and
//! never adds whitespace of its own.

/// Escapes text for an XML text node.
///
/// Characters XML 1.0 does not allow are dropped — Word rejects a
/// document that contains them.
///
/// ```
/// use markpop::docx::xml::escape_text;
/// assert_eq!(escape_text("a & b"), "a &amp; b");
/// ```
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Escapes text for an XML attribute value.
///
/// ```
/// use markpop::docx::xml::escape_attr;
/// assert_eq!(escape_attr("say \"hi\""), "say &quot;hi&quot;");
/// ```
pub fn escape_attr(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// An XML element with attributes, child elements and text.
///
/// ```
/// use markpop::docx::xml::Element;
/// let e = Element::new("w:p").child(Element::new("w:t").text("hi"));
/// assert_eq!(e.to_string(), "<w:p><w:t>hi</w:t></w:p>");
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    tag: String,
    attrs: Vec<(String, String)>,
    children: Vec<Element>,
    text: Option<String>,
}

impl Element {
    /// Starts an element with the given tag name.
    pub fn new(tag: &str) -> Element {
        Element {
            tag: tag.to_string(),
            attrs: Vec::new(),
            children: Vec::new(),
            text: None,
        }
    }

    /// Adds an attribute.
    pub fn attr(mut self, name: &str, value: impl Into<String>) -> Element {
        self.attrs.push((name.to_string(), value.into()));
        self
    }

    /// Adds a child element.
    pub fn child(mut self, child: Element) -> Element {
        self.children.push(child);
        self
    }

    /// Adds several child elements.
    pub fn children(mut self, children: impl IntoIterator<Item = Element>) -> Element {
        self.children.extend(children);
        self
    }

    /// Sets the element's text content.
    pub fn text(mut self, text: impl Into<String>) -> Element {
        self.text = Some(text.into());
        self
    }

    /// Whether the element has neither children nor text.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty() && self.text.is_none()
    }

    /// The element's tag name.
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Appends the element's XML to `out`.
    pub fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.tag);
        for (name, value) in &self.attrs {
            out.push(' ');
            out.push_str(name);
            out.push_str("=\"");
            out.push_str(&escape_attr(value));
            out.push('"');
        }
        if self.is_empty() {
            out.push_str("/>");
            return;
        }
        out.push('>');
        if let Some(text) = &self.text {
            out.push_str(&escape_text(text));
        }
        for child in &self.children {
            child.write(out);
        }
        out.push_str("</");
        out.push_str(&self.tag);
        out.push('>');
    }

    /// Renders the element to a string.
    pub fn to_string(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_escapes_the_five_entities() {
        assert_eq!(escape_text("a & b < c > d"), "a &amp; b &lt; c &gt; d");
    }

    #[test]
    fn attributes_also_escape_quotes() {
        assert_eq!(
            escape_attr("say \"hi\" & 'bye'"),
            "say &quot;hi&quot; &amp; &apos;bye&apos;"
        );
    }

    #[test]
    fn control_characters_are_dropped() {
        // XML 1.0 forbids these; Word rejects documents containing them.
        assert_eq!(escape_text("a\u{0}b\u{8}c"), "abc");
        assert_eq!(
            escape_text("keep\ttab\nand\rreturn"),
            "keep\ttab\nand\rreturn"
        );
    }

    #[test]
    fn non_ascii_text_survives() {
        assert_eq!(escape_text("中文 — ok"), "中文 — ok");
    }

    #[test]
    fn elements_render_with_attributes_and_children() {
        let e = Element::new("w:p")
            .attr("xml:space", "preserve")
            .child(Element::new("w:t").text("hi"));
        assert_eq!(
            e.to_string(),
            r#"<w:p xml:space="preserve"><w:t>hi</w:t></w:p>"#
        );
    }

    #[test]
    fn empty_elements_are_self_closing() {
        assert_eq!(Element::new("w:b").to_string(), "<w:b/>");
    }

    #[test]
    fn text_only_elements_keep_their_text() {
        assert_eq!(
            Element::new("w:t").text("a & b").to_string(),
            "<w:t>a &amp; b</w:t>"
        );
    }

    #[test]
    fn children_keep_their_order() {
        let e = Element::new("w:p")
            .child(Element::new("w:r").text("1"))
            .child(Element::new("w:r").text("2"));
        assert_eq!(e.to_string(), "<w:p><w:r>1</w:r><w:r>2</w:r></w:p>");
    }

    #[test]
    fn an_element_reports_whether_it_is_empty() {
        assert!(Element::new("w:b").is_empty());
        assert!(!Element::new("w:b").text("x").is_empty());
        assert!(!Element::new("w:b").child(Element::new("w:i")).is_empty());
    }

    #[test]
    fn no_whitespace_is_inserted_between_elements() {
        // A space between two runs is real text in Word.
        let e = Element::new("w:p")
            .child(Element::new("w:r").text("a"))
            .child(Element::new("w:r").text("b"));
        assert!(!e.to_string().contains(' '));
    }

    #[test]
    fn the_tag_name_is_available() {
        assert_eq!(Element::new("w:p").tag(), "w:p");
    }
}
