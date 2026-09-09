//! Internal link resolution shared by the MOBI6 and KF8 writers.
//!
//! An EPUB links between its own documents with ordinary hrefs
//! (`../Text/notes.xhtml#n1`, `#local`). Neither Kindle format understands
//! those, because both formats concatenate every document into one byte
//! stream and throw the file boundaries away:
//!
//! * MOBI6 wants `<a filepos=0000001014>`, a decimal byte offset into the
//!   uncompressed text blob.
//! * KF8 wants `<a href="kindle:pos:fid:0002:off:000000001D">`, where the
//!   fid is the target's fragment and the offset is a base-32 byte offset
//!   inside that fragment.
//!
//! Both need the same three things, which is what this module provides:
//! resolving an href against the document that contains it, finding every
//! anchor a link could target, and locating the `href` attributes to
//! overwrite. The writers differ only in what they write, so they own the
//! rewriting itself.
//!
//! Fragment names repeat across documents constantly — a book whose every
//! chapter numbers its own footnotes from one has a `ftn-1` in each — so a
//! fragment is only ever looked up inside the document that declares it.
//! Resolving them in one global table sends every chapter's first footnote
//! to the same place.

use std::collections::HashMap;

/// URI schemes that never point inside the book. `kindle:` is in the list
/// because a link already rewritten to `kindle:pos:` or `kindle:embed:`
/// must not be rewritten twice.
const EXTERNAL_SCHEMES: [&str; 8] = [
    "http://",
    "https://",
    "mailto:",
    "kindle:",
    "tel:",
    "data:",
    "javascript:",
    "ftp://",
];

/// Where a link points, once resolved against the document holding it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// Leave the href exactly as it is. Either it points outside the book,
    /// or it names no destination at all, as `href=""` does. Overwriting an
    /// empty one would report a link the author never wrote as broken.
    Leave,
    /// Points at a document kindling put in the text stream. `file` is that
    /// document's position in spine order; `fragment` is `None` for a
    /// whole-document link.
    Internal {
        file: usize,
        fragment: Option<String>,
    },
    /// Names a document that is not in the spine. kindlegen reports these
    /// as "Hyperlink not resolved" and writes a dead link of the same
    /// width; kindling does the same.
    Unresolved,
}

/// An `id` or `name` attribute that a fragment link can target, and the
/// byte offset of the `<` that opens the element carrying it.
///
/// The offset is of the element, not of the attribute: a reader told to
/// jump to a byte in the middle of a tag would land mid-markup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub name: String,
    pub offset: usize,
}

/// An `href` attribute on an `<a>` element, as a byte range to overwrite
/// and the value it currently holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HrefAttr {
    /// Byte range of the whole attribute, `href` through the closing
    /// quote. Overwriting exactly this range leaves the rest of the tag
    /// intact.
    pub start: usize,
    pub end: usize,
    /// The attribute value, still percent-encoded.
    pub value: String,
}

/// Split an href into its path and fragment parts. An empty fragment
/// (`page.xhtml#`) counts as no fragment.
pub(crate) fn split_href(href: &str) -> (&str, Option<&str>) {
    match href.find('#') {
        Some(i) => {
            let frag = &href[i + 1..];
            (&href[..i], if frag.is_empty() { None } else { Some(frag) })
        }
        None => (href, None),
    }
}

/// True when the href points outside the book.
pub(crate) fn is_external_href(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    EXTERNAL_SCHEMES.iter().any(|s| lower.starts_with(s))
}

