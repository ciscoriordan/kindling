/// INDX record building for MOBI dictionary index.
///
/// Builds 3 sub-indexes within the orth range:
///   Sub-index 1: Headword entries (TAGX tags 1, 2) with ORDT/SPL sort tables
///   Sub-index 2: Character mapping (TAGX tag 37)
///   Sub-index 3: "default" index name (TAGX tag 1)

use std::collections::HashSet;

use rayon::prelude::*;

use crate::vwi::encode_vwi_inv;

const INDX_HEADER_LENGTH: usize = 192;
const MAX_INDX_DATA_SIZE: usize = 64000;

/// ORDT/SPL sort tables extracted from kindlegen Greek dictionary output.
/// Embedded as static data (3906 bytes).
const ORDT_GREEK: &[u8] = include_bytes!("ordt_greek.bin");

/// A lookup term for the orth index.
#[derive(Debug)]
#[allow(dead_code)]
pub struct LookupTerm {
    pub label: String,
    pub label_bytes: Vec<u8>,
    pub start_pos: usize,
    pub text_len: usize,
    pub headword_display_len: usize,
    pub source_ordinal: usize,
}

/// Tag definition for TAGX section.
#[derive(Clone, Copy)]
struct TagDef {
    tag_id: u8,
    num_values: u8,
    mask: u8,
}

/// Encode a label string for use in an INDX entry.
///
/// Labels are always written as UTF-16BE. The primary INDX header declares
/// encoding `65002` (0xFDEA), which downstream parsers (iscc/mobi,
/// KindleUnpack, libmobi) interpret as a fixed 2-byte-per-character label
/// encoding. Storing ASCII labels as raw 1-byte-per-char bytes used to work
/// on Kindle firmware but crashed iscc/mobi whenever a label had an odd
/// byte count (e.g. "charlie" → 7 bytes), because the parser tried to
/// decode the trailing byte as a UTF-16BE code unit. See issue #5.
pub fn encode_indx_label(text: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len() * 2);
    for c in text.chars() {
        let cp = c as u32;
        if cp <= 0xFFFF {
            result.push((cp >> 8) as u8);
            result.push((cp & 0xFF) as u8);
        } else {
            // Surrogate pair for supplementary characters
            let adjusted = cp - 0x10000;
            let high = 0xD800 + (adjusted >> 10);
            let low = 0xDC00 + (adjusted & 0x3FF);
            result.push((high >> 8) as u8);
            result.push((high & 0xFF) as u8);
            result.push((low >> 8) as u8);
            result.push((low & 0xFF) as u8);
        }
    }
    result
}

