// Section 8: OPF prefix attribute and manifest item property grammar.
//
// Covers the epubcheck OPF_004 through OPF_028 family of rules. These only
// apply to EPUB 3 packages (package_version == "3.0"), because the prefix
// attribute and the manifest properties vocabulary are both EPUB 3 features.
//
//   R8.1  Malformed package prefix attribute (OPF_004)
//   R8.2  Duplicate prefix in package prefix attribute (OPF_005)
//   R8.3  Reserved prefix rebound to non-standard URI (OPF_006)
//   R8.4  Prefix maps to a malformed URI (OPF_007)
//   R8.5  Manifest item property invalid for media-type (OPF_012)
//   R8.6  Spine XHTML uses a feature without declaring the property (OPF_014)
//   R8.7  Manifest declares a property the content doesn't actually use (OPF_015)
//   R8.8  Property value is syntactically malformed (OPF_026)
//   R8.9  Unknown property without a declared prefix (OPF_027)
//   R8.10 Property uses an undeclared prefix (OPF_028)

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct OpfGrammarChecks;

impl Check for OpfGrammarChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R8.1", "R8.2", "R8.3", "R8.4", "R8.5", "R8.6", "R8.7", "R8.8", "R8.9", "R8.10",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        if epub.opf.package_version != "3.0" {
            return;
        }

        let opf_bytes = match fs::read_to_string(&epub.opf_path) {
            Ok(s) => s,
            Err(_) => return,
        };

        let declared_prefixes = scan_package_prefix_attr(&opf_bytes, report);
        let manifest_items = parse_manifest_items(&opf_bytes);

        run_property_grammar_rules(epub, &manifest_items, &declared_prefixes, report);
        run_content_vs_property_rules(epub, &manifest_items, report);
    }
}

// ---------------------------------------------------------------------------
// Default and reserved prefixes
// ---------------------------------------------------------------------------

/// Reserved prefixes that must not be rebound to a non-standard URI.
const RESERVED_PREFIXES: &[&str] = &[
    "dcterms", "epub", "marc", "media", "onix", "opf", "rendition", "schema", "xsd",
];

/// Known EPUB property names (from default vocabulary) that need no prefix.
const KNOWN_UNPREFIXED_PROPERTIES: &[&str] = &[
    // Manifest item properties (EPUB 3 packages)
    "cover-image",
    "mathml",
    "nav",
    "remote-resources",
    "scripted",
    "svg",
    "switch",
    // Amazon-legacy manifest property used in KDP workflows
    "coverimage",
];

/// Prefixes that ship in the default EPUB 3 package vocabulary.
const DEFAULT_PREFIXES: &[&str] = &[
    "dcterms", "marc", "media", "onix", "rendition", "schema", "xsd", "opf", "epub",
];

