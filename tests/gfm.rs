//! GitHub Flavored Markdown extension conformance.
//!
//! GFM 0.29 extends CommonMark with tables, strikethrough, autolink
//! literals, task list items and a raw-HTML tag filter. The core language is
//! covered by `commonmark.rs`; this harness checks the extensions.

#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use support::{compare_gfm, examples};

const SPEC: &str = include_str!("data/gfm-0.29.txt");

/// The lowest pass rate each extension must keep.
const FLOORS: &[(&str, f32)] = &[
    ("Tables (extension)", 1.00),
    ("Strikethrough (extension)", 1.00),
    ("Autolinks (extension)", 1.00),
    ("Task list items (extension)", 1.00),
    ("Disallowed Raw HTML (extension)", 1.00),
];

#[test]
fn gfm_conformance() {
    let all = examples(SPEC);
    assert_eq!(all.len(), 24, "the GFM fixture should hold every example");
    let mut stats: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let verbose = std::env::var_os("GFM_FAILURES").is_some();
    for example in &all {
        let entry = stats.entry(example.section.as_str()).or_insert((0, 0));
        match compare_gfm(example) {
            Ok(()) => entry.0 += 1,
            Err(message) => {
                if failures.len() < 24 {
                    failures.push(message.clone());
                }
                if verbose {
                    eprintln!("\n{message}");
                }
            }
        }
        entry.1 += 1;
    }

    let (mut passed, mut total) = (0, 0);
    eprintln!("\nGFM 0.29 extension conformance:");
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
}
