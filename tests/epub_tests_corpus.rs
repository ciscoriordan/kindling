//! w3c/epub-tests corpus harness.
//!
//! Runs every test EPUB in a local checkout of
//! [w3c/epub-tests](https://github.com/w3c/epub-tests) through
//! `kindling::validate::validate_opf` and prints a category-level summary
//! plus a rule-frequency histogram. Gated on the `KINDLING_CORPUS_DIR`
//! environment variable and `#[ignore]` so regular `cargo test` skips it.
//!
//! # Running
//!
//! ```text
//! KINDLING_CORPUS_DIR=~/Documents/epub-tests \
//!   cargo test --release --test epub_tests_corpus -- --ignored --nocapture
//! ```
//!
//! A JSON report is written to `target/epub_tests_report.json` for
//! programmatic comparison across runs.
//!
//! # Baseline interpretation
//!
//! The w3c corpus is an EPUB 3 reading-system conformance suite, not a
//! validator suite. Most tests are valid EPUB 3 that exercise reading-system
//! behaviors Kindle does not share (scripting, MathML, fixed-layout variants,
//! audio / video, etc.). Kindling is Kindle-oriented, so several rules will
//! fire on almost every test and are treated as expected noise rather than
//! real false positives:
//!
//! - `R4.1.1` info: marketing cover must be uploaded separately to KDP
//! - `R4.2.1` error: no internal `coverimage` manifest entry
//! - `R5.2.1` warning: no NCX (EPUB 3 uses nav-only by default)
//!
//! The "beyond noise" column in the per-category table counts tests that
//! fire at least one rule outside this set. That is where the signal lives.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use kindling::validate::{Finding, validate_opf};
use kindling::kdp_rules::Severity;

const EXPECTED_CORPUS_SHA: &str = "45feac979d9b12b502f124db7bc5056977628417";

const KINDLE_NOISE_IDS: &[&str] = &["R4.1.1", "R4.2.1", "R5.2.1"];

fn is_noise(rule_id: &str) -> bool {
    KINDLE_NOISE_IDS.contains(&rule_id)
}

struct TestRow {
    test_id: String,
    category: String,
    outcome: Outcome,
}

enum Outcome {
    Validated {
        errors: usize,
        warnings: usize,
        rule_ids: Vec<&'static str>,
    },
    ParseFailure(String),
}

#[derive(Default)]
struct CategoryStats {
    total: usize,
    clean: usize,
    noise_only: usize,
    beyond_noise: usize,
    parse_failures: usize,
}

fn category_of(test_id: &str) -> &str {
    test_id.split('-').next().unwrap_or("other")
}

fn read_subdirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(iter) = fs::read_dir(root) else { return out };
    for entry in iter.flatten() {
        let p = entry.path();
        if p.is_dir() {
            out.push(p);
        }
    }
    out.sort();
    out
}

fn run_one(opf_path: &Path) -> Outcome {
    match validate_opf(opf_path) {
        Ok(report) => {
            let errors = report.error_count();
            let warnings = report.warning_count();
            let mut rule_ids: Vec<&'static str> = report
                .findings
                .iter()
                .filter_map(|f: &Finding| f.rule_id)
                .collect();
            rule_ids.sort_unstable();
            rule_ids.dedup();
            Outcome::Validated { errors, warnings, rule_ids }
        }
        Err(e) => Outcome::ParseFailure(format!("{e}")),
    }
}

