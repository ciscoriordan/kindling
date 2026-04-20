// Section 15: Kindle dictionary rules (Amazon-legacy KDP + epubcheck EPUB 3 DICT).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use regex::Regex;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::profile::Profile;
use crate::validate::ValidationReport;

pub struct DictChecks;

impl Check for DictChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R15.1", "R15.2", "R15.3", "R15.4", "R15.5", "R15.6", "R15.7",
            "R15.e1", "R15.e2", "R15.e3", "R15.e4", "R15.e5", "R15.e6", "R15.e7",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        if epub.profile != Profile::Dict {
            return;
        }
        run_legacy_rules(epub, report);
        run_epub3_rules(epub, report);
    }
}

/// R15.1-R15.7: Amazon-legacy KDP dictionary format, fires on any Dict profile.
fn run_legacy_rules(epub: &ExtractedEpub, report: &mut ValidationReport) {
    let opf = &epub.opf;

    // R15.1: <x-metadata><DictionaryInLanguage> present and BCP47-valid.
    if opf.dict_in_language.is_empty() {
        report.emit("R15.1", "Missing or empty <DictionaryInLanguage>.");
    } else if !is_valid_bcp47(&opf.dict_in_language) {
        report.emit(
            "R15.1",
            format!("Value '{}' is not a valid BCP47 code.", opf.dict_in_language),
        );
    }

    // R15.2: <x-metadata><DictionaryOutLanguage> present and BCP47-valid.
    if opf.dict_out_language.is_empty() {
        report.emit("R15.2", "Missing or empty <DictionaryOutLanguage>.");
    } else if !is_valid_bcp47(&opf.dict_out_language) {
        report.emit(
            "R15.2",
            format!("Value '{}' is not a valid BCP47 code.", opf.dict_out_language),
        );
    }

    // Scan all spine content once for idx:entry names, orth values, and mbp:frameset.
    let scan = scan_spine_content(epub);

    // R15.3: DefaultLookupIndex must match at least one idx:entry name="...".
    // Default when not specified in OPF is "default".
    let default_index = if opf.default_lookup_index.is_empty() {
        "default"
    } else {
        opf.default_lookup_index.as_str()
    };
    if !scan.idx_entry_names.is_empty() && !scan.idx_entry_names.contains(default_index) {
        report.emit(
            "R15.3",
            format!(
                "DefaultLookupIndex is '{}' but no <idx:entry name=\"{}\"> was found in spine \
                 content.",
                default_index, default_index
            ),
        );
    }

    // R15.4: At least one <idx:entry> element in spine content.
    if scan.idx_entry_count == 0 {
        report.emit("R15.4", "");
    }

    // R15.5: Spine content wrapped in <mbp:frameset> (warning).
    for missing in &scan.spine_files_without_frameset {
        report.emit_at(
            "R15.5",
            "",
            Some(PathBuf::from(missing)),
            None,
        );
    }

    // R15.6: Every <idx:orth> has a non-empty value="...".
    for (file, line) in &scan.empty_orth_locations {
        report.emit_at("R15.6", "", Some(PathBuf::from(file)), Some(*line));
    }

    // R15.7: OPF <guide> should contain a <reference type="index" .../>.
    if !guide_has_index_reference(&epub.opf_path) {
        report.emit("R15.7", "");
    }
}

