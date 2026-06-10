//! Per-language dictionary build tests.
//!
//! One test per supported input language, asserting the language-specific
//! parts of the orth INDX: the language id in the INDX header, the
//! dictionary input locale in the MOBI header, EXTH 531, the label
//! encoding (UTF-16BE or generated-ORDT symbols), the entry sort
//! invariant for that encoding, and which collation tables are embedded.
//! Languages on the generated-ORDT path (zh, ko, ar; ja has its own file
//! in `tests/ja_dict.rs`) are additionally cross-validated against
//! committed kindlegen reference builds of the same fixture content.
//!
//! These are structural tests: they pin kindling's output to the layout
//! kindlegen produces and to the layouts known to work on devices. They
//! cannot prove on-device lookup behavior by themselves; see the
//! supported-languages table in the README for per-language verification
//! status.

mod common;

use common::*;

use std::fs;
use std::path::{Path, PathBuf};

use kindling::ordt::{used_bytes, uses_generated_ordt, OrdtTables};

struct LangCase {
    /// BCP 47 / ISO 639 code written to DictionaryInLanguage.
    code: &'static str,
    /// Expected INDX header language field (offset 32 of the orth primary).
    indx_lang: u32,
    /// Expected MOBI header dictionary input language (full Windows LCID,
    /// offset 80 relative to the MOBI magic).
    mobi_locale: u32,
    /// Headwords for the synthesized dictionary, chosen to exercise the
    /// language's interesting byte classes.
    headwords: &'static [&'static str],
    /// Committed parity fixture (with kindlegen_reference.mobi) to build
    /// from instead of synthesizing; enables kindlegen cross-validation.
    fixture: Option<&'static str>,
}

const CASES: &[LangCase] = &[
    LangCase {
        code: "en",
        indx_lang: 9,
        mobi_locale: 0x0409,
        headwords: &["apple", "Apple", "queue", "zebra"],
        fixture: None,
    },
    LangCase {
        code: "el",
        indx_lang: 8,
        mobi_locale: 0x0408,
        // Mixed monotonic and polytonic, the lemma use case.
        headwords: &["λόγος", "θάλασσα", "ἀγάπη", "ἄνθρωπος"],
        fixture: None,
    },
    LangCase {
        code: "fr",
        indx_lang: 12,
        mobi_locale: 0x040C,
        headwords: &["meme", "même", "mère", "café"],
        fixture: None,
    },
    LangCase {
        code: "ru",
        indx_lang: 25,
        mobi_locale: 0x0419,
        // Москва exercises the uppercase Cyrillic block, ёлка the
        // out-of-row U+0451.
        headwords: &["любовь", "вода", "гора", "Москва", "ёлка"],
        fixture: None,
    },
    LangCase {
        code: "tr",
        indx_lang: 31,
        mobi_locale: 0x041F,
        // Dotless ı and the other Turkish-specific letters.
        headwords: &["ışık", "iğne", "içmek", "üzüm", "şeker"],
        fixture: None,
    },
    LangCase {
        code: "zh",
        indx_lang: 4,
        mobi_locale: 0x0804,
        headwords: &["爱", "水", "山", "人", "中国", "北京", "上海", "书"],
        fixture: Some("simple_dict_zh"),
    },
    LangCase {
        code: "ko",
        // kindlegen quirk: Korean gets the full LCID, not the primary id.
        indx_lang: 0x0412,
        mobi_locale: 0x0412,
        headwords: &["사랑", "물", "산", "한국", "서울", "김치"],
        fixture: Some("simple_dict_ko"),
    },
    LangCase {
        code: "ar",
        indx_lang: 1,
        mobi_locale: 0x0401,
        headwords: &["حب", "ماء", "جبل", "كتاب", "مكتبة", "شمس", "قمر"],
        fixture: Some("simple_dict_ar"),
    },
];

fn case(code: &str) -> &'static LangCase {
    CASES.iter().find(|c| c.code == code).unwrap()
}

