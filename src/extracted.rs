// Extracted EPUB content with lazy HTML/CSS caches.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lightningcss::stylesheet::{ParserOptions, StyleSheet};

use crate::epub;
use crate::opf::OPFData;
use crate::profile::Profile;

// ---------------------------------------------------------------------------
// CssSummary: lifetime-free digest of a parsed CSS stylesheet
// ---------------------------------------------------------------------------

/// A lightweight, owned summary of a parsed CSS file. This avoids the lifetime
/// issues with storing `lightningcss::stylesheet::StyleSheet<'i, 'o>` in a
/// cache: we parse once, extract everything the check modules need, and discard
/// the AST.
#[derive(Clone, Debug)]
pub struct CssSummary {
    /// `None` if lightningcss parsed the file successfully; `Some(msg)` with
    /// the error description if parsing failed.
    pub parse_error: Option<String>,

    /// `@import` targets: `(line, url)`.
    pub imports: Vec<(usize, String)>,

    /// `url()` references (excluding those inside `@import` / `@namespace`
    /// preludes and `data:` URIs): `(line, url)`.
    pub url_refs: Vec<(usize, String)>,

    /// `@font-face` blocks. Each entry holds the 1-based line of the rule and
    /// the `src` URLs declared inside the block (empty vec when the block has
    /// no `src` descriptor at all).
    pub font_faces: Vec<CssFontFace>,

    /// 1-based line numbers of `@namespace` rules.
    pub namespace_lines: Vec<usize>,

    /// Unsupported `@media` feature names: `(line, feature)`.
    pub media_features: Vec<(usize, String)>,

    /// Property names that appear in declarations (lowercased). Useful for
    /// R6.14-style forbidden-property checks.
    pub property_names: HashSet<String>,

    /// Forbidden `position` values: `(line, value)`.
    pub forbidden_positions: Vec<(usize, String)>,
}

/// A single `@font-face` block extracted from a stylesheet.
#[derive(Clone, Debug)]
pub struct CssFontFace {
    /// 1-based line of the `@font-face` rule.
    pub line: usize,
    /// URLs from the `src` descriptor. Empty if `src` is absent entirely.
    pub src_urls: Vec<(usize, String)>,
    /// True when the block has no `src:` descriptor at all.
    pub missing_src: bool,
}

/// A ready-to-inspect EPUB: OPF parsed, content root set, parser caches ready.
pub struct ExtractedEpub {
    pub root: PathBuf,
    pub opf_path: PathBuf,
    pub opf: OPFData,
    pub profile: Profile,

    // Optional tempdir cleanup guard for from_epub_path callers.
    _temp_dir: Option<PathBuf>,

    raw_cache: RefCell<HashMap<String, Option<String>>>,
    html_cache: RefCell<HashMap<String, scraper::Html>>,
    css_text_cache: RefCell<HashMap<String, String>>,
    css_summary_cache: RefCell<HashMap<String, CssSummary>>,
    ids_cache: RefCell<HashMap<String, HashSet<String>>>,
}

impl ExtractedEpub {
    /// Build from an already-extracted OPF on disk.
    pub fn from_opf_path(opf_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let opf = OPFData::parse(opf_path)?;
        let root = opf.base_dir.clone();
        let profile = Profile::autodetect(&opf, &root);
        Ok(Self {
            root,
            opf_path: opf_path.to_path_buf(),
            opf,
            profile,
            _temp_dir: None,
            raw_cache: RefCell::new(HashMap::new()),
            html_cache: RefCell::new(HashMap::new()),
            css_text_cache: RefCell::new(HashMap::new()),
            css_summary_cache: RefCell::new(HashMap::new()),
            ids_cache: RefCell::new(HashMap::new()),
        })
    }

    /// Unzip an EPUB into a temp directory and build from there.
    #[allow(dead_code)]
    pub fn from_epub_path(epub_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let (temp_dir, opf_path) = epub::extract_epub(epub_path)?;
        let mut me = Self::from_opf_path(&opf_path)?;
        me._temp_dir = Some(temp_dir);
        Ok(me)
    }

    /// Raw file text relative to the content root. Caches a miss so we do not
    /// re-read failed paths.
    pub fn read(&self, href: &str) -> Option<String> {
        if let Some(cached) = self.raw_cache.borrow().get(href) {
            return cached.clone();
        }
        let path = self.root.join(href);
        let content = fs::read_to_string(&path).ok();
        self.raw_cache.borrow_mut().insert(href.to_string(), content.clone());
        content
    }

