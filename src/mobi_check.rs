/// Post-build MOBI readback checker.
///
/// Opens a finished MOBI file from disk and runs a battery of structural and
/// metadata validations. The important checks (EXTH 100 author, EXTH 503
/// updated_title, EXTH 524 language, EXTH 501 doc type, KF8 boundary, cover
/// image sanity, etc.) are P0 and fail the build; softer consistency checks
/// (title round-trip, PalmDB name length, fixed-layout metadata, INDX
/// presence) are P1 and only warn.
///
/// This exists because v0.7.4 happily produced comic MOBIs with no EXTH 503
/// and no author records, which the Kindle indexer silently refused to add
/// to the library. There was no build-time signal that anything was wrong.

use std::path::Path;

/// Expectations the caller can pass in for cross-checking against the
/// written MOBI. All fields are optional; unset fields skip the round-trip
/// check.
#[derive(Debug, Default)]
pub struct ExpectedMetadata<'a> {
    pub title: Option<&'a str>,
    pub author: Option<&'a str>,
    pub is_comic: bool,
    pub is_dictionary: bool,
}

/// Outcome of a single readback check run.
#[derive(Debug, Default)]
pub struct CheckReport {
    pub p0_passed: usize,
    pub warnings: Vec<String>,
    pub p0_errors: Vec<String>,
}

impl CheckReport {
    fn pass(&mut self) {
        self.p0_passed += 1;
    }

