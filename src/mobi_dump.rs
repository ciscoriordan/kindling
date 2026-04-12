/// Structural MOBI/AZW3 dumper.
///
/// Parses a finished MOBI file and emits one line per semantic field in
/// `section.field = value` form so `diff -u` surfaces structural differences
/// instead of an offset cascade. Text and image record contents are
/// intentionally summarized (length + 4-byte magic only) to keep diffs
/// focused on index / header / EXTH / INDX state.
///
/// This is a read-only parser that duplicates a fair bit of mobi_check's
/// PalmDB and MOBI header walking. The duplication is intentional: the
/// dumper needs a different output shape and we'd rather iterate on it
/// without perturbing the readback checker. If the overlap becomes painful
/// we can factor both onto a shared primitives module later.

use std::fmt::Write as _;
use std::io;
use std::path::Path;

// ---------------------------------------------------------------------
// Byte readers
// ---------------------------------------------------------------------

fn read_u16_be(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn read_u32_be(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// Render the first 4 bytes of a buffer as either a quoted ASCII magic
/// string or a hex literal if any byte is non-printable.
fn magic_or_hex(buf: &[u8]) -> String {
    if buf.len() < 4 {
        let mut s = String::from("0x");
        for b in buf {
            let _ = write!(s, "{:02X}", b);
        }
        return s;
    }
    let head = &buf[..4];
    if head.iter().all(|&b| (0x20..0x7F).contains(&b)) {
        format!("\"{}\"", String::from_utf8_lossy(head))
    } else {
        format!(
            "0x{:02X}{:02X}{:02X}{:02X}",
            head[0], head[1], head[2], head[3]
        )
    }
}

/// Render a byte slice as a hex literal prefixed with `0x`.
fn to_hex(buf: &[u8]) -> String {
    let mut s = String::from("0x");
    for b in buf {
        let _ = write!(s, "{:02X}", b);
    }
    s
}

/// Pretty-print a u32 that may encode "no such record" as 0xFFFFFFFF.
fn opt_record_idx(v: u32) -> String {
    if v == 0xFFFFFFFF {
        "NONE".to_string()
    } else {
        v.to_string()
    }
}

/// Escape a string for dump output. Newlines, tabs, non-printables, quotes,
/// and backslashes are rendered as escapes so the output stays single-line.
fn quote_str(s: &str) -> String {
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
                let _ = write!(out, "\\x{:02X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// ---------------------------------------------------------------------
// PalmDB
// ---------------------------------------------------------------------

struct PalmDb {
    /// Byte offset in the file where each record begins.
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

fn parse_palmdb(data: &[u8]) -> io::Result<PalmDb> {
    if data.len() < 78 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("PalmDB header truncated: {} bytes", data.len()),
        ));
    }
    let num_records = read_u16_be(data, 76).unwrap_or(0) as usize;
    let list_end = 78 + num_records * 8;
    if data.len() < list_end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "PalmDB record list truncated",
        ));
    }
    let mut offsets = Vec::with_capacity(num_records);
    for i in 0..num_records {
        offsets.push(read_u32_be(data, 78 + i * 8).unwrap_or(0));
    }
    Ok(PalmDb { offsets, num_records })
}

fn dump_palmdb_header(out: &mut String, data: &[u8], palmdb: &PalmDb) {
    // PalmDB name is a 32-byte null-terminated field.
    let name_end = data[..32].iter().position(|&b| b == 0).unwrap_or(32);
    let name = String::from_utf8_lossy(&data[..name_end]).into_owned();
    let _ = writeln!(out, "palmdb.name = {}", quote_str(&name));

    let attrs = read_u16_be(data, 32).unwrap_or(0);
    let version = read_u16_be(data, 34).unwrap_or(0);
    let ctime = read_u32_be(data, 36).unwrap_or(0);
    let mtime = read_u32_be(data, 40).unwrap_or(0);
    let btime = read_u32_be(data, 44).unwrap_or(0);
    let mod_num = read_u32_be(data, 48).unwrap_or(0);
    let _appinfo = read_u32_be(data, 52).unwrap_or(0);
    let _sortinfo = read_u32_be(data, 56).unwrap_or(0);
    let ptype = String::from_utf8_lossy(&data[60..64]).into_owned();
    let creator = String::from_utf8_lossy(&data[64..68]).into_owned();
    let uid = read_u32_be(data, 68).unwrap_or(0);
    let _next_rec_list = read_u32_be(data, 72).unwrap_or(0);

    let _ = writeln!(out, "palmdb.attributes = 0x{:04X}", attrs);
    let _ = writeln!(out, "palmdb.version = {}", version);
    let _ = writeln!(out, "palmdb.ctime = {}", ctime);
    let _ = writeln!(out, "palmdb.mtime = {}", mtime);
    let _ = writeln!(out, "palmdb.btime = {}", btime);
    let _ = writeln!(out, "palmdb.mod_num = {}", mod_num);
    let _ = writeln!(out, "palmdb.type = {}", quote_str(&ptype));
    let _ = writeln!(out, "palmdb.creator = {}", quote_str(&creator));
    let _ = writeln!(out, "palmdb.uid = {}", uid);
    let _ = writeln!(out, "palmdb.rec_count = {}", palmdb.num_records);
}

// ---------------------------------------------------------------------
// MOBI section (PalmDOC + MOBI header + EXTH)
// ---------------------------------------------------------------------

struct MobiHeader {
    header_length: u32,
    exth: Vec<(u32, Vec<u8>)>,
    file_version: u32,
}

