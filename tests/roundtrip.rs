//! Structural round-trip tests for kindling's MOBI/AZW3 output.
//!
//! Each test builds one of the parity fixtures with `kindling-cli`, parses
//! the resulting file with the tiny parser in `tests/common/mod.rs`, and
//! asserts the PalmDB header, MOBI header, EXTH, INDX / SKEL / FRAG
//! records, and text blob have the shape we expect. These catch format-
//! level regressions where libmobi would accept an output that does not
//! round-trip back into the same structure.
//!
//! These tests are kept deliberately independent of kindling's own
//! internals: we parse bytes out of the written file without touching any
//! private kindling API. `tests/common/mod.rs` re-implements PalmDOC LZ77
//! decompression inline to avoid exposing `src/palmdoc.rs` through the
//! public crate API just for testing.

mod common;

use common::*;

use std::fs;

fn build_and_parse(opf_name: &str, fixture: &str, out_ext: &str) -> ParsedMobi {
    let dir = parity_fixture(fixture);
    let opf = dir.join(opf_name);
    let tmp = std::env::temp_dir()
        .join("kindling_roundtrip")
        .join(fixture);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join(format!("out.{out_ext}"));
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

fn build_comic_and_parse() -> ParsedMobi {
    let cbz = parity_fixture("simple_comic").join("simple_comic.cbz");
    let tmp = std::env::temp_dir().join("kindling_roundtrip").join("comic");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.azw3");
    kindling_comic(&cbz, &out);
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap()
}

// ---------------------------------------------------------------------------
// PalmDB shape (applies to all three fixtures)
// ---------------------------------------------------------------------------

fn assert_palmdb_shape(parsed: &ParsedMobi, ctx: &str) {
    assert_eq!(
        &parsed.palmdb.ty, b"BOOK",
        "{ctx}: PalmDB type is {:?}, expected BOOK",
        parsed.palmdb.ty
    );
    assert_eq!(
        &parsed.palmdb.creator, b"MOBI",
        "{ctx}: PalmDB creator is {:?}, expected MOBI",
        parsed.palmdb.creator
    );
    assert!(
        parsed.palmdb.num_records > 0,
        "{ctx}: record count is 0"
    );
    for (i, pair) in parsed.palmdb.offsets.windows(2).enumerate() {
        assert!(
            pair[1] > pair[0],
            "{ctx}: record offsets not monotonic at {i}: {} -> {}",
            pair[0],
            pair[1]
        );
    }
}

fn assert_required_exth(section: &MobiSection, ctx: &str, need_501: bool) {
    for rtype in [100u32, 503, 524] {
        assert!(
            section.exth_first(rtype).is_some(),
            "{ctx}: missing required EXTH {rtype}. Present: {:?}",
            section.exth_types()
        );
    }
    if need_501 {
        assert!(
            section.exth_first(501).is_some(),
            "{ctx}: missing required EXTH 501 (cdetype). Present: {:?}",
            section.exth_types()
        );
    }
}

// ---------------------------------------------------------------------------
// Dictionary round-trip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_simple_dict() {
    let parsed = build_and_parse("simple_dict.opf", "simple_dict", "mobi");
    assert_palmdb_shape(&parsed, "dict");

    let kf7 = &parsed.kf7;
    // Dictionaries are MOBI7 (KF7). Kindling writes file_version 6 or 7;
    // kindlegen writes 6. Either is acceptable for a non-KF8 file.
    assert!(
        kf7.header.file_version >= 6 && kf7.header.file_version < 8,
        "dict: KF7 file_version should be 6 or 7, got {}",
        kf7.header.file_version
    );
    // Dictionaries use UTF-8 in kindling (65001); kindlegen-built dicts may
    // use UTF-16 (65002). Accept either.
    assert!(
        kf7.header.encoding == 65001 || kf7.header.encoding == 65002,
        "dict: unexpected encoding {}",
        kf7.header.encoding
    );
    assert!(
        kf7.header.orth_index != 0xFFFFFFFF && kf7.header.orth_index != 0,
        "dict: orth_index must point somewhere (got 0x{:08x})",
        kf7.header.orth_index
    );
    // Dict is KF7-only, no KF8 boundary.
    assert!(
        parsed.kf8_boundary.is_none(),
        "dict: should not have a KF8 boundary"
    );

    // 501 is a warning, not a requirement, on kindling dicts.
    assert_required_exth(kf7, "dict KF7", false);

    // INDX structure: at least one primary INDX record must exist.
    let indx_records = find_indx_records(&parsed);
    assert!(
        !indx_records.is_empty(),
        "dict: no INDX records found in MOBI"
    );

    // Parse the primary orth INDX.
    let primary_idx = kf7.header.orth_index as usize;
    assert!(
        primary_idx < parsed.palmdb.num_records,
        "dict: orth_index {} out of bounds (num_records {})",
        primary_idx,
        parsed.palmdb.num_records
    );
    let primary = parsed.palmdb.record(&parsed.raw, primary_idx);
    let hdr = parse_indx_header(primary)
        .unwrap_or_else(|e| panic!("dict: primary INDX record {primary_idx}: {e}"));
    assert!(
        hdr.num_records >= 1,
        "dict: primary INDX claims {} data records, need >= 1",
        hdr.num_records
    );

    // Decompress text blob and confirm its length equals the declared
    // text_length in the PalmDOC header.
    let blob = extract_text_blob(&parsed, kf7);
    assert_eq!(
        blob.len() as u32,
        kf7.header.text_length,
        "dict: decompressed blob is {} bytes, header says {}",
        blob.len(),
        kf7.header.text_length
    );

    // The decompressed blob should contain each headword wrapped in <b>.
    let blob_str = String::from_utf8_lossy(&blob);
    for hw in ["alpha", "bravo", "charlie", "delta", "echo"] {
        let needle = format!("<b>{hw}</b>");
        assert!(
            blob_str.contains(&needle),
            "dict: decompressed text missing {:?}. First 200 bytes: {:?}",
            needle,
            &blob_str.chars().take(200).collect::<String>()
        );
    }

    // Grab the first 5 bytes of the first data INDX record's first label.
    // For our fixture (sorted alphabetically) the first entry is "alpha".
    let label = first_indx_label_prefix(&parsed, primary_idx, 5);
    assert!(
        !label.is_empty(),
        "dict: could not read first INDX data record label prefix"
    );
}

