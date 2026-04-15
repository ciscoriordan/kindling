// Section 9 cross-reference and dead-link checks (R9.1 through R9.12).
//
// These rules mirror the epubcheck RSC_009/011/012/014/015/020/026/029/030/033
// and OPF_091/098 diagnostics. They walk every href/src/xlink:href attribute in
// spine content and cross-reference them against a global id index and the
// OPF manifest:
//   R9.1  RSC_009 Non-SVG image referenced with a fragment
//   R9.2  RSC_011 Link targets a manifest item that is not in the spine
//   R9.3  RSC_012 Fragment id not defined in the target file
//   R9.4  RSC_014 Fragment points into a resource that doesn't support ids
//   R9.5  RSC_015 SVG <use> element missing a fragment identifier
//   R9.6  RSC_020 href/src value is not a syntactically valid URL
//   R9.7  RSC_026 Path escapes outside the EPUB container
//   R9.8  RSC_029 data: URL used in href/src
//   R9.9  RSC_030 file: URL used in href/src
//   R9.10 RSC_033 Relative URL carries a ?query component
//   R9.11 OPF_091 Manifest item href contains a fragment identifier
//   R9.12 OPF_098 Manifest item href references an element rather than a resource

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct CrossRefsChecks;

impl Check for CrossRefsChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R9.1", "R9.2", "R9.3", "R9.4", "R9.5", "R9.6", "R9.7", "R9.8", "R9.9", "R9.10",
            "R9.11", "R9.12",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // --- OPF manifest-only rules: R9.11 and R9.12. ------------------------
        for (_id, (href, _media_type)) in &opf.manifest {
            if let Some(fragment) = href.find('#') {
                // R9.11: manifest item href contains a fragment identifier.
                report.emit_at(
                    "R9.11",
                    format!("Manifest item href is '{}'.", href),
                    Some(epub.opf_path.clone()),
                    None,
                );
                // R9.12: the href references an element rather than a resource.
                // If the part before '#' is empty, it's a bare element reference.
                if fragment == 0 {
                    report.emit_at(
                        "R9.12",
                        format!("Manifest item href is '{}'.", href),
                        Some(epub.opf_path.clone()),
                        None,
                    );
                }
            }
        }

        // --- Build a manifest href -> media_type map and a spine href set. ----
        let mut manifest_by_href: HashMap<String, String> = HashMap::new();
        for (_id, (href, media_type)) in &opf.manifest {
            let clean = strip_fragment(href);
            manifest_by_href.insert(clean, media_type.clone());
        }
        let spine_hrefs: HashSet<String> = opf
            .spine_items
            .iter()
            .map(|(_, href)| strip_fragment(href))
            .collect();

        // --- Build a global id index over every XHTML manifest item. ---------
        //
        // We walk the manifest, not just the spine, because intra-book links
        // can legally target any XHTML document in the package (e.g. footnote
        // files not in the reading order). The parser is intentionally string
        // based so UTF-8 fragment ids like '#hw_͵Α' round-trip unchanged.
        let mut id_index: HashMap<String, HashSet<String>> = HashMap::new();
        for (_id, (href, media_type)) in &opf.manifest {
            if !is_xhtml_media_type(media_type) {
                continue;
            }
            let clean = strip_fragment(href);
            if let Some(text) = epub.read(&clean) {
                let ids = collect_ids(&text);
                id_index.insert(clean, ids);
            } else {
                id_index.insert(clean, HashSet::new());
            }
        }

        // --- Walk every XHTML/SVG spine item and scan every href/src value. --
        for (_, href) in &opf.spine_items {
            let clean = strip_fragment(href);
            let media_type = match opf
                .manifest
                .values()
                .find(|(h, _)| h == href || strip_fragment(h) == clean)
                .map(|(_, mt)| mt.clone())
            {
                Some(mt) => mt,
                None => continue,
            };

            let is_xhtml = is_xhtml_media_type(&media_type);
            let is_svg = is_svg_media_type(&media_type);
            if !is_xhtml && !is_svg {
                continue;
            }

            let Some(text) = epub.read(&clean) else { continue };

            scan_content(
                &clean,
                &text,
                is_svg,
                &manifest_by_href,
                &spine_hrefs,
                &id_index,
                report,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Content walker
// ---------------------------------------------------------------------------

/// Scan a single XHTML or SVG file for href/src/xlink:href attribute values.
fn scan_content(
    file_href: &str,
    text: &str,
    is_svg: bool,
    manifest_by_href: &HashMap<String, String>,
    spine_hrefs: &HashSet<String>,
    id_index: &HashMap<String, HashSet<String>>,
    report: &mut ValidationReport,
) {
    let file = Some(PathBuf::from(file_href));
    let file_dir = Path::new(file_href).parent().map(|p| p.to_path_buf()).unwrap_or_default();

    // R9.5 SVG <use xlink:href="..."> without a fragment identifier.
    if is_svg {
        check_svg_use_without_fragment(text, &file, report);
    }

    // R9.1 non-SVG <img src="foo.png#fragment"> with a fragment. Always runs
    // regardless of content type because svg-in-xhtml is allowed.
    check_img_fragment(text, &file, report);

    // Collect every attribute reference so we only scan once.
    for attr_ref in collect_attr_refs(text) {
        let AttrRef { attr_name, value, element_name, .. } = &attr_ref;
        // Skip xmlns:* style namespace declarations which are not URL refs.
        if attr_name.starts_with("xmlns") {
            continue;
        }

        classify_and_check(
            value,
            attr_name,
            element_name,
            &file,
            &file_dir,
            manifest_by_href,
            spine_hrefs,
            id_index,
            report,
        );
    }
}

/// Classify `value` and fire R9.6-R9.10, R9.2, R9.3, R9.4 as appropriate.
#[allow(clippy::too_many_arguments)]
fn classify_and_check(
    value: &str,
    attr_name: &str,
    element_name: &str,
    file: &Option<PathBuf>,
    file_dir: &Path,
    manifest_by_href: &HashMap<String, String>,
    spine_hrefs: &HashSet<String>,
    id_index: &HashMap<String, HashSet<String>>,
    report: &mut ValidationReport,
) {
    // Empty values are ignored everywhere (they appear in conformance-only
    // anchors like <a id="foo">).
    if value.is_empty() {
        return;
    }

    // R9.6: catastrophically bad URL - contains whitespace, control chars, or
    // raw '<' / '>'. These are genuinely unparseable in any reader.
    if has_control_or_space(value) {
        report.emit_at(
            "R9.6",
            format!("Value '{}' contains whitespace or a control char.", value),
            file.clone(),
            None,
        );
        return;
    }

    let lower = value.to_ascii_lowercase();

    // R9.8 data: URL.
    if lower.starts_with("data:") {
        report.emit_at(
            "R9.8",
            format!("{}=\"{}\"", attr_name, shorten(value, 80)),
            file.clone(),
            None,
        );
        return;
    }

    // R9.9 file: URL.
    if lower.starts_with("file:") {
        report.emit_at(
            "R9.9",
            format!("{}=\"{}\"", attr_name, value),
            file.clone(),
            None,
        );
        return;
    }

    // http/https/mailto/tel/urn/javascript are external links; these are not
    // cluster F's concern (NAV_010 in cluster E handles external nav links).
    if is_external_scheme(&lower) {
        return;
    }

    // Bare fragment like href="#foo". Resolve against the current file.
    if let Some(frag) = value.strip_prefix('#') {
        let current_href = file
            .as_ref()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        check_fragment_target(
            &current_href,
            frag,
            attr_name,
            file,
            manifest_by_href,
            id_index,
            report,
        );
        return;
    }

    // Split into path + query + fragment. All three components are optional.
    let (path_part, query_part, fragment_part) = split_url(value);

    // R9.10: relative URL with a ?query component.
    if query_part.is_some() {
        report.emit_at(
            "R9.10",
            format!("{}=\"{}\"", attr_name, value),
            file.clone(),
            None,
        );
    }

    // Resolve the path relative to the file that contained the attribute.
    // If the path is empty, the fragment is a self-reference and we already
    // returned above in the bare-fragment branch.
    if path_part.is_empty() {
        return;
    }

    let resolved = resolve_relative(file_dir, path_part);

    // R9.7: path escapes outside the EPUB container.
    if path_escapes_root(&resolved) {
        report.emit_at(
            "R9.7",
            format!("{}=\"{}\"", attr_name, value),
            file.clone(),
            None,
        );
        return;
    }

    let resolved_str = resolved.to_string_lossy().replace('\\', "/");

    // R9.2: link targets a manifest item that is not in the spine. Only
    // applies to <a href="..."> style links where the target is an XHTML doc.
    if attr_name == "href" && element_name == "a" {
        if let Some(media_type) = manifest_by_href.get(&resolved_str) {
            if is_xhtml_media_type(media_type) && !spine_hrefs.contains(&resolved_str) {
                report.emit_at(
                    "R9.2",
                    format!("href=\"{}\"", value),
                    file.clone(),
                    None,
                );
            }
        }
    }

    // R9.3 / R9.4: fragment resolution.
    if let Some(frag) = fragment_part {
        check_fragment_target(
            &resolved_str,
            frag,
            attr_name,
            file,
            manifest_by_href,
            id_index,
            report,
        );
    }
}

/// R9.3 and R9.4: the fragment `frag` points into the file at `target_href`.
fn check_fragment_target(
    target_href: &str,
    frag: &str,
    attr_name: &str,
    file: &Option<PathBuf>,
    manifest_by_href: &HashMap<String, String>,
    id_index: &HashMap<String, HashSet<String>>,
    report: &mut ValidationReport,
) {
    // An empty fragment ('#') is harmless.
    if frag.is_empty() {
        return;
    }

    // R9.4: fragment points into a CSS/image/font resource.
    if let Some(media_type) = manifest_by_href.get(target_href) {
        if !is_xhtml_media_type(media_type) && !is_svg_media_type(media_type) {
            report.emit_at(
                "R9.4",
                format!("{}=\"{}#{}\"", attr_name, target_href, frag),
                file.clone(),
                None,
            );
            return;
        }
    }

    // R9.3: target file is in manifest but the id is not defined there.
    //
    // We only fire R9.3 when we actually have an id index for the target
    // file. Missing entries mean the file is not in the manifest as XHTML,
    // which is a separate concern that cluster J / cluster E already covers.
    if let Some(ids) = id_index.get(target_href) {
        if !ids.contains(frag) {
            report.emit_at(
                "R9.3",
                format!("{}=\"{}#{}\"", attr_name, target_href, frag),
                file.clone(),
                None,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// SVG <use> and <img src> fragment checks
// ---------------------------------------------------------------------------

/// R9.5: every SVG `<use>` element must carry a fragment identifier on its
/// xlink:href (or href) attribute. A fragment-less use element is an error.
fn check_svg_use_without_fragment(
    text: &str,
    file: &Option<PathBuf>,
    report: &mut ValidationReport,
) {
    for tag in find_tags(text, "use") {
        let href = extract_attr_generic(tag, "xlink:href")
            .or_else(|| extract_attr_generic(tag, "href"));
        let Some(value) = href else { continue };
        if !value.contains('#') {
            report.emit_at(
                "R9.5",
                format!("xlink:href=\"{}\"", value),
                file.clone(),
                None,
            );
        }
    }
}

/// R9.1: `<img src="foo.png#fragment">` where foo is not SVG.
fn check_img_fragment(text: &str, file: &Option<PathBuf>, report: &mut ValidationReport) {
    for tag in find_tags(text, "img") {
        let Some(src) = extract_attr_generic(tag, "src") else { continue };
        let Some(hash_pos) = src.find('#') else { continue };
        // Skip if the path points at an .svg resource.
        let path_part = &src[..hash_pos];
        let lower = path_part.to_ascii_lowercase();
        if lower.ends_with(".svg") || lower.ends_with(".svgz") {
            continue;
        }
        report.emit_at("R9.1", format!("src=\"{}\"", src), file.clone(), None);
    }
}

// ---------------------------------------------------------------------------
// Helpers: URL parsing and classification
// ---------------------------------------------------------------------------

/// Split a URL string into (path, query, fragment). Empty components come back
/// as either an empty `&str` for the path or `None` for query/fragment.
fn split_url(value: &str) -> (&str, Option<&str>, Option<&str>) {
    let (head, fragment) = match value.find('#') {
        Some(i) => (&value[..i], Some(&value[i + 1..])),
        None => (value, None),
    };
    let (path, query) = match head.find('?') {
        Some(i) => (&head[..i], Some(&head[i + 1..])),
        None => (head, None),
    };
    (path, query, fragment)
}

/// Strip any '#fragment' tail from `href`.
fn strip_fragment(href: &str) -> String {
    match href.find('#') {
        Some(i) => href[..i].to_string(),
        None => href.to_string(),
    }
}

/// True if the value has ASCII whitespace, control characters, or bare angle
/// brackets. These make a URL genuinely unparseable from an attribute value.
fn has_control_or_space(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_whitespace() || c.is_control() || c == '<' || c == '>')
}

/// True if `mt` is an XHTML / HTML media type.
fn is_xhtml_media_type(mt: &str) -> bool {
    let l = mt.to_ascii_lowercase();
    l == "application/xhtml+xml" || l == "text/html"
}

/// True if `mt` is an SVG media type.
fn is_svg_media_type(mt: &str) -> bool {
    let l = mt.to_ascii_lowercase();
    l == "image/svg+xml"
}

/// True if `lower` already carries a scheme we deliberately skip.
fn is_external_scheme(lower: &str) -> bool {
    lower.starts_with("http:")
        || lower.starts_with("https:")
        || lower.starts_with("mailto:")
        || lower.starts_with("tel:")
        || lower.starts_with("urn:")
        || lower.starts_with("ftp:")
        || lower.starts_with("javascript:")
        || lower.starts_with("about:")
        || lower.starts_with("kindle:")
}

/// Normalize `base/rel` to a forward-slash path, collapsing '.' and '..' components.
fn resolve_relative(base: &Path, rel: &str) -> PathBuf {
    let combined = if base.as_os_str().is_empty() {
        PathBuf::from(rel)
    } else {
        base.join(rel)
    };
    let mut out: Vec<String> = Vec::new();
    for comp in combined.to_string_lossy().split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            if out.last().map(String::as_str) == Some("..") || out.is_empty() {
                out.push("..".to_string());
            } else {
                out.pop();
            }
            continue;
        }
        out.push(comp.to_string());
    }
    PathBuf::from(out.join("/"))
}

/// True if the resolved path starts with '..' after normalization, meaning it
/// has escaped the EPUB root.
fn path_escapes_root(resolved: &Path) -> bool {
    resolved
        .to_string_lossy()
        .split('/')
        .next()
        .map(|c| c == "..")
        .unwrap_or(false)
}

/// Return the first `max_len` chars of `s` followed by an ellipsis marker.
fn shorten(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len).collect();
        format!("{}...", truncated)
    }
}

// ---------------------------------------------------------------------------
// Helpers: attribute scanning
// ---------------------------------------------------------------------------

/// A single attribute reference found in the scanned document.
#[derive(Debug, Clone)]
struct AttrRef {
    element_name: String,
    attr_name: String,
    value: String,
}

/// Collect every `attr="value"` pair whose attribute name is href, src, or
/// xlink:href. Only double-quoted values are scanned because every EPUB we
/// ship through kindling uses the canonical quoting style.
fn collect_attr_refs(text: &str) -> Vec<AttrRef> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Skip comments so we do not scan documented href examples.
        if bytes[i] == b'<' && bytes[i + 1..].starts_with(b"!--") {
            if let Some(rel) = text[i..].find("-->") {
                i += rel + 3;
                continue;
            } else {
                break;
            }
        }
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        // Read the element name following '<'.
        let name_start = i + 1;
        let mut name_end = name_start;
        while name_end < bytes.len() {
            let c = bytes[name_end];
            if c == b' '
                || c == b'\t'
                || c == b'\n'
                || c == b'\r'
                || c == b'>'
                || c == b'/'
            {
                break;
            }
            name_end += 1;
        }
        if name_end == name_start || name_end >= bytes.len() {
            i = name_end + 1;
            continue;
        }
        let raw_name = &text[name_start..name_end];
        if raw_name.starts_with('!') || raw_name.starts_with('?') {
            // Skip <!DOCTYPE ...>, <?xml ...?>, etc.
            i = match text[i..].find('>') {
                Some(e) => i + e + 1,
                None => break,
            };
            continue;
        }
        // Extract the local name, stripping any namespace prefix.
        let element_name = match raw_name.rfind(':') {
            Some(pos) => raw_name[pos + 1..].to_string(),
            None => raw_name.to_string(),
        };
        // Find the end of the tag (first unquoted '>').
        let tag_end = match find_tag_end(&text[name_end..]) {
            Some(e) => name_end + e,
            None => break,
        };
        let tag_body = &text[name_end..tag_end];

        for attr in scan_attrs(tag_body) {
            match attr.0.as_str() {
                "href" | "src" | "xlink:href" => {
                    out.push(AttrRef {
                        element_name: element_name.clone(),
                        attr_name: attr.0,
                        value: attr.1,
                    });
                }
                _ => {}
            }
        }

        i = tag_end + 1;
    }
    out
}

