//! End-to-end tests over the public API.
//!
//! These complement the unit tests inside each module: here we only care
//! about properties that must hold for *any* input — well-formed output,
//! zoom invariant geometry, determinism, and no panics.

use markdown2svg::scene::FontKind;
use markdown2svg::text::{Metrics, TextStyle};
use markdown2svg::{
    CanvasPreset, Color, Draw, Options, Theme, parse, render, render_pages, render_with, to_pages,
    to_scene,
};

/// A document exercising every supported construct.
const KITCHEN_SINK: &str = r#"
# Heading one

Some **bold**, *italic*, ***both***, ~~struck~~ and `inline code` text with a
[link](https://example.com) and an autolink <https://example.com/a>.

## Heading two

### Heading three

#### Heading four

##### Heading five

###### Heading six

Setext heading
==============

Another kind
------------

> A quote
>
> > A nested quote with **styling**

- bullet one
- bullet two
  - nested bullet
1. ordered one
2. ordered two

| left | center | right |
|:-----|:------:|------:|
| a    | b      | c     |

```rust
fn main() { println!("hi"); }
```

    indented code

---

![an image](https://example.com/x.png)
"#;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Checks that `svg` is well-formed XML-ish markup: a single root element,
/// balanced tags, self-closing leaves and fully quoted attributes.
///
/// Text nodes are checked separately, so any `<` inside content is a bug.
fn assert_well_formed(svg: &str) {
    let mut stack: Vec<String> = Vec::new();
    let mut roots = 0usize;
    let bytes = svg.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            // Text node: must not contain unescaped markup characters.
            let c = bytes[i] as char;
            assert!(c != '>', "stray `>` in text at byte {i} of\n{svg}");
            i += 1;
            continue;
        }
        let end = svg[i..]
            .find('>')
            .unwrap_or_else(|| panic!("unterminated tag at {i}"))
            + i;
        let raw = &svg[i + 1..end];
        if let Some(name) = raw.strip_prefix('/') {
            let top = stack
                .pop()
                .unwrap_or_else(|| panic!("unexpected `</{name}>`"));
            assert_eq!(top, name.trim(), "mismatched close tag");
        } else {
            let self_closing = raw.ends_with('/');
            let body = raw.trim_end_matches('/');
            let name = body
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            assert!(!name.is_empty(), "empty tag name");
            // Every attribute must be `key="value"`.
            let mut rest = body[name.len()..].trim();
            while !rest.is_empty() {
                let eq = rest
                    .find('=')
                    .unwrap_or_else(|| panic!("attribute without `=` in <{body}>"));
                let key = rest[..eq].trim();
                assert!(
                    !key.is_empty() && !key.contains(char::is_whitespace),
                    "bad key {key:?}"
                );
                rest = rest[eq + 1..].trim_start();
                assert!(
                    rest.starts_with('"'),
                    "unquoted value for {key} in <{body}>"
                );
                let close = rest[1..]
                    .find('"')
                    .unwrap_or_else(|| panic!("unterminated value for {key}"));
                rest = rest[close + 2..].trim_start();
            }
            if stack.is_empty() {
                roots += 1;
            }
            if !self_closing {
                stack.push(name);
            }
        }
        i = end + 1;
    }
    assert!(stack.is_empty(), "unclosed elements: {stack:?}");
    assert_eq!(roots, 1, "expected exactly one root element");
}

/// Extracts the root `<svg ...>` attributes as `(width, height, viewBox)`.
fn canvas(svg: &str) -> (f32, f32, String) {
    let first = svg.lines().next().expect("svg has a first line");
    let attr = |name: &str| -> String {
        let needle = format!("{name}=\"");
        let start = first
            .find(&needle)
            .unwrap_or_else(|| panic!("missing {name}"))
            + needle.len();
        first[start..].split('"').next().unwrap().to_string()
    };
    (
        attr("width").parse().unwrap(),
        attr("height").parse().unwrap(),
        attr("viewBox"),
    )
}

/// A deterministic pseudo-random generator (xorshift), so fuzz-style tests
/// are reproducible without pulling in a crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next() as usize) % items.len()]
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn kitchen_sink_renders_well_formed_svg() {
    let svg = render(KITCHEN_SINK);
    assert_well_formed(&svg);
    assert!(svg.starts_with("<svg "), "root element missing");
    assert!(svg.trim_end().ends_with("</svg>"));
}

