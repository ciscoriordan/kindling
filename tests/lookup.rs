//! End-to-end tests for the on-device lookup simulator (`kindling::lookup`).
//!
//! Each test builds a committed language fixture with kindling and asserts the
//! simulator resolves (or fails to resolve) the same forms the firmware would.
//! This exercises all three collation modes and, crucially, shows the
//! simulator discriminates the issue #17 alias fix from its absence: with the
//! Cyrillic aliases a lowercase query resolves, and under `--strict-accents`
//! (which suppresses them) the very same query misses, exactly the on-device
//! behavior BoboTiG reported.

mod common;

use std::path::Path;
use std::process::Command;

use common::{kindling_bin, kindling_build};
use kindling::lookup::{lookup, report};

fn build_ru_strict(out: &Path) {
    let opf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/langs/ru/src/dict.opf");
    let status = Command::new(kindling_bin())
        .arg("build")
        .arg(&opf)
        .arg("-o")
        .arg(out)
        .arg("--no-validate")
        .env("KINDLING_STRICT_ACCENTS", "1")
        .output()
        .expect("spawn kindling-cli");
    assert!(
        status.status.success(),
        "strict build failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("kindling_lookup_test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn build(code: &str, out: &Path) {
    let opf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/langs")
        .join(code)
        .join("src/dict.opf");
    kindling_build(&opf, out);
}

#[test]
fn russian_alias_resolves_lowercase_headword() {
    // The ru fixture has the mixed-case headword "Москва"; issue #17 adds the
    // lowercased "москва" as an alias pointing at the same entry.
    let out = tmp("ru.mobi");
    build("ru", &out);
    let data = std::fs::read(&out).unwrap();

    let upper = lookup(&data, "Москва").expect("headword resolves");
    let lower = lookup(&data, "москва").expect("lowercase alias resolves");
    assert_eq!(
        upper.position, lower.position,
        "alias must point at the same entry as the headword"
    );
    // An uppercased query for a lowercase headword resolves via query folding.
    assert!(lookup(&data, "ВОДА").is_some(), "вода reachable from ВОДА");
    assert!(lookup(&data, "несуществующее").is_none(), "nonsense misses");
}

#[test]
fn russian_strict_accents_suppresses_alias() {
    // Same fixture, built with --strict-accents: no aliases, so the lowercase
    // form of the uppercase-initial headword no longer resolves, while the
    // headword itself still does. This is the pre-issue-#17 behavior and the
    // proof the simulator is a real regression test, not a rubber stamp.
    let out = tmp("ru-strict.mobi");
    build_ru_strict(&out);
    let data = std::fs::read(&out).unwrap();

    assert!(lookup(&data, "Москва").is_some(), "headword still resolves");
    assert!(
        lookup(&data, "москва").is_none(),
        "without the alias, the lowercase form must miss"
    );
}

#[test]
fn french_accent_and_case_fold() {
    // Latin exact-accent default (generated ORDT labels): an exact accented
    // query returns itself, an unaccented or uppercased query folds to the
    // accented headword, and a non-headword misses.
    let out = tmp("fr.mobi");
    build("fr", &out);
    let data = std::fs::read(&out).unwrap();

    let exact = lookup(&data, "rivière").expect("exact accented headword");
    assert_eq!(exact.matched_label, "rivière");
    let folded = lookup(&data, "riviere").expect("unaccented folds to accented");
    assert_eq!(folded.matched_label, "rivière");
    let cased = lookup(&data, "RIVIÈRE").expect("uppercase folds to headword");
    assert_eq!(cased.matched_label, "rivière");
    assert!(lookup(&data, "zzzzz").is_none(), "non-headword misses");
}

#[test]
fn japanese_literal_match() {
    // Generated ORDT literal path: a kanji headword resolves by code point.
    let out = tmp("ja.mobi");
    build("ja", &out);
    let data = std::fs::read(&out).unwrap();
    assert!(lookup(&data, "水").is_some(), "kanji headword resolves");
    assert!(
        lookup(&data, "存在しない語").is_none(),
        "non-headword misses"
    );
}

/// The simulator is used on kindling's own output all day; this is the only
/// test that points it at kindlegen's. It also pins the index-pointer
/// behavior from issue #49: on a well-formed dictionary the record the header
/// names must be the record the index is in, so the recovery path stays a
/// recovery path and does not quietly become the normal one.
#[test]
fn kindlegen_reference_dictionaries_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let cases: [(&str, &str); 6] = [
        ("langs/en/en-kindlegen.mobi", "book"),
        ("langs/fr/fr-kindlegen.mobi", "rivière"),
        ("langs/ru/ru-kindlegen.mobi", "Москва"),
        ("langs/el/el-kindlegen.mobi", "νερό"),
        ("langs/ja/ja-kindlegen.mobi", "水"),
        ("parity/simple_dict/kindlegen_reference.mobi", "alpha"),
    ];
    for (path, word) in cases {
        let data = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let r = report(&data, word);
        assert!(
            r.result.is_some(),
            "{path}: {word:?} should resolve, index at {:?} with {} headwords",
            r.index_record,
            r.entries
        );
        assert_eq!(
            r.index_record.map(|i| i as u32),
            r.declared_index_record,
            "{path}: the header should name the record the orth index is actually in"
        );
        println!(
            "  \u{2713} {path}: {word:?} resolves via record {:?}",
            r.index_record
        );
    }
}