/// Resolve `.` and `..` segments in a slash-separated path.
pub(crate) fn normalize_path(path: &str) -> String {
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

/// Percent-decode a path. Bytes are decoded then read back as UTF-8 so a
/// multi-byte character written as several escapes survives; an escape
/// that does not form valid UTF-8 is left alone rather than mangled.
pub(crate) fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Directory part of a manifest href, `""` when it has none.
fn parent_dir(href: &str) -> &str {
    match href.rfind('/') {
        Some(i) => &href[..i],
        None => "",
    }
}

/// Build the lookup a resolver needs: every spine document's normalized
/// manifest href mapped to its position in spine order.
///
/// The hrefs come from `OPFData::get_content_html_hrefs`, which is
/// index-aligned with the documents kindling actually writes, so the
/// position is also the KF8 fragment id.
pub(crate) fn build_document_index(hrefs: &[String]) -> HashMap<String, usize> {
    let mut map: HashMap<String, usize> = HashMap::new();
    for (i, href) in hrefs.iter().enumerate() {
        map.entry(normalize_path(&percent_decode(href)))
            .or_insert(i);
    }
    map
}

/// Resolve one `<a href>` against the document that contains it.
///
/// `doc_href` is the containing document's own manifest href, which is what
/// makes `../Text/notes.xhtml` and a bare `#frag` mean different things in
/// different documents.
///
/// `redirects` names documents that are in the spine but not in this half of
/// the file, mapped to the document a link to them should land on instead.
/// The KF8 half drops a bare in-spine cover page, because the metadata cover
/// already renders full-page, and a table of contents that links to it would
/// otherwise be pointing at nothing. The destination is not in doubt, so
/// such a link goes to the start of the book rather than being killed. Pass
/// an empty map where nothing is dropped.
pub(crate) fn resolve(
    doc_href: &str,
    self_index: usize,
    href: &str,
    documents: &HashMap<String, usize>,
    redirects: &HashMap<String, usize>,
) -> Resolution {
    let href = href.trim();
    if href.is_empty() || is_external_href(href) {
        return Resolution::Leave;
    }
    let (path, fragment) = split_href(href);
    let fragment = fragment.map(percent_decode);

    // A bare `#frag` stays in the document that wrote it, and a bare `#`
    // means the top of that document, which is where kindlegen sends it.
    if path.is_empty() {
        return Resolution::Internal {
            file: self_index,
            fragment,
        };
    }

    // The containing document's own href may be percent-encoded too, and
    // the directory it contributes has to be in the same form as the keys
    // in `documents` for the join to match.
    let doc_decoded = percent_decode(doc_href);
    let dir = parent_dir(&doc_decoded);
    let decoded = percent_decode(path);
    let joined = if dir.is_empty() {
        decoded
    } else {
        format!("{}/{}", dir, decoded)
    };
    let target = normalize_path(&joined);

    if let Some(&file) = redirects.get(&target) {
        // The document itself is gone, so any fragment inside it is too.
        return Resolution::Internal {
            file,
            fragment: None,
        };
    }
    match documents.get(&target) {
        Some(&file) => Resolution::Internal { file, fragment },
        // Not in the spine. A manifest-only document is not in the text
        // stream at all, so there is no byte to point at.
        None => Resolution::Unresolved,
    }
}

/// Encode `value` as zero-padded base-32 using the alphabet Kindle uses for
/// `kindle:pos` offsets and fragment ids (`0-9`, then `A-V`).
///
/// A value too large for `width` characters is truncated to the low bits,
/// which is what the format's fixed-width field forces; at ten characters
/// that is 50 bits, far past any real book.
pub(crate) fn encode_base32(value: usize, width: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = vec![b'0'; width];
    let mut v = value;
    for slot in out.iter_mut().rev() {
        *slot = CHARS[v % 32];
        v /= 32;
    }
    String::from_utf8(out).expect("base-32 alphabet is ASCII")
}

// ---------------------------------------------------------------------------
// Tag scanning
// ---------------------------------------------------------------------------

/// Elements whose `id` is not a usable jump target.
///
/// `<html>`, `<head>` and `<body>` do not survive into the text stream:
/// MOBI6 merges every document's body contents and drops the wrappers, and
/// KF8 keeps the body tag on the skeleton rather than in the fragment. A
/// link to `#some-body-id` therefore has no element to land on. kindlegen
/// treats these as unresolved too, and warns.
const NON_TARGET_TAGS: [&str; 3] = ["html", "head", "body"];

/// One element's opening tag.
pub(crate) struct TagSpan {
    /// Offset of `<`.
    pub start: usize,
    /// Lowercased element name.
    pub name: String,
    /// Offset just past `>`.
    pub end: usize,
    /// True for `</name>`, which closes an element rather than opening one.
    pub closing: bool,
    /// True for `<name/>`, which opens and closes in one tag.
    pub self_closing: bool,
}

/// Walk the tags of `html`, calling `f` for each with the tag span and the
/// raw attribute text between the name and the closing `>`.
///
/// This is a scanner rather than a regular expression because an attribute
/// value may legally contain `>`, which a `<[^>]*>` pattern cuts in half.
/// Comments, CDATA sections, doctypes and processing instructions are
/// skipped. Closing tags are reported, with `closing` set and no attributes,
/// because a caller counting nesting depth needs them; callers that only
/// care about elements being opened skip them.
pub(crate) fn for_each_tag<F: FnMut(&TagSpan, &str)>(html: &str, mut f: F) {
    let bytes = html.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        let start = i;
        let rest = &bytes[i + 1..];
        // `<!-- … -->`, `<![CDATA[ … ]]>`, `<!DOCTYPE …>`, `<?xml … ?>`.
        if rest.first() == Some(&b'!') || rest.first() == Some(&b'?') {
            if html[i..].starts_with("<!--") {
                i = html[i + 4..]
                    .find("-->")
                    .map(|p| i + 4 + p + 3)
                    .unwrap_or(bytes.len());
            } else {
                i = html[i..]
                    .find('>')
                    .map(|p| i + p + 1)
                    .unwrap_or(bytes.len());
            }
            continue;
        }
        let closing = rest.first() == Some(&b'/');
        // Element name.
        let mut j = i + 1 + usize::from(closing);
        while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b':') {
            j += 1;
        }
        if j == i + 1 + usize::from(closing) {
            // A bare `<` in text, not a tag.
            i += 1;
            continue;
        }
        let name = html[i + 1 + usize::from(closing)..j].to_ascii_lowercase();
        // Attributes, stopping at the `>` that is not inside a quoted value.
        //
        // Only a quote that opens an attribute value counts, which means one
        // that follows `=` and any whitespace. A quote anywhere else is an
        // ordinary character. That distinction matters because a stray quote
        // inside a tag is common in converted books — an inch mark in
        // `<span title="a 5" gun">` is the usual way it happens — and
        // treating it as opening a value makes the scanner hunt for a
        // closing quote through the rest of the document, silently losing
        // every link and anchor after it. HTML tokenizers end the tag at
        // that `>`, and so does kindlegen.
        let mut k = j;
        let mut quote: Option<u8> = None;
        let mut expecting_value = false;
        while k < bytes.len() {
            let b = bytes[k];
            match quote {
                Some(q) => {
                    if b == q {
                        quote = None;
                    }
                }
                None => {
                    if b == b'>' {
                        break;
                    } else if b == b'=' {
                        expecting_value = true;
                    } else if expecting_value && (b == b'"' || b == b'\'') {
                        quote = Some(b);
                        expecting_value = false;
                    } else if !b.is_ascii_whitespace() {
                        // An unquoted value, or the next attribute name.
                        expecting_value = false;
                    }
                }
            }
            k += 1;
        }
        if k >= bytes.len() {
            break;
        }
        let attrs = &html[j..k];
        let self_closing = attrs.trim_end().ends_with('/');
        let span = TagSpan {
            start,
            name,
            end: k + 1,
            closing,
            self_closing,
        };
        f(&span, attrs);
        i = k + 1;
    }
}

