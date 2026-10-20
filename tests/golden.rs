//! Golden-file test.
//!
//! Locks the exact bytes of a small rendered document. This is the strongest
//! form of the determinism guarantee: the same input must produce the same
//! numbers, on every platform, in every build.
//!
//! To regenerate after an intentional change:
//!
//! ```bash
//! cargo run --quiet -- tests/golden/basic.md -o tests/golden/basic.svg
//! ```

const SOURCE: &str = include_str!("golden/basic.md");
const EXPECTED: &str = include_str!("golden/basic.svg");

#[test]
fn output_matches_the_committed_golden_file() {
    // `.gitattributes` pins LF, but tolerate a CRLF checkout anyway: this
    // assertion is about geometry and markup, not line endings.
    let expected = EXPECTED.replace("\r\n", "\n");
    let actual = markdown2svg::render(SOURCE);
    assert_eq!(
        actual, expected,
        "\nSVG output changed.\n\
         If the change is intentional, regenerate the golden file:\n  \
         cargo run --quiet -- tests/golden/basic.md -o tests/golden/basic.svg\n"
    );
}

#[test]
fn golden_output_is_zoom_independent_geometry() {
    let base = markdown2svg::render(SOURCE);
    let zoomed =
        markdown2svg::render_with(SOURCE, &markdown2svg::Options::default().with_zoom(2.0))
            .unwrap();
    // Same elements, different canvas size.
    assert_eq!(base.lines().count(), zoomed.lines().count());
    assert!(zoomed.contains(r#"width="1600""#));
    assert!(zoomed.contains(r#"viewBox="0 0 800"#));
}
