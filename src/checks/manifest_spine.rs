// Section 7 manifest/spine integrity rules (R7.1 through R7.13).
//
// These checks mirror the epubcheck OPF_* family that catches structural
// inconsistencies between the OPF manifest, the spine, and files on disk.
// Rule ids map to epubcheck codes as follows:
//   R7.1  OPF_003  file present but not in manifest
//   R7.2  OPF_013  declared media-type does not match file bytes
//   R7.3  OPF_029  file bytes do not match any declared media-type
//   R7.4  OPF_033  spine contains no linear content at all
//   R7.5  OPF_034  duplicate itemref idref
//   R7.6  OPF_035  text/html used where application/xhtml+xml was expected
//   R7.7  OPF_037  deprecated media-type
//   R7.8  OPF_040  dangling fallback chain
//   R7.9  OPF_041  dangling fallback-style chain
//   R7.10 OPF_042  non-permissible spine media-type with no fallback
//   R7.11 OPF_043  spine fallback chain never reaches xhtml/svg
//   R7.12 OPF_074  duplicate manifest href
//   R7.13 OPF_099  manifest item href points at the OPF file itself

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::opf::ManifestItem;
use crate::validate::ValidationReport;

/// Build artifacts we expect to find beside an unpacked OPF that must not
/// trip R7.1 even when they are not in the manifest.
const IGNORED_EXTENSIONS: &[&str] = &[
    "epub", "mobi", "azw", "azw3", "kf8", "kfx", "kpf", "ds_store",
];

/// File names that must be ignored anywhere under the content root.
const IGNORED_FILE_NAMES: &[&str] = &[
    ".DS_Store",
    "Thumbs.db",
    "mimetype",
];

/// Directory names whose entire subtrees are skipped by R7.1.
const IGNORED_DIR_NAMES: &[&str] = &[
    "META-INF",
    ".git",
    "__MACOSX",
];

/// Canonical xhtml/svg media-types that satisfy R7.10 / R7.11.
const CORE_MEDIA_TYPES: &[&str] = &[
    "application/xhtml+xml",
    "image/svg+xml",
    "application/x-dtbook+xml",
];

/// Deprecated media-types that trigger R7.7.
const DEPRECATED_MEDIA_TYPES: &[(&str, &str)] = &[
    ("image/jpg", "image/jpeg"),
    ("text/xml", "application/xml"),
    ("application/x-dtbook+xml", "application/xhtml+xml"),
    ("text/x-oeb1-document", "application/xhtml+xml"),
];

pub struct ManifestSpineChecks;

impl Check for ManifestSpineChecks {
    fn ids(&self) -> &'static [&'static str] {
        &[
            "R7.1", "R7.2", "R7.3", "R7.4", "R7.5", "R7.6", "R7.7",
            "R7.8", "R7.9", "R7.10", "R7.11", "R7.12", "R7.13",
        ]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // R7.1: every file under base_dir must be declared in the manifest.
        check_undeclared_files(opf.base_dir.as_path(), &epub.opf_path, &opf.manifest_items, report);

        // R7.2, R7.3: declared vs. actual media-type.
        check_media_type_magic(opf.base_dir.as_path(), &opf.manifest_items, report);

        // R7.4: spine must have at least one linear itemref.
        check_spine_all_nonlinear(&opf.raw_itemrefs, report);

        // R7.5: duplicate itemref idref.
        check_duplicate_itemrefs(&opf.raw_itemrefs, report);

        // R7.6: text/html declared where xhtml was expected.
        for item in &opf.manifest_items {
            if is_text_html_for_xhtml(&item.href, &item.media_type) {
                report.emit_at(
                    "R7.6",
                    format!(
                        "Item id=\"{}\" href=\"{}\" uses text/html.",
                        item.id, item.href
                    ),
                    Some(PathBuf::from(item.href.clone())),
                    None,
                );
            }
        }

        // R7.7: deprecated media-types.
        for item in &opf.manifest_items {
            if let Some(replacement) = deprecated_replacement(&item.media_type) {
                report.emit_at(
                    "R7.7",
                    format!(
                        "Item id=\"{}\" href=\"{}\" uses deprecated media-type \"{}\" \
                         (prefer \"{}\").",
                        item.id, item.href, item.media_type, replacement
                    ),
                    Some(PathBuf::from(item.href.clone())),
                    None,
                );
            }
        }

        // R7.8, R7.9: dangling fallback / fallback-style.
        check_fallback_targets(&opf.manifest_items, report);

        // R7.10, R7.11: spine media-type permissibility and fallback chain.
        check_spine_media_types(&opf.manifest_items, &opf.raw_itemrefs, report);

        // R7.12: duplicate manifest href.
        check_duplicate_hrefs(&opf.manifest_items, report);

        // R7.13: manifest href pointing at the OPF itself.
        check_self_reference(&opf.manifest_items, &epub.opf_path, opf.base_dir.as_path(), report);
    }
}