// ---------------------------------------------------------------------------
// Book (KF8-only) round-trip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_simple_book() {
    let parsed = build_and_parse("simple_book.opf", "simple_book", "azw3");
    assert_palmdb_shape(&parsed, "book");

    let kf7 = &parsed.kf7;
    assert!(
        kf7.header.file_version >= 8,
        "book: KF8-only output must have file_version >= 8, got {}",
        kf7.header.file_version
    );
    assert_eq!(
        kf7.header.encoding, 65001,
        "book: KF8 encoding must be UTF-8 (65001), got {}",
        kf7.header.encoding
    );
    // KF8 orth_index points to fragment INDX record (matches KCC/kindlegen).
    assert_ne!(
        kf7.header.orth_index, 0xFFFFFFFF,
        "book: KF8 orth_index should point to fragment INDX, got 0x{:08x}",
        kf7.header.orth_index
    );
    assert_required_exth(kf7, "book KF8", true);

    // KF8: FDST / skeleton / fragment indices must point at real records.
    let skel_idx = kf7.header.skeleton_index as usize;
    let frag_idx = kf7.header.fragment_index as usize;
    assert!(
        skel_idx > 0 && skel_idx < parsed.palmdb.num_records,
        "book: skeleton_index {} out of bounds (num_records {})",
        skel_idx,
        parsed.palmdb.num_records
    );
    assert!(
        frag_idx > 0 && frag_idx < parsed.palmdb.num_records,
        "book: fragment_index {} out of bounds",
        frag_idx
    );
    let skel_rec = parsed.palmdb.record(&parsed.raw, skel_idx);
    assert_eq!(
        &skel_rec[..4],
        b"INDX",
        "book: skeleton_index record {} is not INDX (first 4 bytes {:?})",
        skel_idx,
        &skel_rec[..4.min(skel_rec.len())]
    );
    let frag_rec = parsed.palmdb.record(&parsed.raw, frag_idx);
    assert_eq!(
        &frag_rec[..4],
        b"INDX",
        "book: fragment_index record {} is not INDX (first 4 bytes {:?})",
        frag_idx,
        &frag_rec[..4.min(frag_rec.len())]
    );

    // Decompressed text length equals PalmDOC text_length.
    let blob = extract_text_blob(&parsed, kf7);
    assert_eq!(
        blob.len() as u32,
        kf7.header.text_length,
        "book: decompressed blob {} bytes, header says {}",
        blob.len(),
        kf7.header.text_length
    );

    // Text blob should still contain the chapter headers.
    let blob_str = String::from_utf8_lossy(&blob);
    for s in ["Chapter One", "Chapter Two", "Chapter Three"] {
        assert!(
            blob_str.contains(s),
            "book: blob missing {s:?}. First 200 chars: {}",
            blob_str.chars().take(200).collect::<String>()
        );
    }
}

