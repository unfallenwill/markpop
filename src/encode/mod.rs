//! Bitmap encoders.
//!
//! Both encoders consume the same [`Bitmap`](crate::raster::Bitmap) produced
//! by the rasterizer, which keeps the output engines independent of each
//! other and of the layout engine.

pub mod deflate;
pub mod png;
pub mod webp;