/// Find one attribute inside a tag's attribute text.
///
/// Returns the attribute's byte range relative to `attrs` and its value.
/// Only quoted values are recognized; an unquoted value is left alone
/// rather than guessed at, so such a link keeps its href and stays inert
/// instead of being rewritten to the wrong place.
fn find_attr(attrs: &str, wanted: &str) -> Option<(usize, usize, String)> {
    // Characters that can appear inside an attribute name. A name has to
    // start after one of something else, otherwise the tail of a longer
    // name reads as a name of its own and `data.href` is mistaken for
    // `href`.
    fn name_char(b: u8) -> bool {
        b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b':' || b == b'.'
    }

    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip to the start of an attribute name.
        while i < bytes.len() && !bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i > 0 && i < bytes.len() && name_char(bytes[i - 1]) {
            // Mid-name, so not the start of one. Step over the rest of it.
            while i < bytes.len() && name_char(bytes[i]) {
                i += 1;
            }
            continue;
        }
        let name_start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric()
                || bytes[i] == b'-'
                || bytes[i] == b'_'
                || bytes[i] == b':')
        {
            i += 1;
        }
        if i == name_start {
            break;
        }
        let name = &attrs[name_start..i];
        // Optional whitespace, `=`, whitespace, then the value.
        let mut j = i;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b'=' {
            // Valueless attribute; carry on from where the name ended.
            continue;
        }
        j += 1;
        while j < bytes.len() && bytes[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= bytes.len() {
            break;
        }
        let quote = bytes[j];
        if quote != b'"' && quote != b'\'' {
            // Unquoted value: skip it without interpreting it.
            while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            i = j;
            continue;
        }
        let value_start = j + 1;
        let Some(rel_end) = attrs[value_start..].find(quote as char) else {
            break;
        };
        let value_end = value_start + rel_end;
        if name.eq_ignore_ascii_case(wanted) {
            return Some((
                name_start,
                value_end + 1,
                attrs[value_start..value_end].to_string(),
            ));
        }
        i = value_end + 1;
    }
    None
}