    /// Parsed HTML for `href`, if the file exists and is readable.
    #[allow(dead_code)]
    pub fn html(&self, href: &str) -> Option<Ref<'_, scraper::Html>> {
        if !self.html_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            let doc = scraper::Html::parse_document(&text);
            self.html_cache.borrow_mut().insert(href.to_string(), doc);
        }
        Some(Ref::map(self.html_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Raw CSS text for `href`. Callers that only need text scanning can use
    /// this directly; callers that need parsed information should prefer
    /// `css_summary()`.
    #[allow(dead_code)]
    pub fn css(&self, href: &str) -> Option<Ref<'_, String>> {
        if !self.css_text_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            self.css_text_cache.borrow_mut().insert(href.to_string(), text);
        }
        Some(Ref::map(self.css_text_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Parsed CSS summary for `href`. Returns `None` only when the file cannot
    /// be read at all. Parse errors are captured inside `CssSummary::parse_error`
    /// so callers always get a summary even for broken CSS.
    #[allow(dead_code)]
    pub fn css_summary(&self, href: &str) -> Option<Ref<'_, CssSummary>> {
        if !self.css_summary_cache.borrow().contains_key(href) {
            // Ensure the text is in the css_text_cache first.
            let text = {
                if !self.css_text_cache.borrow().contains_key(href) {
                    let raw = self.read(href)?;
                    self.css_text_cache.borrow_mut().insert(href.to_string(), raw);
                }
                let cache = self.css_text_cache.borrow();
                cache.get(href).unwrap().clone()
            };
            let summary = build_css_summary(&text);
            self.css_summary_cache
                .borrow_mut()
                .insert(href.to_string(), summary);
        }
        Some(Ref::map(self.css_summary_cache.borrow(), |m| {
            m.get(href).unwrap()
        }))
    }

    /// Set of `id` attribute values found in `href`, for link-target checks.
    #[allow(dead_code)]
    pub fn ids(&self, href: &str) -> Option<Ref<'_, HashSet<String>>> {
        if !self.ids_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            let set = collect_ids(&text);
            self.ids_cache.borrow_mut().insert(href.to_string(), set);
        }
        Some(Ref::map(self.ids_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Every manifest href, fragment stripped.
    #[allow(dead_code)]
    pub fn manifest_hrefs(&self) -> HashSet<String> {
        self.opf
            .manifest
            .values()
            .map(|(href, _)| match href.find('#') {
                Some(i) => href[..i].to_string(),
                None => href.clone(),
            })
            .collect()
    }
}

impl Drop for ExtractedEpub {
    fn drop(&mut self) {
        if let Some(ref dir) = self._temp_dir {
            if dir.exists() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CssSummary builder: text-scan extraction (mirrors css_forbidden.rs helpers)
// ---------------------------------------------------------------------------

/// Build a `CssSummary` from raw CSS text. Strips a leading BOM and @charset
/// before parsing so lightningcss gets a clean input.
fn build_css_summary(raw_text: &str) -> CssSummary {
    let text = strip_css_prologue(raw_text);

    // R6.13: try parsing with lightningcss.
    let parse_error = match StyleSheet::parse(text, ParserOptions::default()) {
        Ok(_) => None,
        Err(e) => Some(format!("{}", e)),
    };

    let imports = find_imports(text);
    let url_refs = find_url_refs(text);
    let font_faces = find_font_faces(text);
    let namespace_lines = find_namespace_lines(text);
    let media_features = find_media_features(text);
    let property_names = find_property_names(text);
    let forbidden_positions = find_forbidden_positions(text);

    CssSummary {
        parse_error,
        imports,
        url_refs,
        font_faces,
        namespace_lines,
        media_features,
        property_names,
        forbidden_positions,
    }
}

/// Strip a UTF-8 BOM and a leading `@charset "..."` rule.
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

// -- @import extraction -----------------------------------------------------

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

// -- url() extraction -------------------------------------------------------

fn find_url_refs(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("url(") {
        let abs = pos + idx;
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

fn belongs_to_namespace(before: &str) -> bool {
    if before.ends_with("@namespace") {
        return true;
    }
    let after_ident = before.trim_end_matches(|c: char| {
        c.is_ascii_alphanumeric() || c == '-' || c == '_'
    });
    if after_ident.len() == before.len() {
        return false;
    }
    let after_space = after_ident.trim_end();
    after_space.ends_with("@namespace")
}

// -- @font-face extraction --------------------------------------------------

fn find_font_faces(text: &str) -> Vec<CssFontFace> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("@font-face") {
        let abs = pos + idx;
        let after = abs + "@font-face".len();
        let Some(open_rel) = text[after..].find('{') else { break };
        let open = after + open_rel;
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
        let missing_src = !block_lower.contains("src:");

        // Collect src url() targets inside the block.
        let block_urls: Vec<(usize, String)> = find_url_refs(block)
            .into_iter()
            .map(|(rel_line, url)| {
                let abs_line = line_of(text, open + 1) + rel_line - 1;
                (abs_line, url)
            })
            .collect();

        out.push(CssFontFace {
            line: line_of(text, abs),
            src_urls: block_urls,
            missing_src,
        });
        pos = close + 1;
    }
    out
}

// -- @namespace extraction --------------------------------------------------

fn find_namespace_lines(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    for (abs, _) in iter_ascii_matches(&lower, "@namespace") {
        if let Some(prev) = lower[..abs].chars().last() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                continue;
            }
        }
        out.push(line_of(text, abs));
    }
    out
}

// -- @media feature extraction ----------------------------------------------

const UNSUPPORTED_MEDIA_FEATURES: &[&str] = &[
    "hover",
    "any-hover",
    "pointer",
    "any-pointer",
    "color-gamut",
    "prefers-color-scheme",
];

fn find_media_features(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(idx) = lower[pos..].find("@media") {
        let abs = pos + idx;
        if let Some(prev) = lower[..abs].chars().last() {
            if prev.is_ascii_alphanumeric() || prev == '-' || prev == '_' {
                pos = abs + "@media".len();
                continue;
            }
        }
        let after = abs + "@media".len();
        let Some(brace_rel) = text[after..].find('{') else { break };
        let prelude = &lower[after..after + brace_rel];
        let prelude_orig_start = after;
        for feat in UNSUPPORTED_MEDIA_FEATURES {
            if let Some(f_idx) = prelude.find(feat) {
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

// -- property name extraction -----------------------------------------------

/// Collect lowercased property names from declarations. This is a best-effort
/// text scan: it looks for `identifier:` patterns outside comments and
/// at-rule preludes.
fn find_property_names(text: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    while i < len {
        // Skip comments.
        if i + 1 < len && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            if let Some(end) = lower[i + 2..].find("*/") {
                i = i + 2 + end + 2;
            } else {
                break;
            }
            continue;
        }

        // Look for `property-name:` (possibly with whitespace before the colon).
        if bytes[i] == b':' {
            // Walk backwards to find the property name.
            let colon = i;
            let mut j = colon;
            // Skip whitespace before colon.
            while j > 0 && bytes[j - 1].is_ascii_whitespace() {
                j -= 1;
            }
            let name_end = j;
            // Collect identifier characters [a-z0-9-_].
            while j > 0
                && (bytes[j - 1].is_ascii_alphanumeric()
                    || bytes[j - 1] == b'-'
                    || bytes[j - 1] == b'_')
            {
                j -= 1;
            }
            if j < name_end {
                // Reject if the character before the name is alphanumeric (part
                // of a longer token) or if the name starts with `--` (custom
                // property).
                let name = &lower[j..name_end];
                let boundary_ok = j == 0
                    || (!bytes[j - 1].is_ascii_alphanumeric() && bytes[j - 1] != b'_');
                if boundary_ok && !name.starts_with("--") {
                    out.insert(name.to_string());
                }
            }
        }
        i += 1;
    }
    out
}

// -- forbidden position extraction ------------------------------------------

const FORBIDDEN_POSITION_VALUES: &[&str] = &["fixed", "absolute", "sticky"];

fn find_forbidden_positions(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lower = text.to_ascii_lowercase();
    for (abs, matched) in iter_ascii_matches(&lower, "position") {
        let after = abs + matched.len();
        let rest = &lower[after..];
        let rest_trim = rest.trim_start();
        if !rest_trim.starts_with(':') {
            continue;
        }
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
// Shared small helpers (used by CssSummary builder)
// ---------------------------------------------------------------------------

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
        let end = trimmed
            .find(|c: char| c == ')' || c.is_whitespace())
            .unwrap_or(trimmed.len());
        return Some(trimmed[..end].to_string());
    }
    let end = rest.find(end_char)?;
    Some(rest[..end].to_string())
}

fn line_of(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

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
// HTML id collection
// ---------------------------------------------------------------------------

/// Collect every `id="..."` attribute value from an HTML string.
fn collect_ids(html: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut rest = html;
    while let Some(idx) = rest.find(" id=\"") {
        rest = &rest[idx + 5..];
        let Some(end) = rest.find('"') else { break };
        out.insert(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}
