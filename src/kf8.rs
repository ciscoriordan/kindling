/// KF8 (Kindle Format 8) dual-format support for book MOBIs.
///
/// Produces KF8 text records, FDST, skeleton/fragment/NCX INDX records,
/// CNCX records, and DATP record for the KF8 section of a dual KF7+KF8
/// MOBI file.
///
/// The assembly follows Calibre's writer8/skeleton.py:
/// - Each spine item is split into a SKELETON (XHTML shell with empty
///   aid-marked body) and one or more FRAGMENT chunks (the body contents).
/// - The text flow is laid out as
///     [skel_0][frag_0_0][frag_0_1]...[skel_1][frag_1_0]...
/// - The skeleton INDX lists each skel's absolute start offset and length.
/// - The fragment INDX lists each chunk's insert position (absolute byte
///   offset in the combined text where the fragment content should be
///   spliced before `</body>`), a CNCX selector offset, file number,
///   global sequence number, and the (start_pos, length) geometry of the
///   fragment chunk.
/// - The CNCX records hold the XPath selector strings
///   (e.g. `P-//*[@aid='0']`) that Kindle uses to locate the target
///   element inside the skeleton.
///
/// All INDX records follow Calibre's byte layout (see
/// writer8/index.py): 192-byte header, TAGX section at offset 192 in
/// the primary record, followed by the entry geometry and an IDXT
/// footer; data records carry the per-entry control-byte-prefixed
/// tag value stream and an IDXT footer pointing at each entry's
/// starting offset.

use regex::Regex;

use crate::cncx::CncxBuilder;
use crate::palmdoc;
use crate::vwi::{encode_vwi, encode_vwi_inv};

const RECORD_SIZE: usize = 4096;
const INDX_HEADER_LENGTH: usize = 192;

/// Represents the complete KF8 section (all records after the BOUNDARY).
pub struct Kf8Section {
    /// KF8 text records (compressed, with trailing bytes)
    pub text_records: Vec<Vec<u8>>,
    /// Uncompressed text length (HTML + CSS flows combined)
    pub text_length: usize,
    /// FDST record
    pub fdst: Vec<u8>,
    /// Fragment INDX records (primary + data)
    pub fragment_indx: Vec<Vec<u8>>,
    /// CNCX records that back the fragment INDX tag 2 (aid selector)
    /// offsets. These are PalmDB records and must be written to the
    /// output file by the caller. In Calibre they immediately follow
    /// the fragment index records (between the fragment INDX data
    /// record and the skeleton INDX primary record).
    #[allow(dead_code)]
    pub cncx_records: Vec<Vec<u8>>,
    /// Skeleton INDX records (primary + data)
    pub skeleton_indx: Vec<Vec<u8>>,
    /// NCX INDX records (primary + data)
    pub ncx_indx: Vec<Vec<u8>>,
    /// DATP record
    pub datp: Vec<u8>,
    /// Number of flows (typically 2: HTML + CSS)
    pub flow_count: usize,
    /// CSS content that was appended as a separate flow
    #[allow(dead_code)]
    pub css_content: Vec<u8>,
    /// Uncompressed KF8 HTML bytes (the HTML flow only, without CSS).
    /// Exposed so the MOBI writer can run `html_check::validate_text_blob`
    /// on the assembled blob before compression/record splitting. This is
    /// a snapshot; the record bytes are derived from HTML+CSS combined.
    pub html_bytes: Vec<u8>,
}

/// Information about a skeleton (one per spine HTML file).
#[derive(Debug, Clone)]
struct SkeletonEntry {
    /// Label like "SKEL0000000000"
    label: String,
    /// Absolute byte offset in the combined KF8 text where this
    /// skeleton starts
    start_pos: usize,
    /// Byte length of this skeleton's XHTML shell (NOT including
    /// spliced fragment content)
    length: usize,
    /// Number of fragment chunks belonging to this skeleton
    chunk_count: usize,
}

/// Information about a fragment chunk.
#[derive(Debug, Clone)]
struct FragmentEntry {
    /// Insert position (absolute byte offset in combined text where
    /// the fragment bytes should be spliced into the skeleton to
    /// reconstruct the rendered page)
    insert_pos: usize,
    /// Base-32 CNCX selector string like `"P-//*[@aid='0']"`
    selector: String,
    /// Which skeleton (spine file) this fragment belongs to
    file_number: usize,
    /// Globally unique monotonic sequence number across all fragments
    /// in the book
    sequence_number: usize,
    /// Position of this fragment inside its skeleton's fragment
    /// payload block (i.e., offset relative to the first fragment of
    /// that skel). For a 1-fragment-per-skel comic page this is 0.
    start_pos: usize,
    /// Byte length of this fragment chunk
    length: usize,
}

/// Build the complete KF8 section for a book MOBI.
pub fn build_kf8_section(
    html_parts: &[String],
    css_content: &str,
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
    no_compress: bool,
) -> Kf8Section {
    // Step 1: Build the split skeletons + fragments from each HTML part.
    let (kf8_html, skeleton_entries, fragment_entries) =
        build_kf8_html(html_parts, href_to_recindex, spine_items);

    // Step 2: Append CSS as a separate flow.
    let css_bytes = css_content.as_bytes();
    let html_length = kf8_html.len();
    let total_text_length = html_length + css_bytes.len();

    let html_bytes_snapshot = kf8_html.clone();

    let mut combined_text = kf8_html;
    combined_text.extend_from_slice(css_bytes);

    // Step 3: Compress text into records.
    let (text_records, text_length) = if no_compress {
        split_text_uncompressed_kf8(&combined_text)
    } else {
        compress_text_kf8(&combined_text)
    };

    // Step 4: FDST record.
    let fdst = build_fdst(html_length, total_text_length);

    // Step 5: Skeleton INDX records.
    let skeleton_indx = build_skeleton_indx(&skeleton_entries);

    // Step 6: Fragment INDX records. Also builds the CNCX containing
    // the selector strings referenced by tag 2 of each fragment entry.
    let (fragment_indx, cncx_records) = build_fragment_indx_with_cncx(&fragment_entries);

    // Step 7: NCX INDX (minimal - one entry per skeleton).
    let ncx_indx = build_ncx_indx(html_parts.len().max(1));

    // Step 8: DATP record (stub).
    let datp = build_datp();

    let flow_count = if css_bytes.is_empty() { 1 } else { 2 };

    Kf8Section {
        text_records,
        text_length,
        fdst,
        fragment_indx,
        cncx_records,
        skeleton_indx,
        ncx_indx,
        datp,
        flow_count,
        css_content: css_bytes.to_vec(),
        html_bytes: html_bytes_snapshot,
    }
}

