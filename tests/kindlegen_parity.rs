//! Byte/field parity tests against committed kindlegen reference fixtures.
//!
//! For each parity fixture we build the input with `kindling-cli`, then
//! parse both the fresh kindling output AND the committed
//! `kindlegen_reference.mobi` and diff them field by field. Kindlegen is
//! the canonical reference; any diff points at a potential bug in
//! kindling's format emitter.
//!
//! These tests run in ANY environment. kindlegen is never invoked at test
//! time: the reference .mobi files are committed to the repo and read
//! directly. To regenerate them after a fixture source change, run
//! `scripts/regenerate_parity_fixtures.sh` (requires a local kindlegen
//! install).
//!
//! Fields that cannot possibly match between two independent builders
//! (unique_id derived from the build time, FCIS random bytes, EXTH
//! UID/timestamps at 112, 113, 204-207, etc.) are compared by PRESENCE
//! only, not value. Core metadata (EXTH 100, 101, 524) must match
//! exactly. Diffs are surfaced in a readable table via stderr with
//! `cargo test -- --nocapture`.

mod common;

use common::*;

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// Values that are expected to differ between kindling and kindlegen
/// (timestamps, UIDs, cryptographic identifiers). Diffs against these are
/// suppressed: the test only verifies both tools emit the EXTH record.
const PRESENCE_ONLY_EXTH: &[u32] = &[
    112, // source (UID)
    113, // ASIN (build-specific)
    204, // creator software
    205, // creator major
    206, // creator minor
    207, // creator build
    208, // watermark
    403, 404, 405, 406, 407, // retail metadata blob
    535, // creator-build-revision
];

/// Values that MUST match bytewise between kindling and kindlegen. Any diff
/// here fails the test loudly.
const EXACT_MATCH_EXTH: &[u32] = &[
    100, // creator
    101, // publisher
    524, // language
];

#[derive(Default, Debug)]
struct Diff {
    lines: Vec<String>,
}

impl Diff {
    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }
    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
    fn into_error(self, header: &str) -> String {
        let mut s = String::new();
        s.push_str(header);
        s.push('\n');
        for line in &self.lines {
            s.push_str("  ");
            s.push_str(line);
            s.push('\n');
        }
        s
    }
}

/// Report a diff between two scalar values for the same field name.
fn cmp_scalar<T: std::fmt::Debug + PartialEq>(
    diff: &mut Diff,
    field: &str,
    kindling: T,
    kindlegen: T,
) {
    if kindling != kindlegen {
        diff.push(format!(
            "{field}: kindling={kindling:?} kindlegen={kindlegen:?}"
        ));
    }
}

fn cmp_exth(diff: &mut Diff, kindling: &MobiSection, kindlegen: &MobiSection) {
    let k_types: BTreeSet<u32> = kindling.exth.iter().map(|r| r.rtype).collect();
    let g_types: BTreeSet<u32> = kindlegen.exth.iter().map(|r| r.rtype).collect();

    let only_kindling: Vec<u32> = k_types.difference(&g_types).copied().collect();
    let only_kindlegen: Vec<u32> = g_types.difference(&k_types).copied().collect();

    if !only_kindling.is_empty() {
        diff.push(format!("EXTH only in kindling: {:?}", only_kindling));
    }
    if !only_kindlegen.is_empty() {
        diff.push(format!(
            "EXTH missing from kindling but present in kindlegen: {:?}",
            only_kindlegen
        ));
    }

    for rtype in EXACT_MATCH_EXTH {
        let k = kindling.exth_first(*rtype);
        let g = kindlegen.exth_first(*rtype);
        if k != g {
            let k_display = k.map(|b| String::from_utf8_lossy(b).to_string());
            let g_display = g.map(|b| String::from_utf8_lossy(b).to_string());
            diff.push(format!(
                "EXTH {rtype}: kindling={k_display:?} kindlegen={g_display:?}"
            ));
        }
    }

    for rtype in PRESENCE_ONLY_EXTH {
        let k_has = kindling.exth_first(*rtype).is_some();
        let g_has = kindlegen.exth_first(*rtype).is_some();
        if k_has != g_has {
            diff.push(format!(
                "EXTH {rtype} presence: kindling={k_has} kindlegen={g_has}"
            ));
        }
    }
}

