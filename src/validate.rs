/// Manuscript validator for the Amazon Kindle Publishing Guidelines.
///
/// Runs a set of checks against an OPF manifest plus its referenced content
/// files and produces a `ValidationReport` with findings at `Info`, `Warning`
/// or `Error` severity. The checks correspond to numbered sections of the
/// Amazon Kindle Publishing Guidelines (version 2026.1).
///
/// This is intended to be a pre-flight sanity check before running
/// `kindling build`: it catches the most common authoring mistakes (missing
/// cover image, nested <p> tags, unsupported tags, oversized images, duplicate
/// HTML cover pages, etc.) without needing to actually build a MOBI.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::opf::OPFData;

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Informational note. Does not affect pass/fail.
    Info,
    /// Something that should probably be fixed but is not a hard failure.
    Warning,
    /// Something that will almost certainly fail conversion or render badly.
    Error,
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Level::Info => write!(f, "info"),
            Level::Warning => write!(f, "warning"),
            Level::Error => write!(f, "error"),
        }
    }
}

/// A single finding from a validation check.
#[derive(Debug, Clone)]
pub struct Finding {
    pub level: Level,
    /// Kindle Publishing Guidelines section identifier, e.g. "4.2" or "6.4".
    pub section: String,
    pub message: String,
    /// Optional source file this finding is about (relative or absolute).
    pub file: Option<PathBuf>,
    /// Optional 1-based line number within `file`.
    pub line: Option<usize>,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] section {}: {}", self.level, self.section, self.message)?;
        if let Some(ref file) = self.file {
            write!(f, " ({}", file.display())?;
            if let Some(line) = self.line {
                write!(f, ":{}", line)?;
            }
            write!(f, ")")?;
        }
        Ok(())
    }
}

/// Full report from `validate_opf`.
#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    pub findings: Vec<Finding>,
}

impl ValidationReport {
    pub fn new() -> Self {
        ValidationReport { findings: Vec::new() }
    }

    pub fn push(&mut self, f: Finding) {
        self.findings.push(f);
    }

    pub fn info(&mut self, section: &str, message: impl Into<String>) {
        self.push(Finding {
            level: Level::Info,
            section: section.to_string(),
            message: message.into(),
            file: None,
            line: None,
        });
    }

    pub fn warning(&mut self, section: &str, message: impl Into<String>) {
        self.push(Finding {
            level: Level::Warning,
            section: section.to_string(),
            message: message.into(),
            file: None,
            line: None,
        });
    }

    pub fn error(&mut self, section: &str, message: impl Into<String>) {
        self.push(Finding {
            level: Level::Error,
            section: section.to_string(),
            message: message.into(),
            file: None,
            line: None,
        });
    }

    pub fn at(
        &mut self,
        level: Level,
        section: &str,
        message: impl Into<String>,
        file: Option<PathBuf>,
        line: Option<usize>,
    ) {
        self.push(Finding {
            level,
            section: section.to_string(),
            message: message.into(),
            file,
            line,
        });
    }

    pub fn error_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Error).count()
    }

    pub fn warning_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Warning).count()
    }

    pub fn info_count(&self) -> usize {
        self.findings.iter().filter(|f| f.level == Level::Info).count()
    }
}

/// Run all validation checks against the OPF at `opf_path` and its referenced
/// content files. Returns a `ValidationReport` collecting every finding.
///
/// Returns `Err` only if the OPF itself cannot be parsed (the normal flow is
/// to always succeed and return a possibly-empty or error-rich report).
pub fn validate_opf(opf_path: &Path) -> Result<ValidationReport, Box<dyn std::error::Error>> {
    let opf = OPFData::parse(opf_path)?;
    let mut report = ValidationReport::new();

    // Informational note about marketing cover image (cannot be validated
    // from the EPUB source per section 4.1).
    report.info(
        "4.1",
        "Marketing cover image is uploaded separately to KDP and cannot be \
         validated from the manuscript. Ensure you upload a 2560x1600 JPEG \
         per Kindle Publishing Guidelines.",
    );

    check_internal_cover(&opf, &mut report);
    check_navigation(opf_path, &opf, &mut report);
    check_content_files(&opf, &mut report);
    check_image_files(&opf, &mut report);
    check_file_case_matches(&opf, &mut report);

    Ok(report)
}

// ---------------------------------------------------------------------------
// Section 4.2: Internal Content Cover Image
// ---------------------------------------------------------------------------