/// R15.e1-R15.e7: epubcheck EPUB 3 DICT rules, gated on package_version == "3.0".
fn run_epub3_rules(epub: &ExtractedEpub, report: &mut ValidationReport) {
    if epub.opf.package_version != "3.0" {
        return;
    }
    let opf = &epub.opf;

    // R15.e1 (OPF_078): At least one content doc with epub:type="dictionary".
    let mut dict_typed_content_found = false;
    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        if let Ok(content) = fs::read_to_string(&full) {
            if content.contains("epub:type=\"dictionary\"")
                || content.contains("epub:type='dictionary'")
            {
                dict_typed_content_found = true;
                break;
            }
        }
    }
    if !dict_typed_content_found {
        report.emit("R15.e1", "");
    }

    // R15.e2 (OPF_079): if dict content found but <dc:type>dictionary</dc:type> missing.
    let has_dict_content = dict_typed_content_found
        || scan_spine_for_idx_entry(epub)
        || !opf.dict_in_language.is_empty();
    let declares_dict_type = opf
        .dc_types
        .iter()
        .any(|t| t.eq_ignore_ascii_case("dictionary"));
    if has_dict_content && !declares_dict_type {
        report.emit("R15.e2", "");
    }

    // Parse <collection> elements from the OPF for R15.e3-R15.e7.
    let collections = parse_dictionary_collections(&epub.opf_path);
    let manifest_hrefs: HashSet<String> = opf
        .manifest
        .values()
        .map(|(href, _)| href.clone())
        .collect();
    let href_to_media: HashMap<String, String> = opf
        .manifest
        .values()
        .map(|(href, mt)| (href.clone(), mt.clone()))
        .collect();

    for collection in &collections {
        let mut skm_count = 0usize;

        for link in &collection.links {
            // R15.e4 (OPF_081): each linked resource must be in the manifest.
            if !manifest_hrefs.contains(link) {
                report.emit(
                    "R15.e4",
                    format!("Collection references '{}' not in manifest.", link),
                );
                continue;
            }

            let media = href_to_media.get(link).cloned().unwrap_or_default();
            let is_xhtml = media == "application/xhtml+xml";
            let is_skm = media == "application/vnd.epub.search-key-map+xml"
                || link.to_lowercase().ends_with(".xml");

            if is_skm && !is_xhtml {
                skm_count += 1;
                // R15.e3 (OPF_080): SKM must use .xml extension.
                if !link.to_lowercase().ends_with(".xml") {
                    report.emit(
                        "R15.e3",
                        format!("Search Key Map '{}' does not use .xml extension.", link),
                    );
                }
            }

            // R15.e7 (OPF_084): only XHTML or SKM allowed in dict collection.
            if !is_xhtml && !is_skm {
                report.emit(
                    "R15.e7",
                    format!(
                        "Collection resource '{}' has media-type '{}', not XHTML or SKM.",
                        link, media
                    ),
                );
            }
        }

        // R15.e5 (OPF_082): at most one SKM per collection.
        if skm_count > 1 {
            report.emit(
                "R15.e5",
                format!("Collection contains {} Search Key Map documents.", skm_count),
            );
        }
        // R15.e6 (OPF_083): at least one SKM per collection.
        if skm_count == 0 {
            report.emit("R15.e6", "");
        }
    }
}

/// Aggregated findings from a single scan of every spine content file.
struct SpineScan {
    idx_entry_count: usize,
    idx_entry_names: HashSet<String>,
    spine_files_without_frameset: Vec<String>,
    empty_orth_locations: Vec<(String, usize)>,
}

/// Walk every spine file once and collect every dict-related finding.
fn scan_spine_content(epub: &ExtractedEpub) -> SpineScan {
    let opf = &epub.opf;
    let mut scan = SpineScan {
        idx_entry_count: 0,
        idx_entry_names: HashSet::new(),
        spine_files_without_frameset: Vec::new(),
        empty_orth_locations: Vec::new(),
    };

    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        let content = match fs::read_to_string(&full) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let entries_in_file = count_idx_entries(&content);
        scan.idx_entry_count += entries_in_file;

        for name in collect_idx_entry_names(&content) {
            scan.idx_entry_names.insert(name);
        }

        if entries_in_file > 0 && !content.contains("<mbp:frameset") {
            scan.spine_files_without_frameset.push(href.clone());
        }

        for line_no in find_empty_orth_lines(&content) {
            scan.empty_orth_locations.push((href.clone(), line_no));
        }
    }

    scan
}

/// Fast path for R15.e2: true if any spine file has an `idx:entry` marker.
fn scan_spine_for_idx_entry(epub: &ExtractedEpub) -> bool {
    let opf = &epub.opf;
    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        if let Ok(content) = fs::read_to_string(&full) {
            if content.contains("<idx:entry") {
                return true;
            }
        }
    }
    false
}

/// Count `<idx:entry` opening tags in a single HTML file.
fn count_idx_entries(content: &str) -> usize {
    content.matches("<idx:entry").count()
}