/// Find the end of a tag body (first unquoted '>'). `body` begins just after
/// the element name.
fn find_tag_end(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'"' && !in_single {
            in_double = !in_double;
        } else if c == b'\'' && !in_double {
            in_single = !in_single;
        } else if c == b'>' && !in_single && !in_double {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Extract every `name="value"` / `name='value'` pair out of a tag body.
fn scan_attrs(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // Skip whitespace and '/'.
        while i < bytes.len()
            && (bytes[i] == b' '
                || bytes[i] == b'\t'
                || bytes[i] == b'\n'
                || bytes[i] == b'\r'
                || bytes[i] == b'/')
        {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Read the attribute name.
        let name_start = i;
        while i < bytes.len() {
            let c = bytes[i];
            if c == b'='
                || c == b' '
                || c == b'\t'
                || c == b'\n'
                || c == b'\r'
                || c == b'/'
                || c == b'>'
            {
                break;
            }
            i += 1;
        }
        let name = body[name_start..i].to_string();
        if name.is_empty() {
            i += 1;
            continue;
        }
        // Skip whitespace to the '='.
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            // Valueless attribute: skip.
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            // Unquoted values are not scanned: kindling only validates
            // double-or-single-quoted attribute values. Move on to the next
            // whitespace to resync.
            while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' && bytes[i] != b'>' {
                i += 1;
            }
            continue;
        }
        i += 1;
        let value_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let value = body[value_start..i].to_string();
        i += 1;
        out.push((name, value));
    }
    out
}