/// Collect every anchor in `html` that a fragment link could target.
///
/// `id` counts on any element. The legacy `name` attribute counts only on
/// `<a>`, which is the only element it ever meant an anchor on: EPUBs from
/// older toolchains still write `<a name="note1">` and kindlegen resolves
/// those, but `name` on a form control or a `<meta>` is an unrelated
/// attribute and must not shadow a real anchor of the same value.
///
/// When a document declares the same name twice the first wins, matching
/// how a browser resolves a duplicate id.
pub(crate) fn scan_anchors(html: &str) -> Vec<Anchor> {
    let mut out: Vec<Anchor> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for_each_tag(html, |tag, attrs| {
        if tag.closing || NON_TARGET_TAGS.contains(&tag.name.as_str()) {
            return;
        }
        let attributes: &[&str] = if tag.name == "a" {
            &["id", "name"]
        } else {
            &["id"]
        };
        for attr in attributes {
            if let Some((_, _, value)) = find_attr(attrs, attr) {
                if !value.is_empty() && seen.insert(value.clone()) {
                    out.push(Anchor {
                        name: value,
                        offset: tag.start,
                    });
                }
            }
        }
    });
    out
}

/// Collect the ids declared on a document's wrapper elements.
///
/// A link to one of these is a link to the document itself, because that is
/// all a `<body id>` can mean, and it is how such a link behaves in a
/// browser. Neither Kindle format has a byte to point at for the wrapper
/// itself — MOBI6 drops the tags when it merges, KF8 leaves the body tag on
/// the skeleton rather than in the fragment — so the writers resolve these
/// to the start of the document's content instead of killing the link.
///
/// This is what kindlegen does in KF8. In MOBI6 kindlegen gives up and
/// reports the link as unresolved; kindling resolves it there too, because
/// the destination is not in doubt.
pub(crate) fn scan_document_anchors(html: &str) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for_each_tag(html, |tag, attrs| {
        if tag.closing || !NON_TARGET_TAGS.contains(&tag.name.as_str()) {
            return;
        }
        for attr in ["id", "name"] {
            if let Some((_, _, value)) = find_attr(attrs, attr) {
                if !value.is_empty() {
                    out.insert(value);
                }
            }
        }
    });
    out
}

