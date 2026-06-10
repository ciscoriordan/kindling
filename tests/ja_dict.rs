//! Japanese dictionary INDX tests (issue #11).
//!
//! Kindle resolves Japanese dictionary lookups through generated ORDT
//! collation tables embedded in the orth primary INDX; an index sorted in
//! plain UTF-16BE order without them fails on device. These tests build
//! the `simple_dict_ja` fixture with kindling and assert the generated
//! structure, then cross-validate kindling's collation against the
//! committed kindlegen reference build of the same fixture: kindlegen's
//! physical entry order must be non-decreasing under kindling's sort
//! keys (ties allowed; the firmware scans equal-weight ranges).

mod common;

use common::*;

use std::fs;
use std::path::PathBuf;

use kindling::ordt::{used_bytes, OrdtTables};

/// All 14 headwords in the simple_dict_ja fixture. Chosen to cover the
/// interesting byte classes: plain hiragana/katakana (ignorable trail
/// bytes), the prolonged sound mark (trail 0xBC), weighted trail bytes
/// (0xB5, 0xB9, 0x9F), expansion escapes (0x8C in がき, 0xE6 in 愛),
/// and literal symbols (0x99 in す, 0x80 in む).
const JA_HEADWORDS: [&str; 14] = [
    "サクラ",
    "あい",
    "食べる",
    "かき",
    "ケーキ",
    "愛",
    "きゃく",
    "アイス",
    "にほん",
    "がき",
    "柿",
    "さくら",
    "す",
    "む",
];

/// `subdir` must be unique per test: the integration tests run in
/// parallel and would otherwise delete each other's output mid-build.
fn build_ja_and_parse(subdir: &str) -> ParsedMobi {
    let dir = parity_fixture("simple_dict_ja");
    let opf = dir.join("simple_dict_ja.opf");
    let tmp = std::env::temp_dir().join("kindling_roundtrip").join(subdir);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.mobi");
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

fn load_ja_reference() -> ParsedMobi {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/parity/simple_dict_ja/kindlegen_reference.mobi");
    let raw = fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", p.display()))
}

fn orth_primary(parsed: &ParsedMobi) -> (usize, &[u8]) {
    let idx = parsed.kf7.header.orth_index as usize;
    assert_ne!(
        parsed.kf7.header.orth_index, 0xFFFFFFFF,
        "dictionary has no orth index"
    );
    (idx, parsed.palmdb.record(&parsed.raw, idx))
}

#[test]
fn ja_dict_orth_indx_language_and_ordt_tables() {
    let parsed = build_ja_and_parse("simple_dict_ja_structure");
    let (_, primary) = orth_primary(&parsed);

    assert_eq!(
        u32_be(primary, 32),
        17,
        "INDX language must be the Japanese primary LCID (0x11)"
    );
    assert_eq!(u32_be(primary, 28), 0xFDEA, "INDX encoding");
    assert_eq!(
        u32_be(primary, 164),
        0,
        "ordt_type 0 = u16 BE symbol labels (kindlegen's Japanese form)"
    );

    let ordt = parse_indx_ordt2(primary).expect("ja dict must embed generated ORDT tables");
    assert_eq!(ordt.ordt_type, 2, "normalized to 2-byte label symbols");

    // The table always ends with the full hiragana block ぁ..ん.
    let hira: Vec<u32> = (0x3041..=0x3093).collect();
    assert!(
        ordt.codepoints.len() > hira.len(),
        "table too small: {} entries",
        ordt.codepoints.len()
    );
    assert_eq!(
        &ordt.codepoints[ordt.codepoints.len() - hira.len()..],
        &hira[..],
        "ORDT2 must end with the hiragana character block"
    );

    // ORDT1 weight table parallels ORDT2: same count, u16 BE entries,
    // "ORDT" magic, and the hiragana weights strictly increase.
    let ordt1_off = u32_be(primary, 172) as usize;
    assert_eq!(&primary[ordt1_off..ordt1_off + 4], b"ORDT");
    let n = ordt.codepoints.len();
    let w = |i: usize| u16_be(primary, ordt1_off + 4 + 2 * i);
    for i in (n - hira.len() + 1)..n {
        assert_eq!(w(i), w(i - 1) + 1, "hiragana weights must be sequential");
    }
}

#[test]
fn ja_dict_labels_decode_and_sort_by_collation_key() {
    let parsed = build_ja_and_parse("simple_dict_ja_labels");
    let (idx, primary) = orth_primary(&parsed);
    let ordt = parse_indx_ordt2(primary).expect("ORDT tables");
    let indx = parse_indx(&parsed, idx).expect("parse orth INDX");

    assert_eq!(indx.entries.len(), JA_HEADWORDS.len());

    // Every entry label decodes back to one of the fixture headwords.
    let mut decoded: Vec<String> = Vec::new();
    for e in &indx.entries {
        let s = decode_indx_label_ordt_text(&e.label, &ordt).unwrap_or_else(|| {
            panic!("label bytes {:02X?} did not decode", e.label)
        });
        decoded.push(s);
    }
    let mut got: Vec<&str> = decoded.iter().map(|s| s.as_str()).collect();
    got.sort_unstable();
    let mut want = JA_HEADWORDS.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "decoded headword set mismatch");

    // Entries must be stored in collation-key order. Rebuilding OrdtTables
    // from the same headword set reproduces the build's symbol table, so
    // its sort_key applies directly to the on-disk label bytes.
    let ja = OrdtTables::new(&used_bytes(JA_HEADWORDS.iter().copied()));
    let keys: Vec<Vec<u32>> = indx.entries.iter().map(|e| ja.sort_key(&e.label)).collect();
    for i in 1..keys.len() {
        assert!(
            keys[i - 1] <= keys[i],
            "entries out of collation order at {}: {:?} ({:?}) > {:?} ({:?})",
            i,
            decoded[i - 1],
            keys[i - 1],
            decoded[i],
            keys[i]
        );
    }

    // Kana folding sanity: hiragana and katakana spellings whose UTF-8
    // trail bytes are all ignorable carry equal keys (the firmware finds
    // katakana entries by scanning the shared equal-weight range). Note
    // this is byte-level cp1252 folding, exactly like kindlegen: pairs
    // with weighted trail bytes (e.g. サ, whose 0xB5 folds to 'm') keep
    // distinct keys in kindlegen's output too.
    assert_eq!(
        ja.sort_key(&ja.encode_label("あい")),
        ja.sort_key(&ja.encode_label("アイ")),
    );
}

