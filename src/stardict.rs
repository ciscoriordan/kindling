//! StarDict format builder for GoldenDict / GoldenDict-ng / KOReader / sdcv.
//!
//! Reads the same OPF + idx-marked HTML that `kindling build` consumes for
//! MOBI output, but emits the four-file StarDict bundle (`.ifo` / `.idx` /
//! `.dict` / `.syn`) instead. Inflections from `<idx:iform>` become `.syn`
//! redirects; headwords from `<idx:orth>` become flat `.idx` entries; entry
//! HTML is stripped of Kindle-specific `<idx:*>` markup so it renders cleanly
//! in non-Kindle dictionary apps.
//!
//! The on-disk format follows the StarDict 2.4.2 spec:
//!
//! * `<name>.ifo` — UTF-8 key=value manifest, first line is the magic string
//!   "StarDict's dict ifo file".
//! * `<name>.idx` — concatenation of `(word\0, offset:u32be, size:u32be)`,
//!   sorted by `g_ascii_strcasecmp` (ASCII case-insensitive bytewise) so
//!   readers can binary-search.
//! * `<name>.dict` — concatenation of per-entry payloads. With
//!   `sametypesequence=h` each payload is interpreted as HTML.
//! * `<name>.syn` — optional alternate-form index;
//!   `(form\0, original_word_index:u32be)` with `original_word_index` an
//!   index into the sorted `.idx`, not a byte offset. Same sort.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::opf::{self, DictionaryEntry, OPFData};

/// Caller overrides for the metadata that lands in `.ifo`. Fields left as
/// `None` fall back to the OPF (`bookname` <- `dc:title`, `author` <-
/// `dc:creator`, `date` <- `dc:date`). `email`, `website`, and `description`
/// have no OPF counterpart and are simply omitted from the `.ifo` when
/// `None` or empty.
#[derive(Default, Debug, Clone)]
pub struct StarDictOptions {
    pub bookname: Option<String>,
    pub author: Option<String>,
    pub email: Option<String>,
    pub website: Option<String>,
    pub description: Option<String>,
    pub date: Option<String>,
}

/// What was written, for the CLI summary and tests.
#[derive(Debug)]
pub struct StarDictReport {
    pub wordcount: usize,
    pub synwordcount: usize,
    pub ifo_path: PathBuf,
    pub idx_path: PathBuf,
    pub dict_path: PathBuf,
    pub syn_path: PathBuf,
}

