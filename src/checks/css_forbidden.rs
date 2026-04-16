// Section 6: CSS forbidden properties and parse rules (R6.13 through R6.17,
// plus R6.e1 and R6.e2).
//
// Cluster I ports epubcheck CSS rule subset that actually matters for KDP:
//   R6.13  CSS parse error                  (epubcheck CSS_005 / CSS_027)
//   R6.14  Forbidden position declaration   (epubcheck CSS_007 class)
//   R6.15  @import target unresolvable      (epubcheck CSS_015)
//   R6.16  url() target unresolvable        (epubcheck CSS_020)
//   R6.17  @font-face silently dropped      (epubcheck CSS_008 / CSS_017)
//   R6.e1  @namespace rule present          (epubcheck CSS_025)
//   R6.e2  Unsupported @media feature       (epubcheck CSS_019)
//
// Cluster B (parse_encoding.rs) already owns R6.6 through R6.12 and handles
// BOM / @charset checks, so this module intentionally skips those concerns.
//
// All text-scanning extraction lives on `ExtractedEpub::css_summary` so this
// module only consumes the cache and applies the rule-specific classification
// logic (manifest resolution, external-URL detection).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct CssForbiddenChecks;

impl Check for CssForbiddenChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R6.13", "R6.14", "R6.15", "R6.16", "R6.17", "R6.e1", "R6.e2",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // Pre-compute the lowercase manifest href set so resource-resolution
        // checks (R6.15 / R6.16 / R6.17) are case-insensitive against the
        // authoring spelling.
        let manifest_hrefs: HashSet<String> = opf
            .manifest
            .values()
            .map(|(href, _)| normalize_manifest_href(href))
            .collect();

        let base_dir = opf.base_dir.clone();

        for (_id, (href, media_type)) in &opf.manifest {
            if !is_css_media_type(media_type) {
                continue;
            }
            let Some(summary) = epub.css_summary(href) else { continue };
            let file = Some(PathBuf::from(href));

            // R6.13 CSS parse error. lightningcss only returns an `Err` for
            // hard syntax errors; style-level mistakes survive with
            // `error_recovery` disabled, which is what we want for a
            // STEAL-grade check.
            if let Some(err) = &summary.parse_error {
                report.emit_at(
                    "R6.13",
                    format!("lightningcss error: {}", err),
                    file.clone(),
                    None,
                );
            }

            // R6.14 Forbidden position.
            for (line, value) in &summary.forbidden_positions {
                report.emit_at(
                    "R6.14",
                    format!("Found 'position: {}'.", value),
                    file.clone(),
                    Some(*line),
                );
            }

            // R6.15 @import unresolvable.
            for (line, target) in &summary.imports {
                if let Some(reason) =
                    classify_import(target, href, &manifest_hrefs, base_dir.as_path())
                {
                    report.emit_at(
                        "R6.15",
                        format!("@import \"{}\" {}.", target, reason),
                        file.clone(),
                        Some(*line),
                    );
                }
            }

            // R6.16 url() unresolvable.
            for (line, target) in &summary.url_refs {
                if let Some(reason) =
                    classify_url(target, href, &manifest_hrefs, base_dir.as_path())
                {
                    report.emit_at(
                        "R6.16",
                        format!("url(\"{}\") {}.", target, reason),
                        file.clone(),
                        Some(*line),
                    );
                }
            }

            // R6.17 @font-face that Kindle will drop.
            for face in &summary.font_faces {
                if face.missing_src {
                    report.emit_at(
                        "R6.17",
                        "@font-face block has no src descriptor; Kindle will drop it."
                            .to_string(),
                        file.clone(),
                        Some(face.line),
                    );
                    continue;
                }
                for (line, target) in &face.src_urls {
                    if target.starts_with('#') {
                        continue;
                    }
                    if is_external_url(target)
                        || !resolves_to_manifest(
                            target,
                            href,
                            &manifest_hrefs,
                            base_dir.as_path(),
                        )
                    {
                        report.emit_at(
                            "R6.17",
                            format!(
                                "@font-face src url(\"{}\") not in manifest; \
                                 Kindle will drop the font.",
                                target
                            ),
                            file.clone(),
                            Some(*line),
                        );
                    }
                }
            }

            // R6.e1 @namespace.
            for line in &summary.namespace_lines {
                report.emit_at("R6.e1", "", file.clone(), Some(*line));
            }

            // R6.e2 Unsupported @media feature.
            for (line, feat) in &summary.media_features {
                report.emit_at(
                    "R6.e2",
                    format!("Feature '{}' not supported by Kindle readers.", feat),
                    file.clone(),
                    Some(*line),
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers: media type / manifest normalization
// ---------------------------------------------------------------------------

fn is_css_media_type(mt: &str) -> bool {
    mt.eq_ignore_ascii_case("text/css")
}

/// Normalize a manifest href to its lowercase path for case-insensitive
/// resource-resolution checks. Fragment and query are dropped.
fn normalize_manifest_href(href: &str) -> String {
    let base = href.split(['?', '#']).next().unwrap_or(href);
    base.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// R6.15 classifier: `@import` targets
// ---------------------------------------------------------------------------

/// None if the import target is resolvable in the manifest. Otherwise return a
/// short human-readable reason.
fn classify_import(
    target: &str,
    css_href: &str,
    manifest_hrefs: &HashSet<String>,
    base_dir: &Path,
) -> Option<&'static str> {
    if is_external_url(target) {
        return Some("is an external URL");
    }
    if !resolves_to_manifest(target, css_href, manifest_hrefs, base_dir) {
        return Some("is not in the manifest");
    }
    None
}

// ---------------------------------------------------------------------------
// R6.16 classifier: `url()` references
// ---------------------------------------------------------------------------

/// None if the url target is resolvable. Otherwise a short reason. Fragment-
/// only targets like `url(#gradient)` are ignored (SVG internal refs).
fn classify_url(
    target: &str,
    css_href: &str,
    manifest_hrefs: &HashSet<String>,
    base_dir: &Path,
) -> Option<&'static str> {
    if target.starts_with('#') {
        return None;
    }
    if is_external_url(target) {
        return Some("is an external URL");
    }
    if !resolves_to_manifest(target, css_href, manifest_hrefs, base_dir) {
        return Some("is not in the manifest");
    }
    None
}

// ---------------------------------------------------------------------------
// Shared small helpers
// ---------------------------------------------------------------------------

/// True if `target` is an absolute http(s):// URL or a protocol-relative URL.
fn is_external_url(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("//")
        || lower.starts_with("ftp://")
}

/// Resolve `target` against the css file's directory and look it up in the
/// manifest href set. Case-insensitive. Fragment and query are dropped.
fn resolves_to_manifest(
    target: &str,
    css_href: &str,
    manifest_hrefs: &HashSet<String>,
    base_dir: &Path,
) -> bool {
    let bare = target.split(['?', '#']).next().unwrap_or(target);
    if bare.is_empty() {
        return true;
    }
    // Resolve `target` relative to the css file's directory, then express the
    // result as a path relative to `base_dir` (= the manifest root).
    let css_path = Path::new(css_href);
    let css_dir = css_path.parent().unwrap_or_else(|| Path::new(""));
    let joined = css_dir.join(bare);
    let normalized = normalize_relative_path(&joined);
    let needle = normalized.to_ascii_lowercase();
    if manifest_hrefs.contains(&needle) {
        return true;
    }
    // Fall back: some authoring tools reference the href verbatim from the
    // manifest root, ignoring the stylesheet's directory.
    let raw = bare.to_ascii_lowercase();
    if manifest_hrefs.contains(&raw) {
        return true;
    }
    // Final fallback: check disk. A file that physically exists relative to
    // the content root (e.g. a resource added by a repacker after OPF write)
    // is treated as present to avoid false positives on round-tripped books.
    let disk_path = base_dir.join(&normalized);
    disk_path.exists()
}

/// Collapse `./` and `..` segments in a relative path into a clean string with
/// forward slashes. Leading `..` are preserved so the result still matches
/// literal `../` manifest hrefs rather than escaping the manifest root.
fn normalize_relative_path(path: &Path) -> String {
    let mut out: Vec<String> = Vec::new();
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if out.last().is_some_and(|s| s != "..") {
                    out.pop();
                } else {
                    out.push("..".to_string());
                }
            }
            Component::Normal(os) => out.push(os.to_string_lossy().to_string()),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out.join("/")
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lightningcss::stylesheet::{ParserOptions, StyleSheet};

    // ---- R6.13 parse error ----
    //
    // R6.13 detection lives in `CssSummary::parse_error`, which is built by
    // `ExtractedEpub::css_summary`. These tests pin lightningcss's behaviour
    // for the inputs we rely on so a future bump cannot silently turn a
    // hard error into a recoverable one.

    #[test]
    fn r6_13_valid_css_parses_clean() {
        let css = "body { color: red; } h1 { font-size: 2em; }";
        assert!(StyleSheet::parse(css, ParserOptions::default()).is_ok());
    }

    #[test]
    fn r6_13_garbage_returns_err() {
        // Truly malformed input with no recoverable structure.
        let css = "this is not valid css at all $#@!";
        assert!(StyleSheet::parse(css, ParserOptions::default()).is_err());
    }

    #[test]
    fn r6_13_stray_brace_returns_err() {
        let css = "{";
        assert!(StyleSheet::parse(css, ParserOptions::default()).is_err());
    }

    #[test]
    fn r6_13_declaration_without_block_returns_err() {
        // Lightningcss treats a bare `@media` with no prelude/block as a
        // parse error.
        let css = "@media";
        assert!(StyleSheet::parse(css, ParserOptions::default()).is_err());
    }

    // ---- R6.15 @import classifier ----

    #[test]
    fn r6_15_external_import_classified() {
        let base = std::env::temp_dir();
        let hrefs: HashSet<String> = HashSet::new();
        assert!(
            classify_import("https://cdn/reset.css", "a.css", &hrefs, &base).is_some()
        );
    }

    #[test]
    fn r6_15_manifested_import_clean() {
        let base = std::env::temp_dir();
        let mut hrefs = HashSet::new();
        hrefs.insert("reset.css".to_string());
        assert!(classify_import("reset.css", "a.css", &hrefs, &base).is_none());
    }

    // ---- R6.16 url() classifier ----

    #[test]
    fn r6_16_fragment_only_url_clean() {
        let base = std::env::temp_dir();
        let hrefs: HashSet<String> = HashSet::new();
        assert!(classify_url("#gradient", "a.css", &hrefs, &base).is_none());
    }

    #[test]
    fn r6_16_external_url_classified() {
        let base = std::env::temp_dir();
        let hrefs: HashSet<String> = HashSet::new();
        assert!(
            classify_url("https://cdn/bg.png", "a.css", &hrefs, &base).is_some()
        );
    }

    #[test]
    fn r6_16_relative_url_resolves_against_css_dir() {
        let base = std::env::temp_dir();
        let mut hrefs = HashSet::new();
        hrefs.insert("images/bg.png".to_string());
        assert!(
            classify_url("../images/bg.png", "css/style.css", &hrefs, &base).is_none()
        );
    }

    // ---- helpers ----

    #[test]
    fn normalize_relative_path_collapses_dotdot() {
        let p = Path::new("css/../images/bg.png");
        assert_eq!(normalize_relative_path(p), "images/bg.png");
    }

    #[test]
    fn normalize_relative_path_keeps_leading_dotdot() {
        let p = Path::new("../shared/bg.png");
        assert_eq!(normalize_relative_path(p), "../shared/bg.png");
    }

    #[test]
    fn is_external_url_detects_schemes() {
        assert!(is_external_url("http://a"));
        assert!(is_external_url("HTTPS://a"));
        assert!(is_external_url("//cdn/a.css"));
        assert!(!is_external_url("a.css"));
        assert!(!is_external_url("../a.css"));
    }
}
