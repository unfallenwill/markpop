//! Writing Markdown as Microsoft Word documents.
//!
//! A `.docx` is a ZIP container holding OOXML parts. The Markdown AST is
//! mapped onto a Word model, the model is serialized into parts, and the
//! parts are zipped — three stages, each testable on its own.

pub mod media;
pub mod model;
pub mod xml;
pub mod zip;
