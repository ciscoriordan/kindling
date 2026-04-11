//! EPUB repair pass for common Kindle ingest issues.
//!
//! Port of the fix list from innocenat/kindle-epub-fix (UNLICENSE public
//! domain), a browser tool that rewrites EPUBs so Amazon's Send-to-Kindle
//! service accepts them without mangling encoding or breaking internal
//! links. Reference: https://github.com/innocenat/kindle-epub-fix
//!
//! The fixes this module applies, all structural and content-preserving:
//!
//! 1. `fixEncoding`     - prepend `<?xml version="1.0" encoding="utf-8"?>`
//!    to any XHTML/HTML file that is missing an XML declaration. Amazon's
//!    legacy ingest assumes ISO-8859-1 when no encoding is declared, which
//!    corrupts any non-ASCII character.
//! 2. `fixBodyIdLink`   - when a `<body>` element has an `id`, rewrite every
//!    `filename#body-id` reference elsewhere in the EPUB to just `filename`,
//!    because Kindle silently drops fragments that point into a body tag.
//! 3. `fixBookLanguage` - ensure the OPF has a `<dc:language>` element and,
//!    if it is missing entirely, fall back to `en`. The reference tool also
//!    prompts the user when a language is outside Kindle's allowed list; we
//!    record that as a warning instead of prompting, because this is a CLI.
//! 4. `fixStrayIMG`     - delete `<img>` tags that have no `src` attribute,
//!    which otherwise become broken image placeholders on Kindle.
//!
//! This module only touches files that need touching. On a clean EPUB the
//! output is byte-identical to the input (via `std::fs::copy`) so content
//! hashes remain stable for callers that use EPUB hashing as book identity.
//! The pass is idempotent: running it twice equals running it once.
//!
//! DRM-encrypted EPUBs are rejected with `RepairError::DrmEncrypted` and
//! never touched. This module does not link, call, or reference any DRM
//! removal code.
//!
//! This is a separate pass from `crate::validate`, which reports KDP rule
//! violations non-destructively. Where a validator rule corresponds to a
//! repair fix, the `Fix` doc comment references the rule id.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Report produced by [`repair_epub`]. Lists every fix applied plus any
/// non-fatal warnings emitted during the pass.
#[derive(Debug, Clone)]
pub struct RepairReport {
    pub fixes_applied: Vec<Fix>,
    pub warnings: Vec<Warning>,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
}

impl RepairReport {
    /// True if at least one fix was recorded.
    pub fn any_fixes(&self) -> bool {
        !self.fixes_applied.is_empty()
    }

    /// Number of fixes recorded.
    pub fn fix_count(&self) -> usize {
        self.fixes_applied.len()
    }

    /// Render the report as a JSON object using only the standard library.
    pub fn to_json(&self) -> String {
        let mut s = String::new();
        s.push('{');
        s.push_str(&format!(
            "\"input_path\":{},",
            json_string(&self.input_path.display().to_string())
        ));
        s.push_str(&format!(
            "\"output_path\":{},",
            json_string(&self.output_path.display().to_string())
        ));
        s.push_str("\"fixes_applied\":[");
        for (i, f) in self.fixes_applied.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&f.to_json());
        }
        s.push_str("],");
        s.push_str("\"warnings\":[");
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            s.push_str(&w.to_json());
        }
        s.push_str("]}");
        s
    }
}

/// One structural fix applied to the EPUB. Each variant corresponds to one
/// of the fixes from innocenat/kindle-epub-fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// Prepended a UTF-8 XML declaration to an XHTML/HTML file that lacked
    /// one. Relates loosely to KDP rule R6.1 (well-formed XHTML).
    AddedXmlDeclaration { file: String },

    /// Rewrote a `filename#body-id` hyperlink to just `filename`. Kindle
    /// silently drops fragments that point at a `<body>` element. The
    /// `file` field is the file the reference lives in; `original_href` is
    /// the broken href; `new_href` is the repaired one.
    FixedBodyIdLink {
        file: String,
        original_href: String,
        new_href: String,
    },

    /// Injected a `<dc:language>` element into the OPF because it was
    /// missing. Kindle requires a language tag to pick fonts and hyphenation.
    AddedLanguageTag {
        file: String,
        lang: String,
        source: LangSource,
    },

    /// Removed one or more stray `<img>` tags with no `src` attribute.
    /// `count` is the number of img tags deleted in this file.
    RemovedStrayImg { file: String, count: usize },
}

/// Where an injected language tag came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LangSource {
    /// The language was already present in the OPF; we did not need to
    /// inject one. This variant is unused for now but reserved so the API
    /// can distinguish a found language from a fallback later.
    FromOpf,
    /// No language tag was present; we fell back to English.
    FallbackEn,
}

impl fmt::Display for LangSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LangSource::FromOpf => write!(f, "from-opf"),
            LangSource::FallbackEn => write!(f, "fallback-en"),
        }
    }
}

impl Fix {
    /// One-line human description for printing on stderr.
    pub fn describe(&self) -> String {
        match self {
            Fix::AddedXmlDeclaration { file } => {
                format!("added XML declaration to {}", file)
            }
            Fix::FixedBodyIdLink {
                file,
                original_href,
                new_href,
            } => format!(
                "rewrote body-id link {} to {} in {}",
                original_href, new_href, file
            ),
            Fix::AddedLanguageTag { file, lang, source } => format!(
                "added dc:language={} ({}) to {}",
                lang, source, file
            ),
            Fix::RemovedStrayImg { file, count } => format!(
                "removed {} stray img tag{} in {}",
                count,
                if *count == 1 { "" } else { "s" },
                file
            ),
        }
    }

    fn to_json(&self) -> String {
        match self {
            Fix::AddedXmlDeclaration { file } => format!(
                "{{\"kind\":\"added_xml_declaration\",\"file\":{}}}",
                json_string(file)
            ),
            Fix::FixedBodyIdLink {
                file,
                original_href,
                new_href,
            } => format!(
                "{{\"kind\":\"fixed_body_id_link\",\"file\":{},\"original_href\":{},\"new_href\":{}}}",
                json_string(file),
                json_string(original_href),
                json_string(new_href)
            ),
            Fix::AddedLanguageTag { file, lang, source } => format!(
                "{{\"kind\":\"added_language_tag\",\"file\":{},\"lang\":{},\"source\":{}}}",
                json_string(file),
                json_string(lang),
                json_string(&source.to_string())
            ),
            Fix::RemovedStrayImg { file, count } => format!(
                "{{\"kind\":\"removed_stray_img\",\"file\":{},\"count\":{}}}",
                json_string(file),
                count
            ),
        }
    }
}

