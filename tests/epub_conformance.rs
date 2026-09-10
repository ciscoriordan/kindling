//! EPUB 2.0.1 and EPUB 3.3 output conformance tests for `kindling::epub_build`.
//!
//! Two layers:
//!
//! * Always-on structural tests build EPUB2 / EPUB3-book / EPUB3-dictionary
//!   from the committed `clean_dict` / `clean_book` fixtures and assert the
//!   spec-defining structure: the package `version` attribute (`2.0` for
//!   EPUB 2.0.1, `3.0` for EPUB 3.3), required Dublin Core metadata, the NCX
//!   plus `spine toc` of EPUB 2.0.1 versus the nav document plus
//!   `dcterms:modified` of EPUB 3.3, the EPUB Dictionaries and Glossaries layer
//!   (`dc:type=dictionary`, the Search Key Map and its `properties`,
//!   `source-language`/`target-language`, `epub:type` semantics), and the OCF
//!   invariant (`mimetype` first and STORED). These need no external tools.
//!
//! * A gated `#[ignore]` epubcheck round-trip builds each variant and runs
//!   epubcheck, asserting it validates under "EPUB version 2.0.1 rules" /
//!   "EPUB version 3.3 rules" with no errors. Run it with:
//!
//!   ```text
//!   cargo test --test epub_conformance -- --ignored
//!   ```
//!
//!   It requires `epubcheck` on `PATH` (override the command via the
//!   `KINDLING_EPUBCHECK` environment variable, e.g. when its Java runtime is
//!   not linked into `PATH`).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use kindling::epub_build::{DictMode, EpubMeta, build_epub2, build_epub3};
use kindling::opf::OPFData;

// ---------------------------------------------------------------------------
// Fixtures + output helpers
// ---------------------------------------------------------------------------

fn fixture_opf(name: &str, file: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
        .join(file)
}

fn dict_opf() -> OPFData {
    OPFData::parse(&fixture_opf("clean_dict", "clean_dict.opf")).expect("parse clean_dict OPF")
}

fn book_opf() -> OPFData {
    OPFData::parse(&fixture_opf("clean_book", "clean_book.opf")).expect("parse clean_book OPF")
}

fn out_path(tag: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "kindling_epub_conformance_{}_{}.epub",
        tag,
        std::process::id()
    ));
    p
}

/// A flat read of an output EPUB: ordered entry names, the first entry's name
/// and whether it is stored (uncompressed), and the text of the OPF/XML/XHTML
/// entries plus `mimetype`.
struct Epub {
    names: Vec<String>,
    first_name: String,
    first_stored: bool,
    files: HashMap<String, String>,
}

fn read_epub(path: &Path) -> Epub {
    let f = std::fs::File::open(path).expect("open output epub");
    let mut zip = zip::ZipArchive::new(f).expect("open zip");
    let mut names = Vec::new();
    let mut files = HashMap::new();
    let mut first_name = String::new();
    let mut first_stored = false;
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).expect("zip entry");
        let name = e.name().to_string();
        if i == 0 {
            first_name = name.clone();
            first_stored = e.compression() == zip::CompressionMethod::Stored;
        }
        if name == "mimetype"
            || name.ends_with(".opf")
            || name.ends_with(".xhtml")
            || name.ends_with(".xml")
        {
            let mut s = String::new();
            if e.read_to_string(&mut s).is_ok() {
                files.insert(name.clone(), s);
            }
        }
        names.push(name);
    }
    Epub {
        names,
        first_name,
        first_stored,
        files,
    }
}

impl Epub {
    fn opf(&self) -> &str {
        self.files
            .get("OEBPS/content.opf")
            .map(String::as_str)
            .unwrap_or("")
    }
    fn skm(&self) -> Option<&String> {
        self.files.get("OEBPS/skm.xml")
    }
    fn has_file(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }
    /// Content documents as `(zip entry name, text)`, in file order.
    fn named_content_docs(&self) -> Vec<(&String, &String)> {
        let mut v: Vec<(&String, &String)> = self
            .files
            .iter()
            .filter(|(k, _)| k.starts_with("OEBPS/content_") && k.ends_with(".xhtml"))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }
    fn content_docs(&self) -> Vec<&String> {
        let mut v: Vec<(&String, &String)> = self
            .files
            .iter()
            .filter(|(k, _)| k.starts_with("OEBPS/content_") && k.ends_with(".xhtml"))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v.into_iter().map(|(_, v)| v).collect()
    }
}

