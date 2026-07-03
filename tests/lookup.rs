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
use kindling::lookup::lookup;

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
    assert!(lookup(&data, "存在しない語").is_none(), "non-headword misses");
}