/// Build KF8 HTML with aid attributes, kindle:embed image URLs, and the
/// skeleton/fragment split.
///
/// Returns (combined_text_bytes, skeleton_entries, fragment_entries).
/// The combined text layout is `[skel_0][frag_0_0]...[skel_1][frag_1_0]...`.
fn build_kf8_html(
    html_parts: &[String],
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
) -> (Vec<u8>, Vec<SkeletonEntry>, Vec<FragmentEntry>) {
    let path_to_recindex = build_image_path_lookup(href_to_recindex, spine_items);

    let mut aid_counter: u32 = 0;
    let mut skeleton_entries: Vec<SkeletonEntry> = Vec::new();
    let mut fragment_entries: Vec<FragmentEntry> = Vec::new();
    let mut combined: Vec<u8> = Vec::new();
    let mut global_seq: usize = 0;

    for (skel_idx, raw_part) in html_parts.iter().enumerate() {
        // 1. Normalize this spine item into Calibre-style KF8 output:
        //    add aid attributes to aid-able tags and rewrite image src
        //    URLs to `kindle:embed:....`.
        let processed = process_kf8_part(raw_part, &mut aid_counter, &path_to_recindex);

        // 2. Split into (skeleton, body_inner). The body tag is left
        //    on the skeleton with its aid attribute intact, but the
        //    body's inner content moves into the fragment.
        let split = split_skeleton_and_body(&processed);
        let skel_bytes = split.skeleton.as_bytes();
        let body_inner = split.body_inner.as_bytes();

        // 3. Emit the skeleton into the combined text.
        let skel_start = combined.len();
        combined.extend_from_slice(skel_bytes);
        let skel_len = skel_bytes.len();

        // 4. Absolute insert position: where the body's inner bytes
        //    should be spliced back when reconstructing the page. That
        //    is `skel_start + split.body_inner_offset` (the byte offset
        //    of `</body>` inside the skeleton, i.e. exactly where the
        //    fragment content belongs in the final rendered document).
        let insert_pos = skel_start + split.body_inner_offset;

        // 5. Emit a single fragment that contains the body's inner
        //    content (everything between `<body ...>` and `</body>`).
        //    More complex pages could split the inner content further
        //    — for simple comic pages there is always one chunk.
        let frag_start = combined.len();
        combined.extend_from_slice(body_inner);
        let frag_len = body_inner.len();

        let _ = frag_start; // start position in combined text, not stored
        // Compute start_pos relative to the first fragment of this
        // skel's fragment block (which is `skel_start + skel_len`).
        // With one fragment per skel, this is 0.
        let relative_start = 0usize;

        fragment_entries.push(FragmentEntry {
            insert_pos,
            selector: format!("P-//*[@aid='{}']", split.body_aid),
            file_number: skel_idx,
            sequence_number: global_seq,
            start_pos: relative_start,
            length: frag_len,
        });
        global_seq += 1;

        skeleton_entries.push(SkeletonEntry {
            label: format!("SKEL{:010}", skel_idx),
            start_pos: skel_start,
            length: skel_len,
            chunk_count: 1,
        });
    }

    (combined, skeleton_entries, fragment_entries)
}

/// Output of `split_skeleton_and_body`.
struct SkelSplit {
    /// The XHTML shell. Body tag is present with its aid attribute and
    /// closing `</body></html>` suffix, but the body is empty.
    skeleton: String,
    /// The body's inner content (everything that was between `<body ...>`
    /// and `</body>` in the original processed HTML).
    body_inner: String,
    /// Byte offset inside `skeleton` where `body_inner` should be
    /// spliced back to reconstruct the original processed HTML.
    /// Points at the byte just after `<body ...>` (which is also the
    /// byte of `</body>` in the emptied shell).
    body_inner_offset: usize,
    /// The aid attribute value of the body tag, used to build the
    /// fragment's CNCX selector string.
    body_aid: String,
}

/// Split a processed HTML document into (skeleton_with_empty_body, body_inner).
///
/// The input must already have aid attributes applied and image src
/// rewrites done (i.e. be the output of `process_kf8_part`).
///
/// The split logic scans for the first `<body ...>` opening tag and the
/// LAST `</body>` closing tag, lifts everything between them into the
/// body_inner, and leaves the skeleton with an empty body.
fn split_skeleton_and_body(html: &str) -> SkelSplit {
    // Locate `<body` (start of body open tag) and its closing `>`.
    let bytes = html.as_bytes();
    let (open_start, open_end, body_aid) = match find_body_open(bytes) {
        Some(v) => v,
        None => {
            // No body tag: treat the whole thing as a skeleton with no
            // fragment content. This keeps dictionaries / fallback
            // callers safe even though they don't exercise this path.
            return SkelSplit {
                skeleton: html.to_string(),
                body_inner: String::new(),
                body_inner_offset: html.len(),
                body_aid: "0".to_string(),
            };
        }
    };

    // Locate the LAST `</body>` so nested body tags (shouldn't exist,
    // but just in case) don't trip us up.
    let close_start = match find_last_close_body(bytes) {
        Some(v) => v,
        None => {
            return SkelSplit {
                skeleton: html.to_string(),
                body_inner: String::new(),
                body_inner_offset: html.len(),
                body_aid: body_aid,
            };
        }
    };

    if close_start <= open_end {
        // Degenerate case: empty body. The skeleton is the whole doc,
        // and there is no fragment content.
        return SkelSplit {
            skeleton: html.to_string(),
            body_inner: String::new(),
            body_inner_offset: open_end,
            body_aid,
        };
    }

    let head = &html[..open_end]; // up to and including `<body ...>`
    let inner = &html[open_end..close_start];
    let tail = &html[close_start..]; // starts with `</body>`

    let mut skeleton = String::with_capacity(head.len() + tail.len());
    skeleton.push_str(head);
    skeleton.push_str(tail);

    SkelSplit {
        skeleton,
        body_inner: inner.to_string(),
        body_inner_offset: head.len(),
        body_aid,
    }
    .also(|_| {
        // dead-code trick to avoid unused open_start warning
        let _ = open_start;
    })
}