/// Collect every `href` attribute on an `<a>` element in `html`, as byte
/// ranges into `html`.
///
/// Ranges are returned in document order and never overlap, so a caller can
/// rewrite them front to back while tracking how much the text has shifted.
pub(crate) fn scan_hrefs(html: &str) -> Vec<HrefAttr> {
    let mut out: Vec<HrefAttr> = Vec::new();
    for_each_tag(html, |tag, attrs| {
        if tag.closing || tag.name != "a" {
            return;
        }
        // `attrs` starts at the byte after the element name.
        let attrs_start = tag.end - 1 - attrs.len();
        if let Some((rel_start, rel_end, value)) = find_attr(attrs, "href") {
            out.push(HrefAttr {
                start: attrs_start + rel_start,
                end: attrs_start + rel_end,
                value,
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docs(hrefs: &[&str]) -> HashMap<String, usize> {
        build_document_index(&hrefs.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn splits_href_into_path_and_fragment() {
        assert_eq!(split_href("a.xhtml#f1"), ("a.xhtml", Some("f1")));
        assert_eq!(split_href("a.xhtml"), ("a.xhtml", None));
        assert_eq!(split_href("#f1"), ("", Some("f1")));
        // A trailing `#` names no fragment.
        assert_eq!(split_href("a.xhtml#"), ("a.xhtml", None));
    }

    #[test]
    fn normalizes_dot_segments() {
        assert_eq!(normalize_path("Text/../Images/a.jpg"), "Images/a.jpg");
        assert_eq!(normalize_path("./a.xhtml"), "a.xhtml");
        assert_eq!(normalize_path("Text//a.xhtml"), "Text/a.xhtml");
    }

    #[test]
    fn percent_decodes_multibyte_escapes() {
        assert_eq!(percent_decode("off%20spine.xhtml"), "off spine.xhtml");
        // Three escapes forming one character, not three replacement chars.
        assert_eq!(percent_decode("%E2%80%94"), "\u{2014}");
        // Not an escape; left alone rather than mangled.
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
    }

    #[test]
    fn resolves_relative_paths_against_the_containing_document() {
        let d = docs(&["OEBPS/Text/ch1.xhtml", "OEBPS/Text/notes.xhtml"]);
        assert_eq!(
            resolve(
                "OEBPS/Text/ch1.xhtml",
                0,
                "../Text/notes.xhtml#n1",
                &d,
                &HashMap::new()
            ),
            Resolution::Internal {
                file: 1,
                fragment: Some("n1".to_string())
            }
        );
        assert_eq!(
            resolve(
                "OEBPS/Text/ch1.xhtml",
                0,
                "notes.xhtml",
                &d,
                &HashMap::new()
            ),
            Resolution::Internal {
                file: 1,
                fragment: None
            }
        );
    }

    #[test]
    fn keeps_a_bare_fragment_in_its_own_document() {
        let d = docs(&["Text/ch1.xhtml", "Text/ch2.xhtml"]);
        // The same fragment name in two documents must not collide: each
        // resolves to the document that wrote it.
        assert_eq!(
            resolve("Text/ch1.xhtml", 0, "#local", &d, &HashMap::new()),
            Resolution::Internal {
                file: 0,
                fragment: Some("local".to_string())
            }
        );
        assert_eq!(
            resolve("Text/ch2.xhtml", 1, "#local", &d, &HashMap::new()),
            Resolution::Internal {
                file: 1,
                fragment: Some("local".to_string())
            }
        );
    }

    #[test]
    fn leaves_external_schemes_alone() {
        let d = docs(&["a.xhtml"]);
        for href in [
            "https://example.org/",
            "http://example.org/",
            "mailto:a@example.org",
            "tel:+15550100",
            "kindle:pos:fid:0001:off:0000000000",
        ] {
            assert_eq!(
                resolve("a.xhtml", 0, href, &d, &HashMap::new()),
                Resolution::Leave,
                "{href}"
            );
        }
    }

    #[test]
    fn reports_documents_outside_the_spine_as_unresolved() {
        let d = docs(&["Text/a.xhtml"]);
        assert_eq!(
            resolve("Text/a.xhtml", 0, "missing.xhtml#x", &d, &HashMap::new()),
            Resolution::Unresolved
        );
    }

    #[test]
    fn sends_a_bare_hash_to_the_top_of_its_own_document() {
        // kindlegen resolves `href="#"` to the start of the document that
        // wrote it, in both formats. Treating it as broken both kills a
        // working link and inflates the unresolved count.
        let d = docs(&["Text/a.xhtml", "Text/b.xhtml"]);
        assert_eq!(
            resolve("Text/b.xhtml", 1, "#", &d, &HashMap::new()),
            Resolution::Internal {
                file: 1,
                fragment: None
            }
        );
    }

    #[test]
    fn leaves_an_empty_href_alone() {
        // `href=""` names no destination. Overwriting it reports a link the
        // author never wrote as broken.
        let d = docs(&["Text/a.xhtml"]);
        assert_eq!(
            resolve("Text/a.xhtml", 0, "", &d, &HashMap::new()),
            Resolution::Leave
        );
        assert_eq!(
            resolve("Text/a.xhtml", 0, "   ", &d, &HashMap::new()),
            Resolution::Leave
        );
    }

    #[test]
    fn finds_ids_on_the_wrapper_elements() {
        let html = r#"<html id="h"><head></head><body id="b"><p id="real">x</p></body></html>"#;
        let w = scan_document_anchors(html);
        assert!(w.contains("b"), "{w:?}");
        assert!(w.contains("h"), "{w:?}");
        assert!(!w.contains("real"), "{w:?}");
        // And the two scans do not overlap: a wrapper id is never also a
        // byte offset, which is what makes the fallback unambiguous.
        let names: Vec<String> = scan_anchors(html).into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[test]
    fn resolves_percent_encoded_document_names() {
        let d = docs(&["Text/off spine.xhtml"]);
        assert_eq!(
            resolve(
                "Text/a.xhtml",
                9,
                "off%20spine.xhtml#deep",
                &d,
                &HashMap::new()
            ),
            Resolution::Internal {
                file: 0,
                fragment: Some("deep".to_string())
            }
        );
    }

    #[test]
    fn resolves_relative_to_a_percent_encoded_containing_document() {
        // The directory the link is resolved against comes from the
        // containing document's own href, which can be encoded as well.
        let d = docs(&["My Text/a.xhtml", "My Text/b.xhtml"]);
        assert_eq!(
            resolve("My%20Text/a.xhtml", 0, "b.xhtml#f", &d, &HashMap::new()),
            Resolution::Internal {
                file: 1,
                fragment: Some("f".to_string())
            }
        );
    }

    #[test]
    fn decodes_percent_encoded_fragments() {
        let d = docs(&["a.xhtml", "b.xhtml"]);
        assert_eq!(
            resolve("a.xhtml", 0, "b.xhtml#note%201", &d, &HashMap::new()),
            Resolution::Internal {
                file: 1,
                fragment: Some("note 1".to_string())
            }
        );
    }

    #[test]
    fn encodes_base32_the_way_kindle_reads_it() {
        // Values checked against kindlegen output: 423 -> D7, 94 -> 2U.
        assert_eq!(encode_base32(423, 10), "00000000D7");
        assert_eq!(encode_base32(94, 10), "000000002U");
        assert_eq!(encode_base32(0, 10), "0000000000");
        assert_eq!(encode_base32(1, 4), "0001");
        assert_eq!(encode_base32(31, 4), "000V");
        assert_eq!(encode_base32(32, 4), "0010");
    }

    #[test]
    fn finds_anchors_at_the_start_of_their_element() {
        let html = r#"<h1 id="top">T</h1><p>x <span id="deep">y</span></p>"#;
        let anchors = scan_anchors(html);
        assert_eq!(
            anchors[0],
            Anchor {
                name: "top".into(),
                offset: 0
            }
        );
        // The offset is the `<` of the span, not of its id attribute.
        let span_at = html.find("<span").unwrap();
        assert_eq!(
            anchors[1],
            Anchor {
                name: "deep".into(),
                offset: span_at
            }
        );
    }

    #[test]
    fn finds_legacy_name_anchors() {
        let html = r#"<p><a name="note1">n</a></p>"#;
        let anchors = scan_anchors(html);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "note1");
        assert_eq!(anchors[0].offset, html.find("<a").unwrap());
    }

    #[test]
    fn ignores_name_on_elements_where_it_is_not_an_anchor() {
        // A form control named `n1` must not shadow the paragraph that
        // actually declares the anchor.
        let html = r#"<input name="n1"/><p id="n1">real</p>"#;
        let anchors = scan_anchors(html);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].offset, html.find("<p").unwrap());
    }

    #[test]
    fn ignores_anchors_on_wrapper_elements() {
        // These tags do not survive into the text stream, so an id on them
        // has no byte to point at.
        let html =
            r#"<html id="h"><head id="hd"></head><body id="b"><p id="real">x</p></body></html>"#;
        let names: Vec<_> = scan_anchors(html).into_iter().map(|a| a.name).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[test]
    fn keeps_the_first_of_a_duplicated_anchor_name() {
        let html = r#"<p id="dup">one</p><p id="dup">two</p>"#;
        let anchors = scan_anchors(html);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].offset, 0);
    }

    #[test]
    fn finds_href_attributes_only_on_anchors() {
        let html = r#"<link href="s.css"/><a href="x.xhtml#f">t</a><img src="i.jpg"/>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "x.xhtml#f");
        assert_eq!(&html[hrefs[0].start..hrefs[0].end], r#"href="x.xhtml#f""#);
    }

    #[test]
    fn finds_href_after_other_attributes() {
        let html = r#"<a id="r" epub:type="noteref" href="n.xhtml#n1" class="c">t</a>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(&html[hrefs[0].start..hrefs[0].end], r#"href="n.xhtml#n1""#);
    }

    #[test]
    fn reads_single_quoted_attribute_values() {
        let html = "<a href='n.xhtml#n1'>t</a>";
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs[0].value, "n.xhtml#n1");
        assert_eq!(&html[hrefs[0].start..hrefs[0].end], "href='n.xhtml#n1'");
    }

    #[test]
    fn survives_a_greater_than_inside_an_attribute_value() {
        // A `<[^>]*>` pattern cuts this tag in half and loses the href.
        let html = r#"<a title="a > b" href="n.xhtml#n1">t</a><p id="after">x</p>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "n.xhtml#n1");
        let anchors = scan_anchors(html);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "after");
    }

    #[test]
    fn skips_comments_and_declarations() {
        let html = r#"<!DOCTYPE html><!-- <a href="no.xhtml"> --><a href="yes.xhtml">t</a>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "yes.xhtml");
    }

    #[test]
    fn ignores_an_anchor_with_no_href() {
        let html = r#"<a id="target">t</a><a href="x.xhtml">u</a>"#;
        assert_eq!(scan_hrefs(html).len(), 1);
    }

    #[test]
    fn leaves_unquoted_href_values_untouched() {
        // Rewriting a value we cannot delimit would corrupt the tag.
        let html = "<a href=x.xhtml>t</a>";
        assert!(scan_hrefs(html).is_empty());
    }

    // -----------------------------------------------------------------
    // Adversarial input
    //
    // Every offset these scanners return is used to slice a `&str`, and
    // slicing a multi-byte character in half panics. A panic here is a
    // build crash on somebody's book, so the bar is that no input reaches
    // one, however malformed.
    // -----------------------------------------------------------------

    /// Markup a real EPUB might contain, plus shapes designed to break a
    /// scanner. Used by the sweep tests below, which check both that
    /// nothing panics and that every offset lands on a character boundary.
    const NASTY: &[&str] = &[
        "",
        "<",
        ">",
        "<>",
        "</>",
        "< p>",
        "<p",
        "<p ",
        "<p id",
        "<p id=",
        "<p id=\"",
        "<p id=\"unterminated",
        "<a href=\"x.xhtml",
        "<!--",
        "<!-- <a href=\"no.xhtml\"> ",
        "<![CDATA[ <a href=\"no\"> ]]>",
        "<?xml version=\"1.0\"?>",
        "<!DOCTYPE html>",
        "<a foo bar baz>t</a>",
        "<a foo=bar href=\"x.xhtml#f\">t</a>",
        "<a data-href=\"decoy.xhtml\" href=\"real.xhtml\">t</a>",
        "<a xlink:href=\"decoy.xhtml\" href=\"real.xhtml\">t</a>",
        "<a href=\"one.xhtml\" href=\"two.xhtml\">t</a>",
        "<a title=\"a > b\" href=\"x.xhtml\">t</a>",
        "<a title='he said \"hi\"' href='x.xhtml'>t</a>",
        "<a href = \"spaced.xhtml\" >t</a>",
        "<p id=\"\">empty id</p>",
        "<h2:ns id=\"prefixed\">x</h2:ns>",
        "<p1 id=\"digit-in-name\">x</p1>",
        // Non-ASCII in every position an offset could be taken near.
        "<p id=\"przypis\">Powiedział że ę</p>",
        "Ala ma kota<a href=\"ń.xhtml#ę\">ł</a>ę",
        "<p id=\"注釈-1\">日本語のテキスト</p>",
        "<p>😀<a href=\"x.xhtml\">😀</a>😀</p>",
        "\u{2014}<a href=\"x.xhtml\">\u{2014}</a>\u{2014}",
        "<p id=\"\u{1F600}\">supplementary plane id</p>",
        // A multi-byte character immediately before and after a tag edge.
        "ę<p id=\"a\">ę</p>ę",
        "<a href=\"%E2%80%94.xhtml#%C4%99\">percent</a>",
        // Stray quotes inside a tag: an inch mark, and an apostrophe in an
        // unquoted value. Both are odd counts and used to swallow the rest
        // of the document.
        "<p title=\"5\" inch\">a</p><a href=\"real.xhtml\">t</a><p id=\"later\">z</p>",
        "<a title=it's href=\"real.xhtml\">t</a><p id=\"later\">z</p>",
        "<span title=\"a 5\" gun\">GUN</span><a href=\"x.xhtml#f\">t</a>",
    ];

    #[test]
    fn scanners_never_panic_on_malformed_or_non_ascii_input() {
        for input in NASTY {
            // The assertions are that these return at all, and that every
            // offset they hand back can be used to slice without panicking.
            for anchor in scan_anchors(input) {
                assert!(
                    input.is_char_boundary(anchor.offset),
                    "anchor offset {} is mid-character in {input:?}",
                    anchor.offset
                );
                let _ = &input[anchor.offset..];
                assert!(
                    input[anchor.offset..].starts_with('<'),
                    "anchor in {input:?} does not point at an element"
                );
            }
            for href in scan_hrefs(input) {
                assert!(
                    input.is_char_boundary(href.start) && input.is_char_boundary(href.end),
                    "href range {}..{} is mid-character in {input:?}",
                    href.start,
                    href.end
                );
                let slice = &input[href.start..href.end];
                assert!(
                    slice.starts_with("href") || slice.starts_with("HREF"),
                    "href range in {input:?} covers {slice:?}"
                );
            }
        }
    }

    #[test]
    fn scanners_terminate_on_every_prefix_of_nasty_input() {
        // Truncation is where a scanner is most likely to spin or to read
        // past the end, and a record boundary can truncate anything. Each
        // call is expected to return; the test hanging is the failure.
        for input in NASTY {
            for end in 0..=input.len() {
                if !input.is_char_boundary(end) {
                    continue;
                }
                let prefix = &input[..end];
                let _ = scan_anchors(prefix);
                let _ = scan_hrefs(prefix);
            }
        }
    }

    #[test]
    fn rewriting_every_href_reassembles_the_document() {
        // What the writers actually do with these ranges: cut the document
        // at each href and paste it back. If a range were wrong, the result
        // would not match.
        for input in NASTY {
            let mut out = String::new();
            let mut cursor = 0usize;
            for href in scan_hrefs(input) {
                assert!(href.start >= cursor, "href ranges overlap in {input:?}");
                out.push_str(&input[cursor..href.start]);
                out.push_str(&input[href.start..href.end]);
                cursor = href.end;
            }
            out.push_str(&input[cursor..]);
            assert_eq!(&out, input, "reassembly lost bytes for {input:?}");
        }
    }

    #[test]
    fn an_attribute_name_ending_in_href_is_not_the_href() {
        // `data-href` and `xlink:href` are real attributes on real EPUB
        // markup; rewriting one of them would point the link at nothing.
        for html in [
            r#"<a data-href="decoy.xhtml" href="real.xhtml">t</a>"#,
            r#"<a xlink:href="decoy.xhtml" href="real.xhtml">t</a>"#,
        ] {
            let hrefs = scan_hrefs(html);
            assert_eq!(hrefs.len(), 1, "{html}");
            assert_eq!(hrefs[0].value, "real.xhtml", "{html}");
        }
    }

    #[test]
    fn the_tail_of_a_longer_attribute_name_is_not_the_href() {
        // `data-href` is caught by the name scan alone, but a separator the
        // scan does not know about leaves `href` looking like a fresh name.
        for html in [
            r#"<a data.href="decoy.xhtml" href="real.xhtml">t</a>"#,
            r#"<a x.href="decoy.xhtml" href="real.xhtml">t</a>"#,
        ] {
            let hrefs = scan_hrefs(html);
            assert_eq!(hrefs.len(), 1, "{html}");
            assert_eq!(hrefs[0].value, "real.xhtml", "{html}");
        }
    }

    #[test]
    fn a_repeated_href_resolves_to_the_first() {
        let html = r#"<a href="one.xhtml" href="two.xhtml">t</a>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "one.xhtml");
    }

    #[test]
    fn a_valueless_attribute_does_not_hide_the_href() {
        let html = r#"<a hidden href="x.xhtml">t</a>"#;
        assert_eq!(scan_hrefs(html)[0].value, "x.xhtml");
    }

    #[test]
    fn an_unquoted_attribute_does_not_hide_the_href() {
        let html = r#"<a class=big href="x.xhtml">t</a>"#;
        assert_eq!(scan_hrefs(html)[0].value, "x.xhtml");
    }

    #[test]
    fn finds_anchors_and_links_around_non_ascii_text() {
        let html = "Powiedział<a href=\"ń.xhtml#ę\">że</a><p id=\"przypis\">ę</p>";
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "ń.xhtml#ę");
        assert_eq!(&html[hrefs[0].start..hrefs[0].end], "href=\"ń.xhtml#ę\"");
        let anchors = scan_anchors(html);
        assert_eq!(anchors.len(), 1);
        assert_eq!(anchors[0].name, "przypis");
        assert_eq!(anchors[0].offset, html.find("<p").unwrap());
    }

    #[test]
    fn a_stray_quote_in_a_tag_does_not_swallow_the_rest_of_the_document() {
        // An inch mark inside an attribute value is a routine defect in
        // converted books. Treating it as opening a value sends the scanner
        // looking for a closing quote through everything that follows, so
        // every link and anchor after it goes unseen: in KF8 they keep a
        // bare href and stop navigating at all, and anchors other documents
        // point at are lost, which turns their links into dead placeholders.
        let html = r#"<p title="5" inch">a</p><a href="real.xhtml">t</a><p id="later">z</p>"#;

        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1, "the link after the stray quote was lost");
        assert_eq!(hrefs[0].value, "real.xhtml");

        let names: Vec<String> = scan_anchors(html).into_iter().map(|a| a.name).collect();
        assert_eq!(
            names,
            vec!["later"],
            "the anchor after the stray quote was lost"
        );
    }

    #[test]
    fn a_stray_quote_after_an_unquoted_value_does_not_swallow_the_document() {
        let html = r#"<a title=it's href="real.xhtml">t</a><p id="later">z</p>"#;
        assert_eq!(scan_hrefs(html).len(), 1, "{html}");
        assert_eq!(scan_anchors(html).len(), 1, "{html}");
    }

    #[test]
    fn a_quote_inside_a_quoted_value_is_still_part_of_the_value() {
        // The other half of the rule: quotes that do open a value must keep
        // working, including the opposite quote character inside one.
        let html = r#"<a title='He said "hi"' href="real.xhtml">t</a><p id="later">z</p>"#;
        let hrefs = scan_hrefs(html);
        assert_eq!(hrefs.len(), 1);
        assert_eq!(hrefs[0].value, "real.xhtml");
        assert_eq!(scan_anchors(html).len(), 1);

        let html = r#"<a title="it's" href="real.xhtml">t</a><p id="later">z</p>"#;
        assert_eq!(scan_hrefs(html)[0].value, "real.xhtml");
        assert_eq!(scan_anchors(html).len(), 1);
    }

    #[test]
    fn resolves_a_non_ascii_fragment_and_document_name() {
        let d = docs(&["Text/\u{2014}.xhtml"]);
        assert_eq!(
            resolve(
                "Text/a.xhtml",
                0,
                "%E2%80%94.xhtml#%C4%99",
                &d,
                &HashMap::new()
            ),
            Resolution::Internal {
                file: 0,
                fragment: Some("\u{119}".to_string())
            }
        );
    }
}
