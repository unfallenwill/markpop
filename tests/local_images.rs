//! End-to-end acceptance for local image support (图文混排).
//!
//! Every test works through the public API against real files in a
//! temporary directory, mirroring how documents reference images.

use markpop::{encode::png, Options};

/// Writes a solid-color PNG fixture and returns the directory it lives in.
fn fixture(test: &str, name: &str, w: u32, h: u32) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("markpop-img-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut pixels = [0u8; 4].repeat((w * h) as usize);
    for px in pixels.as_chunks_mut::<4>().0 {
        px.copy_from_slice(&[200, 30, 30, 255]);
    }
    std::fs::write(dir.join(name), png::encode(w, h, &pixels)).unwrap();
    dir
}

fn opts(dir: &std::path::Path) -> Options {
    Options::default().with_base_dir(dir.display().to_string())
}

#[test]
fn local_figure_renders_as_real_image() {
    let dir = fixture("figure", "fig.png", 100, 50);
    let svg = markpop::render_bytes("![产品图](fig.png)", &opts(&dir)).unwrap();
    let svg = String::from_utf8(svg).unwrap();
    // Native 100x50 fits the content width, so the figure keeps its size.
    assert!(svg.contains("<image"), "{svg}");
    assert!(svg.contains("data:image/png;base64,"), "{svg}");
    assert!(svg.contains(r#"width="100" height="50""#), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_sits_in_the_text_flow() {
    let dir = fixture("inline", "inline.png", 100, 50);
    let svg = markpop::render_bytes(
        "前文 ![i](inline.png) 后文",
        &opts(&dir),
    )
    .unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(svg.contains("<image"), "{svg}");
    // 1.2 em tall (16 px body → 19.2), aspect 2:1 → 38.4 wide.
    assert!(svg.contains(r#"width="38.4""#), "{svg}");
    // The surrounding text survives on the same line.
    assert!(svg.contains("前文"), "{svg}");
    assert!(svg.contains("后文"), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_file_degrades_to_placeholder() {
    let dir = std::env::temp_dir().join(format!("markpop-img-none-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let svg = markpop::render_bytes("![alt](missing.png)", &opts(&dir)).unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(!svg.contains("<image"), "{svg}");
    assert!(svg.contains(">alt</text>"), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unsupported_format_degrades_to_placeholder() {
    let dir = std::env::temp_dir().join(format!("markpop-img-jpg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // A file that claims to be a JPEG but is not one we decode.
    std::fs::write(dir.join("photo.jpg"), [0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
    let svg = markpop::render_bytes("![p](photo.jpg)", &opts(&dir)).unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(!svg.contains("<image"), "{svg}");
    assert!(svg.contains(">p</text>"), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_local_images_flag_disables_reading() {
    let dir = fixture("disabled", "real.png", 40, 20);
    let opts = opts(&dir).with_local_images(false);
    let svg = markpop::render_bytes("![alt](real.png)", &opts).unwrap();
    let svg = String::from_utf8(svg).unwrap();
    assert!(!svg.contains("<image"), "{svg}");
    assert!(svg.contains(">alt</text>"), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rendering_is_deterministic() {
    let dir = fixture("determinism", "same.png", 64, 32);
    let a = markpop::render_bytes("![i](same.png)", &opts(&dir)).unwrap();
    let b = markpop::render_bytes("![i](same.png)", &opts(&dir)).unwrap();
    assert_eq!(a, b, "same input must give identical bytes");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bmp_images_are_supported() {
    let dir = std::env::temp_dir().join(format!("markpop-img-bmp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Hand-built 2x1 24-bit BMP (54-byte headers, bottom-up, red pixel).
    let mut bmp = Vec::new();
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&70u32.to_le_bytes()); // file size
    bmp.extend_from_slice(&[0; 4]); // reserved
    bmp.extend_from_slice(&54u32.to_le_bytes()); // pixel offset
    bmp.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    bmp.extend_from_slice(&2i32.to_le_bytes()); // width
    bmp.extend_from_slice(&1i32.to_le_bytes()); // height
    bmp.extend_from_slice(&1u16.to_le_bytes()); // planes
    bmp.extend_from_slice(&24u16.to_le_bytes()); // bpp
    bmp.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    bmp.extend_from_slice(&8u32.to_le_bytes()); // image size
    bmp.extend_from_slice(&[0; 16]); // resolutions, colors
    bmp.extend_from_slice(&[0u8, 0, 255]); // red pixel (BGR)
    bmp.extend_from_slice(&[255u8, 255, 255]); // white pixel
    bmp.extend_from_slice(&[0u8; 2]); // row padding to 4 bytes
    std::fs::write(dir.join("tiny.bmp"), &bmp).unwrap();

    let svg = markpop::render_bytes("![b](tiny.bmp)", &opts(&dir)).unwrap();
    let svg = String::from_utf8(svg).unwrap();
    // BMP is re-encoded to PNG for the data URI.
    assert!(svg.contains("<image"), "{svg}");
    assert!(svg.contains("data:image/png;base64,"), "{svg}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn oversized_figure_is_capped_by_the_page() {
    let dir = fixture("tall", "tall.png", 50, 4000);
    let opts = opts(&dir).with_page_height(400.0);
    let pages = markpop::render_bytes_pages("![t](tall.png)", &opts).unwrap();
    assert_eq!(pages.len(), 1);
    let svg = String::from_utf8(pages[0].clone()).unwrap();
    // Content height is 400 - 2*48 = 304: the figure must fit inside it.
    let capped = 304.0_f32;
    assert!(
        svg.contains(&format!(r#"height="{capped}""#)),
        "figure must be capped to the page: {svg}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