/// Build all INDX records for the orthographic (dictionary) index.
///
/// Returns a list of record byte vectors:
/// [primary1, data1_1, ..., primary2, data2_1, primary3, data3_1]
pub fn build_orth_indx(
    lookup_terms: &[LookupTerm],
    headword_chars: &HashSet<char>,
    strict_accents: bool,
) -> Vec<Vec<u8>> {
    // --- Sub-index 1: Headword entries ---
    let tag_defs1 = [
        TagDef {
            tag_id: 1,
            num_values: 1,
            mask: 0x01,
        }, // start position
        TagDef {
            tag_id: 2,
            num_values: 1,
            mask: 0x02,
        }, // text length
    ];
    let tagx1 = build_tagx(&tag_defs1);

    // Encode every routing entry in parallel. `encode_indx_entry` is a pure
    // function of the label + tag_values (prefix compression is disabled, so
    // the `prev_label_bytes` argument is ignored for the output bytes). The
    // data-record chunking below is serial because it needs to track
    // running byte sizes, but the per-entry encoding was the dominant cost.
    let encoded_entries: Vec<Vec<u8>> = lookup_terms
        .par_iter()
        .map(|term| {
            let tag_values: [(u8, u32); 2] =
                [(1, term.start_pos as u32), (2, term.text_len as u32)];
            encode_indx_entry(&term.label_bytes, &[], &tag_values, &tag_defs1)
        })
        .collect();

    let mut data_records: Vec<Vec<u8>> = Vec::new();
    let mut current_entries: Vec<Vec<u8>> = Vec::new();
    let mut current_data_size: usize = 0;
    let mut last_labels: Vec<Vec<u8>> = Vec::new();
    let mut prev_label_bytes: Vec<u8> = Vec::new();

    let total_terms = lookup_terms.len();
    for (term_idx, (term, entry_bytes)) in lookup_terms
        .iter()
        .zip(encoded_entries.into_iter())
        .enumerate()
    {
        if term_idx % 500000 == 0 && term_idx > 0 {
            eprintln!(
                "  Encoded {} / {} INDX entries ({:.0}%)...",
                term_idx,
                total_terms,
                100.0 * term_idx as f64 / total_terms as f64
            );
        }

        let entry_overhead = entry_bytes.len() + 2;

        if current_data_size + entry_overhead > MAX_INDX_DATA_SIZE && !current_entries.is_empty() {
            let rec = build_indx_data_record(&current_entries);
            data_records.push(rec);
            last_labels.push(prev_label_bytes.clone());

            current_entries.clear();
            current_data_size = 0;
            prev_label_bytes.clear();

            // Re-encode without prefix reference
            // (No-op with prefix compression disabled: entry_bytes is already
            //  produced from an empty prev.  Reuse the already-encoded bytes.)
            current_entries.push(entry_bytes);
            current_data_size += entry_overhead;
        } else {
            current_entries.push(entry_bytes);
            current_data_size += entry_overhead;
        }
        prev_label_bytes = term.label_bytes.clone();
    }

    if !current_entries.is_empty() {
        let rec = build_indx_data_record(&current_entries);
        data_records.push(rec);
        last_labels.push(prev_label_bytes);
    }

    let primary1 = build_indx_primary(&tagx1, data_records.len(), lookup_terms.len(), &last_labels, 199, strict_accents);

    let mut sub1 = vec![primary1];
    sub1.extend(data_records);

    // --- Sub-index 2: Character mapping (unique headword chars) ---
    let tag_defs2 = [TagDef {
        tag_id: 37,
        num_values: 1,
        mask: 0x01,
    }];
    let tagx2 = build_tagx(&tag_defs2);

    let mut chars: Vec<char> = headword_chars.iter().copied().collect();
    chars.sort();

    let mut char_entries: Vec<Vec<u8>> = Vec::new();
    for ch in &chars {
        let mut label_bytes = Vec::new();
        let cp = *ch as u32;
        label_bytes.push((cp >> 8) as u8);
        label_bytes.push((cp & 0xFF) as u8);

        let tag_values = vec![(37u8, 0u32)];
        let entry = encode_indx_entry(&label_bytes, &[], &tag_values, &tag_defs2);
        char_entries.push(entry);
    }

    let char_data_rec = if char_entries.is_empty() {
        build_indx_data_record(&[])
    } else {
        build_indx_data_record(&char_entries)
    };

    let last_char_label = if let Some(ch) = chars.last() {
        let cp = *ch as u32;
        vec![(cp >> 8) as u8, (cp & 0xFF) as u8]
    } else {
        vec![]
    };

    let char_primary = build_indx_primary(&tagx2, 1, chars.len(), &[last_char_label], 192, false);
    let sub2 = vec![char_primary, char_data_rec];

    // --- Sub-index 3: "default" index name ---
    let tag_defs3 = [TagDef {
        tag_id: 1,
        num_values: 1,
        mask: 0x01,
    }];
    let tagx3 = build_tagx(&tag_defs3);

    let default_label = b"default".to_vec();
    let tag_values3 = vec![(1u8, 0u32)];
    let default_entry = encode_indx_entry(&default_label, &[], &tag_values3, &tag_defs3);
    let default_data_rec = build_indx_data_record(&[default_entry]);
    let default_primary = build_indx_primary(&tagx3, 1, 1, &[default_label], 192, false);
    let sub3 = vec![default_primary, default_data_rec];

    let total_sub1 = sub1.len();
    eprintln!(
        "  Sub-index 1: {} records ({} entries)",
        total_sub1,
        lookup_terms.len()
    );
    eprintln!("  Sub-index 2: {} records ({} chars)", sub2.len(), chars.len());
    eprintln!("  Sub-index 3: {} records (default)", sub3.len());

    let mut all = sub1;
    all.extend(sub2);
    all.extend(sub3);
    all
}