/// Build a StarDict bundle from an OPF dictionary into `output_dir`.
///
/// File stems are derived from the directory name, so
/// `output_dir = "lemma_greek_en"` produces `lemma_greek_en/lemma_greek_en.ifo`
/// and friends. The directory is created if missing.
pub fn build_stardict(
    opf_path: &Path,
    output_dir: &Path,
    options: &StarDictOptions,
) -> Result<StarDictReport, Box<dyn std::error::Error>> {
    let opf = OPFData::parse(opf_path)?;

    let mut entries: Vec<DictionaryEntry> = Vec::new();
    for html_path in opf.get_content_html_paths() {
        entries.extend(opf::parse_dictionary_html(&html_path)?);
    }
    if entries.is_empty() {
        return Err("No dictionary entries found in HTML content files".into());
    }
    eprintln!("Parsed {} dictionary entries", entries.len());

    fs::create_dir_all(output_dir)?;
    let stem = output_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "dict".to_string());

    let cleaned: Vec<CleanEntry> = entries
        .into_iter()
        .filter_map(|e| {
            if e.headword.is_empty() {
                return None;
            }
            let body = clean_entry_html(&e.html_content, &e.headword);
            if body.is_empty() {
                return None;
            }
            Some(CleanEntry {
                headword: e.headword,
                inflections: e.inflections,
                body: body.into_bytes(),
            })
        })
        .collect();

    if cleaned.is_empty() {
        return Err("All dictionary entries dropped during HTML cleanup".into());
    }

    // Sort by g_ascii_strcasecmp; ties keep original spine order (stable).
    let mut order: Vec<usize> = (0..cleaned.len()).collect();
    order.sort_by(|&a, &b| {
        ascii_case_cmp(cleaned[a].headword.as_bytes(), cleaned[b].headword.as_bytes())
            .then(a.cmp(&b))
    });

    // Write .dict + .idx
    let ifo_path = output_dir.join(format!("{}.ifo", stem));
    let idx_path = output_dir.join(format!("{}.idx", stem));
    let dict_path = output_dir.join(format!("{}.dict", stem));
    let syn_path = output_dir.join(format!("{}.syn", stem));

    let mut dict_buf: Vec<u8> = Vec::new();
    let mut idx_buf: Vec<u8> = Vec::new();
    // For .syn we need to look up the new (sorted) index of each headword.
    let mut headword_to_new_idx: HashMap<String, u32> = HashMap::new();
    for (new_idx, &orig_idx) in order.iter().enumerate() {
        let entry = &cleaned[orig_idx];
        let offset = u32::try_from(dict_buf.len()).map_err(|_| ".dict exceeds 4 GiB")?;
        let size = u32::try_from(entry.body.len()).map_err(|_| "entry exceeds 4 GiB")?;
        dict_buf.extend_from_slice(&entry.body);

        idx_buf.extend_from_slice(entry.headword.as_bytes());
        idx_buf.push(0);
        idx_buf.extend_from_slice(&offset.to_be_bytes());
        idx_buf.extend_from_slice(&size.to_be_bytes());

        // First occurrence (case-insensitive) wins; later duplicates fall
        // through to whichever .idx slot they land in. This matters for
        // syn lookup: we want the inflection to point at the canonical lemma.
        headword_to_new_idx
            .entry(entry.headword.to_ascii_lowercase())
            .or_insert(new_idx as u32);
    }

    let wordcount = order.len();
    let idxfilesize = idx_buf.len();

    fs::write(&dict_path, &dict_buf)?;
    fs::write(&idx_path, &idx_buf)?;

    // .syn: every (inflection, lemma_index) pair, sorted by g_ascii_strcasecmp
    // on the inflection. Skip forms that collide with an existing headword
    // (StarDict readers will hit the headword first via .idx anyway, and
    // duplicating them in .syn just wastes space and slows lookup).
    let mut syn_pairs: Vec<(String, u32)> = Vec::new();
    let mut headword_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in &cleaned {
        headword_set.insert(entry.headword.to_ascii_lowercase());
    }
    for (new_idx, &orig_idx) in order.iter().enumerate() {
        let entry = &cleaned[orig_idx];
        for form in &entry.inflections {
            if form.is_empty() {
                continue;
            }
            if headword_set.contains(&form.to_ascii_lowercase()) {
                continue;
            }
            syn_pairs.push((form.clone(), new_idx as u32));
        }
    }
    // Sort by form (g_ascii_strcasecmp), then by index for determinism.
    syn_pairs.sort_by(|a, b| {
        ascii_case_cmp(a.0.as_bytes(), b.0.as_bytes()).then(a.1.cmp(&b.1))
    });
    // Deduplicate exact (form, index) pairs that may arise when an
    // inflection is filed under multiple lemmas with the same casing.
    syn_pairs.dedup();

    let mut syn_buf: Vec<u8> = Vec::new();
    for (form, idx) in &syn_pairs {
        syn_buf.extend_from_slice(form.as_bytes());
        syn_buf.push(0);
        syn_buf.extend_from_slice(&idx.to_be_bytes());
    }
    let synwordcount = syn_pairs.len();
    if synwordcount > 0 {
        fs::write(&syn_path, &syn_buf)?;
    } else {
        // No inflections -> no .syn file. Remove a stale one if present so a
        // rebuild does not leave an old file behind.
        let _ = fs::remove_file(&syn_path);
    }

    // .ifo
    let raw_bookname = options
        .bookname
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if opf.title.is_empty() {
                None
            } else {
                Some(opf.title.clone())
            }
        })
        .unwrap_or_else(|| stem.clone());
    let bookname = augment_bookname_with_lang_pair(
        &raw_bookname,
        &opf.dict_in_language,
        &opf.dict_out_language,
    );
    let author = options
        .author
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| opf.author.clone());
    let date = options
        .date
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| opf.date.clone());
    let email = options.email.clone().unwrap_or_default();
    let website = options.website.clone().unwrap_or_default();
    let description = options.description.clone().unwrap_or_default();

    let mut ifo = String::new();
    ifo.push_str("StarDict's dict ifo file\n");
    ifo.push_str("version=2.4.2\n");
    ifo.push_str(&format!("bookname={}\n", sanitize_ifo_value(&bookname)));
    ifo.push_str(&format!("wordcount={}\n", wordcount));
    if synwordcount > 0 {
        ifo.push_str(&format!("synwordcount={}\n", synwordcount));
    }
    ifo.push_str(&format!("idxfilesize={}\n", idxfilesize));
    if !author.is_empty() {
        ifo.push_str(&format!("author={}\n", sanitize_ifo_value(&author)));
    }
    if !email.is_empty() {
        ifo.push_str(&format!("email={}\n", sanitize_ifo_value(&email)));
    }
    if !website.is_empty() {
        ifo.push_str(&format!("website={}\n", sanitize_ifo_value(&website)));
    }
    if !description.is_empty() {
        ifo.push_str(&format!(
            "description={}\n",
            sanitize_ifo_value(&description)
        ));
    }
    if !date.is_empty() {
        ifo.push_str(&format!("date={}\n", sanitize_ifo_value(&date)));
    }
    ifo.push_str("sametypesequence=h\n");
    fs::write(&ifo_path, ifo.as_bytes())?;

    Ok(StarDictReport {
        wordcount,
        synwordcount,
        ifo_path,
        idx_path,
        dict_path,
        syn_path,
    })
}