/// Non-fatal warning emitted during repair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub file: String,
    pub message: String,
}

impl Warning {
    fn to_json(&self) -> String {
        format!(
            "{{\"file\":{},\"message\":{}}}",
            json_string(&self.file),
            json_string(&self.message)
        )
    }
}

/// Errors from [`repair_epub`].
#[derive(Debug)]
pub enum RepairError {
    Io(std::io::Error),
    ZipRead(zip::result::ZipError),
    ZipWrite(zip::result::ZipError),
    /// Input is DRM-encrypted. We refuse to touch it.
    DrmEncrypted,
    /// Input is not a ZIP archive or lacks EPUB structural markers.
    NotAnEpub,
    /// OPF could not be parsed for a fix that needs it.
    MalformedOpf(String),
}

impl fmt::Display for RepairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RepairError::Io(e) => write!(f, "I/O error: {}", e),
            RepairError::ZipRead(e) => write!(f, "ZIP read error: {}", e),
            RepairError::ZipWrite(e) => write!(f, "ZIP write error: {}", e),
            RepairError::DrmEncrypted => write!(
                f,
                "EPUB is DRM-encrypted; refusing to repair"
            ),
            RepairError::NotAnEpub => write!(f, "input is not a valid EPUB archive"),
            RepairError::MalformedOpf(m) => write!(f, "malformed OPF: {}", m),
        }
    }
}

impl std::error::Error for RepairError {}

impl From<std::io::Error> for RepairError {
    fn from(e: std::io::Error) -> Self {
        RepairError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Repair an EPUB file.
///
/// Reads `input`, applies the fixes listed at the top of this module, and
/// writes the result to `output`. Returns a [`RepairReport`] describing what
/// was fixed.
///
/// If no fixes are needed, `output` is a byte-identical copy of `input` so
/// that downstream content hashing stays stable.
///
/// DRM-encrypted EPUBs are rejected with [`RepairError::DrmEncrypted`]. This
/// function does not and will never decrypt.
///
/// This function is idempotent: running it twice on the same input produces
/// the same output as running it once.
pub fn repair_epub(input: &Path, output: &Path) -> Result<RepairReport, RepairError> {
    let report = repair_epub_inner(input, output, false)?;
    Ok(report)
}

/// Scan an EPUB and report what would be fixed, without writing an output
/// file. Used by `kindling repair --dry-run`.
pub fn scan_epub(input: &Path) -> Result<RepairReport, RepairError> {
    repair_epub_inner(input, input, true)
}

fn repair_epub_inner(
    input: &Path,
    output: &Path,
    dry_run: bool,
) -> Result<RepairReport, RepairError> {
    // Load the archive. BTreeMap gives us a stable iteration order for
    // determinism, which matters for byte-stability of the re-zipped output
    // on subsequent runs.
    let archive_bytes = std::fs::read(input)?;
    let reader = Cursor::new(&archive_bytes);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|_| RepairError::NotAnEpub)?;

    // Refuse DRM-encrypted files. Presence of META-INF/encryption.xml or
    // META-INF/rights.xml is the universal marker (Adobe ADEPT, B&N Social
    // DRM, Readium LCP all use one of these).
    for name in archive.file_names() {
        let lower = name.to_ascii_lowercase();
        if lower == "meta-inf/encryption.xml" || lower == "meta-inf/rights.xml" {
            return Err(RepairError::DrmEncrypted);
        }
    }

    // Sanity-check EPUB structure: we need a mimetype file and a
    // META-INF/container.xml. If neither is present, this is not an EPUB.
    let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
    let has_container = names
        .iter()
        .any(|n| n.eq_ignore_ascii_case("META-INF/container.xml"));
    if !has_container {
        return Err(RepairError::NotAnEpub);
    }

    // Classify each entry: text files we might edit, binary files we pass
    // through. The text set matches the reference implementation.
    let mut text_files: BTreeMap<String, String> = BTreeMap::new();
    let mut binary_files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(RepairError::ZipRead)?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        order.push(name.clone());

        if is_text_file(&name) {
            let mut s = String::new();
            if entry.read_to_string(&mut s).is_ok() {
                text_files.insert(name, s);
                continue;
            }
            // Fallback: if the entry was not valid UTF-8, treat it as
            // binary and leave it alone. We never corrupt encodings.
            let mut buf = Vec::new();
            // We already consumed the reader above on failure. Re-open by
            // index to read fresh bytes.
            drop(entry);
            let mut entry = archive
                .by_index(i)
                .map_err(RepairError::ZipRead)?;
            entry.read_to_end(&mut buf)?;
            binary_files.insert(name, buf);
        } else {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            binary_files.insert(name, buf);
        }
    }

    // Locate the OPF via container.xml. We only need the relative path.
    let container_key = names
        .iter()
        .find(|n| n.eq_ignore_ascii_case("META-INF/container.xml"))
        .cloned()
        .ok_or(RepairError::NotAnEpub)?;
    let container_xml = text_files
        .get(&container_key)
        .cloned()
        .ok_or_else(|| {
            RepairError::MalformedOpf("container.xml not readable as text".into())
        })?;
    let opf_path = parse_container_rootfile(&container_xml)
        .ok_or_else(|| RepairError::MalformedOpf("no rootfile in container.xml".into()))?;

    // Apply the fixes, in the same order as the reference implementation:
    //   fixBodyIdLink, fixBookLanguage, fixStrayIMG, fixEncoding
    let mut fixes: Vec<Fix> = Vec::new();
    let mut warnings: Vec<Warning> = Vec::new();

    fix_body_id_link(&mut text_files, &mut fixes);
    fix_book_language(&opf_path, &mut text_files, &mut fixes, &mut warnings);
    fix_stray_img(&mut text_files, &mut fixes);
    fix_encoding(&mut text_files, &mut fixes);

    let report = RepairReport {
        fixes_applied: fixes,
        warnings,
        input_path: input.to_path_buf(),
        output_path: output.to_path_buf(),
    };

    if dry_run {
        return Ok(report);
    }

