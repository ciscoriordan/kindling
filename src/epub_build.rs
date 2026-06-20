//! EPUB2 and EPUB3 output builders.
//!
//! Reads the same canonical input as the MOBI and StarDict builders (an OPF
//! plus per-letter content HTML) and emits a reflowable EPUB. Two entry
//! points are exposed:
//!
//! * [`build_epub2`] — a generic, reflowable EPUB 2.0.1 book (OPF 2.0.1,
//!   `<package version="2.0">` plus an NCX). The package `version` attribute is
//!   `2.0`; `2.0.1` is the spec revision the output conforms to (epubcheck
//!   reports "EPUB version 2.0.1 rules"). Never dictionary-aware: idx/x-metadata
//!   markup is stripped to a plain readable book.
//! * [`build_epub3`] — a generic EPUB 3.3 book (`<package version="3.0">` plus
//!   an EPUB3 navigation document) by default. The package `version` attribute
//!   is `3.0` for every EPUB 3.x; `3.3` is the spec revision the output conforms
//!   to (epubcheck reports "EPUB version 3.3 rules"). When the resolved
//!   [`DictMode`] is `Dictionary`, an extra layer from the EPUB Dictionaries and
//!   Glossaries spec (a profile built on top of EPUB 3.3, NOT part of EPUB 3.3
//!   core) is emitted: a single Search Key Map (`skm.xml`), `dc:type=dictionary`,
//!   source/target language metadata, and `epub:type` semantics in the content.
//!   The dictionary layer is EPUB3-only by design; there is no EPUB2 dictionary
//!   mode (an "EPUB2 dictionary" is not a standardized, interoperable format).
//!
//! Specs: EPUB 2.0.1 (IDPF OPF/OCF 2.0.1), EPUB 3.3 (W3C Recommendation,
//! <https://www.w3.org/TR/epub-33/>), and EPUB Dictionaries and Glossaries 1.1
//! (<https://www.w3.org/TR/epub-dictionaries/>) for the dictionary layer.
//!
//! Both builders write a spec-conformant EPUB zip: the `mimetype` entry first
//! and STORED (uncompressed), then `META-INF/container.xml`, then the
//! `OEBPS/*` payload deflated. The non-conformant
//! [`crate::epub::create_epub_from_dir`] (which does a plain deflate of every
//! entry, mimetype included) is deliberately not reused for output.
//!
//! Each dictionary entry body is re-parsed with `scraper` (html5ever) and
//! re-serialized as well-formed XHTML, because the Kindle idx-style source
//! HTML uses HTML5 conventions (void `<br>`/`<hr>`/`<img>` without a trailing
//! slash, single-quoted attributes, raw `&`) that epubcheck rejects in an
//! XHTML5 content document.

use std::fs;
use std::io::Write;
use std::path::Path;

use scraper::Node;

use crate::opf::{self, DictionaryEntry, OPFData};

/// Fallback dictionary title used when the OPF carries no `dc:title`.
const DEFAULT_TITLE: &str = "Dictionary";

/// Fixed `dcterms:modified` timestamp. The naming convention forbids dates in
/// any name a user or downstream system might see, but a build-info timestamp
/// is permitted as *content*. EPUB3 requires exactly one `dcterms:modified`
/// meta; a fixed value keeps output byte-stable across rebuilds.
const FIXED_MODIFIED: &str = "2026-06-20T00:00:00Z";

/// Resolution policy for the EPUB3 dictionary layer.
#[derive(Debug, Clone)]
pub enum DictMode {
    /// Emit a dictionary when the OPF looks like one (either
    /// `OPFData::is_dictionary()` is true or `dc_types` contains
    /// `"dictionary"`); otherwise emit a plain book. Source/target languages
    /// come from `dict_in_language` / `dict_out_language`.
    Auto,
    /// Force a plain book even when the input carries dictionary markup.
    Book,
    /// Force a dictionary with the given source/target language codes,
    /// overriding `Auto` detection and the OPF's own language fields.
    Dictionary { source: String, target: String },
}

impl Default for DictMode {
    fn default() -> Self {
        DictMode::Auto
    }
}

/// Caller overrides for EPUB metadata. Fields left as `None` fall back to the
/// OPF (`title` <- `dc:title`, `author` <- `dc:creator`, `identifier` <-
/// `dc:identifier`). `dict_mode` selects the dictionary layer for EPUB3.
#[derive(Debug, Clone, Default)]
pub struct EpubMeta {
    pub title: Option<String>,
    pub author: Option<String>,
    pub identifier: Option<String>,
    pub dict_mode: DictMode,
}

/// The resolved dictionary decision after applying [`DictMode`] against an OPF.
enum ResolvedMode {
    Book,
    Dictionary { source: String, target: String },
}

impl EpubMeta {
    /// Resolve [`DictMode`] against the OPF. `Auto` consults
    /// `OPFData::is_dictionary()` and `dc_types`. `Book` always returns
    /// `Book`. `Dictionary{..}` always returns a dictionary with the supplied
    /// languages.
    fn resolve_mode(&self, opf: &OPFData) -> ResolvedMode {
        match &self.dict_mode {
            DictMode::Book => ResolvedMode::Book,
            DictMode::Dictionary { source, target } => ResolvedMode::Dictionary {
                source: source.clone(),
                target: target.clone(),
            },
            DictMode::Auto => {
                let looks_like_dict = opf.is_dictionary()
                    || opf
                        .dc_types
                        .iter()
                        .any(|t| t.eq_ignore_ascii_case("dictionary"));
                if looks_like_dict {
                    ResolvedMode::Dictionary {
                        source: opf.dict_in_language.clone(),
                        target: opf.dict_out_language.clone(),
                    }
                } else {
                    ResolvedMode::Book
                }
            }
        }
    }