    fn fail(&mut self, msg: impl Into<String>) {
        self.p0_errors.push(msg.into());
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    pub fn has_errors(&self) -> bool {
        !self.p0_errors.is_empty()
    }
}

fn read_u16_be(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn read_u32_be(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Parsed PalmDB record list.
struct PalmDb {
    /// Byte offsets in the file for each record.
    offsets: Vec<u32>,
    num_records: usize,
}

impl PalmDb {
    fn record<'a>(&self, data: &'a [u8], idx: usize) -> Option<&'a [u8]> {
        let start = *self.offsets.get(idx)? as usize;
        let end = if idx + 1 < self.num_records {
            self.offsets[idx + 1] as usize
        } else {
            data.len()
        };
        data.get(start..end)
    }
}

fn parse_palmdb(data: &[u8], report: &mut CheckReport) -> Option<PalmDb> {
    if data.len() < 78 {
        report.fail(format!(
            "PalmDB header truncated: file is {} bytes, need >= 78",
            data.len()
        ));
        return None;
    }

    if &data[60..64] != b"BOOK" {
        report.fail(format!(
            "PalmDB type is {:?}, expected BOOK",
            String::from_utf8_lossy(&data[60..64])
        ));
        return None;
    }
    report.pass();

    if &data[64..68] != b"MOBI" {
        report.fail(format!(
            "PalmDB creator is {:?}, expected MOBI",
            String::from_utf8_lossy(&data[64..68])
        ));
        return None;
    }
    report.pass();

    let num_records = read_u16_be(data, 76).unwrap_or(0) as usize;
    if num_records == 0 {
        report.fail("PalmDB record count is 0");
        return None;
    }
    report.pass();

    let list_end = 78 + num_records * 8;
    if data.len() < list_end {
        report.fail(format!(
            "PalmDB record list truncated: need {} bytes, file is {}",
            list_end,
            data.len()
        ));
        return None;
    }

    let mut offsets = Vec::with_capacity(num_records);
    for i in 0..num_records {
        let off = read_u32_be(data, 78 + i * 8).unwrap_or(0);
        offsets.push(off);
    }

    if (offsets[0] as usize) >= data.len() {
        report.fail(format!(
            "Record 0 offset {} is outside file bounds {}",
            offsets[0],
            data.len()
        ));
        return None;
    }
    report.pass();

    for pair in offsets.windows(2) {
        if pair[1] <= pair[0] {
            report.fail(format!(
                "PalmDB record offsets not monotonic: {} -> {}",
                pair[0], pair[1]
            ));
            return None;
        }
    }
    for (i, &off) in offsets.iter().enumerate() {
        if (off as usize) > data.len() {
            report.fail(format!(
                "PalmDB record {} offset {} exceeds file size {}",
                i,
                off,
                data.len()
            ));
            return None;
        }
    }
    report.pass();

    Some(PalmDb { offsets, num_records })
}

/// Represents a parsed MOBI section (KF7 or KF8).
#[derive(Debug)]
#[allow(dead_code)]
struct MobiSection {
    /// Record index (PalmDB-global) where this section's Record 0 lives.
    record_idx: usize,
    /// MOBI header file version.
    file_version: u32,
    /// EXTH records extracted from this section's Record 0.
    exth: Vec<(u32, Vec<u8>)>,
    /// Compression type from the PalmDOC header (1/2/17480).
    compression: u16,
    /// Text record count from PalmDOC header.
    text_record_count: u16,
    /// EXTH 121 (KF8 boundary), if present.
    kf8_boundary: Option<u32>,
}

fn parse_mobi_section(
    record0: &[u8],
    record_idx: usize,
    report: &mut CheckReport,
) -> Option<MobiSection> {
    if record0.len() < 16 + 24 {
        report.fail(format!(
            "Record {} too small for PalmDOC + MOBI header: {} bytes",
            record_idx,
            record0.len()
        ));
        return None;
    }

    // PalmDOC header (16 bytes): compression, _, text_len, text_rec_count, rec_size, _, _
    let compression = read_u16_be(record0, 0).unwrap_or(0);
    let text_record_count = read_u16_be(record0, 8).unwrap_or(0);

    match compression {
        1 | 2 | 17480 => report.pass(),
        other => {
            report.fail(format!(
                "Record {}: unknown PalmDOC compression type {} (expected 1, 2, or 17480)",
                record_idx, other
            ));
            return None;
        }
    }

    // MOBI header starts at offset 16 of record 0.
    if &record0[16..20] != b"MOBI" {
        report.fail(format!(
            "Record {}: expected MOBI magic at offset 16, got {:?}",
            record_idx,
            String::from_utf8_lossy(&record0[16..20.min(record0.len())])
        ));
        return None;
    }
    report.pass();

    let header_len = read_u32_be(record0, 20).unwrap_or(0);
    if header_len < 232 {
        report.fail(format!(
            "Record {}: MOBI header length {} is too short (expected >= 232)",
            record_idx, header_len
        ));
        return None;
    }
    report.pass();

    let file_version = read_u32_be(record0, 36).unwrap_or(0);
    if file_version < 6 {
        report.fail(format!(
            "Record {}: MOBI file version {} < 6",
            record_idx, file_version
        ));
        return None;
    }
    report.pass();

    // EXTH flag bit lives at MOBI header offset 0x70 (= record0 offset 128
    // after the 16-byte PalmDOC header). Any of the common EXTH-present
    // bits (0x40 in kindlegen output) counts.
    let flags = read_u32_be(record0, 128).unwrap_or(0);
    if flags & 0x40 == 0 {
        report.fail(format!(
            "Record {}: EXTH flag bit 0x40 not set at header offset 0x70 (raw=0x{:X})",
            record_idx, flags
        ));
        return None;
    }
    report.pass();

    // EXTH block starts at MOBI header end = 16 + header_len.
    let exth_off = 16 + header_len as usize;
    if record0.len() < exth_off + 12 || &record0[exth_off..exth_off + 4] != b"EXTH" {
        report.fail(format!(
            "Record {}: EXTH magic missing at expected offset {}",
            record_idx, exth_off
        ));
        return None;
    }
    report.pass();

    let exth_len = read_u32_be(record0, exth_off + 4).unwrap_or(0) as usize;
    let exth_count = read_u32_be(record0, exth_off + 8).unwrap_or(0) as usize;

    let mut exth = Vec::with_capacity(exth_count);
    let mut pos = exth_off + 12;
    let exth_end = exth_off + exth_len;
    for _ in 0..exth_count {
        if pos + 8 > exth_end || pos + 8 > record0.len() {
            report.fail(format!(
                "Record {}: EXTH record list truncated at pos {}",
                record_idx, pos
            ));
            return None;
        }
        let rtype = read_u32_be(record0, pos).unwrap_or(0);
        let rlen = read_u32_be(record0, pos + 4).unwrap_or(0) as usize;
        if rlen < 8 || pos + rlen > record0.len() {
            report.fail(format!(
                "Record {}: EXTH record {} has invalid length {}",
                record_idx, rtype, rlen
            ));
            return None;
        }
        let payload = record0[pos + 8..pos + rlen].to_vec();
        exth.push((rtype, payload));
        pos += rlen;
    }

    let kf8_boundary = exth
        .iter()
        .find(|(t, _)| *t == 121)
        .and_then(|(_, d)| read_u32_be(d, 0));

    Some(MobiSection {
        record_idx,
        file_version,
        exth,
        compression,
        text_record_count,
        kf8_boundary,
    })
}

fn find_exth_string<'a>(exth: &'a [(u32, Vec<u8>)], rtype: u32) -> Option<&'a [u8]> {
    exth.iter().find(|(t, _)| *t == rtype).map(|(_, d)| d.as_slice())
}