/// Reserved-prefix canonical URIs for the OPF_006 check.
fn reserved_prefix_canonical(name: &str) -> Option<&'static str> {
    match name {
        "dcterms" => Some("http://purl.org/dc/terms/"),
        "epub" => Some("http://www.idpf.org/2007/ops"),
        "marc" => Some("http://id.loc.gov/vocabulary/"),
        "media" => Some("http://www.idpf.org/epub/vocab/overlays/#"),
        "onix" => Some("http://www.editeur.org/ONIX/book/codelists/current.html#"),
        "opf" => Some("http://www.idpf.org/2007/opf"),
        "rendition" => Some("http://www.idpf.org/vocab/rendition/#"),
        "schema" => Some("http://schema.org/"),
        "xsd" => Some("http://www.w3.org/2001/XMLSchema#"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// R8.1 - R8.4: Package prefix attribute grammar
// ---------------------------------------------------------------------------

/// Parse the `<package prefix="...">` attribute and emit R8.1 through R8.4.
/// Returns the map of declared prefix name to URI (empty if no prefix attr).
fn scan_package_prefix_attr(
    opf_text: &str,
    report: &mut ValidationReport,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let Some(package_open) = find_package_open_tag(opf_text) else {
        return out;
    };
    let Some(prefix_value) = extract_attr_value(package_open, "prefix") else {
        return out;
    };

    let tokens: Vec<&str> = prefix_value.split_whitespace().collect();
    if tokens.is_empty() {
        return out;
    }
    if tokens.len() % 2 != 0 {
        report.emit(
            "R8.1",
            format!("prefix=\"{}\" has an odd number of tokens.", prefix_value),
        );
        return out;
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut i = 0;
    while i + 1 < tokens.len() {
        let name_tok = tokens[i];
        let uri_tok = tokens[i + 1];
        i += 2;

        if !name_tok.ends_with(':') || name_tok.len() < 2 {
            report.emit(
                "R8.1",
                format!("'{}' is not a valid prefix name (expected 'name:').", name_tok),
            );
            continue;
        }
        let name = &name_tok[..name_tok.len() - 1];
        if !is_valid_ncname(name) {
            report.emit(
                "R8.1",
                format!("'{}' is not a valid NCName for a prefix.", name),
            );
            continue;
        }

        if !is_valid_uri(uri_tok) {
            report.emit(
                "R8.4",
                format!("Prefix '{}' maps to malformed URI '{}'.", name, uri_tok),
            );
            continue;
        }

        if !seen.insert(name.to_string()) {
            report.emit(
                "R8.2",
                format!("Prefix '{}' is declared more than once.", name),
            );
            continue;
        }

        if RESERVED_PREFIXES.contains(&name) {
            let canonical = reserved_prefix_canonical(name).unwrap_or("");
            if !canonical.is_empty() && uri_tok != canonical {
                report.emit(
                    "R8.3",
                    format!(
                        "Reserved prefix '{}' rebound to '{}'; canonical is '{}'.",
                        name, uri_tok, canonical
                    ),
                );
                continue;
            }
        }

        out.insert(name.to_string(), uri_tok.to_string());
    }

    out
}

/// Locate the `<package ... >` open tag body, including attributes but not the
/// leading `<package` or trailing `>`.
fn find_package_open_tag(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let idx = lower.find("<package")?;
    let after = idx + "<package".len();
    let rest = &text[after..];
    let end = rest.find('>')?;
    Some(&rest[..end])
}

/// Extract a double- or single-quoted attribute value from an open-tag body.
fn extract_attr_value(tag_body: &str, attr: &str) -> Option<String> {
    let needle_eq = format!("{}=", attr);
    let start = tag_body.find(&needle_eq)? + needle_eq.len();
    let rest = &tag_body[start..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

/// True if `s` is a valid XML NCName (letter/underscore, then letters/digits/
/// underscore/dash/period).
fn is_valid_ncname(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else { return false };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') {
            return false;
        }
    }
    true
}

/// Best-effort URI validity check: require `scheme:` with a plausible scheme
/// and a non-empty body.
fn is_valid_uri(s: &str) -> bool {
    let Some(colon) = s.find(':') else { return false };
    if colon == 0 || colon + 1 >= s.len() {
        return false;
    }
    let scheme = &s[..colon];
    let mut scheme_chars = scheme.chars();
    let Some(first) = scheme_chars.next() else { return false };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    for c in scheme_chars {
        if !(c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.') {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Manifest item parsing
// ---------------------------------------------------------------------------

/// One parsed manifest item with its `properties` attribute split into tokens.
#[derive(Debug, Clone)]
struct ManifestItem {
    id: String,
    href: String,
    media_type: String,
    properties: Vec<String>,
}

/// Walk the OPF `<manifest>` and return every `<item>` with its attributes.
fn parse_manifest_items(opf_text: &str) -> Vec<ManifestItem> {
    let mut out = Vec::new();
    let lower = opf_text.to_ascii_lowercase();
    let Some(manifest_start) = lower.find("<manifest") else {
        return out;
    };
    let manifest_end = lower[manifest_start..]
        .find("</manifest>")
        .map(|e| manifest_start + e)
        .unwrap_or(opf_text.len());
    let body = &opf_text[manifest_start..manifest_end];

    let mut rest = body;
    while let Some(idx) = rest.find("<item") {
        rest = &rest[idx + "<item".len()..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        // Skip <itemref> inside spine; this is a manifest scan so we bail
        // once we have skipped past the opening <manifest> tag. Presence of
        // leading whitespace or `/` (self-close) is fine, but `ref` means
        // <itemref>, which we do not want.
        if tag.starts_with("ref") || tag.starts_with("ref/") {
            rest = &rest[end..];
            continue;
        }
        let id = extract_attr_value(tag, "id").unwrap_or_default();
        let href = extract_attr_value(tag, "href").unwrap_or_default();
        let media_type = extract_attr_value(tag, "media-type").unwrap_or_default();
        let props_raw = extract_attr_value(tag, "properties").unwrap_or_default();
        let properties: Vec<String> = props_raw
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if !id.is_empty() || !href.is_empty() {
            out.push(ManifestItem { id, href, media_type, properties });
        }
        rest = &rest[end..];
    }
    out
}

// ---------------------------------------------------------------------------
// R8.5, R8.8 - R8.10: Property value and media-type grammar
// ---------------------------------------------------------------------------

/// Allowed media-types for each core property that restricts its host item.
fn allowed_media_types(property: &str) -> Option<&'static [&'static str]> {
    match property {
        "cover-image" | "coverimage" => Some(&[
            "image/jpeg",
            "image/png",
            "image/gif",
            "image/svg+xml",
            "image/webp",
        ]),
        "nav" => Some(&["application/xhtml+xml"]),
        "mathml" | "scripted" | "remote-resources" => {
            Some(&["application/xhtml+xml", "image/svg+xml"])
        }
        "svg" => Some(&["application/xhtml+xml"]),
        _ => None,
    }
}

/// Run R8.5, R8.8, R8.9, R8.10 across every manifest item's `properties`.
fn run_property_grammar_rules(
    epub: &ExtractedEpub,
    items: &[ManifestItem],
    declared_prefixes: &HashMap<String, String>,
    report: &mut ValidationReport,
) {
    let _ = epub;
    for item in items {
        let file = if item.href.is_empty() {
            None
        } else {
            Some(PathBuf::from(&item.href))
        };
        for prop in &item.properties {
            // R8.8: syntactic validity of a property value.
            if !is_valid_property_token(prop) {
                report.emit_at(
                    "R8.8",
                    format!("Property '{}' on item '{}' is malformed.", prop, item.id),
                    file.clone(),
                    None,
                );
                continue;
            }

            if let Some((prefix, local)) = split_prefixed_property(prop) {
                // Prefixed property: default or declared prefix must know it.
                if !DEFAULT_PREFIXES.contains(&prefix)
                    && !declared_prefixes.contains_key(prefix)
                {
                    report.emit_at(
                        "R8.10",
                        format!(
                            "Property '{}' uses undeclared prefix '{}' on item '{}'.",
                            prop, prefix, item.id
                        ),
                        file.clone(),
                        None,
                    );
                    continue;
                }
                let _ = local;
            } else {
                // Unprefixed: must be a known EPUB property name.
                if !KNOWN_UNPREFIXED_PROPERTIES.contains(&prop.as_str()) {
                    report.emit_at(
                        "R8.9",
                        format!(
                            "Unknown property '{}' on item '{}' has no declared prefix.",
                            prop, item.id
                        ),
                        file.clone(),
                        None,
                    );
                    continue;
                }
                // R8.5: unprefixed core property must match its media-type.
                if let Some(allowed) = allowed_media_types(prop) {
                    let mt = item.media_type.to_ascii_lowercase();
                    if !allowed.iter().any(|a| a.eq_ignore_ascii_case(&mt)) {
                        report.emit_at(
                            "R8.5",
                            format!(
                                "Property '{}' not permitted on media-type '{}' \
                                 (item '{}').",
                                prop, item.media_type, item.id
                            ),
                            file.clone(),
                            None,
                        );
                    }
                }
            }
        }
    }
}

/// Syntactic check: property token must be a valid NCName or prefixed NCName.
fn is_valid_property_token(prop: &str) -> bool {
    if prop.is_empty() {
        return false;
    }
    if let Some((prefix, local)) = split_prefixed_property(prop) {
        // Allow hyphen in the local name for properties like remote-resources.
        is_valid_ncname(prefix) && is_valid_property_local(local)
    } else {
        is_valid_property_local(prop)
    }
}

/// Local-name of a property: letter/underscore, then letters/digits/-/_/./.
fn is_valid_property_local(s: &str) -> bool {
    is_valid_ncname(s)
}

/// Split a `prefix:local` property. Returns `None` if no colon is present.
fn split_prefixed_property(prop: &str) -> Option<(&str, &str)> {
    let idx = prop.find(':')?;
    Some((&prop[..idx], &prop[idx + 1..]))
}

// ---------------------------------------------------------------------------
// R8.6, R8.7: Content vs declared-properties cross-check
// ---------------------------------------------------------------------------

/// One spine XHTML item's detected feature flags.
#[derive(Debug, Default, Clone)]
struct ContentFeatures {
    has_mathml: bool,
    has_svg: bool,
    has_scripted: bool,
    has_remote_resources: bool,
}

/// Walk each spine XHTML and emit R8.6 / R8.7 for feature/property mismatches.
fn run_content_vs_property_rules(
    epub: &ExtractedEpub,
    items: &[ManifestItem],
    report: &mut ValidationReport,
) {
    // Index manifest items by id for spine lookups.
    let by_id: HashMap<String, &ManifestItem> =
        items.iter().map(|i| (i.id.clone(), i)).collect();

    for (id, _href) in &epub.opf.spine_items {
        let Some(item) = by_id.get(id) else { continue };
        if !is_xhtml_media_type(&item.media_type) {
            continue;
        }
        let full = epub.opf.base_dir.join(&item.href);
        let Ok(bytes) = fs::read(&full) else { continue };
        let features = detect_content_features(&bytes);

        let declared: HashSet<&str> = item.properties.iter().map(|s| s.as_str()).collect();
        let file = Some(PathBuf::from(&item.href));

        // R8.6: content uses feature but property is not declared.
        if features.has_mathml && !declared.contains("mathml") {
            report.emit_at(
                "R8.6",
                format!("Item '{}' uses MathML but does not declare properties=\"mathml\".", id),
                file.clone(),
                None,
            );
        }
        if features.has_svg && !declared.contains("svg") {
            report.emit_at(
                "R8.6",
                format!("Item '{}' uses SVG but does not declare properties=\"svg\".", id),
                file.clone(),
                None,
            );
        }
        if features.has_scripted && !declared.contains("scripted") {
            report.emit_at(
                "R8.6",
                format!("Item '{}' contains scripts but does not declare properties=\"scripted\".", id),
                file.clone(),
                None,
            );
        }
        if features.has_remote_resources && !declared.contains("remote-resources") {
            report.emit_at(
                "R8.6",
                format!(
                    "Item '{}' references remote resources but does not declare \
                     properties=\"remote-resources\".",
                    id
                ),
                file.clone(),
                None,
            );
        }

        // R8.7: property is declared but content does not use it.
        if declared.contains("mathml") && !features.has_mathml {
            report.emit_at(
                "R8.7",
                format!("Item '{}' declares properties=\"mathml\" but no <math> is present.", id),
                file.clone(),
                None,
            );
        }
        if declared.contains("svg") && !features.has_svg {
            report.emit_at(
                "R8.7",
                format!("Item '{}' declares properties=\"svg\" but no <svg> is present.", id),
                file.clone(),
                None,
            );
        }
        if declared.contains("scripted") && !features.has_scripted {
            report.emit_at(
                "R8.7",
                format!("Item '{}' declares properties=\"scripted\" but no <script> is present.", id),
                file.clone(),
                None,
            );
        }
        if declared.contains("remote-resources") && !features.has_remote_resources {
            report.emit_at(
                "R8.7",
                format!(
                    "Item '{}' declares properties=\"remote-resources\" but no \
                     http(s) URL is present.",
                    id
                ),
                file.clone(),
                None,
            );
        }
    }
}

/// True if `mt` is an XHTML media type.
fn is_xhtml_media_type(mt: &str) -> bool {
    let l = mt.to_ascii_lowercase();
    l == "application/xhtml+xml" || l == "text/html"
}

/// Detect MathML / SVG / scripts / remote resources by substring-scanning the
/// raw file bytes. Cheap enough to run on every spine file.
fn detect_content_features(bytes: &[u8]) -> ContentFeatures {
    let text = std::str::from_utf8(bytes)
        .map(|s| s.to_string())
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).to_string());

    // Ignore xmlns declarations when looking for element usage. We only care
    // about real element start tags of the form `<math `, `<svg `, etc.
    let mut features = ContentFeatures::default();

    features.has_mathml = has_element_start(&text, "math");
    features.has_svg = has_element_start(&text, "svg");
    features.has_scripted = has_element_start(&text, "script");

    // Remote resources: any attribute value that starts with http:// or
    // https://. Substring search is a sound lower bound.
    features.has_remote_resources = has_remote_attr(&text);

    features
}

/// True if `content` contains `<name ` or `<name>` where `name` is used as an
/// element (not part of an attribute name or namespace declaration).
fn has_element_start(content: &str, name: &str) -> bool {
    let needle_space = format!("<{} ", name);
    let needle_close = format!("<{}>", name);
    let needle_slash = format!("<{}/", name);
    let needle_ns = format!("<{}:", name);
    content.contains(&needle_space)
        || content.contains(&needle_close)
        || content.contains(&needle_slash)
        || content.contains(&needle_ns)
}

/// True if any `src=` or `href=` attribute value begins with http:// or https://.
fn has_remote_attr(content: &str) -> bool {
    for needle in &["src=\"http://", "src='http://", "href=\"http://", "href='http://",
                    "src=\"https://", "src='https://", "href=\"https://", "href='https://"]
    {
        if content.contains(needle) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Helper: build a tiny ValidationReport wrapper for unit tests ----

    fn fresh_report() -> ValidationReport {
        ValidationReport::new()
    }

    fn rule_ids(report: &ValidationReport) -> Vec<&'static str> {
        report.findings.iter().filter_map(|f| f.rule_id).collect()
    }

    // ---- R8.1 Malformed prefix attribute grammar ----

    #[test]
    fn r8_1_odd_token_count_fires() {
        let opf = r#"<package version="3.0" prefix="foo: http://example.com extra">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).contains(&"R8.1"));
    }

    #[test]
    fn r8_1_prefix_without_colon_fires() {
        let opf = r#"<package version="3.0" prefix="foo http://example.com">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).contains(&"R8.1"));
    }

    #[test]
    fn r8_1_well_formed_clean() {
        let opf = r#"<package version="3.0" prefix="foo: http://example.com/foo">"#;
        let mut report = fresh_report();
        let map = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).is_empty());
        assert_eq!(map.get("foo").map(String::as_str), Some("http://example.com/foo"));
    }

    // ---- R8.2 Duplicate prefix ----

    #[test]
    fn r8_2_duplicate_prefix_fires() {
        let opf = r#"<package prefix="foo: http://a.example foo: http://b.example">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).contains(&"R8.2"));
    }

    // ---- R8.3 Reserved prefix rebound ----

    #[test]
    fn r8_3_reserved_prefix_rebound_fires() {
        let opf = r#"<package prefix="epub: http://evil.example">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).contains(&"R8.3"));
    }

    #[test]
    fn r8_3_reserved_prefix_canonical_clean() {
        let opf = r#"<package prefix="epub: http://www.idpf.org/2007/ops">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(!rule_ids(&report).contains(&"R8.3"));
    }

    // ---- R8.4 Malformed URI ----

    #[test]
    fn r8_4_garbage_uri_fires() {
        let opf = r#"<package prefix="foo: not-a-uri">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(rule_ids(&report).contains(&"R8.4"));
    }

    #[test]
    fn r8_4_custom_scheme_clean() {
        let opf = r#"<package prefix="foo: urn:isbn:9780000000001">"#;
        let mut report = fresh_report();
        let _ = scan_package_prefix_attr(opf, &mut report);
        assert!(!rule_ids(&report).contains(&"R8.4"));
    }

    // ---- R8.5 Property invalid for media-type ----

    #[test]
    fn r8_5_cover_image_on_non_image_fires() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["cover-image".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(rule_ids(&report).contains(&"R8.5"));
    }

    #[test]
    fn r8_5_nav_on_non_xhtml_fires() {
        let items = vec![ManifestItem {
            id: "n".into(),
            href: "nav.css".into(),
            media_type: "text/css".into(),
            properties: vec!["nav".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(rule_ids(&report).contains(&"R8.5"));
    }

    #[test]
    fn r8_5_mathml_on_xhtml_clean() {
        let items = vec![ManifestItem {
            id: "m".into(),
            href: "c.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["mathml".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(!rule_ids(&report).contains(&"R8.5"));
    }

    #[test]
    fn r8_5_cover_image_on_jpeg_clean() {
        let items = vec![ManifestItem {
            id: "c".into(),
            href: "cover.jpg".into(),
            media_type: "image/jpeg".into(),
            properties: vec!["cover-image".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(!rule_ids(&report).contains(&"R8.5"));
    }

    // ---- R8.8 Malformed property token ----

    #[test]
    fn r8_8_empty_like_token_fires() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["1bad".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(rule_ids(&report).contains(&"R8.8"));
    }

    // ---- R8.9 Unknown unprefixed property ----

    #[test]
    fn r8_9_unknown_unprefixed_fires() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["futuristic".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(rule_ids(&report).contains(&"R8.9"));
    }

    // ---- R8.10 Undeclared prefix on property ----

    #[test]
    fn r8_10_undeclared_prefix_fires() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["myns:thing".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(rule_ids(&report).contains(&"R8.10"));
    }

    #[test]
    fn r8_10_declared_prefix_clean() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["myns:thing".into()],
        }];
        let mut declared = HashMap::new();
        declared.insert("myns".to_string(), "http://example.com/ns".to_string());
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &declared, &mut report);
        assert!(!rule_ids(&report).contains(&"R8.10"));
    }

    #[test]
    fn r8_10_default_prefix_clean() {
        let items = vec![ManifestItem {
            id: "x".into(),
            href: "x.xhtml".into(),
            media_type: "application/xhtml+xml".into(),
            properties: vec!["rendition:layout".into()],
        }];
        let epub = dummy_epub();
        let mut report = fresh_report();
        run_property_grammar_rules(&epub, &items, &HashMap::new(), &mut report);
        assert!(!rule_ids(&report).contains(&"R8.10"));
    }

    // ---- Helpers for property grammar ----

    #[test]
    fn ncname_accepts_simple_names() {
        assert!(is_valid_ncname("foo"));
        assert!(is_valid_ncname("Foo_bar"));
        assert!(is_valid_ncname("foo-bar.baz"));
    }

    #[test]
    fn ncname_rejects_digits_first_and_colons() {
        assert!(!is_valid_ncname("1bad"));
        assert!(!is_valid_ncname("a:b"));
        assert!(!is_valid_ncname(""));
    }

    #[test]
    fn uri_accepts_http_and_urn() {
        assert!(is_valid_uri("http://example.com"));
        assert!(is_valid_uri("https://example.com/foo"));
        assert!(is_valid_uri("urn:isbn:9780000000001"));
    }

    #[test]
    fn uri_rejects_naked_text() {
        assert!(!is_valid_uri(""));
        assert!(!is_valid_uri("not-a-uri"));
        assert!(!is_valid_uri("://oops"));
    }

    #[test]
    fn split_prefixed_property_works() {
        assert_eq!(split_prefixed_property("rendition:layout"), Some(("rendition", "layout")));
        assert_eq!(split_prefixed_property("cover-image"), None);
    }

    // ---- Content feature detection ----

    #[test]
    fn detect_mathml_fires_on_math_element() {
        let html = br#"<html><body><math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math></body></html>"#;
        let f = detect_content_features(html);
        assert!(f.has_mathml);
    }

    #[test]
    fn detect_mathml_ignored_when_only_xmlns() {
        let html = br#"<html xmlns:math="http://exslt.org/math"><body><p>no math here</p></body></html>"#;
        let f = detect_content_features(html);
        assert!(!f.has_mathml);
    }

    #[test]
    fn detect_svg_fires_on_svg_element() {
        let html = br#"<html><body><svg width="1"/></body></html>"#;
        let f = detect_content_features(html);
        assert!(f.has_svg);
    }

    #[test]
    fn detect_script_fires_on_script_element() {
        let html = br#"<html><body><script>alert(1)</script></body></html>"#;
        let f = detect_content_features(html);
        assert!(f.has_scripted);
    }

    #[test]
    fn detect_remote_fires_on_http_src() {
        let html = br#"<html><body><img src="https://cdn.example.com/a.jpg"/></body></html>"#;
        let f = detect_content_features(html);
        assert!(f.has_remote_resources);
    }

    #[test]
    fn detect_remote_ignored_on_relative_href() {
        let html = br#"<html><body><a href="other.xhtml">other</a></body></html>"#;
        let f = detect_content_features(html);
        assert!(!f.has_remote_resources);
    }

    // ---- Dummy ExtractedEpub for unit tests that only need the run funcs ----

    fn dummy_epub() -> ExtractedEpub {
        use crate::opf::OPFData;
        use std::collections::HashMap;
        use std::path::PathBuf;

        let tmp = std::env::temp_dir().join(format!(
            "kindling_opf_grammar_test_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&tmp);
        let opf_path = tmp.join("x.opf");
        let _ = std::fs::write(&opf_path, b"<package version=\"3.0\"></package>");

        let opf = OPFData {
            base_dir: tmp.clone(),
            title: String::new(),
            author: String::new(),
            language: String::new(),
            identifier: String::new(),
            date: String::new(),
            dict_in_language: String::new(),
            dict_out_language: String::new(),
            default_lookup_index: "default".into(),
            spine_items: Vec::new(),
            manifest: HashMap::new(),
            coverimage_id: None,
            is_fixed_layout: false,
            original_resolution: None,
            page_progression_direction: None,
            package_version: "3.0".into(),
            dc_types: Vec::new(),
            manifest_items: Vec::new(),
            raw_itemrefs: Vec::new(),
            embedded_cover_href: None,
        };
        // Construct via public API. We fall back to from_opf_path so we get
        // the real caches; if parse fails for any reason the unit tests that
        // use this helper are still fine because we only dereference it.
        let _ = opf;
        let _ = PathBuf::new();
        ExtractedEpub::from_opf_path(&opf_path)
            .expect("test OPFData should parse")
    }

    // ---- Manifest parser smoke test ----

    #[test]
    fn parse_manifest_items_reads_properties() {
        let opf = r#"<package>
          <manifest>
            <item id="a" href="a.xhtml" media-type="application/xhtml+xml" properties="nav"/>
            <item id="b" href="b.jpg" media-type="image/jpeg" properties="cover-image"/>
          </manifest>
        </package>"#;
        let items = parse_manifest_items(opf);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].properties, vec!["nav".to_string()]);
        assert_eq!(items[1].properties, vec!["cover-image".to_string()]);
    }
}