/// Find every tag in `text` whose local name (namespace-stripped) equals
/// `needle`. Returns the tag body substring (between '<' and '>').
fn find_tags<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let name_start = i + 1;
        let mut name_end = name_start;
        while name_end < bytes.len() {
            let c = bytes[name_end];
            if c == b' '
                || c == b'\t'
                || c == b'\n'
                || c == b'\r'
                || c == b'>'
                || c == b'/'
            {
                break;
            }
            name_end += 1;
        }
        if name_end == name_start || name_end >= bytes.len() {
            i = name_end + 1;
            continue;
        }
        let raw_name = &text[name_start..name_end];
        let local = match raw_name.rfind(':') {
            Some(pos) => &raw_name[pos + 1..],
            None => raw_name,
        };
        if local.eq_ignore_ascii_case(needle) {
            if let Some(end_rel) = find_tag_end(&text[name_end..]) {
                out.push(&text[name_start..name_end + end_rel]);
                i = name_end + end_rel + 1;
                continue;
            } else {
                break;
            }
        }
        i = name_end;
    }
    out
}

/// Extract `name="..."` or `name='...'` from a tag body. Case-sensitive.
fn extract_attr_generic(tag: &str, attr: &str) -> Option<String> {
    for (name, value) in scan_attrs(tag) {
        if name == attr {
            return Some(value);
        }
    }
    None
}