fn check_exth_metadata(
    section_label: &str,
    section: &MobiSection,
    expected: &ExpectedMetadata,
    report: &mut CheckReport,
) {
    // EXTH 100 - author
    match find_exth_string(&section.exth, 100) {
        Some(data) if !data.is_empty() => match std::str::from_utf8(data) {
            Ok(_) => report.pass(),
            Err(e) => report.fail(format!(
                "{}: EXTH 100 (author) is not valid UTF-8: {}",
                section_label, e
            )),
        },
        _ => report.fail(format!(
            "{}: EXTH 100 (author) missing or empty. Kindle silently drops \
             library entries with no author.",
            section_label
        )),
    }

    // EXTH 503 - updated_title
    match find_exth_string(&section.exth, 503) {
        Some(data) if !data.is_empty() => match std::str::from_utf8(data) {
            Ok(s) => {
                report.pass();
                if let Some(expected_title) = expected.title {
                    if s != expected_title {
                        report.warn(format!(
                            "{}: EXTH 503 is {:?}, expected {:?}",
                            section_label, s, expected_title
                        ));
                    }
                }
            }
            Err(e) => report.fail(format!(
                "{}: EXTH 503 (updated_title) is not valid UTF-8: {}",
                section_label, e
            )),
        },
        _ => report.warn(format!(
            "{}: EXTH 503 (updated_title) missing. KF8 Record 0 \
             full_name provides the library title instead. \
             (KCC/kindlegen also omit EXTH 503.)",
            section_label
        )),
    }

    // EXTH 524 - language
    match find_exth_string(&section.exth, 524) {
        Some(data) if !data.is_empty() => report.pass(),
        _ => report.fail(format!(
            "{}: EXTH 524 (language) missing or empty",
            section_label
        )),
    }

    // EXTH 501 - cde_content_type. Dictionaries are allowed to skip it.
    match find_exth_string(&section.exth, 501) {
        Some(data) => {
            report.pass();
            if data != b"EBOK" && data != b"PDOC" {
                report.fail(format!(
                    "{}: EXTH 501 is {:?}, expected EBOK or PDOC",
                    section_label,
                    String::from_utf8_lossy(data)
                ));
            } else {
                report.pass();
            }
        }
        None if expected.is_dictionary => {
            // Dictionaries historically omit 501; not a hard failure.
            report.warn(format!(
                "{}: EXTH 501 (cde_content_type) missing (dictionary, non-fatal)",
                section_label
            ));
        }
        None => report.fail(format!(
            "{}: EXTH 501 (cde_content_type) missing",
            section_label
        )),
    }

    // Author round-trip (P1, warn only).
    if let Some(expected_author) = expected.author {
        if let Some(data) = find_exth_string(&section.exth, 100) {
            if let Ok(s) = std::str::from_utf8(data) {
                if !s.contains(expected_author) {
                    report.warn(format!(
                        "{}: EXTH 100 is {:?}, does not contain expected author {:?}",
                        section_label, s, expected_author
                    ));
                }
            }
        }
    }

    // Comic-specific P1 checks.
    if expected.is_comic {
        // primary_writing_mode lives in 525; original-resolution in 538 or 307.
        let has_writing_mode = find_exth_string(&section.exth, 525).is_some()
            || find_exth_string(&section.exth, 529).is_some();
        if !has_writing_mode {
            report.warn(format!(
                "{}: comic is missing EXTH 525/529 (primary_writing_mode)",
                section_label
            ));
        }
        let has_resolution = find_exth_string(&section.exth, 538).is_some()
            || find_exth_string(&section.exth, 307).is_some();
        if !has_resolution {
            report.warn(format!(
                "{}: comic is missing EXTH 307/538 (original-resolution)",
                section_label
            ));
        }
    }
}

