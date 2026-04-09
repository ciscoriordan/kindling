/// EXTH record building for MOBI format.
///
/// EXTH records store metadata (author, title, dictionary languages, etc.)
/// and the fontsignature (EXTH 300) which lists Unicode coverage of headwords.

use std::collections::HashSet;

/// Build a single EXTH record: type (u32 BE) + length (u32 BE) + data.
pub fn exth_record(rec_type: u32, data: &[u8]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(8 + data.len());
    rec.extend_from_slice(&rec_type.to_be_bytes());
    rec.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
    rec.extend_from_slice(data);
    rec
}

/// USB range -> bit mapping (from OS/2 OpenType spec).
/// (range_start, range_end, usb_index, bit)
const USB_RANGES: &[(u32, u32, usize, u32)] = &[
    (0x0020, 0x007F, 0, 0),  // Basic Latin
    (0x0080, 0x00FF, 0, 1),  // Latin-1 Supplement
    (0x0100, 0x024F, 0, 2),  // Latin Extended-A
    (0x0250, 0x02AF, 0, 3),  // Latin Extended-B
    (0x02B0, 0x02FF, 0, 5),  // Spacing Modifier Letters
    (0x0300, 0x036F, 0, 6),  // Combining Diacritical Marks
    (0x0370, 0x03FF, 0, 7),  // Greek and Coptic
    (0x0400, 0x04FF, 0, 9),  // Cyrillic
    (0x0530, 0x058F, 0, 10), // Armenian
    (0x0590, 0x05FF, 0, 11), // Hebrew
    (0x0600, 0x06FF, 0, 13), // Arabic
    (0x0E00, 0x0E7F, 0, 24), // Thai
    (0x10A0, 0x10FF, 0, 26), // Georgian
    (0x1100, 0x11FF, 0, 28), // Hangul Jamo
    (0x1E00, 0x1EFF, 0, 29), // Latin Extended Additional
    (0x1F00, 0x1FFF, 0, 30), // Greek Extended
    (0x2000, 0x206F, 0, 31), // General Punctuation
    (0x2070, 0x209F, 1, 0),  // Superscripts and Subscripts
    (0x20A0, 0x20CF, 1, 1),  // Currency Symbols
    (0x2100, 0x214F, 1, 3),  // Letterlike Symbols
    (0x2150, 0x218F, 1, 4),  // Number Forms
    (0x2190, 0x21FF, 1, 5),  // Arrows
    (0x2200, 0x22FF, 1, 6),  // Mathematical Operators
    (0x3000, 0x303F, 1, 20), // CJK Symbols and Punctuation
    (0x3040, 0x309F, 1, 21), // Hiragana
    (0x30A0, 0x30FF, 1, 22), // Katakana
    (0x3100, 0x312F, 1, 23), // Bopomofo
    (0x3130, 0x318F, 1, 24), // Hangul Compatibility Jamo
    (0x4E00, 0x9FFF, 1, 27), // CJK Unified Ideographs
    (0xAC00, 0xD7AF, 1, 28), // Hangul Syllables
    (0xFB00, 0xFB06, 1, 30), // Alphabetic Presentation Forms (Latin)
    (0xFB50, 0xFDFF, 1, 31), // Arabic Presentation Forms-A
    (0xFE70, 0xFEFF, 2, 0),  // Arabic Presentation Forms-B
];

