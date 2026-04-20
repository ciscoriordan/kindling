// Section 16 OPF metadata rules (R16.1 through R16.8).
//
// These are the "package identity and Dublin Core" rules from epubcheck's
// OPF family. They fire against the raw OPF document so namespace-prefixed
// attributes like `opf:scheme` are preserved exactly as the author wrote
// them, which matters for R16.7 (UUID scheme).
//
//   R16.1 Unique-identifier points at a missing dc:identifier   (OPF_030)
//   R16.2 <package> has no unique-identifier attribute          (OPF_048)
//   R16.3 <dc:date> is not W3CDTF syntax                        (OPF_053)
//   R16.4 <dc:date> is syntactically W3CDTF but an invalid date (OPF_054)
//   R16.5 Empty Dublin Core element                             (OPF_055)
//   R16.6 Empty <metadata> child (meta/x-metadata)              (OPF_072)
//   R16.7 opf:scheme="UUID" value is not a valid RFC 4122 UUID  (OPF_085)
//   R16.8 <dc:language> is not a well-formed BCP47 tag          (OPF_092)

use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

use regex::Regex;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct MetadataChecks;

impl Check for MetadataChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R16.1", "R16.2", "R16.3", "R16.4", "R16.5", "R16.6", "R16.7", "R16.8",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf_bytes = match fs::read(&epub.opf_path) {
            Ok(b) => b,
            Err(_) => return,
        };
        let content = match std::str::from_utf8(&opf_bytes) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(&opf_bytes).to_string(),
        };
        let file = Some(opf_file_label(epub));

        check_package_identifier(&content, &file, report);
        check_dc_date(&content, &file, report);
        check_empty_dc_elements(&content, &file, report);
        check_empty_meta_children(&content, &file, report);
        check_uuid_identifier(&content, &file, report);
        check_dc_language(&content, &file, report);
    }
}

/// Nice path label for a finding from the OPF itself.
fn opf_file_label(epub: &ExtractedEpub) -> PathBuf {
    epub.opf_path
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("content.opf"))
}

// ---------------------------------------------------------------------------
// R16.1 / R16.2: <package unique-identifier="..."> vs <dc:identifier id="...">
// ---------------------------------------------------------------------------

/// Emit R16.1 or R16.2 based on the package element and dc:identifier ids.
fn check_package_identifier(content: &str, file: &Option<PathBuf>, report: &mut ValidationReport) {
    let pkg_open = match find_open_tag(content, "package") {
        Some(o) => o,
        None => return,
    };
    let unique_id_attr = extract_attr(&pkg_open, "unique-identifier");

    match unique_id_attr {
        None => {
            report.emit_at("R16.2", "", file.clone(), None);
        }
        Some(unique_id) => {
            let ids = collect_dc_identifier_ids(content);
            if !ids.iter().any(|id| id == &unique_id) {
                report.emit_at(
                    "R16.1",
                    format!(
                        "unique-identifier=\"{}\" has no matching <dc:identifier id=\"{}\">.",
                        unique_id, unique_id
                    ),
                    file.clone(),
                    None,
                );
            }
        }
    }
}