/// Tiny extension trait so we can append side-effecting closures without
/// rebinding — only used above to silence the unused `open_start` var.
trait Also: Sized {
    fn also<F: FnOnce(&Self)>(self, f: F) -> Self {
        f(&self);
        self
    }
}
impl<T> Also for T {}

/// Find the first `<body ...>` open tag. Returns (tag_start, after_tag_end, aid).
///
/// `tag_start` is the byte index of the `<`, `after_tag_end` is the byte
/// index just past the matching `>`, and `aid` is the value of the
/// `aid="..."` attribute on the tag (defaulting to "0" if absent).
fn find_body_open(bytes: &[u8]) -> Option<(usize, usize, String)> {
    let haystack = bytes;
    let needle = b"<body";
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if &haystack[i..i + needle.len()] == needle {
            // Verify next byte is whitespace or '>' (so we don't match
            // `<bodytext` or similar).
            let after = haystack[i + needle.len()];
            if after == b' '
                || after == b'\t'
                || after == b'\n'
                || after == b'\r'
                || after == b'>'
                || after == b'/'
            {
                // Find the closing '>' of this tag.
                let mut j = i + needle.len();
                while j < haystack.len() && haystack[j] != b'>' {
                    j += 1;
                }
                if j >= haystack.len() {
                    return None;
                }
                let tag_str = std::str::from_utf8(&haystack[i..=j]).ok()?;
                let aid = extract_aid_value(tag_str).unwrap_or_else(|| "0".to_string());
                return Some((i, j + 1, aid));
            }
        }
        i += 1;
    }
    None
}