/// Build EXTH 300 fontsignature from the set of codepoints in headwords.
///
/// Structure: USB[4] (16 bytes, LE) + CSB[2] (8 bytes, LE) + padding (8 bytes)
/// + sorted list of non-ASCII codepoints each stored as (cp + 0x0400) BE.
fn build_fontsignature(headword_chars: &HashSet<u32>) -> Vec<u8> {
    let mut usb = [0u32; 4];
    let mut csb = [0u32; 2];

    for &cp in headword_chars {
        for &(range_start, range_end, usb_idx, bit) in USB_RANGES {
            if cp >= range_start && cp <= range_end {
                usb[usb_idx] |= 1 << bit;
                break;
            }
        }
    }

    // Kindlegen always sets USB[3] bit 31 (reserved)
    usb[3] |= 1 << 31;

    // CSB: set code page bits based on character ranges present
    if usb[0] & (1 << 7) != 0 {
        // Greek and Coptic present
        csb[0] |= 0x00002000;
    }

    // USB and CSB stored as little-endian uint32 (Windows native)
    let mut header = Vec::with_capacity(32);
    for &v in &usb {
        header.extend_from_slice(&v.to_le_bytes());
    }
    for &v in &csb {
        header.extend_from_slice(&v.to_le_bytes());
    }

    // 8 bytes padding
    header.extend_from_slice(&[0u8; 8]);

    // Character list: 4-byte hash prefix + unique non-ASCII codepoints
    // shifted by +0x0400, big-endian
    let mut non_ascii: Vec<u16> = headword_chars
        .iter()
        .filter(|&&cp| cp > 0x7F)
        .map(|&cp| (cp + 0x0400) as u16)
        .collect();
    non_ascii.sort();

    // Hash prefix: compute MD5 of the codepoint bytes
    let mut cp_bytes = Vec::with_capacity(non_ascii.len() * 2);
    for &v in &non_ascii {
        cp_bytes.extend_from_slice(&v.to_be_bytes());
    }
    let cp_hash = md5_hash(&cp_bytes);

    // Sort the prefix bytes {BE, EC, ED, F4} by their hash-derived order
    let mut prefix_bytes = [0xBEu8, 0xEC, 0xED, 0xF4];
    prefix_bytes.sort_by_key(|&b| cp_hash[(b as usize) % cp_hash.len()]);

    let mut char_data = Vec::new();
    char_data.extend_from_slice(&prefix_bytes);
    for &v in &non_ascii {
        char_data.extend_from_slice(&v.to_be_bytes());
    }

    header.extend_from_slice(&char_data);
    header
}

/// Simple MD5 hash implementation (only needs 16 output bytes).
fn md5_hash(data: &[u8]) -> [u8; 16] {
    // Minimal MD5 for fontsignature prefix ordering.
    // We implement the full MD5 algorithm here.
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;

    // Padding
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    // Initial hash values
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xEFCDAB89;
    let mut c0: u32 = 0x98BADCFE;
    let mut d0: u32 = 0x10325476;

    // Per-round shift amounts
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20,
        5, 9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    // Pre-computed T table (floor(2^32 * abs(sin(i+1))) for i in 0..63)
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };

            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g])).rotate_left(S[i]),
            );
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.to_le_bytes());
    result
}

/// Fixed-layout metadata parsed from OPF.
pub struct FixedLayoutMeta {
    pub is_fixed_layout: bool,
    pub original_resolution: Option<String>,
    pub page_progression_direction: Option<String>,
}