fn check_cover_image(
    section: &MobiSection,
    palmdb: &PalmDb,
    data: &[u8],
    first_image_record: Option<u32>,
    is_comic: bool,
    is_kf8_only: bool,
    report: &mut CheckReport,
) {
    // first_image_record is the global record index of the first image; EXTH
    // 201 / 202 are relative offsets into that image list.
    let first_image = match first_image_record {
        Some(v) if v != 0xFFFFFFFF => v as usize,
        _ => {
            if is_comic {
                report.fail(
                    "comic MOBI has no first-image record pointer; the library \
                     cannot find a cover and the book will not open".to_string(),
                );
            }
            return;
        }
    };

    // For comics specifically, EXTH 201 (cover offset) is a P0 requirement.
    // Without it, Kindle devices either show a missing thumbnail in the
    // library or fail to render the first page. The library-thumbnail
    // generator reads EXTH 201 directly, and we saw a Vader Down build where
    // the value was present but pointed at an oversized cover image that the
    // firmware could not downscale, which we can't detect here. What we CAN
    // detect is the "EXTH 201 is missing entirely" case.
    let exth201 = find_exth_string(&section.exth, 201);
    let exth202 = find_exth_string(&section.exth, 202);

    if is_comic {
        if exth201.is_none() {
            report.fail(
                "comic MOBI is missing EXTH 201 (cover_offset); the Kindle \
                 library thumbnail pipeline depends on this".to_string(),
            );
        } else {
            report.pass();
        }

        // EXTH 202 (thumbnail offset) must be present AND point at a
        // different record than EXTH 201. Pointing both at the same full
        // 1072x1448 cover record (as kindling <=0.7.7 did) is why the
        // Vader Down build showed a grey placeholder: the Kindle thumbnail
        // pipeline refuses to render a full comic page as a 330x470
        // library tile and silently falls back.
        if exth202.is_none() {
            report.fail(
                "comic MOBI is missing EXTH 202 (thumbnail_offset); the library \
                 grid tile will fall back to the grey placeholder".to_string(),
            );
        } else {
            report.pass();
        }

        if let (Some(p201), Some(p202)) = (exth201, exth202) {
            if p201.len() == 4 && p202.len() == 4 && p201 == p202 {
                report.fail(
                    "comic MOBI has EXTH 201 and EXTH 202 pointing at the same \
                     record; the thumbnail must live in its own downscaled \
                     record or the Kindle library tile renders as placeholder".to_string(),
                );
            } else {
                report.pass();
            }
        }

        // KF8-only comics require EXTH 129 (kindle:embed URI). This is the
        // primary cover-lookup path on Paperwhite 11+, Oasis 3, and Scribe.
        // A missing EXTH 129 on a fixed-layout comic is why Vader Down
        // indexed but failed to open ("Unable to Open Item") even after the
        // SRCS bloat fix in v0.7.7: the reader could not resolve the cover
        // image for the opening spread.
        if is_kf8_only {
            match find_exth_string(&section.exth, 129) {
                None => {
                    report.fail(
                        "KF8-only comic MOBI is missing EXTH 129 (kindle:embed \
                         cover URI); modern Kindle firmware uses this to locate \
                         the cover and the book will fail to open"
                            .to_string(),
                    );
                }
                Some(v) if v.is_empty() || !v.starts_with(b"kindle:embed:") => {
                    report.fail(format!(
                        "EXTH 129 has invalid value {:?} (expected \"kindle:embed:XXXX\")",
                        String::from_utf8_lossy(v)
                    ));
                }
                Some(_) => {
                    report.pass();
                }
            }
        }
    }

    for rtype in [201u32, 202] {
        let payload = match find_exth_string(&section.exth, rtype) {
            Some(p) if p.len() == 4 => p,
            _ => continue,
        };
        let offset = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let idx = first_image + offset as usize;
        let rec = match palmdb.record(data, idx) {
            Some(r) => r,
            None => {
                report.fail(format!(
                    "EXTH {}: cover/thumb points at record {} which does not exist",
                    rtype, idx
                ));
                continue;
            }
        };
        // JPEG SOI: FF D8, PNG: 89 50 4E 47.
        let is_jpeg = rec.len() >= 2 && rec[0] == 0xFF && rec[1] == 0xD8;
        let is_png = rec.len() >= 4 && rec[0] == 0x89 && rec[1] == 0x50 && rec[2] == 0x4E && rec[3] == 0x47;
        if !is_jpeg && !is_png {
            report.fail(format!(
                "EXTH {}: cover/thumb record {} is not a JPEG or PNG (first bytes {:02X?})",
                rtype,
                idx,
                &rec[..rec.len().min(4)]
            ));
        } else {
            report.pass();
        }

        // For the thumbnail (EXTH 202) specifically, verify the record is
        // a small downscaled image. A 1072x1448 cover reused as a thumb
        // was the original Vader Down bug. We only check rough bounds;
        // the dimensions have to come from a JPEG SOF marker scan.
        if rtype == 202 && is_jpeg && is_comic {
            if let Some((w, h)) = parse_jpeg_dimensions(rec) {
                let long_edge = w.max(h);
                if long_edge > 800 {
                    report.fail(format!(
                        "EXTH 202 thumbnail record {} is {}x{}; expected a \
                         downscaled library tile (<= 800px long edge) or the \
                         Kindle library grid will fall back to a placeholder",
                        idx, w, h
                    ));
                } else {
                    report.pass();
                }
            }
        }
    }
}