#[test]
#[ignore = "corpus harness: set KINDLING_CORPUS_DIR, run with `cargo test --release --test epub_tests_corpus -- --ignored --nocapture`"]
fn epub_tests_corpus_baseline() {
    let corpus = std::env::var("KINDLING_CORPUS_DIR")
        .expect("set KINDLING_CORPUS_DIR to a local w3c/epub-tests checkout");
    let corpus_root = PathBuf::from(&corpus).join("tests");
    assert!(
        corpus_root.exists(),
        "corpus tests dir does not exist: {}",
        corpus_root.display()
    );

    // Check that the corpus checkout matches the pinned SHA.
    if let Ok(output) = std::process::Command::new("git")
        .args(["-C", &corpus, "rev-parse", "HEAD"])
        .output()
    {
        let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if actual != EXPECTED_CORPUS_SHA {
            eprintln!(
                "WARNING: corpus is at {actual}, expected {EXPECTED_CORPUS_SHA}. Results may differ from baseline."
            );
        }
    }

    let mut rows: Vec<TestRow> = Vec::new();
    for test_dir in read_subdirs(&corpus_root) {
        let Some(test_id) = test_dir.file_name().map(|s| s.to_string_lossy().into_owned())
        else {
            continue;
        };
        // Template directories (e.g. xx-epub-template) are scaffolding, not tests.
        if test_id.starts_with("xx-") {
            continue;
        }
        let opf_path = test_dir.join("EPUB").join("package.opf");
        if !opf_path.exists() {
            continue;
        }
        let category = category_of(&test_id).to_string();
        let outcome = run_one(&opf_path);
        rows.push(TestRow { test_id, category, outcome });
    }

    // Aggregate per category.
    let mut by_category: BTreeMap<String, CategoryStats> = BTreeMap::new();
    for r in &rows {
        let stats = by_category.entry(r.category.clone()).or_default();
        stats.total += 1;
        match &r.outcome {
            Outcome::Validated { rule_ids, .. } => {
                let beyond = rule_ids.iter().any(|id| !is_noise(id));
                let only_noise = !beyond && !rule_ids.is_empty();
                let clean = rule_ids.is_empty();
                if clean {
                    stats.clean += 1;
                } else if only_noise {
                    stats.noise_only += 1;
                } else {
                    stats.beyond_noise += 1;
                }
            }
            Outcome::ParseFailure(_) => {
                stats.parse_failures += 1;
            }
        }
    }

    // Rule-id histogram across all tests, counting "test hit", not per-finding.
    let mut rule_tests_hit: BTreeMap<&str, usize> = BTreeMap::new();
    for r in &rows {
        if let Outcome::Validated { rule_ids, .. } = &r.outcome {
            for id in rule_ids {
                *rule_tests_hit.entry(id).or_default() += 1;
            }
        }
    }

    // Top "beyond-noise" tests, ranked by number of distinct non-noise rules fired.
    let mut beyond_noise_tests: Vec<(&str, Vec<&str>)> = rows
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::Validated { rule_ids, .. } => {
                let hits: Vec<&str> = rule_ids
                    .iter()
                    .filter(|id| !is_noise(id))
                    .copied()
                    .collect();
                if hits.is_empty() {
                    None
                } else {
                    Some((r.test_id.as_str(), hits))
                }
            }
            _ => None,
        })
        .collect();
    beyond_noise_tests.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    let parse_failures: Vec<(&str, &str)> = rows
        .iter()
        .filter_map(|r| match &r.outcome {
            Outcome::ParseFailure(msg) => Some((r.test_id.as_str(), msg.as_str())),
            _ => None,
        })
        .collect();

    // Text report.
    println!();
    println!("=== kindling v{} against w3c/epub-tests corpus ===", env!("CARGO_PKG_VERSION"));
    println!("corpus: {}", corpus);
    println!("tests considered: {}", rows.len());
    println!("Kindle-expected noise rules (filtered from 'beyond-noise' column): {:?}", KINDLE_NOISE_IDS);
    println!();

    println!(
        "{:<6} {:>6} {:>7} {:>12} {:>14} {:>14}",
        "cat", "total", "clean", "noise_only", "beyond_noise", "parse_failures"
    );
    println!("{}", "-".repeat(63));
    for (cat, stats) in &by_category {
        println!(
            "{:<6} {:>6} {:>7} {:>12} {:>14} {:>14}",
            cat,
            stats.total,
            stats.clean,
            stats.noise_only,
            stats.beyond_noise,
            stats.parse_failures,
        );
    }
    let total: usize = by_category.values().map(|s| s.total).sum();
    let clean: usize = by_category.values().map(|s| s.clean).sum();
    let noise_only: usize = by_category.values().map(|s| s.noise_only).sum();
    let beyond_noise: usize = by_category.values().map(|s| s.beyond_noise).sum();
    let parse_failures_count: usize = by_category.values().map(|s| s.parse_failures).sum();
    println!("{}", "-".repeat(63));
    println!(
        "{:<6} {:>6} {:>7} {:>12} {:>14} {:>14}",
        "all", total, clean, noise_only, beyond_noise, parse_failures_count
    );

    println!();
    println!("top rule hits (distinct tests fired on, descending):");
    let mut rule_hits_vec: Vec<(&&str, &usize)> = rule_tests_hit.iter().collect();
    rule_hits_vec.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (id, n) in rule_hits_vec.iter().take(20) {
        let marker = if is_noise(id) { " (noise)" } else { "" };
        println!("  {:>8} x{:<4}{}", id, n, marker);
    }

    println!();
    println!("top beyond-noise tests (most distinct non-noise rules fired):");
    for (test_id, hits) in beyond_noise_tests.iter().take(15) {
        println!("  {:<50} {}", test_id, hits.join(", "));
    }

    if !parse_failures.is_empty() {
        println!();
        println!("parse failures ({}):", parse_failures.len());
        for (test_id, msg) in &parse_failures {
            println!("  {}: {}", test_id, msg);
        }
    }

    // JSON report next to the target dir.
    let out_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("epub_tests_report.json");
    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let json = build_json(&rows, &by_category, &rule_tests_hit, corpus.as_str());
    fs::write(&out_path, json).expect("write epub_tests_report.json");
    println!();
    println!("wrote {}", out_path.display());

    // Sanity assertion: we should have actually looked at something.
    assert!(rows.len() > 100, "corpus walk produced only {} tests; is KINDLING_CORPUS_DIR pointing at a w3c/epub-tests checkout?", rows.len());
}