// ---------------------------------------------------------------------------
// R7.1 file-system walk
// ---------------------------------------------------------------------------

/// Recursively walk `root` and fire R7.1 for anything that is not declared in
/// the manifest and is not a known build artifact or the OPF itself.
fn check_undeclared_files(
    root: &Path,
    opf_path: &Path,
    items: &[ManifestItem],
    report: &mut ValidationReport,
) {
    if !root.is_dir() {
        return;
    }
    let manifest_paths: HashSet<PathBuf> = items
        .iter()
        .map(|i| normalize(&root.join(strip_fragment(&i.href))))
        .collect();
    let opf_canonical = normalize(opf_path);

    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if is_ignored_dir_name(&name) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            if is_ignored_file_name(&name) {
                continue;
            }
            if is_ignored_extension(&path) {
                continue;
            }
            if normalize(&path) == opf_canonical {
                continue;
            }
            if manifest_paths.contains(&normalize(&path)) {
                continue;
            }
            let rel = match path.strip_prefix(root) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(_) => path.to_string_lossy().to_string(),
            };
            report.emit_at(
                "R7.1",
                format!("File \"{}\" is under the content root but not declared in the manifest.", rel),
                Some(PathBuf::from(rel)),
                None,
            );
        }
    }
}

fn is_ignored_extension(path: &Path) -> bool {
    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_ascii_lowercase(),
        None => return false,
    };
    IGNORED_EXTENSIONS.contains(&ext.as_str())
}