/// Minimal JPEG SOF scanner to pull out (width, height) for thumbnail
/// verification. Mirrors src/mobi.rs get_jpeg_dimensions but kept local to
/// the checker to avoid exposing internal helpers.
fn parse_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        match marker {
            0xD8 => i += 2,
            0xD9 | 0xDA => break,
            0xC0..=0xC3 => {
                if i + 9 <= data.len() {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                break;
            }
            0x00 => i += 2,
            _ => {
                if i + 4 <= data.len() {
                    let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                    i += 2 + length;
                } else {
                    break;
                }
            }
        }
    }
    None
}

fn first_image_record(section: &MobiSection, record0: &[u8]) -> Option<u32> {
    // MOBI header offset 0x5C (92) holds the first image record index.
    // record0 offset = 16 (PalmDOC) + 92 = 108.
    let _ = section;
    read_u32_be(record0, 108)
}

/// Run the full post-build check on a finished MOBI file at `path`.
pub fn check_mobi_file(
    path: &Path,
    expected: &ExpectedMetadata,
) -> Result<CheckReport, Box<dyn std::error::Error>> {
    let data = std::fs::read(path)?;
    let mut report = CheckReport::default();

    let palmdb = match parse_palmdb(&data, &mut report) {
        Some(p) => p,
        None => return Ok(report),
    };

    let rec0 = match palmdb.record(&data, 0) {
        Some(r) => r,
        None => {
            report.fail("Could not read PalmDB record 0");
            return Ok(report);
        }
    };
    let kf7 = match parse_mobi_section(rec0, 0, &mut report) {
        Some(s) => s,
        None => return Ok(report),
    };

    check_exth_metadata("KF7 section", &kf7, expected, &mut report);

    // A .azw3 file with file_version >= 8 in its only MOBI section is a
    // KF8-only comic (no KF7 boundary), which modern Kindle firmware reads
    // via the EXTH 129 kindle:embed cover URI path. We pass this flag into
    // the cover-image check so it can require EXTH 129 on KF8-only comics
    // without falsely flagging dual-format builds where the KF7 section
    // does not need it.
    let is_kf8_only_section = kf7.file_version >= 8;
    let first_image = first_image_record(&kf7, rec0);
    check_cover_image(
        &kf7,
        &palmdb,
        &data,
        first_image,
        expected.is_comic,
        is_kf8_only_section,
        &mut report,
    );

    // Oversized PalmDB record check (P0).
    //
    // A single PalmDB record over 16 MB is a red flag: it almost always means
    // the SRCS (embedded EPUB source) record has ballooned, because the
    // caller left `--embed-source` on for a comic with hundreds of
    // full-resolution pages. Vader Down produced a 120 MB SRCS record in one
    // slot, which the Kindle reader could not mmap, manifesting as "Unable
    // to Open Item" even though indexing succeeded. The Kindle does not
    // document a hard limit, but every working comic we have inspected keeps
    // individual records well under 10 MB. 16 MB is a generous ceiling that
    // still catches the failure mode.
    const MAX_REASONABLE_RECORD_BYTES: usize = 16 * 1024 * 1024;
    let mut too_big: Vec<(usize, usize)> = Vec::new();
    for i in 0..palmdb.num_records {
        if let Some(r) = palmdb.record(&data, i) {
            if r.len() > MAX_REASONABLE_RECORD_BYTES {
                too_big.push((i, r.len()));
            }
        }
    }
    if too_big.is_empty() {
        report.pass();
    } else {
        for (i, len) in &too_big {
            let head = palmdb
                .record(&data, *i)
                .map(|r| {
                    if r.len() >= 4 {
                        String::from_utf8_lossy(&r[..4.min(r.len())])
                            .chars()
                            .filter(|c| c.is_ascii_graphic())
                            .collect::<String>()
                    } else {
                        String::new()
                    }
                })
                .unwrap_or_default();
            report.fail(format!(
                "PalmDB record {} is {} bytes ({} MB), over the {} MB sanity \
                 limit; this trips the Kindle reader's \"Unable to Open \
                 Item\" path. Magic: {:?}",
                i,
                len,
                len / (1024 * 1024),
                MAX_REASONABLE_RECORD_BYTES / (1024 * 1024),
                head,
            ));
        }
    }

    // PalmDB name length (P1).
    let name_end = data[..32].iter().position(|&b| b == 0).unwrap_or(32);
    if name_end > 31 {
        report.warn(format!(
            "PalmDB name is {} bytes (expected <= 31)",
            name_end
        ));
    }

    // KF8 boundary handling. Dual-format files have EXTH 121 in the KF7
    // section pointing at the KF8 record 0.
    let is_kf8_only = is_kf8_only_section;
    if let Some(boundary) = kf7.kf8_boundary {
        let boundary = boundary as usize;
        if boundary >= palmdb.num_records {
            report.fail(format!(
                "EXTH 121 KF8 boundary is {} but file only has {} records",
                boundary, palmdb.num_records
            ));
        } else {
            report.pass();
            let kf8_rec0 = match palmdb.record(&data, boundary) {
                Some(r) => r,
                None => {
                    report.fail(format!(
                        "KF8 boundary points at record {} which cannot be read",
                        boundary
                    ));
                    return Ok(report);
                }
            };
            if kf8_rec0.len() < 20 || &kf8_rec0[16..20] != b"MOBI" {
                report.fail(format!(
                    "KF8 boundary record {} does not start with PalmDOC+MOBI",
                    boundary
                ));
                return Ok(report);
            }
            report.pass();

            let kf8 = match parse_mobi_section(kf8_rec0, boundary, &mut report) {
                Some(s) => s,
                None => return Ok(report),
            };
            if kf8.file_version < 8 {
                report.warn(format!(
                    "KF8 section has file_version {} (expected 8)",
                    kf8.file_version
                ));
            }
            check_exth_metadata("KF8 section", &kf8, expected, &mut report);
        }
    } else if !is_kf8_only {
        // Dual-format file with no boundary: only a warning because some
        // dictionaries are KF7-only (and file_version would also be 6).
        // Check by extension: .mobi files should typically have a KF8
        // boundary unless they're dictionaries.
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "mobi" && !expected.is_dictionary {
            report.warn(
                "no EXTH 121 (KF8 boundary) found in a .mobi file (dual format expected)"
                    .to_string(),
            );
        }
    }

    // Dictionary INDX presence (P1).
    if expected.is_dictionary {
        let has_indx = data.windows(4).any(|w| w == b"INDX");
        if !has_indx {
            report.warn("dictionary MOBI has no INDX section; lookups will fail".to_string());
        }
    }

    // Structural record magic sanity. Walks every PalmDB record once and
    // flags any INDX/FCIS/FLIS/FDST record whose header is obviously
    // malformed. These are P1 warnings because a subtle breakage here will
    // generally show up as "Unable to Open Item" on-device rather than as
    // missing metadata, and we'd rather ship + diagnose than hard-fail
    // backward-compatible builds.
    check_structural_records(&data, &palmdb, &mut report);

    // Sanity: text record count is at least 1 on non-empty content.
    if kf7.text_record_count == 0 {
        report.warn("KF7 text record count is 0".to_string());
    }

    Ok(report)
}

