// Section 11: Fixed-layout EPUB rules (R11.1 through R11.9).
//
// These rules apply to fixed-layout EPUBs (comic books, textbooks, children's
// picture books) and port the STEAL-grade subset of epubcheck's HTM_046
// through HTM_060 and OPF_011 families. They fire only on Profile::Comic or
// Profile::Textbook so that reflowable books and dictionaries are unaffected.
//
//   R11.1 Missing rendition:layout=pre-paginated in OPF when content is FL  (OPF_011)
//   R11.2 Fixed-layout XHTML missing <meta name="viewport">                 (HTM_046)
//   R11.3 Viewport meta lacks both width and height                         (HTM_047)
//   R11.4 Invalid rendition:spread value                                    (HTM_048)
//   R11.5 Invalid rendition:orientation value                               (HTM_049)
//   R11.6 Invalid rendition:layout value                                    (HTM_050)
//   R11.7 Conflict: OPF pre-paginated but XHTML has no viewport meta        (HTM_051)
//   R11.8 Fixed-layout page has no image content (breaks comic e-ink)       (HTM_052)
//   R11.9 Missing original-resolution metadata on KF8 fixed-layout          (HTM_053)
//
// Profile gating: these rules run only when `epub.profile` is Comic or
// Textbook. Default and Dict profiles short-circuit at the top of `run`.

use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::profile::Profile;
use crate::validate::ValidationReport;

pub struct FixedLayoutChecks;

impl Check for FixedLayoutChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R11.1", "R11.2", "R11.3", "R11.4", "R11.5", "R11.6", "R11.7",
            "R11.8", "R11.9",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        // Profile gate: only fixed-layout profiles pay the scan cost.
        if epub.profile != Profile::Comic && epub.profile != Profile::Textbook {
            return;
        }

        let opf = &epub.opf;

        // Read the raw OPF bytes so we can inspect namespace-prefixed attrs
        // like `property="rendition:spread"` exactly as the author wrote them.
        let opf_content = match fs::read_to_string(&epub.opf_path) {
            Ok(s) => s,
            Err(_) => return,
        };

        let opf_file = Some(opf_file_label(epub));

        // Distinguish EPUB 3 `rendition:layout` from Amazon legacy
        // `<meta name="fixed-layout" content="true"/>`. The viewport-meta
        // rules (R11.2 / R11.7) only apply to EPUB 3 pre-paginated content
        // because legacy KF8 comics get their dimensions from
        // `original-resolution` in the OPF.
        let uses_rendition_layout = opf_has_rendition_layout_preparginated(&opf_content);

        // R11.1: OPF must declare rendition:layout=pre-paginated when content
        // looks fixed-layout (meaning auto-detection trips, but is_fixed_layout
        // is false because there is no explicit OPF declaration).
        if !opf.is_fixed_layout {
            report.emit_at(
                "R11.1",
                "Viewport meta present but OPF lacks \
                 <meta property=\"rendition:layout\">pre-paginated</meta>.",
                opf_file.clone(),
                None,
            );
        }

        // R11.4 / R11.5 / R11.6: validate rendition:* values in OPF metadata.
        check_rendition_values(&opf_content, &opf_file, report);

        // R11.9: pre-paginated KF8 builds should carry original-resolution.
        if opf.is_fixed_layout && opf.original_resolution.is_none() {
            report.emit_at(
                "R11.9",
                "Fixed-layout OPF has no \
                 <meta name=\"original-resolution\" content=\"WxH\"/>.",
                opf_file.clone(),
                None,
            );
        }

        // R11.2 / R11.3 / R11.7 / R11.8: scan each spine XHTML.
        // Use raw_itemrefs so we can honor per-itemref
        // properties="rendition:layout-reflowable" overrides.
        for item in &opf.raw_itemrefs {
            let props_lower = item.properties.to_ascii_lowercase();
            if props_lower.contains("rendition:layout-reflowable") {
                continue;
            }
            let Some((href, _mt)) = opf.manifest.get(&item.idref) else {
                continue;
            };
            let full = opf.base_dir.join(href);
            let bytes = match fs::read(&full) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let content = match std::str::from_utf8(&bytes) {
                Ok(s) => s.to_string(),
                Err(_) => String::from_utf8_lossy(&bytes).to_string(),
            };
            scan_spine_xhtml(href, &content, uses_rendition_layout, report);
        }
    }
}