/// Build an EXTH header for a regular book (non-dictionary).
///
/// Includes basic metadata (title, author, date) but skips dictionary-specific
/// records like language pairs (531/532), InMemory flag (547), and fontsignature (300).
/// If `cover_offset` is Some, sets EXTH 201 (CoverOffset) and 202 (ThumbOffset)
/// to the 0-based index of the cover image within the image records.
/// If `fixed_layout` is provided and indicates fixed-layout content, adds EXTH
/// records 122, 307, and 527.
/// If `hd_geometry` is Some, adds EXTH 536 with the HD image geometry string
/// (format: "WxH:start-end|").
///
/// ## Document type (`doc_type`)
/// Controls EXTH record 501 which determines where the book appears on Kindle:
/// - `Some("EBOK")`: appears under "Books". WARNING: Amazon may auto-delete
///   sideloaded EBOK files when the Kindle connects to WiFi, since it checks
///   whether the ASIN is in the user's purchase history.
/// - `Some("PDOC")` or `None`: appears under "Documents" (default, safe).
///   No cloud deletion risk for sideloaded content.
///
/// ## Series metadata
/// - `description`: EXTH 103, maps to ComicInfo.xml `<Summary>`.
/// - `subject`: EXTH 105, maps to ComicInfo.xml `<Genre>`.
/// - `series`: EXTH 112 (`calibre:series`), maps to ComicInfo.xml `<Series>`.
/// - `series_index`: EXTH 113 (`calibre:series_index`), maps to ComicInfo.xml `<Number>`.
///
/// Calibre and other readers use EXTH 112/113 to group books into series and
/// display volume numbering.
pub fn build_book_exth(
    title: &str,
    author: &str,
    date: &str,
    language: &str,
    cover_offset: Option<u32>,
    fixed_layout: Option<&FixedLayoutMeta>,
    kf8_boundary_record: Option<u32>,
    hd_geometry: Option<&str>,
    creator_tag: bool,
    doc_type: Option<&str>,
    description: Option<&str>,
    subject: Option<&str>,
    series: Option<&str>,
    series_index: Option<&str>,
) -> Vec<u8> {
    let mut records: Vec<Vec<u8>> = Vec::new();

    // Publishing date (106)
    if !date.is_empty() {
        records.push(exth_record(106, date.as_bytes()));
    }

    // Author (100)
    if !author.is_empty() {
        records.push(exth_record(100, author.as_bytes()));
    }

    // Description/summary (103) - maps to ComicInfo.xml <Summary>
    if let Some(desc) = description {
        if !desc.is_empty() {
            records.push(exth_record(103, desc.as_bytes()));
        }
    }

    // Subject/genre (105) - maps to ComicInfo.xml <Genre>
    if let Some(subj) = subject {
        if !subj.is_empty() {
            records.push(exth_record(105, subj.as_bytes()));
        }
    }

    // EXTH 542 - content-dependent 4-byte hash
    let title_bytes = if title.is_empty() {
        b"Book".to_vec()
    } else {
        title.as_bytes().to_vec()
    };
    let exth542_hash = md5_hash(&title_bytes);
    records.push(exth_record(542, &exth542_hash[..4]));

    // Language (524)
    if !language.is_empty() {
        records.push(exth_record(524, language.as_bytes()));
    }

    // Writing mode (525) - use horizontal-rl for RTL fixed-layout content
    let writing_mode = if fixed_layout
        .map(|fl| fl.page_progression_direction.as_deref() == Some("rtl"))
        .unwrap_or(false)
    {
        b"horizontal-rl" as &[u8]
    } else {
        b"horizontal-lr" as &[u8]
    };
    records.push(exth_record(525, writing_mode));

    // EXTH 131 (value 0)
    records.push(exth_record(131, &0u32.to_be_bytes()));

    // Creator software identity
    if creator_tag {
        records.push(exth_record(204, &300u32.to_be_bytes())); // platform = 300 (kindling)
        records.push(exth_record(205, &0u32.to_be_bytes()));
        records.push(exth_record(206, &2u32.to_be_bytes()));
        records.push(exth_record(535, b"kindling-0.2.0"));
    } else {
        records.push(exth_record(204, &201u32.to_be_bytes())); // platform = 201 (Mac)
        records.push(exth_record(205, &2u32.to_be_bytes()));
        records.push(exth_record(206, &9u32.to_be_bytes()));
        records.push(exth_record(535, b"0730-890adc2"));
    }

    // Creator build (207)
    records.push(exth_record(207, &0u32.to_be_bytes())); // build = 0

    // Cover image offset (201) and thumb offset (202)
    if let Some(offset) = cover_offset {
        records.push(exth_record(201, &offset.to_be_bytes()));
        records.push(exth_record(202, &offset.to_be_bytes()));
    }

    // Fixed-layout metadata
    if let Some(fl) = fixed_layout {
        if fl.is_fixed_layout {
            // EXTH 122: fixed-layout flag
            records.push(exth_record(122, b"true"));

            // EXTH 307: original-resolution
            let resolution = fl.original_resolution.as_deref().unwrap_or("1072x1448");
            records.push(exth_record(307, resolution.as_bytes()));

            // EXTH 527: page-progression-direction
            let ppd = fl.page_progression_direction.as_deref().unwrap_or("ltr");
            records.push(exth_record(527, ppd.as_bytes()));
        }
    }

    // Series name (112) - calibre:series, maps to ComicInfo.xml <Series>
    if let Some(s) = series {
        if !s.is_empty() {
            records.push(exth_record(112, s.as_bytes()));
        }
    }

    // Series index (113) - calibre:series_index, maps to ComicInfo.xml <Number>
    if let Some(si) = series_index {
        if !si.is_empty() {
            records.push(exth_record(113, si.as_bytes()));
        }
    }

    // Document type (501) - controls where the book appears on Kindle.
    // "EBOK" = Books shelf (WARNING: Amazon may auto-delete sideloaded EBOK
    //          content when the Kindle connects to WiFi, since it verifies
    //          the ASIN against the user's purchase history).
    // "PDOC" = Documents shelf (default, safe for sideloaded content).
    match doc_type {
        Some("EBOK") => {
            records.push(exth_record(501, b"EBOK"));
        }
        _ => {
            // PDOC is the default. We write it explicitly for clarity.
            records.push(exth_record(501, b"PDOC"));
        }
    }

    // EXTH 547 (InMemory)
    records.push(exth_record(547, b"InMemory"));

    // EXTH 125 (value 21 to match kindlegen)
    records.push(exth_record(125, &21u32.to_be_bytes()));

    // EXTH 121: KF8 boundary record (global index of KF8 Record 0)
    if let Some(boundary) = kf8_boundary_record {
        records.push(exth_record(121, &boundary.to_be_bytes()));
    }

    // EXTH 536: HD image geometry string (format: "WxH:start-end|")
    if let Some(geometry) = hd_geometry {
        records.push(exth_record(536, geometry.as_bytes()));
    }

    // Assemble EXTH
    let record_data: Vec<u8> = records.iter().flat_map(|r| r.iter().copied()).collect();
    let exth_length = 12 + record_data.len();
    let padding = (4 - (exth_length % 4)) % 4;
    let padded_length = exth_length + padding;

    let mut exth = Vec::with_capacity(padded_length);
    exth.extend_from_slice(b"EXTH");
    exth.extend_from_slice(&(padded_length as u32).to_be_bytes());
    exth.extend_from_slice(&(records.len() as u32).to_be_bytes());
    exth.extend_from_slice(&record_data);
    exth.extend_from_slice(&vec![0u8; padding]);

    exth
}

