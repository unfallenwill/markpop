//! Decoding of local raster images (PNG, BMP) for real 图文混排 rendering.
//!
//! The decoders are deliberately narrow — each unsupported feature
//! (interlaced PNG, RLE BMP, JPEG, WebP, …) decodes to `None`, and the
//! layout falls back to the image placeholder it has always drawn. A
//! picture the decoder cannot read must degrade, never crash or error.

mod bmp;
mod inflate;
mod png;

pub(crate) use inflate::inflate;
pub(crate) use png::DecodedPng;
#[cfg(test)]
pub(crate) use png::SIGNATURE;

/// Hard cap on total pixels; a corrupt or hostile header must not make
/// us allocate gigabytes. Shared with the BMP decoder.
pub(crate) const MAX_PIXELS: u64 = 100_000_000;

/// A decoded image in straight-alpha RGBA8.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Decoded {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

/// Detects the format from the magic bytes and decodes.
pub(crate) fn decode(data: &[u8]) -> Option<Decoded> {
    if data.len() >= 8 && data[..8] == png::SIGNATURE {
        let DecodedPng { width, height, pixels } = png::decode_png(data)?;
        return Some(Decoded { width, height, pixels });
    }
    if data.len() >= 2 && &data[..2] == b"BM" {
        return bmp::decode_bmp(data);
    }
    None
}

/// The MIME type of a raster file, from its magic bytes. Used for the
/// SVG data URI when the original bytes are embedded as-is.
pub(crate) fn mime(data: &[u8]) -> Option<&'static str> {
    if data.len() >= 8 && data[..8] == png::SIGNATURE {
        return Some("image/png");
    }
    None
}