// ---------------------------------------------------------------------------
// File-label helper
// ---------------------------------------------------------------------------

/// Pretty OPF filename for location reporting.
fn opf_file_label(epub: &ExtractedEpub) -> PathBuf {
    epub.opf_path
        .file_name()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("content.opf"))
}

// ---------------------------------------------------------------------------
// Spine XHTML scanning (R11.2, R11.3, R11.7, R11.8)
// ---------------------------------------------------------------------------

/// Run all per-spine-item rules against a single XHTML file.
///
/// `uses_rendition_layout` indicates whether the OPF explicitly declares
/// `<meta property="rendition:layout">pre-paginated</meta>` (EPUB 3 form).
/// Amazon-legacy `<meta name="fixed-layout" content="true"/>` KF8 comics do
/// not require per-page viewport metas because Kindle derives page
/// dimensions from the OPF `original-resolution` value instead. R11.2 and
/// R11.7 therefore only fire when the EPUB 3 form is used.
fn scan_spine_xhtml(
    href: &str,
    content: &str,
    uses_rendition_layout: bool,
    report: &mut ValidationReport,
) {
    let file = Some(PathBuf::from(href));

    let viewport = find_viewport_meta(content);

    match viewport {
        None => {
            // R11.2: EPUB 3 fixed-layout XHTML must carry a viewport meta.
            if uses_rendition_layout {
                report.emit_at(
                    "R11.2",
                    "No <meta name=\"viewport\" ...> in this fixed-layout \
                     content document.",
                    file.clone(),
                    None,
                );
                // R11.7: OPF says pre-paginated but the XHTML has no
                // viewport -- the OPF and the content disagree.
                report.emit_at(
                    "R11.7",
                    "OPF declares rendition:layout=pre-paginated but this \
                     content has no viewport meta.",
                    file.clone(),
                    None,
                );
            }
        }
        Some(ref vp) => {
            // R11.3: viewport must specify both width and height.
            if !vp.has_width || !vp.has_height {
                report.emit_at(
                    "R11.3",
                    format!(
                        "Viewport meta is \"{}\"; must include both width= \
                         and height=.",
                        vp.raw
                    ),
                    file.clone(),
                    None,
                );
            }
        }
    }

    // R11.8: fixed-layout page must contain at least one image element
    // (<img>, <image>, or <svg> with an <image>). A pure-text fixed-layout
    // page renders as a blank rectangle on e-ink comic devices.
    if !has_image_element(content) {
        report.emit_at(
            "R11.8",
            "Fixed-layout page has no <img>, <image>, or <svg image>; \
             Kindle comic readers render it blank.",
            file.clone(),
            None,
        );
    }
}

/// True if the raw OPF text contains an EPUB 3
/// `<meta property="rendition:layout">pre-paginated</meta>` declaration.
fn opf_has_rendition_layout_preparginated(opf_content: &str) -> bool {
    for (prop, value) in iter_rendition_meta(opf_content) {
        if prop == "rendition:layout" && value == "pre-paginated" {
            return true;
        }
    }
    false
}

/// A parsed `<meta name="viewport" content="...">` element.
#[derive(Debug, Clone)]
struct ViewportMeta {
    raw: String,
    has_width: bool,
    has_height: bool,
}