/// Build a TAGX section.
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

/// Encode a single INDX entry.
///
/// No prefix compression (kindlegen doesn't use it for dictionary entries).
/// Tag values are encoded using inverted VWI.
fn encode_indx_entry(
    label_bytes: &[u8],
    _prev_label_bytes: &[u8],
    tag_values: &[(u8, u32)],
    tag_defs: &[TagDef],
) -> Vec<u8> {
    // No prefix compression
    let prefix_len: u8 = 0;

    let new_len;
    let new_bytes: Vec<u8>;
    if label_bytes.len() > 31 {
        // Label too long for 5-bit field - truncate
        let max_len = if label_bytes.len() % 2 == 0 { 30 } else { 31 };
        new_bytes = label_bytes[..max_len].to_vec();
        new_len = max_len;
    } else {
        new_bytes = label_bytes.to_vec();
        new_len = label_bytes.len();
    }

    // First byte: prefix_len (3 bits) | new_label_len (5 bits)
    let byte0 = ((prefix_len & 0x07) << 5) | (new_len as u8 & 0x1F);

    // Control byte: which tags are present
    let mut control: u8 = 0;
    for td in tag_defs {
        if tag_values.iter().any(|(id, _)| *id == td.tag_id) {
            control |= td.mask;
        }
    }

    // Encode tag values as inverted VWI
    let mut tag_data = Vec::new();
    for td in tag_defs {
        if let Some((_, val)) = tag_values.iter().find(|(id, _)| *id == td.tag_id) {
            tag_data.extend_from_slice(&encode_vwi_inv(*val));
        }
    }

    let mut entry = Vec::with_capacity(1 + new_len + 1 + tag_data.len());
    entry.push(byte0);
    entry.extend_from_slice(&new_bytes);
    entry.push(control);
    entry.extend_from_slice(&tag_data);
    entry
}

/// Build an INDX data record containing multiple entries.
fn build_indx_data_record(entry_list: &[Vec<u8>]) -> Vec<u8> {
    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    // 'INDX'
    header[0..4].copy_from_slice(b"INDX");
    // header length
    put32(&mut header, 4, INDX_HEADER_LENGTH as u32);
    // index type = 0 (normal/orth)
    put32(&mut header, 8, 0);
    // generation = 1 (data record)
    put32(&mut header, 12, 1);

    // Compute entry offsets
    let mut entries_data = Vec::new();
    let mut offsets: Vec<u16> = Vec::new();

    for entry_bytes in entry_list {
        let offset = INDX_HEADER_LENGTH + entries_data.len();
        offsets.push(offset as u16);
        entries_data.extend_from_slice(entry_bytes);
    }

    // Build IDXT section
    let mut idxt = Vec::new();
    idxt.extend_from_slice(b"IDXT");
    for &off in &offsets {
        idxt.extend_from_slice(&off.to_be_bytes());
    }

    // Set header fields
    let entry_count = entry_list.len() as u32;
    let idxt_offset = (INDX_HEADER_LENGTH + entries_data.len()) as u32;
    put32(&mut header, 20, idxt_offset);
    put32(&mut header, 24, entry_count);
    put32(&mut header, 28, 0xFFFFFFFF); // index encoding (data recs)
    put32(&mut header, 32, 0xFFFFFFFF); // index language (data recs)

    // Assemble record
    let mut record = header;
    record.extend_from_slice(&entries_data);
    record.extend_from_slice(&idxt);

    // Pad to even length
    if record.len() % 2 != 0 {
        record.push(0x00);
    }

    record
}

