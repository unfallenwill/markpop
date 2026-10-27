//! Writing Markdown as Microsoft Word documents.
//!
//! A `.docx` is a ZIP container holding OOXML parts. The Markdown AST is
//! mapped onto a Word model, the model is serialized into parts, and the
//! parts are zipped — three stages, each testable on its own.

pub mod content;
pub mod media;
pub mod model;
pub mod write;
pub mod xml;
pub mod zip;

use crate::options::{Options, RenderError};
use crate::parse::parse;

/// Renders Markdown as a Word document.
///
/// The document uses Word's built-in styles, so the theme does not affect
/// how it looks; Word paginates the flow itself.
///
/// ```
/// let bytes = markpop::docx::render_bytes("# Hello", &markpop::Options::default()).unwrap();
/// assert_eq!(&bytes[..4], b"PK\x03\x04");
/// ```
pub fn render_bytes(markdown: &str, options: &Options) -> Result<Vec<u8>, RenderError> {
    let doc = parse(markdown);
    let content = content::build(&doc, options);

    let mut package = zip::ZipWriter::new();
    package.add(
        "[Content_Types].xml",
        write::content_types(content.images.len()).into_bytes(),
    );
    package.add("_rels/.rels", write::package_rels().into_bytes());
    package.add("word/document.xml", write::document(&content).into_bytes());
    package.add(
        "word/_rels/document.xml.rels",
        write::document_rels(&content.images, &content.links).into_bytes(),
    );
    package.add("word/styles.xml", write::styles().into_bytes());
    package.add(
        "word/numbering.xml",
        write::numbering(&content.numbering).into_bytes(),
    );
    package.add(
        "word/settings.xml",
        write::settings(content.toc).into_bytes(),
    );
    for image in &content.images {
        package.add(&format!("word/media/{}", image.name), image.bytes.clone());
    }
    package.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_is_a_zip_with_the_expected_parts() {
        let bytes = render_bytes("# Hi\n", &Options::default()).unwrap();
        assert_eq!(&bytes[..4], &[0x50, 0x4b, 0x03, 0x04]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("[Content_Types].xml"));
        assert!(text.contains("word/document.xml"));
    }

    #[test]
    fn the_same_input_produces_the_same_bytes() {
        let a = render_bytes("# Hi\n\ntext", &Options::default()).unwrap();
        let b = render_bytes("# Hi\n\ntext", &Options::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn an_empty_document_is_still_a_valid_package() {
        let bytes = render_bytes("", &Options::default()).unwrap();
        let entries = zip::read_archive(&bytes).expect("a readable archive");
        assert!(entries.iter().any(|(name, _)| name == "word/document.xml"));
    }
}