    fn effective_title(&self, opf: &OPFData) -> String {
        self.title
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if opf.title.is_empty() {
                    None
                } else {
                    Some(opf.title.clone())
                }
            })
            .unwrap_or_else(|| DEFAULT_TITLE.to_string())
    }

    fn effective_author(&self, opf: &OPFData) -> String {
        self.author
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if opf.author.is_empty() {
                    crate::DEFAULT_AUTHOR.to_string()
                } else {
                    opf.author.clone()
                }
            })
    }

    /// Stable `urn:uuid:` identifier. Derived deterministically from the OPF
    /// identifier (or title) so rebuilds are byte-stable and two different
    /// dictionaries do not collide, while never embedding a date.
    fn effective_identifier(&self, opf: &OPFData) -> String {
        let seed = self
            .identifier
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                if opf.identifier.is_empty() {
                    None
                } else {
                    Some(opf.identifier.clone())
                }
            })
            .unwrap_or_else(|| self.effective_title(opf));
        format!("urn:uuid:{}", uuid_from_seed(&seed))
    }
}

// ---------------------------------------------------------------------------
// Public builders
// ---------------------------------------------------------------------------

/// Build a generic, reflowable EPUB2 book at `out_path`. Always a plain book:
/// dictionary markup in the input is stripped to readable XHTML and the
/// `dict_mode` field of `meta` is ignored.
pub fn build_epub2(
    opf: &OPFData,
    out_path: &Path,
    meta: &EpubMeta,
) -> Result<(), Box<dyn std::error::Error>> {
    let title = meta.effective_title(opf);
    let author = meta.effective_author(opf);
    let identifier = meta.effective_identifier(opf);
    let language = if opf.language.is_empty() {
        "en".to_string()
    } else {
        opf.language.clone()
    };

    let docs = build_book_documents(opf, &title, &language)?;
    if docs.is_empty() {
        return Err("No content documents found for EPUB output".into());
    }

    let mut files: Vec<ZipEntry> = Vec::new();

    // content.opf (EPUB2)
    let opf_xml = render_epub2_opf(&title, &author, &identifier, &language, &docs);
    files.push(ZipEntry::text("OEBPS/content.opf", opf_xml));

    // toc.ncx
    let ncx_xml = render_ncx(&title, &identifier, &docs);
    files.push(ZipEntry::text("OEBPS/toc.ncx", ncx_xml));

    // content documents
    for d in &docs {
        files.push(ZipEntry::text(
            format!("OEBPS/{}", d.filename),
            d.xhtml.clone(),
        ));
    }

    write_epub(out_path, &files)?;
    Ok(())
}

/// Build a generic EPUB3 book at `out_path`. When the resolved [`DictMode`]
/// is a dictionary, the W3C EPUB-Dictionaries layer (Search Key Map,
/// `dc:type=dictionary`, source/target language metadata, `epub:type`
/// semantics) is emitted on top of the generic-book packaging.
pub fn build_epub3(
    opf: &OPFData,
    out_path: &Path,
    meta: &EpubMeta,
) -> Result<(), Box<dyn std::error::Error>> {
    match meta.resolve_mode(opf) {
        ResolvedMode::Dictionary { source, target } => {
            build_epub3_dictionary(opf, out_path, meta, &source, &target)
        }
        ResolvedMode::Book => build_epub3_book(opf, out_path, meta),
    }
}

// ---------------------------------------------------------------------------
// EPUB3 generic book
// ---------------------------------------------------------------------------

