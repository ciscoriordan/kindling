/// KF8 (Kindle Format 8) dual-format support for book MOBIs.
///
/// Produces KF8 text records, FDST, skeleton/fragment/NCX INDX records,
/// and DATP record for the KF8 section of a dual KF7+KF8 MOBI file.

use regex::Regex;

use crate::palmdoc;
use crate::vwi::encode_vwi_inv;

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
}

/// Information about a skeleton chunk (one per source HTML file).
struct SkeletonEntry {
    /// Label like "SKEL0000000000"
    label: String,
    /// Byte offset in the KF8 text where this skeleton starts
    offset: usize,
    /// Length of this skeleton's HTML content
    length: usize,
    /// Number of fragments in this skeleton
    frag_count: usize,
}

/// Information about a fragment within a skeleton.
struct FragmentEntry {
    /// Label like "FRAG0000000000"
    label: String,
    /// Insert position (byte offset within the skeleton's text)
    insert_position: usize,
    /// File number (which skeleton this belongs to)
    file_number: usize,
    /// Sequence number within the skeleton
    _seq: usize,
}

/// Build the complete KF8 section for a book MOBI.
///
/// Takes the original HTML content parts (one per spine item), the CSS content,
/// and the image href-to-recindex mapping. Returns all KF8 records.
///
/// `image_recindex_offset` is the number of KF8-relative records before image records
/// (i.e., how many records are in the KF8 section before images start, minus 1 for
/// the 0-based indexing used in kindle:embed URLs).
pub fn build_kf8_section(
    html_parts: &[String],
    css_content: &str,
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
    no_compress: bool,
) -> Kf8Section {
    // Step 1: Build KF8 text with aid= attributes and kindle:embed image refs
    let (kf8_html, skeleton_entries, fragment_entries) =
        build_kf8_html(html_parts, href_to_recindex, spine_items);

    // Step 2: Append CSS as a separate flow
    let css_bytes = css_content.as_bytes();
    let html_length = kf8_html.len();
    let total_text_length = html_length + css_bytes.len();

    let mut combined_text = kf8_html;
    combined_text.extend_from_slice(css_bytes);

    // Step 3: Compress text into records
    let (text_records, text_length) = if no_compress {
        split_text_uncompressed_kf8(&combined_text)
    } else {
        compress_text_kf8(&combined_text)
    };

    // Step 4: Build FDST record
    let fdst = build_fdst(html_length, total_text_length);

    // Step 5: Build skeleton INDX
    let skeleton_indx = build_skeleton_indx(&skeleton_entries);

    // Step 6: Build fragment INDX
    let fragment_indx = build_fragment_indx(&fragment_entries);

    // Step 7: Build NCX INDX (minimal)
    let ncx_indx = build_ncx_indx(html_parts.len());

    // Step 8: Build DATP record (stub)
    let datp = build_datp();

    let flow_count = if css_bytes.is_empty() { 1 } else { 2 };

    Kf8Section {
        text_records,
        text_length,
        fdst,
        fragment_indx,
        skeleton_indx,
        ncx_indx,
        datp,
        flow_count,
        css_content: css_bytes.to_vec(),
    }
}

/// Build KF8 HTML with aid= attributes and kindle:embed image URLs.
///
/// Returns the combined HTML bytes, skeleton entries, and fragment entries.
fn build_kf8_html(
    html_parts: &[String],
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
) -> (Vec<u8>, Vec<SkeletonEntry>, Vec<FragmentEntry>) {
    let mut aid_counter: u32 = 0;
    let mut skeleton_entries = Vec::new();
    let mut fragment_entries = Vec::new();
    let mut combined_html = Vec::new();
    let mut global_frag_idx: usize = 0;

    // Build path lookup for images (same logic as rewrite_image_src in mobi.rs)
    let path_to_recindex = build_image_path_lookup(href_to_recindex, spine_items);

    // Process each HTML part as a skeleton
    for (skel_idx, part) in html_parts.iter().enumerate() {
        let skel_start = combined_html.len();

        // Add aid= attributes to HTML elements and rewrite image sources
        let processed = process_kf8_part(part, &mut aid_counter, &path_to_recindex);
        combined_html.extend_from_slice(processed.as_bytes());

        let skel_length = combined_html.len() - skel_start;

        // Each skeleton gets one fragment for now (simple case)
        fragment_entries.push(FragmentEntry {
            label: format!("FRAG{:010}", global_frag_idx),
            insert_position: 0, // Fragment at the beginning of the skeleton
            file_number: skel_idx,
            _seq: 0,
        });

        skeleton_entries.push(SkeletonEntry {
            label: format!("SKEL{:010}", skel_idx),
            offset: skel_start,
            length: skel_length,
            frag_count: 1,
        });

        global_frag_idx += 1;
    }

    (combined_html, skeleton_entries, fragment_entries)
}