/// Collect the `id` attribute value from every `<dc:identifier ...>` element.
fn collect_dc_identifier_ids(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for tag in find_all_open_tags(content, "dc:identifier") {
        if let Some(id) = extract_attr(&tag, "id") {
            out.push(id);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// R16.3 / R16.4: <dc:date> W3CDTF checks
// ---------------------------------------------------------------------------

/// Emit R16.3 (syntax) or R16.4 (invalid date) for every `<dc:date>` value.
fn check_dc_date(content: &str, file: &Option<PathBuf>, report: &mut ValidationReport) {
    for value in extract_element_texts(content, "dc:date") {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // Empty is handled by R16.5; skip here.
            continue;
        }
        match parse_w3cdtf(trimmed) {
            DateKind::BadSyntax => {
                report.emit_at(
                    "R16.3",
                    format!("Value \"{}\" is not W3CDTF syntax.", trimmed),
                    file.clone(),
                    None,
                );
            }
            DateKind::SyntaxOkButInvalid => {
                report.emit_at(
                    "R16.4",
                    format!("Value \"{}\" is not a valid calendar date.", trimmed),
                    file.clone(),
                    None,
                );
            }
            DateKind::Valid => {}
        }
    }
}

/// Result of classifying a W3CDTF candidate string.
enum DateKind {
    BadSyntax,
    SyntaxOkButInvalid,
    Valid,
}

/// Classify `value` as W3CDTF: bad syntax, good syntax with bad date, or valid.
fn parse_w3cdtf(value: &str) -> DateKind {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^(?P<y>\d{4})(?:-(?P<mo>\d{2})(?:-(?P<d>\d{2})(?:T(?P<hh>\d{2}):(?P<mm>\d{2}):(?P<ss>\d{2})Z)?)?)?$",
        )
        .unwrap()
    });
    let caps = match re.captures(value) {
        Some(c) => c,
        None => return DateKind::BadSyntax,
    };

    // Year-only is always valid once the regex matches.
    let month = match caps.name("mo") {
        Some(m) => m.as_str().parse::<u32>().unwrap_or(0),
        None => return DateKind::Valid,
    };
    if !(1..=12).contains(&month) {
        return DateKind::SyntaxOkButInvalid;
    }

    // YYYY-MM is valid as long as the month is in range.
    let day = match caps.name("d") {
        Some(d) => d.as_str().parse::<u32>().unwrap_or(0),
        None => return DateKind::Valid,
    };
    let year: i32 = caps.name("y").unwrap().as_str().parse().unwrap_or(0);
    if !is_valid_ymd(year, month, day) {
        return DateKind::SyntaxOkButInvalid;
    }

    // Time component, if present, must be in-range.
    if let Some(h) = caps.name("hh") {
        let hh: u32 = h.as_str().parse().unwrap_or(0);
        let mm: u32 = caps.name("mm").unwrap().as_str().parse().unwrap_or(0);
        let ss: u32 = caps.name("ss").unwrap().as_str().parse().unwrap_or(0);
        if hh > 23 || mm > 59 || ss > 59 {
            return DateKind::SyntaxOkButInvalid;
        }
    }

    DateKind::Valid
}

/// True if year/month/day form a real calendar date.
fn is_valid_ymd(year: i32, month: u32, day: u32) -> bool {
    if !(1..=12).contains(&month) {
        return false;
    }
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    (1..=max_day).contains(&day)
}

/// True if `year` is a Gregorian leap year.
fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

// ---------------------------------------------------------------------------
// R16.5: Empty Dublin Core elements
// ---------------------------------------------------------------------------