fn dump_mobi_section(
    out: &mut String,
    label: &str,
    record0: &[u8],
    record_idx: usize,
) -> Option<MobiHeader> {
    if record0.len() < 16 + 24 {
        let _ = writeln!(
            out,
            "{}.error = \"record {} too small for PalmDOC+MOBI header ({} bytes)\"",
            label,
            record_idx,
            record0.len()
        );
        return None;
    }

    // PalmDOC header (16 bytes at offset 0).
    let compression = read_u16_be(record0, 0).unwrap_or(0);
    // offset 2: unused
    let text_length = read_u32_be(record0, 4).unwrap_or(0);
    let text_record_count = read_u16_be(record0, 8).unwrap_or(0);
    let text_record_size = read_u16_be(record0, 10).unwrap_or(0);
    let encryption = read_u16_be(record0, 12).unwrap_or(0);

    let _ = writeln!(out, "palmdoc.compression = {}", compression);
    let _ = writeln!(out, "palmdoc.text_length = {}", text_length);
    let _ = writeln!(out, "palmdoc.text_record_count = {}", text_record_count);
    let _ = writeln!(out, "palmdoc.text_record_size = {}", text_record_size);
    let _ = writeln!(out, "palmdoc.encryption = {}", encryption);

    // MOBI header at offset 16.
    if &record0[16..20] != b"MOBI" {
        let _ = writeln!(
            out,
            "{}.mobi.identifier_raw = {}",
            label,
            magic_or_hex(&record0[16..20])
        );
        return None;
    }

    let header_length = read_u32_be(record0, 20).unwrap_or(0);
    let mobi_type = read_u32_be(record0, 24).unwrap_or(0);
    let text_encoding = read_u32_be(record0, 28).unwrap_or(0);
    let unique_id = read_u32_be(record0, 32).unwrap_or(0);
    let file_version = read_u32_be(record0, 36).unwrap_or(0);
    let orth_index = read_u32_be(record0, 40).unwrap_or(0);
    let infl_index = read_u32_be(record0, 44).unwrap_or(0);
    let names_index = read_u32_be(record0, 48).unwrap_or(0);
    let keys_index = read_u32_be(record0, 52).unwrap_or(0);
    let _extra0 = read_u32_be(record0, 56).unwrap_or(0);
    let _extra1 = read_u32_be(record0, 60).unwrap_or(0);
    let _extra2 = read_u32_be(record0, 64).unwrap_or(0);
    let _extra3 = read_u32_be(record0, 68).unwrap_or(0);
    let first_non_text_index = read_u32_be(record0, 72).unwrap_or(0);
    let full_name_offset = read_u32_be(record0, 76).unwrap_or(0);
    let full_name_length = read_u32_be(record0, 80).unwrap_or(0);
    let locale = read_u32_be(record0, 84).unwrap_or(0);
    let dict_input_lang = read_u32_be(record0, 88).unwrap_or(0);
    let dict_output_lang = read_u32_be(record0, 92).unwrap_or(0);
    let min_version = read_u32_be(record0, 96).unwrap_or(0);
    let first_image_index = read_u32_be(record0, 100).unwrap_or(0);
    let huff_rec_index = read_u32_be(record0, 104).unwrap_or(0);
    let huff_rec_count = read_u32_be(record0, 108).unwrap_or(0);
    let datp_rec_index = read_u32_be(record0, 112).unwrap_or(0);
    let datp_rec_count = read_u32_be(record0, 116).unwrap_or(0);
    let exth_flags = read_u32_be(record0, 120).unwrap_or(0);
    // offsets 124..172: 32 bytes unknown + 8 bytes reserved area
    let drm_offset = read_u32_be(record0, 168).unwrap_or(0);
    let drm_count = read_u32_be(record0, 172).unwrap_or(0);
    let drm_size = read_u32_be(record0, 176).unwrap_or(0);
    let drm_flags = read_u32_be(record0, 180).unwrap_or(0);
    // offsets 184..192 unused
    let first_text_index = read_u16_be(record0, 192).unwrap_or(0);
    let last_text_index = read_u16_be(record0, 194).unwrap_or(0);
    let fdst_index = read_u32_be(record0, 192).unwrap_or(0); // KF8 alias for first/last
    let fdst_section_count = read_u32_be(record0, 196).unwrap_or(0);
    let fcis_index = read_u32_be(record0, 200).unwrap_or(0);
    let fcis_count = read_u32_be(record0, 204).unwrap_or(0);
    let flis_index = read_u32_be(record0, 208).unwrap_or(0);
    let flis_count = read_u32_be(record0, 212).unwrap_or(0);
    let srcs_index = read_u32_be(record0, 224).unwrap_or(0);
    let srcs_count = read_u32_be(record0, 228).unwrap_or(0);
    // extra_record_flags: MOBI-header offset 224 (= record0 offset 240) as u32.
    // libmobi docs often say u16 at MOBI+242, but both kindlegen and kindling
    // actually write a full u32 at MOBI+224. Verified by dumping the raw
    // bytes of kindlegen_reference.mobi.
    let extra_record_flags = read_u32_be(record0, 240).unwrap_or(0);
    let primary_index_record = read_u32_be(record0, 244).unwrap_or(0);
    // KF8-specific extensions at offsets 248 and beyond (relative to MOBI
    // header start, which is record0+16, so record0 offsets add 16).
    // NOTE: older libmobi docs show the KF8 sect/skel/datp/oth at header
    // offsets 232-248, which in record0 coords is 248-264. We try both
    // documented slots; the one we trust most is the libmobi definitions.
    let fragment_index = read_u32_be(record0, 16 + 232).unwrap_or(0xFFFFFFFF);
    let skeleton_index = read_u32_be(record0, 16 + 236).unwrap_or(0xFFFFFFFF);
    let datp_index = read_u32_be(record0, 16 + 240).unwrap_or(0xFFFFFFFF);
    let oth_index = read_u32_be(record0, 16 + 244).unwrap_or(0xFFFFFFFF);

    let _ = writeln!(out, "mobi.identifier = \"MOBI\"");
    let _ = writeln!(out, "mobi.header_length = {}", header_length);
    let _ = writeln!(out, "mobi.mobi_type = {}", mobi_type);
    let _ = writeln!(out, "mobi.text_encoding = {}", text_encoding);
    let _ = writeln!(out, "mobi.unique_id = {}", unique_id);
    let _ = writeln!(out, "mobi.file_version = {}", file_version);
    let _ = writeln!(out, "mobi.orth_index = {}", opt_record_idx(orth_index));
    let _ = writeln!(out, "mobi.infl_index = {}", opt_record_idx(infl_index));
    let _ = writeln!(out, "mobi.names_index = {}", opt_record_idx(names_index));
    let _ = writeln!(out, "mobi.keys_index = {}", opt_record_idx(keys_index));
    let _ = writeln!(
        out,
        "mobi.first_non_text_index = {}",
        opt_record_idx(first_non_text_index)
    );
    let _ = writeln!(out, "mobi.first_text_index = {}", first_text_index);
    let _ = writeln!(out, "mobi.last_text_index = {}", last_text_index);
    let _ = writeln!(out, "mobi.full_name_offset = {}", full_name_offset);
    let _ = writeln!(out, "mobi.full_name_length = {}", full_name_length);

    // Decode full_name — offset is relative to MOBI header start (record0+16).
    let name_abs = 16 + full_name_offset as usize;
    let name_end = name_abs + full_name_length as usize;
    let full_name = if name_end <= record0.len() {
        String::from_utf8_lossy(&record0[name_abs..name_end]).into_owned()
    } else {
        String::new()
    };
    let _ = writeln!(out, "mobi.full_name = {}", quote_str(&full_name));

    let _ = writeln!(out, "mobi.locale = {}", locale);
    let _ = writeln!(out, "mobi.dict_input_lang = {}", dict_input_lang);
    let _ = writeln!(out, "mobi.dict_output_lang = {}", dict_output_lang);
    let _ = writeln!(out, "mobi.min_version = {}", min_version);
    let _ = writeln!(
        out,
        "mobi.first_image_index = {}",
        opt_record_idx(first_image_index)
    );
    let _ = writeln!(
        out,
        "mobi.huff_rec_index = {}",
        opt_record_idx(huff_rec_index)
    );
    let _ = writeln!(out, "mobi.huff_rec_count = {}", huff_rec_count);
    let _ = writeln!(
        out,
        "mobi.datp_rec_index = {}",
        opt_record_idx(datp_rec_index)
    );
    let _ = writeln!(out, "mobi.datp_rec_count = {}", datp_rec_count);
    let _ = writeln!(out, "mobi.exth_flags = 0x{:08X}", exth_flags);
    let _ = writeln!(out, "mobi.drm_offset = 0x{:08X}", drm_offset);
    let _ = writeln!(out, "mobi.drm_count = {}", drm_count);
    let _ = writeln!(out, "mobi.drm_size = {}", drm_size);
    let _ = writeln!(out, "mobi.drm_flags = 0x{:08X}", drm_flags);
    let _ = writeln!(out, "mobi.fdst_index = {}", opt_record_idx(fdst_index));
    let _ = writeln!(out, "mobi.fdst_section_count = {}", fdst_section_count);
    let _ = writeln!(out, "mobi.fcis_index = {}", opt_record_idx(fcis_index));
    let _ = writeln!(out, "mobi.fcis_count = {}", fcis_count);
    let _ = writeln!(out, "mobi.flis_index = {}", opt_record_idx(flis_index));
    let _ = writeln!(out, "mobi.flis_count = {}", flis_count);
    let _ = writeln!(out, "mobi.srcs_index = {}", opt_record_idx(srcs_index));
    let _ = writeln!(out, "mobi.srcs_count = {}", srcs_count);
    let _ = writeln!(out, "mobi.extra_record_flags = 0x{:08X}", extra_record_flags);
    let _ = writeln!(
        out,
        "mobi.primary_index_record = {}",
        opt_record_idx(primary_index_record)
    );
    let _ = writeln!(
        out,
        "mobi.fragment_index = {}",
        opt_record_idx(fragment_index)
    );
    let _ = writeln!(
        out,
        "mobi.skeleton_index = {}",
        opt_record_idx(skeleton_index)
    );
    let _ = writeln!(out, "mobi.datp_index = {}", opt_record_idx(datp_index));
    let _ = writeln!(out, "mobi.oth_index = {}", opt_record_idx(oth_index));

    // EXTH parsing. EXTH block starts at MOBI header end = 16 + header_len.
    let exth_off = 16 + header_length as usize;
    let mut exth: Vec<(u32, Vec<u8>)> = Vec::new();
    if exth_off + 12 <= record0.len() && &record0[exth_off..exth_off + 4] == b"EXTH" {
        let exth_len = read_u32_be(record0, exth_off + 4).unwrap_or(0) as usize;
        let exth_count = read_u32_be(record0, exth_off + 8).unwrap_or(0) as usize;
        let _ = writeln!(out, "exth.length = {}", exth_len);
        let _ = writeln!(out, "exth.count = {}", exth_count);

        let exth_end = exth_off + exth_len;
        let mut pos = exth_off + 12;
        for _ in 0..exth_count {
            if pos + 8 > exth_end || pos + 8 > record0.len() {
                break;
            }
            let rtype = read_u32_be(record0, pos).unwrap_or(0);
            let rlen = read_u32_be(record0, pos + 4).unwrap_or(0) as usize;
            if rlen < 8 || pos + rlen > record0.len() {
                break;
            }
            let payload = record0[pos + 8..pos + rlen].to_vec();
            exth.push((rtype, payload));
            pos += rlen;
        }

        // Sort EXTH by type for stable diff output, then by order of appearance
        // within the same type (some types appear multiple times, e.g. 101
        // contributors).
        let mut sorted: Vec<(usize, &(u32, Vec<u8>))> = exth.iter().enumerate().collect();
        sorted.sort_by_key(|(i, (t, _))| (*t, *i));
        for (_, (rtype, payload)) in sorted {
            dump_exth_record(out, *rtype, payload);
        }
    } else {
        let _ = writeln!(out, "exth.present = false");
    }

    Some(MobiHeader {
        header_length,
        exth,
        file_version,
    })
}