    // Byte-stable path. If nothing changed, copy the original bytes exactly.
    // This preserves ZIP compression, central directory order, timestamps
    // and so on, which matters for callers using EPUB hashes as identity.
    if !report.any_fixes() {
        // Handle in-place repair (input == output) by skipping the copy.
        let same = paths_equal(input, output);
        if !same {
            std::fs::copy(input, output)?;
        }
        return Ok(report);
    }

    // Re-zip. We must preserve EPUB structural requirements:
    //   * `mimetype` must be the first entry and stored uncompressed
    //   * all other entries can be deflated
    let mut out_buf: Vec<u8> = Vec::new();
    {
        let cursor = Cursor::new(&mut out_buf);
        let mut writer = zip::ZipWriter::new(cursor);

        let stored = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(fixed_timestamp());
        let deflate = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(fixed_timestamp());

        // Write mimetype first if present. The text-file classifier picks
        // up `mimetype` only if it has a recognised extension, which it
        // does not, so it lives in `binary_files`.
        let mimetype_key = order
            .iter()
            .find(|n| n.as_str() == "mimetype")
            .cloned();
        if let Some(ref k) = mimetype_key {
            let bytes = binary_files
                .get(k)
                .cloned()
                .or_else(|| text_files.get(k).map(|s| s.as_bytes().to_vec()))
                .unwrap_or_default();
            writer
                .start_file(k, stored)
                .map_err(RepairError::ZipWrite)?;
            writer.write_all(&bytes)?;
        }

        // Write every other entry in original order so a clean re-zip of
        // an already-clean file has deterministic layout.
        for name in &order {
            if Some(name) == mimetype_key.as_ref() {
                continue;
            }
            if let Some(text) = text_files.get(name) {
                writer
                    .start_file(name, deflate)
                    .map_err(RepairError::ZipWrite)?;
                writer.write_all(text.as_bytes())?;
            } else if let Some(bin) = binary_files.get(name) {
                writer
                    .start_file(name, deflate)
                    .map_err(RepairError::ZipWrite)?;
                writer.write_all(bin)?;
            }
        }

        writer.finish().map_err(RepairError::ZipWrite)?;
    }

    std::fs::write(output, &out_buf)?;

    Ok(report)
}

// ---------------------------------------------------------------------------
// Fix helpers
// ---------------------------------------------------------------------------

/// True if `name` is a text file we should read as UTF-8 and potentially edit.
/// Matches the reference set: html, xhtml, htm, xml, svg, css, opf, ncx.
fn is_text_file(name: &str) -> bool {
    match extension(name).as_deref() {
        Some("html") | Some("xhtml") | Some("htm") | Some("xml") | Some("svg")
        | Some("css") | Some("opf") | Some("ncx") => true,
        _ => false,
    }
}

/// Lowercased file extension, if any.
fn extension(name: &str) -> Option<String> {
    let base = name.rsplit('/').next().unwrap_or(name);
    let dot = base.rfind('.')?;
    Some(base[dot + 1..].to_ascii_lowercase())
}

fn is_html_like(name: &str) -> bool {
    matches!(
        extension(name).as_deref(),
        Some("html") | Some("xhtml") | Some("htm")
    )
}

/// Last path segment (basename). EPUB entries use forward slashes regardless
/// of platform.
fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

// --- Fix 1: XML declaration ------------------------------------------------

/// Prepend `<?xml version="1.0" encoding="utf-8"?>` to any XHTML/HTML file
/// that does not start with an XML declaration. Matches the regex from the
/// reference implementation: leading whitespace trimmed, then
/// `<?xml version="X.Y" encoding="CHARSET"?>` expected.
fn fix_encoding(text_files: &mut BTreeMap<String, String>, fixes: &mut Vec<Fix>) {
    const DECL: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>";

    let names: Vec<String> = text_files
        .keys()
        .filter(|n| is_html_like(n))
        .cloned()
        .collect();

    for name in names {
        let content = text_files.get(&name).cloned().unwrap_or_default();
        let trimmed = content.trim_start();
        if has_xml_declaration(trimmed) {
            // Keep the trimmed version if it differs, so that idempotent
            // runs do not slowly eat leading whitespace. Actually, no:
            // idempotence requires that we do NOT mutate unless we are
            // applying a fix. Leave the content alone.
            continue;
        }
        let new_content = format!("{}\n{}", DECL, trimmed);
        text_files.insert(name.clone(), new_content);
        fixes.push(Fix::AddedXmlDeclaration { file: name });
    }
}

/// True if the given text starts with an XML declaration. We implement this
/// by scanning characters rather than compiling a regex, to avoid dragging
/// in a regex dependency for one call and to make the matching rules
/// obvious.
fn has_xml_declaration(s: &str) -> bool {
    // Expected prefix pattern (case-insensitive):
    //   <?xml<WS>+version=<QUOTE><DIGITS.DIGITS><QUOTE><WS>+
    //         encoding=<QUOTE><CHARSET><QUOTE>.*?\?>
    let bytes = s.as_bytes();
    if bytes.len() < 6 {
        return false;
    }
    if !s.to_ascii_lowercase().starts_with("<?xml") {
        return false;
    }
    // Must end the declaration with ?>
    let end = match s.find("?>") {
        Some(i) => i,
        None => return false,
    };
    let head = &s[..end].to_ascii_lowercase();
    head.contains("version=") && head.contains("encoding=")
}

// --- Fix 2: body-id link rewriting -----------------------------------------

/// For every XHTML/HTML file whose `<body>` tag has an `id`, any href
/// anywhere in the EPUB of the form `basename(file)#body-id` is rewritten to
/// just `basename(file)`. This matches the reference's `fixBodyIdLink`
/// behaviour, including doing a literal substring replacement.
fn fix_body_id_link(text_files: &mut BTreeMap<String, String>, fixes: &mut Vec<Fix>) {
    // Step 1: collect (broken_href, repaired_href) pairs from every HTML
    // file with a body id.
    let mut rewrites: Vec<(String, String)> = Vec::new();
    for (name, content) in text_files.iter() {
        if !is_html_like(name) {
            continue;
        }
        if let Some(body_id) = find_body_id(content) {
            if body_id.is_empty() {
                continue;
            }
            let base = basename(name);
            let broken = format!("{}#{}", base, body_id);
            let repaired = base.to_string();
            rewrites.push((broken, repaired));
        }
    }

    if rewrites.is_empty() {
        return;
    }

    // Step 2: apply every rewrite across every text file. Order of rewrites
    // does not matter because the broken hrefs are disjoint (they embed the
    // unique body-id).
    let names: Vec<String> = text_files.keys().cloned().collect();
    for name in names {
        let mut content = text_files.get(&name).cloned().unwrap_or_default();
        let mut file_changed = false;
        for (broken, repaired) in &rewrites {
            if content.contains(broken.as_str()) {
                content = content.replace(broken.as_str(), repaired.as_str());
                fixes.push(Fix::FixedBodyIdLink {
                    file: name.clone(),
                    original_href: broken.clone(),
                    new_href: repaired.clone(),
                });
                file_changed = true;
            }
        }
        if file_changed {
            text_files.insert(name, content);
        }
    }
}

