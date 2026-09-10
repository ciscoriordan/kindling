//! Internal link resolution, end to end (issue #50).
//!
//! An EPUB's footnote markers are ordinary hrefs, and neither Kindle format
//! follows an href: MOBI6 follows `filepos`, a byte offset into the merged
//! text blob, and KF8 follows `kindle:pos:fid:FFFF:off:OOOOOOOOOO`, a
//! fragment plus a base-32 byte offset inside it. These tests build the
//! `footnote_links` fixture and check that every link lands on the element
//! the source EPUB named.
//!
//! The fixture exists to make the failure modes distinguishable. Its three
//! documents deliberately reuse fragment names — `top`, `ref-1` and `local`
//! all appear in more than one — because a resolver that looks fragments up
//! in one table across the whole book sends every chapter's first footnote
//! to the same place and still looks plausible. Each assertion below names
//! the marker text it expects to find, so a wrong-but-valid offset fails
//! rather than passing.
//!
//! Chapter one carries an image, and it sits between a link and that link's
//! target on purpose. Rewriting an image `src` to a `recindex` shortens the
//! document by nine bytes here, so if that rewrite ever moves back to after
//! the offsets are measured, every target past the image is wrong by exactly
//! that much and `legacy_mobi_filepos_targets_land_on_the_named_element`
//! fails. Without the image the fixture cannot tell the two orderings apart.

mod common;