/// Emit lines for a single EXTH record. String-valued records emit a
/// `.value` line; numeric u32-valued records emit a `.value_u32` line; other
/// records emit `.value_hex`.
fn dump_exth_record(out: &mut String, rtype: u32, payload: &[u8]) {
    let _ = writeln!(out, "exth[{}].name = {}", rtype, quote_str(exth_type_name(rtype)));
    let _ = writeln!(out, "exth[{}].value_len = {}", rtype, payload.len());

    if is_numeric_exth(rtype) && payload.len() == 4 {
        let v = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let _ = writeln!(out, "exth[{}].value_u32 = {}", rtype, v);
        return;
    }

    if is_string_exth(rtype) {
        if let Ok(s) = std::str::from_utf8(payload) {
            let _ = writeln!(out, "exth[{}].value = {}", rtype, quote_str(s));
            return;
        }
    }

    let _ = writeln!(out, "exth[{}].value_hex = {}", rtype, to_hex(payload));
}

/// Well-known EXTH types. Mirrors the libmobi + MobileRead wiki names.
fn exth_type_name(rtype: u32) -> &'static str {
    match rtype {
        100 => "creator",
        101 => "publisher",
        103 => "description",
        104 => "isbn",
        105 => "subject",
        106 => "publishingdate",
        108 => "contributor",
        109 => "rights",
        112 => "source",
        113 => "asin",
        114 => "versionnumber",
        115 => "sample",
        116 => "startreading",
        117 => "adult",
        118 => "retail_price",
        119 => "retail_price_currency",
        121 => "kf8boundary",
        125 => "kf8_count_resources_fonts",
        129 => "kf8_cover_uri",
        131 => "kf8_unknown131",
        201 => "coveroffset",
        202 => "thumboffset",
        203 => "hasfakecover",
        204 => "creatorsoftware",
        205 => "creatormajor",
        206 => "creatorminor",
        207 => "creatorbuild",
        208 => "watermark",
        209 => "tamperkey",
        300 => "fontsignature",
        401 => "clippinglimit",
        402 => "publisherlimit",
        403 => "unknown403",
        404 => "ttsdisable",
        406 => "rental_expiration",
        450 => "unknown450",
        451 => "unknown451",
        452 => "unknown452",
        453 => "unknown453",
        501 => "cde_content_type",
        502 => "lastupdatetime",
        503 => "updatedtitle",
        504 => "asin2",
        524 => "language",
        525 => "writingmode",
        526 => "nominalpagecount",
        527 => "pageprogressiondirection",
        528 => "overrideslhfeatures",
        529 => "originalsourcedesc",
        534 => "inputsourcetype",
        535 => "creatorbuildrev",
        536 => "containerinfo",
        538 => "originalresolution",
        542 => "unknown542",
        _ => "unknown",
    }
}