/// Build the primary INDX header record.
///
/// For sub-index 1 (headwords), header_length=199 includes the embedded
/// "default" string. For sub-indexes 2 and 3, header_length=192.
///
/// `strict_accents` only matters for sub-index 1: when true, the ORDT/SPL
/// collation blob is omitted so Kindle falls back to raw UTF-16BE
/// ordering and exact-accent hits beat fuzzy ones on-device. Sub-indexes
/// 2 and 3 always pass false; they never embed the blob regardless.
fn build_indx_primary(
    tagx: &[u8],
    num_data_records: usize,
    total_entries: usize,
    last_labels: &[Vec<u8>],
    header_length: usize,
    strict_accents: bool,
) -> Vec<u8> {
    let embed_default = header_length == 199;
    let default_str: &[u8] = if embed_default { b"default" } else { b"" };

    let mut header = vec![0u8; INDX_HEADER_LENGTH];
    header[0..4].copy_from_slice(b"INDX");
    put32(&mut header, 4, header_length as u32);
    put32(&mut header, 8, 0); // index type = 0 (orth)
    put32(&mut header, 12, 0); // generation = 0 (primary)
    // Offset 16: kindlegen always writes 2 here
    put32(&mut header, 16, 2);
    // offset 20: IDXT offset (set below)
    put32(&mut header, 24, num_data_records as u32); // routing entry count
    put32(&mut header, 28, 0xFDEA); // index encoding
    put32(&mut header, 32, 8); // index language
    put32(&mut header, 36, total_entries as u32); // total entry count

    // offset 180 = 0xC0 (192, the fixed header portion)
    put32(&mut header, 180, INDX_HEADER_LENGTH as u32);

    // Offset where routing entries start (after header + default + TAGX)
    let entries_start = header_length + tagx.len();

    // Routing entries
    let mut routing_entries = Vec::new();
    let mut routing_offsets: Vec<u16> = Vec::new();

    for label_bytes in last_labels {
        let offset = entries_start + routing_entries.len();
        routing_offsets.push(offset as u16);

        let mut label_len = label_bytes.len().min(31);
        if label_bytes.len() % 2 == 0 && label_len % 2 != 0 {
            label_len -= 1;
        }
        let truncated = &label_bytes[..label_len];
        let byte0 = (label_len as u8) & 0x1F;
        routing_entries.push(byte0);
        routing_entries.extend_from_slice(truncated);
        routing_entries.push(0); // control byte = 0 (no tags)
    }

    // Build IDXT for routing entries
    let mut idxt = Vec::new();
    idxt.extend_from_slice(b"IDXT");
    for &off in &routing_offsets {
        idxt.extend_from_slice(&off.to_be_bytes());
    }

    // Set IDXT offset in header
    let idxt_offset = entries_start + routing_entries.len();
    put32(&mut header, 20, idxt_offset as u32);

    // Assemble: header + [default] + TAGX + routing entries + IDXT
    let mut record = header;
    record.extend_from_slice(default_str);
    record.extend_from_slice(tagx);
    record.extend_from_slice(&routing_entries);
    record.extend_from_slice(&idxt);

    // Pad to 4-byte boundary
    while record.len() % 4 != 0 {
        record.push(0x00);
    }

    // Append ORDT/SPL sort tables for the main headword sub-index (hdr=199).
    // `strict_accents` suppresses the embed so Kindle reverts to plain
    // UTF-16BE collation; exact-accented headwords then beat fuzzy matches
    // at lookup time (see the --strict-accents CLI flag).
    if embed_default && !strict_accents && !ORDT_GREEK.is_empty() {
        let ordt_start = record.len();
        record.extend_from_slice(ORDT_GREEK);

        // The ORDT blob contains: 2B padding + ORDT1(12B) + ORDT2(12B) +
        // SPL1-SPL6 sections. Find actual offsets by scanning for magic bytes.
        let mut ordt1_abs = ordt_start + 2;
        let mut ordt2_abs = ordt_start + 14;
        let mut spl1_abs = ordt_start + 26;
        let mut spl2_abs = ordt_start + 286;
        let mut spl3_abs = ordt_start + 546;
        let mut spl4_abs = ordt_start + 2870;
        let mut spl5_abs = ordt_start + 3130;
        let mut spl6_abs = ordt_start + 3390;

        // Scan for SPL magic bytes
        for i in ordt_start..record.len().saturating_sub(4) {
            let magic = &record[i..i + 4];
            match magic {
                b"SPL1" => spl1_abs = i,
                b"SPL2" => spl2_abs = i,
                b"SPL3" => spl3_abs = i,
                b"SPL4" => spl4_abs = i,
                b"SPL5" => spl5_abs = i,
                b"SPL6" => spl6_abs = i,
                _ => {}
            }
        }
        // Also find ORDT magic positions
        for i in ordt_start..ordt_start + 30 {
            if i + 4 <= record.len() && &record[i..i + 4] == b"ORDT" {
                if i == ordt_start + 2 || ordt1_abs == ordt_start + 2 {
                    ordt1_abs = i;
                    // Look for second ORDT
                    for j in (i + 4)..ordt_start + 30 {
                        if j + 4 <= record.len() && &record[j..j + 4] == b"ORDT" {
                            ordt2_abs = j;
                            break;
                        }
                    }
                    break;
                }
            }
        }

        // ORDT/ORDT2 pointers
        put32(&mut record, 164, 0); // ocnt = 0 (UTF-16BE mode)
        put32(&mut record, 168, 7); // oentries
        put32(&mut record, 172, ordt1_abs as u32); // ordt1 offset
        put32(&mut record, 176, ordt2_abs as u32); // ordt2 offset
        put32(&mut record, 184, 7); // name_len ("default")

        // SPL table pointers
        put32(&mut record, 56, 2); // spl_count
        put32(&mut record, 60, spl1_abs as u32);
        put32(&mut record, 64, spl2_abs as u32);
        put32(&mut record, 68, spl4_abs as u32);
        put32(&mut record, 72, spl5_abs as u32);
        put32(&mut record, 76, spl3_abs as u32);
        put32(&mut record, 80, spl6_abs as u32);

        // Constant collation parameters
        put32(&mut record, 84, 2317);
        put32(&mut record, 88, 65); // 'A'
        put32(&mut record, 92, 90); // 'Z'
        put32(&mut record, 96, 36);
        put32(&mut record, 100, 130);
        put32(&mut record, 104, 120);
        put32(&mut record, 108, 90);
        put32(&mut record, 112, 60);
        put32(&mut record, 116, 40);
        put32(&mut record, 120, 0xFFFFFFA6); // -90 as signed
        put32(&mut record, 124, 1);
        put32(&mut record, 128, 4);
        put32(&mut record, 132, 7);
        put32(&mut record, 136, 13);
        put32(&mut record, 140, 50);
        put32(&mut record, 144, 4);
    }

    record
}