use common::*;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn fixture_dir(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Build the fixture, optionally as a legacy dual-format `.mobi`, and parse
/// the result.
///
/// `slot` names a directory of this test's own: cargo runs the tests in
/// this file on parallel threads, and a shared output directory means one
/// test deletes the file another is reading.
fn build_footnote_fixture(legacy: bool, slot: &str) -> ParsedMobi {
    let opf = fixture_dir("footnote_links").join("footnote_links.opf");
    let tmp = std::env::temp_dir().join("kindling_links").join(slot);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join(if legacy { "out.mobi" } else { "out.azw3" });

    let mut cmd = Command::new(kindling_bin());
    cmd.arg("build")
        .arg(&opf)
        .arg("-o")
        .arg(&out)
        .arg("--no-validate");
    if legacy {
        cmd.arg("--legacy-mobi");
    }
    let run = cmd.output().expect("failed to spawn kindling-cli");
    assert!(
        run.status.success(),
        "build failed: {:?}\n--stderr--\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let raw = fs::read(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

/// Decode a zero-padded base-32 value in Kindle's `0-9A-V` alphabet.
fn decode_base32(s: &str) -> usize {
    s.chars().fold(0usize, |acc, c| {
        let digit = match c {
            '0'..='9' => c as usize - '0' as usize,
            'A'..='V' => c as usize - 'A' as usize + 10,
            _ => panic!("{s:?} is not base-32"),
        };
        acc * 32 + digit
    })
}

/// The immediate text content of the element whose `<` is at `at`: the bytes
/// between that tag's `>` and the next `<`.
///
/// Every assertion below is written against this rather than a window of the
/// blob, because a window wide enough to hold an element is also wide enough
/// to reach the next one, and would accept an offset that is merely close.
fn element_text(blob: &str, at: usize) -> String {
    let tail = &blob[at..];
    let close = tail.find('>').map(|i| i + 1).unwrap_or(0);
    let end = tail[close..].find('<').map(|i| close + i).unwrap_or(close);
    tail[close..end].to_string()
}

/// The link's own text, which the fixture uses as its name.
fn link_label(blob: &str, at: usize) -> String {
    element_text(blob, at)
}

// ---------------------------------------------------------------------------
// MOBI6 / KF7: filepos
// ---------------------------------------------------------------------------

/// One rewritten link in the merged MOBI6 blob.
#[derive(Debug)]
struct Kf7Link {
    /// The link's own text, which the fixture uses as its name.
    label: String,
    /// Byte offset of the `<a` that carries it.
    tag_start: usize,
    /// Byte offset it points at, or `None` when it was left unresolved.
    target: Option<usize>,
}

/// Every `filepos` in the merged blob, in document order.
fn kf7_links(blob: &str) -> Vec<Kf7Link> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = blob[from..].find("filepos=") {
        let at = from + rel;
        let value = &blob[at + "filepos=".len()..at + "filepos=".len() + 10];
        let tag_start = blob[..at].rfind('<').expect("filepos outside a tag");
        out.push(Kf7Link {
            label: link_label(blob, tag_start),
            tag_start,
            target: value.parse::<usize>().ok(),
        });
        from = at + "filepos=".len() + 10;
    }
    out
}

#[test]
fn legacy_mobi_rewrites_every_internal_link_to_a_filepos() {
    let parsed = build_footnote_fixture(true, "rewrites");
    let kf7 = &parsed.kf7;
    let blob = extract_text_blob(&parsed, kf7);
    let blob = String::from_utf8(blob).expect("KF7 blob is UTF-8");

    // A MOBI6 reader ignores href entirely, so an internal link that still
    // has one is dead. Only the external link may keep it.
    let remaining: Vec<&str> = blob
        .match_indices("<a ")
        .filter_map(|(i, _)| {
            let tag_end = blob[i..].find('>').map(|e| i + e)?;
            let tag = &blob[i..tag_end];
            tag.contains("href=").then_some(tag)
        })
        .collect();
    assert_eq!(
        remaining,
        vec![r#"<a href="https://example.org/""#, "<a href=\"\""],
        "only the external link and the empty one should keep an href"
    );

    // Four cross-document fragment links, three same-document ones, one
    // whole-document link, one to another document's body id, and the two
    // the fixture makes unresolvable. The empty href is not among them.
    let links = kf7_links(&blob);
    assert_eq!(
        links.len(),
        11,
        "expected eleven rewritten links: {links:?}"
    );
}

#[test]
fn legacy_mobi_sends_a_document_link_to_the_start_of_that_document() {
    let parsed = build_footnote_fixture(true, "documentlinks");
    let kf7 = &parsed.kf7;
    let blob = String::from_utf8(extract_text_blob(&parsed, kf7)).unwrap();
    let links = kf7_links(&blob);

    let target = |label: &str| -> usize {
        links
            .iter()
            .find(|l| l.label == label)
            .unwrap_or_else(|| panic!("no link labelled {label:?}"))
            .target
            .unwrap_or_else(|| panic!("{label:?} did not resolve"))
    };

    // A fragment naming a document's `<body id>` is a link to the document.
    // The wrapper tags do not survive the merge, so there is no element to
    // point at, and killing the link loses a destination that is not in
    // doubt. It lands where the whole-document link lands.
    assert_eq!(
        target("CH2_TO_CH1_BODY"),
        target("CH2_TO_CH1_WHOLE"),
        "a link to ch1's body id should reach ch1"
    );

    // `href="#"` is the top of the document that wrote it, which is what
    // kindlegen does. Chapter two's own heading is what it must reach.
    let at = target("CH2_BARE_HASH");
    assert_eq!(element_text(&blob, at).trim(), "CH2_HEADING");
}

#[test]
fn legacy_mobi_filepos_targets_land_on_the_named_element() {
    let parsed = build_footnote_fixture(true, "targets");
    let kf7 = &parsed.kf7;
    let blob = String::from_utf8(extract_text_blob(&parsed, kf7)).unwrap();

    // Each link's target must contain the marker text named here. The two
    // `#local` links are the discriminating pair: both documents declare a
    // `local`, so a resolver with one global fragment table sends both to
    // chapter one's.
    let expected: &[(&str, &str)] = &[
        ("CH1_SAMEFILE_LINK", "CH1_LOCAL_TARGET"),
        ("CH2_SAMEFILE_LINK", "CH2_LOCAL_TARGET"),
        ("CH2_TO_CH1_WHOLE", "CH1_HEADING"),
    ];

    let links = kf7_links(&blob);
    for (label, marker) in expected {
        let hits: Vec<&Kf7Link> = links.iter().filter(|l| l.label == *label).collect();
        assert!(!hits.is_empty(), "no link labelled {label:?} in the blob");
        for link in hits {
            let at = link
                .target
                .unwrap_or_else(|| panic!("{label:?} did not resolve"));
            assert_eq!(
                &blob[at..at + 1],
                "<",
                "{label:?} points at byte {at}, which is not the start of an element"
            );
            // The marker has to be the text of the element the offset names,
            // not merely somewhere after it.
            let text = element_text(&blob, at);
            assert_eq!(
                text.trim(),
                *marker,
                "{label:?} points at {at}, whose element reads {text:?}"
            );
        }
    }

    // Footnotes are a round trip, and closing it is the strongest check
    // available: each chapter's marker points at its own note, and that
    // note's back-link points at the marker that sent the reader there.
    // Both chapters name their marker `ref-1` and both notes are reached by
    // a link labelled "[1]", so a resolver with one global fragment table
    // collapses all four onto chapter one and still produces live links.
    let markers: Vec<&Kf7Link> = links.iter().filter(|l| l.label == "[1]").collect();
    assert_eq!(markers.len(), 2, "expected two footnote markers");
    for (i, note) in [r#"<p id="n1""#, r#"<p id="n2""#].iter().enumerate() {
        let at = markers[i].target.expect("footnote marker did not resolve");
        assert!(
            blob[at..].starts_with(note),
            "footnote marker {i} reached {:?}, expected {note:?}",
            &blob[at..at + 60]
        );
    }
    for (label, marker) in [("NOTE_ONE_BACKLINK", 0), ("NOTE_TWO_BACKLINK", 1)] {
        let back = links
            .iter()
            .find(|l| l.label == label)
            .unwrap_or_else(|| panic!("no link labelled {label:?}"));
        assert_eq!(
            back.target,
            Some(markers[marker].tag_start),
            "{label:?} should point back at the marker in its own chapter"
        );
    }
}

#[test]
fn legacy_mobi_marks_a_link_it_cannot_resolve_as_dead() {
    let parsed = build_footnote_fixture(true, "dead");
    let kf7 = &parsed.kf7;
    let blob = String::from_utf8(extract_text_blob(&parsed, kf7)).unwrap();

    // A fragment no element declares and a document outside the spine.
    // Both get the same inert, same-width marker kindlegen writes, rather
    // than an offset that would jump somewhere arbitrary.
    let dead: Vec<String> = kf7_links(&blob)
        .into_iter()
        .filter(|l| l.target.is_none())
        .map(|l| l.label)
        .collect();
    assert_eq!(dead.len(), 2, "expected exactly two dead links: {dead:?}");
    assert!(dead.iter().any(|d| d == "CH1_DANGLING"), "{dead:?}");
    assert!(dead.iter().any(|d| d == "NOTES_MISSING_FILE"), "{dead:?}");
    assert_eq!(blob.matches("filepos=XXXXXXXXXX").count(), 2);
}

// ---------------------------------------------------------------------------
// KF8: kindle:pos
// ---------------------------------------------------------------------------

/// Byte offset just past the `<body ...>` tag in a reconstructed part —
/// where that part's fragment content begins, which is what a
/// `kindle:pos` offset counts from.
fn body_content_start(part: &str) -> usize {
    let open = part.find("<body").expect("reconstructed part has a body");
    part[open..].find('>').expect("unterminated body tag") + open + 1
}

#[test]
fn kf8_resolves_fragment_links_to_a_real_offset() {
    let parsed = build_footnote_fixture(false, "kf8_offsets");
    let parts = reconstruct_parts_from_mobi(&parsed).expect("reconstruct KF8 parts");
    let parts: Vec<String> = parts
        .into_iter()
        .map(|p| String::from_utf8(p).expect("KF8 part is UTF-8"))
        .collect();

    // A reader resolves `kindle:pos:fid:F:off:O` by starting at fragment
    // F's content and walking O bytes into it. Reconstructing the parts and
    // measuring from the body's first byte is that same arithmetic.
    // The two `#local` links are the discriminating pair here, as in the
    // MOBI6 test: both documents declare a `local`.
    let expected: &[(&str, &str)] = &[
        ("CH1_SAMEFILE_LINK", "CH1_LOCAL_TARGET"),
        ("CH2_SAMEFILE_LINK", "CH2_LOCAL_TARGET"),
    ];
    // Each note's back-link must reach the chapter that cites it, which the
    // fragment id alone settles.
    let backlinks: &[(&str, usize)] = &[("NOTE_ONE_BACKLINK", 0), ("NOTE_TWO_BACKLINK", 1)];

    let mut seen = 0usize;
    let mut backlinks_seen = 0usize;
    let mut note_targets: Vec<(usize, usize)> = Vec::new();
    for part in &parts {
        let mut from = 0usize;
        while let Some(rel) = part[from..].find("kindle:pos:fid:") {
            let at = from + rel;
            let fid = decode_base32(&part[at + 15..at + 19]);
            let off = decode_base32(&part[at + 24..at + 34]);
            let tag_start = part[..at].rfind('<').unwrap();
            let label = link_label(part, tag_start);

            let target = &parts[fid];
            let pos = body_content_start(target) + off;
            assert!(
                pos < target.len(),
                "{label:?} resolves past the end of document {fid}"
            );

            if let Some((_, marker)) = expected.iter().find(|(l, _)| *l == label) {
                assert_eq!(
                    &target[pos..pos + 1],
                    "<",
                    "{label:?} points at byte {pos} of document {fid}, \
                     which is not the start of an element"
                );
                let text = element_text(target, pos);
                assert_eq!(
                    text.trim(),
                    *marker,
                    "{label:?} resolved to {pos} in document {fid}, \
                     whose element reads {text:?}"
                );
                seen += 1;
            }
            if let Some((_, chapter)) = backlinks.iter().find(|(l, _)| *l == label) {
                assert_eq!(
                    fid, *chapter,
                    "{label:?} should land in document {chapter}, not {fid}"
                );
                assert!(
                    target[pos..].starts_with("<a "),
                    "{label:?} should reach the marker anchor, got {:?}",
                    &target[pos..(pos + 60).min(target.len())]
                );
                backlinks_seen += 1;
            }
            if label == "[1]" {
                note_targets.push((fid, pos));
            }
            from = at + 34;
        }
    }
    assert_eq!(seen, expected.len(), "not every expected link was found");
    assert_eq!(backlinks_seen, backlinks.len(), "a back-link was missing");

    // Same discriminator as the MOBI6 test: the two footnote markers share
    // a label and a fragment name but must reach different notes.
    assert_eq!(note_targets.len(), 2, "expected two footnote markers");
    for (i, expect) in [r#"<p id="n1""#, r#"<p id="n2""#].iter().enumerate() {
        let (fid, pos) = note_targets[i];
        assert!(
            parts[fid][pos..].starts_with(expect),
            "footnote marker {i} reached {:?}, expected {expect:?}",
            &parts[fid][pos..pos + 60]
        );
    }
}

#[test]
fn kf8_leaves_no_bare_fragment_links_behind() {
    let parsed = build_footnote_fixture(false, "kf8_bare");
    let kf8 = parsed.kf8_or_kf7();
    let blob = String::from_utf8(extract_text_blob(&parsed, kf8)).unwrap();

    // A KF8 reader cannot follow `href="#local"`: the document it was
    // written in no longer exists as a separate file.
    assert!(
        !blob.contains(r##"href="#"##),
        "a same-document fragment link survived into the KF8 text"
    );
    // The external link is the one href that should still be an href.
    assert!(blob.contains(r#"href="https://example.org/""#));
    // A whole-document link is offset zero, not a dead link.
    assert!(blob.contains("kindle:pos:fid:0000:off:0000000000"));
}

/// Build the cover-page fixture as a KF8 `.azw3` and parse it.
// ---------------------------------------------------------------------------
// Dictionary cross-references (issue #54)
// ---------------------------------------------------------------------------

fn build_dict_xrefs(slot: &str) -> ParsedMobi {
    let opf = fixture_dir("dict_xrefs").join("dict_xrefs.opf");
    // Its own directory per test: these run on parallel threads, and a
    // shared output path means one test deletes the file another is reading.
    let tmp = std::env::temp_dir().join("kindling_links").join(slot);
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.mobi");
    let run = Command::new(kindling_bin())
        .arg("build")
        .arg(&opf)
        .arg("-o")
        .arg(&out)
        .arg("--no-validate")
        .arg("--no-compress")
        .output()
        .expect("failed to spawn kindling-cli");
    assert!(
        run.status.success(),
        "build failed: {:?}\n--stderr--\n{}",
        run.status.code(),
        String::from_utf8_lossy(&run.stderr)
    );
    let raw = fs::read(&out).unwrap_or_else(|e| panic!("read {}: {e}", out.display()));
    parse_mobi_file(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", out.display()))
}

fn dict_blob(slot: &str) -> String {
    let parsed = build_dict_xrefs(slot);
    let kf7 = &parsed.kf7;
    String::from_utf8(extract_text_blob(&parsed, kf7)).expect("blob is UTF-8")
}

/// A cross-reference names a headword, and the anchor that headword was
/// written on does not survive: reader.dict puts `id="hw_<headword>"` on the
/// `<idx:entry>` element and the MOBI strip removes that element. The link
/// has to reach the entry anyway.
#[test]
fn a_dictionary_cross_reference_reaches_the_entry_it_names() {
    let blob = dict_blob("xref_entry");
    let links = kf7_links(&blob);

    for (label, expect) in [
        ("zeta", "<b>zeta</b>"),
        ("beta", "<b>beta</b>"),
        ("alpha", "<b>alpha</b>"),
    ] {
        let link = links
            .iter()
            .find(|l| l.label == label)
            .unwrap_or_else(|| panic!("no link labelled {label:?} in:\n{blob}"));
        let at = link
            .target
            .unwrap_or_else(|| panic!("{label} was left unresolved"));
        assert!(
            blob[at..].starts_with(expect),
            "{label} landed at {at} on {:?}, not on {expect}",
            &blob[at..(at + 40).min(blob.len())]
        );
    }
}

/// Both source files define `<a id="dup">`. A resolver with one book-wide
/// fragment table sends both `#dup` links to whichever it saw first, which
/// is a live link to the wrong definition. kindlegen resolves these per
/// file and so must kindling.
#[test]
fn a_bare_fragment_stays_in_the_file_that_wrote_it() {
    let blob = dict_blob("xref_perfile");
    let links = kf7_links(&blob);

    for (label, expect) in [
        ("TO_DUP_ONE", "DUPANCHOR_ONE"),
        ("TO_DUP_TWO", "DUPANCHOR_TWO"),
    ] {
        let link = links
            .iter()
            .find(|l| l.label == label)
            .unwrap_or_else(|| panic!("no link labelled {label:?} in:\n{blob}"));
        let at = link
            .target
            .unwrap_or_else(|| panic!("{label} was left unresolved"));
        assert_eq!(
            element_text(&blob, at),
            expect,
            "{label} landed at {at} on the wrong file's anchor"
        );
    }
}

/// A fragment naming a headword the dictionary does not have stays inert,
/// and keeps its width so nothing after it moves.
#[test]
fn a_cross_reference_to_a_missing_headword_stays_dead() {
    let blob = dict_blob("xref_dead");
    let dead = kf7_links(&blob)
        .into_iter()
        .find(|l| l.label == "DEAD")
        .expect("the fixture's dead link");
    assert!(dead.target.is_none(), "a missing headword must not resolve");
    assert!(
        blob.contains(&format!("filepos={}", "X".repeat(10))),
        "an unresolved link keeps the same-width inert marker"
    );
}

/// Every internal link is rewritten: a MOBI6 reader ignores href entirely,
/// so one left behind is dead however well-formed it looks.
#[test]
fn no_dictionary_cross_reference_keeps_its_href() {
    let blob = dict_blob("xref_href");
    assert!(
        !blob.contains("href="),
        "an href survived into the dictionary blob:\n{blob}"
    );
}

fn build_cover_fixture() -> ParsedMobi {
    let opf = fixture_dir("cover_page_links").join("cover_page_links.opf");
    let tmp = std::env::temp_dir()
        .join("kindling_links")
        .join("coverpage");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();
    let out = tmp.join("out.azw3");
    let run = Command::new(kindling_bin())
        .arg("build")
        .arg(&opf)
        .arg("-o")
        .arg(&out)
        .arg("--no-validate")
        .output()
        .expect("failed to spawn kindling-cli");
    assert!(
        run.status.success(),
        "build failed: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let raw = fs::read(&out).unwrap();
    parse_mobi_file(&raw).unwrap()
}

#[test]
fn kf8_sends_a_link_to_the_dropped_cover_page_to_the_start_of_the_book() {
    // The KF8 half drops a bare in-spine cover page, because the metadata
    // cover renders full-page. A table of contents that lists the cover then
    // points at a document the file does not contain. The destination is not
    // in doubt, so the link goes to the start of the book rather than being
    // written off as broken, which is what kindlegen does with it too.
    let parsed = build_cover_fixture();
    let kf8 = parsed.kf8_or_kf7();
    let blob = String::from_utf8(extract_text_blob(&parsed, kf8)).unwrap();

    assert!(
        !blob.contains(&"X".repeat(34)),
        "a link was written off as broken"
    );
    assert!(
        blob.contains(r#"<a href="kindle:pos:fid:0000:off:0000000000""#),
        "the cover link should reach the first document the file has"
    );

    // The control has to keep working: a link to a real chapter still lands
    // on the element it names, not at the start of the book.
    let parts: Vec<String> = reconstruct_parts_from_mobi(&parsed)
        .expect("reconstruct")
        .into_iter()
        .map(|p| String::from_utf8(p).unwrap())
        .collect();
    let at = blob.find("kindle:pos:fid:0001:off:").expect("control link");
    let off = decode_base32(&blob[at + 24..at + 34]);
    let target = &parts[1];
    let pos = body_content_start(target) + off;
    assert_eq!(element_text(target, pos).trim(), "CHAPTER_MARK");
}
