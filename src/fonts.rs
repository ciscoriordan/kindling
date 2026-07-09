/// EPUB font embedding for KF8 books (issue #18).
///
/// Collects font files from the OPF manifest, undoes IDPF/Adobe font
/// obfuscation when the EPUB declares it in `META-INF/encryption.xml`,
/// and wraps each font in the KF8 FONT resource container (zlib-deflated,
/// unobfuscated). The MOBI writer appends these records after the image
/// records, so a font's `kindle:embed` index is
/// `image_count + font_position + 1`; the CSS flow's `@font-face src:
/// url(...)` values are rewritten to those `kindle:embed` URLs.
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::opf::OPFData;

/// One embeddable font collected from the manifest.
pub struct EmbeddedFont {
    /// Manifest-style href, percent-decoded, dot segments resolved,
    /// relative to the OPF base dir. Keys the CSS `url()` rewrite.
    pub href: String,
    /// Media type used in the `kindle:embed` URL query.
    pub media_type: String,
    /// The complete FONT container record.
    pub record: Vec<u8>,
}

/// Font media types accepted for embedding. Kindle supports OpenType and
/// TrueType; WOFF/WOFF2 are not supported by the renderer and are skipped
/// with a warning.
fn is_font_media_type(media_type: &str) -> bool {
    matches!(
        media_type,
        "application/vnd.ms-opentype"
            | "application/x-font-ttf"
            | "application/x-font-otf"
            | "application/x-font-truetype"
            | "application/x-font-opentype"
            | "application/font-sfnt"
            | "font/otf"
            | "font/ttf"
            | "font/sfnt"
    )
}

fn is_woff(media_type: &str, href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    matches!(
        media_type,
        "application/font-woff" | "font/woff" | "font/woff2"
    ) || lower.ends_with(".woff")
        || lower.ends_with(".woff2")
}

fn has_font_extension(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    lower.ends_with(".ttf") || lower.ends_with(".otf") || lower.ends_with(".ttc")
}

/// Media type for the `kindle:embed` URL: normalize by sniffing the font
/// data so generic manifest types (application/octet-stream) still get a
/// type the renderer recognizes.
fn embed_media_type(data: &[u8]) -> &'static str {
    if data.len() >= 4 && &data[0..4] == b"OTTO" {
        "application/vnd.ms-opentype"
    } else {
        "application/x-font-ttf"
    }
}

/// True when the data starts like an OpenType/TrueType font.
fn looks_like_font(data: &[u8]) -> bool {
    data.len() >= 4
        && matches!(
            &data[0..4],
            b"OTTO" | b"true" | b"ttcf" | [0x00, 0x01, 0x00, 0x00]
        )
}

/// Collect the embeddable fonts declared in the OPF manifest, in
/// deterministic (manifest id) order. Obfuscated fonts are decoded using
/// the package identifiers as candidate keys; fonts that still do not
/// look like a font afterwards are skipped with a warning rather than
/// shipped as garbage bytes.
pub fn collect_fonts(opf: &OPFData) -> Vec<EmbeddedFont> {
    let mut items: Vec<(String, String, String)> = opf
        .manifest_items
        .iter()
        .filter(|item| {
            is_font_media_type(&item.media_type)
                || is_woff(&item.media_type, &item.href)
                || has_font_extension(&item.href)
        })
        .map(|item| (item.id.clone(), item.href.clone(), item.media_type.clone()))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));

    let obfuscated = parse_encryption_xml(&opf.base_dir);

    let mut fonts = Vec::new();
    for (_, href, media_type) in items {
        if is_woff(&media_type, &href) {
            eprintln!(
                "Warning: skipping font {} - Kindle does not support WOFF; \
                 convert it to TTF or OTF to embed it",
                href
            );
            continue;
        }
        let decoded_href = percent_decode_str(&href);
        let path = opf.base_dir.join(&decoded_href);
        let data = std::fs::read(&path).or_else(|_| std::fs::read(opf.base_dir.join(&href)));
        let Ok(mut data) = data else {
            eprintln!("Warning: font {} not found on disk - skipping", href);
            continue;
        };

        if let Some(algorithm) = font_obfuscation(&obfuscated, &path, &opf.base_dir, &decoded_href)
        {
            match deobfuscate(&data, algorithm, &opf.dc_identifiers) {
                Some(clear) => data = clear,
                None => {
                    eprintln!(
                        "Warning: could not deobfuscate font {} ({}) - skipping",
                        href,
                        algorithm.name()
                    );
                    continue;
                }
            }
        } else if !looks_like_font(&data) {
            eprintln!(
                "Warning: {} does not look like a TTF/OTF font - skipping",
                href
            );
            continue;
        }

        let media_type = embed_media_type(&data).to_string();
        fonts.push(EmbeddedFont {
            href: normalize_path(&decoded_href),
            media_type,
            record: build_font_record(&data),
        });
    }
    fonts
}