/// Process a single HTML part for KF8: add aid= attributes, rewrite image sources.
fn process_kf8_part(
    html: &str,
    aid_counter: &mut u32,
    path_to_recindex: &std::collections::HashMap<String, usize>,
) -> String {
    let mut result = html.to_string();

    // Rewrite image src to kindle:embed format
    let src_re = Regex::new(r#"(?i)\bsrc\s*=\s*"([^"]*)""#).unwrap();
    result = src_re
        .replace_all(&result, |caps: &regex::Captures| {
            let src_path = caps.get(1).unwrap().as_str();
            if let Some(&recindex) = path_to_recindex.get(src_path) {
                format!("src=\"kindle:embed:{}?mime=image/jpeg\"", encode_base32(recindex))
            } else {
                // Try filename-only match
                if let Some(fname) = src_path.rsplit('/').next() {
                    if let Some(&recindex) = path_to_recindex.get(fname) {
                        format!("src=\"kindle:embed:{}?mime=image/jpeg\"", encode_base32(recindex))
                    } else {
                        caps.get(0).unwrap().as_str().to_string()
                    }
                } else {
                    caps.get(0).unwrap().as_str().to_string()
                }
            }
        })
        .to_string();

    // Add aid= attributes to block-level HTML elements
    // Match opening tags for common block/inline elements
    let tag_re = Regex::new(r"<(p|div|h[1-6]|li|ul|ol|table|tr|td|th|section|article|aside|nav|header|footer|figure|figcaption|blockquote|img|span|a|em|strong|b|i|body)(\s|>|/)").unwrap();
    result = tag_re
        .replace_all(&result, |caps: &regex::Captures| {
            let tag = caps.get(1).unwrap().as_str();
            let after = caps.get(2).unwrap().as_str();
            let aid = encode_aid(*aid_counter);
            *aid_counter += 1;
            format!("<{} aid=\"{}\"{}",tag, aid, after)
        })
        .to_string();

    result
}

/// Encode an aid value as a short string (base-62 or similar compact encoding).
/// KF8 uses short incrementing IDs like "0", "1", ..., "9", "A", "B", etc.
fn encode_aid(value: u32) -> String {
    if value == 0 {
        return "0".to_string();
    }
    // Use alphanumeric encoding: 0-9, A-Z, a-z (62 chars)
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let base = CHARS.len() as u32;
    let mut result = Vec::new();
    let mut v = value;
    while v > 0 {
        result.push(CHARS[(v % base) as usize]);
        v /= base;
    }
    result.reverse();
    String::from_utf8(result).unwrap()
}

/// Encode a 1-based record index as base-32 (0-9, A-V) for kindle:embed URLs.
/// The encoding is 4 characters, zero-padded.
fn encode_base32(recindex: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut result = [b'0'; 4];
    let mut v = recindex;
    for i in (0..4).rev() {
        result[i] = CHARS[v % 32];
        v /= 32;
    }
    String::from_utf8(result.to_vec()).unwrap()
}

/// Build the path-to-recindex lookup map (same as in mobi.rs but factored out here).
fn build_image_path_lookup(
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
) -> std::collections::HashMap<String, usize> {
    let mut path_to_recindex: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (href, &recindex) in href_to_recindex {
        path_to_recindex.insert(href.clone(), recindex);
        // Filename only
        if let Some(fname) = href.rsplit('/').next() {
            path_to_recindex.entry(fname.to_string()).or_insert(recindex);
        }
    }

    for (_, spine_href) in spine_items {
        if let Some((spine_dir, _)) = spine_href.rsplit_once('/') {
            for (href, &recindex) in href_to_recindex {
                let relative = format!("../{}", href);
                path_to_recindex.entry(relative).or_insert(recindex);
                let _ = spine_dir; // used for context
            }
        }
    }

    path_to_recindex
}

/// Compress KF8 text into PalmDOC records with KF8 trailing bytes.
fn compress_text_kf8(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();
    let chunk_size = RECORD_SIZE;

    let chunks: Vec<&[u8]> = text_bytes.chunks(chunk_size).collect();
    let records: Vec<Vec<u8>> = chunks
        .iter()
        .map(|chunk| {
            let mut compressed = palmdoc::compress(chunk);
            // Trailing bytes: TBS(0x81) then multibyte(0x00)
            compressed.push(0x81);
            compressed.push(0x00);
            compressed
        })
        .collect();

    (records, total_length)
}

/// Split KF8 text into uncompressed records.
fn split_text_uncompressed_kf8(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();
    let chunk_size = RECORD_SIZE;

    let records: Vec<Vec<u8>> = text_bytes
        .chunks(chunk_size)
        .map(|chunk| {
            let mut rec = chunk.to_vec();
            // Trailing bytes: TBS(0x81) then multibyte(0x00)
            rec.push(0x81);
            rec.push(0x00);
            rec
        })
        .collect();

    (records, total_length)
}

/// Build the FDST (Flow Descriptor Table) record.
///
/// For a typical book with HTML + CSS, there are 2 flows:
/// Flow 0 = HTML text (0..html_length)
/// Flow 1 = CSS content (html_length..total_length)
fn build_fdst(html_length: usize, total_length: usize) -> Vec<u8> {
    let flow_count = if total_length > html_length { 2 } else { 1 };

    // FDST header: magic(4) + mystery/offset(4) + entry_count(4)
    // Entries: pairs of (start: u32, end: u32)
    let record_size = 12 + flow_count * 8;
    let mut fdst = Vec::with_capacity(record_size);
    fdst.extend_from_slice(b"FDST");
    fdst.extend_from_slice(&12u32.to_be_bytes()); // offset to entries (always 12)
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

/// Build skeleton INDX records (primary + data).
///
/// One entry per source HTML file, mapping skeleton chunks.
fn build_skeleton_indx(skeletons: &[SkeletonEntry]) -> Vec<Vec<u8>> {
    if skeletons.is_empty() {
        // Return minimal stub
        return build_minimal_indx();
    }

    // TAGX for skeleton: tags 1=offset, 6=pair(length, frag_count)
    let tagx = build_tagx(&[
        TagDef { tag_id: 1, num_values: 1, mask: 0x01 }, // offset
        TagDef { tag_id: 6, num_values: 2, mask: 0x02 }, // pair: length, frag_count
    ]);

    // Build data entries
    let mut entries: Vec<Vec<u8>> = Vec::new();
    for skel in skeletons {
        let label_bytes = skel.label.as_bytes().to_vec();
        let tag_values = vec![
            (1u8, skel.offset as u32),
            (6u8, skel.length as u32),    // first value of pair
            (60u8, skel.frag_count as u32), // second value of pair (synthetic tag)
        ];
        let entry = encode_kf8_indx_entry(&label_bytes, &tag_values, true);
        entries.push(entry);
    }

    let data_record = build_kf8_indx_data_record(&entries);

    // Build primary record
    let last_label = skeletons.last().unwrap().label.as_bytes().to_vec();
    let primary = build_kf8_indx_primary(&tagx, 1, skeletons.len(), &[last_label]);

    vec![primary, data_record]
}

/// Build fragment INDX records (primary + data).
///
/// One entry per navigable fragment within skeletons.
fn build_fragment_indx(fragments: &[FragmentEntry]) -> Vec<Vec<u8>> {
    if fragments.is_empty() {
        return build_minimal_indx();
    }

    // TAGX for fragment: tags 2=insert_position, 6=pair(file_number, seq)
    let tagx = build_tagx(&[
        TagDef { tag_id: 2, num_values: 1, mask: 0x01 }, // insert position
        TagDef { tag_id: 6, num_values: 2, mask: 0x02 }, // pair: file_number, seq
    ]);

    let mut entries: Vec<Vec<u8>> = Vec::new();
    for frag in fragments {
        let label_bytes = frag.label.as_bytes().to_vec();
        let tag_values = vec![
            (2u8, frag.insert_position as u32),
            (6u8, frag.file_number as u32),     // first value of pair
            (60u8, frag._seq as u32),            // second value of pair
        ];
        let entry = encode_kf8_indx_entry(&label_bytes, &tag_values, true);
        entries.push(entry);
    }

    let data_record = build_kf8_indx_data_record(&entries);

    let last_label = fragments.last().unwrap().label.as_bytes().to_vec();
    let primary = build_kf8_indx_primary(&tagx, 1, fragments.len(), &[last_label]);

    vec![primary, data_record]
}

/// Build NCX INDX records (minimal - just one entry).
fn build_ncx_indx(num_files: usize) -> Vec<Vec<u8>> {
    // TAGX for NCX: tags 1=position, 6=pair(fragment_index, file_index)
    let tagx = build_tagx(&[
        TagDef { tag_id: 1, num_values: 1, mask: 0x01 }, // position
        TagDef { tag_id: 6, num_values: 2, mask: 0x02 }, // pair: fragment_index, file_index
    ]);

    let mut entries: Vec<Vec<u8>> = Vec::new();
    // One NCX entry per file
    for i in 0..num_files {
        let label = format!("{:010}", i);
        let label_bytes = label.as_bytes().to_vec();
        let tag_values = vec![
            (1u8, 0u32),        // position = 0 (start of file)
            (6u8, i as u32),    // fragment_index
            (60u8, i as u32),   // file_index
        ];
        let entry = encode_kf8_indx_entry(&label_bytes, &tag_values, true);
        entries.push(entry);
    }

    let data_record = build_kf8_indx_data_record(&entries);

    let last_label = format!("{:010}", num_files.saturating_sub(1));
    let primary = build_kf8_indx_primary(&tagx, 1, num_files, &[last_label.as_bytes().to_vec()]);

    vec![primary, data_record]
}

/// Build DATP record (stub - minimal valid record).
fn build_datp() -> Vec<u8> {
    let mut datp = vec![0u8; 152];
    datp[0..4].copy_from_slice(b"DATP");
    // Minimal DATP: magic + zeroed data
    // Set a small header value
    put32(&mut datp, 4, 0x0000000D); // matches kindlegen's pattern
    datp
}

/// Build a minimal INDX pair (primary + empty data record).
fn build_minimal_indx() -> Vec<Vec<u8>> {
    let tagx = build_tagx(&[
        TagDef { tag_id: 1, num_values: 1, mask: 0x01 },
    ]);
    let data_rec = build_kf8_indx_data_record(&[]);
    let primary = build_kf8_indx_primary(&tagx, 1, 0, &[]);
    vec![primary, data_rec]
}

// --- INDX building helpers for KF8 ---

#[derive(Clone, Copy)]
struct TagDef {
    tag_id: u8,
    num_values: u8,
    mask: u8,
}

/// Build a TAGX section for KF8 INDX records.
fn build_tagx(tag_defs: &[TagDef]) -> Vec<u8> {
    let mut tag_data = Vec::new();
    for td in tag_defs {
        tag_data.push(td.tag_id);
        tag_data.push(td.num_values);
        tag_data.push(td.mask);
        tag_data.push(0); // end_flag = 0
    }
    // End marker
    tag_data.extend_from_slice(&[0, 0, 0, 1]);

    let total_length = 12 + tag_data.len();
    let control_byte_count: u32 = 1;

    let mut result = Vec::with_capacity(total_length);
    result.extend_from_slice(b"TAGX");
    result.extend_from_slice(&(total_length as u32).to_be_bytes());
    result.extend_from_slice(&control_byte_count.to_be_bytes());
    result.extend_from_slice(&tag_data);
    result
}

/// Encode a KF8 INDX entry with label and tag values.
///
/// `has_pair` indicates whether there's a tag 6 pair (which encodes as two VWI values).
fn encode_kf8_indx_entry(
    label_bytes: &[u8],
    tag_values: &[(u8, u32)],
    has_pair: bool,
) -> Vec<u8> {
    let label_len = label_bytes.len().min(31);

    // First byte: prefix_len (3 bits) | new_label_len (5 bits)
    let byte0 = (label_len as u8) & 0x1F;

    // Control byte: which tag groups are present
    let mut control: u8 = 0;
    if tag_values.iter().any(|(id, _)| *id == 1 || *id == 2) {
        control |= 0x01; // tag 1 or 2 present
    }
    if has_pair && tag_values.iter().any(|(id, _)| *id == 6) {
        control |= 0x02; // tag 6 (pair) present
    }

    // Encode tag values as inverted VWI
    let mut tag_data = Vec::new();
    for (id, val) in tag_values {
        if *id == 60 {
            // Synthetic tag for second value of pair - just encode the value
            tag_data.extend_from_slice(&encode_vwi_inv(*val));
        } else if *id != 6 || !has_pair {
            tag_data.extend_from_slice(&encode_vwi_inv(*val));
        } else {
            // Tag 6 first value of pair
            tag_data.extend_from_slice(&encode_vwi_inv(*val));
        }
    }

    let mut entry = Vec::with_capacity(1 + label_len + 1 + tag_data.len());
    entry.push(byte0);
    entry.extend_from_slice(&label_bytes[..label_len]);
    entry.push(control);
    entry.extend_from_slice(&tag_data);
    entry
}

/// Build a KF8 INDX data record.
fn build_kf8_indx_data_record(entry_list: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    header[0..4].copy_from_slice(b"INDX");
    put32(&mut header, 4, INDX_HEADER_LENGTH as u32);
    put32(&mut header, 8, 0);  // index type
    put32(&mut header, 12, 1); // generation = 1 (data record)

    let mut entries_data = Vec::new();
    let mut offsets: Vec<u16> = Vec::new();

    for entry_bytes in entry_list {
        let offset = INDX_HEADER_LENGTH + entries_data.len();
        offsets.push(offset as u16);
        entries_data.extend_from_slice(entry_bytes);
    }

    // IDXT section
    let mut idxt = Vec::new();
    idxt.extend_from_slice(b"IDXT");
    for &off in &offsets {
        idxt.extend_from_slice(&off.to_be_bytes());
    }

    let entry_count = entry_list.len() as u32;
    let idxt_offset = (INDX_HEADER_LENGTH + entries_data.len()) as u32;
    put32(&mut header, 20, idxt_offset);
    put32(&mut header, 24, entry_count);
    put32(&mut header, 28, 0xFFFFFFFF);
    put32(&mut header, 32, 0xFFFFFFFF);

    let mut record = header;
    record.extend_from_slice(&entries_data);
    record.extend_from_slice(&idxt);

    // Pad to even length
    if record.len() % 2 != 0 {
        record.push(0x00);
    }

    record
}

/// Build a KF8 INDX primary record.
fn build_kf8_indx_primary(
    tagx: &[u8],
    num_data_records: usize,
    total_entries: usize,
    last_labels: &[Vec<u8>],
) -> Vec<u8> {
    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    header[0..4].copy_from_slice(b"INDX");
    put32(&mut header, 4, INDX_HEADER_LENGTH as u32);
    put32(&mut header, 8, 0);  // index type
    put32(&mut header, 12, 0); // generation = 0 (primary)
    put32(&mut header, 16, 2); // kindlegen always writes 2
    put32(&mut header, 24, num_data_records as u32);
    put32(&mut header, 28, 0xFDEA); // index encoding
    put32(&mut header, 32, 8);       // index language
    put32(&mut header, 36, total_entries as u32);
    put32(&mut header, 180, INDX_HEADER_LENGTH as u32);

    let entries_start = INDX_HEADER_LENGTH + tagx.len();

    // Routing entries
    let mut routing_entries = Vec::new();
    let mut routing_offsets: Vec<u16> = Vec::new();

    for label_bytes in last_labels {
        let offset = entries_start + routing_entries.len();
        routing_offsets.push(offset as u16);

        let label_len = label_bytes.len().min(31);
        let byte0 = (label_len as u8) & 0x1F;
        routing_entries.push(byte0);
        routing_entries.extend_from_slice(&label_bytes[..label_len]);
        routing_entries.push(0); // control byte = 0
    }

    // IDXT
    let mut idxt = Vec::new();
    idxt.extend_from_slice(b"IDXT");
    for &off in &routing_offsets {
        idxt.extend_from_slice(&off.to_be_bytes());
    }

    let idxt_offset = entries_start + routing_entries.len();
    put32(&mut header, 20, idxt_offset as u32);

    let mut record = header;
    record.extend_from_slice(tagx);
    record.extend_from_slice(&routing_entries);
    record.extend_from_slice(&idxt);

    // Pad to 4-byte boundary
    while record.len() % 4 != 0 {
        record.push(0x00);
    }

    record
}

/// Write a big-endian u32 into a byte buffer.
fn put32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