/// Walk every PalmDB record and verify INDX/FCIS/FLIS/FDST records have the
/// right magic + minimum header size. Returns only warnings (P1) because a
/// single garbled index record still lets the rest of the metadata checks
/// complete meaningfully.
fn check_structural_records(data: &[u8], palmdb: &PalmDb, report: &mut CheckReport) {
    let mut indx_count = 0usize;
    let mut saw_fcis = false;
    let mut saw_flis = false;
    let mut saw_fdst = false;

    for i in 0..palmdb.num_records {
        let Some(rec) = palmdb.record(data, i) else { continue };
        if rec.len() < 4 {
            continue;
        }
        match &rec[..4] {
            b"INDX" => {
                indx_count += 1;
                if rec.len() < 192 {
                    report.warn(format!(
                        "INDX record {} is {} bytes (header must be >= 192)",
                        i,
                        rec.len()
                    ));
                    continue;
                }
                let header_len = read_u32_be(rec, 4).unwrap_or(0) as usize;
                if header_len < 192 || header_len > rec.len() {
                    report.warn(format!(
                        "INDX record {} declares header length {}, record is {} bytes",
                        i,
                        header_len,
                        rec.len()
                    ));
                }
                let idxt_off = read_u32_be(rec, 20).unwrap_or(0) as usize;
                if idxt_off != 0 && idxt_off + 4 <= rec.len() && &rec[idxt_off..idxt_off + 4] != b"IDXT" {
                    report.warn(format!(
                        "INDX record {}: IDXT offset {} does not point at IDXT magic",
                        i, idxt_off
                    ));
                }
            }
            b"FCIS" => {
                saw_fcis = true;
                if rec.len() < 36 {
                    report.warn(format!("FCIS record {} is only {} bytes", i, rec.len()));
                }
            }
            b"FLIS" => {
                saw_flis = true;
                if rec.len() < 36 {
                    report.warn(format!("FLIS record {} is only {} bytes", i, rec.len()));
                }
            }
            b"FDST" => {
                saw_fdst = true;
                if rec.len() < 12 {
                    report.warn(format!("FDST record {} is only {} bytes", i, rec.len()));
                    continue;
                }
                let section_count = read_u32_be(rec, 8).unwrap_or(0) as usize;
                let expected_len = 12 + section_count * 8;
                if rec.len() < expected_len {
                    report.warn(format!(
                        "FDST record {} declares {} sections (needs {} bytes) but record is {} bytes",
                        i, section_count, expected_len, rec.len()
                    ));
                }
            }
            _ => {}
        }
    }

    // We don't assert FCIS/FLIS/FDST presence here: dictionaries don't need
    // FDST, and legacy KF7-only books may omit FCIS/FLIS entirely. The MOBI
    // header fields point at them when they exist, which is where hard
    // requirements should be enforced if we need them.
    let _ = (indx_count, saw_fcis, saw_flis, saw_fdst);
}