/// Collect every `name="..."` value from `<idx:entry>` open tags.
fn collect_idx_entry_names(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = content;
    while let Some(idx) = rest.find("<idx:entry") {
        rest = &rest[idx + "<idx:entry".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if let Some(name) = extract_dq_attr(tag, "name") {
            out.push(name);
        }
        rest = &rest[end..];
    }
    out
}

/// Find 1-based line numbers of every `<idx:orth>` that has no headword text.
///
/// An `<idx:orth>` is considered populated when it either has a non-empty
/// `value="..."` attribute (Kindle "attribute form") or a non-empty textual
/// body (Kindle "body form", e.g. `<idx:orth><b>foo</b></idx:orth>` that
/// PyGlossary emits). Only when both are empty do we flag R15.6, matching
/// kindlegen which accepted either style.
fn find_empty_orth_lines(content: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut byte_pos = 0usize;
    while let Some(idx) = content[byte_pos..].find("<idx:orth") {
        let abs = byte_pos + idx;
        let after_tag_name = abs + "<idx:orth".len();
        let end = match content[after_tag_name..].find('>') {
            Some(e) => after_tag_name + e,
            None => break,
        };
        let tag = &content[after_tag_name..end];
        let value = extract_dq_attr(tag, "value").unwrap_or_default();
        let self_closing = tag.ends_with('/');
        let body_has_text = if value.is_empty() && !self_closing {
            let body_start = end + 1;
            match content[body_start..].find("</idx:orth>") {
                Some(rel) => has_non_whitespace_text(&content[body_start..body_start + rel]),
                None => false,
            }
        } else {
            false
        };
        if value.is_empty() && !body_has_text {
            out.push(line_of(content, abs));
        }
        byte_pos = end + 1;
    }
    out
}

/// True if `body` contains any non-whitespace character outside of `<...>`
/// tag markup. Used to accept `<idx:orth><b>foo</b></idx:orth>` as populated.
fn has_non_whitespace_text(body: &str) -> bool {
    let mut in_tag = false;
    for ch in body.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() => return true,
            _ => {}
        }
    }
    false
}

/// Extract a double-quoted attribute value from an open-tag body.
fn extract_dq_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// 1-based line number of `byte_offset` inside `content`.
fn line_of(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

/// Conservative BCP47 check. Accepts ISO 639 primary tag with optional region.
fn is_valid_bcp47(tag: &str) -> bool {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^[a-z]{2,3}(-[A-Z][a-z]{3})?(-[A-Z]{2}|-\d{3})?(-[a-z0-9]+)*$").unwrap()
    });
    re.is_match(tag)
}

/// True if the raw OPF has `<reference type="index" ...>` inside `<guide>`.
fn guide_has_index_reference(opf_path: &std::path::Path) -> bool {
    let content = match fs::read_to_string(opf_path) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let lower = content.to_ascii_lowercase();
    let guide_start = match lower.find("<guide") {
        Some(i) => i,
        None => return false,
    };
    let guide_end = lower[guide_start..]
        .find("</guide>")
        .map(|e| guide_start + e)
        .unwrap_or(content.len());
    let guide_block = &lower[guide_start..guide_end];

    let mut rest = guide_block;
    while let Some(idx) = rest.find("<reference") {
        rest = &rest[idx + "<reference".len()..];
        let Some(end) = rest.find('>') else { return false };
        let tag = &rest[..end];
        if tag.contains("type=\"index\"") || tag.contains("type='index'") {
            return true;
        }
        rest = &rest[end..];
    }
    false
}

/// A single `<collection role="...">` block parsed from the OPF.
struct DictCollection {
    links: Vec<String>,
}

