//! CommonMark conformance harness.
//!
//! The official specification ships 652 example pairs (Markdown → expected
//! HTML). Each example is parsed with our parser and re-serialized to HTML by
//! [`support`]; per-section pass rates are printed and each section has a
//! floor, so a change cannot silently lose ground.
//!
//! Raw HTML is kept as data (rendered literally in SVG, never executed) and
//! images are drawn as figures; their examples still have to produce the
//! right structure, which the serializer checks.

#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use support::{compare, examples};

const SPEC: &str = include_str!("data/commonmark-0.31.2.txt");

/// The lowest pass rate each section must keep.
const FLOORS: &[(&str, f32)] = &[
    ("Tabs", 1.00),
    ("Backslash escapes", 1.00),
    ("Entity and numeric character references", 1.00),
    ("Precedence", 1.00),
    ("Thematic breaks", 1.00),
    ("ATX headings", 1.00),
    ("Setext headings", 1.00),
    ("Indented code blocks", 1.00),
    ("Fenced code blocks", 1.00),
    ("HTML blocks", 1.00),
    ("Link reference definitions", 1.00),
    ("Paragraphs", 1.00),
    ("Blank lines", 1.00),
    ("Block quotes", 1.00),
    ("List items", 1.00),
    ("Lists", 1.00),
    ("Inlines", 1.00),
    ("Code spans", 1.00),
    ("Emphasis and strong emphasis", 1.00),
    ("Links", 1.00),
    ("Images", 1.00),
    ("Autolinks", 1.00),
    ("Raw HTML", 1.00),
    ("Hard line breaks", 1.00),
    ("Soft line breaks", 1.00),
    ("Textual content", 1.00),
];

#[test]
fn commonmark_conformance() {
    let all = examples(SPEC);
    assert_eq!(all.len(), 652, "the spec fixture should hold every example");
    let mut stats: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let verbose = std::env::var_os("COMMONMARK_FAILURES").is_some();
    let mut shown: BTreeMap<&str, usize> = BTreeMap::new();
    for example in &all {
        let entry = stats.entry(example.section.as_str()).or_insert((0, 0));
        match compare(example) {
            Ok(()) => entry.0 += 1,
            Err(message) => {
                if failures.len() < 40 {
                    failures.push(message.clone());
                }
                let count = shown.entry(example.section.as_str()).or_insert(0);
                if verbose && *count < 6 {
                    *count += 1;
                    eprintln!("\n{message}");
                }
            }
        }
        entry.1 += 1;
    }

    let (mut passed, mut total) = (0, 0);
    eprintln!("\nCommonMark 0.31.2 conformance:");
    for (section, (ok, count)) in &stats {
        passed += ok;
        total += count;
        eprintln!(
            "  {:>4}/{:<4} {:>5.1}%  {section}",
            ok,
            count,
            *ok as f32 * 100.0 / *count as f32
        );
        let floor = FLOORS
            .iter()
            .find(|(name, _)| name == section)
            .map(|(_, floor)| *floor)
            .unwrap_or(0.0);
        let rate = *ok as f32 / *count as f32;
        assert!(
            rate >= floor,
            "section `{section}` dropped to {:.0}% (floor {:.0}%)\n{}",
            rate * 100.0,
            floor * 100.0,
            failures.join("\n\n")
        );
    }
    eprintln!(
        "  total {}/{} ({:.1}%)\n",
        passed,
        total,
        passed as f32 * 100.0 / total as f32
    );
    assert_eq!(passed, 652, "every CommonMark example must pass");
}