fn build_json(
    rows: &[TestRow],
    by_category: &BTreeMap<String, CategoryStats>,
    rule_tests_hit: &BTreeMap<&str, usize>,
    corpus: &str,
) -> String {
    let mut s = String::new();
    s.push_str("{\n");
    s.push_str(&format!("  \"kindling_version\": \"{}\",\n", env!("CARGO_PKG_VERSION")));
    s.push_str(&format!("  \"corpus\": {},\n", json_str(corpus)));
    s.push_str(&format!("  \"corpus_sha\": {},\n", json_str(EXPECTED_CORPUS_SHA)));
    s.push_str(&format!("  \"total_tests\": {},\n", rows.len()));
    s.push_str("  \"noise_ids\": [");
    for (i, id) in KINDLE_NOISE_IDS.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        s.push_str(&json_str(id));
    }
    s.push_str("],\n");

    // categories
    s.push_str("  \"categories\": {\n");
    let mut first = true;
    for (cat, stats) in by_category {
        if !first {
            s.push_str(",\n");
        }
        first = false;
        s.push_str(&format!(
            "    {}: {{\"total\": {}, \"clean\": {}, \"noise_only\": {}, \"beyond_noise\": {}, \"parse_failures\": {}}}",
            json_str(cat),
            stats.total,
            stats.clean,
            stats.noise_only,
            stats.beyond_noise,
            stats.parse_failures
        ));
    }
    s.push_str("\n  },\n");

    // rule_hits
    s.push_str("  \"rule_hits\": {\n");
    let mut first = true;
    let mut sorted: Vec<(&&str, &usize)> = rule_tests_hit.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    for (id, n) in &sorted {
        if !first {
            s.push_str(",\n");
        }
        first = false;
        s.push_str(&format!("    {}: {}", json_str(id), n));
    }
    s.push_str("\n  },\n");

    // per-test rows
    s.push_str("  \"tests\": [\n");
    let mut first = true;
    for r in rows {
        if !first {
            s.push_str(",\n");
        }
        first = false;
        s.push_str("    {");
        s.push_str(&format!("\"id\": {}, \"category\": {}, ", json_str(&r.test_id), json_str(&r.category)));
        match &r.outcome {
            Outcome::Validated { errors, warnings, rule_ids } => {
                s.push_str(&format!(
                    "\"parsed\": true, \"errors\": {}, \"warnings\": {}, \"rules\": [",
                    errors, warnings
                ));
                for (i, id) in rule_ids.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&json_str(id));
                }
                s.push(']');
            }
            Outcome::ParseFailure(msg) => {
                s.push_str(&format!(
                    "\"parsed\": false, \"parse_error\": {}",
                    json_str(msg)
                ));
            }
        }
        s.push('}');
    }
    s.push_str("\n  ]\n");
    s.push_str("}\n");
    s
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// Silence "unused" on the Severity re-export; we may want to filter by level
// later and keeping the import pins the public API shape.
#[allow(dead_code)]
fn _pin_severity(_: Severity) {}