fn is_string_exth(rtype: u32) -> bool {
    matches!(
        rtype,
        100 | 101
            | 103
            | 104
            | 105
            | 106
            | 108
            | 109
            | 112
            | 113
            | 117
            | 118
            | 119
            | 129
            | 208
            | 501
            | 503
            | 504
            | 524
            | 525
            | 527
            | 529
            | 534
            | 535
            | 536
            | 538
    )
}

fn is_numeric_exth(rtype: u32) -> bool {
    matches!(
        rtype,
        115 | 116
            | 121
            | 125
            | 131
            | 201
            | 202
            | 203
            | 204
            | 205
            | 206
            | 207
            | 401
            | 402
            | 403
            | 404
            | 406
            | 502
            | 526
    )
}

// ---------------------------------------------------------------------
// INDX record dumping
// ---------------------------------------------------------------------

/// One entry in a parsed TAGX block. The bitmask identifies which bit(s)
/// of the entry control byte indicate that this tag is present; for most
/// scalar tags num_values=1 and the mask picks a single bit, but ARRAY
/// tags reserve a multi-bit slot that stores the element count.
#[derive(Clone, Debug)]
struct TagDef {
    tag_id: u8,
    num_values: u8,
    mask: u8,
    _end_flag: u8,
}

/// Schema for a data-entry block: the list of tag definitions taken from
/// a paired primary INDX's TAGX plus the control-byte count that governs
/// how many leading control bytes each entry starts with.
#[derive(Clone, Debug, Default)]
struct TagxSchema {
    tags: Vec<TagDef>,
    control_byte_count: u32,
}

/// Parse a TAGX block starting at `tagx_start`. Returns the parsed schema
/// plus the length field so the caller can skip forward.
fn parse_tagx(rec: &[u8], tagx_start: usize) -> Option<(TagxSchema, usize)> {
    if tagx_start + 12 > rec.len() || &rec[tagx_start..tagx_start + 4] != b"TAGX" {
        return None;
    }
    let tagx_len = read_u32_be(rec, tagx_start + 4)? as usize;
    let control_byte_count = read_u32_be(rec, tagx_start + 8)?;
    let mut tags = Vec::new();
    let mut i = tagx_start + 12;
    let tagx_end = (tagx_start + tagx_len).min(rec.len());
    while i + 4 <= tagx_end {
        let tag_id = rec[i];
        let num_values = rec[i + 1];
        let mask = rec[i + 2];
        let end_flag = rec[i + 3];
        tags.push(TagDef {
            tag_id,
            num_values,
            mask,
            _end_flag: end_flag,
        });
        i += 4;
        if tag_id == 0 && mask == 0 && end_flag == 1 {
            break;
        }
    }
    Some((
        TagxSchema {
            tags,
            control_byte_count,
        },
        tagx_len,
    ))
}