#[test]
fn ja_dict_kindlegen_reference_cross_validation() {
    // Decode kindlegen's entries in their physical (binary-search) order
    // and check that kindling's collation keys are non-decreasing along
    // it. This pins kindling's weight classes to kindlegen's: any
    // divergence in the byte folding or escape handling shows up as an
    // order violation here.
    let reference = load_ja_reference();
    let (idx, primary) = orth_primary(&reference);

    assert_eq!(u32_be(primary, 32), 17, "kindlegen reference INDX language");
    let ordt = parse_indx_ordt2(primary).expect("kindlegen reference has ORDT tables");
    let indx = parse_indx(&reference, idx).expect("parse kindlegen orth INDX");
    assert_eq!(indx.entries.len(), JA_HEADWORDS.len());

    let mut decoded: Vec<String> = Vec::new();
    for e in &indx.entries {
        let s = decode_indx_label_ordt_text(&e.label, &ordt).unwrap_or_else(|| {
            panic!("kindlegen label bytes {:02X?} did not decode", e.label)
        });
        decoded.push(s);
    }
    let mut got: Vec<&str> = decoded.iter().map(|s| s.as_str()).collect();
    got.sort_unstable();
    let mut want = JA_HEADWORDS.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "kindlegen decoded headword set mismatch");

    let ja = OrdtTables::new(&used_bytes(decoded.iter().map(|s| s.as_str())));
    let keys: Vec<Vec<u32>> = decoded
        .iter()
        .map(|s| ja.sort_key(&ja.encode_label(s)))
        .collect();
    for i in 1..keys.len() {
        assert!(
            keys[i - 1] <= keys[i],
            "kindling key order disagrees with kindlegen at {}: {:?} ({:?}) > {:?} ({:?})",
            i,
            decoded[i - 1],
            keys[i - 1],
            decoded[i],
            keys[i]
        );
    }
}

#[test]
fn english_dict_indx_language_matches_kindlegen() {
    // Non-Japanese dictionaries keep UTF-16BE labels and the static
    // Greek collation blob, but the INDX language field now follows the
    // declared input language like kindlegen (en = 9) instead of the old
    // hardcoded 8.
    let dir = parity_fixture("simple_dict");
    let opf = dir.join("simple_dict.opf");
    let tmp = std::env::temp_dir()
        .join("kindling_roundtrip")
        .join("simple_dict_lang");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.mobi");
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap();
    let parsed = parse_mobi_file(&raw).unwrap();
    let (_, primary) = orth_primary(&parsed);
    assert_eq!(u32_be(primary, 32), 9, "English INDX language (LCID 0x09)");

    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/parity/simple_dict/kindlegen_reference.mobi");
    let reference = parse_mobi_file(&fs::read(&p).unwrap()).unwrap();
    let (_, ref_primary) = orth_primary(&reference);
    assert_eq!(u32_be(ref_primary, 32), 9, "kindlegen writes 9 for en");
}