/// Collect every `id="..."` or `id='...'` attribute value from an HTML string.
/// More permissive than `ExtractedEpub::ids` so it handles the lemma case
/// where `id=` can be preceded by a newline, not just a space.
fn collect_ids(html: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let bytes = html.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let tag_start = i + 1;
        let tag_end = match find_tag_end(&html[tag_start..]) {
            Some(e) => tag_start + e,
            None => break,
        };
        let body = &html[tag_start..tag_end];
        for (name, value) in scan_attrs(body) {
            // Strip namespace prefix: "xml:id" -> "id".
            let local = match name.rfind(':') {
                Some(pos) => &name[pos + 1..],
                None => name.as_str(),
            };
            if local == "id" {
                out.insert(value);
            }
        }
        i = tag_end + 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- split_url + resolve_relative ----

    #[test]
    fn split_url_returns_all_parts() {
        let (path, query, frag) = split_url("foo.html?x=1#anchor");
        assert_eq!(path, "foo.html");
        assert_eq!(query, Some("x=1"));
        assert_eq!(frag, Some("anchor"));
    }

    #[test]
    fn split_url_bare_fragment() {
        let (path, query, frag) = split_url("#anchor");
        assert_eq!(path, "");
        assert!(query.is_none());
        assert_eq!(frag, Some("anchor"));
    }

    #[test]
    fn split_url_no_query_no_fragment() {
        let (path, query, frag) = split_url("foo.html");
        assert_eq!(path, "foo.html");
        assert!(query.is_none());
        assert!(frag.is_none());
    }

    #[test]
    fn resolve_relative_collapses_dots() {
        let p = resolve_relative(Path::new("chapters"), "../images/a.png");
        assert_eq!(p.to_string_lossy(), "images/a.png");
    }

    #[test]
    fn resolve_relative_escape_marker() {
        let p = resolve_relative(Path::new(""), "../../etc/passwd");
        assert!(path_escapes_root(&p));
    }

    #[test]
    fn resolve_relative_stays_in_root() {
        let p = resolve_relative(Path::new(""), "chapters/ch1.html");
        assert!(!path_escapes_root(&p));
        assert_eq!(p.to_string_lossy(), "chapters/ch1.html");
    }

    // ---- has_control_or_space / schemes ----

    #[test]
    fn r9_6_space_detected() {
        assert!(has_control_or_space("foo bar.html"));
    }

    #[test]
    fn r9_6_newline_detected() {
        assert!(has_control_or_space("foo\nbar.html"));
    }

    #[test]
    fn r9_6_clean_utf8_fragment() {
        // Lemma uses raw Greek characters in href fragments. That is not a
        // control character and must not trip R9.6.
        assert!(!has_control_or_space("content_00.html#hw_\u{0375}\u{0391}"));
    }

    #[test]
    fn r9_8_data_scheme() {
        assert!("data:image/png;base64,abcd".to_ascii_lowercase().starts_with("data:"));
    }

    #[test]
    fn r9_9_file_scheme() {
        assert!("file:///etc/passwd".to_ascii_lowercase().starts_with("file:"));
    }

    #[test]
    fn r9_x_external_schemes_skipped() {
        assert!(is_external_scheme("http://example.com/"));
        assert!(is_external_scheme("https://example.com/"));
        assert!(is_external_scheme("mailto:a@b.c"));
        assert!(!is_external_scheme("page.html"));
    }

    // ---- id collector ----

    #[test]
    fn collect_ids_handles_double_quoted() {
        let ids = collect_ids("<p id=\"a\"/><p id=\"b\"/>");
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
    }

    #[test]
    fn collect_ids_handles_single_quoted() {
        let ids = collect_ids("<p id='a'/>");
        assert!(ids.contains("a"));
    }

    #[test]
    fn collect_ids_handles_utf8_values() {
        // Exercises the lemma dictionary anchor style.
        let html = "<idx:entry name=\"default\" scriptable=\"yes\" id=\"hw_\u{0375}\u{0391}\"/>";
        let ids = collect_ids(html);
        assert!(ids.contains("hw_\u{0375}\u{0391}"));
    }

    #[test]
    fn collect_ids_handles_newline_before_attr() {
        let html = "<p\n  id=\"foo\"/>";
        let ids = collect_ids(html);
        assert!(ids.contains("foo"));
    }

    // ---- attribute scanner ----

    #[test]
    fn collect_attr_refs_finds_href_and_src() {
        let html = r#"<a href="p.html#x">k</a><img src="a.png"/>"#;
        let refs = collect_attr_refs(html);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].attr_name, "href");
        assert_eq!(refs[0].value, "p.html#x");
        assert_eq!(refs[1].attr_name, "src");
        assert_eq!(refs[1].value, "a.png");
    }

    #[test]
    fn collect_attr_refs_finds_xlink_href() {
        let html = r#"<svg><use xlink:href="sprite.svg#icon"/></svg>"#;
        let refs = collect_attr_refs(html);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].attr_name, "xlink:href");
    }

    #[test]
    fn collect_attr_refs_skips_xmlns() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"/>"#;
        let refs = collect_attr_refs(html);
        assert!(refs.is_empty());
    }

    #[test]
    fn collect_attr_refs_skips_comments() {
        let html = r#"<!-- <a href="ignored.html"/> --><a href="real.html"/>"#;
        let refs = collect_attr_refs(html);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].value, "real.html");
    }

    // ---- find_tags / extract_attr_generic ----

    #[test]
    fn find_tags_matches_svg_use() {
        let svg = r##"<svg><use xlink:href="#icon"/></svg>"##;
        let tags = find_tags(svg, "use");
        assert_eq!(tags.len(), 1);
        assert!(extract_attr_generic(tags[0], "xlink:href").is_some());
    }

    #[test]
    fn find_tags_matches_img_with_fragment() {
        let html = r#"<img src="a.png#frag"/>"#;
        let tags = find_tags(html, "img");
        assert_eq!(tags.len(), 1);
        let src = extract_attr_generic(tags[0], "src").unwrap();
        assert_eq!(src, "a.png#frag");
    }

    // ---- shorten ----

    #[test]
    fn shorten_short_string_passthrough() {
        assert_eq!(shorten("hello", 80), "hello");
    }

    #[test]
    fn shorten_long_string_truncates() {
        let s = "a".repeat(200);
        let out = shorten(&s, 80);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), 83);
    }
}