/// Emit R16.5 for every empty `<dc:*>` element inside `<metadata>`.
fn check_empty_dc_elements(
    content: &str,
    file: &Option<PathBuf>,
    report: &mut ValidationReport,
) {
    let metadata = match extract_element_inner(content, "metadata") {
        Some(s) => s,
        None => return,
    };
    for (tag, text) in iter_child_elements(&metadata) {
        if !tag.starts_with("dc:") {
            continue;
        }
        if text.trim().is_empty() {
            report.emit_at(
                "R16.5",
                format!("Element <{}> is empty.", tag),
                file.clone(),
                None,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R16.6: Empty <meta> and <x-metadata> children
// ---------------------------------------------------------------------------

/// Emit R16.6 for empty `<meta>` or `<x-metadata>` children of `<metadata>`.
fn check_empty_meta_children(
    content: &str,
    file: &Option<PathBuf>,
    report: &mut ValidationReport,
) {
    let metadata = match extract_element_inner(content, "metadata") {
        Some(s) => s,
        None => return,
    };
    for (tag, text, open_tag_body) in iter_child_elements_with_tag_body(&metadata) {
        if tag != "meta" && tag != "x-metadata" {
            continue;
        }
        // Some <meta> forms carry their payload as attributes rather than text
        // (e.g. <meta name="cover" content="..."/>, <meta property="..." .../>).
        // Those are not "empty" even though they have no text children.
        if meta_has_payload_attributes(&open_tag_body) {
            continue;
        }
        if text.trim().is_empty() {
            report.emit_at(
                "R16.6",
                format!("Element <{}> is empty.", tag),
                file.clone(),
                None,
            );
        }
    }
}

/// True if the open-tag body has any attribute that gives the `<meta>` element
/// its value without needing child text: name/content, property, refines, etc.
fn meta_has_payload_attributes(open_tag_body: &str) -> bool {
    let candidates = [
        "name=", "content=", "property=", "refines=", "scheme=", "id=",
    ];
    for cand in candidates {
        if open_tag_body.contains(cand) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// R16.7: opf:scheme="UUID" identifier format
// ---------------------------------------------------------------------------

/// Emit R16.7 for every `<dc:identifier opf:scheme="UUID">...</dc:identifier>`
/// whose body is not a syntactically-valid RFC 4122 UUID. Identifiers that
/// carry no `opf:scheme` attribute at all must be silent, even if the body
/// text is not a UUID.
fn check_uuid_identifier(content: &str, file: &Option<PathBuf>, report: &mut ValidationReport) {
    for (open_body, inner) in iter_elements_with_body(content, "dc:identifier") {
        let scheme = match extract_attr(&open_body, "opf:scheme") {
            Some(s) => s,
            None => continue,
        };
        if !scheme.eq_ignore_ascii_case("UUID") {
            continue;
        }
        let value = inner.trim();
        if !is_valid_uuid(value) {
            report.emit_at(
                "R16.7",
                format!("Value \"{}\" is not a valid RFC 4122 UUID.", value),
                file.clone(),
                None,
            );
        }
    }
}

/// True if `value` matches `xxxxxxxx-xxxx-Mxxx-Nxxx-xxxxxxxxxxxx` where M is
/// 1-5 and N is one of 8, 9, a, b (RFC 4122 variants).
fn is_valid_uuid(value: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-5][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$",
        )
        .unwrap()
    });
    re.is_match(value)
}

// ---------------------------------------------------------------------------
// R16.8: <dc:language> BCP47 tag
// ---------------------------------------------------------------------------

/// Emit R16.8 for every `<dc:language>` whose body is not a BCP47 tag.
fn check_dc_language(content: &str, file: &Option<PathBuf>, report: &mut ValidationReport) {
    for value in extract_element_texts(content, "dc:language") {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // Empty is handled by R16.5; skip here.
            continue;
        }
        if !is_valid_bcp47(trimmed) {
            report.emit_at(
                "R16.8",
                format!("Value \"{}\" is not a BCP47 language tag.", trimmed),
                file.clone(),
                None,
            );
        }
    }
}

/// Conservative BCP47 pattern: primary language + optional Script/Region/variant.
fn is_valid_bcp47(tag: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"^[a-z]{2,3}(-[A-Z]{4})?(-[A-Z]{2}|-\d{3})?(-[a-zA-Z0-9]+)*$").unwrap()
    });
    re.is_match(tag)
}

// ---------------------------------------------------------------------------
// Low-level XML helpers (hand-rolled because quick-xml's text events are
// easier to reason about against raw bytes for this narrow job).
// ---------------------------------------------------------------------------

/// Return the first `<tag ...>` (start or empty) open tag body, minus brackets.
///
/// Tag name matching is case-insensitive so OEB 1.x OPFs (`<dc:Title>`,
/// `<dc:Identifier>`, etc.) bind to the same checks as OPF 2.0+
/// (`<dc:title>`, `<dc:identifier>`).
fn find_open_tag(content: &str, tag: &str) -> Option<String> {
    let (_, after) = find_open_tag_pos(content, tag, 0)?;
    let end = content[after..].find('>')?;
    let body = &content[after..after + end];
    let body = body.trim_end_matches('/').trim();
    Some(body.to_string())
}

