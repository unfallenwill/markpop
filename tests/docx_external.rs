//! Checks the generated package with independent tools.
//!
//! MarkPop's own reader and Microsoft's Word are two witnesses; the .NET
//! XML parser and the .NET ZIP reader are two more, and they are not
//! written by us. A bug our own reader and writer agree on cannot hide
//! from them.

use std::path::PathBuf;
use std::process::Command;

use markpop::{Options, render_docx};

/// A document with the constructs that make XML interesting.
const DOCUMENT: &str = r#"
# Title & <friends>

Text with a [link](https://example.com/?a=1&b=2), `code`, and 中文.

| a | b |
|---|---|
| 1 | 2 |

> quoted

- one
- two
"#;

/// Whether PowerShell can be run here.
fn powershell_available() -> bool {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", "exit 0"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Runs the verification script and returns its report.
fn verify(path: &PathBuf) -> String {
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-docx.ps1");
    let report = path.with_extension("report.txt");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg(path)
        .arg(&report)
        .output()
        .expect("run the verifier");
    let body = std::fs::read_to_string(&report).unwrap_or_default();
    let _ = std::fs::remove_file(&report);
    assert!(
        output.status.success(),
        "the verifier failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    body
}

#[test]
fn the_package_is_well_formed_for_dotnet() {
    if !powershell_available() {
        eprintln!("skipping: no PowerShell");
        return;
    }
    let mut path = std::env::temp_dir();
    path.push(format!("markpop-external-{}.docx", std::process::id()));
    let bytes = render_docx(DOCUMENT, &Options::default()).expect("render");
    std::fs::write(&path, bytes).expect("write");

    let report = verify(&path);
    let _ = std::fs::remove_file(&path);

    assert!(
        !report.contains("ERROR="),
        "the .NET tools rejected the package: {report}"
    );
    assert!(report.contains("VERIFIED=ok"), "{report}");
    for part in [
        "word/document.xml",
        "word/styles.xml",
        "word/numbering.xml",
        "word/settings.xml",
        "[Content_Types].xml",
        "_rels/.rels",
        "word/_rels/document.xml.rels",
    ] {
        assert!(
            report.contains(&format!("XML_OK={part}")),
            "{part} was not checked: {report}"
        );
    }
}