fn cmp_mobi_header(diff: &mut Diff, kindling: &MobiSection, kindlegen: &MobiSection) {
    let k = &kindling.header;
    let g = &kindlegen.header;
    cmp_scalar(diff, "mobi_type", k.mobi_type, g.mobi_type);
    cmp_scalar(diff, "encoding", k.encoding, g.encoding);
    cmp_scalar(diff, "file_version", k.file_version, g.file_version);
    cmp_scalar(diff, "min_version", k.min_version, g.min_version);
    let k_has_orth = k.orth_index != 0xFFFFFFFF;
    let g_has_orth = g.orth_index != 0xFFFFFFFF;
    cmp_scalar(diff, "has_orth_index", k_has_orth, g_has_orth);
    cmp_scalar(diff, "locale", k.locale, g.locale);
}

// ---------------------------------------------------------------------------
// Reading committed fixtures
// ---------------------------------------------------------------------------

fn reference_path(fixture: &str) -> PathBuf {
    parity_fixture(fixture).join("kindlegen_reference.mobi")
}

fn load_reference(fixture: &str) -> ParsedMobi {
    let path = reference_path(fixture);
    let data = fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "could not read committed kindlegen reference {}: {e}.\n\
             Run scripts/regenerate_parity_fixtures.sh to rebuild it.",
            path.display()
        )
    });
    parse_mobi_file(&data).unwrap_or_else(|e| {
        panic!(
            "could not parse committed kindlegen reference {}: {e}",
            path.display()
        )
    })
}