fn build_epub3_book(
    opf: &OPFData,
    out_path: &Path,
    meta: &EpubMeta,
) -> Result<(), Box<dyn std::error::Error>> {
    let title = meta.effective_title(opf);
    let author = meta.effective_author(opf);
    let identifier = meta.effective_identifier(opf);
    let language = if opf.language.is_empty() {
        "en".to_string()
    } else {
        opf.language.clone()
    };

    let docs = build_book_documents(opf, &title, &language)?;
    if docs.is_empty() {
        return Err("No content documents found for EPUB output".into());
    }

    let mut files: Vec<ZipEntry> = Vec::new();

    let nav_xhtml = render_nav(&title, &language, &docs);
    files.push(ZipEntry::text("OEBPS/nav.xhtml", nav_xhtml));

    let opf_xml = render_epub3_book_opf(&title, &author, &identifier, &language, &docs);
    files.push(ZipEntry::text("OEBPS/content.opf", opf_xml));

    for d in &docs {
        files.push(ZipEntry::text(
            format!("OEBPS/{}", d.filename),
            d.xhtml.clone(),
        ));
    }

    write_epub(out_path, &files)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// EPUB3 dictionary
// ---------------------------------------------------------------------------

/// One dictionary content document (`content_NN.xhtml`) plus the entries it
/// holds, retained so the Search Key Map can point at the right file/anchor.
struct DictDoc {
    filename: String,
    title: String,
    xhtml: String,
    entries: Vec<DictEntryOut>,
}

/// A single emitted dictionary entry: its stable anchor and searchable forms.
struct DictEntryOut {
    anchor: String,
    headword: String,
    inflections: Vec<String>,
}

fn build_epub3_dictionary(
    opf: &OPFData,
    out_path: &Path,
    meta: &EpubMeta,
    source: &str,
    target: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let title = meta.effective_title(opf);
    let author = meta.effective_author(opf);
    let identifier = meta.effective_identifier(opf);
    // Source language drives `xml:lang` on the content/skm. Fall back to the
    // OPF primary language, then "und" (undetermined) so the attribute is
    // never empty.
    let src_lang = if source.is_empty() {
        if opf.language.is_empty() {
            "und".to_string()
        } else {
            opf.language.clone()
        }
    } else {
        source.to_string()
    };
    let tgt_lang = if target.is_empty() {
        src_lang.clone()
    } else {
        target.to_string()
    };

    let html_paths = opf.get_content_html_paths();

    // Phase 1: parse every file, assign each entry a globally-unique anchor and
    // a stable output filename, and build a headword -> (filename, anchor) map
    // so cross-references can be rewritten to targets that actually exist in
    // the output. We keep the parsed entries around for phase 2.
    struct ParsedFile {
        filename: String,
        title: String,
        entries: Vec<(DictionaryEntry, String)>, // (entry, anchor)
    }
    let mut parsed: Vec<ParsedFile> = Vec::new();
    let mut seen_anchors: std::collections::HashSet<String> = std::collections::HashSet::new();
    // First writer wins: maps the sanitized source fragment key (the `hw_X`
    // form used in the input HTML, derived from the headword) to its target.
    let mut xref_map: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut file_index = 0usize;

    for html_path in &html_paths {
        let entries = opf::parse_dictionary_html(html_path)?;
        if entries.is_empty() {
            continue;
        }
        let mut kept: Vec<(DictionaryEntry, String)> = Vec::new();
        for e in entries {
            if e.headword.is_empty() {
                continue;
            }
            let anchor = unique_anchor(&e.headword, &mut seen_anchors);
            kept.push((e, anchor));
        }
        if kept.is_empty() {
            continue;
        }
        file_index += 1;
        let filename = format!("content_{:02}.xhtml", file_index);
        let doc_title = kept
            .iter()
            .map(|(e, _)| e.headword.clone())
            .next()
            .unwrap_or_else(|| title.clone());

        for (e, anchor) in &kept {
            // The input anchors entries by `hw_<headword>` (raw UTF-8). Build
            // the same fragment key so `content_NN.html#hw_<headword>` and
            // bare `#hw_<headword>` cross-references can be resolved. First
            // occurrence wins, matching the canonical-lemma convention.
            let frag_key = format!("hw_{}", e.headword);
            xref_map
                .entry(frag_key)
                .or_insert_with(|| (filename.clone(), anchor.clone()));
        }

        parsed.push(ParsedFile {
            filename,
            title: doc_title,
            entries: kept,
        });
    }

    if parsed.is_empty() {
        return Err("No dictionary entries found in HTML content files".into());
    }

    // Phase 2: emit each content document, rewriting cross-references against
    // the global map and collecting the per-entry search forms for the SKM.
    let mut docs: Vec<DictDoc> = Vec::new();
    for pf in &parsed {
        let mut body = String::new();
        let mut entry_outs: Vec<DictEntryOut> = Vec::new();
        for (e, anchor) in &pf.entries {
            let body_html =
                rewrite_crossrefs(&clean_entry_body(&e.html_content, &e.headword), &xref_map);
            body.push_str("    <article epub:type=\"dictentry\" id=\"");
            body.push_str(anchor);
            body.push_str("\"><dfn>");
            body.push_str(&xml_escape_text(&e.headword));
            body.push_str("</dfn>");
            body.push_str(&body_html);
            body.push_str("</article>\n");
            entry_outs.push(DictEntryOut {
                anchor: anchor.clone(),
                headword: e.headword.clone(),
                inflections: e.inflections.clone(),
            });
        }
        let xhtml = render_dict_content_doc(&pf.title, &src_lang, &body);
        docs.push(DictDoc {
            filename: pf.filename.clone(),
            title: pf.title.clone(),
            xhtml,
            entries: entry_outs,
        });
    }

    let mut files: Vec<ZipEntry> = Vec::new();

    let nav_xhtml = render_dict_nav(&title, &src_lang, &docs);
    files.push(ZipEntry::text("OEBPS/nav.xhtml", nav_xhtml));

    let skm_xml = render_skm(&src_lang, &docs);
    files.push(ZipEntry::text("OEBPS/skm.xml", skm_xml));

    let opf_xml = render_epub3_dict_opf(&title, &author, &identifier, &src_lang, &tgt_lang, &docs);
    files.push(ZipEntry::text("OEBPS/content.opf", opf_xml));

    for d in &docs {
        files.push(ZipEntry::text(
            format!("OEBPS/{}", d.filename),
            d.xhtml.clone(),
        ));
    }

    write_epub(out_path, &files)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Generic book content documents (used by EPUB2 and EPUB3 book modes)
// ---------------------------------------------------------------------------

/// One generic content document derived from a spine HTML file.
struct BookDoc {
    /// Output filename, e.g. `content_01.xhtml`.
    filename: String,
    /// Document `<title>` and TOC label.
    title: String,
    /// Manifest id, e.g. `content_01`.
    id: String,
    /// Well-formed XHTML5 document text.
    xhtml: String,
}

/// Turn the spine HTML files into plain, reflowable XHTML documents. Any
/// dictionary (idx) markup is stripped to a readable body; the result is a
/// generic book regardless of whether the input was a dictionary.
fn build_book_documents(
    opf: &OPFData,
    title: &str,
    language: &str,
) -> Result<Vec<BookDoc>, Box<dyn std::error::Error>> {
    let mut docs = Vec::new();
    let mut idx = 0usize;
    for html_path in opf.get_content_html_paths() {
        let raw = match fs::read_to_string(&html_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        idx += 1;
        let filename = format!("content_{:02}.xhtml", idx);
        let id = format!("content_{:02}", idx);

        // Prefer the source document's <title>; otherwise the book title.
        let doc_title = extract_title(&raw).unwrap_or_else(|| title.to_string());

        // Strip idx/mbp markup and re-serialize the body as well-formed XHTML.
        let body_source = extract_body(&raw);
        let cleaned = strip_dictionary_markup(&body_source);
        let serialized = serialize_fragment_xhtml(&cleaned);
        // Book mode has no dictionary anchoring, so internal `content_NN.html#hw_`
        // cross-references would dangle. Drop those hrefs (leaving the link
        // text) rather than emit references to resources that do not exist.
        let body = strip_internal_xref_hrefs(&serialized);

        let xhtml = render_book_content_doc(&doc_title, language, &body);
        docs.push(BookDoc {
            filename,
            title: doc_title,
            id,
            xhtml,
        });
    }
    Ok(docs)
}

// ---------------------------------------------------------------------------
// Rendering: OPF / NCX / nav
// ---------------------------------------------------------------------------

fn render_epub2_opf(
    title: &str,
    author: &str,
    identifier: &str,
    language: &str,
    docs: &[BookDoc],
) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\" unique-identifier=\"bookid\">\n",
    );
    s.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");
    s.push_str(&format!(
        "    <dc:identifier id=\"bookid\">{}</dc:identifier>\n",
        xml_escape_text(identifier)
    ));
    s.push_str(&format!(
        "    <dc:title>{}</dc:title>\n",
        xml_escape_text(title)
    ));
    s.push_str(&format!(
        "    <dc:language>{}</dc:language>\n",
        xml_escape_text(language)
    ));
    s.push_str(&format!(
        "    <dc:creator opf:role=\"aut\">{}</dc:creator>\n",
        xml_escape_text(author)
    ));
    s.push_str("  </metadata>\n");
    s.push_str("  <manifest>\n");
    s.push_str("    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");
    for d in docs {
        s.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            d.id, d.filename
        ));
    }
    s.push_str("  </manifest>\n");
    s.push_str("  <spine toc=\"ncx\">\n");
    for d in docs {
        s.push_str(&format!("    <itemref idref=\"{}\"/>\n", d.id));
    }
    s.push_str("  </spine>\n");
    s.push_str("</package>\n");
    s
}