/// Section 4.2: cover image must be declared with Method 1 (properties=
/// "coverimage") or Method 2 (meta name="cover"), and the book must NOT also
/// have an HTML cover page in the spine.
fn check_internal_cover(opf: &OPFData, report: &mut ValidationReport) {
    let cover_href = opf.get_cover_image_href();
    if cover_href.is_none() {
        report.error(
            "4.2",
            "No internal content cover image declared. Add either \
             <item properties=\"coverimage\" ...> (Method 1, preferred) or \
             <meta name=\"cover\" content=\"<id>\"/> (Method 2) to the OPF.",
        );
        return;
    }

    let cover_href = cover_href.unwrap();

    // Check cover image exists on disk and validate its size.
    let cover_path = opf.base_dir.join(&cover_href);
    if !cover_path.exists() {
        report.push(Finding {
            level: Level::Error,
            section: "4.2".to_string(),
            message: format!(
                "Cover image {} declared in OPF but file does not exist",
                cover_href
            ),
            file: Some(PathBuf::from(&cover_href)),
            line: None,
        });
    } else {
        // Amazon section 4.1: "Covers with less than 500 pixels on the
        // shortest side are not displayed on the website." Warn at <500px.
        if let Ok((w, h)) = image::image_dimensions(&cover_path) {
            let shortest = w.min(h);
            if shortest < 500 {
                report.push(Finding {
                    level: Level::Warning,
                    section: "4.2".to_string(),
                    message: format!(
                        "Cover image is {}x{} px; shortest side {} < 500 px. \
                         Kindle will not display covers under 500 px on the \
                         shortest side.",
                        w, h, shortest
                    ),
                    file: Some(PathBuf::from(&cover_href)),
                    line: None,
                });
            }
        }
    }

    // Section 4.2: "Do not add an HTML cover page to the content in addition
    // to the cover image. This may result in the cover appearing twice in the
    // book or cause the book to fail conversion." Detect this by looking for
    // a spine entry whose id/href contains "cover" or which is an HTML file
    // that only contains a single <img> pointing at the cover image.
    for (idref, href) in &opf.spine_items {
        if looks_like_html_cover_page(opf, idref, href, &cover_href) {
            report.push(Finding {
                level: Level::Error,
                section: "4.2".to_string(),
                message: format!(
                    "Spine entry '{}' ({}) looks like an HTML cover page but \
                     a cover image is also declared. Per KPG 4.2, do not add \
                     an HTML cover page in addition to the cover image; \
                     remove the HTML page from the spine.",
                    idref, href
                ),
                file: Some(PathBuf::from(href)),
                line: None,
            });
        }
    }
}

/// True if the spine item looks like an HTML cover page (file named cover.*
/// or id containing "cover" with a body that is essentially a single image).
fn looks_like_html_cover_page(
    opf: &OPFData,
    idref: &str,
    href: &str,
    cover_image_href: &str,
) -> bool {
    let href_lower = href.to_lowercase();
    let idref_lower = idref.to_lowercase();
    let file_stem = Path::new(&href_lower)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Must be HTML
    let is_html = href_lower.ends_with(".html")
        || href_lower.ends_with(".xhtml")
        || href_lower.ends_with(".htm");
    if !is_html {
        return false;
    }

    // Heuristic: id or filename contains "cover".
    let name_match = idref_lower.contains("cover") || file_stem.contains("cover");
    if !name_match {
        return false;
    }

    // Confirm by reading: does the body contain an <img> referencing the
    // cover image? (Avoids flagging unrelated files that just happen to be
    // named "cover".)
    let full_path = opf.base_dir.join(href);
    let content = match fs::read_to_string(&full_path) {
        Ok(s) => s,
        Err(_) => return true, // can't read - still looks like one, flag it
    };

    let image_stem = Path::new(cover_image_href)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    content.contains("<img") && (image_stem.is_empty() || content.contains(image_stem))
}

// ---------------------------------------------------------------------------
// Section 5: Navigation (TOC, NCX)
// ---------------------------------------------------------------------------