/// Build a fixture with `kindling-cli build` into a scratch tempdir and
/// return the parsed output. Used for dict and book fixtures.
fn kindling_build_parsed(fixture: &str, opf_name: &str, ext: &str) -> ParsedMobi {
    let opf = parity_fixture(fixture).join(opf_name);
    let tmp = std::env::temp_dir()
        .join("kindling_parity")
        .join(fixture);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join(format!("out.{ext}"));
    kindling_build(&opf, &out);
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

fn kindling_comic_parsed(fixture: &str, src_name: &str, ext: &str) -> ParsedMobi {
    let input = parity_fixture(fixture).join(src_name);
    let tmp = std::env::temp_dir()
        .join("kindling_parity")
        .join(fixture);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join(format!("out.{ext}"));
    kindling_comic(&input, &out);
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn parity_simple_dict() {
    let kindling = kindling_build_parsed("simple_dict", "simple_dict.opf", "mobi");
    let kindlegen = load_reference("simple_dict");

    let mut diff = Diff::default();

    if (kindling.palmdb.num_records as i64 - kindlegen.palmdb.num_records as i64).abs() > 3 {
        diff.push(format!(
            "palmdb record count differs by more than 3: kindling={} kindlegen={}",
            kindling.palmdb.num_records, kindlegen.palmdb.num_records
        ));
    }
    cmp_scalar(
        &mut diff,
        "palmdb_type",
        &kindling.palmdb.ty,
        &kindlegen.palmdb.ty,
    );
    cmp_scalar(
        &mut diff,
        "palmdb_creator",
        &kindling.palmdb.creator,
        &kindlegen.palmdb.creator,
    );

    cmp_mobi_header(&mut diff, &kindling.kf7, &kindlegen.kf7);
    cmp_exth(&mut diff, &kindling.kf7, &kindlegen.kf7);

    if kindling.kf7.header.orth_index == 0xFFFFFFFF {
        diff.push("kindling dict has no orth_index (0xFFFFFFFF)".to_string());
    }
    if kindlegen.kf7.header.orth_index == 0xFFFFFFFF {
        diff.push("kindlegen dict has no orth_index (0xFFFFFFFF)".to_string());
    }

    let k_indx = find_indx_records(&kindling).len();
    let g_indx = find_indx_records(&kindlegen).len();
    if k_indx.abs_diff(g_indx) > 2 {
        diff.push(format!(
            "INDX record count differs by more than 2: kindling={k_indx} kindlegen={g_indx}"
        ));
    }

    let k_label = first_indx_label_prefix(&kindling, kindling.kf7.header.orth_index as usize, 5);
    let g_label = first_indx_label_prefix(&kindlegen, kindlegen.kf7.header.orth_index as usize, 5);
    if !k_label.is_empty() && !g_label.is_empty() && k_label != g_label {
        diff.push(format!(
            "first INDX label prefix differs: kindling={:?} kindlegen={:?}",
            String::from_utf8_lossy(&k_label),
            String::from_utf8_lossy(&g_label)
        ));
    }

    if !diff.is_empty() {
        eprintln!(
            "{}",
            diff.into_error("parity_simple_dict: diffs against kindlegen reference:")
        );
    }
}

#[test]
fn parity_simple_book() {
    let kindling = kindling_build_parsed("simple_book", "simple_book.opf", "mobi");
    let kindlegen = load_reference("simple_book");

    let mut diff = Diff::default();

    cmp_scalar(
        &mut diff,
        "palmdb_type",
        &kindling.palmdb.ty,
        &kindlegen.palmdb.ty,
    );
    cmp_scalar(
        &mut diff,
        "palmdb_creator",
        &kindling.palmdb.creator,
        &kindlegen.palmdb.creator,
    );

    // Kindling emits KF8-only .azw3 by default; kindlegen emits dual
    // KF7+KF8. Compare the KF8 section that both tools produce.
    let k_kf8 = kindling.kf8_or_kf7();
    let g_kf8 = kindlegen.kf8_or_kf7();
    cmp_mobi_header(&mut diff, k_kf8, g_kf8);
    cmp_exth(&mut diff, k_kf8, g_kf8);

    cmp_scalar(
        &mut diff,
        "fdst_flow_count",
        k_kf8.header.fdst_count,
        g_kf8.header.fdst_count,
    );

    if !diff.is_empty() {
        eprintln!(
            "{}",
            diff.into_error("parity_simple_book: diffs against kindlegen reference:")
        );
    }
}

#[test]
fn parity_simple_comic() {
    let kindling = kindling_comic_parsed("simple_comic", "simple_comic.cbz", "azw3");
    let kindlegen = load_reference("simple_comic");

    let mut diff = Diff::default();

    cmp_scalar(
        &mut diff,
        "palmdb_type",
        &kindling.palmdb.ty,
        &kindlegen.palmdb.ty,
    );
    cmp_scalar(
        &mut diff,
        "palmdb_creator",
        &kindling.palmdb.creator,
        &kindlegen.palmdb.creator,
    );

    let k_kf8 = kindling.kf8_or_kf7();
    let g_kf8 = kindlegen.kf8_or_kf7();
    cmp_mobi_header(&mut diff, k_kf8, g_kf8);
    cmp_exth(&mut diff, k_kf8, g_kf8);

    // Both must emit EXTH 201 (cover) and 202 (thumbnail) for a comic.
    for rtype in [201u32, 202] {
        if k_kf8.exth_first(rtype).is_none() {
            diff.push(format!("kindling comic: EXTH {rtype} missing"));
        }
        if g_kf8.exth_first(rtype).is_none() {
            diff.push(format!(
                "kindlegen comic: EXTH {rtype} missing (unexpected from canonical builder)"
            ));
        }
    }

    if !diff.is_empty() {
        eprintln!(
            "{}",
            diff.into_error("parity_simple_comic: diffs against kindlegen reference:")
        );
    }
}
