// Section 6 parse-time, DOCTYPE, and encoding checks (R6.6 through R6.12).
//
// These are the "Vader Down bug class" rules: parse-time landmines that
// silently break Kindle rendering without obvious errors. They catch:
//   R6.6  XML 1.0 required              (epubcheck HTM_001)
//   R6.7  No external entities          (epubcheck HTM_003 / OPF_073)
//   R6.8  Irregular DOCTYPE             (epubcheck HTM_004)
//   R6.9  EPUB namespace wrong          (epubcheck HTM_010)  // Vader Down
//   R6.10 Undeclared entity             (epubcheck HTM_011)
//   R6.11 HTML must be UTF-8            (epubcheck HTM_058 / RSC_027 / RSC_028)
//   R6.12 CSS must be UTF-8             (epubcheck CSS_003 / CSS_004)

use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct ParseEncodingChecks;

impl Check for ParseEncodingChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R6.6", "R6.7", "R6.8", "R6.9", "R6.10", "R6.11", "R6.12",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        for (_id, (href, media_type)) in &opf.manifest {
            let full = opf.base_dir.join(href);
            let bytes = match fs::read(&full) {
                Ok(b) => b,
                Err(_) => continue,
            };

            if is_xhtml_media_type(media_type) {
                scan_xhtml(href, &bytes, report);
            } else if is_css_media_type(media_type) {
                scan_css(href, &bytes, report);
            }
        }

        // R6.7 also applies to the OPF file itself.
        if let Ok(opf_bytes) = fs::read(&epub.opf_path) {
            let href = epub
                .opf_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "content.opf".to_string());
            scan_opf_entities(&href, &opf_bytes, report);
        }
    }
}

/// True if `mt` is any recognized (X)HTML media type.
fn is_xhtml_media_type(mt: &str) -> bool {
    let l = mt.to_ascii_lowercase();
    l == "application/xhtml+xml" || l == "text/html"
}

/// True if `mt` is a CSS media type.
fn is_css_media_type(mt: &str) -> bool {
    let l = mt.to_ascii_lowercase();
    l == "text/css"
}

/// Run every XHTML-targeted rule against a single file.
fn scan_xhtml(href: &str, bytes: &[u8], report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    // R6.11 BOM / encoding checks run on raw bytes first.
    check_html_encoding(href, bytes, report);

    // Strip a UTF-8 BOM if present, then decode best-effort. Non-UTF-8 payloads
    // already fired R6.11 above; fall back to a lossy decode so the remaining
    // substring checks still work on ASCII-compatible content.
    let content_bytes = strip_utf8_bom(bytes);
    let content = match std::str::from_utf8(content_bytes) {
        Ok(s) => s.to_string(),
        Err(_) => String::from_utf8_lossy(content_bytes).to_string(),
    };

    // R6.6 XML 1.0 required.
    if let Some(ver) = xml_declaration_version(&content) {
        if ver != "1.0" {
            report.emit_at(
                "R6.6",
                format!("XML declaration sets version=\"{}\".", ver),
                file.clone(),
                None,
            );
        }
    }

    // R6.7 No external entities.
    if has_external_entity(&content) {
        report.emit_at("R6.7", "", file.clone(), None);
    }

    // R6.8 Irregular DOCTYPE.
    if let Some(dt) = extract_doctype(&content) {
        if !is_canonical_doctype(&dt) {
            let trimmed: String = dt.chars().take(120).collect();
            report.emit_at(
                "R6.8",
                format!("Found: <!DOCTYPE{}...", trimmed),
                file.clone(),
                None,
            );
        }
    }

    // R6.9 EPUB namespace wrong.
    if let Some(bad) = wrong_epub_namespace(&content) {
        report.emit_at(
            "R6.9",
            format!("Found xmlns:epub=\"{}\".", bad),
            file.clone(),
            None,
        );
    }

    // R6.10 Undeclared named entity.
    if let Some(name) = find_undeclared_entity(&content) {
        report.emit_at(
            "R6.10",
            format!("Entity: &{};", name),
            file.clone(),
            None,
        );
    }
}