/// Build the complete EXTH header with metadata records for dictionaries.
///
/// Record order matches kindlegen output for maximum compatibility.
pub fn build_exth(
    title: &str,
    author: &str,
    date: &str,
    language: &str,
    dict_in_language: &str,
    dict_out_language: &str,
    headword_chars: &HashSet<u32>,
    creator_tag: bool,
) -> Vec<u8> {
    let mut records: Vec<Vec<u8>> = Vec::new();

    // Publishing date (106)
    if !date.is_empty() {
        records.push(exth_record(106, date.as_bytes()));
    }

    // Author (100)
    if !author.is_empty() {
        records.push(exth_record(100, author.as_bytes()));
    }

    // EXTH 542 - content-dependent 4-byte hash
    let title_bytes = if title.is_empty() {
        b"Dictionary".to_vec()
    } else {
        title.as_bytes().to_vec()
    };
    let exth542_hash = md5_hash(&title_bytes);
    records.push(exth_record(542, &exth542_hash[..4]));

    // Dictionary languages
    if !dict_in_language.is_empty() {
        records.push(exth_record(531, dict_in_language.as_bytes()));
    }
    if !dict_out_language.is_empty() {
        records.push(exth_record(532, dict_out_language.as_bytes()));
    }

    // Language (524)
    if !language.is_empty() {
        records.push(exth_record(524, language.as_bytes()));
    }

    // Writing mode (525)
    records.push(exth_record(525, b"horizontal-lr"));

    // EXTH 131 (value 0)
    records.push(exth_record(131, &0u32.to_be_bytes()));

    // EXTH 300 - fontsignature
    records.push(exth_record(300, &build_fontsignature(headword_chars)));

    // Creator software identity
    if creator_tag {
        records.push(exth_record(204, &300u32.to_be_bytes())); // platform = 300 (kindling)
        records.push(exth_record(205, &0u32.to_be_bytes()));
        records.push(exth_record(206, &2u32.to_be_bytes()));
        records.push(exth_record(535, b"kindling-0.2.0"));
    } else {
        records.push(exth_record(204, &201u32.to_be_bytes())); // platform = 201 (Mac)
        records.push(exth_record(205, &2u32.to_be_bytes()));
        records.push(exth_record(206, &9u32.to_be_bytes()));
        records.push(exth_record(535, b"0730-890adc2"));
    }

    // Creator build (207)
    records.push(exth_record(207, &0u32.to_be_bytes())); // build = 0

    // Dictionary in-memory flag
    records.push(exth_record(547, b"InMemory"));

    // EXTH 125 (value 1)
    records.push(exth_record(125, &1u32.to_be_bytes()));

    // Assemble EXTH
    let record_data: Vec<u8> = records.iter().flat_map(|r| r.iter().copied()).collect();
    let exth_length = 12 + record_data.len();
    let padding = (4 - (exth_length % 4)) % 4;
    let padded_length = exth_length + padding;

    let mut exth = Vec::with_capacity(padded_length);
    exth.extend_from_slice(b"EXTH");
    exth.extend_from_slice(&(padded_length as u32).to_be_bytes());
    exth.extend_from_slice(&(records.len() as u32).to_be_bytes());
    exth.extend_from_slice(&record_data);
    exth.extend_from_slice(&vec![0u8; padding]);

    exth
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Parse EXTH records from a raw EXTH block. Returns a vec of (type, data) pairs.
    fn parse_exth_records(exth: &[u8]) -> Vec<(u32, Vec<u8>)> {
        assert_eq!(&exth[0..4], b"EXTH");
        let _exth_len = u32::from_be_bytes([exth[4], exth[5], exth[6], exth[7]]) as usize;
        let rec_count = u32::from_be_bytes([exth[8], exth[9], exth[10], exth[11]]) as usize;
        let mut offset = 12;
        let mut records = Vec::new();
        for _ in 0..rec_count {
            let rec_type = u32::from_be_bytes([
                exth[offset], exth[offset + 1], exth[offset + 2], exth[offset + 3],
            ]);
            let rec_len = u32::from_be_bytes([
                exth[offset + 4], exth[offset + 5], exth[offset + 6], exth[offset + 7],
            ]) as usize;
            let data = exth[offset + 8..offset + rec_len].to_vec();
            records.push((rec_type, data));
            offset += rec_len;
        }
        records
    }

    /// Helper to find a record by type.
    fn find_record(records: &[(u32, Vec<u8>)], rec_type: u32) -> Option<Vec<u8>> {
        records.iter().find(|(t, _)| *t == rec_type).map(|(_, d)| d.clone())
    }

    #[test]
    fn test_exth_doc_type_pdoc_default() {
        let exth = build_book_exth(
            "Test Book", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            None,          // doc_type: None should produce PDOC
            None, None, None, None,
        );
        let records = parse_exth_records(&exth);
        let rec501 = find_record(&records, 501).expect("EXTH 501 should exist");
        assert_eq!(rec501, b"PDOC", "Default doc_type should be PDOC");
    }

    #[test]
    fn test_exth_doc_type_pdoc_explicit() {
        let exth = build_book_exth(
            "Test Book", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            Some("PDOC"),
            None, None, None, None,
        );
        let records = parse_exth_records(&exth);
        let rec501 = find_record(&records, 501).expect("EXTH 501 should exist");
        assert_eq!(rec501, b"PDOC");
    }

    #[test]
    fn test_exth_doc_type_ebok() {
        let exth = build_book_exth(
            "Test Book", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            Some("EBOK"),
            None, None, None, None,
        );
        let records = parse_exth_records(&exth);
        let rec501 = find_record(&records, 501).expect("EXTH 501 should exist");
        assert_eq!(rec501, b"EBOK", "doc_type EBOK should produce EXTH 501 = EBOK");
    }

    #[test]
    fn test_exth_series_metadata() {
        let exth = build_book_exth(
            "One Piece Vol 1", "Eiichiro Oda", "2026-01-01", "en",
            None, None, None, None, false,
            None,
            Some("Luffy begins his adventure"),  // description (103)
            Some("Manga, Adventure"),             // subject (105)
            Some("One Piece"),                    // series (112)
            Some("1"),                            // series_index (113)
        );
        let records = parse_exth_records(&exth);

        let desc = find_record(&records, 103).expect("EXTH 103 (description) should exist");
        assert_eq!(std::str::from_utf8(&desc).unwrap(), "Luffy begins his adventure");

        let subj = find_record(&records, 105).expect("EXTH 105 (subject) should exist");
        assert_eq!(std::str::from_utf8(&subj).unwrap(), "Manga, Adventure");

        let series = find_record(&records, 112).expect("EXTH 112 (series) should exist");
        assert_eq!(std::str::from_utf8(&series).unwrap(), "One Piece");

        let si = find_record(&records, 113).expect("EXTH 113 (series_index) should exist");
        assert_eq!(std::str::from_utf8(&si).unwrap(), "1");
    }

    #[test]
    fn test_exth_series_metadata_omitted_when_none() {
        let exth = build_book_exth(
            "Standalone Book", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            None, None, None, None, None,
        );
        let records = parse_exth_records(&exth);

        assert!(find_record(&records, 103).is_none(), "EXTH 103 should be absent when None");
        assert!(find_record(&records, 105).is_none(), "EXTH 105 should be absent when None");
        assert!(find_record(&records, 112).is_none(), "EXTH 112 should be absent when None");
        assert!(find_record(&records, 113).is_none(), "EXTH 113 should be absent when None");
    }

    #[test]
    fn test_exth_series_metadata_omitted_when_empty() {
        let exth = build_book_exth(
            "Standalone Book", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            None,
            Some(""),  // empty description
            Some(""),  // empty subject
            Some(""),  // empty series
            Some(""),  // empty series_index
        );
        let records = parse_exth_records(&exth);

        assert!(find_record(&records, 103).is_none(), "Empty description should not produce EXTH 103");
        assert!(find_record(&records, 105).is_none(), "Empty subject should not produce EXTH 105");
        assert!(find_record(&records, 112).is_none(), "Empty series should not produce EXTH 112");
        assert!(find_record(&records, 113).is_none(), "Empty series_index should not produce EXTH 113");
    }

    #[test]
    fn test_exth_header_structure() {
        // Verify the EXTH block has valid structure: magic, length, record count
        let exth = build_book_exth(
            "Test", "Author", "2026-01-01", "en",
            None, None, None, None, false,
            Some("EBOK"),
            Some("A test book"), Some("Fiction"), Some("Test Series"), Some("3"),
        );

        // Must start with "EXTH"
        assert_eq!(&exth[0..4], b"EXTH");

        // Length field must match actual length
        let stated_len = u32::from_be_bytes([exth[4], exth[5], exth[6], exth[7]]) as usize;
        assert_eq!(stated_len, exth.len(), "EXTH stated length must match actual length");

        // Length must be 4-byte aligned
        assert_eq!(exth.len() % 4, 0, "EXTH length must be 4-byte aligned");
    }

    #[test]
    fn test_exth_dict_unchanged() {
        // Verify build_exth (dictionary) still works without changes
        let mut chars = HashSet::new();
        chars.insert(0x0041); // A
        chars.insert(0x03B1); // alpha
        let exth = build_exth(
            "Test Dict", "Author", "2026-01-01", "en", "el", "en", &chars, false,
        );
        assert_eq!(&exth[0..4], b"EXTH");
        let records = parse_exth_records(&exth);
        // Should have dictionary language records
        assert!(find_record(&records, 531).is_some(), "Dict should have EXTH 531");
        assert!(find_record(&records, 532).is_some(), "Dict should have EXTH 532");
        // Should NOT have doc_type or series records
        assert!(find_record(&records, 501).is_none(), "Dict should not have EXTH 501");
        assert!(find_record(&records, 112).is_none(), "Dict should not have EXTH 112");
    }
}