/// Return every `<tag ...>` open-tag body in document order. Case-insensitive.
fn find_all_open_tags(content: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some((_, after)) = find_open_tag_pos(content, tag, cursor) {
        let end = match content[after..].find('>') {
            Some(e) => e,
            None => break,
        };
        let body = &content[after..after + end];
        let body = body.trim_end_matches('/').trim();
        out.push(body.to_string());
        cursor = after + end + 1;
    }
    out
}

/// Case-insensitively find the next `<tag` (at or after `start`) whose name
/// ends in a tag-terminator byte. Returns (open_bracket_pos, after_name_pos).
fn find_open_tag_pos(content: &str, tag: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = content.as_bytes();
    let tag_bytes = tag.as_bytes();
    let mut cursor = start;
    loop {
        let rel = content[cursor..].find('<')?;
        let abs = cursor + rel;
        let name_start = abs + 1;
        let name_end = name_start + tag_bytes.len();
        if name_end > bytes.len() {
            return None;
        }
        if bytes[name_start..name_end].eq_ignore_ascii_case(tag_bytes) {
            let next = *bytes.get(name_end)?;
            if next == b' '
                || next == b'\t'
                || next == b'\n'
                || next == b'\r'
                || next == b'>'
                || next == b'/'
            {
                return Some((abs, name_end));
            }
        }
        cursor = abs + 1;
    }
}

/// Return the inner text of the first `<tag>...</tag>` element.
/// Case-insensitive on the tag name.
fn extract_element_inner(content: &str, tag: &str) -> Option<String> {
    let (_, after_open) = find_open_tag_pos(content, tag, 0)?;
    let tag_end = content[after_open..].find('>')?;
    let body_start = after_open + tag_end + 1;
    let close_idx = find_close_tag_pos(&content[body_start..], tag)?;
    Some(content[body_start..body_start + close_idx].to_string())
}

/// Case-insensitively find the next `</tag>` in `content`, returning its
/// byte offset within `content`.
fn find_close_tag_pos(content: &str, tag: &str) -> Option<usize> {
    let bytes = content.as_bytes();
    let tag_bytes = tag.as_bytes();
    let mut cursor = 0usize;
    loop {
        let rel = content[cursor..].find("</")?;
        let abs = cursor + rel;
        let name_start = abs + 2;
        let name_end = name_start + tag_bytes.len();
        if name_end >= bytes.len() {
            return None;
        }
        if bytes[name_start..name_end].eq_ignore_ascii_case(tag_bytes)
            && bytes[name_end] == b'>'
        {
            return Some(abs);
        }
        cursor = abs + 1;
    }
}

/// Yield every `<tagName>text</tagName>` child of `inner` as (tag, text) pairs.
/// Ignores self-closing elements because they have no textual payload.
fn iter_child_elements(inner: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let bytes = inner.as_bytes();
    while cursor < bytes.len() {
        // Advance to the next `<` that isn't a closing tag or PI.
        let lt = match inner[cursor..].find('<') {
            Some(i) => cursor + i,
            None => break,
        };
        if lt + 1 >= bytes.len() {
            break;
        }
        let c = bytes[lt + 1];
        if c == b'/' || c == b'!' || c == b'?' {
            cursor = lt + 1;
            continue;
        }
        // Parse the tag name.
        let name_start = lt + 1;
        let mut name_end = name_start;
        while name_end < bytes.len() {
            let ch = bytes[name_end];
            if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'/' || ch == b'>'
            {
                break;
            }
            name_end += 1;
        }
        let name = &inner[name_start..name_end];
        // Find end of the opening tag.
        let gt = match inner[name_end..].find('>') {
            Some(i) => name_end + i,
            None => break,
        };
        let open_body = &inner[name_start..gt];
        let self_closing = open_body.trim_end().ends_with('/');
        if self_closing {
            // `<tag .../>` has no textual body; treat as empty.
            out.push((name.to_string(), String::new()));
            cursor = gt + 1;
            continue;
        }
        let body_start = gt + 1;
        let close = format!("</{}>", name);
        let body_end = match inner[body_start..].find(&close) {
            Some(i) => body_start + i,
            None => break,
        };
        let text = &inner[body_start..body_end];
        out.push((name.to_string(), text.to_string()));
        cursor = body_end + close.len();
    }
    out
}