/// Run every CSS-targeted rule against a single file.
fn scan_css(href: &str, bytes: &[u8], report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    // R6.12 CSS must be UTF-8. BOM check first.
    if starts_with_utf16_bom(bytes) {
        report.emit_at("R6.12", "File begins with a UTF-16 BOM.", file.clone(), None);
        return;
    }

    let content_bytes = strip_utf8_bom(bytes);
    let content = String::from_utf8_lossy(content_bytes);

    if let Some(cs) = css_charset(&content) {
        if !cs.eq_ignore_ascii_case("utf-8") {
            report.emit_at(
                "R6.12",
                format!("@charset is \"{}\".", cs),
                file.clone(),
                None,
            );
        }
    }
}

/// R6.7 also runs against the OPF body for the HTM_003 / OPF_073 pair.
fn scan_opf_entities(href: &str, bytes: &[u8], report: &mut ValidationReport) {
    let content_bytes = strip_utf8_bom(bytes);
    let content = String::from_utf8_lossy(content_bytes);
    if has_external_entity(&content) {
        report.emit_at("R6.7", "", Some(PathBuf::from(href)), None);
    }
}

// ---------------------------------------------------------------------------
// Helpers: BOM / encoding
// ---------------------------------------------------------------------------

/// True if `bytes` starts with a UTF-16 LE or BE BOM.
fn starts_with_utf16_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

/// Strip a leading UTF-8 BOM so the rest of the scanners see plain text.
fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    }
}