/// Wrap raw font bytes in the KF8 FONT resource container:
/// magic "FONT", uncompressed size, flags (bit 0 = zlib), data offset,
/// XOR key length, XOR key offset, then the deflated font data. This
/// matches kindlegen's own output byte-for-byte in the header (flags=0x1,
/// data at 24, no XOR key): kindlegen compresses but does not obfuscate,
/// so kindling doesn't either.
fn build_font_record(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    encoder.write_all(data).unwrap();
    let compressed = encoder.finish().unwrap();

    let mut record = Vec::with_capacity(24 + compressed.len());
    record.extend_from_slice(b"FONT");
    record.extend_from_slice(&(data.len() as u32).to_be_bytes());
    record.extend_from_slice(&1u32.to_be_bytes()); // flags: zlib, not obfuscated
    record.extend_from_slice(&24u32.to_be_bytes()); // data offset
    record.extend_from_slice(&0u32.to_be_bytes()); // XOR key length
    record.extend_from_slice(&0u32.to_be_bytes()); // XOR key offset (none, like kindlegen)
    record.extend_from_slice(&compressed);
    record
}

/// Rewrite `@font-face` `src: url(...)` references in one stylesheet to
/// `kindle:embed` URLs. `css_dir` is the stylesheet's directory relative
/// to the OPF base dir ("" when it sits next to the OPF); `font_embeds`
/// maps normalized font hrefs to their 1-based `kindle:embed` index and
/// media type. URLs that do not resolve to an embedded font are left
/// untouched.
pub fn rewrite_css_font_urls(
    css: &str,
    css_dir: &str,
    font_embeds: &std::collections::HashMap<String, (usize, String)>,
) -> String {
    if font_embeds.is_empty() {
        return css.to_string();
    }
    let url_re = regex::Regex::new(r"(?i)url\(\s*([^)]+?)\s*\)").unwrap();
    url_re
        .replace_all(css, |caps: &regex::Captures| {
            let raw = caps.get(1).unwrap().as_str().trim();
            let target = raw
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| raw.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
                .unwrap_or(raw)
                .trim();
            let file = target.split('#').next().unwrap_or(target);
            let file = file.split('?').next().unwrap_or(file);
            let decoded = percent_decode_str(file);
            let joined = if css_dir.is_empty() {
                decoded
            } else {
                format!("{}/{}", css_dir, decoded)
            };
            match font_embeds.get(&normalize_path(&joined)) {
                Some((recindex, media_type)) => format!(
                    "url(kindle:embed:{}?mime={})",
                    encode_kindle_embed_base32(*recindex),
                    media_type
                ),
                None => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .to_string()
}

/// Strip the CSS that locks Kindle's user font switching (issue #19).
///
/// The Kindle renderer treats any `font-family` the book CSS applies to
/// its text as author intent and keeps it over the reader's Aa menu font
/// choice — so a book whose stylesheet names fonts that are not even
/// embedded (common in Chinese EPUBs: `font-family: "SimSun", "宋体",
/// sans-serif`) redraws on a font switch but never changes face. When a
/// book embeds no usable fonts there is nothing for those declarations to
/// select, so dropping them costs nothing and returns font control to the
/// reader (KOReader does the equivalent at render time).
///
/// Two transforms:
///  1. `@font-face` rules are removed entirely — with no embedded font
///     records their `src` URLs resolve to nothing on device.
///  2. `font-family` declarations are removed, except that a declaration
///     whose family list includes the generic `monospace` is rewritten to
///     plain `font-family: monospace` so code blocks keep their fixed
///     pitch (the generic resolves on device and carries real meaning;
///     named families do not).
pub fn strip_font_locking_css(css: &str) -> String {
    let font_face_re = regex::Regex::new(r"(?is)@font-face\s*\{[^}]*\}").unwrap();
    let css = font_face_re.replace_all(css, "");
    let family_re = regex::Regex::new(r"(?is)font-family\s*:\s*([^;}]*)(;?)").unwrap();
    family_re
        .replace_all(&css, |caps: &regex::Captures| {
            let families = caps.get(1).unwrap().as_str();
            let keeps_monospace = families.split(',').any(|f| {
                f.trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .trim()
                    .eq_ignore_ascii_case("monospace")
            });
            if keeps_monospace {
                format!("font-family: monospace{}", caps.get(2).unwrap().as_str())
            } else {
                String::new()
            }
        })
        .to_string()
}

/// Apply [`strip_font_locking_css`] to the contents of every inline
/// `<style>` block in an HTML document, leaving everything else untouched.
pub fn strip_font_locking_style_blocks(html: &str) -> String {
    let style_re = regex::Regex::new(r"(?is)(<style\b[^>]*>)(.*?)(</style>)").unwrap();
    style_re
        .replace_all(html, |caps: &regex::Captures| {
            format!(
                "{}{}{}",
                caps.get(1).unwrap().as_str(),
                strip_font_locking_css(caps.get(2).unwrap().as_str()),
                caps.get(3).unwrap().as_str()
            )
        })
        .to_string()
}

/// 4-char base32 (0-9, A-V) encoding used by `kindle:embed` indices,
/// mirroring the cover URI encoding in `mobi.rs`.
fn encode_kindle_embed_base32(mut value: usize) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = [b'0'; 4];
    for slot in out.iter_mut().rev() {
        *slot = DIGITS[value % 32];
        value /= 32;
    }
    String::from_utf8_lossy(&out).to_string()
}

// -- EPUB font obfuscation ---------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObfuscationAlgorithm {
    /// http://www.idpf.org/2008/embedding - SHA-1 of the packed unique
    /// identifier, XOR over the first 1040 bytes.
    Idpf,
    /// http://ns.adobe.com/pdf/enc#RC - 16 bytes parsed from the urn:uuid
    /// identifier, XOR over the first 1024 bytes.
    Adobe,
}

impl ObfuscationAlgorithm {
    fn name(&self) -> &'static str {
        match self {
            ObfuscationAlgorithm::Idpf => "IDPF obfuscation",
            ObfuscationAlgorithm::Adobe => "Adobe obfuscation",
        }
    }
}