/// Section 5: recommend a TOC for any book > 20 pages of content. Section 5.2:
/// NCX file should be declared in the manifest and referenced via `spine
/// toc="..."`.
fn check_navigation(opf_path: &Path, opf: &OPFData, report: &mut ValidationReport) {
    // NCX: look for manifest item with media-type application/x-dtbncx+xml.
    let ncx_id: Option<String> = opf
        .manifest
        .iter()
        .find(|(_, (_, mt))| mt == "application/x-dtbncx+xml")
        .map(|(id, _)| id.clone());

    if ncx_id.is_none() {
        report.warning(
            "5.2",
            "No NCX file found in manifest (media-type \
             application/x-dtbncx+xml). Amazon requires a logical TOC via \
             an NCX or a toc nav element for all Kindle books.",
        );
    }

    // Check spine toc attribute: re-read the OPF to look for <spine toc="...">.
    // OPFData doesn't currently capture this, but we can grep the raw text.
    if let Ok(raw) = fs::read_to_string(opf_path) {
        let has_spine_toc = raw.lines().any(|l| {
            let lt = l.trim();
            lt.contains("<spine") && lt.contains("toc=")
        });
        if let Some(ref id) = ncx_id {
            if !has_spine_toc {
                report.warning(
                    "5.2",
                    format!(
                        "NCX '{}' is declared in manifest but <spine> element \
                         has no toc=\"<id>\" attribute. KPG 5.2 requires \
                         referencing the NCX from the spine.",
                        id
                    ),
                );
            }
        }
    }

    // Estimate content length. Warn only when we can open all spine HTML.
    let mut total_chars: usize = 0;
    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        if let Ok(content) = fs::read_to_string(&full) {
            total_chars += strip_tags_len(&content);
        }
    }
    // Rough: 1800 chars per printed page. > 20 pages ~ 36000 chars.
    let approx_pages = total_chars / 1800;
    if approx_pages > 20 && ncx_id.is_none() {
        report.warning(
            "5",
            format!(
                "Book is approximately {} pages long (> 20) but has no TOC. \
                 KPG 5 strongly recommends a logical TOC for books over 20 \
                 pages.",
                approx_pages
            ),
        );
    }
}