/// Like `iter_child_elements` but also returns the raw open-tag body so the
/// caller can inspect attributes without re-parsing.
fn iter_child_elements_with_tag_body(inner: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let bytes = inner.as_bytes();
    while cursor < bytes.len() {
        let lt = match inner[cursor..].find('<') {
            Some(i) => cursor + i,
            None => break,
        };
        if lt + 1 >= bytes.len() {
            break;
        }
        let c = bytes[lt + 1];
        if c == b'/' || c == b'!' || c == b'?' {
            cursor = lt + 1;
            continue;
        }
        let name_start = lt + 1;
        let mut name_end = name_start;
        while name_end < bytes.len() {
            let ch = bytes[name_end];
            if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'/' || ch == b'>'
            {
                break;
            }
            name_end += 1;
        }
        let name = &inner[name_start..name_end];
        let gt = match inner[name_end..].find('>') {
            Some(i) => name_end + i,
            None => break,
        };
        let open_body_raw = &inner[name_end..gt];
        let self_closing = open_body_raw.trim_end().ends_with('/');
        let trimmed_open = open_body_raw.trim().trim_end_matches('/').trim();
        if self_closing {
            out.push((name.to_string(), String::new(), trimmed_open.to_string()));
            cursor = gt + 1;
            continue;
        }
        let body_start = gt + 1;
        let close = format!("</{}>", name);
        let body_end = match inner[body_start..].find(&close) {
            Some(i) => body_start + i,
            None => break,
        };
        let text = &inner[body_start..body_end];
        out.push((
            name.to_string(),
            text.to_string(),
            trimmed_open.to_string(),
        ));
        cursor = body_end + close.len();
    }
    out
}

/// Return every `(open_body, inner_text)` pair for `<tag>...</tag>` matches.
/// Self-closing `<tag .../>` variants are returned with an empty inner.
/// Case-insensitive on the tag name.
fn iter_elements_with_body(content: &str, tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some((_, after)) = find_open_tag_pos(content, tag, cursor) {
        let gt = match content[after..].find('>') {
            Some(e) => after + e,
            None => break,
        };
        let open_body_raw = &content[after..gt];
        let self_closing = open_body_raw.trim_end().ends_with('/');
        let open_body = open_body_raw.trim().trim_end_matches('/').trim().to_string();
        if self_closing {
            out.push((open_body, String::new()));
            cursor = gt + 1;
            continue;
        }
        let body_start = gt + 1;
        let body_end_rel = match find_close_tag_pos(&content[body_start..], tag) {
            Some(e) => e,
            None => break,
        };
        let inner = content[body_start..body_start + body_end_rel].to_string();
        out.push((open_body, inner));
        cursor = body_start + body_end_rel + format!("</{}>", tag).len();
    }
    out
}

/// Return every inner-text body of `<tag>...</tag>` in document order.
fn extract_element_texts(content: &str, tag: &str) -> Vec<String> {
    iter_elements_with_body(content, tag)
        .into_iter()
        .map(|(_, inner)| inner)
        .collect()
}