/// Write a minimal OPF dictionary source for `case` into `dir`.
fn synthesize_source(dir: &Path, c: &LangCase) -> PathBuf {
    let mut html = String::from(
        "<!DOCTYPE html>\n<html xmlns:idx=\"http://www.mobipocket.com/idx\" \
         xmlns:mbp=\"http://www.mobipocket.com\">\n<head><title>Lang Test</title></head>\n\
         <body><mbp:frameset>\n",
    );
    for w in c.headwords {
        html.push_str(&format!(
            "<idx:entry name=\"default\" scriptable=\"yes\">\
             <idx:orth value=\"{w}\"><b>{w}</b></idx:orth>\
             <p>entry for {w}</p></idx:entry><mbp:pagebreak/>\n"
        ));
    }
    html.push_str("</mbp:frameset></body></html>\n");
    fs::write(dir.join("content.html"), html).unwrap();

    let uid = format!("kindling-langtest-{}", c.code);
    fs::write(
        dir.join("toc.ncx"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n\
             <head><meta name=\"dtb:uid\" content=\"{uid}\"/><meta name=\"dtb:depth\" content=\"1\"/>\n\
             <meta name=\"dtb:totalPageCount\" content=\"0\"/><meta name=\"dtb:maxPageNumber\" content=\"0\"/></head>\n\
             <docTitle><text>Lang Test</text></docTitle>\n\
             <navMap><navPoint id=\"n1\" playOrder=\"1\"><navLabel><text>E</text></navLabel>\
             <content src=\"content.html\"/></navPoint></navMap>\n</ncx>\n"
        ),
    )
    .unwrap();

    let opf = dir.join("dict.opf");
    fs::write(
        &opf,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <package version=\"2.0\" xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"BookId\">\n\
             <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n\
             <dc:title>Lang Test {code}</dc:title>\n\
             <dc:language>{code}</dc:language>\n\
             <dc:creator>Kindling Lang Suite</dc:creator>\n\
             <dc:identifier id=\"BookId\">{uid}</dc:identifier>\n\
             <x-metadata>\n\
             <DictionaryInLanguage>{code}</DictionaryInLanguage>\n\
             <DictionaryOutLanguage>en</DictionaryOutLanguage>\n\
             <DefaultLookupIndex>default</DefaultLookupIndex>\n\
             </x-metadata>\n\
             </metadata>\n\
             <manifest>\n\
             <item id=\"content\" href=\"content.html\" media-type=\"application/xhtml+xml\"/>\n\
             <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n\
             </manifest>\n\
             <spine toc=\"ncx\"><itemref idref=\"content\"/></spine>\n\
             </package>\n",
            code = c.code,
        ),
    )
    .unwrap();
    opf
}

fn build_case(c: &LangCase) -> ParsedMobi {
    let tmp = std::env::temp_dir()
        .join("kindling_langtest")
        .join(c.code);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let opf = match c.fixture {
        Some(fixture) => parity_fixture(fixture).join(format!("{fixture}.opf")),
        None => synthesize_source(&tmp, c),
    };
    let out = tmp.join("out.mobi");
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {} output: {e}", c.code))
}

fn orth_primary(parsed: &ParsedMobi) -> (usize, &[u8]) {
    assert_ne!(
        parsed.kf7.header.orth_index, 0xFFFFFFFF,
        "dictionary has no orth index"
    );
    let idx = parsed.kf7.header.orth_index as usize;
    (idx, parsed.palmdb.record(&parsed.raw, idx))
}

fn decode_utf16_label(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&units).expect("UTF-16BE label")
}