/// Decode an inverted variable-length integer used by kindlegen for INDX
/// data-record tag values. Each byte carries 7 data bits in big-endian
/// order; the HIGH bit indicates the LAST byte (high bit SET = stop).
/// This is the inverse of the forward VWI convention libmobi uses for
/// header counts; it matches vwi::encode_vwi_inv in this crate.
/// Returns the decoded value and the number of bytes consumed.
fn read_vwi(bytes: &[u8]) -> Option<(u32, usize)> {
    let mut value: u32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        value = (value << 7) | (b & 0x7F) as u32;
        if b & 0x80 != 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Decode an INDX data-entry tag block given a TAGX schema.
///
/// Returns a vec of (tag_id, Vec<u32>) pairs. For tags present via a
/// single-bit mask the Vec has `num_values` entries; for tags using a
/// multi-bit mask (ARRAY tags), the count is `(control_byte_mask_slot /
/// shift) * num_values`.
///
/// `control_bytes` is the leading control-byte run (1 byte for
/// control_byte_count=1). `tag_stream` is the VWI-encoded bytes following
/// the control bytes.
fn decode_entry_tags(
    control_bytes: &[u8],
    tag_stream: &[u8],
    schema: &TagxSchema,
) -> Option<Vec<(u8, Vec<u32>)>> {
    let control = *control_bytes.first()?;
    let mut values: Vec<(u8, Vec<u32>)> = Vec::new();
    let mut cursor = 0usize;

    for td in &schema.tags {
        if td.tag_id == 0 && td.mask == 0 {
            continue;
        }
        if td.mask == 0 {
            continue;
        }

        // Check whether this tag's slot is non-zero in the control byte.
        // For a single-bit mask (e.g. 0x01), this is a boolean presence
        // check. For multi-bit masks (e.g. 0x04 with bits [2:3]), the
        // slot value is the array count.
        let slot = control & td.mask;
        if slot == 0 {
            continue;
        }
        let shift = td.mask.trailing_zeros();
        let count_multiplier = (slot >> shift) as usize;
        let total_values = count_multiplier * td.num_values as usize;

        let mut decoded = Vec::with_capacity(total_values);
        for _ in 0..total_values {
            if cursor >= tag_stream.len() {
                return None;
            }
            let (v, n) = read_vwi(&tag_stream[cursor..])?;
            decoded.push(v);
            cursor += n;
        }
        values.push((td.tag_id, decoded));
    }

    Some(values)
}

/// Dump an INDX record at `rec_idx`. Emits all fixed-header fields, the
/// ORDT2 codepoint list (if any), TAGX definitions (if any), and every
/// routing or data entry with decoded labels and tag values.
///
/// `paired_schema` is the TAGX schema taken from this record's primary
/// (for data records, generation=1) or from the record itself (for
/// primaries, generation=0). When None, tag decoding falls back to raw
/// hex.
fn dump_indx_record(
    out: &mut String,
    rec_idx: usize,
    rec: &[u8],
    paired_schema: Option<&TagxSchema>,
    paired_ordt2: Option<&[u32]>,
) {
    let label = format!("indx[{}]", rec_idx);
    if rec.len() < 192 || &rec[..4] != b"INDX" {
        let _ = writeln!(out, "{}.length = {}", label, rec.len());
        let _ = writeln!(out, "{}.magic = {}", label, magic_or_hex(rec));
        return;
    }

    let header_length = read_u32_be(rec, 4).unwrap_or(0);
    let index_type = read_u32_be(rec, 8).unwrap_or(0);
    let gen_number = read_u32_be(rec, 12).unwrap_or(0);
    // offset 16 is a kindlegen-constant (2)
    let const16 = read_u32_be(rec, 16).unwrap_or(0);
    let idxt_offset = read_u32_be(rec, 20).unwrap_or(0);
    let idxt_count = read_u32_be(rec, 24).unwrap_or(0);
    let encoding = read_u32_be(rec, 28).unwrap_or(0);
    let language = read_u32_be(rec, 32).unwrap_or(0);
    let total_entry_count = read_u32_be(rec, 36).unwrap_or(0);
    let ordt_offset_legacy = read_u32_be(rec, 40).unwrap_or(0);
    let ligt_offset = read_u32_be(rec, 44).unwrap_or(0);
    let ligt_entries = read_u32_be(rec, 48).unwrap_or(0);
    let cncx_records_count = read_u32_be(rec, 52).unwrap_or(0);
    // offsets 56..164 are reserved/unknown in most writers
    let ordt_type = read_u32_be(rec, 164).unwrap_or(0);
    let ordt_entries_count = read_u32_be(rec, 168).unwrap_or(0);
    let ordt1_offset = read_u32_be(rec, 172).unwrap_or(0);
    let ordt2_offset = read_u32_be(rec, 176).unwrap_or(0);
    let tagx_offset = read_u32_be(rec, 180).unwrap_or(0);

    let _ = writeln!(out, "{}.identifier = \"INDX\"", label);
    let _ = writeln!(out, "{}.length = {}", label, rec.len());
    let _ = writeln!(out, "{}.header_length = {}", label, header_length);
    let _ = writeln!(out, "{}.index_type = {}", label, index_type);
    let _ = writeln!(out, "{}.generation = {}", label, gen_number);
    let _ = writeln!(out, "{}.const16 = {}", label, const16);
    let _ = writeln!(out, "{}.idxt_offset = {}", label, idxt_offset);
    let _ = writeln!(out, "{}.idxt_count = {}", label, idxt_count);
    let _ = writeln!(out, "{}.encoding = {}", label, encoding);
    let _ = writeln!(out, "{}.language = {}", label, language);
    let _ = writeln!(out, "{}.total_entry_count = {}", label, total_entry_count);
    let _ = writeln!(
        out,
        "{}.ordt_offset_legacy = {}",
        label, ordt_offset_legacy
    );
    let _ = writeln!(out, "{}.ligt_offset = {}", label, ligt_offset);
    let _ = writeln!(out, "{}.ligt_entries = {}", label, ligt_entries);
    let _ = writeln!(
        out,
        "{}.cncx_records_count = {}",
        label, cncx_records_count
    );
    let _ = writeln!(out, "{}.ordt_type = {}", label, ordt_type);
    let _ = writeln!(
        out,
        "{}.ordt_entries_count = {}",
        label, ordt_entries_count
    );
    let _ = writeln!(out, "{}.ordt1_offset = {}", label, ordt1_offset);
    let _ = writeln!(out, "{}.ordt2_offset = {}", label, ordt2_offset);
    let _ = writeln!(out, "{}.tagx_offset_field = {}", label, tagx_offset);

    // ORDT2 codepoint table decoding. libmobi's `mobi_parse_ordt` reads
    // N u16 BE codepoints starting at ordt2_offset + 4 (skipping the
    // ORDT2 magic). kindling places the ORDT2 magic directly at
    // ordt2_offset so we skip 4 bytes.
    let ordt2: Option<Vec<u32>> = if ordt_type == 1 && ordt_entries_count > 0 {
        let start = ordt2_offset as usize + 4;
        let end = start + 2 * ordt_entries_count as usize;
        if end <= rec.len() {
            let mut cps = Vec::with_capacity(ordt_entries_count as usize);
            for i in 0..ordt_entries_count as usize {
                let cp = read_u16_be(rec, start + 2 * i).unwrap_or(0) as u32;
                cps.push(cp);
            }
            Some(cps)
        } else {
            None
        }
    } else {
        None
    };

    if let Some(cps) = &ordt2 {
        let _ = writeln!(out, "{}.ordt2_entries = {}", label, cps.len());
        // Emit one line per codepoint so a diff can see exactly which
        // codepoint moved where in the table.
        for (i, cp) in cps.iter().enumerate() {
            let _ = writeln!(out, "{}.ordt2[{}] = U+{:04X}", label, i, cp);
        }
    }

    // TAGX parsing. Kindlegen places TAGX right after the primary header
    // region at rec offset `header_length` — the declared header_length
    // already includes the 7-byte "default" string for sub-index 1
    // (header_length=199), so the TAGX start is simply `header_length`.
    // Data records (generation=1) have no TAGX of their own.
    //
    // Note: older versions of this dumper computed tagx_start as
    // `header_length + default_str_len`, which double-counted the
    // default string for header_length=199 and silently skipped the
    // TAGX block on the orth primary (indx[3]).
    let own_schema = if gen_number == 0 {
        let tagx_start = header_length as usize;
        if let Some((schema, tagx_len)) = parse_tagx(rec, tagx_start) {
            let _ = writeln!(out, "{}.tagx.length = {}", label, tagx_len);
            let _ = writeln!(
                out,
                "{}.tagx.control_byte_count = {}",
                label, schema.control_byte_count
            );
            for (idx, td) in schema.tags.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{}.tagx[{}] = (tag_id={}, num_values={}, bitmask=0x{:02X}, end={})",
                    label, idx, td.tag_id, td.num_values, td.mask, td._end_flag
                );
            }
            Some(schema)
        } else {
            None
        }
    } else {
        None
    };

    // IDXT block: read the offsets of each entry.
    let idxt_off = idxt_offset as usize;
    let mut entry_offsets: Vec<usize> = Vec::new();
    if idxt_off + 4 <= rec.len() && &rec[idxt_off..idxt_off + 4] == b"IDXT" {
        let _ = writeln!(out, "{}.idxt.magic = \"IDXT\"", label);
        for i in 0..idxt_count as usize {
            let off = match read_u16_be(rec, idxt_off + 4 + 2 * i) {
                Some(v) => v as usize,
                None => break,
            };
            entry_offsets.push(off);
        }
        // Emit the IDXT offsets as a compact list for easy visual diffing.
        let offsets_list: Vec<String> =
            entry_offsets.iter().map(|o| format!("0x{:04X}", o)).collect();
        let _ = writeln!(
            out,
            "{}.idxt_offsets = [{}]",
            label,
            offsets_list.join(", ")
        );
    }

    // For primaries, entries are routing records (label_len u8, label
    // bytes, u16 count). For data records (generation=1), each entry is
    // a label + control byte(s) + VWI-encoded tag values decoded via
    // the paired primary's TAGX schema.
    //
    // Label decoding uses this record's own ORDT2 table when present;
    // data records don't carry their own ORDT2, so we fall back to the
    // paired primary's table.
    let effective_ordt2 = ordt2.as_deref().or(paired_ordt2);
    let effective_schema = if gen_number == 0 {
        own_schema.as_ref()
    } else {
        paired_schema
    };

    for (i, &off) in entry_offsets.iter().enumerate() {
        let end = if i + 1 < entry_offsets.len() {
            entry_offsets[i + 1]
        } else {
            idxt_off
        };
        if off >= rec.len() || end > rec.len() || end < off {
            continue;
        }
        let entry = &rec[off..end];
        let decoded =
            decode_indx_entry(entry, gen_number, effective_ordt2, effective_schema);
        let _ = writeln!(out, "{}.entries[{}] = {}", label, i, decoded);
    }
}

