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

// ---------------------------------------------------------------------------
// 图文混排 layout acceptance — every rule from the spec gets a coordinate
// assertion on the Scene, not just "something rendered".
// ---------------------------------------------------------------------------

use markpop::{to_scene_with_options, Draw};

fn images_of(scene: &markpop::Scene) -> Vec<&markpop::scene::ImageDraw> {
    scene
        .draws
        .iter()
        .filter_map(|d| match d {
            Draw::Image(i) => Some(i),
            _ => None,
        })
        .collect()
}

fn texts_of(scene: &markpop::Scene) -> Vec<&markpop::scene::TextDraw> {
    scene
        .draws
        .iter()
        .filter_map(|d| match d {
            Draw::Text(t) => Some(t),
            _ => None,
        })
        .collect()
}

fn content_width() -> f32 {
    markpop::Theme::light().width - 2.0 * markpop::Theme::light().padding
}

#[test]
fn inline_image_pushed_to_next_line_when_not_fitting() {
    let dir = fixture("push", "p.png", 100, 50);
    // 42 full-width CJK glyphs fill the first line exactly (one em per
    // glyph is the crate's promise): 672 px + space + image > usable width.
    let md = format!("{} ![i](p.png)", "字".repeat(42));
    let scene = to_scene_with_options(&md, &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    let padding = markpop::Theme::light().padding;
    assert!(
        (images[0].x - padding).abs() < 1.0,
        "image starts the next line at the left edge: {images:?}"
    );
    assert!(images[0].y > 64.0, "below the first line: {images:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_wider_than_line_is_scaled_to_line() {
    let dir = fixture("wide", "w.png", 2000, 50);
    let md = "start ![w](w.png) end";
    let scene = to_scene_with_options(md, &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    assert!(
        (images[0].w - content_width()).abs() < 1.0,
        "oversized inline image scales to the line: {} vs {}",
        images[0].w,
        content_width()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn multiple_inline_images_flow_with_text() {
    let dir = fixture("multi", "a.png", 40, 20);
    std::fs::copy(dir.join("a.png"), dir.join("b.png")).unwrap();
    let scene = to_scene_with_options("![a](a.png) 中间文字 ![b](b.png)", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 2);
    assert_eq!(scene.images.len(), 2, "two distinct files decode twice");
    let texts = texts_of(&scene);
    assert!(texts.iter().any(|t| t.text.contains("中间文字")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn same_image_twice_is_decoded_once() {
    let dir = fixture("cache", "c.png", 30, 30);
    let scene = to_scene_with_options("![x](c.png) and ![y](c.png)", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 2, "both references draw");
    assert_eq!(scene.images.len(), 1, "but the file decodes once");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn adjacent_images_without_text_sit_side_by_side() {
    let dir = fixture("adj", "a.png", 30, 20);
    std::fs::copy(dir.join("a.png"), dir.join("b.png")).unwrap();
    let scene = to_scene_with_options("![a](a.png)![b](b.png)", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 2);
    assert!(
        (images[1].x - (images[0].x + images[0].w)).abs() < 1.0,
        "second image starts where the first ends: {:?}",
        images
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_inside_heading() {
    let dir = fixture("head", "h.png", 40, 20);
    let scene = to_scene_with_options("## 标题 ![h](h.png) 混排", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1, "heading inline image renders");
    let expected_h = 1.2 * markpop::Theme::light().heading_size(2);
    assert!(
        (images[0].h - expected_h).abs() < 1.0,
        "height follows the heading font size: {} vs {}",
        images[0].h,
        expected_h
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_inside_table_cell() {
    let dir = fixture("cell", "t.png", 30, 15);
    let md = "| 图 | 文 |\n| --- | --- |\n| ![t](t.png) | 字 |";
    let scene = to_scene_with_options(md, &opts(&dir));
    assert_eq!(images_of(&scene).len(), 1, "cell image renders");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_inside_list_item() {
    let dir = fixture("li", "l.png", 30, 15);
    let scene = to_scene_with_options("- 项目 ![l](l.png) 混排", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    let padding = markpop::Theme::light().padding;
    assert!(images[0].x > padding, "list indent applies: {images:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_image_inside_blockquote() {
    let dir = fixture("bq", "q.png", 30, 15);
    let scene = to_scene_with_options("> 引用 ![q](q.png) 混排", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    let padding = markpop::Theme::light().padding;
    assert!(images[0].x > padding, "quote indent applies: {images:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reference_style_image_resolves() {
    let dir = fixture("ref", "r.png", 60, 30);
    let md = "![alt][r]\n\n[r]: r.png";
    let scene = to_scene_with_options(md, &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1, "reference definitions resolve: {scene:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn subdirectory_and_parent_paths_resolve() {
    let dir = fixture("paths", "p.png", 30, 15);
    std::fs::create_dir_all(dir.join("deep").join("sub")).unwrap();
    std::fs::copy(
        dir.join("p.png"),
        dir.join("deep").join("sub").join("p.png"),
    ).unwrap();
    let base = opts(&dir.join("deep"));
    // ../p.png climbs out of deep/ back to the base dir.
    let scene = to_scene_with_options("![a](../p.png) ![b](sub/p.png)", &base);
    assert_eq!(images_of(&scene).len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cjk_adjacent_to_image_wraps_correctly() {
    let dir = fixture("cjk", "c.png", 30, 15);
    let scene = to_scene_with_options("中![c](c.png)文混合，无空格相邻也不崩。", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    let texts = texts_of(&scene);
    assert!(texts.iter().any(|t| t.text.contains("中")));
    assert!(texts.iter().any(|t| t.text.contains("文")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tiny_figure_keeps_native_size_and_centers() {
    let dir = fixture("tinyfig", "t.png", 120, 60);
    let scene = to_scene_with_options("![t](t.png)", &opts(&dir));
    let images = images_of(&scene);
    assert_eq!(images.len(), 1);
    let padding = markpop::Theme::light().padding;
    assert!((images[0].w - 120.0).abs() < 1.0, "native width kept");
    assert!(
        images[0].x > padding && images[0].x + images[0].w < padding + content_width(),
        "centered: x = {}",
        images[0].x
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn wide_figure_scales_to_content_width() {
    let dir = fixture("widefig", "w.png", 2000, 100);
    let scene = to_scene_with_options("![w](w.png)", &opts(&dir));
    let images = images_of(&scene);
    assert!((images[0].w - content_width()).abs() < 1.0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn figure_inside_blockquote_and_list_item() {
    let dir = fixture("nest", "n.png", 200, 80);
    let quote = to_scene_with_options("> ![n](n.png)", &opts(&dir));
    assert_eq!(images_of(&quote).len(), 1, "figure inside quote renders");
    let list = to_scene_with_options("- ![n](n.png)", &opts(&dir));
    let images = images_of(&list);
    assert_eq!(images.len(), 1, "figure inside list item renders");
    let padding = markpop::Theme::light().padding;
    assert!(images[0].x > padding, "list indent applies: {images:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