/// Extract `attr="value"` or `attr='value'` from an open-tag body.
fn extract_attr(tag_body: &str, attr: &str) -> Option<String> {
    let bytes = tag_body.as_bytes();
    let attr_bytes = attr.as_bytes();
    let mut i = 0usize;
    while i + attr_bytes.len() < bytes.len() {
        // Require that `attr` is preceded by whitespace or start of string so
        // we don't match `xopf:scheme` when searching for `opf:scheme`.
        let prev_ok = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if prev_ok && bytes[i..].starts_with(attr_bytes) {
            let after = i + attr_bytes.len();
            if after < bytes.len() && bytes[after] == b'=' {
                let q_pos = after + 1;
                if q_pos >= bytes.len() {
                    return None;
                }
                let quote = bytes[q_pos];
                if quote != b'"' && quote != b'\'' {
                    return None;
                }
                let rest = &tag_body[q_pos + 1..];
                let close = rest.find(quote as char)?;
                return Some(rest[..close].to_string());
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report() -> ValidationReport {
        ValidationReport::new()
    }

    fn fired(report: &ValidationReport, rule: &str) -> bool {
        report.findings.iter().any(|f| f.rule_id == Some(rule))
    }

    // ---- R16.1 / R16.2 ----

    #[test]
    fn r16_1_unique_id_without_matching_dc_identifier_fires() {
        let opf = r#"<?xml version="1.0"?>
<package version="2.0" unique-identifier="MissingId">
  <metadata>
    <dc:identifier id="SomethingElse">abc</dc:identifier>
  </metadata>
</package>"#;
        let mut r = make_report();
        check_package_identifier(opf, &None, &mut r);
        assert!(fired(&r, "R16.1"));
    }

    #[test]
    fn r16_1_matching_dc_identifier_clean() {
        let opf = r#"<package unique-identifier="BookId">
  <metadata><dc:identifier id="BookId">x</dc:identifier></metadata>
</package>"#;
        let mut r = make_report();
        check_package_identifier(opf, &None, &mut r);
        assert!(!fired(&r, "R16.1"));
        assert!(!fired(&r, "R16.2"));
    }

    #[test]
    fn r16_1_matches_oeb1x_capitalized_dc_identifier() {
        // PyGlossary / OEB 1.x emits <dc:Identifier> (capital I). Case-
        // insensitive matching must bind it to unique-identifier. Issue #3.
        let opf = r#"<package unique-identifier="uid">
  <metadata>
    <dc-metadata>
      <dc:Identifier id="uid">abc</dc:Identifier>
    </dc-metadata>
  </metadata>
</package>"#;
        let mut r = make_report();
        check_package_identifier(opf, &None, &mut r);
        assert!(!fired(&r, "R16.1"));
    }

    #[test]
    fn r16_2_package_without_unique_identifier_fires() {
        let opf = r#"<package version="2.0">
  <metadata><dc:identifier id="BookId">x</dc:identifier></metadata>
</package>"#;
        let mut r = make_report();
        check_package_identifier(opf, &None, &mut r);
        assert!(fired(&r, "R16.2"));
    }

    // ---- R16.3 / R16.4 ----

    #[test]
    fn r16_3_bad_syntax_date_fires() {
        assert!(matches!(parse_w3cdtf("2024/01/15"), DateKind::BadSyntax));
        assert!(matches!(parse_w3cdtf("15-01-2024"), DateKind::BadSyntax));
        assert!(matches!(parse_w3cdtf("2024-1-1"), DateKind::BadSyntax));
    }

    #[test]
    fn r16_3_year_only_valid() {
        assert!(matches!(parse_w3cdtf("2026"), DateKind::Valid));
    }

    #[test]
    fn r16_3_yyyy_mm_valid() {
        assert!(matches!(parse_w3cdtf("2026-04"), DateKind::Valid));
    }

    #[test]
    fn r16_3_yyyy_mm_dd_valid() {
        assert!(matches!(parse_w3cdtf("2026-04-15"), DateKind::Valid));
    }

    #[test]
    fn r16_3_iso_with_time_valid() {
        assert!(matches!(
            parse_w3cdtf("2026-04-15T12:34:56Z"),
            DateKind::Valid
        ));
    }

    #[test]
    fn r16_4_invalid_calendar_date_fires() {
        assert!(matches!(
            parse_w3cdtf("2024-02-30"),
            DateKind::SyntaxOkButInvalid
        ));
        assert!(matches!(
            parse_w3cdtf("2024-13-01"),
            DateKind::SyntaxOkButInvalid
        ));
        assert!(matches!(
            parse_w3cdtf("2024-04-31"),
            DateKind::SyntaxOkButInvalid
        ));
    }

    #[test]
    fn r16_4_leap_day_valid() {
        assert!(matches!(parse_w3cdtf("2024-02-29"), DateKind::Valid));
        assert!(matches!(
            parse_w3cdtf("2023-02-29"),
            DateKind::SyntaxOkButInvalid
        ));
    }

    // ---- R16.5 ----

    #[test]
    fn r16_5_empty_dc_title_fires() {
        let opf = r#"<package>
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title></dc:title>
    <dc:creator>Someone</dc:creator>
  </metadata>
</package>"#;
        let mut r = make_report();
        check_empty_dc_elements(opf, &None, &mut r);
        assert!(fired(&r, "R16.5"));
        assert_eq!(
            r.findings.iter().filter(|f| f.rule_id == Some("R16.5")).count(),
            1
        );
    }

    #[test]
    fn r16_5_whitespace_only_dc_language_fires() {
        let opf = r#"<package><metadata><dc:language>   </dc:language></metadata></package>"#;
        let mut r = make_report();
        check_empty_dc_elements(opf, &None, &mut r);
        assert!(fired(&r, "R16.5"));
    }

    #[test]
    fn r16_5_self_closing_dc_creator_fires() {
        let opf = r#"<package><metadata><dc:creator/></metadata></package>"#;
        let mut r = make_report();
        check_empty_dc_elements(opf, &None, &mut r);
        assert!(fired(&r, "R16.5"));
    }

    #[test]
    fn r16_5_populated_dc_title_clean() {
        let opf = r#"<package><metadata><dc:title>Hi</dc:title></metadata></package>"#;
        let mut r = make_report();
        check_empty_dc_elements(opf, &None, &mut r);
        assert!(!fired(&r, "R16.5"));
    }

    // ---- R16.6 ----

    #[test]
    fn r16_6_empty_meta_without_attributes_fires() {
        let opf = r#"<package><metadata><meta></meta></metadata></package>"#;
        let mut r = make_report();
        check_empty_meta_children(opf, &None, &mut r);
        assert!(fired(&r, "R16.6"));
    }

    #[test]
    fn r16_6_meta_with_name_content_clean() {
        let opf =
            r#"<package><metadata><meta name="cover" content="cover-img"/></metadata></package>"#;
        let mut r = make_report();
        check_empty_meta_children(opf, &None, &mut r);
        assert!(!fired(&r, "R16.6"));
    }

    #[test]
    fn r16_6_meta_with_property_and_text_clean() {
        let opf = r#"<package><metadata><meta property="rendition:layout">reflowable</meta></metadata></package>"#;
        let mut r = make_report();
        check_empty_meta_children(opf, &None, &mut r);
        assert!(!fired(&r, "R16.6"));
    }

    // ---- R16.7 ----

    #[test]
    fn r16_7_uuid_scheme_with_bad_value_fires() {
        let opf = r#"<package><metadata>
            <dc:identifier opf:scheme="UUID">not-a-uuid</dc:identifier>
        </metadata></package>"#;
        let mut r = make_report();
        check_uuid_identifier(opf, &None, &mut r);
        assert!(fired(&r, "R16.7"));
    }

    #[test]
    fn r16_7_uuid_scheme_with_valid_uuid_clean() {
        let opf = r#"<package><metadata>
            <dc:identifier opf:scheme="UUID">550e8400-e29b-41d4-a716-446655440000</dc:identifier>
        </metadata></package>"#;
        let mut r = make_report();
        check_uuid_identifier(opf, &None, &mut r);
        assert!(!fired(&r, "R16.7"));
    }

    #[test]
    fn r16_7_identifier_without_scheme_attribute_clean() {
        // Critical for lemma: no opf:scheme means we must NOT fire.
        let opf = r#"<package><metadata>
            <dc:identifier id="BookId">LemmaGreekENEL</dc:identifier>
        </metadata></package>"#;
        let mut r = make_report();
        check_uuid_identifier(opf, &None, &mut r);
        assert!(!fired(&r, "R16.7"));
    }

    #[test]
    fn r16_7_identifier_with_isbn_scheme_clean() {
        let opf = r#"<package><metadata>
            <dc:identifier opf:scheme="ISBN">978-3-16-148410-0</dc:identifier>
        </metadata></package>"#;
        let mut r = make_report();
        check_uuid_identifier(opf, &None, &mut r);
        assert!(!fired(&r, "R16.7"));
    }

    #[test]
    fn uuid_regex_accepts_rfc4122() {
        assert!(is_valid_uuid("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_uuid("f47ac10b-58cc-4372-a567-0e02b2c3d479"));
        assert!(is_valid_uuid("6BA7B810-9DAD-11D1-80B4-00C04FD430C8"));
    }

    #[test]
    fn uuid_regex_rejects_non_uuid() {
        assert!(!is_valid_uuid(""));
        assert!(!is_valid_uuid("LemmaGreekENEL"));
        assert!(!is_valid_uuid("not-a-uuid"));
        // Wrong version nibble:
        assert!(!is_valid_uuid("550e8400-e29b-61d4-a716-446655440000"));
        // Wrong variant nibble:
        assert!(!is_valid_uuid("550e8400-e29b-41d4-c716-446655440000"));
    }

    // ---- R16.8 ----

    #[test]
    fn r16_8_bad_language_tag_fires() {
        let opf = r#"<package><metadata><dc:language>not_a_tag</dc:language></metadata></package>"#;
        let mut r = make_report();
        check_dc_language(opf, &None, &mut r);
        assert!(fired(&r, "R16.8"));
    }

    #[test]
    fn r16_8_good_language_tag_clean() {
        for tag in &["en", "el", "grc", "en-US", "zh-Hant", "zh-Hant-TW"] {
            let opf = format!(
                r#"<package><metadata><dc:language>{}</dc:language></metadata></package>"#,
                tag
            );
            let mut r = make_report();
            check_dc_language(&opf, &None, &mut r);
            assert!(!fired(&r, "R16.8"), "tag '{}' should be accepted", tag);
        }
    }

    #[test]
    fn bcp47_rejects_underscore_and_garbage() {
        assert!(!is_valid_bcp47(""));
        assert!(!is_valid_bcp47("en_US"));
        assert!(!is_valid_bcp47("ENGLISH"));
        assert!(!is_valid_bcp47("1234"));
        assert!(!is_valid_bcp47("not_a_tag"));
    }

    // ---- extract_attr ----

    #[test]
    fn extract_attr_finds_opf_scheme() {
        let body = r#"id="BookId" opf:scheme="UUID""#;
        assert_eq!(extract_attr(body, "opf:scheme"), Some("UUID".to_string()));
    }

    #[test]
    fn extract_attr_ignores_adjacent_prefix() {
        // `xopf:scheme` must not match a search for `opf:scheme`.
        let body = r#"xopf:scheme="UUID""#;
        assert_eq!(extract_attr(body, "opf:scheme"), None);
    }

    #[test]
    fn extract_attr_single_quoted() {
        let body = r#"id='BookId' opf:scheme='UUID'"#;
        assert_eq!(extract_attr(body, "opf:scheme"), Some("UUID".to_string()));
    }

    // ---- iter_elements_with_body ----

    #[test]
    fn iter_elements_with_body_returns_inner_and_attrs() {
        let opf = r#"<metadata>
          <dc:identifier opf:scheme="UUID">abc</dc:identifier>
          <dc:identifier id="BookId">LemmaGreekENEL</dc:identifier>
        </metadata>"#;
        let items = iter_elements_with_body(opf, "dc:identifier");
        assert_eq!(items.len(), 2);
        assert!(items[0].0.contains("opf:scheme=\"UUID\""));
        assert_eq!(items[0].1.trim(), "abc");
        assert!(!items[1].0.contains("opf:scheme"));
        assert_eq!(items[1].1.trim(), "LemmaGreekENEL");
    }
}