/// Decode a single INDX entry. For routing records (generation=0) the
/// layout is [u8 label_len][label_bytes][u16 BE count]. For data records
/// (generation=1) the layout is [u8 byte0 = prefix_len<<5 | new_len]
/// [label_bytes][u8 control_byte(s)][VWI-encoded tag values...].
///
/// When `ordt2` is Some and the record uses ORDT2 encoding (ordt_type=1),
/// each label byte is looked up in the codepoint table to produce UTF-8.
/// Otherwise we try UTF-16BE, then fall back to hex. When a data record
/// has a paired TAGX `schema`, the tag bytes are decoded into explicit
/// `tag[N] = [values]` fragments; when decoding fails (truncated /
/// schema mismatch), we fall back to raw hex with a `# decode_failed`
/// trailing comment.
fn decode_indx_entry(
    entry: &[u8],
    generation: u32,
    ordt2: Option<&[u32]>,
    schema: Option<&TagxSchema>,
) -> String {
    if entry.is_empty() {
        return "(empty)".to_string();
    }

    if generation == 0 {
        // Routing entry: one-byte length, label, u16 BE count.
        let label_len = entry[0] as usize;
        if 1 + label_len > entry.len() {
            return format!("(truncated routing, raw={})", to_hex(entry));
        }
        let label_bytes = &entry[1..1 + label_len];
        let label = decode_label(label_bytes, ordt2);
        let count_bytes = &entry[1 + label_len..];
        let count = if count_bytes.len() >= 2 {
            u16::from_be_bytes([count_bytes[0], count_bytes[1]]) as u32
        } else {
            0
        };
        format!(
            "{{ routing_label = {}, label_bytes = {}, record_count = {} }}",
            quote_str(&label),
            to_hex(label_bytes),
            count
        )
    } else {
        // Data entry: prefix_len (3 bits) | new_len (5 bits), label,
        // control_byte_count bytes of control, VWI-encoded tag values.
        let byte0 = entry[0];
        let prefix_len = (byte0 >> 5) & 0x07;
        let new_len = (byte0 & 0x1F) as usize;
        if 1 + new_len + 1 > entry.len() {
            return format!("(truncated data, raw={})", to_hex(entry));
        }
        let label_bytes = &entry[1..1 + new_len];
        let label = decode_label(label_bytes, ordt2);

        let control_byte_count = schema.map(|s| s.control_byte_count as usize).unwrap_or(1);
        let control_start = 1 + new_len;
        let control_end = control_start + control_byte_count;
        if control_end > entry.len() {
            return format!(
                "{{ label = {}, label_bytes = {}, prefix_len = {}, new_len = {}, tag_bytes = {} }} # decode_failed:truncated_control",
                quote_str(&label),
                to_hex(label_bytes),
                prefix_len,
                new_len,
                to_hex(&entry[control_start..])
            );
        }
        let control_bytes = &entry[control_start..control_end];
        let tail = &entry[control_end..];

        // Render a single-byte control as a u8 literal for readability;
        // a multi-byte control as a hex blob. Either way we pass all
        // control bytes through to the decoder.
        let control_repr = if control_bytes.len() == 1 {
            format!("0x{:02X}", control_bytes[0])
        } else {
            to_hex(control_bytes)
        };

        if let Some(schema) = schema {
            if let Some(decoded) = decode_entry_tags(control_bytes, tail, schema) {
                // Render as `tag[N] = [a, b, c]` sorted by tag_id for stable diff.
                let mut decoded_sorted = decoded.clone();
                decoded_sorted.sort_by_key(|(id, _)| *id);
                let mut parts = Vec::new();
                parts.push(format!("label = {}", quote_str(&label)));
                parts.push(format!("label_bytes = {}", to_hex(label_bytes)));
                parts.push(format!("control = {}", control_repr));
                for (tag_id, values) in &decoded_sorted {
                    let vs: Vec<String> = values.iter().map(|v| v.to_string()).collect();
                    parts.push(format!("tag[{}] = [{}]", tag_id, vs.join(", ")));
                }
                return format!("{{ {} }}", parts.join(", "));
            }
        }

        format!(
            "{{ label = {}, label_bytes = {}, prefix_len = {}, new_len = {}, control = {}, tag_bytes = {} }}{}",
            quote_str(&label),
            to_hex(label_bytes),
            prefix_len,
            new_len,
            control_repr,
            to_hex(tail),
            if schema.is_some() {
                " # decode_failed"
            } else {
                " # no_schema"
            }
        )
    }
}

