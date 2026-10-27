//! Opens a generated document in Microsoft Word and checks its structure.
//!
//! Word is the oracle for a `.docx`: a package our own reader accepts
//! can still be one Word refuses to open. The test skips itself when
//! Word is not installed, so it never fails a build on another machine.

use std::path::PathBuf;
use std::process::Command;

use markpop::{Options, render_docx};

/// A document with a heading, a list, a table and a link.
const DOCUMENT: &str = r#"
# Title

Some text with a [link](https://example.com).

## Section

- one
- two

| a | b |
|---|---|
| 1 | 2 |
"#;

/// Whether Word can be driven on this machine.
fn word_available() -> bool {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "try { $w = New-Object -ComObject Word.Application; $w.Quit(); exit 0 } catch { exit 1 }",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Runs the probe script against a document and returns its report.
fn probe(path: &PathBuf) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/probe-word.ps1");
    let report = path.with_extension("report.txt");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(path)
        .arg(&report)
        .output()
        .expect("run the probe");
    let report_body = std::fs::read_to_string(&report).unwrap_or_default();
    let _ = std::fs::remove_file(&report);
    assert!(
        output.status.success(),
        "the probe failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    report_body
}

/// Reads a `KEY=VALUE` line out of a report.
fn field(report: &str, key: &str) -> String {
    report
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .unwrap_or_default()
        .to_string()
}

/// Reads a number out of a report.
fn number(report: &str, key: &str) -> u32 {
    field(report, key).parse().unwrap_or(0)
}

#[test]
fn word_opens_the_document_and_finds_its_structure() {
    if !word_available() {
        eprintln!("skipping: Microsoft Word is not installed");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("markpop-word-{}.docx", std::process::id()));
    let bytes = render_docx(DOCUMENT, &Options::default()).expect("render");
    std::fs::write(&path, bytes).expect("write");

    let report = probe(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        field(&report, "ERROR").is_empty(),
        "Word could not open the document: {report}"
    );
    assert_eq!(field(&report, "OPENED"), "ok", "{report}");
    assert!(number(&report, "PARAGRAPHS") > 5, "{report}");
    assert!(number(&report, "TABLES") >= 1, "no table: {report}");
    assert!(number(&report, "LISTS") >= 1, "no list: {report}");
    assert!(number(&report, "HYPERLINKS") >= 1, "no link: {report}");
    assert!(
        field(&report, "TABLE1A1").starts_with('a'),
        "unexpected first cell: {report}"
    );
    assert!(
        field(&report, "TEXT").contains("Title"),
        "no text: {report}"
    );
}

#[test]
fn word_builds_the_table_of_contents() {
    if !word_available() {
        eprintln!("skipping: Microsoft Word is not installed");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("markpop-word-toc-{}.docx", std::process::id()));
    let options = Options::default().with_toc(true);
    let bytes = render_docx(DOCUMENT, &options).expect("render");
    std::fs::write(&path, bytes).expect("write");

    let report = probe(&path);
    let _ = std::fs::remove_file(&path);

    assert_eq!(field(&report, "OPENED"), "ok", "{report}");
    assert!(
        field(&report, "FIELD1").contains("TOC"),
        "no table of contents field: {report}"
    );
}