struct CleanEntry {
    headword: String,
    inflections: Vec<String>,
    body: Vec<u8>,
}

/// Strip Kindle-only `idx:*` and `mbp:*` markup from an entry's HTML so the
/// payload renders correctly in StarDict readers.
///
/// * `<idx:entry>...</idx:entry>` outer wrapper -> dropped (body kept).
/// * `<idx:orth>...</idx:orth>` body form        -> wrapper dropped, body kept.
/// * `<idx:orth value="X"/>` self-closing        -> replaced with `<b>X</b>`
///   so the headword is still visible in apps that show entries verbatim.
/// * `<idx:infl>...</idx:infl>` and `<idx:iform .../>` -> dropped (the same
///   information is exposed via `.syn`, and inline iform tags would render
///   as visible noise in a non-Kindle reader).
/// * `<idx:short>` and `<mbp:*>` tags             -> wrappers dropped.
///
/// Cross-references that target MOBI per-letter HTML files
/// (`href="content_NN.html#hw_X"` or `href="#hw_X"` for same-page links) are
/// rewritten to StarDict's [`bword://X`] scheme so GoldenDict, GoldenDict-ng,
/// and KOReader resolve them as in-dictionary lookups instead of dead
/// document links. Hrefs that do not match the `(content_\d+\.html)?#hw_…`
/// shape pass through unchanged.
///
/// If the cleaned body ends up empty (e.g. an entry whose HTML was only
/// `<idx:orth value="x"/>` with no surrounding text), we synthesise a
/// `<b>headword</b>` so the entry still has visible content.
fn clean_entry_html(html: &str, headword: &str) -> String {
    static ENTRY_OPEN: OnceLock<Regex> = OnceLock::new();
    static ENTRY_CLOSE: OnceLock<Regex> = OnceLock::new();
    static ORTH_SELF: OnceLock<Regex> = OnceLock::new();
    static ORTH_OPEN: OnceLock<Regex> = OnceLock::new();
    static ORTH_CLOSE: OnceLock<Regex> = OnceLock::new();
    static IFORM: OnceLock<Regex> = OnceLock::new();
    static INFL_BLOCK: OnceLock<Regex> = OnceLock::new();
    static SHORT_OPEN: OnceLock<Regex> = OnceLock::new();
    static SHORT_CLOSE: OnceLock<Regex> = OnceLock::new();
    static MBP_PAGEBREAK: OnceLock<Regex> = OnceLock::new();
    static MBP_FRAMESET: OnceLock<Regex> = OnceLock::new();
    static CROSSREF: OnceLock<Regex> = OnceLock::new();
    static BLANK_LINES: OnceLock<Regex> = OnceLock::new();

    let entry_open = ENTRY_OPEN.get_or_init(|| Regex::new(r"<idx:entry\b[^>]*>\s*").unwrap());
    let entry_close = ENTRY_CLOSE.get_or_init(|| Regex::new(r"\s*</idx:entry>").unwrap());
    let orth_self = ORTH_SELF.get_or_init(|| {
        Regex::new(r#"<idx:orth\b[^>]*\svalue="([^"]*)"[^>]*/>"#).unwrap()
    });
    let orth_open = ORTH_OPEN.get_or_init(|| Regex::new(r"<idx:orth\b[^>]*>").unwrap());
    let orth_close = ORTH_CLOSE.get_or_init(|| Regex::new(r"</idx:orth>").unwrap());
    let iform = IFORM.get_or_init(|| Regex::new(r"<idx:iform\b[^/]*/>\s*").unwrap());
    let infl_block = INFL_BLOCK
        .get_or_init(|| Regex::new(r"(?s)<idx:infl\b[^>]*>.*?</idx:infl>\s*").unwrap());
    let short_open = SHORT_OPEN.get_or_init(|| Regex::new(r"<idx:short\b[^>]*>").unwrap());
    let short_close = SHORT_CLOSE.get_or_init(|| Regex::new(r"</idx:short>").unwrap());
    let mbp_pagebreak = MBP_PAGEBREAK.get_or_init(|| Regex::new(r"<mbp:pagebreak\b[^>]*/?>").unwrap());
    let mbp_frameset = MBP_FRAMESET.get_or_init(|| Regex::new(r"</?mbp:frameset\b[^>]*>").unwrap());
    // Match either `href="content_NN.html#hw_X"` or `href="#hw_X"` and
    // capture the X. The `(?:content_\d+\.html)?` group makes the per-letter
    // file optional so same-file links rewrite too. Headwords in lemma's
    // output are raw UTF-8, so `[^"]+` is sufficient — no percent-decoding
    // is needed.
    let crossref = CROSSREF.get_or_init(|| {
        Regex::new(r#"href\s*=\s*"(?:content_\d+\.html)?#hw_([^"]+)""#).unwrap()
    });
    let blank_lines = BLANK_LINES.get_or_init(|| Regex::new(r"\n\s*\n").unwrap());

    let mut s = html.to_string();

    // 1) Inflections: drop entirely (Kindle-only metadata, surfaced via .syn).
    s = infl_block.replace_all(&s, "").into_owned();
    s = iform.replace_all(&s, "").into_owned();

    // 2) Outer entry wrapper: drop, keep body.
    s = entry_open.replace_all(&s, "").into_owned();
    s = entry_close.replace_all(&s, "").into_owned();

    // 3) Orth: self-closing -> bold headword; body form -> unwrap.
    s = orth_self.replace_all(&s, "<b>$1</b>").into_owned();
    s = orth_open.replace_all(&s, "").into_owned();
    s = orth_close.replace_all(&s, "").into_owned();

    // 4) Short, mbp:*: drop wrappers.
    s = short_open.replace_all(&s, "").into_owned();
    s = short_close.replace_all(&s, "").into_owned();
    s = mbp_pagebreak.replace_all(&s, "").into_owned();
    s = mbp_frameset.replace_all(&s, "").into_owned();

    // 5) Cross-references: per-letter file + hw_ fragment -> bword://word.
    //    Both inter-letter (`content_19.html#hw_τη`) and same-page (`#hw_τη`)
    //    links collapse to the same StarDict-native form, since StarDict
    //    has no concept of per-letter files.
    s = crossref.replace_all(&s, r#"href="bword://$1""#).into_owned();

    // Collapse the blank lines left behind by the above stripping so the
    // .dict body is compact. We deliberately do not re-flow whitespace
    // inside the entry body — that is presentational and the source HTML
    // already chose its line breaks.
    let mut out = blank_lines.replace_all(s.trim(), "\n").into_owned();
    if out.is_empty() {
        out = format!("<b>{}</b>", html_escape(headword));
    }
    out
}

/// Compare two byte slices using glib's `g_ascii_strcasecmp` semantics, which
/// is what every StarDict reader uses to binary-search the index. ASCII
/// `A`-`Z` fold to `a`-`z`; everything else is compared bytewise as-is. UTF-8
/// multi-byte sequences therefore compare by raw byte order, which for the
/// Greek BMP coincides with Unicode codepoint order.
pub(crate) fn ascii_case_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let aa = ascii_lower(a[i]);
        let bb = ascii_lower(b[i]);
        if aa != bb {
            return aa.cmp(&bb);
        }
    }
    a.len().cmp(&b.len())
}

#[inline]
fn ascii_lower(b: u8) -> u8 {
    if b.is_ascii_uppercase() { b + 32 } else { b }
}

/// `.ifo` is line-oriented `key=value`, so embedded newlines or carriage
/// returns would silently corrupt downstream metadata. Replace them with a
/// single space; everything else is UTF-8 verbatim.
fn sanitize_ifo_value(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// Append ` (in-out)` to the bookname when the OPF declares the dictionary's
/// in/out languages and the bookname does not already contain a 2–3 letter
/// hyphen-separated pair.
///
/// GoldenDict-ng / GoldenDict / KOReader populate "Translates from / to" by
/// regex-parsing the `.dict` filename and the `.ifo` `bookname` for a
/// `xx-yy` or `xxx-yyy` pair (case-insensitive, bounded by non-letters); they
/// do not read any explicit `.ifo` language field. StarDict 2.4.2 has no
/// formal slot for source/target languages either, so embedding the codes in
/// the bookname is the de-facto standard. Auto-augmenting from the OPF means
/// every dictionary built through kindling renders correctly in those readers
/// without callers having to remember to bake the codes into `dc:title`.
fn augment_bookname_with_lang_pair(bookname: &str, in_lang: &str, out_lang: &str) -> String {
    if in_lang.is_empty() || out_lang.is_empty() {
        return bookname.to_string();
    }
    if bookname_has_detectable_lang_pair(bookname) {
        return bookname.to_string();
    }
    let suffix = format!(
        "({}-{})",
        in_lang.to_ascii_lowercase(),
        out_lang.to_ascii_lowercase()
    );
    if bookname.is_empty() {
        suffix
    } else {
        format!("{} {}", bookname, suffix)
    }
}

/// Mirror GoldenDict-ng's `LangCoder::findLangIdPairFromName` regex so we
/// agree on what counts as "already has a language pair". Case-insensitive,
/// 2–3 ASCII letters on each side of a hyphen, bounded by start/end of
/// string or a non-letter on both sides. Long names like `Greek-English`
/// don't match (each side exceeds 3 letters) and so will still get the
/// suffix appended.
fn bookname_has_detectable_lang_pair(s: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)(^|[^a-zA-Z])[a-zA-Z]{2,3}-[a-zA-Z]{2,3}($|[^a-zA-Z])").unwrap()
    });
    re.is_match(s)
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_case_cmp_orders_case_insensitively() {
        assert_eq!(ascii_case_cmp(b"alpha", b"BETA"), std::cmp::Ordering::Less);
        assert_eq!(ascii_case_cmp(b"Bravo", b"bravo"), std::cmp::Ordering::Equal);
        assert_eq!(ascii_case_cmp(b"charlie", b"Bravo"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn ascii_case_cmp_is_bytewise_for_non_ascii() {
        // Greek alpha (CE B1) sorts before Greek beta (CE B2) by raw bytes.
        assert_eq!(ascii_case_cmp("α".as_bytes(), "β".as_bytes()), std::cmp::Ordering::Less);
    }

    #[test]
    fn clean_entry_html_strips_idx_markup() {
        let raw = r#"<idx:entry name="default" scriptable="yes">
<idx:orth value="alpha"><b>alpha</b><idx:infl><idx:iform value="alphas"/></idx:infl></idx:orth>
<p>Definition.</p>
</idx:entry>"#;
        let cleaned = clean_entry_html(raw, "alpha");
        assert!(!cleaned.contains("idx:"), "idx markup leaked: {}", cleaned);
        assert!(cleaned.contains("<b>alpha</b>"), "headword missing: {}", cleaned);
        assert!(cleaned.contains("<p>Definition.</p>"), "definition missing: {}", cleaned);
    }

    #[test]
    fn clean_entry_html_self_closing_orth_synthesises_bold_headword() {
        let raw = r#"<idx:entry><idx:orth value="kindling"/><p>Material…</p></idx:entry>"#;
        let cleaned = clean_entry_html(raw, "kindling");
        assert!(cleaned.starts_with("<b>kindling</b>"), "got: {}", cleaned);
        assert!(cleaned.contains("<p>Material…</p>"));
    }

    #[test]
    fn clean_entry_html_synthesises_headword_when_body_is_empty() {
        let raw = r#"<idx:entry><idx:orth value="x"/></idx:entry>"#;
        let cleaned = clean_entry_html(raw, "x");
        assert_eq!(cleaned, "<b>x</b>");
    }

    #[test]
    fn clean_entry_html_rewrites_cross_letter_crossrefs() {
        let raw = r##"<idx:entry><idx:orth value="τη"/><p>See <a href="content_19.html#hw_τη">τη</a>.</p></idx:entry>"##;
        let cleaned = clean_entry_html(raw, "τη");
        assert!(
            cleaned.contains(r##"href="bword://τη""##),
            "expected bword rewrite, got: {}",
            cleaned
        );
        assert!(!cleaned.contains("content_19.html"), "stale per-letter href: {}", cleaned);
    }

    #[test]
    fn clean_entry_html_rewrites_same_page_crossrefs() {
        let raw = r##"<idx:entry><idx:orth value="α"/><p>Compare <a href="#hw_β">β</a>.</p></idx:entry>"##;
        let cleaned = clean_entry_html(raw, "α");
        assert!(
            cleaned.contains(r##"href="bword://β""##),
            "expected bword rewrite for same-page crossref, got: {}",
            cleaned
        );
        assert!(!cleaned.contains("#hw_"), "stale hw_ fragment: {}", cleaned);
    }

    #[test]
    fn clean_entry_html_passes_through_external_hrefs() {
        let raw = r##"<idx:entry><idx:orth value="x"/><p><a href="https://example.com/page#hw_x">link</a></p></idx:entry>"##;
        let cleaned = clean_entry_html(raw, "x");
        assert!(
            cleaned.contains(r##"href="https://example.com/page#hw_x""##),
            "external href should be untouched, got: {}",
            cleaned
        );
        assert!(!cleaned.contains("bword://"), "non-internal link should not be rewritten: {}", cleaned);
    }

    #[test]
    fn clean_entry_html_passes_through_non_hw_fragments() {
        // Fragment without the `hw_` prefix (e.g. usage notes, footnotes)
        // should be left alone since it is not a headword reference.
        let raw = r##"<idx:entry><idx:orth value="x"/><p><a href="content_05.html#note1">note</a></p></idx:entry>"##;
        let cleaned = clean_entry_html(raw, "x");
        assert!(cleaned.contains(r##"href="content_05.html#note1""##), "got: {}", cleaned);
        assert!(!cleaned.contains("bword://"), "got: {}", cleaned);
    }

    #[test]
    fn sanitize_ifo_strips_newlines() {
        assert_eq!(sanitize_ifo_value("foo\nbar"), "foo bar");
        assert_eq!(sanitize_ifo_value("foo\r\nbar"), "foo  bar");
        assert_eq!(sanitize_ifo_value("plain"), "plain");
    }

    #[test]
    fn augment_bookname_appends_pair_when_absent() {
        assert_eq!(
            augment_bookname_with_lang_pair("Lemma Greek Dictionary", "el", "en"),
            "Lemma Greek Dictionary (el-en)"
        );
    }

    #[test]
    fn augment_bookname_appends_for_long_dash_separated_words() {
        // "Greek-English" looks like a pair but each side is too long for the
        // 2-3 letter regex, so GoldenDict-ng won't detect it. We still need
        // to append the codes so language detection works.
        assert_eq!(
            augment_bookname_with_lang_pair("Greek-English Dictionary", "el", "en"),
            "Greek-English Dictionary (el-en)"
        );
    }

    #[test]
    fn augment_bookname_skips_when_pair_already_present() {
        // Already has a detectable pair in parentheses.
        assert_eq!(
            augment_bookname_with_lang_pair("My Dict (en-fr)", "en", "fr"),
            "My Dict (en-fr)"
        );
        // Case-insensitive detection: the existing pair is uppercase.
        assert_eq!(
            augment_bookname_with_lang_pair("RU-EN Dict", "ru", "en"),
            "RU-EN Dict"
        );
        // Three-letter codes still count as a pair.
        assert_eq!(
            augment_bookname_with_lang_pair("Old Dict (grc-eng)", "grc", "eng"),
            "Old Dict (grc-eng)"
        );
    }

    #[test]
    fn augment_bookname_skips_when_either_lang_missing() {
        assert_eq!(
            augment_bookname_with_lang_pair("My Dict", "", "en"),
            "My Dict"
        );
        assert_eq!(
            augment_bookname_with_lang_pair("My Dict", "el", ""),
            "My Dict"
        );
        assert_eq!(augment_bookname_with_lang_pair("My Dict", "", ""), "My Dict");
    }

    #[test]
    fn augment_bookname_handles_monolingual_pair() {
        // GoldenDict-ng accepts identical from/to codes; "el-el" renders as
        // "Translates from: Greek / Translates to: Greek" for a Greek-only
        // dictionary, which is informative even if redundant.
        assert_eq!(
            augment_bookname_with_lang_pair("Lemma Greek Dictionary", "el", "el"),
            "Lemma Greek Dictionary (el-el)"
        );
    }

    #[test]
    fn augment_bookname_lowercases_codes() {
        assert_eq!(
            augment_bookname_with_lang_pair("Dict", "EL", "EN"),
            "Dict (el-en)"
        );
    }

    #[test]
    fn augment_bookname_handles_three_letter_codes() {
        assert_eq!(
            augment_bookname_with_lang_pair("Ancient Greek Dict", "grc", "eng"),
            "Ancient Greek Dict (grc-eng)"
        );
    }

    #[test]
    fn augment_bookname_synthesises_pair_when_bookname_empty() {
        assert_eq!(augment_bookname_with_lang_pair("", "el", "en"), "(el-en)");
    }

    #[test]
    fn detect_lang_pair_matches_short_codes_only() {
        assert!(bookname_has_detectable_lang_pair("My Dict (en-fr)"));
        assert!(bookname_has_detectable_lang_pair("en-fr"));
        assert!(bookname_has_detectable_lang_pair("EN-FR"));
        assert!(bookname_has_detectable_lang_pair("Lemma Dict grc-eng v2"));
        // Long words are not codes.
        assert!(!bookname_has_detectable_lang_pair("Greek-English Dictionary"));
        assert!(!bookname_has_detectable_lang_pair("Lemma Greek Dictionary"));
        // Single-letter words don't qualify.
        assert!(!bookname_has_detectable_lang_pair("a-b dict"));
        // Boundary requirement: surrounding letters disqualify.
        assert!(!bookname_has_detectable_lang_pair("supercala-fragilistic"));
    }
}