/// Decode INDX label bytes to a displayable string.
///
/// When an ORDT2 table is provided, each byte is treated as an index into
/// the codepoint list. When no ORDT2 is present, we try UTF-16BE (the
/// historical kindlegen encoding for non-dict sub-indexes). If both fail
/// we return `hex:0x...` so the diff still shows bytes.
fn decode_label(bytes: &[u8], ordt2: Option<&[u32]>) -> String {
    if let Some(table) = ordt2 {
        let mut out = String::with_capacity(bytes.len());
        let mut all_ok = true;
        for &b in bytes {
            match table.get(b as usize).copied() {
                Some(cp) => {
                    if let Some(c) = char::from_u32(cp) {
                        out.push(c);
                    } else {
                        all_ok = false;
                        break;
                    }
                }
                None => {
                    all_ok = false;
                    break;
                }
            }
        }
        if all_ok {
            return out;
        }
    }

    if bytes.len() % 2 == 0 && !bytes.is_empty() {
        let mut chars: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
        for chunk in bytes.chunks_exact(2) {
            chars.push(u16::from_be_bytes([chunk[0], chunk[1]]));
        }
        if let Ok(s) = String::from_utf16(&chars) {
            if s.chars().all(|c| !c.is_control() || c == ' ') {
                return s;
            }
        }
    }

    format!("hex:{}", to_hex(bytes))
}

// ---------------------------------------------------------------------
// Top-level driver
// ---------------------------------------------------------------------