/// Find the LAST `</body>` close tag. Returns byte index of the `<`.
fn find_last_close_body(bytes: &[u8]) -> Option<usize> {
    let needle = b"</body>";
    if bytes.len() < needle.len() {
        return None;
    }
    let mut i = bytes.len() - needle.len();
    loop {
        if &bytes[i..i + needle.len()] == needle {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

/// Extract the aid attribute value from a tag string like `<body aid="3">`.
fn extract_aid_value(tag_str: &str) -> Option<String> {
    // Handle both single and double-quoted attribute values, as well as
    // surrounding whitespace.
    let re = Regex::new(r#"\baid\s*=\s*["']([^"']*)["']"#).unwrap();
    re.captures(tag_str)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Process a single HTML part for KF8: add aid attributes and rewrite
/// image sources. This is Calibre's aid-able tag set + the standard
/// `kindle:embed:XXXX?mime=image/jpeg` src rewrite.
fn process_kf8_part(
    html: &str,
    aid_counter: &mut u32,
    path_to_recindex: &std::collections::HashMap<String, usize>,
) -> String {
    let mut result = html.to_string();

    // Rewrite image src to kindle:embed format.
    let src_re = Regex::new(r#"(?i)\bsrc\s*=\s*"([^"]*)""#).unwrap();
    result = src_re
        .replace_all(&result, |caps: &regex::Captures| {
            let src_path = caps.get(1).unwrap().as_str();
            if let Some(&recindex) = path_to_recindex.get(src_path) {
                format!(
                    "src=\"kindle:embed:{}?mime=image/jpeg\"",
                    encode_base32_4char(recindex)
                )
            } else if let Some(fname) = src_path.rsplit('/').next() {
                if let Some(&recindex) = path_to_recindex.get(fname) {
                    format!(
                        "src=\"kindle:embed:{}?mime=image/jpeg\"",
                        encode_base32_4char(recindex)
                    )
                } else {
                    caps.get(0).unwrap().as_str().to_string()
                }
            } else {
                caps.get(0).unwrap().as_str().to_string()
            }
        })
        .to_string();

    // Add aid attributes to block / inline tags that are in Calibre's
    // aid-able set. Calibre explicitly pops then re-adds aid so it ends
    // up last — we mimic that by appending the aid=".." attribute just
    // before the closing `>` of each tag, so we need a slightly more
    // careful regex here.
    let tag_re = Regex::new(
        r"(?i)<(p|div|h[1-6]|li|ul|ol|table|tr|td|th|section|article|aside|nav|header|footer|figure|figcaption|blockquote|img|span|a|em|strong|b|i|body)(\s[^>]*?)?(/?)>",
    )
    .unwrap();
    result = tag_re
        .replace_all(&result, |caps: &regex::Captures| {
            let tag = caps.get(1).unwrap().as_str();
            let attrs = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            let self_close = caps.get(3).map(|m| m.as_str()).unwrap_or("");
            // Skip if this tag already has an aid (e.g. double-processing).
            if attrs.contains(" aid=") || attrs.contains("\taid=") {
                return caps.get(0).unwrap().as_str().to_string();
            }
            let aid = encode_aid_base32(*aid_counter);
            *aid_counter += 1;
            format!("<{}{} aid=\"{}\"{}>", tag, attrs, aid, self_close)
        })
        .to_string();

    result
}

/// Encode an aid value as a lowercase base-32 string using Calibre's
/// alphabet (0-9, A-V). No zero padding — bare value, "0" for zero.
fn encode_aid_base32(value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = Vec::new();
    let mut v = value;
    while v > 0 {
        out.push(CHARS[(v & 0x1F) as usize]);
        v >>= 5;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// Encode a 1-based record index as 4 base-32 digits (0-9, A-V),
/// zero-padded. Used by `kindle:embed:XXXX?mime=...` image URLs.
fn encode_base32_4char(recindex: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut result = [b'0'; 4];
    let mut v = recindex;
    for i in (0..4).rev() {
        result[i] = CHARS[v % 32];
        v /= 32;
    }
    String::from_utf8(result.to_vec()).unwrap()
}

/// Build the path-to-recindex lookup map.
fn build_image_path_lookup(
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
) -> std::collections::HashMap<String, usize> {
    let mut path_to_recindex: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (href, &recindex) in href_to_recindex {
        path_to_recindex.insert(href.clone(), recindex);
        if let Some(fname) = href.rsplit('/').next() {
            path_to_recindex.entry(fname.to_string()).or_insert(recindex);
        }
    }

    for (_, spine_href) in spine_items {
        if let Some((spine_dir, _)) = spine_href.rsplit_once('/') {
            let _ = spine_dir;
            for (href, &recindex) in href_to_recindex {
                let relative = format!("../{}", href);
                path_to_recindex.entry(relative).or_insert(recindex);
            }
        }
    }

    path_to_recindex
}

/// Compress KF8 text into PalmDOC records with KF8 trailing bytes.
///
/// KF8 records use `extra_flags = 1` (bit 0 = multibyte only), so only
/// the 1-byte multibyte trailer is appended — no TBS byte.
fn compress_text_kf8(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();
    let chunk_size = RECORD_SIZE;

    let records: Vec<Vec<u8>> = text_bytes
        .chunks(chunk_size)
        .map(|chunk| {
            let mut compressed = palmdoc::compress(chunk);
            // KF8 trailer: multibyte byte only (no TBS).
            compressed.push(0x00);
            compressed
        })
        .collect();

    (records, total_length)
}

/// Split KF8 text into uncompressed records (debug path).
fn split_text_uncompressed_kf8(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();
    let chunk_size = RECORD_SIZE;

    let records: Vec<Vec<u8>> = text_bytes
        .chunks(chunk_size)
        .map(|chunk| {
            let mut rec = chunk.to_vec();
            // KF8 trailer: multibyte byte only (no TBS).
            rec.push(0x00);
            rec
        })
        .collect();

    (records, total_length)
}

/// Build the FDST (Flow Descriptor Table) record.
fn build_fdst(html_length: usize, total_length: usize) -> Vec<u8> {
    let flow_count = if total_length > html_length { 2 } else { 1 };

    let record_size = 12 + flow_count * 8;
    let mut fdst = Vec::with_capacity(record_size);
    fdst.extend_from_slice(b"FDST");
    fdst.extend_from_slice(&12u32.to_be_bytes()); // offset to entries
    fdst.extend_from_slice(&(flow_count as u32).to_be_bytes());

    // Flow 0: HTML
    fdst.extend_from_slice(&0u32.to_be_bytes());
    fdst.extend_from_slice(&(html_length as u32).to_be_bytes());

    // Flow 1: CSS (if present)
    if flow_count > 1 {
        fdst.extend_from_slice(&(html_length as u32).to_be_bytes());
        fdst.extend_from_slice(&(total_length as u32).to_be_bytes());
    }

    fdst
}

// --- INDX BUILDERS ---------------------------------------------------------
//
// These follow Calibre's writer8/index.py layout exactly. Each index has
// a list of `TagMeta` definitions giving (tag_number, values_per_entry,
// bitmask). For each entry, we compute a control byte that encodes
// which tags are present, then emit the entry as:
//
//     [ len_byte | label_bytes | control_byte | inverted_vwi_values... ]
//
// Entries are collected into a DATA record (with IDXT footer listing
// each entry's offset inside the record), and the HEADER record carries
// the TAGX table plus a geometry block with one entry per data record
// giving (last_label, entry_count) for binary-searching the right data
// record at read time.

#[derive(Clone, Copy, Debug)]
struct TagMeta {
    /// Tag number that will appear in raw MOBI dumps (not written, but
    /// kept for readability)
    #[allow(dead_code)]
    number: u8,
    /// Values per entry: how many inverted VWIs follow per "n"
    values_per_entry: u8,
    /// Bitmask for the control byte
    mask: u8,
}

const END_TAG: u8 = 1;

/// Calibre's `mask_to_bit_shifts` for 1-bit and 2-bit-wide masks.
fn mask_shifts(mask: u8) -> u32 {
    match mask {
        1 => 0,
        2 => 1,
        3 => 0,
        4 => 2,
        8 => 3,
        12 => 2,
        16 => 4,
        32 => 5,
        48 => 4,
        64 => 6,
        128 => 7,
        192 => 6,
        _ => 0,
    }
}

/// Build a TAGX section from a list of tag definitions.
///
/// Layout: `TAGX` + u32 length + u32 control_byte_count + per-tag 4 bytes
/// (number, vpe, mask, end_flag) + sentinel (0,0,0,1) end tag.
fn build_tagx(tag_defs: &[(u8, u8, u8)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (num, vpe, mask) in tag_defs {
        body.push(*num);
        body.push(*vpe);
        body.push(*mask);
        body.push(0); // end_flag = 0
    }
    // Sentinel end tag
    body.push(0);
    body.push(0);
    body.push(0);
    body.push(END_TAG);

    let total_length = 12 + body.len();
    let control_byte_count: u32 = 1;

    let mut out = Vec::with_capacity(total_length);
    out.extend_from_slice(b"TAGX");
    out.extend_from_slice(&(total_length as u32).to_be_bytes());
    out.extend_from_slice(&control_byte_count.to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Compute the control byte for an entry given the tag definitions and
/// the number of VWI values per tag for this entry.
fn control_byte_for(tag_defs: &[TagMeta], nvals_per_tag: &[usize]) -> u8 {
    let mut ans: u32 = 0;
    for (tag, &nvals) in tag_defs.iter().zip(nvals_per_tag.iter()) {
        let nentries = (nvals as u32) / (tag.values_per_entry as u32);
        let shifts = mask_shifts(tag.mask);
        ans |= (tag.mask as u32) & (nentries << shifts);
    }
    ans as u8
}

/// Encode an INDX data entry.
///
/// Layout: `[len(label_bytes)] [label_bytes] [control_byte] [vwi_inv values...]`
///
/// The label length is a single byte that mirrors Calibre's
/// `raw.insert(0, len(index_num))`. The control byte marks which tag
/// groups are present in `values`.
fn encode_indx_entry(
    label: &[u8],
    tag_defs: &[TagMeta],
    values_by_tag: &[Vec<u32>],
) -> Vec<u8> {
    assert_eq!(tag_defs.len(), values_by_tag.len());
    let nvals_per_tag: Vec<usize> = values_by_tag.iter().map(|v| v.len()).collect();
    let control = control_byte_for(tag_defs, &nvals_per_tag);

    let mut out = Vec::with_capacity(1 + label.len() + 1 + 8 * tag_defs.len());
    out.push(label.len() as u8);
    out.extend_from_slice(label);
    out.push(control);
    for vals in values_by_tag {
        for v in vals {
            out.extend_from_slice(&encode_vwi_inv(*v));
        }
    }
    out
}

/// Build a KF8 INDX data record (generation 1) wrapping the given
/// already-encoded entries. Follows Calibre's writer8/index.py layout:
///
///   header (192 bytes) + entry_bytes + align_to_4 + `IDXT` + u16 offsets
///
/// The header stores the IDXT offset at byte 20, entry count at byte 24,
/// and a marker `type=1` at byte 12 (Calibre's second-half of unknown1).
fn build_indx_data_record(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    header[0..4].copy_from_slice(b"INDX");
    put32(&mut header, 4, INDX_HEADER_LENGTH as u32);
    // offset 8..12: zeroes (unknown1 first half)
    put32(&mut header, 12, 1); // "type=1" marker for data record
    // offset 16..20: zeroes (index type; stays 0 for data records)

    let mut entries_data = Vec::new();
    let mut offsets: Vec<u16> = Vec::with_capacity(entries.len());
    for e in entries {
        let off = INDX_HEADER_LENGTH + entries_data.len();
        offsets.push(off as u16);
        entries_data.extend_from_slice(e);
    }

    // Align to 4 bytes before IDXT (Calibre's align_block).
    while (INDX_HEADER_LENGTH + entries_data.len()) % 4 != 0 {
        entries_data.push(0);
    }

    let idxt_offset = INDX_HEADER_LENGTH + entries_data.len();
    put32(&mut header, 20, idxt_offset as u32);
    put32(&mut header, 24, entries.len() as u32);
    // offset 28..36: 8 bytes of 0xFF (Calibre writes \xff*8)
    for b in &mut header[28..36] {
        *b = 0xFF;
    }

    let mut idxt = Vec::with_capacity(4 + 2 * offsets.len());
    idxt.extend_from_slice(b"IDXT");
    for o in &offsets {
        idxt.extend_from_slice(&o.to_be_bytes());
    }
    while idxt.len() % 4 != 0 {
        idxt.push(0);
    }

    let mut record = header;
    record.extend_from_slice(&entries_data);
    record.extend_from_slice(&idxt);
    record
}

/// Build a KF8 INDX primary/header record wrapping the given TAGX and
/// geometry (one entry per data record giving (last_label, count)).
///
/// Follows Calibre IndexHeader:
///   offset  4: header_length (192)
///   offset  8: zeroes(8)
///   offset 16: type = 2
///   offset 20: idxt_offset (set later)
///   offset 24: num_of_records
///   offset 28: encoding = 65001
///   offset 32: NULL (0xFFFFFFFF)
///   offset 36: num_of_entries
///   offset 40..52: zeroes (ordt/ligt/num_ordt)
///   offset 52: num_of_cncx
///   offset 56..180: zeroes(124)
///   offset 180: tagx_offset = 192
///   offset 184: zeroes(8)
fn build_indx_primary(
    tagx: &[u8],
    num_data_records: usize,
    num_entries: usize,
    num_cncx: usize,
    geometry: &[(Vec<u8>, u32)], // (last_label_bytes, entry_count_in_record)
) -> Vec<u8> {
    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    header[0..4].copy_from_slice(b"INDX");
    put32(&mut header, 4, INDX_HEADER_LENGTH as u32);
    // offset 8..16 zeroes
    put32(&mut header, 16, 2); // type = 2
    // idxt_offset at 20 — filled in later
    put32(&mut header, 24, num_data_records as u32);
    put32(&mut header, 28, 65001); // encoding = UTF-8
    put32(&mut header, 32, 0xFFFFFFFF); // NULL
    put32(&mut header, 36, num_entries as u32);
    // offset 40..52: zeroes
    put32(&mut header, 52, num_cncx as u32);
    // offset 56..180: zeroes
    put32(&mut header, 180, INDX_HEADER_LENGTH as u32); // tagx_offset
    // offset 184..192: zeroes

    // TAGX section starts at offset 192. Then per-record geometry
    // entries, each formatted as `[len][label][u16 count]`. Then IDXT
    // pointing to the start of each geometry entry.
    let mut tagx_block = tagx.to_vec();
    while tagx_block.len() % 4 != 0 {
        tagx_block.push(0);
    }

    let mut geom_block = Vec::new();
    let mut geom_offsets: Vec<u16> = Vec::with_capacity(geometry.len());
    let geom_base = INDX_HEADER_LENGTH + tagx_block.len();
    for (label, count) in geometry {
        geom_offsets.push((geom_base + geom_block.len()) as u16);
        geom_block.push(label.len() as u8);
        geom_block.extend_from_slice(label);
        geom_block.extend_from_slice(&(*count as u16).to_be_bytes());
    }
    while geom_block.len() % 4 != 0 {
        geom_block.push(0);
    }

    let idxt_offset = geom_base + geom_block.len();
    put32(&mut header, 20, idxt_offset as u32);

    let mut idxt = Vec::with_capacity(4 + 2 * geom_offsets.len());
    idxt.extend_from_slice(b"IDXT");
    for o in &geom_offsets {
        idxt.extend_from_slice(&o.to_be_bytes());
    }
    while idxt.len() % 4 != 0 {
        idxt.push(0);
    }

    let mut record = header;
    record.extend_from_slice(&tagx_block);
    record.extend_from_slice(&geom_block);
    record.extend_from_slice(&idxt);
    record
}

/// Build the skeleton INDX (primary + data).
///
/// Tag layout (Calibre SkelIndex):
///   tag 1 (chunk_count, vpe=1, mask=3) -- values = (n, n) [repeated twice]
///   tag 6 (geometry,    vpe=2, mask=12) -- values = (start_pos, length,
///                                                    start_pos, length)
fn build_skeleton_indx(skels: &[SkeletonEntry]) -> Vec<Vec<u8>> {
    if skels.is_empty() {
        return minimal_indx();
    }

    let tag_defs = [
        TagMeta { number: 1, values_per_entry: 1, mask: 3 },
        TagMeta { number: 6, values_per_entry: 2, mask: 12 },
    ];
    let tagx = build_tagx(&[(1, 1, 3), (6, 2, 12)]);

    let mut entries: Vec<Vec<u8>> = Vec::with_capacity(skels.len());
    for s in skels {
        let chunk_count_vals = vec![s.chunk_count as u32, s.chunk_count as u32];
        let geom_vals = vec![
            s.start_pos as u32,
            s.length as u32,
            s.start_pos as u32,
            s.length as u32,
        ];
        let entry = encode_indx_entry(
            s.label.as_bytes(),
            &tag_defs,
            &[chunk_count_vals, geom_vals],
        );
        entries.push(entry);
    }

    let data_record = build_indx_data_record(&entries);
    let last_label = skels.last().unwrap().label.as_bytes().to_vec();
    let primary = build_indx_primary(
        &tagx,
        1,
        skels.len(),
        0,
        &[(last_label, skels.len() as u32)],
    );

    vec![primary, data_record]
}

/// Build the fragment INDX (primary + data) and the CNCX records that
/// hold the aid selector strings referenced by tag 2 of each entry.
///
/// Tag layout (Calibre ChunkIndex):
///   tag 2 (cncx_offset,     vpe=1, mask=1)
///   tag 3 (file_number,     vpe=1, mask=2)
///   tag 4 (sequence_number, vpe=1, mask=4)
///   tag 6 (geometry,        vpe=2, mask=8) -- values = (start_pos, length)
fn build_fragment_indx_with_cncx(
    frags: &[FragmentEntry],
) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    if frags.is_empty() {
        return (minimal_indx(), Vec::new());
    }

    let tag_defs = [
        TagMeta { number: 2, values_per_entry: 1, mask: 1 },
        TagMeta { number: 3, values_per_entry: 1, mask: 2 },
        TagMeta { number: 4, values_per_entry: 1, mask: 4 },
        TagMeta { number: 6, values_per_entry: 2, mask: 8 },
    ];
    let tagx = build_tagx(&[(2, 1, 1), (3, 1, 2), (4, 1, 4), (6, 2, 8)]);

    // Build the CNCX from the fragment selectors. Deduped automatically
    // by CncxBuilder.
    let mut cncx = CncxBuilder::new();
    let cncx_offsets: Vec<u32> = frags.iter().map(|f| cncx.add(&f.selector)).collect();

    let mut entries: Vec<Vec<u8>> = Vec::with_capacity(frags.len());
    for (f, cncx_off) in frags.iter().zip(cncx_offsets.iter()) {
        // Label is the decimal insert position, 10 chars, zero-padded.
        // libmobi/Kindle parse this with strtoul to recover the byte
        // offset inside the final reconstructed page.
        let label = format!("{:010}", f.insert_pos);
        let label_bytes = label.as_bytes();

        let values: [Vec<u32>; 4] = [
            vec![*cncx_off],
            vec![f.file_number as u32],
            vec![f.sequence_number as u32],
            vec![f.start_pos as u32, f.length as u32],
        ];
        let entry = encode_indx_entry(label_bytes, &tag_defs, &values);
        entries.push(entry);
    }

    let cncx_records = cncx.into_records();
    let num_cncx = cncx_records.len();

    let data_record = build_indx_data_record(&entries);
    let last_label = format!("{:010}", frags.last().unwrap().insert_pos)
        .into_bytes();
    let primary = build_indx_primary(
        &tagx,
        1,
        frags.len(),
        num_cncx,
        &[(last_label, frags.len() as u32)],
    );

    (vec![primary, data_record], cncx_records)
}

/// Build the NCX INDX (minimal stub with one entry per spine file).
///
/// We don't emit a real ToC here — the NCX exists purely so libmobi /
/// Kindle have a non-0xFFFFFFFF index when parsing the header. A single
/// entry with offset 0 (tag 1) is enough to keep the file well-formed.
fn build_ncx_indx(num_files: usize) -> Vec<Vec<u8>> {
    let n = num_files.max(1);
    let tag_defs = [TagMeta { number: 1, values_per_entry: 1, mask: 1 }];
    let tagx = build_tagx(&[(1, 1, 1)]);

    let mut entries: Vec<Vec<u8>> = Vec::with_capacity(n);
    for i in 0..n {
        let label = format!("{:010X}", i);
        let values: [Vec<u32>; 1] = [vec![0u32]]; // offset = 0
        let entry = encode_indx_entry(label.as_bytes(), &tag_defs, &values);
        entries.push(entry);
    }

    let data_record = build_indx_data_record(&entries);
    let last_label = format!("{:010X}", n - 1).into_bytes();
    let primary = build_indx_primary(&tagx, 1, n, 0, &[(last_label, n as u32)]);
    vec![primary, data_record]
}

/// Build a DATP record (stub - minimal valid record).
fn build_datp() -> Vec<u8> {
    let mut datp = vec![0u8; 152];
    datp[0..4].copy_from_slice(b"DATP");
    put32(&mut datp, 4, 0x0000000D);
    datp
}

/// Build a minimal INDX pair for empty tables.
fn minimal_indx() -> Vec<Vec<u8>> {
    let tagx = build_tagx(&[(1, 1, 1)]);
    let data = build_indx_data_record(&[]);
    let primary = build_indx_primary(&tagx, 1, 0, 0, &[]);
    vec![primary, data]
}

/// Write a big-endian u32 into a byte buffer.
fn put32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

// Silence an unused import warning from `encode_vwi` — kept for
// symmetry with `encode_vwi_inv` so contributors discovering this
// module have both forms visible next to each other.
#[allow(dead_code)]
fn _vwi_import_keepalive(v: u32) -> Vec<u8> {
    encode_vwi(v)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_comic_page(aid_body: &str, img_src: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>Page</title></head><body aid=\"{}\"><div aid=\"1\"><img src=\"{}\"/></div></body></html>",
            aid_body, img_src
        )
    }

    #[test]
    fn base32_aid_encoder() {
        assert_eq!(encode_aid_base32(0), "0");
        assert_eq!(encode_aid_base32(1), "1");
        assert_eq!(encode_aid_base32(9), "9");
        assert_eq!(encode_aid_base32(10), "A");
        assert_eq!(encode_aid_base32(31), "V");
        assert_eq!(encode_aid_base32(32), "10");
        assert_eq!(encode_aid_base32(33), "11");
        assert_eq!(encode_aid_base32(63), "1V");
        assert_eq!(encode_aid_base32(1024), "100");
    }

    #[test]
    fn base32_4char_image_recindex() {
        assert_eq!(encode_base32_4char(0), "0000");
        assert_eq!(encode_base32_4char(1), "0001");
        assert_eq!(encode_base32_4char(32), "0010");
        assert_eq!(encode_base32_4char(1024), "0100");
    }

    #[test]
    fn find_body_open_and_close() {
        let html = make_comic_page("0", "img1.jpg");
        let (_, after_open, aid) = find_body_open(html.as_bytes()).expect("body open");
        assert_eq!(aid, "0");
        // `after_open` should point to just past `<body aid="0">`
        let head = &html[..after_open];
        assert!(head.ends_with("<body aid=\"0\">"));

        let close = find_last_close_body(html.as_bytes()).expect("body close");
        assert_eq!(&html[close..close + 7], "</body>");
    }

    #[test]
    fn split_skeleton_reconstructs_original() {
        let html = make_comic_page("0", "img1.jpg");
        let split = split_skeleton_and_body(&html);
        // Skeleton must have an empty body.
        assert!(split.skeleton.contains("<body aid=\"0\"></body>"));
        // body_inner_offset must point at the first byte after `<body aid="0">`.
        assert_eq!(&split.skeleton[split.body_inner_offset..split.body_inner_offset + 7], "</body>");
        // body_inner should be the original body contents.
        assert!(split.body_inner.starts_with("<div aid=\"1\">"));
        assert!(split.body_inner.contains("<img src=\"img1.jpg\"/>"));
        assert!(!split.body_inner.contains("<body"));
        // Reconstructing should give back the original HTML byte-for-byte.
        let mut rebuilt = String::new();
        rebuilt.push_str(&split.skeleton[..split.body_inner_offset]);
        rebuilt.push_str(&split.body_inner);
        rebuilt.push_str(&split.skeleton[split.body_inner_offset..]);
        assert_eq!(rebuilt, html);
    }

    #[test]
    fn global_sequence_number_monotonic_across_pages() {
        let parts: Vec<String> = (0..5)
            .map(|i| {
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html><head><title>P</title></head><body><div><img src=\"page{}.jpg\"/></div></body></html>",
                    i
                )
            })
            .collect();
        let href_to_recindex = std::collections::HashMap::new();
        let spine_items: Vec<(String, String)> = Vec::new();
        let (combined, skels, frags) =
            build_kf8_html(&parts, &href_to_recindex, &spine_items);
        assert_eq!(skels.len(), 5);
        assert_eq!(frags.len(), 5);
        // Global sequence numbers must be 0, 1, 2, 3, 4 in order.
        for (i, f) in frags.iter().enumerate() {
            assert_eq!(f.sequence_number, i, "seq #{i}");
            assert_eq!(f.file_number, i, "file_number #{i}");
        }
        // Each skeleton's start_pos should point at that skeleton inside
        // the combined buffer.
        for s in &skels {
            assert_eq!(
                &combined[s.start_pos..s.start_pos + s.length.min(6)],
                &b"<?xml "[..6]
            );
        }
        // Insert positions must fall inside the skeleton byte range.
        for (f, s) in frags.iter().zip(skels.iter()) {
            assert!(f.insert_pos >= s.start_pos);
            assert!(f.insert_pos <= s.start_pos + s.length);
        }
    }

    #[test]
    fn fragment_selector_uses_body_aid() {
        let parts = vec![make_comic_page("0", "img.jpg")];
        let href_to_recindex = std::collections::HashMap::new();
        let spine_items: Vec<(String, String)> = Vec::new();
        let (_, _, frags) = build_kf8_html(&parts, &href_to_recindex, &spine_items);
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].selector, "P-//*[@aid='0']");
    }

    #[test]
    fn process_kf8_part_adds_aid_to_body() {
        let html = "<html><head></head><body><div><img src=\"a.jpg\"/></div></body></html>";
        let mut counter = 0u32;
        let lookup = std::collections::HashMap::new();
        let out = process_kf8_part(html, &mut counter, &lookup);
        assert!(out.contains("<body"));
        assert!(out.contains("aid=\""));
    }

    #[test]
    fn control_byte_matches_calibre_skeleton() {
        // Calibre SkelIndex: tag 1 (mask=3, vpe=1) nvals=2, tag 6
        // (mask=12, vpe=2) nvals=4 → expected control byte 0x0A.
        let tag_defs = [
            TagMeta { number: 1, values_per_entry: 1, mask: 3 },
            TagMeta { number: 6, values_per_entry: 2, mask: 12 },
        ];
        assert_eq!(control_byte_for(&tag_defs, &[2, 4]), 0x0A);
    }

    #[test]
    fn control_byte_matches_calibre_chunk() {
        // Calibre ChunkIndex: 1+1+1+2 values, masks 1/2/4/8, all with
        // nentries=1 → expected control byte 0x0F.
        let tag_defs = [
            TagMeta { number: 2, values_per_entry: 1, mask: 1 },
            TagMeta { number: 3, values_per_entry: 1, mask: 2 },
            TagMeta { number: 4, values_per_entry: 1, mask: 4 },
            TagMeta { number: 6, values_per_entry: 2, mask: 8 },
        ];
        assert_eq!(control_byte_for(&tag_defs, &[1, 1, 1, 2]), 0x0F);
    }

    #[test]
    fn skeleton_indx_records_have_correct_magic_and_length() {
        let skels = vec![SkeletonEntry {
            label: "SKEL0000000000".to_string(),
            start_pos: 0,
            length: 100,
            chunk_count: 1,
        }];
        let recs = build_skeleton_indx(&skels);
        assert_eq!(recs.len(), 2);
        assert_eq!(&recs[0][0..4], b"INDX");
        assert_eq!(&recs[1][0..4], b"INDX");
        // Primary: header_length=192 at offset 4
        assert_eq!(u32::from_be_bytes(recs[0][4..8].try_into().unwrap()), 192);
        // Primary: type = 2 at offset 16
        assert_eq!(u32::from_be_bytes(recs[0][16..20].try_into().unwrap()), 2);
        // Primary: encoding = 65001 at offset 28
        assert_eq!(u32::from_be_bytes(recs[0][28..32].try_into().unwrap()), 65001);
        // Primary: tagx_offset = 192 at offset 180
        assert_eq!(u32::from_be_bytes(recs[0][180..184].try_into().unwrap()), 192);
        // Data record: "type=1" marker at offset 12
        assert_eq!(u32::from_be_bytes(recs[1][12..16].try_into().unwrap()), 1);
    }

    #[test]
    fn fragment_indx_emits_cncx_records() {
        let frags = vec![FragmentEntry {
            insert_pos: 201,
            selector: "P-//*[@aid='0']".to_string(),
            file_number: 0,
            sequence_number: 0,
            start_pos: 0,
            length: 42,
        }];
        let (recs, cncx) = build_fragment_indx_with_cncx(&frags);
        assert_eq!(recs.len(), 2);
        assert_eq!(cncx.len(), 1, "should emit one CNCX record");
        // CNCX record should embed the selector string
        assert!(
            cncx[0].windows(b"P-//*[@aid='0']".len()).any(|w| w == b"P-//*[@aid='0']"),
            "selector not found in CNCX record"
        );
        // Primary INDX header should report num_of_cncx = 1 at offset 52.
        assert_eq!(u32::from_be_bytes(recs[0][52..56].try_into().unwrap()), 1);
    }

    #[test]
    fn kf8_trailer_has_no_tbs_byte() {
        let text = b"<html><body>hello</body></html>";
        let (records, _) = compress_text_kf8(text);
        assert_eq!(records.len(), 1);
        // Trailer is only the multibyte marker 0x00 (no 0x81 TBS).
        let last = records[0].last().copied().unwrap();
        assert_eq!(last, 0x00, "trailer must be just the 0x00 multibyte byte");
        // And there must not be a 0x81 immediately before it (that would
        // indicate a TBS byte slipped in).
        let second_last = records[0][records[0].len() - 2];
        assert_ne!(second_last, 0x81, "TBS byte must not be appended for KF8");
    }

    #[test]
    fn build_kf8_section_records_are_well_formed() {
        // End-to-end sanity check: build a KF8 section for two simple
        // comic pages and verify all the top-level records exist and
        // have the correct magic bytes / tag counts.
        let parts = vec![
            make_comic_page("0", "p1.jpg"),
            make_comic_page("0", "p2.jpg"),
        ];
        let css = "body{margin:0}";
        let href_to_recindex = std::collections::HashMap::new();
        let spine_items: Vec<(String, String)> = Vec::new();
        let section = build_kf8_section(
            &parts,
            css,
            &href_to_recindex,
            &spine_items,
            true, // no_compress for deterministic byte counts
        );
        assert!(!section.text_records.is_empty());
        assert_eq!(&section.fdst[0..4], b"FDST");
        assert_eq!(section.flow_count, 2);
        assert_eq!(section.fragment_indx.len(), 2); // primary + data
        assert_eq!(section.skeleton_indx.len(), 2);
        assert_eq!(section.ncx_indx.len(), 2);
        assert_eq!(section.cncx_records.len(), 1, "one CNCX record for the deduped selector");
        // Skeleton and fragment primaries should declare 2 entries each.
        assert_eq!(
            u32::from_be_bytes(section.skeleton_indx[0][36..40].try_into().unwrap()),
            2
        );
        assert_eq!(
            u32::from_be_bytes(section.fragment_indx[0][36..40].try_into().unwrap()),
            2
        );
        // Fragment primary must reference one CNCX record at offset 52.
        assert_eq!(
            u32::from_be_bytes(section.fragment_indx[0][52..56].try_into().unwrap()),
            1
        );
        // Text records should NOT end in 0x81 (TBS dropped).
        for r in &section.text_records {
            assert_eq!(*r.last().unwrap(), 0x00);
        }
    }
}