// ---------------------------------------------------------------------------
// Comic (KF8-only, fixed-layout) round-trip
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_simple_comic() {
    let parsed = build_comic_and_parse();
    assert_palmdb_shape(&parsed, "comic");

    let kf7 = &parsed.kf7;
    assert!(
        kf7.header.file_version >= 8,
        "comic: KF8-only file_version >= 8, got {}",
        kf7.header.file_version
    );
    assert_eq!(
        kf7.header.encoding, 65001,
        "comic: encoding must be UTF-8, got {}",
        kf7.header.encoding
    );

    assert_required_exth(kf7, "comic KF8", true);

    // Comics require EXTH 201 (cover offset) and 202 (thumbnail offset) to
    // render in the Kindle library. The pure readback check in
    // src/mobi_check.rs already enforces this but repeating it here keeps
    // the round-trip test self-contained.
    assert!(
        kf7.exth_first(201).is_some(),
        "comic: EXTH 201 (cover_offset) missing. Present: {:?}",
        kf7.exth_types()
    );
    assert!(
        kf7.exth_first(202).is_some(),
        "comic: EXTH 202 (thumbnail_offset) missing. Present: {:?}",
        kf7.exth_types()
    );

    // first_image_record points at a JPEG record.
    let fir = kf7.header.first_image_record as usize;
    assert!(
        fir > 0 && fir < parsed.palmdb.num_records,
        "comic: first_image_record {} out of bounds",
        fir
    );
    let img = parsed.palmdb.record(&parsed.raw, fir);
    assert!(
        img.len() >= 2 && img[0] == 0xFF && img[1] == 0xD8,
        "comic: first image record {fir} is not a JPEG (first bytes: {:?})",
        &img[..img.len().min(4)]
    );

    // Skeleton + fragment INDX records must be present (comics use them for
    // fixed-layout page structure, one skeleton per page).
    let skel_idx = kf7.header.skeleton_index as usize;
    let frag_idx = kf7.header.fragment_index as usize;
    assert!(
        skel_idx > 0 && skel_idx < parsed.palmdb.num_records,
        "comic: skeleton_index {} out of bounds",
        skel_idx
    );
    assert!(
        frag_idx > 0 && frag_idx < parsed.palmdb.num_records,
        "comic: fragment_index {} out of bounds",
        frag_idx
    );
    let skel_rec = parsed.palmdb.record(&parsed.raw, skel_idx);
    assert_eq!(
        &skel_rec[..4],
        b"INDX",
        "comic: skeleton record {skel_idx} is not INDX"
    );

    // Decompressed text length equals header.text_length.
    let blob = extract_text_blob(&parsed, kf7);
    assert_eq!(
        blob.len() as u32,
        kf7.header.text_length,
        "comic: decompressed {} != header {}",
        blob.len(),
        kf7.header.text_length
    );
}