fn is_ignored_file_name(name: &str) -> bool {
    IGNORED_FILE_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIR_NAMES.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// Strip a `#fragment` from an href so the path compares cleanly to disk.
fn strip_fragment(href: &str) -> &str {
    match href.find('#') {
        Some(i) => &href[..i],
        None => href,
    }
}

/// Resolve `..` and canonicalize lexically so comparisons work across the
/// symlink and absolute-path variants of the same file.
fn normalize(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

// ---------------------------------------------------------------------------
// R7.2 / R7.3 magic bytes
// ---------------------------------------------------------------------------

/// What kind of content a file's first few bytes look like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectedKind {
    Jpeg,
    Png,
    Gif,
    Svg,
    Xhtml,
    Css,
    Unknown,
}

/// Inspect each manifest item's declared media-type against the file bytes.
fn check_media_type_magic(
    base_dir: &Path,
    items: &[ManifestItem],
    report: &mut ValidationReport,
) {
    for item in items {
        let path = base_dir.join(strip_fragment(&item.href));
        let bytes = match fs::read(&path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let detected = detect_kind(&bytes);
        let declared = item.media_type.to_ascii_lowercase();

        if let Some(expected_kinds) = media_type_to_kinds(&declared) {
            if detected != DetectedKind::Unknown && !expected_kinds.contains(&detected) {
                report.emit_at(
                    "R7.2",
                    format!(
                        "Item id=\"{}\" href=\"{}\" declares media-type \"{}\" but file bytes \
                         look like {}.",
                        item.id, item.href, item.media_type, kind_name(detected)
                    ),
                    Some(PathBuf::from(item.href.clone())),
                    None,
                );
                continue;
            }
        } else if detected != DetectedKind::Unknown && !declared.is_empty() {
            // Declared an exotic or unknown media-type but the bytes are something
            // we can confidently identify: fire R7.3.
            report.emit_at(
                "R7.3",
                format!(
                    "Item id=\"{}\" href=\"{}\" has declared media-type \"{}\" that does not \
                     match detected content \"{}\".",
                    item.id, item.href, item.media_type, kind_name(detected)
                ),
                Some(PathBuf::from(item.href.clone())),
                None,
            );
        }
    }
}

/// Map a declared media-type to the set of file kinds that satisfy it.
fn media_type_to_kinds(media_type: &str) -> Option<&'static [DetectedKind]> {
    match media_type {
        "image/jpeg" | "image/jpg" => Some(&[DetectedKind::Jpeg]),
        "image/png" => Some(&[DetectedKind::Png]),
        "image/gif" => Some(&[DetectedKind::Gif]),
        "image/svg+xml" => Some(&[DetectedKind::Svg, DetectedKind::Xhtml]),
        "application/xhtml+xml" | "text/html" => Some(&[DetectedKind::Xhtml, DetectedKind::Svg]),
        "text/css" => Some(&[DetectedKind::Css, DetectedKind::Unknown]),
        _ => None,
    }
}

/// Human-readable name for error messages.
fn kind_name(kind: DetectedKind) -> &'static str {
    match kind {
        DetectedKind::Jpeg => "JPEG",
        DetectedKind::Png => "PNG",
        DetectedKind::Gif => "GIF",
        DetectedKind::Svg => "SVG",
        DetectedKind::Xhtml => "XHTML/HTML",
        DetectedKind::Css => "CSS",
        DetectedKind::Unknown => "unknown",
    }
}

/// Detect the kind of a file from its first few bytes.
pub(crate) fn detect_kind(bytes: &[u8]) -> DetectedKind {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return DetectedKind::Jpeg;
    }
    if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        return DetectedKind::Png;
    }
    if bytes.starts_with(&[0x47, 0x49, 0x46, 0x38]) {
        return DetectedKind::Gif;
    }
    // Text-mode probes: skip a UTF-8 BOM and whitespace.
    let probe = text_probe(bytes, 256);
    let trimmed = probe.trim_start();
    if trimmed.starts_with("<?xml") {
        // XML could be SVG, XHTML, NCX, OPF, or something else. We only claim
        // a positive identification when we can see the root element.
        if probe.contains("<svg") {
            return DetectedKind::Svg;
        }
        if probe.contains("<html") || probe.contains("<!DOCTYPE html") {
            return DetectedKind::Xhtml;
        }
        return DetectedKind::Unknown;
    }
    if trimmed.starts_with("<svg") {
        return DetectedKind::Svg;
    }
    if trimmed.starts_with("<html") || trimmed.starts_with("<!DOCTYPE html") {
        return DetectedKind::Xhtml;
    }
    if trimmed.starts_with("@charset") || trimmed.starts_with("/*") {
        return DetectedKind::Css;
    }
    DetectedKind::Unknown
}

/// Decode up to `limit` bytes as UTF-8 for lightweight text probing.
fn text_probe(bytes: &[u8], limit: usize) -> &str {
    let slice = &bytes[..bytes.len().min(limit)];
    let stripped = if slice.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &slice[3..]
    } else {
        slice
    };
    std::str::from_utf8(stripped).unwrap_or("")
}

// ---------------------------------------------------------------------------
// R7.4 / R7.5 spine scans
// ---------------------------------------------------------------------------

/// R7.4: emit if every itemref is `linear="no"`.
fn check_spine_all_nonlinear(
    itemrefs: &[crate::opf::SpineItemRef],
    report: &mut ValidationReport,
) {
    if itemrefs.is_empty() {
        return;
    }
    let has_linear = itemrefs
        .iter()
        .any(|r| !r.linear.eq_ignore_ascii_case("no"));
    if !has_linear {
        report.emit(
            "R7.4",
            format!(
                "Spine has {} itemref(s), all with linear=\"no\".",
                itemrefs.len()
            ),
        );
    }
}

