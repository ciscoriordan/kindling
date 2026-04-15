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

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use lightningcss::stylesheet::{ParserOptions, StyleSheet};

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

        for (_id, (href, media_type)) in &opf.manifest {
            if !is_css_media_type(media_type) {
                continue;
            }
            let Some(text) = epub.read(href) else { continue };

            // Strip a @charset prologue if present so lightningcss can parse
            // the rest of the file even when R6.12 already flagged it. Also
            // drop a leading UTF-8 BOM for the same reason.
            let clean = strip_css_prologue(&text);

            scan_css_file(href, clean, &manifest_hrefs, opf.base_dir.as_path(), report);
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level scan entry
// ---------------------------------------------------------------------------

/// Run every CSS-targeted Cluster I rule against a single stylesheet body.
fn scan_css_file(
    href: &str,
    text: &str,
    manifest_hrefs: &HashSet<String>,
    base_dir: &Path,
    report: &mut ValidationReport,
) {
    let file = Some(PathBuf::from(href));

    // R6.13 CSS parse error. lightningcss only returns an `Err` for hard
    // syntax errors; style-level mistakes survive with `error_recovery`
    // disabled, which is what we want for a STEAL-grade check.
    if let Err(err) = StyleSheet::parse(text, ParserOptions::default()) {
        report.emit_at(
            "R6.13",
            format!("lightningcss error: {}", err),
            file.clone(),
            None,
        );
    }

    // R6.14 Forbidden position.
    for (line, value) in find_forbidden_position(text) {
        report.emit_at(
            "R6.14",
            format!("Found 'position: {}'.", value),
            file.clone(),
            Some(line),
        );
    }

    // R6.15 @import unresolvable.
    for (line, target) in find_imports(text) {
        if let Some(reason) = classify_import(&target, href, manifest_hrefs, base_dir) {
            report.emit_at(
                "R6.15",
                format!("@import \"{}\" {}.", target, reason),
                file.clone(),
                Some(line),
            );
        }
    }

    // R6.16 url() unresolvable.
    for (line, target) in find_url_refs(text) {
        if let Some(reason) = classify_url(&target, href, manifest_hrefs, base_dir) {
            report.emit_at(
                "R6.16",
                format!("url(\"{}\") {}.", target, reason),
                file.clone(),
                Some(line),
            );
        }
    }

    // R6.17 @font-face that Kindle will drop.
    for (line, problem) in find_bad_font_faces(text, href, manifest_hrefs, base_dir) {
        report.emit_at("R6.17", problem, file.clone(), Some(line));
    }

    // R6.e1 @namespace.
    for line in find_namespace_lines(text) {
        report.emit_at("R6.e1", "", file.clone(), Some(line));
    }

    // R6.e2 Unsupported @media feature.
    for (line, feat) in find_unsupported_media_features(text) {
        report.emit_at(
            "R6.e2",
            format!("Feature '{}' not supported by Kindle readers.", feat),
            file.clone(),
            Some(line),
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers: media type / BOM / @charset handling
// ---------------------------------------------------------------------------

fn is_css_media_type(mt: &str) -> bool {
    mt.eq_ignore_ascii_case("text/css")
}

/// Drop a UTF-8 BOM and a leading `@charset "..."` rule so lightningcss has
/// a clean string. Cluster B already fires R6.12 for non-UTF-8 charsets.
fn strip_css_prologue(text: &str) -> &str {
    let t = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let trimmed = t.trim_start();
    let offset = t.len() - trimmed.len();
    if !trimmed.starts_with("@charset") {
        return t;
    }
    if let Some(end) = trimmed.find(';') {
        return &t[offset + end + 1..];
    }
    t
}

/// Normalize a manifest href to its lowercase path for case-insensitive
/// resource-resolution checks. Fragment and query are dropped.
fn normalize_manifest_href(href: &str) -> String {
    let base = href.split(['?', '#']).next().unwrap_or(href);
    base.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// R6.14: forbidden `position` values
// ---------------------------------------------------------------------------

/// Forbidden values of the `position` property in reflowable Kindle content.
/// Epubcheck's CSS_007 rule class treats any non-static position on the root
/// reflowable flow as an authoring mistake because the KF8 renderer collapses
/// them. `sticky` is included because Kindle firmware ignores it silently.
const FORBIDDEN_POSITION_VALUES: &[&str] = &["fixed", "absolute", "sticky"];

/// Return `(line, value)` for every `position: <forbidden>` declaration.
fn find_forbidden_position(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    for (abs, matched) in iter_ascii_matches(&lower, "position") {
        // Must be followed by optional whitespace and a `:`.
        let after = abs + matched.len();
        let rest = &lower[after..];
        let rest_trim = rest.trim_start();
        if !rest_trim.starts_with(':') {
            continue;
        }
        // Must be a property start, not inside an identifier like `x-position`.
        if let Some(prev) = lower[..abs].chars().last() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                continue;
            }
        }
        let value_area = &rest_trim[1..];
        let value = value_area
            .split(|c: char| c == ';' || c == '}' || c == '!')
            .next()
            .unwrap_or("")
            .trim();
        for forbidden in FORBIDDEN_POSITION_VALUES {
            if value.eq_ignore_ascii_case(forbidden) {
                out.push((line_of(text, abs), (*forbidden).to_string()));
                break;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// R6.15: `@import` targets
// ---------------------------------------------------------------------------

/// Return `(line, target)` for every `@import` in the stylesheet. Handles both
/// `@import url("a.css")` and `@import "a.css"` forms.
fn find_imports(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    for (abs, _) in iter_ascii_matches(&lower, "@import") {
        let after = abs + "@import".len();
        let rest_raw = &text[after..];
        let rest = rest_raw.trim_start();
        let target = if rest.starts_with("url(") {
            parse_url_body(&rest[4..])
        } else if rest.starts_with('"') || rest.starts_with('\'') {
            let quote = rest.chars().next().unwrap();
            let after_q = &rest[1..];
            after_q.find(quote).map(|end| after_q[..end].to_string())
        } else {
            None
        };
        if let Some(t) = target {
            out.push((line_of(text, abs), t));
        }
    }
    out
}

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
// R6.16: `url()` references
// ---------------------------------------------------------------------------

/// Return `(line, target)` for every `url(...)` reference in the stylesheet.
/// Skips empty targets, data: URIs, and urls that are part of an `@import`
/// or `@namespace` prelude (those are R6.15 and R6.e1 territory).
fn find_url_refs(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("url(") {
        let abs = pos + idx;
        // Look back over whitespace and identifier characters to find the
        // preceding token. If it is `@import` or `@namespace`, skip this hit
        // so those rule owners do not get duplicate R6.16 findings.
        let before = lower[..abs].trim_end();
        if before.ends_with("@import") || belongs_to_namespace(before) {
            pos = abs + 4;
            continue;
        }
        let body_start = abs + 4;
        if let Some(target) = parse_url_body(&text[body_start..]) {
            if !target.is_empty() && !target.starts_with("data:") {
                out.push((line_of(text, abs), target));
            }
        }
        pos = body_start;
    }
    out
}

/// True if the trailing portion of `before` looks like a `@namespace ...`
/// prelude. The form is `@namespace [prefix] url(...)` where `prefix` is an
/// optional identifier. We accept either `before.ends_with("@namespace")`
/// directly, or `before.ends_with("@namespace <prefix>")` where `<prefix>`
/// is a single identifier.
fn belongs_to_namespace(before: &str) -> bool {
    // Direct: `@namespace url(...)` with no prefix, after trim.
    if before.ends_with("@namespace") {
        return true;
    }
    // Prefixed: strip one trailing identifier and a single whitespace block,
    // then check again.
    let after_ident = before.trim_end_matches(|c: char| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    });
    // Must have actually stripped at least one character (a real prefix).
    if after_ident.len() == before.len() {
        return false;
    }
    let after_space = after_ident.trim_end();
    after_space.ends_with("@namespace")
}

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
// R6.17: @font-face declarations
// ---------------------------------------------------------------------------

/// Scan every `@font-face` block and return a location + human message for any
/// that Kindle will silently drop: missing `src`, or a `src url()` pointing
/// outside the manifest.
fn find_bad_font_faces(
    text: &str,
    css_href: &str,
    manifest_hrefs: &HashSet<String>,
    base_dir: &Path,
) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("@font-face") {
        let abs = pos + idx;
        // Find the opening `{`.
        let after = abs + "@font-face".len();
        let Some(open_rel) = text[after..].find('{') else { break };
        let open = after + open_rel;
        // Balance braces to get the block body.
        let mut depth = 0i32;
        let mut close = None;
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else { break };
        let block = &text[open + 1..close];
        let block_lower = block.to_ascii_lowercase();

        // R6.17a: missing `src:` descriptor entirely.
        if !block_lower.contains("src:") {
            out.push((
                line_of(text, abs),
                "@font-face block has no src descriptor; Kindle will drop it."
                    .to_string(),
            ));
        } else {
            // R6.17b: every `src url()` must resolve in the manifest.
            for (rel_line, target) in find_url_refs(block) {
                if target.starts_with('#') {
                    continue;
                }
                if is_external_url(&target)
                    || !resolves_to_manifest(&target, css_href, manifest_hrefs, base_dir)
                {
                    let abs_line = line_of(text, open + 1) + rel_line - 1;
                    out.push((
                        abs_line,
                        format!(
                            "@font-face src url(\"{}\") not in manifest; \
                             Kindle will drop the font.",
                            target
                        ),
                    ));
                }
            }
        }
        pos = close + 1;
    }
    out
}

// ---------------------------------------------------------------------------
// R6.e1: `@namespace` rule
// ---------------------------------------------------------------------------

/// Return the 1-based line of every `@namespace` rule in `text`.
fn find_namespace_lines(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    for (abs, _) in iter_ascii_matches(&lower, "@namespace") {
        // Must be at a property-ish boundary, not inside an identifier.
        if let Some(prev) = lower[..abs].chars().last() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                continue;
            }
        }
        out.push(line_of(text, abs));
    }
    out
}

// ---------------------------------------------------------------------------
// R6.e2: unsupported @media features
// ---------------------------------------------------------------------------

/// Media features Kindle readers do not honour. `hover`, `pointer`, and
/// `color-gamut` never match an e-ink device; `prefers-color-scheme` is
/// unsupported on all current firmware.
const UNSUPPORTED_MEDIA_FEATURES: &[&str] = &[
    "hover",
    "any-hover",
    "pointer",
    "any-pointer",
    "color-gamut",
    "prefers-color-scheme",
];

/// Return `(line, feature_name)` for every unsupported @media feature used in
/// a media query. This scans the text of each `@media` prelude.
fn find_unsupported_media_features(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("@media") {
        let abs = pos + idx;
        // Must be at an identifier boundary.
        if let Some(prev) = lower[..abs].chars().last() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                pos = abs + "@media".len();
                continue;
            }
        }
        let after = abs + "@media".len();
        // Prelude is everything up to the next `{`.
        let Some(brace_rel) = text[after..].find('{') else {
            break;
        };
        let prelude = &lower[after..after + brace_rel];
        let prelude_orig_start = after;
        for feat in UNSUPPORTED_MEDIA_FEATURES {
            if let Some(f_idx) = prelude.find(feat) {
                // Make sure the hit is a whole feature token, not a prefix of
                // a longer identifier.
                let before_ok = if f_idx == 0 {
                    true
                } else {
                    let prev = prelude.as_bytes()[f_idx - 1];
                    !prev.is_ascii_alphanumeric() && prev != b'-' && prev != b'_'
                };
                let end = f_idx + feat.len();
                let after_ok = if end == prelude.len() {
                    true
                } else {
                    let next = prelude.as_bytes()[end];
                    !next.is_ascii_alphanumeric() && next != b'_'
                };
                if before_ok && after_ok {
                    out.push((
                        line_of(text, prelude_orig_start + f_idx),
                        (*feat).to_string(),
                    ));
                }
            }
        }
        pos = after + brace_rel + 1;
    }
    out
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

/// Parse the body of a `url(...)` call. Accepts optional whitespace and both
/// quoted and unquoted forms. Returns the target string without the closing
/// paren.
fn parse_url_body(body: &str) -> Option<String> {
    let trimmed = body.trim_start();
    let rest: &str;
    let end_char: char;
    if let Some(stripped) = trimmed.strip_prefix('"') {
        rest = stripped;
        end_char = '"';
    } else if let Some(stripped) = trimmed.strip_prefix('\'') {
        rest = stripped;
        end_char = '\'';
    } else {
        // Unquoted: read until `)` or whitespace.
        let end = trimmed
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(trimmed.len());
        return Some(trimmed[..end].to_string());
    }
    let end = rest.find(end_char)?;
    Some(rest[..end].to_string())
}

/// 1-based line number of `byte_offset` in `content`.
fn line_of(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

/// Yield every byte offset where `needle` occurs as a case-sensitive substring
/// in `haystack`. Used to walk the lowercased stylesheet text.
fn iter_ascii_matches<'a>(
    haystack: &'a str,
    needle: &'a str,
) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    let mut pos = 0usize;
    std::iter::from_fn(move || {
        if pos >= haystack.len() {
            return None;
        }
        let idx = haystack[pos..].find(needle)?;
        let abs = pos + idx;
        pos = abs + needle.len();
        Some((abs, needle))
    })
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- R6.13 parse error ----

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

    // ---- R6.14 forbidden position ----

    #[test]
    fn r6_14_position_fixed_fires() {
        let css = "header { position: fixed; top: 0; }";
        let hits = find_forbidden_position(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "fixed");
    }

    #[test]
    fn r6_14_position_absolute_fires() {
        let css = "div { position: absolute; }";
        let hits = find_forbidden_position(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "absolute");
    }

    #[test]
    fn r6_14_position_sticky_fires() {
        let css = ".bar { position : sticky ; }";
        let hits = find_forbidden_position(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "sticky");
    }

    #[test]
    fn r6_14_position_relative_clean() {
        let css = "p { position: relative; }";
        assert!(find_forbidden_position(css).is_empty());
    }

    #[test]
    fn r6_14_position_static_clean() {
        let css = "p { position: static; }";
        assert!(find_forbidden_position(css).is_empty());
    }

    #[test]
    fn r6_14_ignores_position_inside_identifier() {
        let css = "p { --my-position: absolute; }";
        // `--my-position:` is a custom property, not the `position` property.
        assert!(find_forbidden_position(css).is_empty());
    }

    // ---- R6.15 @import ----

    #[test]
    fn r6_15_import_url_quoted_detected() {
        let css = "@import url(\"reset.css\");\nbody {}";
        let hits = find_imports(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "reset.css");
    }

    #[test]
    fn r6_15_import_bare_string_detected() {
        let css = "@import \"reset.css\";";
        let hits = find_imports(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "reset.css");
    }

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

    // ---- R6.16 url() ----

    #[test]
    fn r6_16_url_refs_detected() {
        let css = "body { background: url(\"bg.png\"); }";
        let hits = find_url_refs(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "bg.png");
    }

    #[test]
    fn r6_16_data_uri_skipped() {
        let css = "i { background: url(\"data:image/png;base64,AAA\"); }";
        assert!(find_url_refs(css).is_empty());
    }

    #[test]
    fn r6_16_import_url_not_double_counted() {
        let css = "@import url(\"reset.css\");\nbody { background: url(\"bg.png\"); }";
        let urls = find_url_refs(css);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].1, "bg.png");
    }

    #[test]
    fn r6_16_namespace_url_not_double_counted() {
        // R6.e1 owns @namespace; R6.16 must not also flag the url() body.
        let css = "@namespace svg url(\"http://www.w3.org/2000/svg\");\nbody {}";
        let urls = find_url_refs(css);
        assert!(urls.is_empty(), "urls = {:?}", urls);
    }

    #[test]
    fn r6_16_namespace_url_without_prefix_not_double_counted() {
        let css = "@namespace url(\"http://example.com/ns\");\nbody {}";
        let urls = find_url_refs(css);
        assert!(urls.is_empty(), "urls = {:?}", urls);
    }

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

    // ---- R6.17 @font-face ----

    #[test]
    fn r6_17_missing_src_fires() {
        let css = "@font-face { font-family: 'A'; }";
        let base = std::env::temp_dir();
        let hrefs: HashSet<String> = HashSet::new();
        let hits = find_bad_font_faces(css, "a.css", &hrefs, &base);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("no src"));
    }

    #[test]
    fn r6_17_src_in_manifest_clean() {
        let css = "@font-face { font-family: 'A'; src: url(\"fonts/a.ttf\"); }";
        let base = std::env::temp_dir();
        let mut hrefs = HashSet::new();
        hrefs.insert("fonts/a.ttf".to_string());
        let hits = find_bad_font_faces(css, "a.css", &hrefs, &base);
        assert!(hits.is_empty(), "hits = {:?}", hits);
    }

    #[test]
    fn r6_17_external_src_fires() {
        let css = "@font-face { font-family: 'A'; src: url(\"https://fonts.example/a.ttf\"); }";
        let base = std::env::temp_dir();
        let hrefs: HashSet<String> = HashSet::new();
        let hits = find_bad_font_faces(css, "a.css", &hrefs, &base);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].1.contains("@font-face src"));
    }

    // ---- R6.e1 @namespace ----

    #[test]
    fn r6_e1_namespace_rule_fires() {
        let css = "@namespace svg url(\"http://www.w3.org/2000/svg\");\nbody {}";
        assert_eq!(find_namespace_lines(css).len(), 1);
    }

    #[test]
    fn r6_e1_namespace_inside_identifier_clean() {
        let css = "/* word-namespace is not a rule */ body {}";
        assert!(find_namespace_lines(css).is_empty());
    }

    // ---- R6.e2 @media features ----

    #[test]
    fn r6_e2_hover_media_feature_fires() {
        let css = "@media (hover: hover) { a:hover { color: blue; } }";
        let hits = find_unsupported_media_features(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "hover");
    }

    #[test]
    fn r6_e2_prefers_color_scheme_fires() {
        let css = "@media (prefers-color-scheme: dark) { body { color: white; } }";
        let hits = find_unsupported_media_features(css);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, "prefers-color-scheme");
    }

    #[test]
    fn r6_e2_width_media_clean() {
        let css = "@media (min-width: 600px) { body {} }";
        assert!(find_unsupported_media_features(css).is_empty());
    }

    #[test]
    fn r6_e2_screen_media_clean() {
        let css = "@media screen { body {} }";
        assert!(find_unsupported_media_features(css).is_empty());
    }

    // ---- helpers ----

    #[test]
    fn strip_css_prologue_removes_bom_and_charset() {
        let s = "\u{FEFF}@charset \"UTF-8\";\nbody {}";
        assert_eq!(strip_css_prologue(s), "\nbody {}");
    }

    #[test]
    fn strip_css_prologue_leaves_plain_css() {
        let s = "body { color: red; }";
        assert_eq!(strip_css_prologue(s), s);
    }

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
    fn parse_url_body_handles_quoted() {
        assert_eq!(parse_url_body("\"a.png\")").as_deref(), Some("a.png"));
        assert_eq!(parse_url_body("'a.png')").as_deref(), Some("a.png"));
    }

    #[test]
    fn parse_url_body_handles_unquoted() {
        assert_eq!(parse_url_body("a.png)").as_deref(), Some("a.png"));
        assert_eq!(parse_url_body(" a.png )").as_deref(), Some("a.png"));
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