/// Find `META-INF/encryption.xml` above the OPF dir and return the
/// obfuscated resource URIs (container-root-relative, percent-decoded)
/// with their algorithms, resolved to absolute paths.
fn parse_encryption_xml(base_dir: &Path) -> Vec<(PathBuf, ObfuscationAlgorithm)> {
    let mut dir = base_dir.to_path_buf();
    for _ in 0..4 {
        let candidate = dir.join("META-INF").join("encryption.xml");
        if candidate.exists() {
            let Ok(content) = std::fs::read_to_string(&candidate) else {
                return Vec::new();
            };
            return parse_encryption_entries(&content)
                .into_iter()
                .map(|(uri, algorithm)| (dir.join(percent_decode_str(&uri)), algorithm))
                .collect();
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    Vec::new()
}

/// Pull (CipherReference URI, algorithm) pairs out of encryption.xml.
fn parse_encryption_entries(content: &str) -> Vec<(String, ObfuscationAlgorithm)> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(content);
    reader.config_mut().check_end_names = false;
    let mut entries = Vec::new();
    let mut current: Option<ObfuscationAlgorithm> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.name();
                let local = name.as_ref().rsplit(|&b| b == b':').next().unwrap_or(b"");
                match local {
                    b"EncryptionMethod" => {
                        let algorithm = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"Algorithm")
                            .map(|a| String::from_utf8_lossy(&a.value).to_string());
                        current = match algorithm.as_deref() {
                            Some("http://www.idpf.org/2008/embedding") => {
                                Some(ObfuscationAlgorithm::Idpf)
                            }
                            Some("http://ns.adobe.com/pdf/enc#RC") => {
                                Some(ObfuscationAlgorithm::Adobe)
                            }
                            _ => None,
                        };
                    }
                    b"CipherReference" => {
                        if let Some(algorithm) = current {
                            if let Some(uri) = e
                                .attributes()
                                .flatten()
                                .find(|a| a.key.as_ref() == b"URI")
                                .map(|a| String::from_utf8_lossy(&a.value).to_string())
                            {
                                entries.push((uri, algorithm));
                            }
                        }
                    }
                    b"EncryptedData" => current = None,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    entries
}

/// Return the obfuscation algorithm covering `font_path`, if any.
fn font_obfuscation(
    obfuscated: &[(PathBuf, ObfuscationAlgorithm)],
    font_path: &Path,
    base_dir: &Path,
    decoded_href: &str,
) -> Option<ObfuscationAlgorithm> {
    if obfuscated.is_empty() {
        return None;
    }
    let font_canonical = font_path
        .canonicalize()
        .unwrap_or_else(|_| base_dir.join(decoded_href));
    for (path, algorithm) in obfuscated {
        let entry_canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if entry_canonical == font_canonical {
            return Some(*algorithm);
        }
    }
    None
}

/// Undo IDPF/Adobe font obfuscation, trying every package identifier as a
/// key candidate and validating the result by its font magic. Returns
/// `None` when no candidate produces something that looks like a font.
fn deobfuscate(
    data: &[u8],
    algorithm: ObfuscationAlgorithm,
    identifiers: &[String],
) -> Option<Vec<u8>> {
    for identifier in identifiers {
        let (key, span): (Vec<u8>, usize) = match algorithm {
            ObfuscationAlgorithm::Idpf => {
                // The key is SHA-1 of the identifier with all whitespace
                // removed (IDPF Font Obfuscation spec).
                let packed: String = identifier.chars().filter(|c| !c.is_whitespace()).collect();
                (sha1(packed.as_bytes()).to_vec(), 1040)
            }
            ObfuscationAlgorithm::Adobe => {
                // The key is the 16-byte UUID parsed from the identifier.
                let hex: String = identifier
                    .trim()
                    .trim_start_matches("urn:uuid:")
                    .chars()
                    .filter(|c| c.is_ascii_hexdigit())
                    .collect();
                if hex.len() != 32 {
                    continue;
                }
                let bytes: Vec<u8> = (0..16)
                    .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
                    .collect();
                (bytes, 1024)
            }
        };
        let mut clear = data.to_vec();
        for (i, byte) in clear.iter_mut().take(span).enumerate() {
            *byte ^= key[i % key.len()];
        }
        if looks_like_font(&clear) {
            return Some(clear);
        }
    }
    None
}

/// Minimal SHA-1 (RFC 3174) for the IDPF font obfuscation key. Fonts are
/// small and few, so performance is irrelevant; kept local to avoid a
/// crypto dependency (mirrors the minimal MD5 in `exth.rs`).
fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut message = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(word.try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Resolve `.` and `..` segments in a slash-separated path (same rules as
/// `nav::normalize_path`; kept local to avoid a cross-module dependency).
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

/// Minimal percent-decoder (UTF-8 aware).
fn percent_decode_str(s: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16) {
                    out.push(byte);
                    continue;
                }
                out.push(b'%');
                out.push(h1);
                out.push(h2);
                continue;
            }
            out.push(b'%');
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod font_tests {
    use super::*;

    /// Minimal fake TTF: correct magic, arbitrary body.
    fn fake_ttf() -> Vec<u8> {
        let mut data = vec![0x00, 0x01, 0x00, 0x00];
        data.extend((0..2000u32).flat_map(|i| i.to_be_bytes()));
        data
    }

    #[test]
    fn sha1_matches_known_vectors() {
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
        assert_eq!(
            sha1(b""),
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
                0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );
        println!("  \u{2713} SHA-1 matches RFC 3174 test vectors");
    }

    #[test]
    fn font_record_container_roundtrip() {
        let data = fake_ttf();
        let record = build_font_record(&data);
        assert_eq!(&record[0..4], b"FONT");
        assert_eq!(
            u32::from_be_bytes(record[4..8].try_into().unwrap()) as usize,
            data.len()
        );
        assert_eq!(u32::from_be_bytes(record[8..12].try_into().unwrap()), 1); // zlib
        let data_offset = u32::from_be_bytes(record[12..16].try_into().unwrap()) as usize;
        assert_eq!(data_offset, 24);
        assert_eq!(u32::from_be_bytes(record[16..20].try_into().unwrap()), 0); // no XOR key
        // Round-trip the payload through zlib.
        let mut decoder = flate2::read::ZlibDecoder::new(&record[data_offset..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out).unwrap();
        assert_eq!(out, data);
        println!("  \u{2713} FONT container wraps and round-trips zlib data");
    }

    #[test]
    fn idpf_deobfuscation_recovers_font() {
        let identifier = "urn:uuid:12345678-1234-1234-1234-123456789abc";
        let clear = fake_ttf();
        // Obfuscate per the IDPF spec, then verify deobfuscate() undoes it.
        let packed: String = identifier.chars().filter(|c| !c.is_whitespace()).collect();
        let key = sha1(packed.as_bytes());
        let mut obfuscated = clear.clone();
        for (i, byte) in obfuscated.iter_mut().take(1040).enumerate() {
            *byte ^= key[i % key.len()];
        }
        assert!(!looks_like_font(&obfuscated));
        let recovered = deobfuscate(
            &obfuscated,
            ObfuscationAlgorithm::Idpf,
            &["wrong-id".to_string(), identifier.to_string()],
        )
        .expect("deobfuscation should succeed with the right identifier");
        assert_eq!(recovered, clear);
        println!("  \u{2713} IDPF obfuscation round-trips");
    }

    #[test]
    fn adobe_deobfuscation_recovers_font() {
        let identifier = "urn:uuid:12345678-1234-1234-1234-123456789abc";
        let clear = fake_ttf();
        let hex: String = identifier
            .trim_start_matches("urn:uuid:")
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect();
        let key: Vec<u8> = (0..16)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap())
            .collect();
        let mut obfuscated = clear.clone();
        for (i, byte) in obfuscated.iter_mut().take(1024).enumerate() {
            *byte ^= key[i % key.len()];
        }
        let recovered = deobfuscate(
            &obfuscated,
            ObfuscationAlgorithm::Adobe,
            &[identifier.to_string()],
        )
        .expect("Adobe deobfuscation should succeed");
        assert_eq!(recovered, clear);
        println!("  \u{2713} Adobe obfuscation round-trips");
    }

    #[test]
    fn css_font_urls_rewritten_to_kindle_embed() {
        let mut embeds = std::collections::HashMap::new();
        embeds.insert(
            "fonts/Custom.ttf".to_string(),
            (3usize, "application/x-font-ttf".to_string()),
        );
        let css = r#"@font-face {
  font-family: "Custom";
  src: url("../fonts/Custom.ttf");
}
.body { background: url(images/bg.png); }"#;
        let out = rewrite_css_font_urls(css, "css", &embeds);
        assert!(
            out.contains("url(kindle:embed:0003?mime=application/x-font-ttf)"),
            "font url should be rewritten: {}",
            out
        );
        assert!(
            out.contains("url(images/bg.png)"),
            "non-font url should be untouched: {}",
            out
        );
        println!("  \u{2713} @font-face urls rewritten to kindle:embed");
    }

    #[test]
    fn font_family_stripped_when_no_fonts_embedded() {
        let css = r#"body {
    font-family: "SimSun", "STSong", "Arial", sans-serif;
    line-height: 1.5;
}
.chapter-title { font-family: "SimHei", "黑体", "Arial", sans-serif }
pre, code { font-family: "Courier New", Courier, monospace; }
@font-face {
  font-family: "Dead";
  src: url("../fonts/missing.ttf");
}
p { margin: 0 }"#;
        let out = strip_font_locking_css(css);
        assert!(
            !out.contains("SimSun") && !out.contains("SimHei") && !out.contains("黑体"),
            "named families must be stripped: {}",
            out
        );
        assert!(
            !out.contains("sans-serif"),
            "generic fallbacks of named stacks must go too: {}",
            out
        );
        assert!(
            out.contains("font-family: monospace;"),
            "monospace stacks keep their generic: {}",
            out
        );
        assert!(
            !out.contains("@font-face") && !out.contains("missing.ttf"),
            "dead @font-face rules must be removed: {}",
            out
        );
        assert!(
            out.contains("line-height: 1.5") && out.contains("p { margin: 0 }"),
            "unrelated CSS must survive: {}",
            out
        );
        // Declaration closed by `}` instead of `;` is also stripped.
        let out2 = strip_font_locking_css(".t{font-family:STKaiti}");
        assert_eq!(out2, ".t{}");
        println!("  \u{2713} font-family stripped from unembedded-font CSS");
    }

    #[test]
    fn style_blocks_stripped_in_html() {
        let html = r#"<html><head>
<style type="text/css">
body { font-family: "SimSun", serif; color: black; }
</style></head>
<body><p style="x">font-family: not-css-here</p></body></html>"#;
        let out = strip_font_locking_style_blocks(html);
        assert!(
            !out.contains("SimSun"),
            "style block font-family must be stripped: {}",
            out
        );
        assert!(out.contains("color: black"), "other declarations survive");
        assert!(
            out.contains("font-family: not-css-here"),
            "body text mentioning font-family is untouched"
        );
        println!("  \u{2713} inline <style> blocks stripped");
    }

    #[test]
    fn encryption_xml_entries_parsed() {
        let xml = r#"<?xml version="1.0"?>
<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"
            xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/fonts/A.otf"/></enc:CipherData>
  </enc:EncryptedData>
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://ns.adobe.com/pdf/enc#RC"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/fonts/B.ttf"/></enc:CipherData>
  </enc:EncryptedData>
</encryption>"#;
        let entries = parse_encryption_entries(xml);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "OEBPS/fonts/A.otf");
        assert_eq!(entries[0].1, ObfuscationAlgorithm::Idpf);
        assert_eq!(entries[1].0, "OEBPS/fonts/B.ttf");
        assert_eq!(entries[1].1, ObfuscationAlgorithm::Adobe);
        println!("  \u{2713} encryption.xml entries parsed");
    }
}