/// Return the value of the `id` attribute on the first `<body ...>` tag in
/// `html`, or `None` if none is present. Tolerates attribute quoting and
/// whitespace; does not require a real HTML parser.
fn find_body_id(html: &str) -> Option<String> {
    // Find the opening body tag. Skip content inside comments, PIs, or
    // CDATA would be nice, but the reference does not bother; neither do we.
    let lower = html.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(idx) = lower[search..].find("<body") {
        let abs = search + idx;
        // Ensure the next character is whitespace, `>`, or `/` so we do not
        // match `<bodytag`.
        let after = abs + "<body".len();
        let ch = lower.as_bytes().get(after).copied().unwrap_or(b' ');
        if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'>' || ch == b'/' {
            // Find the end of the tag
            let end = match lower[abs..].find('>') {
                Some(e) => abs + e,
                None => return None,
            };
            let tag = &html[abs..=end];
            return extract_attr(tag, "id");
        }
        search = after;
    }
    None
}

/// Extract `attr="value"` or `attr='value'` from a tag string. Returns the
/// value if present.
fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let target = format!("{}=", attr.to_ascii_lowercase());
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find(&target) {
        let at = search + rel;
        // Attribute names must be preceded by whitespace or `<tagname`.
        if at > 0 {
            let prev = lower.as_bytes()[at - 1];
            if !(prev == b' ' || prev == b'\t' || prev == b'\n' || prev == b'\r') {
                search = at + target.len();
                continue;
            }
        }
        let after = at + target.len();
        let bytes = tag.as_bytes();
        if after >= bytes.len() {
            return None;
        }
        let quote = bytes[after];
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        let value_start = after + 1;
        let rest = &tag[value_start..];
        let end = rest.find(quote as char)?;
        return Some(rest[..end].to_string());
    }
    None
}

// --- Fix 3: language tag ---------------------------------------------------

/// Amazon's KDP allowed-language list, copied verbatim from the reference
/// implementation (snapshotted 2022-09-13). We use it only to emit a
/// warning, never to block the repair.
const ALLOWED_LANGUAGES: &[&str] = &[
    // ISO 639-1
    "af", "gsw", "ar", "eu", "nb", "br", "ca", "zh", "kw", "co", "da", "nl", "stq",
    "en", "fi", "fr", "fy", "gl", "de", "gu", "hi", "is", "ga", "it", "ja", "lb",
    "mr", "ml", "gv", "frr", "nn", "pl", "pt", "oc", "rm", "sco", "gd", "es", "sv",
    "ta", "cy",
    // ISO 639-2
    "afr", "ara", "eus", "baq", "nob", "bre", "cat", "zho", "chi", "cor", "cos",
    "dan", "nld", "dut", "eng", "fin", "fra", "fre", "fry", "glg", "deu", "ger",
    "guj", "hin", "isl", "ice", "gle", "ita", "jpn", "ltz", "mar", "mal", "glv",
    "nor", "nno", "por", "oci", "roh", "gla", "spa", "swe", "tam", "cym", "wel",
];

/// Ensure the OPF has a `<dc:language>`. If missing, inject one with `en`
/// and record an [`Fix::AddedLanguageTag`]. If present but outside the KDP
/// allowed list, emit a warning and leave it alone.
fn fix_book_language(
    opf_path: &str,
    text_files: &mut BTreeMap<String, String>,
    fixes: &mut Vec<Fix>,
    warnings: &mut Vec<Warning>,
) {
    let opf = match text_files.get(opf_path) {
        Some(s) => s.clone(),
        None => return,
    };

    // Is there already a dc:language element?
    if let Some(lang) = find_tag_text(&opf, "dc:language") {
        let simplified = simplify_language(&lang);
        if !ALLOWED_LANGUAGES.iter().any(|a| *a == simplified) {
            warnings.push(Warning {
                file: opf_path.to_string(),
                message: format!(
                    "dc:language '{}' is not in the KDP allowed list; Kindle conversion may fail",
                    lang
                ),
            });
        }
        return;
    }

    // Missing: inject `<dc:language>en</dc:language>` before the closing
    // `</metadata>` tag.
    let needle = "</metadata>";
    let idx = match opf.find(needle) {
        Some(i) => i,
        None => {
            // Try case-insensitive fallback. If still not found, leave the
            // OPF alone and emit a warning.
            let lower = opf.to_ascii_lowercase();
            match lower.find("</metadata>") {
                Some(i) => i,
                None => {
                    warnings.push(Warning {
                        file: opf_path.to_string(),
                        message: "OPF is missing a <metadata> block; cannot inject dc:language"
                            .into(),
                    });
                    return;
                }
            }
        }
    };
    let insert = "    <dc:language>en</dc:language>\n  ";
    let mut new_opf = String::with_capacity(opf.len() + insert.len());
    new_opf.push_str(&opf[..idx]);
    new_opf.push_str(insert);
    new_opf.push_str(&opf[idx..]);
    text_files.insert(opf_path.to_string(), new_opf);
    fixes.push(Fix::AddedLanguageTag {
        file: opf_path.to_string(),
        lang: "en".to_string(),
        source: LangSource::FallbackEn,
    });
}

/// Return the text content of the first `<tag>...</tag>` occurrence, or
/// `None` if absent. Whitespace around the text is trimmed.
fn find_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}", tag);
    let close = format!("</{}>", tag);
    let start = xml.find(&open)?;
    // Skip the rest of the opening tag.
    let gt = xml[start..].find('>')?;
    let text_start = start + gt + 1;
    let rel_end = xml[text_start..].find(&close)?;
    let text = &xml[text_start..text_start + rel_end];
    Some(text.trim().to_string())
}