fn render_ncx(title: &str, identifier: &str, docs: &[BookDoc]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str("<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n");
    s.push_str("  <head>\n");
    s.push_str(&format!(
        "    <meta name=\"dtb:uid\" content=\"{}\"/>\n",
        xml_escape_attr(identifier)
    ));
    s.push_str("    <meta name=\"dtb:depth\" content=\"1\"/>\n");
    s.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    s.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    s.push_str("  </head>\n");
    s.push_str(&format!(
        "  <docTitle><text>{}</text></docTitle>\n",
        xml_escape_text(title)
    ));
    s.push_str("  <navMap>\n");
    for (i, d) in docs.iter().enumerate() {
        s.push_str(&format!(
            "    <navPoint id=\"nav{}\" playOrder=\"{}\">\n",
            i + 1,
            i + 1
        ));
        s.push_str(&format!(
            "      <navLabel><text>{}</text></navLabel>\n",
            xml_escape_text(&d.title)
        ));
        s.push_str(&format!("      <content src=\"{}\"/>\n", d.filename));
        s.push_str("    </navPoint>\n");
    }
    s.push_str("  </navMap>\n");
    s.push_str("</ncx>\n");
    s
}

fn render_epub3_book_opf(
    title: &str,
    author: &str,
    identifier: &str,
    language: &str,
    docs: &[BookDoc],
) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{}\">\n",
        xml_escape_attr(language)
    ));
    s.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n");
    s.push_str(&format!(
        "    <dc:identifier id=\"bookid\">{}</dc:identifier>\n",
        xml_escape_text(identifier)
    ));
    s.push_str(&format!(
        "    <dc:title>{}</dc:title>\n",
        xml_escape_text(title)
    ));
    s.push_str(&format!(
        "    <dc:language>{}</dc:language>\n",
        xml_escape_text(language)
    ));
    s.push_str(&format!(
        "    <dc:creator>{}</dc:creator>\n",
        xml_escape_text(author)
    ));
    s.push_str(&format!(
        "    <meta property=\"dcterms:modified\">{}</meta>\n",
        FIXED_MODIFIED
    ));
    s.push_str("  </metadata>\n");
    s.push_str("  <manifest>\n");
    s.push_str("    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n");
    for d in docs {
        s.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            d.id, d.filename
        ));
    }
    s.push_str("  </manifest>\n");
    s.push_str("  <spine>\n");
    for d in docs {
        s.push_str(&format!("    <itemref idref=\"{}\"/>\n", d.id));
    }
    s.push_str("  </spine>\n");
    s.push_str("</package>\n");
    s
}