/// Parse a MOBI/AZW3 file at `path` and return a structural dump as a
/// String. The dump is designed so `diff -u` between two files surfaces
/// semantic differences without being drowned in offset cascades.
pub fn dump_mobi(path: &Path) -> io::Result<String> {
    let data = std::fs::read(path)?;
    let palmdb = parse_palmdb(&data)?;

    let mut out = String::new();
    dump_palmdb_header(&mut out, &data, &palmdb);

    // PalmDB record list: emit length + magic for every record so the
    // overall layout is visible even before we parse individual records.
    for i in 0..palmdb.num_records {
        let rec = palmdb.record(&data, i).unwrap_or(&[]);
        let _ = writeln!(&mut out, "record[{}].length = {}", i, rec.len());
        let _ = writeln!(&mut out, "record[{}].magic = {}", i, magic_or_hex(rec));
    }

    // MOBI section 0 (always present: KF7 or KF8-only).
    let rec0 = palmdb.record(&data, 0).unwrap_or(&[]);
    let kf7 = dump_mobi_section(&mut out, "section0", rec0, 0);

    // KF8 section if EXTH 121 boundary is set in the KF7 header.
    if let Some(kf7) = kf7.as_ref() {
        if let Some((_, payload)) = kf7.exth.iter().find(|(t, _)| *t == 121) {
            if payload.len() == 4 {
                let boundary = u32::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                ]) as usize;
                if boundary < palmdb.num_records {
                    let _ = writeln!(&mut out, "kf8.boundary_record = {}", boundary);
                    if let Some(kf8_rec0) = palmdb.record(&data, boundary) {
                        dump_mobi_section(&mut out, "section_kf8", kf8_rec0, boundary);
                    }
                }
            }
        }
        let _ = writeln!(&mut out, "section0.file_version = {}", kf7.file_version);
        let _ = writeln!(
            &mut out,
            "section0.header_length_total = {}",
            kf7.header_length
        );
    }

    // INDX records. Walk every PalmDB record twice. First pass: pick
    // out primary (generation=0) INDX records and record their TAGX
    // schema + ORDT2 table keyed by PalmDB record index. Second pass:
    // walk in order, passing each data record the most-recently-seen
    // primary's schema and ORDT2 so entry tag bytes can be decoded.
    let mut indx_records: Vec<usize> = Vec::new();
    for i in 0..palmdb.num_records {
        if let Some(rec) = palmdb.record(&data, i) {
            if rec.len() >= 4 && &rec[..4] == b"INDX" {
                indx_records.push(i);
            }
        }
    }

    let mut primary_info: Vec<(usize, Option<TagxSchema>, Option<Vec<u32>>)> = Vec::new();
    for &i in &indx_records {
        let rec = match palmdb.record(&data, i) {
            Some(r) => r,
            None => continue,
        };
        let header_length = read_u32_be(rec, 4).unwrap_or(0);
        let generation = read_u32_be(rec, 12).unwrap_or(0);
        if generation != 0 {
            continue;
        }
        let schema = parse_tagx(rec, header_length as usize).map(|(s, _)| s);
        let ordt_type = read_u32_be(rec, 164).unwrap_or(0);
        let ordt_entries_count = read_u32_be(rec, 168).unwrap_or(0);
        let ordt2_offset = read_u32_be(rec, 176).unwrap_or(0);
        let ordt2 = if ordt_type == 1 && ordt_entries_count > 0 {
            let start = ordt2_offset as usize + 4;
            let end = start + 2 * ordt_entries_count as usize;
            if end <= rec.len() {
                let mut cps = Vec::with_capacity(ordt_entries_count as usize);
                for k in 0..ordt_entries_count as usize {
                    cps.push(read_u16_be(rec, start + 2 * k).unwrap_or(0) as u32);
                }
                Some(cps)
            } else {
                None
            }
        } else {
            None
        };
        primary_info.push((i, schema, ordt2));
    }

    for &i in &indx_records {
        let rec = match palmdb.record(&data, i) {
            Some(r) => r,
            None => continue,
        };
        let generation = read_u32_be(rec, 12).unwrap_or(0);
        let (schema, ordt2) = if generation == 0 {
            (None, None)
        } else {
            // Find the nearest preceding primary.
            let mut best: Option<&(usize, Option<TagxSchema>, Option<Vec<u32>>)> = None;
            for pi in &primary_info {
                if pi.0 < i {
                    best = Some(pi);
                } else {
                    break;
                }
            }
            match best {
                Some((_, s, o)) => (s.as_ref(), o.as_deref()),
                None => (None, None),
            }
        };
        dump_indx_record(&mut out, i, rec, schema, ordt2);
    }

    Ok(out)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/parity");
        p.push(name);
        p.push("kindlegen_reference.mobi");
        p
    }

    /// The dumper must succeed and emit PalmDB header lines for every
    /// parity fixture. We assert on structural shape, not byte-exact
    /// contents, so committed fixtures can evolve without breaking this
    /// test.
    #[test]
    fn dumps_simple_dict_fixture() {
        let path = fixture("simple_dict");
        if !path.exists() {
            eprintln!("skipping: fixture {} missing", path.display());
            return;
        }
        let dump = dump_mobi(&path).expect("dump should succeed");
        assert!(dump.contains("palmdb.type = \"BOOK\""));
        assert!(dump.contains("palmdb.creator = \"MOBI\""));
        assert!(dump.contains("mobi.identifier = \"MOBI\""));
        assert!(dump.contains("mobi.header_length"));
        assert!(
            dump.contains("exth["),
            "simple_dict should have at least one EXTH record"
        );
        assert!(
            dump.contains("indx["),
            "simple_dict must contain an INDX section"
        );
    }

    #[test]
    fn dumps_simple_book_fixture() {
        let path = fixture("simple_book");
        if !path.exists() {
            eprintln!("skipping: fixture {} missing", path.display());
            return;
        }
        let dump = dump_mobi(&path).expect("dump should succeed");
        assert!(dump.contains("palmdb.type = \"BOOK\""));
        assert!(dump.contains("mobi.identifier = \"MOBI\""));
        assert!(dump.contains("exth[100].name = \"creator\""));
        assert!(dump.contains("exth[524].name = \"language\""));
    }

    #[test]
    fn dumps_simple_comic_fixture() {
        let path = fixture("simple_comic");
        if !path.exists() {
            eprintln!("skipping: fixture {} missing", path.display());
            return;
        }
        let dump = dump_mobi(&path).expect("dump should succeed");
        assert!(dump.contains("palmdb.type = \"BOOK\""));
        assert!(dump.contains("exth[201].name = \"coveroffset\""));
    }

    #[test]
    fn inverted_vwi_decodes_single_and_multi_byte() {
        // Single byte: 0xFD = 1111_1101 ⇒ high bit set ⇒ stop. Value =
        // 0x7D = 125. Consumed 1.
        assert_eq!(read_vwi(&[0xFD]), Some((125, 1)));
        // Two bytes: 0x02 (high bit clear, more), 0xAF (high bit set, stop).
        // Value = (0x02 << 7) | 0x2F = 256 + 47 = 303. Consumed 2.
        assert_eq!(read_vwi(&[0x02, 0xAF]), Some((303, 2)));
        // Three bytes: 0x01 0x00 0x80 ⇒ (1 << 14) | (0 << 7) | 0 = 16384.
        assert_eq!(read_vwi(&[0x01, 0x00, 0x80]), Some((16384, 3)));
        // Unterminated (no high bit ever set): returns None.
        assert_eq!(read_vwi(&[0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn data_entry_decodes_three_scalar_tags() {
        // Schema: tags 1, 2, 42 each with mask = 0x01 / 0x02 / 0x04 and
        // num_values = 1. Control byte 0x07 = all three present.
        let schema = TagxSchema {
            control_byte_count: 1,
            tags: vec![
                TagDef {
                    tag_id: 1,
                    num_values: 1,
                    mask: 0x01,
                    _end_flag: 0,
                },
                TagDef {
                    tag_id: 2,
                    num_values: 1,
                    mask: 0x02,
                    _end_flag: 0,
                },
                TagDef {
                    tag_id: 42,
                    num_values: 1,
                    mask: 0x04,
                    _end_flag: 0,
                },
            ],
        };
        // Entry: byte0=0x00 (prefix=0, len=0), control=0x07, values VWI
        // [125, 73, 1] = bytes 0xFD 0xC9 0x81 (all single-byte).
        let entry = vec![0x00, 0x07, 0xFD, 0xC9, 0x81];
        let s = decode_indx_entry(&entry, 1, None, Some(&schema));
        assert!(s.contains("tag[1] = [125]"), "got {}", s);
        assert!(s.contains("tag[2] = [73]"), "got {}", s);
        assert!(s.contains("tag[42] = [1]"), "got {}", s);
        assert!(!s.contains("decode_failed"), "got {}", s);
    }

    /// Tiny synthetic INDX test: build a minimal PalmDB + MOBI + INDX
    /// record with an ORDT2 table and a single routing entry, then
    /// confirm the dumper decodes the label via the ORDT2 codepoint
    /// list.
    #[test]
    fn ordt2_label_decodes_via_table() {
        // Two-codepoint ORDT2 table: [U+0041 'A', U+0042 'B']. A label
        // of bytes [0x00, 0x01] should decode to "AB".
        let ordt2_cps: Vec<u32> = vec![0x0041, 0x0042];
        let label_bytes = vec![0x00u8, 0x01u8];

        // Minimum-viable INDX record.
        let header_length: u32 = 192;
        let mut rec = vec![0u8; 192];
        rec[0..4].copy_from_slice(b"INDX");
        rec[4..8].copy_from_slice(&header_length.to_be_bytes());
        // generation = 0 (routing)
        rec[12..16].copy_from_slice(&0u32.to_be_bytes());
        // const16 = 2
        rec[16..20].copy_from_slice(&2u32.to_be_bytes());
        // idxt_count = 1
        rec[24..28].copy_from_slice(&1u32.to_be_bytes());
        // ordt_type = 1
        rec[164..168].copy_from_slice(&1u32.to_be_bytes());
        // ordt_entries_count = 2
        rec[168..172].copy_from_slice(&2u32.to_be_bytes());

        // Entry starts at offset 192: [len=2][0x00][0x01][count=0 u16]
        let entry_offset = header_length as usize;
        rec.push(label_bytes.len() as u8);
        rec.extend_from_slice(&label_bytes);
        rec.extend_from_slice(&0u16.to_be_bytes());

        // IDXT follows.
        let idxt_offset = rec.len();
        rec.extend_from_slice(b"IDXT");
        rec.extend_from_slice(&(entry_offset as u16).to_be_bytes());
        // Write idxt_offset into header.
        let idxt_off_u32 = idxt_offset as u32;
        rec[20..24].copy_from_slice(&idxt_off_u32.to_be_bytes());

        // Pad to 4-byte boundary.
        while rec.len() % 4 != 0 {
            rec.push(0);
        }

        // ORDT2 blob: 2 pad + "ORDT"+4 filler + "ORDT" + N*u16 cps.
        let ordt_start = rec.len();
        rec.push(0);
        rec.push(0);
        rec.extend_from_slice(b"ORDT");
        rec.extend_from_slice(&[0, 0, 0, 0]);
        rec.extend_from_slice(b"ORDT");
        for cp in &ordt2_cps {
            rec.extend_from_slice(&(*cp as u16).to_be_bytes());
        }
        let ordt2_abs = ordt_start + 10;
        rec[176..180].copy_from_slice(&(ordt2_abs as u32).to_be_bytes());

        // Dump.
        let mut out = String::new();
        dump_indx_record(&mut out, 7, &rec, None, None);

        assert!(
            out.contains("indx[7].ordt_type = 1"),
            "ordt_type should be 1, got:\n{}",
            out
        );
        assert!(
            out.contains("indx[7].ordt2[0] = U+0041"),
            "ORDT2 should decode U+0041 at index 0, got:\n{}",
            out
        );
        assert!(
            out.contains("indx[7].ordt2[1] = U+0042"),
            "ORDT2 should decode U+0042 at index 1, got:\n{}",
            out
        );
        assert!(
            out.contains("routing_label = \"AB\""),
            "label should decode via ORDT2 to AB, got:\n{}",
            out
        );
    }
}