#[test]
fn every_construct_reaches_the_output() {
    let svg = render(KITCHEN_SINK);
    for needle in [
        "Heading one",
        "Heading six",
        "Setext heading",
        "Another kind",
        "A quote",
        "A nested quote",
        "bullet two",
        "ordered two",
        "nested bullet",
        "inline code",
        "struck",
        "link",
        "an image",
        "fn main()",
        "indented code",
        "https://example.com/x.png",
    ] {
        assert!(svg.contains(needle), "missing {needle:?} in output");
    }
}

#[test]
fn heading_levels_produce_decreasing_font_sizes() {
    let scene = to_scene(KITCHEN_SINK, &Theme::light());
    let sizes: Vec<f32> = (1..=6)
        .map(|level| Theme::light().heading_size(level))
        .collect();
    for pair in sizes.windows(2) {
        assert!(pair[0] >= pair[1], "heading sizes must not grow: {sizes:?}");
    }
    // At least the h1 text is present at its expected size.
    let h1 = scene
        .draws
        .iter()
        .find_map(|d| match d {
            Draw::Text(t) if t.text == "Heading one" => Some(t.size),
            _ => None,
        })
        .expect("h1 text");
    assert_eq!(h1, sizes[0]);
}

#[test]
fn markup_in_text_is_escaped() {
    let svg = render("a < b & c > d and \"quotes\"");
    assert_well_formed(&svg);
    assert!(
        svg.contains("a &lt; b &amp; c &gt; d and &quot;quotes&quot;"),
        "{svg}"
    );
}

#[test]
fn zoom_scales_only_the_canvas() {
    let base = render_with("hello", &Options::default()).unwrap();
    let (w1, h1, vb1) = canvas(&base);

    for (zoom, factor) in [(1.5f32, 1.5f32), (2.0, 2.0), (0.5, 0.5)] {
        let zoomed = render_with("hello", &Options::default().with_zoom(zoom)).unwrap();
        let (w2, h2, vb2) = canvas(&zoomed);
        assert!(
            (w2 - w1 * factor).abs() < 0.05,
            "width at {zoom}x: {w2} vs {}",
            w1 * factor
        );
        assert!(
            (h2 - h1 * factor).abs() < 0.05,
            "height at {zoom}x: {h2} vs {}",
            h1 * factor
        );
        assert_eq!(vb2, vb1, "viewBox must not change with zoom");

        // Geometry: strip the root line, everything else is identical.
        let body = |s: &str| s.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(
            body(&base),
            body(&zoomed),
            "geometry changed at zoom {zoom}"
        );
    }
}

#[test]
fn forced_zoom_error_mentions_zoom() {
    let err = render_with("x", &Options::default().with_zoom(-1.0)).unwrap_err();
    assert!(err.to_string().contains("zoom"), "{err}");
}