fn render_epub3_dict_opf(
    title: &str,
    author: &str,
    identifier: &str,
    source: &str,
    target: &str,
    docs: &[DictDoc],
) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"bookid\" xml:lang=\"{}\">\n",
        xml_escape_attr(source)
    ));
    s.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n");
    s.push_str(&format!(
        "    <dc:identifier id=\"bookid\">{}</dc:identifier>\n",
        xml_escape_text(identifier)
    ));
    s.push_str(&format!(
        "    <dc:title>{}</dc:title>\n",
        xml_escape_text(title)
    ));
    // Both languages declared as dc:language (epubcheck DICT profile requires
    // the languages declared here, not just in the source/target meta).
    s.push_str(&format!(
        "    <dc:language>{}</dc:language>\n",
        xml_escape_text(source)
    ));
    s.push_str(&format!(
        "    <dc:language>{}</dc:language>\n",
        xml_escape_text(target)
    ));
    s.push_str(&format!(
        "    <dc:creator>{}</dc:creator>\n",
        xml_escape_text(author)
    ));
    s.push_str("    <dc:type>dictionary</dc:type>\n");
    s.push_str(&format!(
        "    <meta property=\"dcterms:modified\">{}</meta>\n",
        FIXED_MODIFIED
    ));
    s.push_str(&format!(
        "    <meta property=\"source-language\">{}</meta>\n",
        xml_escape_text(source)
    ));
    s.push_str(&format!(
        "    <meta property=\"target-language\">{}</meta>\n",
        xml_escape_text(target)
    ));
    s.push_str("  </metadata>\n");
    s.push_str("  <manifest>\n");
    s.push_str("    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>\n");
    for (i, d) in docs.iter().enumerate() {
        s.push_str(&format!(
            "    <item id=\"content_{:02}\" href=\"{}\" media-type=\"application/xhtml+xml\"/>\n",
            i + 1,
            d.filename
        ));
    }
    // Exactly one Search Key Map for the whole dictionary. It carries both the
    // search-key-map and dictionary properties; the xhtml content items must
    // NOT carry properties="dictionary".
    s.push_str(
        "    <item id=\"skm\" href=\"skm.xml\" media-type=\"application/vnd.epub.search-key-map+xml\" properties=\"search-key-map dictionary\"/>\n",
    );
    s.push_str("  </manifest>\n");
    s.push_str("  <spine>\n");
    for (i, _d) in docs.iter().enumerate() {
        s.push_str(&format!("    <itemref idref=\"content_{:02}\"/>\n", i + 1));
    }
    s.push_str("  </spine>\n");
    s.push_str("</package>\n");
    s
}

fn render_nav(title: &str, language: &str, docs: &[BookDoc]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{0}\" lang=\"{0}\">\n",
        xml_escape_attr(language)
    ));
    s.push_str(&format!(
        "<head><title>{}</title></head>\n",
        xml_escape_text(title)
    ));
    s.push_str("<body>\n");
    s.push_str("  <nav epub:type=\"toc\" id=\"toc\">\n");
    s.push_str("    <ol>\n");
    for d in docs {
        s.push_str(&format!(
            "      <li><a href=\"{}\">{}</a></li>\n",
            d.filename,
            xml_escape_text(&d.title)
        ));
    }
    s.push_str("    </ol>\n");
    s.push_str("  </nav>\n");
    s.push_str("</body>\n");
    s.push_str("</html>\n");
    s
}

fn render_dict_nav(title: &str, language: &str, docs: &[DictDoc]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{0}\" lang=\"{0}\">\n",
        xml_escape_attr(language)
    ));
    s.push_str(&format!(
        "<head><title>{}</title></head>\n",
        xml_escape_text(title)
    ));
    s.push_str("<body>\n");
    s.push_str("  <nav epub:type=\"toc\" id=\"toc\">\n");
    s.push_str("    <ol>\n");
    for d in docs {
        s.push_str(&format!(
            "      <li><a href=\"{}\">{}</a></li>\n",
            d.filename,
            xml_escape_text(&d.title)
        ));
    }
    s.push_str("    </ol>\n");
    s.push_str("  </nav>\n");
    s.push_str("</body>\n");
    s.push_str("</html>\n");
    s
}

fn render_book_content_doc(title: &str, language: &str, body: &str) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"{0}\" lang=\"{0}\">\n",
        xml_escape_attr(language)
    ));
    s.push_str(&format!(
        "<head><title>{}</title></head>\n",
        xml_escape_text(title)
    ));
    // Wrap the body content in a single block-level <div>. EPUB2 is validated
    // against the XHTML 1.1 content model, where <body> requires block-level
    // children; loose inline content (text, <br/>, <b>, ...) emitted by lemma
    // entries would otherwise be rejected. A flow <div> accepts both.
    s.push_str("<body>\n<div>\n");
    s.push_str(body);
    s.push_str("\n</div>\n</body>\n");
    s.push_str("</html>\n");
    s
}

fn render_dict_content_doc(title: &str, language: &str, body: &str) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"{0}\" lang=\"{0}\">\n",
        xml_escape_attr(language)
    ));
    s.push_str(&format!(
        "<head><title>{}</title></head>\n",
        xml_escape_text(title)
    ));
    s.push_str("<body epub:type=\"dictionary\">\n");
    s.push_str(body);
    s.push_str("</body>\n");
    s.push_str("</html>\n");
    s
}

fn render_skm(language: &str, docs: &[DictDoc]) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    s.push_str(&format!(
        "<search-key-map xmlns=\"http://www.idpf.org/2007/ops\" xml:lang=\"{}\">\n",
        xml_escape_attr(language)
    ));
    for d in docs {
        for e in &d.entries {
            s.push_str(&format!(
                "  <search-key-group href=\"{}#{}\">\n",
                d.filename, e.anchor
            ));
            // One <match> per searchable form: the headword plus each
            // inflection. Dedup so a form that equals the headword (or repeats)
            // is not emitted twice.
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for value in std::iter::once(&e.headword).chain(e.inflections.iter()) {
                if value.is_empty() {
                    continue;
                }
                if seen.insert(value.clone()) {
                    s.push_str(&format!(
                        "    <match value=\"{}\"/>\n",
                        xml_escape_attr(value)
                    ));
                }
            }
            s.push_str("  </search-key-group>\n");
        }
    }
    s.push_str("</search-key-map>\n");
    s
}