/// Write a big-endian u32 into a byte buffer at a given offset.
fn put32(buf: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_be_bytes();
    buf[offset..offset + 4].copy_from_slice(&bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_indx_label_ascii_is_utf16be() {
        // Regression for issue #5: ASCII labels must be stored as UTF-16BE,
        // because the primary INDX header declares encoding 0xFDEA. Raw
        // 1-byte-per-char ASCII crashed iscc/mobi whenever a label had an
        // odd byte count (e.g. "charlie" = 7 bytes).
        assert_eq!(encode_indx_label("djed"),
                   vec![0x00, b'd', 0x00, b'j', 0x00, b'e', 0x00, b'd']);
        assert_eq!(encode_indx_label("charlie"),
                   vec![0x00, b'c', 0x00, b'h', 0x00, b'a', 0x00, b'r',
                        0x00, b'l', 0x00, b'i', 0x00, b'e']);
    }

    #[test]
    fn encode_indx_label_is_always_even_byte_count() {
        // The UTF-16BE invariant guarantees even-byte labels, which downstream
        // 2-byte-per-char parsers (iscc/mobi, KindleUnpack) require.
        for s in ["a", "ab", "abc", "abcdefg", "θάλασσα", "café", "日本語"] {
            assert_eq!(encode_indx_label(s).len() % 2, 0,
                "label {:?} must produce even byte count", s);
        }
    }

    #[test]
    fn encode_indx_label_non_bmp_uses_surrogate_pair() {
        // U+1F600 (GRINNING FACE) → surrogate pair D83D DE00 in UTF-16BE.
        let bytes = encode_indx_label("\u{1F600}");
        assert_eq!(bytes, vec![0xD8, 0x3D, 0xDE, 0x00]);
    }
}