/// Emit R6.11 if `bytes` starts with a non-UTF-8 BOM or the XML declaration
/// advertises a non-UTF-8 encoding.
fn check_html_encoding(href: &str, bytes: &[u8], report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    if starts_with_utf16_bom(bytes) {
        report.emit_at("R6.11", "File begins with a UTF-16 BOM.", file, None);
        return;
    }

    let content_bytes = strip_utf8_bom(bytes);
    let content = String::from_utf8_lossy(content_bytes);
    if let Some(enc) = xml_declaration_encoding(&content) {
        if !enc.eq_ignore_ascii_case("utf-8") {
            report.emit_at(
                "R6.11",
                format!("XML declaration encoding is \"{}\".", enc),
                file,
                None,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers: XML declaration parsing
// ---------------------------------------------------------------------------

/// Locate the `<?xml ... ?>` prolog and return its body without the markers.
fn xml_declaration_body(content: &str) -> Option<&str> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("<?xml")?;
    let end = rest.find("?>")?;
    Some(&rest[..end])
}

/// Return the `version` pseudo-attribute of the XML declaration, if present.
fn xml_declaration_version(content: &str) -> Option<String> {
    let body = xml_declaration_body(content)?;
    extract_pseudo_attr(body, "version")
}

/// Return the `encoding` pseudo-attribute of the XML declaration, if present.
fn xml_declaration_encoding(content: &str) -> Option<String> {
    let body = xml_declaration_body(content)?;
    extract_pseudo_attr(body, "encoding")
}

/// Extract `name="value"` (or `name='value'`) out of an attribute list.
fn extract_pseudo_attr(body: &str, name: &str) -> Option<String> {
    let needle = format!("{}=", name);
    let idx = body.find(&needle)?;
    let rest = &body[idx + needle.len()..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// Helpers: DOCTYPE and entities
// ---------------------------------------------------------------------------

/// True if this DOCTYPE declares an `<!ENTITY ... SYSTEM|PUBLIC ...>`.
fn has_external_entity(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("<!entity") {
        let start = pos + idx;
        let end = match lower[start..].find('>') {
            Some(e) => start + e,
            None => break,
        };
        let body = &lower[start..end];
        if body.contains(" system") || body.contains(" public") {
            return true;
        }
        pos = end + 1;
    }
    false
}

/// Return the body of the first `<!DOCTYPE ... >` declaration.
fn extract_doctype(content: &str) -> Option<String> {
    let lower = content.to_ascii_lowercase();
    let idx = lower.find("<!doctype")?;
    let start = idx + "<!doctype".len();
    let rest = &content[start..];
    // Balance brackets in case the doctype has an internal subset.
    let mut depth: i32 = 0;
    let mut end_idx: Option<usize> = None;
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '>' if depth <= 0 => {
                end_idx = Some(i);
                break;
            }
            _ => {}
        }
    }
    let end = end_idx?;
    Some(rest[..end].to_string())
}

/// Canonical DOCTYPE recognized by Kindle: HTML5 or the XHTML 1.0/1.1 set.
fn is_canonical_doctype(doctype_body: &str) -> bool {
    // HTML5: `<!DOCTYPE html>` leaves only " html" (or "html") in the body.
    let body = doctype_body.trim();
    if body.eq_ignore_ascii_case("html") {
        return true;
    }

    // XHTML 1.0 / 1.1 canonical forms, matched by looking for the DTD URL.
    let lower = body.to_ascii_lowercase();
    let known_dtds = [
        "dtd/xhtml1-strict.dtd",
        "dtd/xhtml1-transitional.dtd",
        "dtd/xhtml1-frameset.dtd",
        "dtd/xhtml11.dtd",
    ];
    for dtd in known_dtds {
        if lower.contains(dtd) {
            return true;
        }
    }

    false
}

/// Return the bad URI if `xmlns:epub="..."` is anything other than the
/// canonical EPUB ops namespace. Returns `None` if the attribute is missing
/// or already correct.
fn wrong_epub_namespace(content: &str) -> Option<String> {
    const NEEDLES: [&str; 2] = ["xmlns:epub=\"", "xmlns:epub='"];
    for needle in NEEDLES {
        if let Some(idx) = content.find(needle) {
            let rest = &content[idx + needle.len()..];
            let quote = needle.chars().last().unwrap();
            if let Some(end) = rest.find(quote) {
                let uri = &rest[..end];
                if uri != "http://www.idpf.org/2007/ops" {
                    return Some(uri.to_string());
                }
            }
        }
    }
    None
}

/// Whitelist of named entities we allow without an explicit declaration.
const ALLOWED_ENTITIES: &[&str] = &[
    // XML 1.0 predefined set.
    "amp", "lt", "gt", "quot", "apos",
    // Common HTML5 entities used in publishing.
    "nbsp", "copy", "reg", "trade", "ndash", "mdash", "hellip", "lsquo",
    "rsquo", "ldquo", "rdquo", "bull",
];

/// Return the name of the first undeclared named entity found in `content`.
fn find_undeclared_entity(content: &str) -> Option<String> {
    let bytes = content.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] != b'&' {
            i += 1;
            continue;
        }
        // Numeric references like &#65; or &#x41; are always fine.
        if i + 1 < bytes.len() && bytes[i + 1] == b'#' {
            i += 1;
            continue;
        }
        // Scan a named reference: [&][A-Za-z][A-Za-z0-9]*[;]
        let start = i + 1;
        let mut j = start;
        if j >= bytes.len() || !bytes[j].is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        while j < bytes.len() && bytes[j].is_ascii_alphanumeric() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b';' {
            i = j;
            continue;
        }
        let name = std::str::from_utf8(&bytes[start..j]).unwrap_or("");
        if !ALLOWED_ENTITIES.contains(&name) {
            return Some(name.to_string());
        }
        i = j + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers: CSS @charset
// ---------------------------------------------------------------------------

/// Return the argument of the first `@charset "..."` rule, if present.
fn css_charset(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    let rest = trimmed.strip_prefix("@charset")?;
    let rest = rest.trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- R6.6 XML 1.0 required ----

    #[test]
    fn r6_6_xml_11_declaration_fires() {
        let s = "<?xml version=\"1.1\" encoding=\"UTF-8\"?><html/>";
        assert_eq!(xml_declaration_version(s).as_deref(), Some("1.1"));
    }

    #[test]
    fn r6_6_xml_10_declaration_clean() {
        let s = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><html/>";
        assert_eq!(xml_declaration_version(s).as_deref(), Some("1.0"));
    }

    // ---- R6.7 External entities ----

    #[test]
    fn r6_7_system_entity_fires() {
        let s = "<!DOCTYPE foo [ <!ENTITY xxe SYSTEM \"file:///etc/passwd\"> ]><html/>";
        assert!(has_external_entity(s));
    }

    #[test]
    fn r6_7_public_entity_fires() {
        let s = "<!DOCTYPE foo [ <!ENTITY bar PUBLIC \"-//X//B\" \"b.dtd\"> ]><html/>";
        assert!(has_external_entity(s));
    }

    #[test]
    fn r6_7_no_entity_clean() {
        let s = "<?xml version=\"1.0\"?><!DOCTYPE html><html/>";
        assert!(!has_external_entity(s));
    }

    // ---- R6.8 Irregular DOCTYPE ----

    #[test]
    fn r6_8_html5_doctype_canonical() {
        let dt = extract_doctype("<!DOCTYPE html><html/>").unwrap();
        assert!(is_canonical_doctype(&dt));
    }

    #[test]
    fn r6_8_xhtml1_strict_canonical() {
        let s = "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.0 Strict//EN\" \
                 \"http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd\">";
        let dt = extract_doctype(s).unwrap();
        assert!(is_canonical_doctype(&dt));
    }

    #[test]
    fn r6_8_irregular_doctype_fires() {
        let s = "<!DOCTYPE potato SYSTEM \"potato.dtd\">";
        let dt = extract_doctype(s).unwrap();
        assert!(!is_canonical_doctype(&dt));
    }

    #[test]
    fn r6_8_no_doctype_ignored() {
        assert!(extract_doctype("<html/>").is_none());
    }

    // ---- R6.9 EPUB namespace ----

    #[test]
    fn r6_9_wrong_namespace_fires() {
        let s = "<html xmlns:epub=\"http://www.example.com/ops\"/>";
        assert_eq!(
            wrong_epub_namespace(s).as_deref(),
            Some("http://www.example.com/ops")
        );
    }

    #[test]
    fn r6_9_correct_namespace_clean() {
        let s = "<html xmlns:epub=\"http://www.idpf.org/2007/ops\"/>";
        assert!(wrong_epub_namespace(s).is_none());
    }

    #[test]
    fn r6_9_no_epub_namespace_clean() {
        let s = "<html xmlns=\"http://www.w3.org/1999/xhtml\"/>";
        assert!(wrong_epub_namespace(s).is_none());
    }

    // ---- R6.10 Undeclared entity ----

    #[test]
    fn r6_10_allowed_entities_clean() {
        let s = "<p>a &amp; b &nbsp; c &mdash; d &hellip;</p>";
        assert!(find_undeclared_entity(s).is_none());
    }

    #[test]
    fn r6_10_unknown_entity_fires() {
        let s = "<p>Hello &foo; world</p>";
        assert_eq!(find_undeclared_entity(s).as_deref(), Some("foo"));
    }

    #[test]
    fn r6_10_numeric_references_clean() {
        let s = "<p>&#8212; &#x2014;</p>";
        assert!(find_undeclared_entity(s).is_none());
    }

    // ---- R6.11 HTML encoding ----

    #[test]
    fn r6_11_utf16le_bom_detected() {
        let bytes = [0xFF, 0xFE, 0x3C, 0x00];
        assert!(starts_with_utf16_bom(&bytes));
    }

    #[test]
    fn r6_11_utf16be_bom_detected() {
        let bytes = [0xFE, 0xFF, 0x00, 0x3C];
        assert!(starts_with_utf16_bom(&bytes));
    }

    #[test]
    fn r6_11_utf8_bom_clean() {
        let bytes = [0xEF, 0xBB, 0xBF, b'<'];
        assert!(!starts_with_utf16_bom(&bytes));
        assert_eq!(strip_utf8_bom(&bytes), &[b'<']);
    }

    #[test]
    fn r6_11_non_utf8_encoding_fires() {
        let s = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><html/>";
        assert_eq!(
            xml_declaration_encoding(s).as_deref(),
            Some("ISO-8859-1")
        );
    }

    #[test]
    fn r6_11_utf8_encoding_clean() {
        let s = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><html/>";
        assert!(xml_declaration_encoding(s)
            .unwrap()
            .eq_ignore_ascii_case("utf-8"));
    }

    // ---- R6.12 CSS encoding ----

    #[test]
    fn r6_12_bad_charset_fires() {
        let s = "@charset \"ISO-8859-1\";\nbody { color: red; }";
        assert_eq!(css_charset(s).as_deref(), Some("ISO-8859-1"));
    }

    #[test]
    fn r6_12_utf8_charset_clean() {
        let s = "@charset \"UTF-8\";\nbody { color: red; }";
        assert!(css_charset(s).unwrap().eq_ignore_ascii_case("utf-8"));
    }

    #[test]
    fn r6_12_no_charset_clean() {
        let s = "body { color: red; }";
        assert!(css_charset(s).is_none());
    }
}