/// Map `en-US` to `en`, strip case.
fn simplify_language(lang: &str) -> String {
    let head = lang.split('-').next().unwrap_or(lang);
    head.to_ascii_lowercase()
}

// --- Fix 4: stray <img> ----------------------------------------------------

/// Remove every `<img ...>` or `<img ... />` tag that has no `src`
/// attribute (or only an empty-string `src`).
fn fix_stray_img(text_files: &mut BTreeMap<String, String>, fixes: &mut Vec<Fix>) {
    let names: Vec<String> = text_files
        .keys()
        .filter(|n| is_html_like(n))
        .cloned()
        .collect();
    for name in names {
        let content = text_files.get(&name).cloned().unwrap_or_default();
        let (new_content, count) = strip_stray_img(&content);
        if count > 0 {
            text_files.insert(name.clone(), new_content);
            fixes.push(Fix::RemovedStrayImg {
                file: name,
                count,
            });
        }
    }
}

/// Parse `html` looking for `<img ...>` tags. For each one missing a
/// non-empty `src` attribute, elide it from the output. Returns the new
/// string and the number of tags removed.
fn strip_stray_img(html: &str) -> (String, usize) {
    let mut out = String::with_capacity(html.len());
    let lower = html.to_ascii_lowercase();
    let mut i = 0usize;
    let bytes = html.as_bytes();
    let mut count = 0usize;

    while i < bytes.len() {
        // Find the next `<img` opening, respecting case.
        let rel = lower[i..].find("<img");
        let Some(rel) = rel else {
            out.push_str(&html[i..]);
            break;
        };
        let tag_start = i + rel;
        // The next character must be whitespace, `>`, or `/` so we do not
        // match `<imgsomething`.
        let after = tag_start + "<img".len();
        let ch = bytes.get(after).copied().unwrap_or(b' ');
        if !(ch == b' '
            || ch == b'\t'
            || ch == b'\n'
            || ch == b'\r'
            || ch == b'>'
            || ch == b'/')
        {
            out.push_str(&html[i..after]);
            i = after;
            continue;
        }
        // Find the end of the tag. `<img>` is void, so no closing tag to
        // worry about. Handle `>` inside quoted attribute values correctly.
        let tag_end = match find_tag_end(html, tag_start) {
            Some(e) => e,
            None => {
                // Malformed; emit the rest unchanged and stop.
                out.push_str(&html[i..]);
                break;
            }
        };
        let tag_text = &html[tag_start..=tag_end];
        // Does this img have a non-empty src?
        let has_src = extract_attr(tag_text, "src")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
        if has_src {
            out.push_str(&html[i..=tag_end]);
        } else {
            out.push_str(&html[i..tag_start]);
            count += 1;
        }
        i = tag_end + 1;
    }

    (out, count)
}