fn build_to<F>(tag: &str, f: F) -> Epub
where
    F: FnOnce(&Path),
{
    let out = out_path(tag);
    f(&out);
    let epub = read_epub(&out);
    let _ = std::fs::remove_file(&out);
    epub
}

// ---------------------------------------------------------------------------
// OCF container invariant (both EPUB 2.0.1 and EPUB 3.3)
// ---------------------------------------------------------------------------

#[test]
fn mimetype_is_first_and_stored() {
    let e = build_to("ocf", |p| {
        build_epub3(&dict_opf(), p, &EpubMeta::default()).unwrap();
    });
    assert_eq!(
        e.first_name, "mimetype",
        "mimetype must be the first zip entry"
    );
    assert!(e.first_stored, "mimetype must be STORED (uncompressed)");
    assert_eq!(
        e.files.get("mimetype").map(String::as_str),
        Some("application/epub+zip"),
        "mimetype content"
    );
    assert!(
        e.has_file("META-INF/container.xml"),
        "OCF requires META-INF/container.xml"
    );
}

// ---------------------------------------------------------------------------
// EPUB 2.0.1
// ---------------------------------------------------------------------------

#[test]
fn epub2_is_plain_2_0_1_even_from_dictionary_input() {
    let e = build_to("e2dict", |p| {
        build_epub2(&dict_opf(), p, &EpubMeta::default()).unwrap();
    });
    let opf = e.opf();
    assert!(
        opf.contains("version=\"2.0\""),
        "EPUB 2.0.1 package version attribute must be 2.0:\n{opf}"
    );
    assert!(opf.contains("<dc:identifier"), "missing dc:identifier");
    assert!(opf.contains("<dc:title"), "missing dc:title");
    assert!(opf.contains("<dc:language"), "missing dc:language");
    assert!(
        opf.contains("application/x-dtbncx+xml"),
        "EPUB 2.0.1 needs an NCX manifest item"
    );
    assert!(
        opf.contains("<spine toc=\"ncx\""),
        "EPUB 2.0.1 spine must reference the NCX via toc"
    );
    assert!(e.has_file("OEBPS/toc.ncx"), "missing toc.ncx");
    // Plain book: the dictionary input's semantics must be dropped.
    assert!(
        !opf.contains("dictionary"),
        "epub2 output must never be a dictionary:\n{opf}"
    );
    assert!(
        e.skm().is_none(),
        "epub2 output must have no Search Key Map"
    );
    for c in e.content_docs() {
        assert!(!c.contains("idx:"), "idx markup leaked into epub2 content");
        assert!(
            !c.contains("epub:type=\"dictionary\""),
            "dictionary semantics leaked into epub2 content"
        );
    }
}

// ---------------------------------------------------------------------------
// Cross-document links (issue #55)
// ---------------------------------------------------------------------------
//
// The exporter renames every spine document to `content_NN.xhtml` and writes
// them flat into `OEBPS/`, so an href the source wrote as
// `../Text/notes.xhtml#n1` names a file the output does not contain.
// epubcheck rejects those as RSC-007 and a reader shows them as dead.
//
// The `footnote_links` fixture is built for this: its documents live in
// `Text/`, they link to each other through `../`, and they reuse the same
// fragment names in more than one document, so a resolver that keeps one
// table of fragments for the whole book sends a link to the wrong file rather
// than to nowhere, and that has to fail too.

fn footnote_opf() -> OPFData {
    OPFData::parse(&fixture_opf("footnote_links", "footnote_links.opf"))
        .expect("parse footnote_links OPF")
}

