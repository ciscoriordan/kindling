// Section 5 TOC/NCX/NAV extras rules (R5.4 through R5.11).
//
// These are the epubcheck NAV_*/NCX_*/OPF_0*/OPF_050 rules that extend the
// existing R5.x navigation catalog:
//   R5.4  Pagebreak without page-list          (epubcheck NAV_003)
//   R5.5  Remote URL in nav or NCX             (epubcheck NAV_010)
//   R5.6  Nav TOC not in spine order           (epubcheck NAV_011)
//   R5.7  NCX dtb:uid mismatch                 (epubcheck NCX_001)
//   R5.8  NCX dtb:uid whitespace               (epubcheck NCX_004)
//   R5.9  NCX empty navPoint text              (epubcheck NCX_006)
//   R5.10 Guide reference not an OPS doc       (epubcheck OPF_032)
//   R5.11 Spine toc not an NCX                 (epubcheck OPF_050)

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::helpers::extract_attr;
use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct TocExtrasChecks;

impl Check for TocExtrasChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R5.4", "R5.5", "R5.6", "R5.7", "R5.8", "R5.9", "R5.10", "R5.11",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // Build href -> media-type map and href -> id map once.
        let href_to_media: HashMap<String, String> = opf
            .manifest
            .values()
            .map(|(href, mt)| (href.clone(), mt.clone()))
            .collect();

        // NAV doc: manifest item with properties containing "nav".
        let nav_href = find_nav_doc_href(epub);

        // NCX doc: manifest item with media-type application/x-dtbncx+xml.
        let ncx_href: Option<String> = opf
            .manifest
            .values()
            .find(|(_, mt)| mt == "application/x-dtbncx+xml")
            .map(|(href, _)| href.clone());

        let opf_text = fs::read_to_string(&epub.opf_path).unwrap_or_default();

        // R5.4: page-list required when content has epub:type="pagebreak".
        if let Some(ref nav) = nav_href {
            let nav_path = opf.base_dir.join(nav);
            if spine_has_pagebreak(epub) {
                if let Ok(nav_text) = fs::read_to_string(&nav_path) {
                    if !nav_has_page_list(&nav_text) {
                        report.emit_at(
                            "R5.4",
                            "",
                            Some(nav_path.clone()),
                            None,
                        );
                    }
                }
            }
        }

        // R5.5: remote links in nav or NCX.
        if let Some(ref nav) = nav_href {
            let nav_path = opf.base_dir.join(nav);
            if let Ok(nav_text) = fs::read_to_string(&nav_path) {
                for remote in find_remote_links(&nav_text) {
                    report.emit_at(
                        "R5.5",
                        format!("Nav document references '{}'.", remote),
                        Some(nav_path.clone()),
                        None,
                    );
                }
            }
        }
        if let Some(ref ncx) = ncx_href {
            let ncx_path = opf.base_dir.join(ncx);
            if let Ok(ncx_text) = fs::read_to_string(&ncx_path) {
                for remote in find_remote_links(&ncx_text) {
                    report.emit_at(
                        "R5.5",
                        format!("NCX references '{}'.", remote),
                        Some(ncx_path.clone()),
                        None,
                    );
                }
            }
        }

        // R5.6: nav TOC order matches spine order.
        if let Some(ref nav) = nav_href {
            let nav_path = opf.base_dir.join(nav);
            if let Ok(nav_text) = fs::read_to_string(&nav_path) {
                if let Some(msg) = nav_toc_out_of_spine_order(&nav_text, &opf.spine_items) {
                    report.emit_at("R5.6", msg, Some(nav_path.clone()), None);
                }
            }
        }

        // R5.7, R5.8: NCX dtb:uid must match OPF dc:identifier on unique-identifier.
        if let Some(ref ncx) = ncx_href {
            let ncx_path = opf.base_dir.join(ncx);
            if let Ok(ncx_text) = fs::read_to_string(&ncx_path) {
                if let Some(uid_raw) = extract_ncx_dtb_uid(&ncx_text) {
                    let uid_trimmed = uid_raw.trim();
                    if uid_trimmed.len() != uid_raw.len() {
                        report.emit_at(
                            "R5.8",
                            format!("dtb:uid is '{}'.", uid_raw),
                            Some(ncx_path.clone()),
                            None,
                        );
                    }
                    if let Some(opf_id) = opf_unique_identifier_value(&opf_text) {
                        if uid_trimmed != opf_id.trim() {
                            report.emit_at(
                                "R5.7",
                                format!(
                                    "NCX dtb:uid '{}' does not match OPF dc:identifier '{}'.",
                                    uid_trimmed, opf_id
                                ),
                                Some(ncx_path.clone()),
                                None,
                            );
                        }
                    }
                }

                // R5.9: empty navPoint <text> labels.
                for empty_line in find_empty_navpoint_text(&ncx_text) {
                    report.emit_at(
                        "R5.9",
                        "",
                        Some(ncx_path.clone()),
                        Some(empty_line),
                    );
                }
            }
        }

        // R5.10: guide reference target must exist in the manifest AND be xhtml+xml.
        for (href, _title) in parse_guide_references(&opf_text) {
            let file_part = strip_fragment(&href);
            if file_part.is_empty() {
                continue;
            }
            match href_to_media.get(&file_part) {
                None => {
                    // Already covered by R5.3.1 (missing from manifest). Do not
                    // double-fire R5.10 for the same condition.
                }
                Some(mt) => {
                    // Legacy Kindle books commonly put
                    // `<reference type="toc" href="toc.ncx"/>` in the guide
                    // and the mobi pipeline explicitly supports it. Accept
                    // NCX as a guide target even though epubcheck OPF_032
                    // considers it non-conformant.
                    if mt != "application/xhtml+xml"
                        && mt != "application/x-dtbncx+xml"
                    {
                        report.emit_at(
                            "R5.10",
                            format!(
                                "Guide reference '{}' has media-type '{}', not application/xhtml+xml.",
                                href, mt
                            ),
                            Some(epub.opf_path.clone()),
                            None,
                        );
                    }
                }
            }
        }

        // R5.11: <spine toc="X"> must name an NCX manifest item.
        if let Some(toc_id) = extract_spine_toc_idref(&opf_text) {
            match opf.manifest.get(&toc_id) {
                Some((_href, media_type)) => {
                    if media_type != "application/x-dtbncx+xml" {
                        report.emit_at(
                            "R5.11",
                            format!(
                                "spine toc='{}' targets manifest item with media-type '{}'.",
                                toc_id, media_type
                            ),
                            Some(epub.opf_path.clone()),
                            None,
                        );
                    }
                }
                None => {
                    report.emit_at(
                        "R5.11",
                        format!("spine toc='{}' does not match any manifest item.", toc_id),
                        Some(epub.opf_path.clone()),
                        None,
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// NAV discovery / pagebreak / page-list
// ---------------------------------------------------------------------------

/// Return the href of the manifest item whose properties contain "nav".
fn find_nav_doc_href(epub: &ExtractedEpub) -> Option<String> {
    let opf_text = fs::read_to_string(&epub.opf_path).ok()?;
    let mut rest = opf_text.as_str();
    while let Some(idx) = rest.find("<item") {
        rest = &rest[idx + "<item".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let properties = extract_attr(tag, "properties").unwrap_or_default();
        if properties.split_whitespace().any(|p| p == "nav") {
            if let Some(href) = extract_attr(tag, "href") {
                return Some(href);
            }
        }
        rest = &rest[end..];
    }
    None
}

/// True if any spine content document contains `epub:type="pagebreak"`.
fn spine_has_pagebreak(epub: &ExtractedEpub) -> bool {
    let opf = &epub.opf;
    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        if let Ok(text) = fs::read_to_string(&full) {
            if contains_pagebreak_epub_type(&text) {
                return true;
            }
        }
    }
    false
}

/// True if `text` has an `epub:type="pagebreak"` token anywhere.
fn contains_pagebreak_epub_type(text: &str) -> bool {
    // Match both the double and single-quoted forms, and tokenised values like
    // epub:type="front pagebreak".
    for needle in &["epub:type=\"", "epub:type='"] {
        let mut rest = text;
        while let Some(idx) = rest.find(needle) {
            rest = &rest[idx + needle.len()..];
            let quote = needle.chars().last().unwrap();
            if let Some(end) = rest.find(quote) {
                let value = &rest[..end];
                if value.split_whitespace().any(|t| t == "pagebreak") {
                    return true;
                }
                rest = &rest[end..];
            } else {
                break;
            }
        }
    }
    false
}

/// True if `nav_text` has a `<nav epub:type="page-list">` element.
fn nav_has_page_list(nav_text: &str) -> bool {
    let mut rest = nav_text;
    while let Some(idx) = rest.find("<nav") {
        rest = &rest[idx + "<nav".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        let value = extract_attr(tag, "epub:type").unwrap_or_default();
        if value.split_whitespace().any(|t| t == "page-list") {
            return true;
        }
        rest = &rest[end..];
    }
    false
}

// ---------------------------------------------------------------------------
// Remote links
// ---------------------------------------------------------------------------

/// Return every `href|src="http(s)://..."` URL found in `text`.
fn find_remote_links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for attr in &["href", "src"] {
        for quote in &["\"", "'"] {
            let needle = format!("{}={}", attr, quote);
            let mut rest = text;
            while let Some(idx) = rest.find(&needle) {
                rest = &rest[idx + needle.len()..];
                if let Some(end) = rest.find(*quote) {
                    let url = &rest[..end];
                    if url.starts_with("http://") || url.starts_with("https://") {
                        out.push(url.to_string());
                    }
                    rest = &rest[end..];
                } else {
                    break;
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Nav TOC order vs spine order
// ---------------------------------------------------------------------------

/// Parse the first `<nav epub:type="toc">` block and return file hrefs
/// (fragment stripped) in document order.
fn extract_nav_toc_hrefs(nav_text: &str) -> Vec<String> {
    let mut out = Vec::new();

    // Find the opening `<nav ... epub:type="toc">` tag.
    let mut rest = nav_text;
    let mut toc_body_start: Option<usize> = None;
    let mut offset = 0usize;
    while let Some(idx) = rest.find("<nav") {
        let abs_open_start = offset + idx;
        rest = &rest[idx + "<nav".len()..];
        offset = abs_open_start + "<nav".len();
        let Some(end) = rest.find('>') else { return out };
        let tag = &rest[..end];
        let epub_type = extract_attr(tag, "epub:type").unwrap_or_default();
        if epub_type.split_whitespace().any(|t| t == "toc") {
            toc_body_start = Some(offset + end + 1);
            break;
        }
        rest = &rest[end..];
        offset += end;
    }

    let Some(start) = toc_body_start else { return out };
    let remaining = &nav_text[start..];
    let Some(end_idx) = remaining.find("</nav>") else { return out };
    let body = &remaining[..end_idx];

    // Collect every <a href="..."> in document order.
    let mut body_rest = body;
    while let Some(idx) = body_rest.find("<a") {
        body_rest = &body_rest[idx + "<a".len()..];
        let Some(tag_end) = body_rest.find('>') else { break };
        let tag = &body_rest[..tag_end];
        if let Some(href) = extract_attr(tag, "href") {
            let file_part = strip_fragment(&href);
            if !file_part.is_empty() {
                out.push(file_part);
            }
        }
        body_rest = &body_rest[tag_end..];
    }

    out
}

/// Return an error message if the nav TOC hrefs are not in spine order.
fn nav_toc_out_of_spine_order(
    nav_text: &str,
    spine_items: &[(String, String)],
) -> Option<String> {
    let nav_hrefs = extract_nav_toc_hrefs(nav_text);
    if nav_hrefs.len() < 2 {
        return None;
    }

    // Map spine href -> position.
    let mut spine_pos: HashMap<String, usize> = HashMap::new();
    for (i, (_, href)) in spine_items.iter().enumerate() {
        spine_pos.entry(href.clone()).or_insert(i);
    }

    // For each nav href that is in the spine, collect its spine position.
    let mut positions: Vec<(usize, String)> = Vec::new();
    for h in &nav_hrefs {
        if let Some(&p) = spine_pos.get(h) {
            positions.push((p, h.clone()));
        }
    }
    if positions.len() < 2 {
        return None;
    }

    for window in positions.windows(2) {
        let (a, b) = (&window[0], &window[1]);
        if a.0 > b.0 {
            return Some(format!(
                "Nav TOC entry '{}' (spine pos {}) precedes '{}' (spine pos {}).",
                a.1, a.0, b.1, b.0
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// NCX parsing
// ---------------------------------------------------------------------------

/// Return the raw content attribute of the first `<meta name="dtb:uid"
/// content="...">` in an NCX document, without trimming.
fn extract_ncx_dtb_uid(ncx_text: &str) -> Option<String> {
    let mut rest = ncx_text;
    while let Some(idx) = rest.find("<meta") {
        rest = &rest[idx + "<meta".len()..];
        let Some(end) = rest.find('>') else { return None };
        let tag = &rest[..end];
        let name = extract_attr(tag, "name").unwrap_or_default();
        if name == "dtb:uid" {
            return extract_attr(tag, "content");
        }
        rest = &rest[end..];
    }
    None
}

/// Return 1-based line numbers of every navPoint whose `<text>` child is empty.
fn find_empty_navpoint_text(ncx_text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    while let Some(idx) = ncx_text[offset..].find("<navPoint") {
        let abs_start = offset + idx;
        let close_needle = "</navPoint>";
        let body_end_rel = match ncx_text[abs_start..].find(close_needle) {
            Some(e) => abs_start + e,
            None => break,
        };
        let navpoint_body = &ncx_text[abs_start..body_end_rel];

        // Grab the first <text>...</text> pair. Accept both empty
        // `<text></text>` and self-closing `<text/>`.
        let mut empty = false;
        let mut text_line = line_of_offset(ncx_text, abs_start);
        if let Some(text_idx) = navpoint_body.find("<text") {
            let abs_text = abs_start + text_idx;
            text_line = line_of_offset(ncx_text, abs_text);
            let after_open = &ncx_text[abs_text + "<text".len()..];
            if let Some(tag_end) = after_open.find('>') {
                let open_tag = &after_open[..tag_end];
                if open_tag.trim_end().ends_with('/') {
                    empty = true;
                } else {
                    let rest_after_open = &after_open[tag_end + 1..];
                    if let Some(close_idx) = rest_after_open.find("</text>") {
                        let inner = &rest_after_open[..close_idx];
                        if inner.trim().is_empty() {
                            empty = true;
                        }
                    }
                }
            }
        } else {
            // navPoint with no <text> element at all is also empty.
            empty = true;
        }

        if empty {
            out.push(text_line);
        }

        offset = body_end_rel + close_needle.len();
    }
    out
}

// ---------------------------------------------------------------------------
// OPF helpers
// ---------------------------------------------------------------------------

/// Return `<spine toc="X">` idref value from raw OPF text.
fn extract_spine_toc_idref(opf_text: &str) -> Option<String> {
    let idx = opf_text.find("<spine")?;
    let rest = &opf_text[idx + "<spine".len()..];
    let end = rest.find('>')?;
    let tag = &rest[..end];
    extract_attr(tag, "toc")
}

/// Return the text content of the `<dc:identifier>` whose `id` attribute
/// matches the `unique-identifier` attribute on `<package>`.
fn opf_unique_identifier_value(opf_text: &str) -> Option<String> {
    // Find unique-identifier attribute on the <package ...> element.
    let pkg_idx = opf_text.find("<package")?;
    let pkg_rest = &opf_text[pkg_idx + "<package".len()..];
    let pkg_end = pkg_rest.find('>')?;
    let pkg_tag = &pkg_rest[..pkg_end];
    let unique_id = extract_attr(pkg_tag, "unique-identifier")?;

    // Find <dc:identifier id="<unique_id>">VALUE</dc:identifier>.
    let mut rest = opf_text;
    while let Some(idx) = rest.find("<dc:identifier") {
        rest = &rest[idx + "<dc:identifier".len()..];
        let Some(tag_end) = rest.find('>') else { return None };
        let tag = &rest[..tag_end];
        let id_attr = extract_attr(tag, "id").unwrap_or_default();
        if id_attr == unique_id {
            let after_open = &rest[tag_end + 1..];
            let close_idx = after_open.find("</dc:identifier>")?;
            return Some(after_open[..close_idx].to_string());
        }
        rest = &rest[tag_end..];
    }
    // Fallback: some OPFs use a stripped-namespace <identifier> tag.
    let mut rest = opf_text;
    while let Some(idx) = rest.find("<identifier") {
        rest = &rest[idx + "<identifier".len()..];
        let Some(tag_end) = rest.find('>') else { return None };
        let tag = &rest[..tag_end];
        let id_attr = extract_attr(tag, "id").unwrap_or_default();
        if id_attr == unique_id {
            let after_open = &rest[tag_end + 1..];
            let close_idx = after_open.find("</identifier>")?;
            return Some(after_open[..close_idx].to_string());
        }
        rest = &rest[tag_end..];
    }
    None
}

/// Parse OPF `<guide><reference href="..."/>` entries. Returns (href, title) pairs.
fn parse_guide_references(opf_text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = opf_text.to_ascii_lowercase();
    let guide_start = match lower.find("<guide") {
        Some(i) => i,
        None => return out,
    };
    let guide_end = lower[guide_start..]
        .find("</guide>")
        .map(|e| guide_start + e)
        .unwrap_or(opf_text.len());
    let guide_block = &opf_text[guide_start..guide_end];

    let mut rest = guide_block;
    while let Some(idx) = rest.find("<reference") {
        rest = &rest[idx + "<reference".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if let Some(href) = extract_attr(tag, "href") {
            let title = extract_attr(tag, "title").unwrap_or_default();
            out.push((href, title));
        }
        rest = &rest[end..];
    }
    out
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// Drop `#fragment` from an href.
fn strip_fragment(href: &str) -> String {
    match href.find('#') {
        Some(i) => href[..i].to_string(),
        None => href.to_string(),
    }
}

/// 1-based line number for a byte offset.
fn line_of_offset(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

#[allow(dead_code)]
fn _unused_path_check(_: &Path, _: &PathBuf) {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- R5.4 pagebreak / page-list ----

    #[test]
    fn r5_4_detects_pagebreak_epub_type() {
        let s = r#"<body><span epub:type="pagebreak" id="p1"></span></body>"#;
        assert!(contains_pagebreak_epub_type(s));
    }

    #[test]
    fn r5_4_detects_tokenised_pagebreak() {
        let s = r#"<span epub:type="front pagebreak"></span>"#;
        assert!(contains_pagebreak_epub_type(s));
    }

    #[test]
    fn r5_4_ignores_other_epub_types() {
        let s = r#"<section epub:type="chapter"></section>"#;
        assert!(!contains_pagebreak_epub_type(s));
    }

    #[test]
    fn r5_4_page_list_detected() {
        let nav = r#"<nav epub:type="page-list"><ol></ol></nav>"#;
        assert!(nav_has_page_list(nav));
    }

    #[test]
    fn r5_4_no_page_list_when_only_toc_nav() {
        let nav = r#"<nav epub:type="toc"><ol></ol></nav>"#;
        assert!(!nav_has_page_list(nav));
    }

    // ---- R5.5 remote links ----

    #[test]
    fn r5_5_finds_http_href() {
        let s = r#"<a href="http://example.com/foo">x</a>"#;
        let urls = find_remote_links(s);
        assert_eq!(urls, vec!["http://example.com/foo".to_string()]);
    }

    #[test]
    fn r5_5_finds_https_src() {
        let s = r#"<img src="https://cdn.example.com/a.png"/>"#;
        let urls = find_remote_links(s);
        assert_eq!(urls, vec!["https://cdn.example.com/a.png".to_string()]);
    }

    #[test]
    fn r5_5_ignores_relative_hrefs() {
        let s = r#"<a href="chapter1.xhtml">x</a>"#;
        assert!(find_remote_links(s).is_empty());
    }

    // ---- R5.6 nav order ----

    #[test]
    fn r5_6_in_order_is_clean() {
        let nav = r#"<nav epub:type="toc"><ol>
            <li><a href="ch1.xhtml">One</a></li>
            <li><a href="ch2.xhtml">Two</a></li>
            <li><a href="ch3.xhtml">Three</a></li>
        </ol></nav>"#;
        let spine = vec![
            ("id1".to_string(), "ch1.xhtml".to_string()),
            ("id2".to_string(), "ch2.xhtml".to_string()),
            ("id3".to_string(), "ch3.xhtml".to_string()),
        ];
        assert!(nav_toc_out_of_spine_order(nav, &spine).is_none());
    }

    #[test]
    fn r5_6_out_of_order_fires() {
        let nav = r#"<nav epub:type="toc"><ol>
            <li><a href="ch2.xhtml">Two</a></li>
            <li><a href="ch1.xhtml">One</a></li>
        </ol></nav>"#;
        let spine = vec![
            ("id1".to_string(), "ch1.xhtml".to_string()),
            ("id2".to_string(), "ch2.xhtml".to_string()),
        ];
        assert!(nav_toc_out_of_spine_order(nav, &spine).is_some());
    }

    #[test]
    fn r5_6_fragments_stripped() {
        let nav = r#"<nav epub:type="toc"><ol>
            <li><a href="ch1.xhtml#s1">A</a></li>
            <li><a href="ch1.xhtml#s2">B</a></li>
            <li><a href="ch2.xhtml">C</a></li>
        </ol></nav>"#;
        let spine = vec![
            ("id1".to_string(), "ch1.xhtml".to_string()),
            ("id2".to_string(), "ch2.xhtml".to_string()),
        ];
        assert!(nav_toc_out_of_spine_order(nav, &spine).is_none());
    }

    // ---- R5.7 / R5.8 dtb:uid ----

    #[test]
    fn r5_7_extract_dtb_uid() {
        let ncx = r#"<ncx><head><meta name="dtb:uid" content="bookuid-123"/></head></ncx>"#;
        assert_eq!(extract_ncx_dtb_uid(ncx).as_deref(), Some("bookuid-123"));
    }

    #[test]
    fn r5_7_opf_unique_identifier_value() {
        let opf = r#"<?xml version="1.0"?><package unique-identifier="BookId">
            <metadata><dc:identifier id="BookId">urn:uuid:abc-123</dc:identifier></metadata>
            </package>"#;
        assert_eq!(
            opf_unique_identifier_value(opf).as_deref(),
            Some("urn:uuid:abc-123")
        );
    }

    #[test]
    fn r5_7_missing_unique_identifier_attr() {
        let opf = r#"<package><metadata></metadata></package>"#;
        assert!(opf_unique_identifier_value(opf).is_none());
    }

    #[test]
    fn r5_8_whitespace_in_dtb_uid() {
        let s = "  bookid  ";
        assert_ne!(s.trim().len(), s.len());
    }

    // ---- R5.9 empty navPoint text ----

    #[test]
    fn r5_9_detects_empty_self_closing_text() {
        let ncx = "<navPoint>\n<navLabel><text/></navLabel>\n</navPoint>";
        let lines = find_empty_navpoint_text(ncx);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn r5_9_detects_empty_text_element() {
        let ncx = "<navPoint>\n<navLabel><text></text></navLabel>\n</navPoint>";
        let lines = find_empty_navpoint_text(ncx);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn r5_9_ignores_populated_text() {
        let ncx = "<navPoint>\n<navLabel><text>Chapter 1</text></navLabel>\n</navPoint>";
        assert!(find_empty_navpoint_text(ncx).is_empty());
    }

    // ---- R5.10 guide reference ----

    #[test]
    fn r5_10_parse_guide_references() {
        let opf = r#"<package><guide>
            <reference type="toc" title="TOC" href="toc.xhtml"/>
            <reference type="cover" href="cover.xhtml"/>
        </guide></package>"#;
        let refs = parse_guide_references(opf);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].0, "toc.xhtml");
        assert_eq!(refs[1].0, "cover.xhtml");
    }

    // ---- R5.11 spine toc ----

    #[test]
    fn r5_11_extract_spine_toc_idref() {
        let opf = r#"<package><spine toc="ncx"><itemref idref="ch1"/></spine></package>"#;
        assert_eq!(extract_spine_toc_idref(opf).as_deref(), Some("ncx"));
    }

    #[test]
    fn r5_11_no_toc_attribute() {
        let opf = r#"<package><spine><itemref idref="ch1"/></spine></package>"#;
        assert!(extract_spine_toc_idref(opf).is_none());
    }
}