/// Find the first `<meta name="viewport" ...>` element in `content`.
/// Returns None if no such element exists.
fn find_viewport_meta(content: &str) -> Option<ViewportMeta> {
    let lower = content.to_ascii_lowercase();
    let mut cursor = 0usize;
    while let Some(idx) = lower[cursor..].find("<meta") {
        let abs = cursor + idx;
        let after = abs + 5;
        let end = match lower[after..].find('>') {
            Some(e) => after + e,
            None => break,
        };
        let tag_lower = &lower[after..end];
        if tag_lower.contains("name=\"viewport\"") || tag_lower.contains("name='viewport'") {
            // Extract raw (case-preserving) content attribute.
            let tag_raw = &content[after..end];
            let content_val = extract_attr_value(tag_raw, "content").unwrap_or_default();
            let low = content_val.to_ascii_lowercase();
            // Match either `width=` or `width :`.
            let has_width = low.contains("width=") || low.contains("width :") || low.contains("width=");
            let has_height = low.contains("height=") || low.contains("height :") || low.contains("height=");
            // Also accept CSS-style separation `width: 1072`.
            let has_width = has_width || low.contains("width:");
            let has_height = has_height || low.contains("height:");
            return Some(ViewportMeta {
                raw: content_val,
                has_width,
                has_height,
            });
        }
        cursor = end + 1;
    }
    None
}

/// Return true if `content` contains at least one image-bearing element.
fn has_image_element(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    // Match common forms. We look for the open-tag start so that closing
    // `</img>` sequences (which shouldn't exist, but might) don't fool us.
    lower.contains("<img ")
        || lower.contains("<img/")
        || lower.contains("<img>")
        || lower.contains("<image ")
        || lower.contains("<image/")
        || lower.contains("<image>")
        || lower.contains("<svg ")
        || lower.contains("<svg>")
}