/// R7.5: emit once per duplicated idref.
fn check_duplicate_itemrefs(
    itemrefs: &[crate::opf::SpineItemRef],
    report: &mut ValidationReport,
) {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut reported: HashSet<&str> = HashSet::new();
    for r in itemrefs {
        if r.idref.is_empty() {
            continue;
        }
        if !seen.insert(r.idref.as_str()) && reported.insert(r.idref.as_str()) {
            report.emit(
                "R7.5",
                format!("Duplicate itemref idref=\"{}\".", r.idref),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// R7.6 text/html, R7.7 deprecated media-types
// ---------------------------------------------------------------------------

/// True if the combination of media-type and file extension suggests the item
/// should have been declared as application/xhtml+xml.
fn is_text_html_for_xhtml(href: &str, media_type: &str) -> bool {
    if !media_type.eq_ignore_ascii_case("text/html") {
        return false;
    }
    let lower = href.to_ascii_lowercase();
    lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")
}

/// Return the recommended replacement if `media_type` matches a deprecated entry.
fn deprecated_replacement(media_type: &str) -> Option<&'static str> {
    let lower = media_type.to_ascii_lowercase();
    for (bad, good) in DEPRECATED_MEDIA_TYPES {
        if lower == *bad {
            return Some(good);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// R7.8 / R7.9 fallback chains
// ---------------------------------------------------------------------------

/// R7.8 / R7.9: fire when an item references a fallback id that does not exist.
fn check_fallback_targets(items: &[ManifestItem], report: &mut ValidationReport) {
    let ids: HashSet<&str> = items.iter().map(|i| i.id.as_str()).collect();
    for item in items {
        if let Some(ref fb) = item.fallback {
            if !ids.contains(fb.as_str()) {
                report.emit_at(
                    "R7.8",
                    format!(
                        "Item id=\"{}\" has fallback=\"{}\" but no manifest item with that id.",
                        item.id, fb
                    ),
                    Some(PathBuf::from(item.href.clone())),
                    None,
                );
            }
        }
        if let Some(ref fs_id) = item.fallback_style {
            if !ids.contains(fs_id.as_str()) {
                report.emit_at(
                    "R7.9",
                    format!(
                        "Item id=\"{}\" has fallback-style=\"{}\" but no manifest item with that id.",
                        item.id, fs_id
                    ),
                    Some(PathBuf::from(item.href.clone())),
                    None,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// R7.10 / R7.11 spine permissibility
// ---------------------------------------------------------------------------

/// R7.10 / R7.11: non-permissible spine media-types must either resolve via a
/// fallback chain to a core media-type (R7.11) or fail R7.10 if no fallback
/// exists at all.
fn check_spine_media_types(
    items: &[ManifestItem],
    itemrefs: &[crate::opf::SpineItemRef],
    report: &mut ValidationReport,
) {
    let by_id: HashMap<&str, &ManifestItem> =
        items.iter().map(|i| (i.id.as_str(), i)).collect();

    for r in itemrefs {
        if r.idref.is_empty() {
            continue;
        }
        let item = match by_id.get(r.idref.as_str()) {
            Some(i) => *i,
            None => continue,
        };
        if is_core_media_type(&item.media_type) {
            continue;
        }
        if item.fallback.is_none() {
            report.emit_at(
                "R7.10",
                format!(
                    "Spine item id=\"{}\" has non-permissible media-type \"{}\" and no fallback.",
                    item.id, item.media_type
                ),
                Some(PathBuf::from(item.href.clone())),
                None,
            );
            continue;
        }
        if !fallback_chain_reaches_core(item, &by_id) {
            report.emit_at(
                "R7.11",
                format!(
                    "Spine item id=\"{}\" fallback chain never reaches an xhtml/svg resource.",
                    item.id
                ),
                Some(PathBuf::from(item.href.clone())),
                None,
            );
        }
    }
}

/// True if `media_type` is one of the core content types allowed in the spine.
fn is_core_media_type(media_type: &str) -> bool {
    CORE_MEDIA_TYPES
        .iter()
        .any(|m| m.eq_ignore_ascii_case(media_type))
}

/// Walk the fallback chain starting at `item` and return true if any link
/// lands on a core media-type without cycling.
fn fallback_chain_reaches_core(
    item: &ManifestItem,
    by_id: &HashMap<&str, &ManifestItem>,
) -> bool {
    let mut visited: HashSet<&str> = HashSet::new();
    visited.insert(item.id.as_str());
    let mut current = item;
    while let Some(ref fb_id) = current.fallback {
        let next = match by_id.get(fb_id.as_str()) {
            Some(n) => *n,
            None => return false,
        };
        if !visited.insert(next.id.as_str()) {
            return false;
        }
        if is_core_media_type(&next.media_type) {
            return true;
        }
        current = next;
    }
    false
}

// ---------------------------------------------------------------------------
// R7.12 / R7.13 href checks
// ---------------------------------------------------------------------------

/// R7.12: fire once per href declared more than once in the manifest.
fn check_duplicate_hrefs(items: &[ManifestItem], report: &mut ValidationReport) {
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut reported: HashSet<String> = HashSet::new();
    for item in items {
        let key = strip_fragment(&item.href).to_string();
        if key.is_empty() {
            continue;
        }
        if let Some(first_id) = seen.get(&key) {
            if reported.insert(key.clone()) {
                report.emit_at(
                    "R7.12",
                    format!(
                        "Manifest href \"{}\" is declared twice (ids \"{}\" and \"{}\").",
                        key, first_id, item.id
                    ),
                    Some(PathBuf::from(key)),
                    None,
                );
            }
        } else {
            seen.insert(key, item.id.clone());
        }
    }
}

/// R7.13: any manifest href that resolves to the OPF file is a self-reference.
fn check_self_reference(
    items: &[ManifestItem],
    opf_path: &Path,
    base_dir: &Path,
    report: &mut ValidationReport,
) {
    let opf_canonical = normalize(opf_path);
    let opf_name = opf_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    for item in items {
        let href = strip_fragment(&item.href);
        if href.is_empty() {
            continue;
        }
        let candidate = normalize(&base_dir.join(href));
        let matches_by_path = candidate == opf_canonical;
        let matches_by_name = !opf_name.is_empty() && href.eq_ignore_ascii_case(&opf_name);
        if matches_by_path || matches_by_name {
            report.emit_at(
                "R7.13",
                format!(
                    "Item id=\"{}\" href=\"{}\" points at the OPF package file itself.",
                    item.id, item.href
                ),
                Some(PathBuf::from(item.href.clone())),
                None,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opf::{ManifestItem, SpineItemRef};

    fn mk_item(
        id: &str,
        href: &str,
        media_type: &str,
        fallback: Option<&str>,
        fallback_style: Option<&str>,
    ) -> ManifestItem {
        ManifestItem {
            id: id.to_string(),
            href: href.to_string(),
            media_type: media_type.to_string(),
            properties: String::new(),
            fallback: fallback.map(|s| s.to_string()),
            fallback_style: fallback_style.map(|s| s.to_string()),
        }
    }

    fn mk_ref(idref: &str, linear: &str) -> SpineItemRef {
        SpineItemRef {
            idref: idref.to_string(),
            linear: linear.to_string(),
            properties: String::new(),
        }
    }

    fn empty_report() -> ValidationReport {
        ValidationReport::new()
    }

    // ---- R7.2 / R7.3: magic bytes ----

    #[test]
    fn detect_kind_recognizes_jpeg_magic() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        assert_eq!(detect_kind(&bytes), DetectedKind::Jpeg);
    }

    #[test]
    fn detect_kind_recognizes_png_magic() {
        let bytes = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A];
        assert_eq!(detect_kind(&bytes), DetectedKind::Png);
    }

    #[test]
    fn detect_kind_recognizes_gif_magic() {
        let bytes = b"GIF89a..........";
        assert_eq!(detect_kind(bytes), DetectedKind::Gif);
    }

    #[test]
    fn detect_kind_recognizes_svg_tag() {
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(detect_kind(bytes), DetectedKind::Svg);
    }

    #[test]
    fn detect_kind_recognizes_xml_html_prolog() {
        let bytes = b"<?xml version=\"1.0\"?><html></html>";
        assert_eq!(detect_kind(bytes), DetectedKind::Xhtml);
    }

    #[test]
    fn detect_kind_recognizes_xml_svg_prolog() {
        let bytes = b"<?xml version=\"1.0\"?><svg xmlns=\"...\"/>";
        assert_eq!(detect_kind(bytes), DetectedKind::Svg);
    }

    #[test]
    fn detect_kind_unknown_bytes() {
        let bytes = b"random garbage no markers";
        assert_eq!(detect_kind(bytes), DetectedKind::Unknown);
    }

    #[test]
    fn check_media_type_magic_fires_r7_2_on_mismatch() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r72_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pretend.jpg");
        std::fs::write(&path, &[0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A]).unwrap();

        let item = mk_item("img", "pretend.jpg", "image/jpeg", None, None);
        let mut report = empty_report();
        check_media_type_magic(&dir, &[item], &mut report);

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule_id == Some("R7.2")),
            "expected R7.2 to fire"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_media_type_magic_fires_r7_3_on_unknown_declared() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r73_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weird.bin");
        std::fs::write(&path, &[0xFFu8, 0xD8, 0xFF, 0xE0, 0x00]).unwrap();

        let item = mk_item("img", "weird.bin", "application/x-secret", None, None);
        let mut report = empty_report();
        check_media_type_magic(&dir, &[item], &mut report);

        assert!(
            report
                .findings
                .iter()
                .any(|f| f.rule_id == Some("R7.3")),
            "expected R7.3 to fire"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_media_type_magic_clean_when_declared_matches_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r72ok_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("real.jpg");
        std::fs::write(&path, &[0xFFu8, 0xD8, 0xFF, 0xE0]).unwrap();
        let item = mk_item("img", "real.jpg", "image/jpeg", None, None);
        let mut report = empty_report();
        check_media_type_magic(&dir, &[item], &mut report);
        assert!(report
            .findings
            .iter()
            .all(|f| f.rule_id != Some("R7.2") && f.rule_id != Some("R7.3")));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- R7.4 / R7.5: spine scans ----

    #[test]
    fn r7_4_all_nonlinear_fires() {
        let refs = vec![mk_ref("a", "no"), mk_ref("b", "no")];
        let mut report = empty_report();
        check_spine_all_nonlinear(&refs, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.4")));
    }

    #[test]
    fn r7_4_mixed_clean() {
        let refs = vec![mk_ref("a", "no"), mk_ref("b", "yes")];
        let mut report = empty_report();
        check_spine_all_nonlinear(&refs, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.4")));
    }

    #[test]
    fn r7_4_default_linear_clean() {
        let refs = vec![mk_ref("a", ""), mk_ref("b", "")];
        let mut report = empty_report();
        check_spine_all_nonlinear(&refs, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.4")));
    }

    #[test]
    fn r7_5_duplicate_itemref_fires() {
        let refs = vec![mk_ref("x", ""), mk_ref("y", ""), mk_ref("x", "")];
        let mut report = empty_report();
        check_duplicate_itemrefs(&refs, &mut report);
        let hits: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule_id == Some("R7.5"))
            .collect();
        assert_eq!(hits.len(), 1, "should fire once per duplicate");
    }

    #[test]
    fn r7_5_unique_itemrefs_clean() {
        let refs = vec![mk_ref("x", ""), mk_ref("y", "")];
        let mut report = empty_report();
        check_duplicate_itemrefs(&refs, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.5")));
    }

    // ---- R7.6 / R7.7: media-type strings ----

    #[test]
    fn r7_6_text_html_on_xhtml_extension_fires() {
        assert!(is_text_html_for_xhtml("page.xhtml", "text/html"));
        assert!(is_text_html_for_xhtml("page.html", "text/html"));
    }

    #[test]
    fn r7_6_xhtml_declared_clean() {
        assert!(!is_text_html_for_xhtml("page.xhtml", "application/xhtml+xml"));
    }

    #[test]
    fn r7_7_deprecated_image_jpg() {
        assert_eq!(deprecated_replacement("image/jpg"), Some("image/jpeg"));
    }

    #[test]
    fn r7_7_deprecated_text_xml() {
        assert_eq!(deprecated_replacement("text/xml"), Some("application/xml"));
    }

    #[test]
    fn r7_7_deprecated_oeb1() {
        assert_eq!(
            deprecated_replacement("text/x-oeb1-document"),
            Some("application/xhtml+xml")
        );
    }

    #[test]
    fn r7_7_canonical_image_jpeg_clean() {
        assert!(deprecated_replacement("image/jpeg").is_none());
    }

    // ---- R7.8 / R7.9: fallback targets ----

    #[test]
    fn r7_8_dangling_fallback_fires() {
        let items = vec![
            mk_item("pdf", "a.pdf", "application/pdf", Some("missing"), None),
        ];
        let mut report = empty_report();
        check_fallback_targets(&items, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.8")));
    }

    #[test]
    fn r7_9_dangling_fallback_style_fires() {
        let items = vec![
            mk_item("pdf", "a.pdf", "application/pdf", None, Some("gone")),
        ];
        let mut report = empty_report();
        check_fallback_targets(&items, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.9")));
    }

    #[test]
    fn r7_8_valid_fallback_clean() {
        let items = vec![
            mk_item("pdf", "a.pdf", "application/pdf", Some("html"), None),
            mk_item("html", "a.xhtml", "application/xhtml+xml", None, None),
        ];
        let mut report = empty_report();
        check_fallback_targets(&items, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.8")));
    }

    // ---- R7.10 / R7.11: spine fallback chains ----

    #[test]
    fn r7_10_non_permissible_no_fallback_fires() {
        let items = vec![mk_item(
            "pdf",
            "a.pdf",
            "application/pdf",
            None,
            None,
        )];
        let refs = vec![mk_ref("pdf", "")];
        let mut report = empty_report();
        check_spine_media_types(&items, &refs, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.10")));
    }

    #[test]
    fn r7_11_fallback_chain_never_reaches_core_fires() {
        let items = vec![
            mk_item("pdf", "a.pdf", "application/pdf", Some("zip"), None),
            mk_item("zip", "b.zip", "application/zip", None, None),
        ];
        let refs = vec![mk_ref("pdf", "")];
        let mut report = empty_report();
        check_spine_media_types(&items, &refs, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.11")));
    }

    #[test]
    fn r7_11_fallback_chain_reaches_xhtml_clean() {
        let items = vec![
            mk_item("pdf", "a.pdf", "application/pdf", Some("html"), None),
            mk_item("html", "a.xhtml", "application/xhtml+xml", None, None),
        ];
        let refs = vec![mk_ref("pdf", "")];
        let mut report = empty_report();
        check_spine_media_types(&items, &refs, &mut report);
        assert!(report.findings.iter().all(|f| {
            f.rule_id != Some("R7.10") && f.rule_id != Some("R7.11")
        }));
    }

    #[test]
    fn r7_10_xhtml_spine_item_clean() {
        let items = vec![mk_item(
            "html",
            "a.xhtml",
            "application/xhtml+xml",
            None,
            None,
        )];
        let refs = vec![mk_ref("html", "")];
        let mut report = empty_report();
        check_spine_media_types(&items, &refs, &mut report);
        assert!(report.findings.iter().all(|f| {
            f.rule_id != Some("R7.10") && f.rule_id != Some("R7.11")
        }));
    }

    #[test]
    fn r7_11_cyclic_fallback_flagged() {
        let items = vec![
            mk_item("a", "a.pdf", "application/pdf", Some("b"), None),
            mk_item("b", "b.pdf", "application/pdf", Some("a"), None),
        ];
        let refs = vec![mk_ref("a", "")];
        let mut report = empty_report();
        check_spine_media_types(&items, &refs, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.11")));
    }

    // ---- R7.12: duplicate href ----

    #[test]
    fn r7_12_duplicate_href_fires() {
        let items = vec![
            mk_item("a", "dup.xhtml", "application/xhtml+xml", None, None),
            mk_item("b", "dup.xhtml", "application/xhtml+xml", None, None),
        ];
        let mut report = empty_report();
        check_duplicate_hrefs(&items, &mut report);
        let hits: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule_id == Some("R7.12"))
            .collect();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn r7_12_fragments_collapse_to_same_file() {
        let items = vec![
            mk_item("a", "doc.xhtml", "application/xhtml+xml", None, None),
            mk_item("b", "doc.xhtml#frag", "application/xhtml+xml", None, None),
        ];
        let mut report = empty_report();
        check_duplicate_hrefs(&items, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.12")));
    }

    #[test]
    fn r7_12_unique_hrefs_clean() {
        let items = vec![
            mk_item("a", "a.xhtml", "application/xhtml+xml", None, None),
            mk_item("b", "b.xhtml", "application/xhtml+xml", None, None),
        ];
        let mut report = empty_report();
        check_duplicate_hrefs(&items, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.12")));
    }

    // ---- R7.13: self-reference ----

    #[test]
    fn r7_13_href_equals_opf_filename_fires() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r713_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let opf = dir.join("book.opf");
        std::fs::write(&opf, b"<package/>").unwrap();

        let items = vec![mk_item(
            "self",
            "book.opf",
            "application/oebps-package+xml",
            None,
            None,
        )];
        let mut report = empty_report();
        check_self_reference(&items, &opf, &dir, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.13")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn r7_13_ordinary_href_clean() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r713ok_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let opf = dir.join("book.opf");
        std::fs::write(&opf, b"<package/>").unwrap();
        std::fs::write(dir.join("chapter.xhtml"), b"<html/>").unwrap();

        let items = vec![mk_item(
            "ch",
            "chapter.xhtml",
            "application/xhtml+xml",
            None,
            None,
        )];
        let mut report = empty_report();
        check_self_reference(&items, &opf, &dir, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.13")));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- R7.1: undeclared files ----

    #[test]
    fn r7_1_undeclared_file_fires() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r71_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let opf = dir.join("book.opf");
        std::fs::write(&opf, b"<package/>").unwrap();
        std::fs::write(dir.join("declared.xhtml"), b"<html/>").unwrap();
        std::fs::write(dir.join("stray.xhtml"), b"<html/>").unwrap();

        let items = vec![mk_item(
            "d",
            "declared.xhtml",
            "application/xhtml+xml",
            None,
            None,
        )];
        let mut report = empty_report();
        check_undeclared_files(&dir, &opf, &items, &mut report);
        assert!(report.findings.iter().any(|f| f.rule_id == Some("R7.1")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn r7_1_build_artifact_clean() {
        let dir = std::env::temp_dir().join(format!(
            "kindling_ms_r71ok_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let opf = dir.join("book.opf");
        std::fs::write(&opf, b"<package/>").unwrap();
        std::fs::write(dir.join("declared.xhtml"), b"<html/>").unwrap();
        std::fs::write(dir.join("out.mobi"), b"\0\0\0\0").unwrap();
        std::fs::write(dir.join("out.epub"), b"PK").unwrap();

        let items = vec![mk_item(
            "d",
            "declared.xhtml",
            "application/xhtml+xml",
            None,
            None,
        )];
        let mut report = empty_report();
        check_undeclared_files(&dir, &opf, &items, &mut report);
        assert!(report.findings.iter().all(|f| f.rule_id != Some("R7.1")));
        std::fs::remove_dir_all(&dir).ok();
    }
}
