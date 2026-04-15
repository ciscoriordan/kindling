// Section 10.4 image integrity rules (R10.4.3, R10.4.4, R10.4.5).
//
// These rules extend the existing image checks in `images.rs`. They look at
// the actual bytes of every manifest image and catch three failure modes
// that kindlegen and Send-to-Kindle routinely trip over:
//
//   R10.4.3  MED_004: magic bytes match a known format but the file is
//            truncated or its header is corrupt (JPEG without EOI, PNG
//            without IHDR, GIF without the terminating `3b`).
//   R10.4.4  PKG_021: generic "cannot parse this image" - bytes are too
//            short to sniff, or no known magic matches at all.
//   R10.4.5  PKG_022: file extension disagrees with the format detected
//            from the magic bytes (e.g. a PNG saved as foo.jpg).
//
// Only plain byte comparisons are used here: the `image` crate is
// deliberately avoided so the checks are fast, deterministic, and do not
// fail on unusual but legal encoder output.

use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

/// Image format detected from magic bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageFormat {
    Jpeg,
    Png,
    Gif,
    Svg,
    Webp,
}

impl ImageFormat {
    /// Short lowercase name used in diagnostic messages.
    fn as_str(&self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "JPEG",
            ImageFormat::Png => "PNG",
            ImageFormat::Gif => "GIF",
            ImageFormat::Svg => "SVG",
            ImageFormat::Webp => "WebP",
        }
    }

    /// Whether `ext` (lowercased, no leading dot) is one of the file
    /// extensions conventionally used for this format.
    fn matches_extension(&self, ext: &str) -> bool {
        match self {
            ImageFormat::Jpeg => ext == "jpg" || ext == "jpeg" || ext == "jpe",
            ImageFormat::Png => ext == "png",
            ImageFormat::Gif => ext == "gif",
            ImageFormat::Svg => ext == "svg",
            ImageFormat::Webp => ext == "webp",
        }
    }
}

pub struct ImageIntegrityChecks;

impl Check for ImageIntegrityChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R10.4.3", "R10.4.4", "R10.4.5"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        let mut items: Vec<(String, String, String)> = opf
            .manifest
            .iter()
            .filter(|(_, (_, mt))| mt.starts_with("image/"))
            .map(|(id, (href, mt))| (id.clone(), href.clone(), mt.clone()))
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        for (_id, href, _media_type) in &items {
            let path = opf.base_dir.join(href);
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            inspect_image_bytes(href, &bytes, report);
        }
    }
}

/// Run the three integrity checks against one image file's bytes.
fn inspect_image_bytes(href: &str, bytes: &[u8], report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    let format = match detect_format(bytes) {
        Some(f) => f,
        None => {
            report.emit_at(
                "R10.4.4",
                format!("No known image magic bytes detected in {}.", href),
                file,
                None,
            );
            return;
        }
    };

    if let Some(reason) = sanity_check(format, bytes) {
        report.emit_at(
            "R10.4.3",
            format!("{} header/trailer is corrupt: {}.", format.as_str(), reason),
            file.clone(),
            None,
        );
    }

    if let Some(ext) = extension_from_href(href) {
        if !format.matches_extension(&ext) {
            report.emit_at(
                "R10.4.5",
                format!(
                    "{} has extension .{} but bytes are {}.",
                    href,
                    ext,
                    format.as_str()
                ),
                file,
                None,
            );
        }
    }
}

/// Identify a file's image format from its leading bytes, or return `None`
/// if no known signature matches.
fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() < 4 {
        return None;
    }

    // JPEG: FF D8 FF
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }

    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.len() >= 8
        && bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
    {
        return Some(ImageFormat::Png);
    }

    // GIF: 47 49 46 38 (37|39) 61 = "GIF87a" or "GIF89a"
    if bytes.len() >= 6
        && bytes.starts_with(&[0x47, 0x49, 0x46, 0x38])
        && (bytes[4] == 0x37 || bytes[4] == 0x39)
        && bytes[5] == 0x61
    {
        return Some(ImageFormat::Gif);
    }

    // WEBP: RIFF xxxx WEBP
    if bytes.len() >= 12
        && bytes.starts_with(&[0x52, 0x49, 0x46, 0x46])
        && &bytes[8..12] == b"WEBP"
    {
        return Some(ImageFormat::Webp);
    }

    // SVG: textual, either an XML prolog or the <svg root. Strip an optional
    // UTF-8 BOM first so SVGs written with a BOM still get detected.
    let text = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    let head_len = text.len().min(256);
    if let Ok(head) = std::str::from_utf8(&text[..head_len]) {
        let trimmed = head.trim_start();
        if trimmed.starts_with("<?xml") || trimmed.starts_with("<svg") {
            return Some(ImageFormat::Svg);
        }
    }

    None
}