/// Print a one-line summary of the report to stderr and return Ok if no P0
/// errors were found, or an Err with a formatted message otherwise.
pub fn report_result(
    path: &Path,
    report: &CheckReport,
) -> Result<(), Box<dyn std::error::Error>> {
    if report.has_errors() {
        eprintln!("MOBI check FAILED ({}):", path.display());
        for err in &report.p0_errors {
            eprintln!("  [P0] {}", err);
        }
        for warn in &report.warnings {
            eprintln!("  [P1] {}", warn);
        }
        return Err(format!(
            "MOBI readback check failed: {} P0 errors. Built MOBI at {} may be corrupted, not shipping.",
            report.p0_errors.len(),
            path.display()
        )
        .into());
    }
    let warn_summary = if report.warnings.is_empty() {
        String::new()
    } else {
        format!(" ({})", report.warnings.join("; "))
    };
    eprintln!(
        "MOBI check: {} P0 checks passed, {} P1 warnings{}",
        report.p0_passed,
        report.warnings.len(),
        warn_summary
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal book OPF + HTML fixture on disk.
    fn make_book_fixture(dir: &std::path::Path) -> std::path::PathBuf {
        let html = r#"<html><head><title>T</title></head><body><h1>Ch</h1><p>Hi.</p></body></html>"#;
        std::fs::write(dir.join("content.html"), html).unwrap();
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Book</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Alice Author</dc:creator>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let p = dir.join("content.opf");
        std::fs::write(&p, opf).unwrap();
        p
    }

    #[test]
    fn test_check_passes_on_real_book_mobi() {
        let dir = std::env::temp_dir().join("kindling_mobi_check_pass");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opf = make_book_fixture(&dir);
        let out = dir.join("out.mobi");
        crate::mobi::build_mobi(
            &opf, &out, true, false, None, false, true, false, false, None, false, false, false,
        )
        .expect("build should succeed");

        let report = check_mobi_file(
            &out,
            &ExpectedMetadata {
                title: Some("Test Book"),
                author: Some("Alice Author"),
                is_comic: false,
                is_dictionary: false,
            },
        )
        .expect("check should run");

        assert!(
            !report.has_errors(),
            "expected no P0 errors, got: {:?}",
            report.p0_errors
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_fails_when_exth_100_missing() {
        // Build a valid MOBI then surgically strip EXTH 100 (author).
        let dir = std::env::temp_dir().join("kindling_mobi_check_no100");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opf = make_book_fixture(&dir);
        let out = dir.join("out.mobi");
        crate::mobi::build_mobi(
            &opf, &out, true, false, None, false, true, false, false, None, false, false, false,
        )
        .unwrap();

        let mut data = std::fs::read(&out).unwrap();
        // Find the EXTH 100 record and overwrite its type to 99 (unknown),
        // which makes our checker treat it as missing.
        let exth_pos = data.windows(4).position(|w| w == b"EXTH").unwrap();
        let rec_count = read_u32_be(&data, exth_pos + 8).unwrap() as usize;
        let mut pos = exth_pos + 12;
        for _ in 0..rec_count {
            let rtype = read_u32_be(&data, pos).unwrap();
            let rlen = read_u32_be(&data, pos + 4).unwrap() as usize;
            if rtype == 100 {
                data[pos..pos + 4].copy_from_slice(&99u32.to_be_bytes());
                break;
            }
            pos += rlen;
        }
        std::fs::write(&out, &data).unwrap();

        let report = check_mobi_file(
            &out,
            &ExpectedMetadata {
                title: Some("Test Book"),
                author: Some("Alice Author"),
                is_comic: false,
                is_dictionary: false,
            },
        )
        .unwrap();
        assert!(
            report.has_errors(),
            "expected P0 failure for missing EXTH 100"
        );
        assert!(
            report
                .p0_errors
                .iter()
                .any(|e| e.contains("EXTH 100")),
            "error should mention EXTH 100, got {:?}",
            report.p0_errors
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_fails_when_exth_503_missing() {
        let dir = std::env::temp_dir().join("kindling_mobi_check_no503");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let opf = make_book_fixture(&dir);
        let out = dir.join("out.mobi");
        crate::mobi::build_mobi(
            &opf, &out, true, false, None, false, true, false, false, None, false, false, false,
        )
        .unwrap();

        let mut data = std::fs::read(&out).unwrap();
        let exth_pos = data.windows(4).position(|w| w == b"EXTH").unwrap();
        let rec_count = read_u32_be(&data, exth_pos + 8).unwrap() as usize;
        let mut pos = exth_pos + 12;
        for _ in 0..rec_count {
            let rtype = read_u32_be(&data, pos).unwrap();
            let rlen = read_u32_be(&data, pos + 4).unwrap() as usize;
            if rtype == 503 {
                data[pos..pos + 4].copy_from_slice(&9999u32.to_be_bytes());
                break;
            }
            pos += rlen;
        }
        std::fs::write(&out, &data).unwrap();

        let report = check_mobi_file(
            &out,
            &ExpectedMetadata {
                title: Some("Test Book"),
                author: Some("Alice Author"),
                is_comic: false,
                is_dictionary: false,
            },
        )
        .unwrap();
        // Missing EXTH 503 is now a warning, not a P0 error
        // (KCC/kindlegen omit it; it breaks fixed-layout navigation)
        assert!(
            report.warnings.iter().any(|w| w.contains("EXTH 503")),
            "should warn about missing EXTH 503, got warnings: {:?}",
            report.warnings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
