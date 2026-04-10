//! End-to-end CI smoke test for the `kindling validate` subcommand.
//!
//! Unlike the unit tests in `src/tests.rs` (which call `validate_opf()`
//! directly with in-memory OPFs), this test runs the *compiled*
//! `kindling-cli` binary against real fixture OPF files on disk. It
//! catches regressions in CLI wiring, argument parsing, stdout format,
//! and exit codes that unit tests would miss.
//!
//! Fixtures live under `tests/fixtures/`:
//!
//! * `clean_book/`          - minimal valid book OPF: cover + NCX + HTML
//! * `clean_dict/`          - minimal valid dictionary OPF with cover,
//!                            usage, content and copyright HTML files
//! * `book_with_warnings/`  - valid OPF whose only finding is R4.2.3
//!                            (cover image too small); used to exercise
//!                            `--strict` mode
//! * `book_with_errors/`    - deliberately broken book OPF that triggers
//!                            R4.2.1 (missing cover), R6.3 (script tag),
//!                            R6.4 (nested `<p>`) and R17.1 (unsupported
//!                            `<script>` tag)
//!
//! Cargo builds the binary automatically before running integration
//! tests and exposes its path via `CARGO_BIN_EXE_kindling-cli`, so this
//! file has no build-script dependency.

use std::path::PathBuf;
use std::process::{Command, Output};

/// Path to the `kindling-cli` binary, resolved at compile time by Cargo.
fn kindling_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kindling-cli")
}

/// Absolute path to `tests/fixtures/<name>`.
fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Run `kindling-cli validate <args...>` and return the full `Output`.
/// Panics if the binary cannot be spawned.
fn run_validate(args: &[&str]) -> Output {
    Command::new(kindling_bin())
        .arg("validate")
        .args(args)
        .output()
        .expect("failed to spawn kindling-cli validate")
}

/// Pretty-print stdout+stderr on failure.
fn dump(output: &Output) -> String {
    format!(
        "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

// ---------------------------------------------------------------------------
// Clean fixtures: should exit 0 with no error findings
// ---------------------------------------------------------------------------

#[test]
fn validate_clean_book_exits_zero() {
    let opf = fixture_dir("clean_book").join("clean_book.opf");
    let out = run_validate(&[opf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "clean_book should validate cleanly\n{}",
        dump(&out)
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "clean_book should exit 0\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("Kindle Publishing Guidelines v"),
        "header missing KPG version line\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("0 errors"),
        "clean_book should have 0 errors\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("0 warnings"),
        "clean_book should have 0 warnings\n{}",
        dump(&out)
    );
}

#[test]
fn validate_clean_dict_exits_zero() {
    let opf = fixture_dir("clean_dict").join("clean_dict.opf");
    let out = run_validate(&[opf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        out.status.success(),
        "clean_dict should validate cleanly\n{}",
        dump(&out)
    );
    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout.contains("Kindle Publishing Guidelines v"),
        "header missing KPG version line\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("0 errors"),
        "clean_dict should have 0 errors\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("0 warnings"),
        "clean_dict should have 0 warnings\n{}",
        dump(&out)
    );
}

#[test]
fn validate_clean_book_strict_still_exits_zero() {
    // --strict should not flip a clean fixture to non-zero.
    let opf = fixture_dir("clean_book").join("clean_book.opf");
    let out = run_validate(&["--strict", opf.to_str().unwrap()]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "clean_book --strict should still exit 0\n{}",
        dump(&out)
    );
}

// ---------------------------------------------------------------------------
// Error fixture: must flag the rules it was constructed to trigger
// ---------------------------------------------------------------------------

#[test]
fn validate_book_with_errors_exits_one() {
    let opf = fixture_dir("book_with_errors").join("book_with_errors.opf");
    let out = run_validate(&[opf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        out.status.code(),
        Some(1),
        "book_with_errors should exit 1\n{}",
        dump(&out)
    );
    assert!(
        !stdout.contains("0 errors"),
        "book_with_errors should report at least one error\n{}",
        dump(&out)
    );

    // Each rule id below corresponds to a deliberately-planted defect in
    // the fixture. Keep these assertions explicit so the test fails loudly
    // if a rule id is renumbered or a check is accidentally removed.
    for rule_id in &["R4.2.1", "R6.3", "R6.4", "R17.1"] {
        assert!(
            stdout.contains(rule_id),
            "expected rule id {} in output\n{}",
            rule_id,
            dump(&out)
        );
    }
}

// ---------------------------------------------------------------------------
// --strict mode: warnings-only fixture
// ---------------------------------------------------------------------------

#[test]
fn validate_book_with_warnings_default_mode_exits_zero() {
    // Without --strict a warning alone should not fail the run.
    let opf = fixture_dir("book_with_warnings").join("book_with_warnings.opf");
    let out = run_validate(&[opf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        out.status.code(),
        Some(0),
        "warnings-only fixture should exit 0 without --strict\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("R4.2.3"),
        "expected the cover-too-small warning R4.2.3 in output\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("0 errors"),
        "warnings-only fixture should report 0 errors\n{}",
        dump(&out)
    );
}

#[test]
fn validate_book_with_warnings_strict_exits_one() {
    let opf = fixture_dir("book_with_warnings").join("book_with_warnings.opf");
    let out = run_validate(&["--strict", opf.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(
        out.status.code(),
        Some(1),
        "--strict should promote warnings to a non-zero exit\n{}",
        dump(&out)
    );
    assert!(
        stdout.contains("R4.2.3"),
        "expected R4.2.3 in --strict output\n{}",
        dump(&out)
    );
}