// ---------------------------------------------------------------------------
// Entry body cleanup + XHTML serialization
// ---------------------------------------------------------------------------

/// Strip the idx/mbp wrapper from a dictionary entry's HTML, leaving the body,
/// then re-serialize as well-formed XHTML. The headword `<dfn>` is emitted by
/// the caller, so the leading `<idx:orth>` headword markup is dropped here to
/// avoid duplicating the headword inside the entry body.
fn clean_entry_body(html: &str, headword: &str) -> String {
    let stripped = strip_dictionary_markup(html);
    let serialized = serialize_fragment_xhtml(&stripped);
    if serialized.trim().is_empty() {
        format!("<p>{}</p>", xml_escape_text(headword))
    } else {
        serialized
    }
}

/// Rewrite the MOBI-style dictionary cross-references that lemma emits so they
/// resolve inside the EPUB.
///
/// The input HTML links between entries with `href="content_NN.html#hw_<word>"`
/// (inter-letter) or bare `href="#hw_<word>"` (same source page). Those target
/// filenames and the `.html` extension do not exist in the EPUB (we emit
/// `.xhtml`, renumber files, and assign deduplicated anchors). Each such href
/// is looked up in `xref_map` by its `hw_<word>` fragment key:
///
/// * found    -> rewritten to `href="<target_file>#<target_anchor>"`.
/// * unknown  -> the `href` attribute is dropped (leaving `<a>text</a>`), since
///   pointing at a missing resource fails epubcheck's DICT profile and a dead
///   anchor is worse than a non-link.
///
/// External hrefs (`http://`, `https://`, `mailto:`, etc.) and fragments that
/// are not `hw_`-prefixed pass through untouched.
fn rewrite_crossrefs(
    body: &str,
    xref_map: &std::collections::HashMap<String, (String, String)>,
) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    // Capture the optional per-letter file and the hw_ fragment word.
    static XREF: OnceLock<Regex> = OnceLock::new();
    let xref =
        XREF.get_or_init(|| Regex::new(r#"href="(?:content_\d+\.html)?#(hw_[^"]+)""#).unwrap());
    xref.replace_all(body, |caps: &regex::Captures| {
        let frag = caps.get(1).unwrap().as_str();
        // The fragment in the source is `hw_<headword>` (raw UTF-8). Our map is
        // keyed on the same form, so look it up directly.
        if let Some((file, anchor)) = xref_map.get(frag) {
            format!("href=\"{}#{}\"", file, anchor)
        } else {
            // Unknown target: drop the href so no dangling resource remains.
            // `<a>` with no href is valid XHTML; epubcheck only flags hrefs
            // that point at a missing resource/fragment.
            String::new()
        }
    })
    .into_owned()
}

/// Drop internal dictionary cross-reference hrefs (`content_NN.html#hw_…` and
/// bare `#hw_…`) entirely, leaving the link text. Used by the generic book
/// path, which has no dictionary anchors to point at. External and non-`hw_`
/// links are untouched.
fn strip_internal_xref_hrefs(body: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static XREF: OnceLock<Regex> = OnceLock::new();
    let xref =
        XREF.get_or_init(|| Regex::new(r#"\s*href="(?:content_\d+\.html)?#hw_[^"]+""#).unwrap());
    xref.replace_all(body, "").into_owned()
}

/// Remove Kindle-only `idx:*` and `mbp:*` markup from an HTML chunk so what
/// remains is plain renderable HTML. Inflection metadata and the orth headword
/// wrapper are dropped entirely (the headword is surfaced separately via
/// `<dfn>` / the Search Key Map). Self-closing orth headwords with no body are
/// also dropped. This is intentionally similar to stardict's `clean_entry_html`
/// but tuned for the EPUB layer: it does not synthesize a bold headword (the
/// caller owns the headword) and it does not rewrite cross-references to a
/// StarDict scheme.
fn strip_dictionary_markup(html: &str) -> String {
    use regex::Regex;
    use std::sync::OnceLock;
    static ENTRY: OnceLock<Regex> = OnceLock::new();
    static ORTH_BLOCK: OnceLock<Regex> = OnceLock::new();
    static ORTH_SELF: OnceLock<Regex> = OnceLock::new();
    static SHORT_OPEN: OnceLock<Regex> = OnceLock::new();
    static SHORT_CLOSE: OnceLock<Regex> = OnceLock::new();
    static INFL_BLOCK: OnceLock<Regex> = OnceLock::new();
    static IFORM: OnceLock<Regex> = OnceLock::new();
    static MBP: OnceLock<Regex> = OnceLock::new();

    let entry = ENTRY.get_or_init(|| Regex::new(r"(?s)</?idx:entry\b[^>]*>").unwrap());
    // Drop the whole <idx:orth>...</idx:orth> block (headword + inflections).
    let orth_block =
        ORTH_BLOCK.get_or_init(|| Regex::new(r"(?s)<idx:orth\b[^>]*>.*?</idx:orth>").unwrap());
    let orth_self = ORTH_SELF.get_or_init(|| Regex::new(r"<idx:orth\b[^>]*/>").unwrap());
    let short_open = SHORT_OPEN.get_or_init(|| Regex::new(r"<idx:short\b[^>]*>").unwrap());
    let short_close = SHORT_CLOSE.get_or_init(|| Regex::new(r"</idx:short>").unwrap());
    let infl_block =
        INFL_BLOCK.get_or_init(|| Regex::new(r"(?s)<idx:infl\b[^>]*>.*?</idx:infl>").unwrap());
    let iform = IFORM.get_or_init(|| Regex::new(r"<idx:iform\b[^>]*/?>").unwrap());
    let mbp = MBP.get_or_init(|| Regex::new(r"</?mbp:[a-zA-Z]+\b[^>]*/?>").unwrap());

    let mut s = html.to_string();
    s = entry.replace_all(&s, "").into_owned();
    s = orth_block.replace_all(&s, "").into_owned();
    s = orth_self.replace_all(&s, "").into_owned();
    s = infl_block.replace_all(&s, "").into_owned();
    s = iform.replace_all(&s, "").into_owned();
    s = short_open.replace_all(&s, "").into_owned();
    s = short_close.replace_all(&s, "").into_owned();
    s = mbp.replace_all(&s, "").into_owned();
    s
}

/// Parse an HTML fragment with html5ever (via `scraper`) and re-serialize it
/// as well-formed XHTML. This is the well-formedness guarantee for epubcheck:
/// void elements get a trailing slash, attributes are double-quoted, entities
/// are normalized, and any tag-soup is repaired by the parser. The fragment is
/// parsed in a `<body>` context, so the walk starts at the synthesized
/// `<html>` element's children.
fn serialize_fragment_xhtml(fragment: &str) -> String {
    let doc = scraper::Html::parse_fragment(fragment);
    let mut out = String::new();
    // parse_fragment wraps content in <html>; serialize that element's
    // descendants only (skipping the synthetic <html> wrapper itself).
    let root = doc.root_element();
    for child in root.children() {
        serialize_node(child, &mut out);
    }
    out
}

/// HTML void elements: serialized as self-closing `<tag/>` with no end tag.
fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Recursively serialize one ego_tree node as XHTML.
fn serialize_node(node: ego_tree::NodeRef<'_, Node>, out: &mut String) {
    match node.value() {
        Node::Text(t) => {
            out.push_str(&xml_escape_text(t));
        }
        Node::Element(el) => {
            let name = el.name();
            // html5ever may surface elements in non-html namespaces (e.g. an
            // un-namespaced idx leftover); use the local name verbatim.
            out.push('<');
            out.push_str(name);
            for (qn, value) in el.attrs.iter() {
                let attr_name = qn.local.as_ref();
                // Drop namespaced/colon attributes and the xmlns declarations
                // that html5ever may attach; they are not valid in our flat
                // XHTML body and the parser already resolved them.
                if attr_name.contains(':') || attr_name.starts_with("xmlns") {
                    continue;
                }
                out.push(' ');
                out.push_str(attr_name);
                out.push_str("=\"");
                out.push_str(&xml_escape_attr(value));
                out.push('"');
            }
            if is_void_element(name) {
                out.push_str("/>");
            } else {
                out.push('>');
                for child in node.children() {
                    serialize_node(child, out);
                }
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
        }
        // Comments, doctypes, processing instructions, document/fragment roots:
        // skip. They carry no renderable content for an entry body.
        _ => {
            for child in node.children() {
                serialize_node(child, out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Extract the `<title>` text from a source HTML document, if present.
fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title>")?;
    let start = open + "<title>".len();
    let close_rel = lower[start..].find("</title>")?;
    let raw = &html[start..start + close_rel];
    let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() { None } else { Some(t) }
}

/// Return the inner HTML of `<body>...</body>` if present, else the whole
/// input (already body-only content).
fn extract_body(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    if let Some(open) = lower.find("<body") {
        if let Some(gt) = html[open..].find('>') {
            let body_start = open + gt + 1;
            if let Some(close_rel) = lower[body_start..].find("</body>") {
                return html[body_start..body_start + close_rel].to_string();
            }
            return html[body_start..].to_string();
        }
    }
    html.to_string()
}

/// Build a stable, unique XML id anchor from a headword. The headword is
/// sanitized to NCName-safe characters (letters, digits, `-`, `_`, `.`),
/// prefixed with `hw_` so it always starts with a valid name-start character,
/// and disambiguated with a numeric suffix on collision.
fn unique_anchor(headword: &str, seen: &mut std::collections::HashSet<String>) -> String {
    let mut base = String::from("hw_");
    for ch in headword.chars() {
        if ch.is_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            base.push(ch);
        } else {
            // Encode other characters as their codepoint so distinct headwords
            // never collapse to the same anchor.
            base.push('_');
            base.push_str(&format!("{:x}", ch as u32));
        }
    }
    if seen.insert(base.clone()) {
        return base;
    }
    let mut n = 2;
    loop {
        let candidate = format!("{}_{}", base, n);
        if seen.insert(candidate.clone()) {
            return candidate;
        }
        n += 1;
    }
}

/// Deterministic UUID (v4-shaped string) derived from a seed via FNV-1a so the
/// identifier is stable across rebuilds and free of any date. Not a real
/// random UUID, but a valid `urn:uuid` lexical form.
fn uuid_from_seed(seed: &str) -> String {
    // Two 64-bit FNV-1a hashes over the seed and the seed reversed give 128
    // bits to fill the UUID fields.
    let h1 = fnv1a_64(seed.as_bytes());
    let h2 = fnv1a_64(seed.bytes().rev().collect::<Vec<u8>>().as_slice());
    let b = [h1.to_be_bytes(), h2.to_be_bytes()].concat();
    // Set version (4) and variant (RFC 4122) bits.
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&b);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// XML-escape text content (`&`, `<`, `>`).
fn xml_escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// XML-escape an attribute value (adds `"` on top of the text set).
fn xml_escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// EPUB zip writer (mimetype first + STORED, container.xml, then OEBPS/*)
// ---------------------------------------------------------------------------

/// A file destined for the EPUB zip.
struct ZipEntry {
    name: String,
    data: Vec<u8>,
}

impl ZipEntry {
    fn text(name: impl Into<String>, text: String) -> Self {
        ZipEntry {
            name: name.into(),
            data: text.into_bytes(),
        }
    }
}

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"#;

/// Write a conformant EPUB zip: `mimetype` first and STORED, then
/// `META-INF/container.xml`, then every supplied OEBPS entry deflated.
fn write_epub(out_path: &Path, files: &[ZipEntry]) -> Result<(), Box<dyn std::error::Error>> {
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut zip = zip::ZipWriter::new(cursor);

        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        // 1) mimetype first, STORED, no extra fields.
        zip.start_file("mimetype", stored)?;
        zip.write_all(b"application/epub+zip")?;

        // 2) META-INF/container.xml, deflated.
        zip.start_file("META-INF/container.xml", deflate)?;
        zip.write_all(CONTAINER_XML.as_bytes())?;

        // 3) OEBPS payload, deflated.
        for f in files {
            zip.start_file(&f.name, deflate)?;
            zip.write_all(&f.data)?;
        }

        zip.finish()?;
    }

    if let Some(parent) = out_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(out_path, &buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_fragment_makes_void_elements_well_formed() {
        let frag = "<p>line<br>two<hr>end</p>";
        let out = serialize_fragment_xhtml(frag);
        assert!(out.contains("<br/>"), "br not self-closed: {}", out);
        assert!(out.contains("<hr/>"), "hr not self-closed: {}", out);
    }

    #[test]
    fn serialize_fragment_escapes_bare_ampersand() {
        let frag = "<p>a & b</p>";
        let out = serialize_fragment_xhtml(frag);
        assert!(out.contains("&amp;"), "bare ampersand not escaped: {}", out);
        assert!(!out.contains("a & b"), "raw ampersand survived: {}", out);
    }

    #[test]
    fn serialize_fragment_normalizes_single_quoted_attrs() {
        let frag = "<div class='def'>hi</div>";
        let out = serialize_fragment_xhtml(frag);
        assert!(
            out.contains("class=\"def\""),
            "attr not double-quoted: {}",
            out
        );
    }

    #[test]
    fn strip_dictionary_markup_drops_idx_and_keeps_body() {
        let raw = r#"<idx:entry name="default" scriptable="yes" id="hw_a">
<idx:short><idx:orth value="a"><b>a</b><idx:infl><idx:iform value="as"/></idx:infl></idx:orth></idx:short>
<div class='def'>1. body text</div>
</idx:entry>"#;
        let cleaned = strip_dictionary_markup(raw);
        assert!(!cleaned.contains("idx:"), "idx leaked: {}", cleaned);
        assert!(cleaned.contains("1. body text"), "body lost: {}", cleaned);
        // The orth headword block is dropped (caller emits <dfn>); the inner
        // <b>a</b> goes with it.
        assert!(
            !cleaned.contains("<b>a</b>"),
            "orth block not dropped: {}",
            cleaned
        );
    }

    #[test]
    fn clean_entry_body_synthesizes_when_empty() {
        let raw = r#"<idx:entry><idx:orth value="x"/></idx:entry>"#;
        let body = clean_entry_body(raw, "x");
        assert_eq!(body, "<p>x</p>");
    }

    #[test]
    fn unique_anchor_is_deterministic_and_unique() {
        let mut seen = std::collections::HashSet::new();
        let a = unique_anchor("α", &mut seen);
        let b = unique_anchor("α", &mut seen);
        assert_ne!(a, b, "collision not disambiguated: {} {}", a, b);
        assert!(a.starts_with("hw_"));
    }

    #[test]
    fn unique_anchor_sanitizes_non_ncname_chars() {
        let mut seen = std::collections::HashSet::new();
        let a = unique_anchor("a b/c", &mut seen);
        // No spaces or slashes survive.
        assert!(!a.contains(' '));
        assert!(!a.contains('/'));
        assert!(a.starts_with("hw_"));
    }

    #[test]
    fn uuid_from_seed_is_stable_and_v4_shaped() {
        let u1 = uuid_from_seed("LemmaGreekENEL");
        let u2 = uuid_from_seed("LemmaGreekENEL");
        assert_eq!(u1, u2, "uuid not stable");
        // 8-4-4-4-12 with version nibble 4.
        let parts: Vec<&str> = u1.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[2].chars().next(), Some('4'), "version nibble: {}", u1);
    }

    #[test]
    fn render_skm_has_one_group_per_entry_with_all_forms() {
        let docs = vec![DictDoc {
            filename: "content_01.xhtml".to_string(),
            title: "α".to_string(),
            xhtml: String::new(),
            entries: vec![DictEntryOut {
                anchor: "hw_a".to_string(),
                headword: "alpha".to_string(),
                inflections: vec!["alphas".to_string(), "alpha".to_string()],
            }],
        }];
        let skm = render_skm("el", &docs);
        // One group.
        assert_eq!(skm.matches("<search-key-group").count(), 1);
        // Headword + the one distinct inflection (the duplicate "alpha" is
        // deduped against the headword) = 2 matches.
        assert_eq!(skm.matches("<match ").count(), 2, "skm: {}", skm);
        assert!(skm.contains("href=\"content_01.xhtml#hw_a\""));
        assert!(skm.contains("value=\"alphas\""));
    }
}