/// Full per-language assertion battery; see module docs.
fn check_language(code: &str) {
    let c = case(code);
    let parsed = build_case(c);
    let (idx, primary) = orth_primary(&parsed);

    // INDX header language and encoding.
    assert_eq!(
        u32_be(primary, 32),
        c.indx_lang,
        "{code}: INDX language field"
    );
    assert_eq!(u32_be(primary, 28), 0xFDEA, "{code}: INDX encoding");

    // MOBI header dictionary input language (offset 80 from MOBI magic,
    // absolute offset 96 in record 0) and EXTH 531 string.
    let rec0 = parsed.palmdb.record(&parsed.raw, 0);
    assert_eq!(
        u32_be(rec0, 96),
        c.mobi_locale,
        "{code}: MOBI header dict input locale"
    );
    let exth531 = parsed
        .kf7
        .exth_first(531)
        .unwrap_or_else(|| panic!("{code}: missing EXTH 531"));
    assert_eq!(
        String::from_utf8_lossy(exth531),
        c.code,
        "{code}: EXTH 531 input language"
    );

    let indx = parse_indx(&parsed, idx).unwrap_or_else(|e| panic!("{code}: parse INDX: {e}"));
    assert_eq!(
        indx.entries.len(),
        c.headwords.len(),
        "{code}: entry count"
    );

    let mut decoded: Vec<String> = Vec::new();
    if uses_generated_ordt(code) {
        // Generated-ORDT path: tables present, labels are symbol
        // sequences, entries sorted by collation key.
        assert_eq!(u32_be(primary, 164), 0, "{code}: ordt_type");
        let ordt = parse_indx_ordt2(primary)
            .unwrap_or_else(|| panic!("{code}: generated ORDT tables missing"));
        for e in &indx.entries {
            decoded.push(
                decode_indx_label_ordt_text(&e.label, &ordt)
                    .unwrap_or_else(|| panic!("{code}: label {:02X?} did not decode", e.label)),
            );
        }
        let tables = OrdtTables::new(&used_bytes(c.headwords.iter().copied()));
        let keys: Vec<Vec<u32>> = indx
            .entries
            .iter()
            .map(|e| tables.sort_key(&e.label))
            .collect();
        for i in 1..keys.len() {
            assert!(
                keys[i - 1] <= keys[i],
                "{code}: entries out of collation order at {i}: {:?} > {:?}",
                decoded[i - 1],
                decoded[i]
            );
        }
    } else {
        // UTF-16BE path: plain labels in byte order, static Greek
        // ORDT/SPL collation blob embedded (spl_count 2, oentries 7).
        assert_eq!(u32_be(primary, 56), 2, "{code}: Greek blob spl_count");
        assert_eq!(u32_be(primary, 168), 7, "{code}: Greek blob oentries");
        for e in &indx.entries {
            decoded.push(decode_utf16_label(&e.label));
        }
        for pair in indx.entries.windows(2) {
            assert!(
                pair[0].label <= pair[1].label,
                "{code}: labels out of UTF-16BE order"
            );
        }
    }

    let mut got: Vec<&str> = decoded.iter().map(|s| s.as_str()).collect();
    got.sort_unstable();
    let mut want = c.headwords.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "{code}: decoded headword set");
}

/// Cross-validate kindling's collation against the committed kindlegen
/// reference for a generated-ORDT language: kindlegen's physical entry
/// order must be non-decreasing under kindling's sort keys.
fn cross_validate_kindlegen(code: &str) {
    let c = case(code);
    let fixture = c.fixture.expect("cross-validation needs a fixture");
    let path = parity_fixture(fixture).join("kindlegen_reference.mobi");
    let reference = parse_mobi_file(&fs::read(&path).unwrap()).unwrap();
    let (idx, primary) = orth_primary(&reference);

    assert_eq!(
        u32_be(primary, 32),
        c.indx_lang,
        "{code}: kindlegen reference INDX language"
    );
    let ordt = parse_indx_ordt2(primary)
        .unwrap_or_else(|| panic!("{code}: kindlegen reference has no ORDT"));
    let indx = parse_indx(&reference, idx).unwrap();
    assert_eq!(indx.entries.len(), c.headwords.len());

    let decoded: Vec<String> = indx
        .entries
        .iter()
        .map(|e| {
            decode_indx_label_ordt_text(&e.label, &ordt).unwrap_or_else(|| {
                panic!("{code}: kindlegen label {:02X?} did not decode", e.label)
            })
        })
        .collect();
    let mut got: Vec<&str> = decoded.iter().map(|s| s.as_str()).collect();
    got.sort_unstable();
    let mut want = c.headwords.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "{code}: kindlegen decoded headword set");

    let tables = OrdtTables::new(&used_bytes(decoded.iter().map(|s| s.as_str())));
    let keys: Vec<Vec<u32>> = decoded
        .iter()
        .map(|s| tables.sort_key(&tables.encode_label(s)))
        .collect();
    for i in 1..keys.len() {
        assert!(
            keys[i - 1] <= keys[i],
            "{code}: kindling key order disagrees with kindlegen at {i}: {:?} > {:?}",
            decoded[i - 1],
            decoded[i]
        );
    }
}

#[test]
fn dict_lang_english() {
    check_language("en");
}

#[test]
fn dict_lang_greek() {
    check_language("el");
}

#[test]
fn dict_lang_french() {
    check_language("fr");
}

#[test]
fn dict_lang_russian() {
    check_language("ru");
}

#[test]
fn dict_lang_turkish() {
    check_language("tr");
}

#[test]
fn dict_lang_chinese() {
    check_language("zh");
}

#[test]
fn dict_lang_korean() {
    check_language("ko");
}

#[test]
fn dict_lang_arabic() {
    check_language("ar");
}

#[test]
fn dict_lang_chinese_kindlegen_cross_validation() {
    cross_validate_kindlegen("zh");
}

#[test]
fn dict_lang_korean_kindlegen_cross_validation() {
    cross_validate_kindlegen("ko");
}

#[test]
fn dict_lang_arabic_kindlegen_cross_validation() {
    cross_validate_kindlegen("ar");
}
