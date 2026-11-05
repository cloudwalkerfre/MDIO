//! Regenerate `mdio_core/tests/unit/ledger_report.txt`.
//!
//! The fixture is the same source as `ledger_report_fixture_is_current`:
//! `trinity::obs::ledger_report_lines()` (feature `trinity-obs`). Keeping the
//! generator here instead of a shell pipeline makes `just regen-ledger-report`
//! actually executable and byte-identical with the test's expectation.

fn main() {
    let lines = mdio_core::trinity::obs::ledger_report_lines();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/unit/ledger_report.txt");
    let body = format!("{}\n", lines.join("\n"));
    std::fs::write(&path, body)
        .unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!(
        "wrote {} ({} lines)",
        path.display(),
        lines.len()
    );
}