#[test]
fn themes_share_geometry_but_differ_in_color() {
    let light = render_with("hello **world**", &Options::default()).unwrap();
    let dark = render_with(
        "hello **world**",
        &Options::default().with_theme(Theme::dark()),
    )
    .unwrap();
    // Compare positions only: colors must differ, geometry must not.
    let positions = |s: &str| {
        s.lines()
            .filter(|l| l.contains("<text"))
            .map(|l| {
                let get = |name: &str| {
                    let needle = format!("{name}=\"");
                    let start = l.find(&needle).expect("attribute") + needle.len();
                    l[start..].split('"').next().unwrap().to_string()
                };
                (get("x"), get("y"), get("font-size"))
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        positions(&light),
        positions(&dark),
        "layout must be theme-independent"
    );
    assert_ne!(light, dark);
}

#[test]
fn output_is_byte_identical_across_renders() {
    let a = render(KITCHEN_SINK);
    let b = render(KITCHEN_SINK);
    assert_eq!(a, b);
}

#[test]
fn empty_and_whitespace_input_is_valid() {
    for src in ["", "   ", "\n\n\n", "\t\t"] {
        let svg = render(src);
        assert_well_formed(&svg);
        assert!(svg.contains("viewBox"));
    }
}

#[test]
fn text_never_overflows_the_canvas() {
    let theme = Theme::light();
    let scene = to_scene(KITCHEN_SINK, &theme);
    let metrics = Metrics::new();
    let right_edge = theme.width - theme.padding + 1.0;
    for draw in &scene.draws {
        if let Draw::Text(t) = draw {
            let style = TextStyle {
                size: t.size,
                family: t.family,
                bold: t.bold,
                italic: t.italic,
                letter_spacing: if t.size > 0.0 {
                    t.letter_spacing / t.size
                } else {
                    0.0
                },
            };
            let w = metrics.measure(&t.text, &style);
            assert!(
                t.x + w <= right_edge,
                "text overflows: {:?} x={} w={} (limit {})",
                t.text,
                t.x,
                w,
                right_edge
            );
            assert!(
                t.x >= theme.padding - 1.0,
                "text starts before the margin: {:?}",
                t.text
            );
        }
    }
}

#[test]
fn long_unbroken_tokens_are_split_instead_of_overflowing() {
    let src = "x".repeat(2000);
    let scene = to_scene(&src, &Theme::light());
    let metrics = Metrics::new();
    for draw in &scene.draws {
        if let Draw::Text(t) = draw {
            let w = metrics.measure(&t.text, &TextStyle::new(t.size, t.family));
            assert!(w <= Theme::light().width, "run of {} px is too wide", w);
        }
    }
}

#[test]
fn cjk_paragraphs_wrap_instead_of_clipping() {
    let src = "中文测试".repeat(200);
    let scene = to_scene(&src, &Theme::light());
    let metrics = Metrics::new();
    let content_right = 800.0 - 48.0;
    for draw in &scene.draws {
        if let Draw::Text(t) = draw {
            let w = metrics.measure(&t.text, &TextStyle::new(t.size, t.family));
            assert!(
                t.x + w <= content_right + 1.0,
                "CJK run overflows: x={} w={}",
                t.x,
                w
            );
        }
    }
}

#[test]
fn code_blocks_use_a_monospace_face() {
    let scene = to_scene("```\nlet x = 1;\n```", &Theme::light());
    let code = scene.find_text("let x").expect("code text");
    assert_eq!(code.family, FontKind::Mono);
}

#[test]
fn inline_code_is_never_split_mid_span_when_it_fits() {
    let scene = to_scene("a `foo bar` b", &Theme::light());
    assert!(
        scene.find_text("foo bar").is_some(),
        "inline code should stay on one line"
    );
}

#[test]
fn center_and_right_aligned_cells_are_positioned_differently() {
    let scene = to_scene(
        "| h |\n|:-:|\n| x |\n\n| h |\n|--:|\n| x |",
        &Theme::light(),
    );
    let xs: Vec<f32> = scene
        .draws
        .iter()
        .filter_map(|d| match d {
            Draw::Text(t) if t.text == "x" => Some(t.x),
            _ => None,
        })
        .collect();
    assert_eq!(xs.len(), 2, "expected two body cells");
    assert!(
        xs[1] > xs[0],
        "right-aligned cell should sit further right: {xs:?}"
    );
}

#[test]
fn random_inputs_never_panic_or_produce_broken_svg() {
    let tokens = [
        "#",
        "##",
        "######",
        "text",
        "中文",
        " ",
        "\n",
        "\n\n",
        "*",
        "**",
        "_",
        "__",
        "~~",
        "`",
        "```",
        "~~~",
        "[",
        "]",
        "(",
        ")",
        "!",
        "|",
        "---",
        ">",
        "-",
        "1.",
        "\\",
        "<",
        "&",
        "\t",
        "    ",
        "===",
        "http://x.y",
        "\"",
        "'",
    ];
    let mut rng = Rng(0x5eed_1234_abcd_9876);
    for case in 0..300 {
        let len = (rng.next() % 60) as usize + 1;
        let src: String = (0..len).map(|_| *rng.pick(&tokens)).collect();
        let svg = render(&src);
        assert_well_formed(&svg);
        assert!(svg.starts_with("<svg "), "case {case}: {src:?}");
        // Parsing must also be total on its own.
        let _ = parse(&src);
    }
}

#[test]
fn shipped_demo_renders_cleanly() {
    // Guards the artifact people actually look at: the committed demo and
    // its screenshots are generated from this exact source.
    let src = include_str!("../demo.md");
    let svg = render(src);
    assert_well_formed(&svg);

    // Indentation inside code blocks is content and must survive.
    assert!(svg.contains(">  → parse"), "code indentation was lost");
    assert!(svg.contains(".with_theme"), "code line missing");

    let theme = Theme::light();
    let metrics = Metrics::new();
    let right_edge = theme.width - theme.padding + 1.0;
    for draw in &to_scene(src, &theme).draws {
        if let Draw::Text(t) = draw {
            let style = TextStyle {
                size: t.size,
                family: t.family,
                bold: t.bold,
                italic: t.italic,
                letter_spacing: if t.size > 0.0 {
                    t.letter_spacing / t.size
                } else {
                    0.0
                },
            };
            let w = metrics.measure(&t.text, &style);
            assert!(t.x + w <= right_edge, "demo text overflows: {:?}", t.text);
        }
    }
}

#[test]
fn scene_is_inspectable_without_serializing() {
    let scene = to_scene("# Hi", &Theme::light());
    assert_eq!(scene.width, 800.0);
    assert!(scene.height > 0.0);
    assert!(scene.find_text("Hi").is_some());
    assert!(scene.background.is_some());
}

#[test]
fn transparent_background_is_supported() {
    let theme = Theme {
        background: None,
        ..Theme::light()
    };
    let svg = render_with("x", &Options::default().with_theme(theme)).unwrap();
    assert_well_formed(&svg);
    assert!(!svg.contains("fill=\"#ffffff\""), "{svg}");
}

#[test]
fn custom_theme_colors_reach_the_output() {
    let theme = Theme {
        link: Color::from_hex("#ff5500").unwrap(),
        ..Theme::light()
    };
    let svg = render_with(
        "[x](https://example.com)",
        &Options::default().with_theme(theme),
    )
    .unwrap();
    assert!(svg.contains("#ff5500"), "custom link color missing");
}

// ---------------------------------------------------------------------------
// Long documents: pagination and table of contents
// ---------------------------------------------------------------------------

#[test]
fn pagination_produces_fixed_height_pages() {
    let opts = Options::default().with_page_height(420.0);
    let pages = render_pages(KITCHEN_SINK, &opts).unwrap();
    assert!(pages.len() > 1, "expected several pages");
    for page in &pages {
        assert_well_formed(page);
        let (width, height, _) = canvas(page);
        assert_eq!(width, 800.0);
        assert_eq!(height, 420.0);
    }
}

#[test]
fn pagination_keeps_every_block_on_exactly_one_page() {
    let opts = Options::default().with_page_height(360.0);
    let scenes = to_pages(KITCHEN_SINK, &opts).unwrap();
    let mut paragraphs = 0;
    for scene in &scenes {
        for draw in &scene.draws {
            if let Draw::Text(t) = draw
                && t.text.trim() == "Some"
            {
                paragraphs += 1;
            }
        }
    }
    assert_eq!(
        paragraphs, 1,
        "the intro paragraph must appear exactly once"
    );
}

#[test]
fn paginated_content_stays_inside_the_page() {
    let theme = Theme::light();
    let opts = Options::default().with_page_height(400.0);
    for scene in to_pages(KITCHEN_SINK, &opts).unwrap() {
        for draw in &scene.draws {
            let bottom = match draw {
                Draw::Text(t) => t.y,
                Draw::Rect(r) => r.y + r.h,
                Draw::Line(l) => l.y1.max(l.y2),
            };
            assert!(
                bottom <= theme.padding * 2.0 + 400.0,
                "draw escapes the page at {bottom}"
            );
        }
    }
}

#[test]
fn toc_is_inserted_as_a_nested_outline() {
    let opts = Options::default().with_toc(true).with_toc_title("目录");
    let scene = to_pages("# One\n\n## Two\n\n### Three\n\n# Four", &opts)
        .unwrap()
        .remove(0);
    assert!(scene.find_text("目录").is_some(), "TOC heading missing");
    let one = scene.find_text("One").expect("entry One");
    let two = scene.find_text("Two").expect("entry Two");
    let three = scene.find_text("Three").expect("entry Three");
    assert!(one.x < two.x, "nested entries should be indented");
    assert!(two.x < three.x, "deeper entries should be indented further");
}

#[test]
fn toc_can_be_disabled_and_respects_empty_documents() {
    let plain = render("just text");
    let with_toc = render_with("just text", &Options::default().with_toc(true)).unwrap();
    assert_eq!(plain, with_toc, "no headings means no TOC");
}

#[test]
fn toc_appears_before_the_document_body() {
    let scene = to_pages("# Alpha\n\nBody text", &Options::default().with_toc(true))
        .unwrap()
        .remove(0);
    let toc = scene.find_text("Contents").expect("contents");
    let body = scene.find_text("Body text").expect("body");
    assert!(toc.y < body.y);
}

// ---------------------------------------------------------------------------
// Themes and typography
// ---------------------------------------------------------------------------

#[test]
fn every_preset_renders_the_demo_without_overflow() {
    let metrics = Metrics::new();
    for theme in [Theme::light(), Theme::dark(), Theme::print()] {
        let scene = to_scene(KITCHEN_SINK, &theme);
        assert!(scene.find_text("Heading one").is_some());
        let right = theme.width - theme.padding + 1.0;
        for draw in &scene.draws {
            if let Draw::Text(t) = draw {
                let style = TextStyle {
                    size: t.size,
                    family: t.family,
                    bold: t.bold,
                    italic: t.italic,
                    letter_spacing: if t.size > 0.0 {
                        t.letter_spacing / t.size
                    } else {
                        0.0
                    },
                };
                assert!(
                    t.x + metrics.measure(&t.text, &style) <= right,
                    "{:?} overflows",
                    t.text
                );
            }
        }
    }
}

#[test]
fn print_theme_is_a_visibly_different_design() {
    let light = render_with("# Title\n\nBody", &Options::default()).unwrap();
    let print = render_with(
        "# Title\n\nBody",
        &Options::default().with_theme(Theme::print()),
    )
    .unwrap();
    assert!(
        print.contains("serif"),
        "print theme should use a serif stack"
    );
    assert_ne!(light, print);
    assert_well_formed(&print);
}

#[test]
fn letter_spacing_is_reflected_in_the_svg() {
    let theme = Theme {
        letter_spacing: 0.05,
        ..Theme::light()
    };
    let svg = render_with("tracked text", &Options::default().with_theme(theme)).unwrap();
    assert!(svg.contains("letter-spacing="), "{svg}");
    // Still no overflow.
    let scene = to_scene(
        "tracked text",
        &Theme {
            letter_spacing: 0.05,
            ..Theme::light()
        },
    );
    let t = scene.find_text("tracked").unwrap();
    let m = Metrics::new();
    let w = m.measure(
        &t.text,
        &TextStyle {
            size: t.size,
            family: t.family,
            bold: t.bold,
            italic: t.italic,
            letter_spacing: t.letter_spacing / t.size,
        },
    );
    assert!(t.x + w <= 752.0 + 1.0);
}

#[test]
fn line_height_changes_vertical_rhythm_only() {
    let tight = Theme {
        line_height: 1.2,
        ..Theme::light()
    };
    let loose = Theme {
        line_height: 2.2,
        ..Theme::light()
    };
    let a = to_scene("one\n\ntwo", &tight);
    let b = to_scene("one\n\ntwo", &loose);
    let y = |s: &markdown2svg::Scene| s.find_text("two").unwrap().y;
    assert!(y(&b) > y(&a), "looser line height should push content down");
    let x = |s: &markdown2svg::Scene| s.find_text("two").unwrap().x;
    assert_eq!(x(&a), x(&b));
}

#[test]
fn font_fallback_stacks_include_cjk_families() {
    let theme = Theme::light();
    for family in ["PingFang SC", "Microsoft YaHei", "sans-serif"] {
        assert!(
            theme.sans_family.contains(family),
            "missing {family} in the stack"
        );
    }
    for family in ["Consolas", "monospace"] {
        assert!(
            theme.mono_family.contains(family),
            "missing {family} in the stack"
        );
    }
}

#[test]
fn cjk_and_latin_mix_wraps_within_the_column() {
    let src = "这是一段中英混排 with English words 和中文一起换行，检查是否溢出。".repeat(6);
    let scene = to_scene(&src, &Theme::light());
    let metrics = Metrics::new();
    for draw in &scene.draws {
        if let Draw::Text(t) = draw {
            let w = metrics.measure(&t.text, &TextStyle::new(t.size, t.family));
            assert!(t.x + w <= 752.0 + 1.0, "mixed line overflows: {:?}", t.text);
        }
    }
}

// ---------------------------------------------------------------------------
// Output engines: SVG / PNG / lossless WebP
// ---------------------------------------------------------------------------

use markdown2svg::{OutputFormat, render_bytes, render_bytes_pages};

/// Reads width/height from a PNG `IHDR` chunk.
fn png_size(data: &[u8]) -> (u32, u32) {
    assert_eq!(
        &data[..8],
        &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
    );
    let len = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    assert_eq!(&data[12..16], b"IHDR");
    assert_eq!(len, 13);
    let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
    let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
    (w, h)
}

/// Reads width/height from a WebP `VP8L` header.
fn webp_size(data: &[u8]) -> (u32, u32) {
    assert_eq!(&data[0..4], b"RIFF");
    assert_eq!(&data[8..12], b"WEBP");
    assert_eq!(&data[12..16], b"VP8L");
    let size = u32::from_le_bytes([data[16], data[17], data[18], data[19]]) as usize;
    // RIFF pads odd chunks to an even length.
    assert_eq!(20 + size + (size & 1), data.len(), "VP8L chunk length");
    assert_eq!(data[20], 0x2F, "VP8L signature");
    // 14-bit width-1, 14-bit height-1, LSB-first across 4 bytes.
    let bits = u32::from_le_bytes([data[21], data[22], data[23], data[24]]);
    ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
}

#[test]
fn svg_engine_matches_the_string_api() {
    let opts = Options::default();
    let bytes = render_bytes(KITCHEN_SINK, &opts).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert_eq!(text, render_with(KITCHEN_SINK, &opts).unwrap());
}

#[test]
fn png_output_has_raster_dimensions() {
    let opts = Options::default().with_format(OutputFormat::Png);
    let bytes = render_bytes(KITCHEN_SINK, &opts).unwrap();
    let (w, h) = png_size(&bytes);
    let scene = to_scene(KITCHEN_SINK, &opts.theme);
    assert_eq!(w, scene.width as u32);
    assert_eq!(h, scene.height.round() as u32);
}

#[test]
fn webp_output_has_lossless_dimensions() {
    let opts = Options::default().with_format(OutputFormat::Webp);
    let bytes = render_bytes(KITCHEN_SINK, &opts).unwrap();
    let (w, h) = webp_size(&bytes);
    let scene = to_scene(KITCHEN_SINK, &opts.theme);
    assert_eq!(w, scene.width as u32);
    assert_eq!(h, scene.height.round() as u32);
}

#[test]
fn raster_zoom_scales_the_bitmap() {
    for (format, read) in [
        (OutputFormat::Png, png_size as fn(&[u8]) -> (u32, u32)),
        (OutputFormat::Webp, webp_size as fn(&[u8]) -> (u32, u32)),
    ] {
        let scene = to_scene("# Hi\n\nBody", &Theme::light());
        let base = render_bytes("# Hi\n\nBody", &Options::default().with_format(format)).unwrap();
        let (w1, h1) = read(&base);
        assert_eq!((w1, h1), (scene.width as u32, scene.height.round() as u32));

        let zoomed = render_bytes(
            "# Hi\n\nBody",
            &Options::default().with_format(format).with_zoom(2.0),
        )
        .unwrap();
        let (w2, h2) = read(&zoomed);
        // Both dimensions come from rounding the scaled scene, so the zoomed
        // bitmap is not necessarily exactly twice the rounded first one.
        assert_eq!(
            (w2, h2),
            (
                (scene.width * 2.0).round() as u32,
                (scene.height * 2.0).round() as u32
            )
        );
    }
}

#[test]
fn webp_lossless_is_smaller_than_png() {
    let png = render_bytes(
        KITCHEN_SINK,
        &Options::default().with_format(OutputFormat::Png),
    )
    .unwrap();
    let webp = render_bytes(
        KITCHEN_SINK,
        &Options::default().with_format(OutputFormat::Webp),
    )
    .unwrap();
    assert!(
        webp.len() < png.len(),
        "webp {} bytes should beat png {} bytes",
        webp.len(),
        png.len()
    );
}

#[test]
fn raster_pages_follow_pagination() {
    for format in [OutputFormat::Png, OutputFormat::Webp] {
        let opts = Options::default()
            .with_format(format)
            .with_page_height(420.0);
        let pages = render_bytes_pages(KITCHEN_SINK, &opts).unwrap();
        assert!(pages.len() > 1);
        for page in &pages {
            let (w, h) = match format {
                OutputFormat::Png => png_size(page),
                _ => webp_size(page),
            };
            assert_eq!(w, 800);
            assert_eq!(h, 420);
        }
    }
}

#[test]
fn raster_output_respects_themes() {
    let light = render_bytes("x", &Options::default().with_format(OutputFormat::Png)).unwrap();
    let dark = render_bytes(
        "x",
        &Options::default()
            .with_format(OutputFormat::Png)
            .with_theme(Theme::dark()),
    )
    .unwrap();
    assert_ne!(light, dark);
    assert_eq!(png_size(&light), png_size(&dark));
}

#[test]
fn explicit_font_file_is_used() {
    let set = markdown2svg::font::discover();
    let Some(path) = set.sans.first().or_else(|| set.fallback.first()) else {
        eprintln!("skipping: no system fonts");
        return;
    };
    let opts = Options::default()
        .with_format(OutputFormat::Png)
        .with_font_file(path.to_string_lossy().to_string());
    let bytes = render_bytes("# Font\n\nText", &opts).unwrap();
    assert_eq!(&bytes[..4], &[0x89, b'P', b'N', b'G']);
}

#[test]
fn missing_font_file_is_an_error() {
    let opts = Options::default()
        .with_format(OutputFormat::Png)
        .with_font_file("definitely-not-a-font.ttf");
    assert_eq!(
        render_bytes("x", &opts),
        Err(markdown2svg::RenderError::FontUnavailable)
    );
}

#[test]
fn svg_never_needs_a_font_file() {
    let opts = Options::default().with_font_file("definitely-not-a-font.ttf");
    assert!(
        render_bytes("x", &opts).is_ok(),
        "SVG must ignore font_file"
    );
}

// ---------------------------------------------------------------------------
// Multi-column typography configuration
// ---------------------------------------------------------------------------

#[test]
fn two_column_layout_uses_both_columns() {
    let theme = Theme::light().with_columns(2);
    let scene = to_scene(
        &"Paragraph text that wraps across the column.\n\n".repeat(30),
        &theme,
    );
    let column_width = theme.column_width();
    let mid = theme.padding + column_width;
    let texts: Vec<&markdown2svg::scene::TextDraw> = scene
        .draws
        .iter()
        .filter_map(|d| match d {
            Draw::Text(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t.x < mid), "nothing in column 1");
    assert!(texts.iter().any(|t| t.x >= mid), "nothing in column 2");
}

#[test]
fn two_column_layout_is_shorter_than_one_column() {
    // Short paragraphs stay on one line in a narrow column, so two columns
    // should nearly halve the height.
    let src = "Short line of text.\n\n".repeat(60);
    let single = to_scene(&src, &Theme::light());
    let double = to_scene(&src, &Theme::light().with_columns(2));
    assert!(
        double.height < single.height * 0.65,
        "two columns: {} vs one column: {}",
        double.height,
        single.height
    );
}

#[test]
fn columns_stay_inside_the_canvas() {
    let theme = Theme::light().with_columns(3);
    let scene = to_scene(
        &"# Heading\n\nSome text with **bold** and `code`.\n\n".repeat(25),
        &theme,
    );
    let metrics = Metrics::new();
    for draw in &scene.draws {
        match draw {
            Draw::Text(t) => {
                let style = TextStyle {
                    size: t.size,
                    family: t.family,
                    bold: t.bold,
                    italic: t.italic,
                    letter_spacing: if t.size > 0.0 {
                        t.letter_spacing / t.size
                    } else {
                        0.0
                    },
                };
                let w = metrics.measure(&t.text, &style);
                assert!(
                    t.x + w <= theme.width - theme.padding + 1.0,
                    "text overflows the canvas: {:?}",
                    t.text
                );
            }
            Draw::Rect(r) => assert!(r.x + r.w <= theme.width + 1.0),
            Draw::Line(l) => assert!(l.x1.max(l.x2) <= theme.width - theme.padding + 1.0),
        }
    }
}

#[test]
fn multi_column_pagination_produces_pages() {
    let opts = Options::default()
        .with_theme(Theme::light().with_columns(2))
        .with_page_height(500.0);
    let pages = render_pages(&"Paragraph words here.\n\n".repeat(60), &opts).unwrap();
    assert!(pages.len() > 1);
    for page in &pages {
        assert!(page.contains(r#"height="500""#));
    }
}

#[test]
fn multi_column_raster_output_matches_the_page_size() {
    let opts = Options::default()
        .with_theme(Theme::light().with_columns(2))
        .with_format(OutputFormat::Png);
    let bytes = render_bytes("A short two-column document.", &opts).unwrap();
    let (w, _h) = png_size(&bytes);
    assert_eq!(w, 800);
}

#[test]
fn typography_configuration_reaches_the_output() {
    // Font size, line height and tracking all change the layout.
    let base = to_scene("Text", &Theme::light());
    let bigger = to_scene("Text", &Theme::light().with_font_size(24.0));
    let a = base.find_text("Text").expect("text");
    let b = bigger.find_text("Text").expect("text");
    assert_eq!(b.size, 24.0);
    assert!(b.y > a.y);

    let loose = to_scene("A\n\nB", &Theme::light().with_line_height(2.4));
    let tight = to_scene("A\n\nB", &Theme::light().with_line_height(1.1));
    assert!(loose.height > tight.height);

    let tracked = to_scene("Tracked", &Theme::light().with_letter_spacing(0.1));
    let plain = to_scene("Tracked", &Theme::light());
    let tw = tracked.find_text("Tracked").unwrap().letter_spacing;
    assert!(tw > 0.0);
    assert_eq!(plain.find_text("Tracked").unwrap().letter_spacing, 0.0);
}

#[test]
fn font_family_configuration_reaches_the_svg() {
    let theme = Theme::light().with_font_family("Georgia, 'Times New Roman', serif");
    let svg = render_with("# Hi", &Options::default().with_theme(theme)).unwrap();
    assert!(svg.contains("Georgia"), "{svg}");
}

#[test]
fn invalid_column_counts_are_rejected() {
    let opts = Options::default().with_theme(Theme::light().with_columns(30));
    assert!(matches!(
        render_bytes("x", &opts),
        Err(markdown2svg::RenderError::InvalidColumns(30))
    ));
}

// ---------------------------------------------------------------------------
// Canvas presets and automatic pagination
// ---------------------------------------------------------------------------

#[test]
fn a4_pages_are_letter_sized_and_do_not_clip() {
    let opts = Options::default().with_canvas(CanvasPreset::A4);
    let mut src = String::new();
    // A long list is the classic case that used to overflow a page.
    for i in 0..120 {
        src.push_str(&format!("{i}. list item with a sentence of text\n"));
    }
    src.push_str("\n\n");
    for i in 0..40 {
        src.push_str(&format!("Paragraph {i} after the list.\n\n"));
    }

    let scenes = to_pages(&src, &opts).unwrap();
    assert!(scenes.len() > 1, "A4 must paginate");
    for scene in &scenes {
        assert_eq!(scene.width, 794.0);
        assert_eq!(scene.height, 1123.0);
        for draw in &scene.draws {
            let bottom = match draw {
                Draw::Text(t) => t.y,
                Draw::Rect(r) => r.y + r.h,
                Draw::Line(l) => l.y1.max(l.y2),
            };
            assert!(
                bottom <= scene.height - 20.0,
                "content clipped at the page bottom: {bottom}"
            );
            let top = match draw {
                Draw::Text(t) => t.y - t.size,
                Draw::Rect(r) => r.y,
                Draw::Line(l) => l.y1.min(l.y2),
            };
            assert!(top >= 20.0, "content above the margin: {top}");
        }
    }
}

#[test]
fn a4_keeps_ordered_list_numbering_across_pages() {
    let opts = Options::default().with_canvas(CanvasPreset::A4);
    let src: String = (1..=150).map(|i| format!("{i}. step {i}\n")).collect();
    let scenes = to_pages(&src, &opts).unwrap();
    assert!(scenes.len() > 1);

    let mut numbers = Vec::new();
    for scene in &scenes {
        for draw in &scene.draws {
            if let Draw::Text(t) = draw
                && t.text.ends_with('.')
                && t.text[..t.text.len() - 1]
                    .chars()
                    .all(|c| c.is_ascii_digit())
            {
                numbers.push(t.text.trim_end_matches('.').parse::<u32>().unwrap());
            }
        }
    }
    assert_eq!(numbers.len(), 150, "all items rendered");
    assert_eq!(
        numbers,
        (1..=150).collect::<Vec<_>>(),
        "numbering continues"
    );
}

#[test]
fn mobile_canvas_is_a_single_long_image() {
    let opts = Options::default().with_canvas(CanvasPreset::Mobile);
    let src = "A paragraph of body text for a mobile long image.\n\n".repeat(50);
    let scenes = to_pages(&src, &opts).unwrap();
    assert_eq!(scenes.len(), 1, "long images do not paginate");
    assert_eq!(scenes[0].width, 750.0);
    assert!(scenes[0].height > 750.0 * 2.0, "long image should be tall");
}

#[test]
fn a4_raster_pages_have_a4_dimensions() {
    let opts = Options::default()
        .with_canvas(CanvasPreset::A4)
        .with_format(OutputFormat::Png);
    let pages = render_bytes_pages(&"Text.\n\n".repeat(80), &opts).unwrap();
    assert!(pages.len() > 1);
    for page in &pages {
        assert_eq!(png_size(page), (794, 1123));
    }
}

#[test]
fn canvas_preset_accepts_explicit_overrides() {
    let opts = Options::default().with_canvas(CanvasPreset::A4);
    let mut overridden = opts.clone();
    overridden.theme.width = 900.0;
    assert!(overridden.validate().is_ok());
    let scene = to_scene("x", &overridden.theme);
    assert_eq!(scene.width, 900.0);
    // Pagination still comes from the preset.
    assert_eq!(overridden.page_height, Some(1123.0));
}