/// Cheap per-format sanity check. Returns a short reason if the file is
/// truncated or its header/trailer is missing, or `None` if it looks fine.
fn sanity_check(format: ImageFormat, bytes: &[u8]) -> Option<&'static str> {
    match format {
        ImageFormat::Jpeg => {
            if bytes.len() < 4 {
                return Some("file is too short");
            }
            let tail = &bytes[bytes.len() - 2..];
            if tail != [0xFF, 0xD9] {
                return Some("missing FF D9 EOI marker");
            }
            None
        }
        ImageFormat::Png => {
            if bytes.len() < 16 {
                return Some("file is too short");
            }
            // IHDR chunk must appear within the first 50 bytes of a PNG.
            let scan_end = bytes.len().min(50);
            if !has_subsequence(&bytes[..scan_end], b"IHDR") {
                return Some("missing IHDR chunk");
            }
            None
        }
        ImageFormat::Gif => {
            if bytes.len() < 8 {
                return Some("file is too short");
            }
            if *bytes.last().unwrap() != 0x3B {
                return Some("missing 3B trailer byte");
            }
            None
        }
        ImageFormat::Svg | ImageFormat::Webp => {
            // SVG and WebP round out the magic-byte sniffer but do not get
            // a cheap per-format trailer check in this cluster.
            None
        }
    }
}