/// Parse every `<collection>` with `role` containing `dictionary` from the OPF.
fn parse_dictionary_collections(opf_path: &std::path::Path) -> Vec<DictCollection> {
    let content = match fs::read_to_string(opf_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut rest = content.as_str();
    while let Some(idx) = rest.find("<collection") {
        rest = &rest[idx..];
        let tag_end = match rest.find('>') {
            Some(e) => e,
            None => break,
        };
        let open_tag = &rest[..tag_end];
        let role = extract_dq_attr(open_tag, "role").unwrap_or_default();

        let close_needle = "</collection>";
        let body_end = match rest.find(close_needle) {
            Some(e) => e,
            None => break,
        };
        let body = &rest[tag_end + 1..body_end];

        if role.to_ascii_lowercase().contains("dictionary") {
            let mut links = Vec::new();
            let mut link_rest = body;
            while let Some(li) = link_rest.find("<link") {
                link_rest = &link_rest[li + "<link".len()..];
                let Some(link_end) = link_rest.find('>') else { break };
                let link_tag = &link_rest[..link_end];
                if let Some(href) = extract_dq_attr(link_tag, "href") {
                    links.push(href);
                }
                link_rest = &link_rest[link_end..];
            }
            out.push(DictCollection { links });
        }

        rest = &rest[body_end + close_needle.len()..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcp47_accepts_common_codes() {
        assert!(is_valid_bcp47("en"));
        assert!(is_valid_bcp47("el"));
        assert!(is_valid_bcp47("grc"));
        assert!(is_valid_bcp47("en-US"));
        assert!(is_valid_bcp47("zh-Hant"));
        assert!(is_valid_bcp47("zh-Hant-TW"));
    }

    #[test]
    fn bcp47_rejects_garbage() {
        assert!(!is_valid_bcp47(""));
        assert!(!is_valid_bcp47("ENGLISH"));
        assert!(!is_valid_bcp47("en_US"));
        assert!(!is_valid_bcp47("123"));
    }

    #[test]
    fn count_idx_entries_counts_opening_tags() {
        let html = r#"<idx:entry><idx:orth value="a"/></idx:entry>
                      <idx:entry>x</idx:entry>"#;
        assert_eq!(count_idx_entries(html), 2);
    }

    #[test]
    fn collect_idx_entry_names_reads_name_attr() {
        let html = r#"<idx:entry name="default" scriptable="yes">x</idx:entry>
                      <idx:entry name="other">y</idx:entry>"#;
        let names = collect_idx_entry_names(html);
        assert!(names.contains(&"default".to_string()));
        assert!(names.contains(&"other".to_string()));
    }

    #[test]
    fn find_empty_orth_lines_flags_missing_value() {
        let html = "<idx:entry>\n  <idx:orth value=\"\"/>\n  <idx:orth value=\"ok\"/>\n</idx:entry>";
        let lines = find_empty_orth_lines(html);
        assert_eq!(lines, vec![2]);
    }

    #[test]
    fn find_empty_orth_lines_flags_missing_attr() {
        let html = "<idx:entry>\n<idx:orth/>\n</idx:entry>";
        let lines = find_empty_orth_lines(html);
        assert_eq!(lines, vec![2]);
    }

    #[test]
    fn find_empty_orth_lines_ignores_populated() {
        let html = "<idx:orth value=\"abc\"/>";
        assert!(find_empty_orth_lines(html).is_empty());
    }

    #[test]
    fn find_empty_orth_lines_accepts_body_form_headword() {
        // PyGlossary-style body form: no `value=` but non-empty body text must
        // be accepted as a valid headword (KPG §15.6). Issue #3 regression.
        let html = "<idx:entry>\n<idx:orth><b>hello</b></idx:orth>\n</idx:entry>";
        assert!(find_empty_orth_lines(html).is_empty());
    }

    #[test]
    fn find_empty_orth_lines_accepts_body_with_br() {
        let html = "<idx:orth>\n<b>-eresse</b><br/>\n</idx:orth>";
        assert!(find_empty_orth_lines(html).is_empty());
    }

    #[test]
    fn find_empty_orth_lines_flags_body_with_only_markup() {
        // A body that contains only markup (no text nodes) is still empty.
        let html = "<idx:orth>\n<br/>\n</idx:orth>";
        let lines = find_empty_orth_lines(html);
        assert_eq!(lines, vec![1]);
    }

    #[test]
    fn has_non_whitespace_text_strips_tags() {
        assert!(!has_non_whitespace_text("<b></b>"));
        assert!(!has_non_whitespace_text("  \n\t"));
        assert!(has_non_whitespace_text("<b>x</b>"));
        assert!(has_non_whitespace_text("hi"));
    }

    #[test]
    fn guide_has_index_reference_detects_type_index() {
        let dir = std::env::temp_dir().join(format!("kindling_dict_guide_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.opf");
        std::fs::write(
            &path,
            r#"<package><guide><reference type="index" href="a.html"/></guide></package>"#,
        )
        .unwrap();
        assert!(guide_has_index_reference(&path));
        std::fs::write(
            &path,
            r#"<package><guide><reference type="toc" href="a.html"/></guide></package>"#,
        )
        .unwrap();
        assert!(!guide_has_index_reference(&path));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_dictionary_collections_extracts_links() {
        let dir = std::env::temp_dir().join(format!("kindling_dict_coll_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("b.opf");
        std::fs::write(
            &path,
            r#"<package>
              <collection role="dictionary">
                <link href="content1.xhtml"/>
                <link href="skm.xml"/>
              </collection>
            </package>"#,
        )
        .unwrap();
        let cols = parse_dictionary_collections(&path);
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].links, vec!["content1.xhtml", "skm.xml"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extract_dq_attr_handles_mid_tag() {
        let tag = r#" name="default" scriptable="yes""#;
        assert_eq!(extract_dq_attr(tag, "name"), Some("default".to_string()));
        assert_eq!(
            extract_dq_attr(tag, "scriptable"),
            Some("yes".to_string())
        );
    }
}
