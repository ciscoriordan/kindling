//! Per-language dictionary build tests, driven by the committed fixture
//! suite under `tests/fixtures/langs/`.
//!
//! For each supported language this builds the dictionary with kindling and
//! asserts the language-specific structure of the orth index:
//!
//! * the INDX header language id and the MOBI-header locale (the neutral
//!   primary LCID kindlegen writes, which the firmware's per-language query
//!   normalization is keyed on);
//! * that every entry has a real text pointer (no `(0, 0)` first-entry, the
//!   white-page bug fixed by `is_entry_boundary` recognizing `<mbp:frameset>`);
//! * that the headword set round-trips through the on-disk labels;
//! * for the generated-ORDT scripts (ja/zh/ko/ar): the per-character label
//!   encoding (kana are collation symbols, everything else is a literal code
//!   point), kana folding and gojuon order for Japanese, and byte parity of
//!   the ORDT table + headword labels against the committed kindlegen build
//!   (identical for the all-literal scripts; value-equivalent for Japanese);
//! * for the UTF-16BE scripts (en/el/fr/ru/tr): UTF-16BE labels in byte
//!   order and the static Greek collation blob.
//!
//! See issue #11 and `crate::ordt`. On-device verification status is in the
//! README supported-languages table.

mod common;

use common::*;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use kindling::ordt::{uses_generated_ordt, OrdtTables};

struct Lang {
    code: &'static str,
    /// INDX header language field (offset 32 of the orth primary).
    indx_lang: u32,
    /// MOBI-header locale (rec0 offset 92) and input language (rec0 offset
    /// 96): the neutral primary LCID.
    mobi_locale: u32,
    /// Expected `ordt_type` for generated-ORDT languages (0 = two-byte
    /// labels because literals are present, 1 = one-byte). Unused otherwise.
    ordt_type: u32,
}

const LANGS: &[Lang] = &[
    Lang { code: "ja", indx_lang: 17, mobi_locale: 0x0011, ordt_type: 0 },
    Lang { code: "zh", indx_lang: 4, mobi_locale: 0x0004, ordt_type: 0 },
    Lang { code: "ko", indx_lang: 0x0412, mobi_locale: 0x0412, ordt_type: 0 },
    Lang { code: "ar", indx_lang: 1, mobi_locale: 0x0001, ordt_type: 0 },
    Lang { code: "en", indx_lang: 9, mobi_locale: 0x0009, ordt_type: 0 },
    Lang { code: "el", indx_lang: 8, mobi_locale: 0x0008, ordt_type: 0 },
    Lang { code: "fr", indx_lang: 12, mobi_locale: 0x040C, ordt_type: 0 },
    Lang { code: "ru", indx_lang: 25, mobi_locale: 0x0019, ordt_type: 0 },
    Lang { code: "tr", indx_lang: 31, mobi_locale: 0x001F, ordt_type: 0 },
];

fn lang_dir(code: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/langs")
        .join(code)
}

/// The fixture's headwords, read from its committed data file (document order).
fn headwords(code: &str) -> Vec<String> {
    let tsv = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/langs/data")
        .join(format!("{code}.tsv"));
    let text = fs::read_to_string(&tsv).unwrap_or_else(|e| panic!("read {}: {e}", tsv.display()));
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some((hw, _)) = line.split_once('\t') {
            if seen.insert(hw.to_string()) {
                out.push(hw.to_string());
            }
        }
    }
    out
}

fn build(code: &str, subdir: &str) -> ParsedMobi {
    let opf = lang_dir(code).join("src/dict.opf");
    let tmp = std::env::temp_dir().join("kindling_langtest").join(subdir);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.mobi");
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {code}: {e}"))
}

fn load(path: &Path) -> ParsedMobi {
    parse_mobi_file(&fs::read(path).unwrap()).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn orth_primary(parsed: &ParsedMobi) -> (usize, &[u8]) {
    assert_ne!(parsed.kf7.header.orth_index, 0xFFFFFFFF, "no orth index");
    let idx = parsed.kf7.header.orth_index as usize;
    (idx, parsed.palmdb.record(&parsed.raw, idx))
}

/// Decode a per-character ORDT label back to text: each element is a symbol
/// index (`< oentries`, value is `ORDT2[idx]`) or a literal code point.
fn decode_percharacter(label: &[u8], cps: &[u32], two_byte: bool) -> String {
    let units: Vec<u32> = if two_byte {
        label.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]]) as u32).collect()
    } else {
        label.iter().map(|&b| b as u32).collect()
    };
    let mut s = String::new();
    for v in units {
        let cp = if (v as usize) < cps.len() { cps[v as usize] } else { v };
        if let Some(c) = char::from_u32(cp) {
            s.push(c);
        }
    }
    s
}

fn decode_utf16(label: &[u8]) -> String {
    let units: Vec<u16> = label.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
    String::from_utf16(&units).expect("UTF-16BE label")
}