/// Byte-level `needle in haystack` for short needles.
fn has_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Extract the lowercased file extension from an href. Fragments and
/// query strings are stripped first.
fn extension_from_href(href: &str) -> Option<String> {
    let path = href.split(['#', '?']).next().unwrap_or(href);
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    let dot = last_segment.rfind('.')?;
    if dot + 1 >= last_segment.len() {
        return None;
    }
    Some(last_segment[dot + 1..].to_ascii_lowercase())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal but structurally-valid PNG: signature, IHDR, IDAT, IEND.
    fn minimal_png() -> Vec<u8> {
        let mut v = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        // IHDR chunk: length 13, type "IHDR", payload, crc32 placeholder.
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x0D]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 0, 0, 0, 0]);
        v.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        v
    }

    fn minimal_jpeg() -> Vec<u8> {
        vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0xFF, 0xD9]
    }

    fn minimal_gif() -> Vec<u8> {
        let mut v = vec![0x47, 0x49, 0x46, 0x38, 0x39, 0x61];
        v.extend_from_slice(&[0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x3B]);
        v
    }

    // ---- detect_format: positive cases ----

    #[test]
    fn detect_format_recognizes_jpeg() {
        assert_eq!(detect_format(&minimal_jpeg()), Some(ImageFormat::Jpeg));
    }

    #[test]
    fn detect_format_recognizes_png() {
        assert_eq!(detect_format(&minimal_png()), Some(ImageFormat::Png));
    }

    #[test]
    fn detect_format_recognizes_gif89a() {
        assert_eq!(detect_format(&minimal_gif()), Some(ImageFormat::Gif));
    }

    #[test]
    fn detect_format_recognizes_gif87a() {
        let bytes = b"GIF87a\x00\x00\x00\x00\x3B";
        assert_eq!(detect_format(bytes), Some(ImageFormat::Gif));
    }

    #[test]
    fn detect_format_recognizes_svg_with_xml_prolog() {
        let bytes = b"<?xml version=\"1.0\"?><svg xmlns=\"...\"/>";
        assert_eq!(detect_format(bytes), Some(ImageFormat::Svg));
    }

    #[test]
    fn detect_format_recognizes_svg_without_prolog() {
        let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
        assert_eq!(detect_format(bytes), Some(ImageFormat::Svg));
    }

    #[test]
    fn detect_format_recognizes_webp() {
        let mut bytes = vec![0x52, 0x49, 0x46, 0x46, 0x24, 0, 0, 0];
        bytes.extend_from_slice(b"WEBPVP8 ");
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Webp));
    }

    // ---- detect_format: negative / edge cases ----

    #[test]
    fn detect_format_rejects_too_short() {
        assert_eq!(detect_format(&[0xFF]), None);
        assert_eq!(detect_format(&[]), None);
    }

    #[test]
    fn detect_format_rejects_garbage() {
        let bytes = b"this is a plain text file, not an image";
        assert_eq!(detect_format(bytes), None);
    }

    #[test]
    fn detect_format_rejects_gif_with_wrong_version() {
        // 'GIF88a' is not a valid signature.
        let bytes = b"GIF88a\x00\x00\x00\x00\x3B";
        assert_eq!(detect_format(bytes), None);
    }

    // ---- sanity_check: JPEG ----

    #[test]
    fn sanity_check_jpeg_clean() {
        assert!(sanity_check(ImageFormat::Jpeg, &minimal_jpeg()).is_none());
    }

    #[test]
    fn sanity_check_jpeg_missing_eoi_fires() {
        let mut bytes = minimal_jpeg();
        // Drop the trailing FF D9 so the EOI marker is gone.
        bytes.truncate(bytes.len() - 2);
        bytes.push(0x00);
        bytes.push(0x00);
        assert!(sanity_check(ImageFormat::Jpeg, &bytes).is_some());
    }

    // ---- sanity_check: PNG ----

    #[test]
    fn sanity_check_png_clean() {
        assert!(sanity_check(ImageFormat::Png, &minimal_png()).is_none());
    }

    #[test]
    fn sanity_check_png_missing_ihdr_fires() {
        // A PNG signature followed by nonsense - no IHDR in the first 50 bytes.
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend_from_slice(&[0u8; 20]);
        assert!(sanity_check(ImageFormat::Png, &bytes).is_some());
    }

    // ---- sanity_check: GIF ----

    #[test]
    fn sanity_check_gif_clean() {
        assert!(sanity_check(ImageFormat::Gif, &minimal_gif()).is_none());
    }

    #[test]
    fn sanity_check_gif_missing_trailer_fires() {
        let mut bytes = minimal_gif();
        let last = bytes.len() - 1;
        bytes[last] = 0x00;
        assert!(sanity_check(ImageFormat::Gif, &bytes).is_some());
    }

    // ---- extension_from_href ----

    #[test]
    fn extension_from_href_handles_plain_path() {
        assert_eq!(extension_from_href("images/foo.jpg").as_deref(), Some("jpg"));
    }

    #[test]
    fn extension_from_href_handles_uppercase() {
        assert_eq!(extension_from_href("IMG.JPEG").as_deref(), Some("jpeg"));
    }

    #[test]
    fn extension_from_href_handles_fragment() {
        assert_eq!(
            extension_from_href("a/b.png#frag").as_deref(),
            Some("png")
        );
    }

    #[test]
    fn extension_from_href_no_extension() {
        assert_eq!(extension_from_href("images/plain"), None);
    }

    // ---- ImageFormat::matches_extension ----

    #[test]
    fn jpeg_matches_jpg_and_jpeg() {
        assert!(ImageFormat::Jpeg.matches_extension("jpg"));
        assert!(ImageFormat::Jpeg.matches_extension("jpeg"));
        assert!(!ImageFormat::Jpeg.matches_extension("png"));
    }

    #[test]
    fn png_only_matches_png() {
        assert!(ImageFormat::Png.matches_extension("png"));
        assert!(!ImageFormat::Png.matches_extension("jpg"));
    }

    #[test]
    fn gif_only_matches_gif() {
        assert!(ImageFormat::Gif.matches_extension("gif"));
        assert!(!ImageFormat::Gif.matches_extension("png"));
    }
}