/// Every `href="..."` value in a document, in order.
fn hrefs_in(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = doc;
    while let Some(at) = rest.find("href=\"") {
        rest = &rest[at + 6..];
        match rest.find('"') {
            Some(end) => {
                out.push(rest[..end].to_string());
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out
}

/// Every `src="..."` value in a document, in order.
fn srcs_in(doc: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = doc;
    while let Some(at) = rest.find("src=\"") {
        rest = &rest[at + 5..];
        match rest.find('"') {
            Some(end) => {
                out.push(rest[..end].to_string());
                rest = &rest[end + 1..];
            }
            None => break,
        }
    }
    out
}

fn has_anchor(doc: &str, fragment: &str) -> bool {
    doc.contains(&format!("id=\"{fragment}\"")) || doc.contains(&format!("name=\"{fragment}\""))
}

/// No link may name a file the export does not contain, or a fragment the
/// target document does not have. Both are RSC-007 / RSC-012.
fn assert_every_link_lands(e: &Epub) {
    let docs = e.named_content_docs();
    for (name, text) in &docs {
        for href in hrefs_in(text) {
            if href.is_empty() || href.starts_with("http") {
                continue;
            }
            let (path, fragment) = match href.split_once('#') {
                Some((p, f)) => (p, Some(f)),
                None => (href.as_str(), None),
            };
            let target: &String = if path.is_empty() {
                text
            } else {
                assert!(
                    !path.contains('/'),
                    "{name}: href {href:?} still points outside OEBPS/"
                );
                let full = format!("OEBPS/{path}");
                e.files
                    .get(&full)
                    .unwrap_or_else(|| panic!("{name}: href {href:?} names a missing file"))
            };
            if let Some(f) = fragment {
                assert!(
                    has_anchor(target, f),
                    "{name}: href {href:?} names a fragment its target does not have"
                );
            }
        }
    }
}

#[test]
fn epub3_book_links_reach_the_exported_documents() {
    let e = build_to("e3links", |p| {
        build_epub3(&footnote_opf(), p, &EpubMeta::default()).unwrap();
    });
    assert_every_link_lands(&e);

    let docs = e.named_content_docs();
    assert_eq!(docs.len(), 3, "fixture has three spine documents");
    let (ch1, ch2, notes) = (docs[0].1, docs[1].1, docs[2].1);

    // Forward: each chapter's marker reaches its own note, and the two notes
    // are in the same file, so only the fragment tells them apart.
    assert!(
        ch1.contains("href=\"content_03.xhtml#n1\""),
        "ch1 marker:\n{ch1}"
    );
    assert!(
        ch2.contains("href=\"content_03.xhtml#n2\""),
        "ch2 marker:\n{ch2}"
    );
    // Back: `ref-1` exists in both chapters, so a back-link that resolves
    // fragments book-wide would send both notes to the same place.
    assert!(
        notes.contains("href=\"content_01.xhtml#ref-1\"")
            && notes.contains("href=\"content_02.xhtml#ref-1\""),
        "notes back-links:\n{notes}"
    );
}

#[test]
fn epub2_book_links_reach_the_exported_documents() {
    let e = build_to("e2links", |p| {
        build_epub2(&footnote_opf(), p, &EpubMeta::default()).unwrap();
    });
    assert_every_link_lands(&e);
    assert!(
        e.named_content_docs()[0]
            .1
            .contains("href=\"content_03.xhtml#n1\"")
    );
}

#[test]
fn links_that_cannot_land_are_handled_rather_than_left_dangling() {
    let e = build_to("e3deadlinks", |p| {
        build_epub3(&footnote_opf(), p, &EpubMeta::default()).unwrap();
    });
    let docs = e.named_content_docs();
    let (ch1, ch2, notes) = (docs[0].1, docs[1].1, docs[2].1);

    // A link to a document that is not in the spine keeps its text and loses
    // its href, the way an unplaceable dictionary cross-reference does.
    assert!(
        !notes.contains("missing.xhtml"),
        "a link to a file outside the spine should not survive:\n{notes}"
    );
    assert!(
        notes.contains("NOTES_MISSING_FILE"),
        "its link text should stay:\n{notes}"
    );
    assert!(
        !notes.contains("<a >"),
        "no empty attribute space left behind:\n{notes}"
    );

    // A fragment the export threw away (here a `<body id>`, whose element the
    // export does not keep) becomes a link to the top of that document.
    assert!(
        ch2.contains("href=\"content_01.xhtml\""),
        "body-id link should fall back to the document:\n{ch2}"
    );

    // Untouched: an external link, an href naming nothing at all, and a bare
    // fragment that resolves inside its own document.
    assert!(ch1.contains("href=\"https://example.org/\""));
    assert!(ch2.contains("href=\"\""));
    assert!(ch1.contains("href=\"#local\"") && ch2.contains("href=\"#local\""));
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------
//
// The exporter carried no image resources at all, so every `<img>` in an
// exported book pointed at a file the archive did not contain: no cover, no
// illustrations, and RSC-007 from epubcheck for each one.

#[test]
fn images_are_carried_into_the_archive_and_declared() {
    let e = build_to("e3images", |p| {
        build_epub3(&footnote_opf(), p, &EpubMeta::default()).unwrap();
    });
    assert!(
        e.names.iter().any(|n| n.starts_with("OEBPS/images/")),
        "no image reached the archive: {:?}",
        e.names
    );
    let opf = e.opf();
    assert!(
        opf.contains("media-type=\"image/jpeg\""),
        "image not declared in the manifest:\n{opf}"
    );
    // The fixture's only image is the book's cover, so EPUB3 must mark it.
    assert!(
        opf.contains("properties=\"cover-image\""),
        "cover image not marked:\n{opf}"
    );
    // Every src now names a file the archive holds.
    for (name, text) in e.named_content_docs() {
        for src in srcs_in(text) {
            assert!(
                e.names.contains(&format!("OEBPS/{src}")),
                "{name}: img src {src:?} names a missing file"
            );
        }
    }
}

#[test]
fn epub2_names_its_cover_through_the_metadata() {
    // EPUB 2.0.1 has no cover-image property; the cover is a manifest id
    // named from a <meta name="cover">.
    let e = build_to("e2images", |p| {
        build_epub2(&footnote_opf(), p, &EpubMeta::default()).unwrap();
    });
    let opf = e.opf();
    assert!(
        opf.contains("<meta name=\"cover\" content=\"img_01\"/>"),
        "EPUB2 cover meta missing:\n{opf}"
    );
    assert!(
        !opf.contains("cover-image"),
        "properties=\"cover-image\" is EPUB3 only:\n{opf}"
    );
}

#[test]
fn epub2_from_plain_book_fixture() {
    let e = build_to("e2book", |p| {
        build_epub2(&book_opf(), p, &EpubMeta::default()).unwrap();
    });
    assert!(e.opf().contains("version=\"2.0\""));
    assert!(e.has_file("OEBPS/toc.ncx"));
    assert!(e.skm().is_none());
}

// ---------------------------------------------------------------------------
// EPUB 3.3 (generic book)
// ---------------------------------------------------------------------------

#[test]
fn epub3_book_from_plain_book_fixture() {
    let e = build_to("e3book", |p| {
        build_epub3(&book_opf(), p, &EpubMeta::default()).unwrap();
    });
    let opf = e.opf();
    assert!(
        opf.contains("version=\"3.0\""),
        "EPUB 3.3 package version attribute must be 3.0:\n{opf}"
    );
    assert!(
        opf.contains("dcterms:modified"),
        "EPUB 3.3 requires a dcterms:modified meta"
    );
    assert!(
        opf.contains("properties=\"nav\""),
        "EPUB 3.3 requires a nav document declared with properties=nav"
    );
    assert!(e.has_file("OEBPS/nav.xhtml"), "missing nav.xhtml");
    assert!(
        !opf.contains("<dc:type>dictionary</dc:type>"),
        "a plain book must not declare dc:type=dictionary"
    );
    assert!(
        e.skm().is_none(),
        "a plain book must have no Search Key Map"
    );
}

// ---------------------------------------------------------------------------
// EPUB 3.3 dictionary (EPUB Dictionaries and Glossaries profile)
// ---------------------------------------------------------------------------

#[test]
fn epub3_dictionary_auto_detected_from_xmetadata() {
    let e = build_to("e3dict", |p| {
        build_epub3(&dict_opf(), p, &EpubMeta::default()).unwrap();
    });
    let opf = e.opf();
    assert!(
        opf.contains("version=\"3.0\""),
        "EPUB 3.3 version attribute"
    );
    assert!(
        opf.contains("<dc:type>dictionary</dc:type>"),
        "auto-detected dictionary must declare dc:type=dictionary:\n{opf}"
    );
    assert!(
        opf.contains("source-language"),
        "dictionary needs source-language metadata"
    );
    assert!(
        opf.contains("target-language"),
        "dictionary needs target-language metadata"
    );
    assert!(
        opf.contains("application/vnd.epub.search-key-map+xml"),
        "missing Search Key Map media-type"
    );
    assert!(
        opf.contains("properties=\"search-key-map dictionary\""),
        "the SKM item must carry both the search-key-map and dictionary properties"
    );
    // `properties="dictionary"` is invalid on an XHTML item.
    assert!(
        !opf.contains("application/xhtml+xml\" properties=\"dictionary\""),
        "the dictionary property must not appear on an xhtml content item"
    );

    let skm = e.skm().expect("a dictionary must ship exactly one skm.xml");
    assert!(skm.contains("<search-key-map"), "skm root element missing");
    assert!(
        skm.contains("xml:lang"),
        "search-key-map root requires xml:lang"
    );
    assert!(skm.contains("<search-key-group"), "skm has no groups");
    assert!(skm.contains("<match "), "skm has no match elements");

    let docs = e.content_docs();
    assert!(!docs.is_empty(), "dictionary produced no content documents");
    assert!(
        docs.iter().any(|c| c.contains("epub:type=\"dictionary\"")),
        "no body carries epub:type=dictionary"
    );
    assert!(
        docs.iter().any(|c| c.contains("epub:type=\"dictentry\"")),
        "no entry carries epub:type=dictentry"
    );
}

#[test]
fn epub3_dictionary_has_one_search_key_group_per_entry() {
    let e = build_to("e3skm", |p| {
        build_epub3(&dict_opf(), p, &EpubMeta::default()).unwrap();
    });
    let skm = e.skm().expect("skm.xml");
    let groups = skm.matches("<search-key-group").count();
    let dictentries: usize = e
        .content_docs()
        .iter()
        .map(|c| c.matches("epub:type=\"dictentry\"").count())
        .sum();
    assert!(groups >= 1, "no search-key-groups emitted");
    assert_eq!(
        groups, dictentries,
        "exactly one search-key-group per dictentry"
    );
}

#[test]
fn epub3_book_mode_forces_plain_on_dictionary_input() {
    let meta = EpubMeta {
        dict_mode: DictMode::Book,
        ..Default::default()
    };
    let e = build_to("e3forcebook", |p| {
        build_epub3(&dict_opf(), p, &meta).unwrap();
    });
    assert!(
        !e.opf().contains("<dc:type>dictionary</dc:type>"),
        "DictMode::Book must not emit a dictionary even on dictionary input"
    );
    assert!(
        e.skm().is_none(),
        "DictMode::Book must not emit a Search Key Map"
    );
}

#[test]
fn epub3_dictionary_mode_overrides_opf_languages() {
    // clean_dict declares en->en; force grc->en and confirm the override wins.
    let meta = EpubMeta {
        dict_mode: DictMode::Dictionary {
            source: "grc".into(),
            target: "en".into(),
        },
        ..Default::default()
    };
    let e = build_to("e3override", |p| {
        build_epub3(&dict_opf(), p, &meta).unwrap();
    });
    let opf = e.opf();
    assert!(
        opf.contains("<dc:type>dictionary</dc:type>"),
        "forced dictionary not emitted"
    );
    assert!(
        opf.contains("<meta property=\"source-language\">grc</meta>"),
        "source-language override (grc) did not win over the OPF's en:\n{opf}"
    );
}

// ---------------------------------------------------------------------------
// Authoritative conformance via epubcheck (gated)
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires epubcheck on PATH; run with --ignored (override cmd via KINDLING_EPUBCHECK)"]
fn epubcheck_validates_all_variants() {
    let cmd = std::env::var("KINDLING_EPUBCHECK").unwrap_or_else(|_| "epubcheck".to_string());

    let e2 = out_path("ec2");
    build_epub2(&dict_opf(), &e2, &EpubMeta::default()).unwrap();
    assert_epubcheck(&cmd, &e2, "2.0.1");

    let e3b = out_path("ec3b");
    build_epub3(&book_opf(), &e3b, &EpubMeta::default()).unwrap();
    assert_epubcheck(&cmd, &e3b, "3.3");

    let e3d = out_path("ec3d");
    build_epub3(&dict_opf(), &e3d, &EpubMeta::default()).unwrap();
    assert_epubcheck(&cmd, &e3d, "3.3");

    // The clean fixtures have neither cross-document links nor images, so
    // they cannot catch a dangling href or a missing picture. footnote_links
    // has both, and epubcheck is the authority the issues were reported
    // against (RSC-007 for each of them).
    let e2l = out_path("ec2links");
    build_epub2(&footnote_opf(), &e2l, &EpubMeta::default()).unwrap();
    assert_epubcheck(&cmd, &e2l, "2.0.1");

    let e3l = out_path("ec3links");
    build_epub3(&footnote_opf(), &e3l, &EpubMeta::default()).unwrap();
    assert_epubcheck(&cmd, &e3l, "3.3");

    for p in [e2, e3b, e3d, e2l, e3l] {
        let _ = std::fs::remove_file(&p);
    }
}

fn assert_epubcheck(cmd: &str, epub: &Path, version: &str) {
    let output = std::process::Command::new(cmd)
        .arg(epub)
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "could not run epubcheck ({cmd}): {e}. Set KINDLING_EPUBCHECK and ensure its \
                 Java runtime is on PATH."
            )
        });
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains(&format!("EPUB version {version} rules")),
        "epubcheck did not validate {} under EPUB {version} rules:\n{combined}",
        epub.display()
    );
    assert!(
        combined.contains("No errors or warnings detected") || combined.contains("/ 0 errors"),
        "epubcheck reported errors for {}:\n{combined}",
        epub.display()
    );
}