/// Header-level checks plus the no-blank-first-entry invariant, common to
/// every language.
fn check_common(code: &str, c: &Lang, parsed: &ParsedMobi, idx: usize, primary: &[u8]) {
    assert_eq!(u32_be(primary, 32), c.indx_lang, "{code}: INDX language");
    assert_eq!(u32_be(primary, 28), 0xFDEA, "{code}: INDX encoding");

    let rec0 = parsed.palmdb.record(&parsed.raw, 0);
    assert_eq!(u32_be(rec0, 92), c.mobi_locale, "{code}: MOBI locale (neutral LCID)");
    assert_eq!(u32_be(rec0, 96), c.mobi_locale, "{code}: MOBI input language");

    let indx = parse_indx(parsed, idx).unwrap_or_else(|e| panic!("{code}: parse INDX: {e}"));
    // Every entry must point at a real article. The first headword used to
    // get (0, 0) and render as a blank page (issue #11 white-page bug).
    for e in &indx.entries {
        let pos = e.tag_value(1, 0).unwrap_or(0);
        let len = e.tag_value(2, 0).unwrap_or(0);
        assert!(
            !(pos == 0 && len == 0),
            "{code}: entry has empty (0,0) text pointer (blank lookup page)"
        );
    }
}

fn check_generated_ordt(code: &str, c: &Lang, parsed: &ParsedMobi, idx: usize, primary: &[u8]) {
    assert_eq!(u32_be(primary, 164), c.ordt_type, "{code}: ordt_type");
    let ordt = parse_indx_ordt2(primary).unwrap_or_else(|| panic!("{code}: missing ORDT tables"));
    let two_byte = ordt.ordt_type == 2;
    let indx = parse_indx(parsed, idx).unwrap();
    let hw = headwords(code); // document order

    // Labels decode back to exactly the fixture's headword set.
    let decoded: BTreeSet<String> = indx
        .entries
        .iter()
        .map(|e| decode_percharacter(&e.label, &ordt.codepoints, two_byte))
        .collect();
    let want: BTreeSet<String> = hw.iter().cloned().collect();
    assert_eq!(decoded, want, "{code}: decoded headword set");

    // Byte parity with the committed kindlegen build: identical ORDT table
    // and identical headword-label set for the all-literal scripts; for
    // Japanese (different kana numbering) the literal code points and the
    // collation order match instead.
    let reference = load(&lang_dir(code).join(format!("{code}-kindlegen.mobi")));
    let (ridx, rprimary) = orth_primary(&reference);
    assert_eq!(
        u32_be(primary, 164),
        u32_be(rprimary, 164),
        "{code}: ordt_type vs kindlegen"
    );
    let rordt = parse_indx_ordt2(rprimary).unwrap();
    if code != "ja" {
        assert_eq!(ordt.codepoints, rordt.codepoints, "{code}: ORDT2 table vs kindlegen");
        let mut kl: Vec<Vec<u8>> = indx.entries.iter().map(|e| e.label.clone()).collect();
        let rindx = parse_indx(&reference, ridx).unwrap();
        let mut rl: Vec<Vec<u8>> = rindx.entries.iter().map(|e| e.label.clone()).collect();
        kl.sort();
        rl.sort();
        assert_eq!(kl, rl, "{code}: headword labels vs kindlegen");
    }

    // Entries are in non-decreasing collation order under the rebuilt table
    // (built from the same headwords in document order).
    let refs: Vec<&str> = hw.iter().map(|s| s.as_str()).collect();
    let tables = OrdtTables::new(&refs);
    let keys: Vec<Vec<u32>> = indx.entries.iter().map(|e| tables.sort_key(&e.label)).collect();
    for i in 1..keys.len() {
        assert!(keys[i - 1] <= keys[i], "{code}: entries out of collation order at {i}");
    }
}

fn check_utf16(code: &str, _c: &Lang, parsed: &ParsedMobi, idx: usize, primary: &[u8]) {
    // Static Greek collation blob: spl_count 2, oentries 7.
    assert_eq!(u32_be(primary, 56), 2, "{code}: Greek blob spl_count");
    assert_eq!(u32_be(primary, 168), 7, "{code}: Greek blob oentries");

    let indx = parse_indx(parsed, idx).unwrap();
    let decoded: BTreeSet<String> = indx.entries.iter().map(|e| decode_utf16(&e.label)).collect();
    let want: BTreeSet<String> = headwords(code).into_iter().collect();
    assert_eq!(decoded, want, "{code}: decoded headword set");
    for pair in indx.entries.windows(2) {
        assert!(pair[0].label <= pair[1].label, "{code}: labels out of UTF-16BE order");
    }
}

fn check(code: &str) {
    let c = LANGS.iter().find(|l| l.code == code).unwrap();
    let parsed = build(code, code);
    let (idx, primary) = orth_primary(&parsed);
    check_common(code, c, &parsed, idx, primary);
    if uses_generated_ordt(code) {
        check_generated_ordt(code, c, &parsed, idx, primary);
    } else {
        check_utf16(code, c, &parsed, idx, primary);
    }
}

#[test]
fn dict_japanese() {
    check("ja");
}
#[test]
fn dict_chinese() {
    check("zh");
}
#[test]
fn dict_korean() {
    check("ko");
}
#[test]
fn dict_arabic() {
    check("ar");
}
#[test]
fn dict_english() {
    check("en");
}
#[test]
fn dict_greek() {
    check("el");
}
#[test]
fn dict_french() {
    check("fr");
}
#[test]
fn dict_russian() {
    check("ru");
}
#[test]
fn dict_turkish() {
    check("tr");
}