/// Rough approximation of text length, stripping HTML tags.
fn strip_tags_len(html: &str) -> usize {
    let mut in_tag = false;
    let mut count = 0usize;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => count += 1,
            _ => {}
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Section 6: HTML and CSS guidelines, 10.3.1, 10.5.1, 17: supported tags
// ---------------------------------------------------------------------------

/// Set of supported HTML tags per KPG section 18.1 / 17. This is conservative:
/// we flag tags that are explicitly called out as unsupported (forms, frames,
/// scripting).
const UNSUPPORTED_TAGS: &[&str] = &[
    "script",
    "form",
    "input",
    "button",
    "select",
    "textarea",
    "fieldset",
    "legend",
    "frame",
    "frameset",
    "iframe",
    "noframes",
    "applet",
    "embed",
    "object",
    "canvas",
];

fn check_content_files(opf: &OPFData, report: &mut ValidationReport) {
    for (_, href) in &opf.spine_items {
        let full = opf.base_dir.join(href);
        let content = match fs::read_to_string(&full) {
            Ok(c) => c,
            Err(_) => continue,
        };
        check_content_html(href, &content, report);
    }
}

fn check_content_html(href: &str, content: &str, report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    // 6.1 Well-formed XHTML: do a quick parseability probe via quick-xml.
    // Many OPF content files are HTML5 and not strictly XML, so only report
    // a warning, not an error. We also skip anything with a <!DOCTYPE html>
    // (HTML5) since those legitimately may not be strict XML.
    if !content.contains("<!DOCTYPE html>") && !content.contains("<!doctype html>") {
        if let Err(e) = try_parse_xml(content) {
            report.at(
                Level::Warning,
                "6.1",
                format!("XHTML is not well-formed: {}", e),
                file.clone(),
                None,
            );
        }
    }

    // 6.3 Avoid scripting: any <script> tag is an error.
    for (line_no, line) in content.lines().enumerate() {
        if contains_tag(line, "script") {
            report.at(
                Level::Error,
                "6.3",
                "<script> tag found. Scripting is not supported; scripts \
                 are stripped during conversion.",
                file.clone(),
                Some(line_no + 1),
            );
        }
    }

    // 6.4 Avoid nested <p> tags.
    if let Some(line_no) = find_nested_p(content) {
        report.at(
            Level::Error,
            "6.4",
            "Nested <p> tags found. Files with nested <p> tags do not \
             convert properly.",
            file.clone(),
            Some(line_no),
        );
    }

    // 6.2 Avoid negative values in CSS for margin/padding/line-height.
    for (line_no, line) in content.lines().enumerate() {
        if has_negative_css(line) {
            report.at(
                Level::Warning,
                "6.2",
                "Negative CSS value for margin/padding/line-height. \
                 Positioning with negative values can cause content to be \
                 cut off at screen edges.",
                file.clone(),
                Some(line_no + 1),
            );
        }
    }

    // 10.3.1 Heading alignment: warn on <h1>-<h6> with explicit text-align.
    for (line_no, line) in content.lines().enumerate() {
        if let Some(tag) = heading_with_text_align(line) {
            report.at(
                Level::Warning,
                "10.3.1",
                format!(
                    "<{}> has an explicit text-align. KPG 10.3.1 recommends \
                     letting headings use the default alignment.",
                    tag
                ),
                file.clone(),
                Some(line_no + 1),
            );
        }
    }

    // 17 / 18.1 Unsupported tags.
    for (line_no, line) in content.lines().enumerate() {
        for &tag in UNSUPPORTED_TAGS {
            if contains_tag(line, tag) {
                report.at(
                    Level::Error,
                    "17",
                    format!(
                        "Unsupported HTML tag <{}>. KPG 6.1 lists forms, \
                         frames, and JavaScript as unsupported; section 18.1 \
                         lists the allowed tags.",
                        tag
                    ),
                    file.clone(),
                    Some(line_no + 1),
                );
            }
        }
    }

    // 10.5.1 Avoid large tables (> 50 rows).
    let table_rows = count_table_rows(content);
    for (table_idx, row_count) in table_rows.iter().enumerate() {
        if *row_count > 50 {
            report.at(
                Level::Warning,
                "10.5.1",
                format!(
                    "Table #{} has {} rows (> 50). KPG 10.5.1 recommends \
                     keeping tables below 100 rows and 10 columns; large \
                     tables do not render well on small screens.",
                    table_idx + 1,
                    row_count
                ),
                file.clone(),
                None,
            );
        }
    }
}

/// Try parsing `content` as XML. Returns Err with a descriptive message if it
/// fails before EOF.
fn try_parse_xml(content: &str) -> Result<(), String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => return Ok(()),
            Err(e) => return Err(format!("{}", e)),
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// Check if a single line contains an opening tag `<name` (followed by a space
/// or `>` or `/`). Case-insensitive.
fn contains_tag(line: &str, name: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let needle_open = format!("<{}", name);
    let mut search = lower.as_str();
    while let Some(idx) = search.find(&needle_open) {
        let after = &search[idx + needle_open.len()..];
        if let Some(c) = after.chars().next() {
            if c == ' ' || c == '>' || c == '/' || c == '\t' || c == '\n' {
                return true;
            }
        } else {
            return true;
        }
        search = &search[idx + needle_open.len()..];
    }
    false
}

/// Find nested <p> tags: a <p> after another <p> with no </p> in between.
/// Returns the 1-based line number of the offending tag if any.
fn find_nested_p(content: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut pos = 0usize;
    let lower = content.to_ascii_lowercase();

    while pos < lower.len() {
        let rest = &lower[pos..];
        let open = rest.find("<p");
        let close = rest.find("</p>");
        match (open, close) {
            (Some(o), Some(c)) if o < c => {
                // Must be a real <p> (followed by space, > or /)
                let after_p = rest.as_bytes().get(o + 2).copied().unwrap_or(b' ');
                if after_p == b' ' || after_p == b'>' || after_p == b'\t' || after_p == b'/' {
                    if depth >= 1 {
                        let abs = pos + o;
                        return Some(line_of(content, abs));
                    }
                    depth += 1;
                }
                pos += o + 2;
            }
            (Some(o), None) => {
                let after_p = rest.as_bytes().get(o + 2).copied().unwrap_or(b' ');
                if after_p == b' ' || after_p == b'>' || after_p == b'\t' || after_p == b'/' {
                    if depth >= 1 {
                        let abs = pos + o;
                        return Some(line_of(content, abs));
                    }
                    depth += 1;
                }
                pos += o + 2;
            }
            (_, Some(c)) => {
                if depth > 0 {
                    depth -= 1;
                }
                pos += c + 4;
            }
            (None, None) => break,
        }
    }

    None
}

/// 1-based line number of a byte offset within `content`.
fn line_of(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

/// Detects common negative CSS values for margin/padding/line-height.
///
/// Matches patterns like `margin: -1em`, `margin-left:-5px`,
/// `padding-top: -2%`, `line-height: -1.2`, both inside `style=` attributes
/// and in `<style>` blocks.
fn has_negative_css(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    for prop in &["margin", "padding", "line-height"] {
        let mut search = l.as_str();
        while let Some(idx) = search.find(prop) {
            let after = &search[idx + prop.len()..];
            // Accept "margin:", "margin-left:", etc.
            let colon = after.find(':');
            if let Some(c) = colon {
                let value = after[c + 1..].trim_start();
                // Match "-" followed by digit (skip "--" which is CSS var)
                let bytes = value.as_bytes();
                if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1].is_ascii_digit() {
                    return true;
                }
                // "margin: 0 -5px" style: scan up to ; or "
                let end = value
                    .find(|c: char| c == ';' || c == '"' || c == '}')
                    .unwrap_or(value.len());
                let vals = &value[..end];
                let chars: Vec<char> = vals.chars().collect();
                for i in 0..chars.len().saturating_sub(1) {
                    if chars[i] == '-' && chars[i + 1].is_ascii_digit() {
                        // Don't match e.g. "calc(10 - 5)": require preceding
                        // whitespace or start of value.
                        let prev = if i == 0 { ' ' } else { chars[i - 1] };
                        if prev == ' ' || prev == ':' || prev == '\t' {
                            return true;
                        }
                    }
                }
            }
            search = &search[idx + prop.len()..];
        }
    }
    false
}

/// Returns the heading tag name (e.g. "h1") if `line` contains a heading tag
/// with an explicit text-align.
fn heading_with_text_align(line: &str) -> Option<&'static str> {
    let l = line.to_ascii_lowercase();
    let tags: &[&'static str] = &["h1", "h2", "h3", "h4", "h5", "h6"];
    for tag in tags {
        let open = format!("<{}", tag);
        if let Some(idx) = l.find(&open) {
            // Find the end of the opening tag
            if let Some(end) = l[idx..].find('>') {
                let tag_content = &l[idx..idx + end];
                if tag_content.contains("text-align") {
                    return Some(tag);
                }
            }
        }
    }
    None
}

/// Count number of <tr> rows per <table> in `content`. Returns a vector with
/// one entry per table.
fn count_table_rows(content: &str) -> Vec<usize> {
    let lower = content.to_ascii_lowercase();
    let mut counts = Vec::new();
    let mut pos = 0usize;
    while let Some(start) = lower[pos..].find("<table") {
        let abs = pos + start;
        let end = match lower[abs..].find("</table>") {
            Some(e) => abs + e,
            None => break,
        };
        let table = &lower[abs..end];
        let row_count = table.matches("<tr").count();
        counts.push(row_count);
        pos = end + 8;
    }
    counts
}

// ---------------------------------------------------------------------------
// Section 10.4.1, 10.4.2: image formats and sizes
// ---------------------------------------------------------------------------

const SUPPORTED_IMAGE_MEDIA: &[&str] = &[
    "image/jpeg",
    "image/jpg",
    "image/png",
    "image/gif",
    "image/svg+xml",
];

fn check_image_files(opf: &OPFData, report: &mut ValidationReport) {
    // Iterate all image manifest items (ordered by id for determinism).
    let mut items: Vec<(String, String, String)> = opf
        .manifest
        .iter()
        .filter(|(_, (_, mt))| mt.starts_with("image/"))
        .map(|(id, (href, mt))| (id.clone(), href.clone(), mt.clone()))
        .collect();
    items.sort_by(|a, b| a.0.cmp(&b.0));

    for (_id, href, media_type) in &items {
        let path = opf.base_dir.join(href);

        // 10.4.1 Supported formats.
        if !SUPPORTED_IMAGE_MEDIA.contains(&media_type.as_str()) {
            report.push(Finding {
                level: Level::Error,
                section: "10.4.1".to_string(),
                message: format!(
                    "Image {} has unsupported media-type '{}'. KPG 10.4.1 \
                     supports JPEG, PNG, GIF, and SVG only.",
                    href, media_type
                ),
                file: Some(PathBuf::from(href)),
                line: None,
            });
        }

        if !path.exists() {
            report.push(Finding {
                level: Level::Error,
                section: "10.4.1".to_string(),
                message: format!("Image {} referenced in manifest but file is missing", href),
                file: Some(PathBuf::from(href)),
                line: None,
            });
            continue;
        }

        // 10.4.2 Image size (file size).
        if let Ok(md) = fs::metadata(&path) {
            // 127 KB = 130048 bytes.
            if md.len() > 127 * 1024 {
                report.push(Finding {
                    level: Level::Warning,
                    section: "10.4.2".to_string(),
                    message: format!(
                        "Image {} is {} bytes (> 127 KB). Older Kindle models \
                         may not render images larger than 127 KB; modern \
                         Kindles handle larger files.",
                        href,
                        md.len()
                    ),
                    file: Some(PathBuf::from(href)),
                    line: None,
                });
            }
        }

        // 10.4.2 Image dimensions (megapixels).
        if media_type != "image/svg+xml" {
            if let Ok((w, h)) = image::image_dimensions(&path) {
                let mp = (w as u64) * (h as u64);
                if mp > 5_000_000 {
                    report.push(Finding {
                        level: Level::Warning,
                        section: "10.4.2".to_string(),
                        message: format!(
                            "Image {} is {}x{} ({} MP). KPG 10.4.2 limits \
                             combined dimensions to 5 megapixels.",
                            href,
                            w,
                            h,
                            mp / 1_000_000
                        ),
                        file: Some(PathBuf::from(href)),
                        line: None,
                    });
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Section 6.5: file references must match case and spelling of source
// ---------------------------------------------------------------------------

/// On case-sensitive filesystems a wrong-case reference will already fail to
/// open. On case-insensitive filesystems (macOS default, Windows) we can
/// still detect a mismatch by comparing the manifest href against the actual
/// filename returned by the directory listing.
fn check_file_case_matches(opf: &OPFData, report: &mut ValidationReport) {
    for (_id, (href, _mt)) in &opf.manifest {
        let rel = Path::new(href);
        let parent = opf.base_dir.join(rel.parent().unwrap_or(Path::new("")));
        let file_name = match rel.file_name().and_then(|s| s.to_str()) {
            Some(n) => n,
            None => continue,
        };

        let entries = match fs::read_dir(&parent) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let mut case_insensitive_match: Option<String> = None;
        let mut exact_match = false;
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name == file_name {
                    exact_match = true;
                    break;
                }
                if name.eq_ignore_ascii_case(file_name) {
                    case_insensitive_match = Some(name.to_string());
                }
            }
        }

        if !exact_match {
            if let Some(actual) = case_insensitive_match {
                report.push(Finding {
                    level: Level::Error,
                    section: "6.5".to_string(),
                    message: format!(
                        "Manifest references '{}' but actual file on disk is \
                         '{}'. KPG 6.5 requires file references to match the \
                         case and spelling of the source file exactly.",
                        file_name, actual
                    ),
                    file: Some(PathBuf::from(href)),
                    line: None,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests (unit-level helpers)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn nested_p_detection_positive() {
        let html = "<p>outer <p>inner</p></p>";
        assert!(find_nested_p(html).is_some());
    }

    #[test]
    fn nested_p_detection_negative() {
        let html = "<p>a</p><p>b</p><p>c</p>";
        assert!(find_nested_p(html).is_none());
    }

    #[test]
    fn contains_tag_matches_word_boundary() {
        assert!(contains_tag("<script>", "script"));
        assert!(contains_tag("<script src=\"x\">", "script"));
        assert!(contains_tag("<script/>", "script"));
        assert!(!contains_tag("<scripting>", "script"));
    }

    #[test]
    fn negative_css_detection() {
        assert!(has_negative_css("margin: -1em;"));
        assert!(has_negative_css("margin-left: -5px;"));
        assert!(has_negative_css("padding:-2%"));
        assert!(has_negative_css("line-height: -1.2"));
        assert!(has_negative_css("margin: 0 -5px 0 0;"));
        assert!(!has_negative_css("margin: 1em;"));
        assert!(!has_negative_css("color: #000;"));
    }

    #[test]
    fn heading_with_text_align_detection() {
        assert_eq!(
            heading_with_text_align("<h1 style=\"text-align:center\">Hi</h1>"),
            Some("h1")
        );
        assert_eq!(heading_with_text_align("<h3>Hi</h3>"), None);
    }

    #[test]
    fn count_table_rows_basic() {
        let html = "<table><tr><td>1</td></tr><tr><td>2</td></tr></table>";
        assert_eq!(count_table_rows(html), vec![2]);
    }
}