/// Return the index of the final `>` byte of the tag whose opening `<`
/// starts at `start`. Handles quoted attribute values so that a `>` inside
/// an attribute does not fool us.
fn find_tag_end(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = start;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => {
                if b == b'"' || b == b'\'' {
                    quote = Some(b);
                } else if b == b'>' {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

// ---------------------------------------------------------------------------
// container.xml helper
// ---------------------------------------------------------------------------

/// Return the full-path of the first `rootfile` element in container.xml,
/// or None if we cannot find one. We must match `<rootfile` followed by
/// whitespace or `/` or `>`, so that the enclosing `<rootfiles>` wrapper
/// does not swallow the search.
fn parse_container_rootfile(xml: &str) -> Option<String> {
    let mut search = 0usize;
    let bytes = xml.as_bytes();
    while let Some(rel) = xml[search..].find("<rootfile") {
        let abs = search + rel;
        let after = abs + "<rootfile".len();
        let ch = bytes.get(after).copied().unwrap_or(b' ');
        if ch == b' ' || ch == b'\t' || ch == b'\n' || ch == b'\r' || ch == b'/' || ch == b'>' {
            let end_rel = xml[abs..].find('>')?;
            let tag = &xml[abs..=abs + end_rel];
            return extract_attr(tag, "full-path");
        }
        search = after;
    }
    None
}

// ---------------------------------------------------------------------------
// Small utilities
// ---------------------------------------------------------------------------

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Fixed zero-time stamp used for re-zipped output so that successive runs
/// on the same logical content produce the same bytes. The EPUB spec does
/// not require any particular timestamp, and we deliberately drop the
/// extended timestamp field just like the reference implementation.
fn fixed_timestamp() -> zip::DateTime {
    // 1980-01-01T00:00:00 is the earliest valid ZIP DOS timestamp.
    zip::DateTime::from_date_and_time(1980, 1, 1, 0, 0, 0)
        .unwrap_or_else(|_| zip::DateTime::default())
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    let ca = a.canonicalize().ok();
    let cb = b.canonicalize().ok();
    match (ca, cb) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory EPUB with the given text/binary entries. `entries`
    /// is a slice of (name, bytes) pairs. `mimetype` is injected as the
    /// first entry (STORED) automatically unless already present.
    fn build_epub(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let stored = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored)
                .last_modified_time(fixed_timestamp());
            let deflate = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .last_modified_time(fixed_timestamp());

            let has_mimetype = entries.iter().any(|(n, _)| *n == "mimetype");
            if !has_mimetype {
                w.start_file("mimetype", stored).unwrap();
                w.write_all(b"application/epub+zip").unwrap();
            }
            for (name, bytes) in entries {
                let opts = if *name == "mimetype" { stored } else { deflate };
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }

    fn minimal_container() -> (&'static str, &'static [u8]) {
        (
            "META-INF/container.xml",
            br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
    }

    fn minimal_opf(with_language: bool) -> Vec<u8> {
        let lang = if with_language {
            "<dc:language>en</dc:language>"
        } else {
            ""
        };
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Repair Test</dc:title>
    <dc:identifier id="uid">urn:uuid:repair-test</dc:identifier>
    <dc:creator>Test</dc:creator>
    {lang}
  </metadata>
  <manifest>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
  </spine>
</package>"#,
            lang = lang
        )
        .into_bytes()
    }

    fn good_xhtml(body_id: Option<&str>, body_inner: &str) -> Vec<u8> {
        let body_attr = match body_id {
            Some(id) => format!(" id=\"{}\"", id),
            None => String::new(),
        };
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Ch1</title></head>
  <body{attr}>
    {inner}
  </body>
</html>"#,
            attr = body_attr,
            inner = body_inner
        )
        .into_bytes()
    }

    fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kindling_repair_{}_{}",
            std::process::id(),
            name
        ));
        std::fs::write(&p, bytes).unwrap();
        p
    }

    fn read_epub_entry(path: &Path, entry_name: &str) -> Option<String> {
        let bytes = std::fs::read(path).ok()?;
        let mut a = zip::ZipArchive::new(Cursor::new(bytes)).ok()?;
        let mut e = a.by_name(entry_name).ok()?;
        let mut s = String::new();
        e.read_to_string(&mut s).ok()?;
        Some(s)
    }

    fn list_entries(path: &Path) -> Vec<String> {
        let bytes = std::fs::read(path).unwrap();
        let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..a.len()).map(|i| a.by_index(i).unwrap().name().to_string()).collect()
    }

    // ------------------- Fix 1: AddedXmlDeclaration ------------------------

    #[test]
    fn fix_encoding_positive_adds_declaration() {
        // ch1.xhtml lacks the `<?xml ...?>` declaration
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>X</title></head><body><p>hi</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
        ]);
        let input = write_tmp("enc_pos_in.epub", &epub);
        let output = write_tmp("enc_pos_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();

        assert_eq!(
            report.fixes_applied.len(),
            1,
            "expected exactly one fix"
        );
        assert!(matches!(
            &report.fixes_applied[0],
            Fix::AddedXmlDeclaration { file } if file == "OEBPS/ch1.xhtml"
        ));

        let out_ch1 = read_epub_entry(&output, "OEBPS/ch1.xhtml").unwrap();
        assert!(
            out_ch1.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"),
            "output should begin with xml declaration: {:?}",
            &out_ch1[..40.min(out_ch1.len())]
        );
    }

    #[test]
    fn fix_encoding_negative_leaves_declaration_alone() {
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("enc_neg_in.epub", &epub);
        let output = write_tmp("enc_neg_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(
            !report
                .fixes_applied
                .iter()
                .any(|f| matches!(f, Fix::AddedXmlDeclaration { .. })),
            "should not report AddedXmlDeclaration on a clean file; got {:?}",
            report.fixes_applied
        );
    }

    #[test]
    fn fix_encoding_uppercase_declaration_is_recognised() {
        // Declarations with upper-case XML are also valid and must not be
        // re-prepended.
        let ch1 = br#"<?XML version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
        ]);
        let input = write_tmp("enc_upper_in.epub", &epub);
        let output = write_tmp("enc_upper_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report.fixes_applied.iter().any(|f| matches!(
            f,
            Fix::AddedXmlDeclaration { .. }
        )));
    }

    // ------------------- Fix 2: FixedBodyIdLink ----------------------------

    #[test]
    fn fix_body_id_link_positive_rewrites_reference() {
        let ch1 = good_xhtml(Some("start"), "<p>Chapter 1</p>");
        let toc = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p><a href="ch1.xhtml#start">Go</a></p>
</body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &ch1),
            ("OEBPS/toc.xhtml", toc),
        ]);
        let input = write_tmp("bodyid_pos_in.epub", &epub);
        let output = write_tmp("bodyid_pos_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();

        let count = report
            .fixes_applied
            .iter()
            .filter(|f| matches!(f, Fix::FixedBodyIdLink { .. }))
            .count();
        assert_eq!(count, 1, "expected one body-id link rewrite: {:?}", report.fixes_applied);

        let out_toc = read_epub_entry(&output, "OEBPS/toc.xhtml").unwrap();
        assert!(out_toc.contains("href=\"ch1.xhtml\""));
        assert!(!out_toc.contains("ch1.xhtml#start"));
    }

    #[test]
    fn fix_body_id_link_negative_no_body_id() {
        let ch1 = good_xhtml(None, "<p>Chapter 1</p>");
        let toc = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p><a href="ch1.xhtml">Go</a></p>
</body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &ch1),
            ("OEBPS/toc.xhtml", toc),
        ]);
        let input = write_tmp("bodyid_neg_in.epub", &epub);
        let output = write_tmp("bodyid_neg_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report.fixes_applied.iter().any(|f| matches!(f, Fix::FixedBodyIdLink { .. })));
    }

    #[test]
    fn fix_body_id_link_ncx_reference_is_rewritten() {
        // NCX files also reference chapters, so the rewrite should apply
        // across file types.
        let ch1 = good_xhtml(Some("top"), "<p>Chapter 1</p>");
        let ncx = br#"<?xml version="1.0" encoding="utf-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="np1" playOrder="1">
      <navLabel><text>Chapter 1</text></navLabel>
      <content src="ch1.xhtml#top"/>
    </navPoint>
  </navMap>
</ncx>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &ch1),
            ("OEBPS/toc.ncx", ncx),
        ]);
        let input = write_tmp("bodyid_ncx_in.epub", &epub);
        let output = write_tmp("bodyid_ncx_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(report.fixes_applied.iter().any(|f| matches!(f, Fix::FixedBodyIdLink { .. })));
        let out_ncx = read_epub_entry(&output, "OEBPS/toc.ncx").unwrap();
        assert!(out_ncx.contains("src=\"ch1.xhtml\""));
        assert!(!out_ncx.contains("ch1.xhtml#top"));
    }

    // ------------------- Fix 3: AddedLanguageTag ---------------------------

    #[test]
    fn fix_language_positive_injects_en_when_missing() {
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(false)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("lang_pos_in.epub", &epub);
        let output = write_tmp("lang_pos_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        let count = report
            .fixes_applied
            .iter()
            .filter(|f| matches!(f, Fix::AddedLanguageTag { .. }))
            .count();
        assert_eq!(count, 1);
        let out_opf = read_epub_entry(&output, "OEBPS/content.opf").unwrap();
        assert!(out_opf.contains("<dc:language>en</dc:language>"));
    }

    #[test]
    fn fix_language_negative_leaves_existing_alone() {
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("lang_neg_in.epub", &epub);
        let output = write_tmp("lang_neg_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report
            .fixes_applied
            .iter()
            .any(|f| matches!(f, Fix::AddedLanguageTag { .. })));
    }

    #[test]
    fn fix_language_unsupported_language_warns_but_does_not_fix() {
        let opf = br#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Weird Lang</dc:title>
    <dc:identifier id="uid">x</dc:identifier>
    <dc:language>xx</dc:language>
  </metadata>
  <manifest><item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/></manifest>
  <spine><itemref idref="ch1"/></spine>
</package>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", opf),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("lang_warn_in.epub", &epub);
        let output = write_tmp("lang_warn_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report
            .fixes_applied
            .iter()
            .any(|f| matches!(f, Fix::AddedLanguageTag { .. })));
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].message.contains("xx"));
    }

    // ------------------- Fix 4: RemovedStrayImg ----------------------------

    #[test]
    fn fix_stray_img_positive_removes_tag() {
        let body = "<p>Before</p><img alt=\"broken\"/><p>After</p>";
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, body)),
        ]);
        let input = write_tmp("img_pos_in.epub", &epub);
        let output = write_tmp("img_pos_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        let img_fixes: Vec<_> = report
            .fixes_applied
            .iter()
            .filter_map(|f| match f {
                Fix::RemovedStrayImg { count, .. } => Some(*count),
                _ => None,
            })
            .collect();
        assert_eq!(img_fixes, vec![1]);
        let out_ch1 = read_epub_entry(&output, "OEBPS/ch1.xhtml").unwrap();
        assert!(!out_ch1.contains("<img"));
        assert!(out_ch1.contains("<p>Before</p>"));
        assert!(out_ch1.contains("<p>After</p>"));
    }

    #[test]
    fn fix_stray_img_negative_leaves_good_img_alone() {
        let body = "<p>Before</p><img src=\"cover.jpg\" alt=\"ok\"/><p>After</p>";
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, body)),
        ]);
        let input = write_tmp("img_neg_in.epub", &epub);
        let output = write_tmp("img_neg_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report
            .fixes_applied
            .iter()
            .any(|f| matches!(f, Fix::RemovedStrayImg { .. })));
    }

    #[test]
    fn fix_stray_img_removes_multiple_in_same_file() {
        let body = "<img/><p>a</p><img alt=\"\"/><p>b</p><img src=\"ok.png\"/>";
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, body)),
        ]);
        let input = write_tmp("img_multi_in.epub", &epub);
        let output = write_tmp("img_multi_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        let total: usize = report
            .fixes_applied
            .iter()
            .filter_map(|f| match f {
                Fix::RemovedStrayImg { count, .. } => Some(*count),
                _ => None,
            })
            .sum();
        assert_eq!(total, 2, "should remove exactly two stray imgs");
        let out_ch1 = read_epub_entry(&output, "OEBPS/ch1.xhtml").unwrap();
        assert!(out_ch1.contains("src=\"ok.png\""));
    }

    #[test]
    fn fix_stray_img_empty_src_is_stripped() {
        let body = "<p>x</p><img src=\"\" alt=\"empty\"/>";
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, body)),
        ]);
        let input = write_tmp("img_empty_in.epub", &epub);
        let output = write_tmp("img_empty_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(report
            .fixes_applied
            .iter()
            .any(|f| matches!(f, Fix::RemovedStrayImg { .. })));
    }

    // ------------------- Idempotence ---------------------------------------

    #[test]
    fn idempotent_on_broken_input() {
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>X</title></head>
<body id="top"><p>hi</p><img alt="bad"/></body></html>"#;
        let nav = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<a href="ch1.xhtml#top">go</a>
</body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(false)),
            ("OEBPS/ch1.xhtml", ch1),
            ("OEBPS/nav.xhtml", nav),
        ]);
        let input = write_tmp("idem_in.epub", &epub);
        let output1 = write_tmp("idem_out1.epub", b"");
        let output2 = write_tmp("idem_out2.epub", b"");

        let r1 = repair_epub(&input, &output1).unwrap();
        assert!(r1.any_fixes(), "first run should record fixes");

        let r2 = repair_epub(&output1, &output2).unwrap();
        assert!(
            !r2.any_fixes(),
            "second run should record no fixes, got {:?}",
            r2.fixes_applied
        );

        // Both outputs, re-run, must be byte-identical.
        let b1 = std::fs::read(&output1).unwrap();
        let b2 = std::fs::read(&output2).unwrap();
        assert_eq!(b1, b2, "idempotent re-run must be byte-identical");
    }

    // ------------------- Byte-stability ------------------------------------

    #[test]
    fn clean_input_is_copied_byte_identical() {
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("clean_in.epub", &epub);
        let output = write_tmp("clean_out.epub", b"");
        let report = repair_epub(&input, &output).unwrap();
        assert!(!report.any_fixes(), "clean epub should need no fixes: {:?}", report.fixes_applied);
        let in_bytes = std::fs::read(&input).unwrap();
        let out_bytes = std::fs::read(&output).unwrap();
        assert_eq!(
            in_bytes, out_bytes,
            "clean input must be copied byte-identically"
        );
    }

    // ------------------- DRM rejection -------------------------------------

    #[test]
    fn drm_encryption_xml_is_rejected() {
        let epub = build_epub(&[
            minimal_container(),
            (
                "META-INF/encryption.xml",
                br#"<?xml version="1.0"?><encryption/>"#,
            ),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("drm_enc_in.epub", &epub);
        let output = write_tmp("drm_enc_out.epub", b"");
        let err = repair_epub(&input, &output).expect_err("should reject DRM");
        assert!(matches!(err, RepairError::DrmEncrypted));
        assert!(
            !output.exists() || std::fs::metadata(&output).unwrap().len() == 0,
            "must not write output for DRM-protected input"
        );
    }

    #[test]
    fn drm_rights_xml_is_rejected() {
        let epub = build_epub(&[
            minimal_container(),
            ("META-INF/rights.xml", b"<rights/>"),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, "<p>hi</p>")),
        ]);
        let input = write_tmp("drm_rights_in.epub", &epub);
        let output = write_tmp("drm_rights_out.epub", b"");
        let err = repair_epub(&input, &output).expect_err("should reject rights.xml");
        assert!(matches!(err, RepairError::DrmEncrypted));
    }

    // ------------------- Not-an-epub ---------------------------------------

    #[test]
    fn non_zip_input_returns_not_an_epub() {
        let input = write_tmp("junk_in.epub", b"not a zip");
        let output = write_tmp("junk_out.epub", b"");
        let err = repair_epub(&input, &output).expect_err("should reject non-zip");
        assert!(matches!(err, RepairError::NotAnEpub));
    }

    #[test]
    fn zip_without_container_returns_not_an_epub() {
        // Zip with no META-INF/container.xml should not be treated as EPUB.
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let deflate = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated);
            w.start_file("hello.txt", deflate).unwrap();
            w.write_all(b"hello").unwrap();
            w.finish().unwrap();
        }
        let input = write_tmp("nocontainer_in.epub", &buf);
        let output = write_tmp("nocontainer_out.epub", b"");
        let err = repair_epub(&input, &output).expect_err("should reject zip without container");
        assert!(matches!(err, RepairError::NotAnEpub));
    }

    // ------------------- Dry-run -------------------------------------------

    #[test]
    fn dry_run_reports_fixes_without_writing() {
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
        ]);
        let input = write_tmp("dry_in.epub", &epub);
        let report = scan_epub(&input).unwrap();
        assert!(report.any_fixes());
        // No output file path was written because scan_epub does not touch disk.
    }

    // ------------------- JSON report ---------------------------------------

    #[test]
    fn json_report_has_expected_shape() {
        let body = "<p>a</p><img/>";
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &good_xhtml(None, body)),
        ]);
        let input = write_tmp("json_in.epub", &epub);
        let output = write_tmp("json_out.epub", b"");
        let r = repair_epub(&input, &output).unwrap();
        let j = r.to_json();
        assert!(j.starts_with('{'));
        assert!(j.contains("\"fixes_applied\""));
        assert!(j.contains("removed_stray_img"));
    }

    // ------------------- Re-zip correctness --------------------------------

    #[test]
    fn rezipped_output_has_mimetype_first_stored() {
        // After repair, the output must still have mimetype as the first
        // entry and STORED so Send-to-Kindle does not reject it.
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
        ]);
        let input = write_tmp("mime_in.epub", &epub);
        let output = write_tmp("mime_out.epub", b"");
        let _ = repair_epub(&input, &output).unwrap();
        let entries = list_entries(&output);
        assert_eq!(entries.first().map(String::as_str), Some("mimetype"));

        // And the mimetype entry specifically is STORED (method 0).
        let bytes = std::fs::read(&output).unwrap();
        let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let e = a.by_name("mimetype").unwrap();
        assert_eq!(e.compression(), CompressionMethod::Stored);
    }

    #[test]
    fn binary_entries_round_trip_unchanged() {
        let fake_png: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 1, 2, 3];
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
            ("OEBPS/cover.png", fake_png),
        ]);
        let input = write_tmp("bin_in.epub", &epub);
        let output = write_tmp("bin_out.epub", b"");
        let _ = repair_epub(&input, &output).unwrap();
        let bytes = std::fs::read(&output).unwrap();
        let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut e = a.by_name("OEBPS/cover.png").unwrap();
        let mut out = Vec::new();
        e.read_to_end(&mut out).unwrap();
        assert_eq!(out, fake_png);
    }

    // ------------------- Extra: edge cases ---------------------------------

    #[test]
    fn body_id_rewrite_only_affects_matching_href() {
        let ch1 = good_xhtml(Some("top"), "<p>c1</p>");
        let ch2 = good_xhtml(Some("bottom"), "<p>c2</p>");
        let toc = br#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><body>
<a href="ch1.xhtml#top">1</a>
<a href="ch2.xhtml#bottom">2</a>
<a href="ch1.xhtml#subsection">still-valid</a>
</body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", &ch1),
            ("OEBPS/ch2.xhtml", &ch2),
            ("OEBPS/toc.xhtml", toc),
        ]);
        let input = write_tmp("bodyid_edge_in.epub", &epub);
        let output = write_tmp("bodyid_edge_out.epub", b"");
        let _ = repair_epub(&input, &output).unwrap();
        let out_toc = read_epub_entry(&output, "OEBPS/toc.xhtml").unwrap();
        assert!(out_toc.contains("href=\"ch1.xhtml\""));
        assert!(out_toc.contains("href=\"ch2.xhtml\""));
        // Non-matching fragment must be left alone
        assert!(out_toc.contains("ch1.xhtml#subsection"));
    }

    #[test]
    fn has_xml_declaration_recognises_single_quotes() {
        assert!(has_xml_declaration(
            "<?xml version='1.0' encoding='utf-8'?>"
        ));
    }

    #[test]
    fn has_xml_declaration_rejects_missing_encoding() {
        // The reference regex requires both version and encoding.
        assert!(!has_xml_declaration("<?xml version=\"1.0\"?>"));
    }

    #[test]
    fn simplify_language_drops_region() {
        assert_eq!(simplify_language("en-US"), "en");
        assert_eq!(simplify_language("zh-Hant"), "zh");
        assert_eq!(simplify_language("EN"), "en");
    }

    #[test]
    fn extract_attr_handles_single_quotes() {
        assert_eq!(
            extract_attr("<img src='foo.png'/>", "src"),
            Some("foo.png".to_string())
        );
    }

    #[test]
    fn find_body_id_ignores_other_tags() {
        let html = r#"<html><head><title id="x">T</title></head><body id="body1"><p>x</p></body></html>"#;
        assert_eq!(find_body_id(html), Some("body1".to_string()));
    }

    #[test]
    fn find_body_id_returns_none_without_id() {
        let html = r#"<html><body><p>x</p></body></html>"#;
        assert_eq!(find_body_id(html), None);
    }

    #[test]
    fn non_utf8_binary_entry_is_preserved_as_binary() {
        // A .xml file that is actually UTF-16 bytes should not be treated
        // as text. We just want to confirm we do not crash and preserve it.
        let bad_bytes: &[u8] = &[0xff, 0xfe, b'<', 0, b'x', 0];
        let ch1 = br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let epub = build_epub(&[
            minimal_container(),
            ("OEBPS/content.opf", &minimal_opf(true)),
            ("OEBPS/ch1.xhtml", ch1),
            ("OEBPS/weird.xml", bad_bytes),
        ]);
        let input = write_tmp("nonutf8_in.epub", &epub);
        let output = write_tmp("nonutf8_out.epub", b"");
        let _ = repair_epub(&input, &output).unwrap();
        let out_bytes = {
            let bytes = std::fs::read(&output).unwrap();
            let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut e = a.by_name("OEBPS/weird.xml").unwrap();
            let mut v = Vec::new();
            e.read_to_end(&mut v).unwrap();
            v
        };
        assert_eq!(out_bytes, bad_bytes);
    }
}