/// Pull `attr="value"` or `attr='value'` out of an open-tag body.
fn extract_attr_value(tag_body: &str, attr: &str) -> Option<String> {
    let needle_eq = format!("{}=", attr);
    let idx = tag_body.find(&needle_eq)?;
    let rest = &tag_body[idx + needle_eq.len()..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &rest[1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

// ---------------------------------------------------------------------------
// OPF rendition:* value checks (R11.4, R11.5, R11.6)
// ---------------------------------------------------------------------------

/// Scan OPF metadata for `<meta property="rendition:spread|orientation|layout">`
/// elements and validate their text-content values.
fn check_rendition_values(
    opf_content: &str,
    opf_file: &Option<PathBuf>,
    report: &mut ValidationReport,
) {
    for (prop, value) in iter_rendition_meta(opf_content) {
        match prop.as_str() {
            "rendition:spread" => {
                if !is_valid_spread(&value) {
                    report.emit_at(
                        "R11.4",
                        format!(
                            "rendition:spread=\"{}\" is not one of none, \
                             landscape, portrait, both, auto.",
                            value
                        ),
                        opf_file.clone(),
                        None,
                    );
                }
            }
            "rendition:orientation" => {
                if !is_valid_orientation(&value) {
                    report.emit_at(
                        "R11.5",
                        format!(
                            "rendition:orientation=\"{}\" is not one of \
                             auto, landscape, portrait.",
                            value
                        ),
                        opf_file.clone(),
                        None,
                    );
                }
            }
            "rendition:layout" => {
                if !is_valid_layout(&value) {
                    report.emit_at(
                        "R11.6",
                        format!(
                            "rendition:layout=\"{}\" is not one of \
                             pre-paginated, reflowable.",
                            value
                        ),
                        opf_file.clone(),
                        None,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Iterate `<meta property="..."]...</meta>` pairs and return the property +
/// inner text body. Also emits entries for self-closing `<meta .../>` forms,
/// which surface an empty value.
fn iter_rendition_meta(content: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let bytes = content.as_bytes();
    while cursor < bytes.len() {
        let idx = match content[cursor..].find("<meta") {
            Some(i) => cursor + i,
            None => break,
        };
        let after = idx + 5;
        // `<metadata>` starts with `<meta` too; require the next char to be
        // whitespace or `/` or `>` so we only match genuine meta elements.
        let next = match bytes.get(after) {
            Some(b) => *b,
            None => break,
        };
        if !(next == b' ' || next == b'\t' || next == b'\n' || next == b'\r'
             || next == b'/' || next == b'>')
        {
            cursor = after;
            continue;
        }
        let end = match content[after..].find('>') {
            Some(e) => after + e,
            None => break,
        };
        let open_body = &content[after..end];
        let self_closing = open_body.trim_end().ends_with('/');
        let trimmed_body = open_body.trim_end_matches('/').trim();

        let property = extract_attr_value(trimmed_body, "property");
        if let Some(prop) = property {
            if prop.starts_with("rendition:") {
                let value = if self_closing {
                    // Self-closing <meta ... /> cannot carry a text value.
                    String::new()
                } else {
                    // Text body up to the matching </meta>.
                    let body_start = end + 1;
                    let close = "</meta>";
                    let close_idx = content[body_start..]
                        .to_ascii_lowercase()
                        .find(close)
                        .map(|i| body_start + i);
                    match close_idx {
                        Some(ci) => content[body_start..ci].trim().to_string(),
                        None => String::new(),
                    }
                };
                out.push((prop, value));
            }
        }
        cursor = end + 1;
    }
    out
}

/// Permitted values for `rendition:spread`.
fn is_valid_spread(value: &str) -> bool {
    matches!(
        value,
        "none" | "landscape" | "portrait" | "both" | "auto"
    )
}

/// Permitted values for `rendition:orientation`.
fn is_valid_orientation(value: &str) -> bool {
    matches!(value, "auto" | "landscape" | "portrait")
}

/// Permitted values for `rendition:layout`.
fn is_valid_layout(value: &str) -> bool {
    matches!(value, "pre-paginated" | "reflowable")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- find_viewport_meta ----

    #[test]
    fn viewport_with_width_and_height_detected() {
        let s = r##"<html><head><meta name="viewport" content="width=1072, height=1448"/></head></html>"##;
        let vp = find_viewport_meta(s).unwrap();
        assert!(vp.has_width);
        assert!(vp.has_height);
        assert!(vp.raw.contains("1072"));
    }

    #[test]
    fn viewport_missing_height_flagged() {
        let s = r##"<meta name="viewport" content="width=1072"/>"##;
        let vp = find_viewport_meta(s).unwrap();
        assert!(vp.has_width);
        assert!(!vp.has_height);
    }

    #[test]
    fn viewport_css_colon_syntax_accepted() {
        // Some authors write CSS-style colon separators.
        let s = r##"<meta name="viewport" content="width: 1072; height: 1448"/>"##;
        let vp = find_viewport_meta(s).unwrap();
        assert!(vp.has_width);
        assert!(vp.has_height);
    }

    #[test]
    fn no_viewport_returns_none() {
        let s = r##"<html><head><title>t</title></head></html>"##;
        assert!(find_viewport_meta(s).is_none());
    }

    #[test]
    fn non_viewport_meta_is_ignored() {
        let s = r##"<meta name="author" content="Jane"/>"##;
        assert!(find_viewport_meta(s).is_none());
    }

    // ---- has_image_element ----

    #[test]
    fn img_tag_detected() {
        assert!(has_image_element(r##"<p><img src="a.jpg"/></p>"##));
    }

    #[test]
    fn svg_image_detected() {
        assert!(has_image_element(r##"<svg><image href="a.png"/></svg>"##));
    }

    #[test]
    fn plain_text_page_has_no_image() {
        assert!(!has_image_element("<p>All text, no pictures.</p>"));
    }

    // ---- rendition:* validators ----

    #[test]
    fn valid_spread_values_accepted() {
        for v in &["none", "landscape", "portrait", "both", "auto"] {
            assert!(is_valid_spread(v), "{} should be valid", v);
        }
    }

    #[test]
    fn invalid_spread_values_rejected() {
        assert!(!is_valid_spread("left"));
        assert!(!is_valid_spread(""));
        assert!(!is_valid_spread("NONE"));
    }

    #[test]
    fn valid_orientation_values_accepted() {
        for v in &["auto", "landscape", "portrait"] {
            assert!(is_valid_orientation(v));
        }
    }

    #[test]
    fn invalid_orientation_rejected() {
        assert!(!is_valid_orientation("both"));
        assert!(!is_valid_orientation("sideways"));
    }

    #[test]
    fn valid_layout_values_accepted() {
        assert!(is_valid_layout("pre-paginated"));
        assert!(is_valid_layout("reflowable"));
    }

    #[test]
    fn invalid_layout_rejected() {
        assert!(!is_valid_layout("fixed"));
        assert!(!is_valid_layout("prepaginated"));
        assert!(!is_valid_layout(""));
    }

    // ---- iter_rendition_meta ----

    #[test]
    fn iter_rendition_meta_finds_layout() {
        let opf = r##"<package><metadata>
          <meta property="rendition:layout">pre-paginated</meta>
          <meta property="rendition:spread">landscape</meta>
        </metadata></package>"##;
        let items = iter_rendition_meta(opf);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], ("rendition:layout".to_string(), "pre-paginated".to_string()));
        assert_eq!(items[1], ("rendition:spread".to_string(), "landscape".to_string()));
    }

    #[test]
    fn iter_rendition_meta_ignores_other_props() {
        let opf = r##"<package><metadata>
          <meta property="dcterms:modified">2026-04-15T00:00:00Z</meta>
        </metadata></package>"##;
        assert_eq!(iter_rendition_meta(opf).len(), 0);
    }

    #[test]
    fn iter_rendition_meta_handles_bad_value() {
        let opf = r##"<package><metadata>
          <meta property="rendition:orientation">sideways</meta>
        </metadata></package>"##;
        let items = iter_rendition_meta(opf);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].1, "sideways");
    }

    // ---- check_rendition_values end-to-end ----

    #[test]
    fn r11_4_bad_spread_fires() {
        let opf = r##"<package><metadata>
          <meta property="rendition:spread">diagonal</meta>
        </metadata></package>"##;
        let mut r = ValidationReport::new();
        check_rendition_values(opf, &None, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.4")));
    }

    #[test]
    fn r11_5_bad_orientation_fires() {
        let opf = r##"<package><metadata>
          <meta property="rendition:orientation">backwards</meta>
        </metadata></package>"##;
        let mut r = ValidationReport::new();
        check_rendition_values(opf, &None, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.5")));
    }

    #[test]
    fn r11_6_bad_layout_fires() {
        let opf = r##"<package><metadata>
          <meta property="rendition:layout">fixed</meta>
        </metadata></package>"##;
        let mut r = ValidationReport::new();
        check_rendition_values(opf, &None, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.6")));
    }

    #[test]
    fn good_rendition_values_are_clean() {
        let opf = r##"<package><metadata>
          <meta property="rendition:layout">pre-paginated</meta>
          <meta property="rendition:spread">landscape</meta>
          <meta property="rendition:orientation">auto</meta>
        </metadata></package>"##;
        let mut r = ValidationReport::new();
        check_rendition_values(opf, &None, &mut r);
        assert_eq!(
            r.findings.iter().filter(|f| {
                matches!(f.rule_id, Some("R11.4") | Some("R11.5") | Some("R11.6"))
            }).count(),
            0
        );
    }

    // ---- scan_spine_xhtml ----

    #[test]
    fn r11_2_missing_viewport_fires_on_rendition_layout_opf() {
        let html = r##"<html><head><title>p1</title></head>
            <body><img src="p1.jpg"/></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p1.xhtml", html, true, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.2")));
    }

    #[test]
    fn r11_2_not_fired_on_amazon_legacy_fixed_layout() {
        // Kindle legacy `<meta name="fixed-layout" content="true"/>` uses
        // `original-resolution` instead of per-page viewports; do not fire.
        let html = r##"<html><body><img src="p.jpg"/></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p.xhtml", html, false, &mut r);
        assert!(!r.findings.iter().any(|f| f.rule_id == Some("R11.2")));
        assert!(!r.findings.iter().any(|f| f.rule_id == Some("R11.7")));
    }

    #[test]
    fn r11_3_viewport_missing_height_fires() {
        let html = r##"<html><head>
            <meta name="viewport" content="width=1072"/>
            </head><body><img src="p1.jpg"/></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p1.xhtml", html, true, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.3")));
        // But R11.2 should NOT fire because a viewport IS present.
        assert!(!r.findings.iter().any(|f| f.rule_id == Some("R11.2")));
    }

    #[test]
    fn r11_7_conflict_fires_when_rendition_layout_and_no_viewport() {
        let html = r##"<html><body><img src="p.jpg"/></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p.xhtml", html, true, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.7")));
    }

    #[test]
    fn r11_8_plaintext_page_fires() {
        let html = r##"<html><head>
            <meta name="viewport" content="width=1072, height=1448"/>
            </head><body><p>All text.</p></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p.xhtml", html, true, &mut r);
        assert!(r.findings.iter().any(|f| f.rule_id == Some("R11.8")));
    }

    #[test]
    fn r11_8_page_with_img_is_clean() {
        let html = r##"<html><head>
            <meta name="viewport" content="width=1072, height=1448"/>
            </head><body><img src="p.jpg"/></body></html>"##;
        let mut r = ValidationReport::new();
        scan_spine_xhtml("p.xhtml", html, true, &mut r);
        assert!(!r.findings.iter().any(|f| f.rule_id == Some("R11.8")));
    }

    // ---- extract_attr_value ----

    #[test]
    fn extract_attr_value_handles_single_quotes() {
        let body = r##"name='viewport' content='width=1072'"##;
        assert_eq!(extract_attr_value(body, "content"), Some("width=1072".to_string()));
    }

    #[test]
    fn extract_attr_value_handles_double_quotes() {
        let body = r##"name="viewport" content="width=1072""##;
        assert_eq!(extract_attr_value(body, "content"), Some("width=1072".to_string()));
    }

    // ---- iter_rendition_meta edge cases ----

    #[test]
    fn iter_rendition_skips_metadata_element() {
        // Make sure we don't mis-match `<metadata>` as `<meta>`.
        let opf = r##"<package><metadata><dc:title>x</dc:title></metadata></package>"##;
        assert_eq!(iter_rendition_meta(opf).len(), 0);
    }

    // ---- opf_has_rendition_layout_preparginated ----

    #[test]
    fn rendition_layout_prepaginated_recognised() {
        let opf = r##"<package><metadata>
          <meta property="rendition:layout">pre-paginated</meta>
        </metadata></package>"##;
        assert!(opf_has_rendition_layout_preparginated(opf));
    }

    #[test]
    fn amazon_legacy_fixed_layout_not_rendition_layout() {
        // Kindle `<meta name="fixed-layout" content="true"/>` is not the
        // EPUB 3 form and must not be confused with it.
        let opf = r##"<package><metadata>
          <meta name="fixed-layout" content="true"/>
          <meta name="original-resolution" content="1072x1448"/>
        </metadata></package>"##;
        assert!(!opf_has_rendition_layout_preparginated(opf));
    }

    #[test]
    fn reflowable_rendition_layout_not_recognised() {
        let opf = r##"<package><metadata>
          <meta property="rendition:layout">reflowable</meta>
        </metadata></package>"##;
        assert!(!opf_has_rendition_layout_preparginated(opf));
    }
}
