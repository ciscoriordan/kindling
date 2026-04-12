//! Shared helpers for the structural round-trip and kindlegen parity tests.
//!
//! This module is included via `mod common;` from each integration test
//! binary (`tests/roundtrip.rs`, `tests/kindlegen_parity.rs`). Rust's test
//! layout builds each top-level file under `tests/` as its own binary, so
//! `tests/common/mod.rs` (a subdirectory with `mod.rs`) is the canonical way
//! to share code without getting a "common" test binary of its own.
//!
//! The parser is deliberately minimal: it knows enough about the PalmDB,
//! PalmDOC, MOBI header, EXTH, and INDX/SKEL/FRAG record layouts to let us
//! write equality assertions against kindling's output. It is NOT a general
//! MOBI reader; do not use it outside tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Path to the `kindling-cli` binary.
pub fn kindling_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kindling-cli")
}

/// `<repo>/tests/fixtures/parity/<name>`.
pub fn parity_fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join(name)
}

// ---------------------------------------------------------------------------
// Building fixtures with kindling-cli
// ---------------------------------------------------------------------------

/// Runs `kindling-cli build <opf> -o <out> --no-validate` and asserts the
/// child exits 0. On failure panics with a dump of stdout/stderr.
pub fn kindling_build(opf: &Path, out: &Path) {
    let status = Command::new(kindling_bin())
        .arg("build")
        .arg(opf)
        .arg("-o")
        .arg(out)
        .arg("--no-validate")
        .output()
        .expect("failed to spawn kindling-cli");
    if !status.status.success() {
        panic!(
            "kindling-cli build {} failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
            opf.display(),
            status.status.code(),
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr),
        );
    }
}

/// Runs `kindling-cli comic <input> -o <out>` (no --no-validate; the comic
/// path has no validation step). Default mode — kindling's "better than
/// kindlegen" output.
pub fn kindling_comic(input: &Path, out: &Path) {
    kindling_comic_inner(input, out, false);
}

/// Runs `kindling-cli comic --kindlegen-parity <input> -o <out>`. Used by
/// the parity-diff tests that want byte-for-byte kindlegen-equivalent
/// output to compare against the committed kindlegen reference.
pub fn kindling_comic_parity(input: &Path, out: &Path) {
    kindling_comic_inner(input, out, true);
}

fn kindling_comic_inner(input: &Path, out: &Path, parity: bool) {
    let mut cmd = Command::new(kindling_bin());
    cmd.arg("comic").arg(input).arg("-o").arg(out);
    if parity {
        cmd.arg("--kindlegen-parity");
    }
    let status = cmd.output().expect("failed to spawn kindling-cli comic");
    if !status.status.success() {
        panic!(
            "kindling-cli comic {} failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
            input.display(),
            status.status.code(),
            String::from_utf8_lossy(&status.stdout),
            String::from_utf8_lossy(&status.stderr),
        );
    }
}

// ---------------------------------------------------------------------------
// PalmDOC LZ77 decompression (inlined to avoid a cross-crate API change)
// ---------------------------------------------------------------------------

/// Decode a single PalmDOC-LZ77 compressed record. Silently accepts
/// truncation and bad escapes so parser errors show up via downstream
/// assertions, not panics.
pub fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        i += 1;
        if b == 0x00 || (0x09..=0x7F).contains(&b) {
            out.push(b);
        } else if (0x01..=0x08).contains(&b) {
            let n = b as usize;
            if i + n > src.len() {
                out.extend_from_slice(&src[i..]);
                break;
            }
            out.extend_from_slice(&src[i..i + n]);
            i += n;
        } else if (0x80..=0xBF).contains(&b) {
            if i >= src.len() {
                break;
            }
            let b2 = src[i];
            i += 1;
            let pair = (((b as u16) << 8) | b2 as u16) & 0x3FFF;
            let dist = (pair >> 3) as usize;
            let len = ((pair & 0x7) + 3) as usize;
            if dist == 0 || dist > out.len() {
                continue;
            }
            let start = out.len() - dist;
            for k in 0..len {
                let byte = out[start + k];
                out.push(byte);
            }
        } else {
            // 0xC0..=0xFF: space + printable (XOR 0x80).
            out.push(b' ');
            out.push(b ^ 0x80);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Byte helpers
// ---------------------------------------------------------------------------

pub fn u16_be(data: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([data[off], data[off + 1]])
}

pub fn u32_be(data: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

// ---------------------------------------------------------------------------
// PalmDB parse
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct PalmDb {
    pub name: String,
    pub creator: [u8; 4],
    pub ty: [u8; 4],
    pub num_records: usize,
    /// Offsets into the raw file, one per record.
    pub offsets: Vec<u32>,
}

impl PalmDb {
    pub fn record<'a>(&self, data: &'a [u8], idx: usize) -> &'a [u8] {
        let start = self.offsets[idx] as usize;
        let end = if idx + 1 < self.num_records {
            self.offsets[idx + 1] as usize
        } else {
            data.len()
        };
        &data[start..end]
    }

    pub fn assert_monotonic(&self) -> Result<(), String> {
        for (i, pair) in self.offsets.windows(2).enumerate() {
            if pair[1] <= pair[0] {
                return Err(format!(
                    "record offsets not monotonic at idx {i}: {} -> {}",
                    pair[0], pair[1]
                ));
            }
        }
        Ok(())
    }
}

pub fn parse_palmdb(data: &[u8]) -> Result<PalmDb, String> {
    if data.len() < 78 {
        return Err(format!("file {} bytes, need >= 78", data.len()));
    }
    let name_end = data[..32].iter().position(|&b| b == 0).unwrap_or(32);
    let name = String::from_utf8_lossy(&data[..name_end]).to_string();
    let mut ty = [0u8; 4];
    ty.copy_from_slice(&data[60..64]);
    let mut creator = [0u8; 4];
    creator.copy_from_slice(&data[64..68]);
    let num_records = u16_be(data, 76) as usize;
    if 78 + num_records * 8 > data.len() {
        return Err(format!(
            "record list needs {} bytes, file is {}",
            78 + num_records * 8,
            data.len()
        ));
    }
    let mut offsets = Vec::with_capacity(num_records);
    for i in 0..num_records {
        offsets.push(u32_be(data, 78 + i * 8));
    }
    Ok(PalmDb {
        name,
        creator,
        ty,
        num_records,
        offsets,
    })
}

// ---------------------------------------------------------------------------
// MOBI header + EXTH parse
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct MobiHeader {
    /// "MOBI" magic confirmed at offset 16 of Record 0.
    pub header_len: u32,
    pub mobi_type: u32,
    pub encoding: u32,
    pub unique_id: u32,
    pub file_version: u32,
    pub orth_index: u32,
    pub first_non_book_record: u32,
    pub locale: u32,
    pub min_version: u32,
    pub first_image_record: u32,
    pub exth_flags: u32,
    pub fdst_index: u32,
    pub fdst_count: u32,
    pub fcis_index: u32,
    pub flis_index: u32,
    pub skeleton_index: u32,
    pub fragment_index: u32,
    pub extra_data_flags: u32,
    pub compression: u16,
    pub text_length: u32,
    pub text_record_count: u16,
    pub record_size: u16,
}

#[derive(Debug, Clone)]
pub struct ExthRecord {
    pub rtype: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct MobiSection {
    pub record_idx: usize,
    pub header: MobiHeader,
    pub exth: Vec<ExthRecord>,
}

impl MobiSection {
    pub fn exth_first(&self, rtype: u32) -> Option<&[u8]> {
        self.exth
            .iter()
            .find(|r| r.rtype == rtype)
            .map(|r| r.data.as_slice())
    }

    pub fn exth_types(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.exth.iter().map(|r| r.rtype).collect();
        v.sort();
        v.dedup();
        v
    }
}

pub fn parse_mobi_section(rec0: &[u8], record_idx: usize) -> Result<MobiSection, String> {
    if rec0.len() < 16 + 24 {
        return Err(format!(
            "record {record_idx} too small for PalmDOC+MOBI header: {}",
            rec0.len()
        ));
    }
    let compression = u16_be(rec0, 0);
    let text_length = u32_be(rec0, 4);
    let text_record_count = u16_be(rec0, 8);
    let record_size = u16_be(rec0, 10);

    if &rec0[16..20] != b"MOBI" {
        return Err(format!(
            "record {record_idx}: expected MOBI magic at offset 16, got {:?}",
            &rec0[16..20.min(rec0.len())]
        ));
    }

    let header_len = u32_be(rec0, 20);
    let mobi_type = u32_be(rec0, 24);
    let encoding = u32_be(rec0, 28);
    let unique_id = u32_be(rec0, 32);
    let file_version = u32_be(rec0, 36);
    let orth_index = u32_be(rec0, 40);
    let first_non_book_record = u32_be(rec0, 80);
    let locale = u32_be(rec0, 92);
    let min_version = u32_be(rec0, 104);
    let first_image_record = u32_be(rec0, 108);
    let exth_flags = u32_be(rec0, 128);
    // KF8-specific header fields. Offsets below are rec0-relative; kindling's
    // writer (src/mobi.rs `build_mobi_header_kf8`) addresses the same fields
    // as MOBI-header-relative (base = rec0+16), so e.g. kindling's "mobi+176"
    // = rec0+192 for the FDST index. The older layout in this file had fdst
    // at rec0+176 and skeleton/fragment at rec0+228/236 (bogus zero values
    // on real kindling output), which made the book/comic roundtrip tests
    // fail with "skeleton_index 0 out of bounds". Corrected offsets match
    // `put32(&mut mobi, N, ...)` in src/mobi.rs shifted by +16.
    let fdst_index = u32_be(rec0, 192);
    let fdst_count = u32_be(rec0, 196);
    let fcis_index = u32_be(rec0, 200);
    let flis_index = u32_be(rec0, 208);
    let extra_data_flags = u32_be(rec0, 240);
    let fragment_index = u32_be(rec0, 248);
    let skeleton_index = u32_be(rec0, 252);

    let header = MobiHeader {
        header_len,
        mobi_type,
        encoding,
        unique_id,
        file_version,
        orth_index,
        first_non_book_record,
        locale,
        min_version,
        first_image_record,
        exth_flags,
        fdst_index,
        fdst_count,
        fcis_index,
        flis_index,
        skeleton_index,
        fragment_index,
        extra_data_flags,
        compression,
        text_length,
        text_record_count,
        record_size,
    };

    let mut exth = Vec::new();
    if exth_flags & 0x40 != 0 {
        let exth_off = 16 + header_len as usize;
        if rec0.len() < exth_off + 12 || &rec0[exth_off..exth_off + 4] != b"EXTH" {
            return Err(format!(
                "record {record_idx}: EXTH flag set but EXTH magic missing at offset {exth_off}"
            ));
        }
        let exth_len = u32_be(rec0, exth_off + 4) as usize;
        let exth_count = u32_be(rec0, exth_off + 8) as usize;
        let mut pos = exth_off + 12;
        let exth_end = exth_off + exth_len;
        for i in 0..exth_count {
            if pos + 8 > exth_end || pos + 8 > rec0.len() {
                return Err(format!(
                    "record {record_idx}: EXTH record list truncated at entry {i} pos {pos}"
                ));
            }
            let rtype = u32_be(rec0, pos);
            let rlen = u32_be(rec0, pos + 4) as usize;
            if rlen < 8 || pos + rlen > rec0.len() {
                return Err(format!(
                    "record {record_idx}: EXTH entry {i} (type {rtype}) has bad length {rlen}"
                ));
            }
            exth.push(ExthRecord {
                rtype,
                data: rec0[pos + 8..pos + rlen].to_vec(),
            });
            pos += rlen;
        }
    }

    Ok(MobiSection {
        record_idx,
        header,
        exth,
    })
}

// ---------------------------------------------------------------------------
// KF8 boundary / sections
// ---------------------------------------------------------------------------

/// Parsed view of a full MOBI file: PalmDB + one or two MOBI sections.
#[derive(Debug)]
pub struct ParsedMobi {
    pub raw: Vec<u8>,
    pub palmdb: PalmDb,
    pub kf7: MobiSection,
    /// Present on dual-format or KF8-only files where KF7.file_version >= 8
    /// (KF8-only) or EXTH 121 is set (dual format). The KF8 section lives
    /// at record `kf8_boundary` in KF7 or at record 0 in KF8-only.
    pub kf8: Option<MobiSection>,
    pub kf8_boundary: Option<u32>,
}

impl ParsedMobi {
    pub fn is_kf8_only(&self) -> bool {
        self.kf7.header.file_version >= 8 && self.kf8_boundary.is_none()
    }

    /// Record 0 of whichever section has the KF8 metadata (KF8-only: record 0;
    /// dual: record at boundary). None on pure KF7 files (dictionaries).
    pub fn kf8_or_kf7(&self) -> &MobiSection {
        self.kf8.as_ref().unwrap_or(&self.kf7)
    }
}

pub fn parse_mobi_file(data: &[u8]) -> Result<ParsedMobi, String> {
    let palmdb = parse_palmdb(data)?;
    palmdb.assert_monotonic()?;
    if &palmdb.ty != b"BOOK" {
        return Err(format!("type {:?} expected BOOK", palmdb.ty));
    }
    if &palmdb.creator != b"MOBI" {
        return Err(format!("creator {:?} expected MOBI", palmdb.creator));
    }
    let rec0 = palmdb.record(data, 0);
    let kf7 = parse_mobi_section(rec0, 0)?;
    let kf8_boundary = kf7
        .exth_first(121)
        .and_then(|d| if d.len() == 4 { Some(u32_be(d, 0)) } else { None });

    let kf8 = if let Some(b) = kf8_boundary {
        if (b as usize) < palmdb.num_records {
            let rec = palmdb.record(data, b as usize);
            Some(parse_mobi_section(rec, b as usize)?)
        } else {
            None
        }
    } else {
        None
    };

    Ok(ParsedMobi {
        raw: data.to_vec(),
        palmdb,
        kf7,
        kf8,
        kf8_boundary,
    })
}

/// Extract the decompressed text blob for a section starting at
/// `first_text_record` (always 1, by convention). Walks `text_record_count`
/// records, decompresses each, and trims trailing "extra data" bytes.
///
/// Kindle text records carry up to N trailing extra-data regions (one per
/// set bit in extra_data_flags). Kindling writes `extra_data_flags = 3`
/// into the MOBI header of KF7 record 0 and `1` into KF8 record 0. The
/// layout from end of record backward (matches libmobi's
/// `mobi_get_record_extrasize` in src/read.c): each high bit 15..=1 that
/// is set pulls a backward-varint-encoded region; bit 0 (multibyte) is
/// then read LAST from the remaining end position. Kindling's KF7 writer
/// puts 0x81 (VWI-encoded 1) at end-of-record and 0x00 (multibyte
/// overlap = 0) just before it — see the chunker in src/mobi.rs
/// compress_text_palmdoc().
pub fn extract_text_blob(parsed: &ParsedMobi, section: &MobiSection) -> Vec<u8> {
    let mut blob = Vec::new();
    // Synthesize the flags we KNOW kindling's layer uses per section kind.
    // We deliberately do NOT read the on-disk extra_data_flags here: KF7
    // records written before the parser at offset-240 was corrected had
    // garbage in that field, and even now it is a per-file toggle whose
    // value (1 vs 3) is equivalent to picking KF8 vs KF7 trailer shapes.
    let extra_data_flags: u32 = if section.header.file_version >= 8 {
        1 // KF8: multibyte only
    } else {
        3 // KF7: multibyte + TBS
    };
    let count = section.header.text_record_count as usize;
    let start = section.record_idx + 1;
    for i in 0..count {
        let rec_idx = start + i;
        if rec_idx >= parsed.palmdb.num_records {
            break;
        }
        let raw = parsed.palmdb.record(&parsed.raw, rec_idx);
        // libmobi's loop: position starts at the last byte; for bits
        // 15..=1 (descending) that are set, read one backward varint and
        // step back by (region total size); finally, for bit 0, read a
        // single byte and subtract (byte & 0x3) + 1. See
        // mobi_get_record_extrasize in libmobi src/read.c:481.
        let mut end = raw.len();
        for bit in (1..16).rev() {
            if extra_data_flags & (1 << bit) != 0 && end > 0 {
                let (_bytes, region_total) = read_backward_varint(&raw[..end]);
                if region_total > 0 && region_total <= end {
                    end -= region_total;
                }
            }
        }
        if extra_data_flags & 1 != 0 && end > 0 {
            // Multibyte overlap byte: low 2 bits = N, total strip = N+1.
            // kindling's chunker splits on codepoint boundaries so N = 0
            // in practice, but strip N+1 bytes to stay correct for any
            // input.
            let n = (raw[end - 1] & 0x3) as usize + 1;
            if end >= n {
                end -= n;
            }
        }
        let decompressed = if section.header.compression == 2 {
            palmdoc_decompress(&raw[..end])
        } else if section.header.compression == 1 {
            raw[..end].to_vec()
        } else {
            // HUFF/CDIC (17480) — not used by kindling. Give up gracefully.
            return blob;
        };
        blob.extend_from_slice(&decompressed);
    }
    blob
}

/// Read a trailing-region size varint from the end of `buf`, walking
/// backwards. MOBI encodes these with 7 bits per byte; the LAST byte of the
/// encoded value (which is the first byte we see when reading backwards) has
/// its high bit set, earlier bytes have the high bit clear. The returned
/// value is the total size of the trailing region in bytes INCLUDING the
/// varint itself.
fn read_backward_varint(buf: &[u8]) -> (usize, usize) {
    let mut value: usize = 0;
    let mut bytes = 0;
    for i in (0..buf.len()).rev() {
        let b = buf[i];
        bytes += 1;
        value = (value << 7) | (b & 0x7F) as usize;
        if b & 0x80 != 0 {
            break;
        }
        if bytes > 4 {
            break;
        }
    }
    (bytes, value)
}

// ---------------------------------------------------------------------------
// INDX record shallow parsing (for dict round-trips & parity diffs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IndxHeader {
    pub header_len: u32,
    pub kind: u32,
    pub idxt_count: u32,
    pub idxt_offset: u32,
    pub code_page: u32,
    pub num_records: u32,
}

pub fn parse_indx_header(rec: &[u8]) -> Result<IndxHeader, String> {
    if rec.len() < 48 || &rec[0..4] != b"INDX" {
        return Err(format!("not an INDX record (got {:?})", &rec[..4.min(rec.len())]));
    }
    Ok(IndxHeader {
        header_len: u32_be(rec, 4),
        // offset 8 is type; kindling uses "0x01" for normal INDX
        kind: u32_be(rec, 8),
        idxt_count: u32_be(rec, 24),
        idxt_offset: u32_be(rec, 20),
        code_page: u32_be(rec, 28),
        num_records: u32_be(rec, 36),
    })
}

/// Find every PalmDB record whose first 4 bytes are `INDX`. Returns their
/// record indices in order.
pub fn find_indx_records(parsed: &ParsedMobi) -> Vec<usize> {
    let mut out = Vec::new();
    for i in 0..parsed.palmdb.num_records {
        let rec = parsed.palmdb.record(&parsed.raw, i);
        if rec.len() >= 4 && &rec[0..4] == b"INDX" {
            out.push(i);
        }
    }
    out
}

/// Pull the label-prefix bytes of the first record in an INDX DATA record
/// (the record right after the primary INDX record). These are the first
/// bytes of the first entry key; for a dictionary the first key should be
/// the collation-order-first headword (e.g. "alpha" for our fixture).
pub fn first_indx_label_prefix(parsed: &ParsedMobi, primary_idx: usize, n: usize) -> Vec<u8> {
    let data_idx = primary_idx + 1;
    if data_idx >= parsed.palmdb.num_records {
        return Vec::new();
    }
    let rec = parsed.palmdb.record(&parsed.raw, data_idx);
    if rec.len() < 4 || &rec[..4] != b"INDX" {
        return Vec::new();
    }
    // DATA INDX layout: INDX header (192 bytes). Entries start right after
    // the header and end at the IDXT table.
    let header_len = if rec.len() >= 8 { u32_be(rec, 4) as usize } else { 0 };
    let idxt_off = if rec.len() >= 24 { u32_be(rec, 20) as usize } else { rec.len() };
    if header_len == 0 || idxt_off <= header_len || idxt_off > rec.len() {
        return Vec::new();
    }
    // The first entry starts at `header_len`, format: [label_len:u8][label][payload]
    let first = header_len;
    if first + 1 > rec.len() {
        return Vec::new();
    }
    let label_len = rec[first] as usize;
    let label_start = first + 1;
    let label_end = label_start + label_len.min(n);
    if label_end > rec.len() {
        return Vec::new();
    }
    rec[label_start..label_end].to_vec()
}

// ---------------------------------------------------------------------------
// ORDT2 parse + label decode (ported from libmobi src/index.c)
// ---------------------------------------------------------------------------
//
// libmobi reference functions (see /tmp/libmobi-ref/src/index.c, kept here
// for anyone tracing the port):
//
//   mobi_parse_ordt (index.c:84)
//   mobi_ordt_getbuffer (index.c:217)
//   mobi_ordt_lookup (index.c:236)
//   primary INDX header ORDT field reads (index.c:573+)
//
// Kindling's encoder counterpart lives in src/indx.rs `Ordt2Table` and
// `encode_indx_label_ordt2`. The on-disk ORDT2 layout kindling writes is:
//
//   [2 byte pad]
//   "ORDT" + 4 filler bytes       (ORDT1 placeholder, pointed at by offset 172)
//   "ORDT" + N u16 BE codepoints  (ORDT2 table, pointed at by offset 176)
//
// All offsets are relative to the primary INDX record start. Offset 164 is
// the ORDT type (1 = 1-byte label indices into ORDT2, 2 = 2-byte indices),
// offset 168 is the number of ORDT2 entries.

/// Parsed ORDT2 codepoint table and mode, extracted from a primary INDX
/// record. Mirrors libmobi's `MOBIOrdt` but only carries the fields we
/// actually use.
#[derive(Debug, Clone)]
pub struct Ordt2Info {
    /// ORDT type: 1 = 1-byte indices into `codepoints`, 2 = 2-byte indices.
    pub ordt_type: u32,
    /// Codepoint table. `codepoints[i]` is the Unicode codepoint that
    /// label-byte-index `i` maps to. Entries are u16 BE on disk, so all
    /// values are BMP codepoints (<= 0xFFFF).
    pub codepoints: Vec<u32>,
}

/// Parse an ORDT2 table out of the primary INDX record if one is present.
///
/// Returns `None` if the header doesn't declare an ORDT, the pointers
/// don't land on "ORDT" magic, or the declared entries overflow the
/// record. Ported from the primary-INDX field reads in libmobi
/// `mobi_parse_index` (src/index.c:573) + `mobi_parse_ordt`.
pub fn parse_indx_ordt2(primary_rec: &[u8]) -> Option<Ordt2Info> {
    if primary_rec.len() < 184 || &primary_rec[..4] != b"INDX" {
        return None;
    }
    let ordt_type = u32_be(primary_rec, 164);
    let entries_count = u32_be(primary_rec, 168) as usize;
    let ordt1_off = u32_be(primary_rec, 172) as usize;
    let ordt2_off = u32_be(primary_rec, 176) as usize;
    // Reject obvious "no ORDT" values. kindlegen-style dicts without an
    // ORDT table tend to leave 164..180 as zeros or 0xFFFFFFFF, in which
    // case we bail out early rather than try to follow garbage pointers.
    if ordt_type == 0
        || ordt_type > 2
        || entries_count == 0
        || entries_count > 4096
        || ordt2_off < 192
        || ordt2_off >= primary_rec.len()
    {
        return None;
    }
    // We don't actually use ORDT1 (libmobi doesn't either beyond logging),
    // but we still follow the pointer to validate the magic for sanity.
    let _ = ordt1_off;
    if primary_rec.len() < ordt2_off + 4 + entries_count * 2 {
        return None;
    }
    if &primary_rec[ordt2_off..ordt2_off + 4] != b"ORDT" {
        return None;
    }
    let table_start = ordt2_off + 4;
    let mut codepoints = Vec::with_capacity(entries_count);
    for i in 0..entries_count {
        codepoints.push(u16_be(primary_rec, table_start + i * 2) as u32);
    }
    Some(Ordt2Info {
        ordt_type,
        codepoints,
    })
}

/// Decode a run of INDX label bytes into a UTF-8 string using an ORDT2
/// table. Mirrors the inner loop of libmobi's `mobi_getstring_ordt`
/// (src/index.c:255) without the ligature/surrogate complexity, which
/// kindling's ORDT2 encoder rejects up-front (BMP-only, no ligatures).
///
/// For `ordt_type == 1`, every label byte is treated as a 1-byte index
/// into `ordt.codepoints`. For `ordt_type == 2`, label bytes are read
/// 2 at a time as big-endian u16 indices. If an index falls outside the
/// table range, the raw numeric value is used as the codepoint directly —
/// this matches libmobi's `mobi_ordt_lookup` fallback.
///
/// Label bytes are bounded by `label_bytes.len()`; the caller is
/// expected to have already sliced off the `[label_len]` prefix and any
/// trailing tag/control bytes.
pub fn decode_indx_label_ordt2(label_bytes: &[u8], ordt: &Ordt2Info) -> String {
    let mut out = String::with_capacity(label_bytes.len());
    let mut i = 0;
    let step = if ordt.ordt_type == 2 { 2 } else { 1 };
    while i + step <= label_bytes.len() {
        let idx: u32 = if ordt.ordt_type == 2 {
            ((label_bytes[i] as u32) << 8) | label_bytes[i + 1] as u32
        } else {
            label_bytes[i] as u32
        };
        i += step;
        let cp = if (idx as usize) < ordt.codepoints.len() {
            ordt.codepoints[idx as usize]
        } else {
            idx
        };
        if let Some(c) = char::from_u32(cp) {
            out.push(c);
        } else {
            out.push('\u{FFFD}');
        }
    }
    out
}

/// Test-only inline encoder that mirrors kindling's
/// `encode_indx_label_ordt2` (src/indx.rs:219). It exists here because
/// that function is `pub(crate)` and the integration-test helper lives
/// in a separate crate. Encoding is trivially:
///
///   1. Build the sorted unique BMP codepoint set across all labels.
///   2. Assign each codepoint an index (0..N-1).
///   3. For each input label, emit one byte per character, the byte
///      being the assigned index.
///
/// Returns (codepoint table, per-label encoded bytes). All inputs must
/// be BMP; supplementary-plane codepoints or >255 unique codepoints
/// return None.
pub fn test_encode_ordt2_labels(labels: &[&str]) -> Option<(Vec<u32>, Vec<Vec<u8>>)> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<u32> = BTreeSet::new();
    for l in labels {
        for c in l.chars() {
            let cp = c as u32;
            if cp > 0xFFFF {
                return None;
            }
            set.insert(cp);
        }
    }
    if set.len() > 255 {
        return None;
    }
    let codepoints: Vec<u32> = set.into_iter().collect();
    let index_of = |c: char| -> Option<u8> {
        codepoints
            .iter()
            .position(|&cp| cp == c as u32)
            .map(|i| i as u8)
    };
    let mut encoded: Vec<Vec<u8>> = Vec::with_capacity(labels.len());
    for l in labels {
        let mut bytes = Vec::with_capacity(l.chars().count());
        for c in l.chars() {
            bytes.push(index_of(c)?);
        }
        encoded.push(bytes);
    }
    Some((codepoints, encoded))
}

// ---------------------------------------------------------------------------
// Forward varlen reader (libmobi mobi_buffer_get_varlen, src/buffer.c:299)
// ---------------------------------------------------------------------------
//
// Used by INDX entry parsing to read tag values out of an entry's
// payload. Each byte carries 7 data bits; the byte with bit 7 SET
// terminates the value (the "inverted VWI" convention kindling's writer
// uses, mirroring kindlegen). Returns (value, bytes_consumed). libmobi
// caps at 4 bytes; we do the same.
fn read_forward_varlen(buf: &[u8], pos: usize) -> Option<(u32, usize)> {
    let mut val: u32 = 0;
    let mut consumed = 0usize;
    while consumed < 4 && pos + consumed < buf.len() {
        let byte = buf[pos + consumed];
        consumed += 1;
        val = (val << 7) | (byte & 0x7F) as u32;
        if byte & 0x80 != 0 {
            return Some((val, consumed));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// INDX TAGX + entry parsing (libmobi src/index.c)
// ---------------------------------------------------------------------------
//
// Ported just enough to walk SKEL and FRAG primary+data INDX records and
// pull per-entry tag values. Reference functions in libmobi:
//   mobi_parse_indx        (src/index.c:512)
//   mobi_parse_tagx        (src/index.c:135)
//   mobi_parse_idxt        (src/index.c:188)
//   mobi_parse_index_entry (src/index.c:340)
//
// Limitations vs libmobi:
//   - Assumes control_byte_count == 1 (kindling and kindlegen both write
//     a single control byte for SKEL/FRAG/NCX).
//   - No ORDT decoding for labels (SKEL/FRAG/NCX labels are decimal
//     ASCII strings or short identifiers, never go through ORDT).
//   - Returns raw label bytes; the caller decides how to interpret them
//     (decimal-string-to-int for FRAG insert positions, etc).

#[derive(Debug, Clone, Copy)]
pub struct TagxTag {
    pub tag: u8,
    pub values_per_entry: u8,
    pub bitmask: u8,
    /// 0 = normal tag definition; 1 = end-of-control-byte sentinel.
    pub control_byte: u8,
}

#[derive(Debug, Clone)]
pub struct IndxEntryTag {
    pub tag_id: u8,
    pub values: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct IndxEntry {
    pub label: Vec<u8>,
    pub tags: Vec<IndxEntryTag>,
}

#[derive(Debug, Clone)]
pub struct ParsedIndx {
    pub tagx: Vec<TagxTag>,
    pub control_byte_count: usize,
    pub total_entries: u32,
    pub entries: Vec<IndxEntry>,
}

impl IndxEntry {
    /// Get the `value_idx`-th value of `tag_id` for this entry, like
    /// libmobi's `mobi_get_indxentry_tagvalue` (where INDX_TAG_*_LENGTH
    /// is `(tag, 1)` and INDX_TAG_*_POSITION is `(tag, 0)`).
    pub fn tag_value(&self, tag_id: u8, value_idx: usize) -> Option<u32> {
        let t = self.tags.iter().find(|t| t.tag_id == tag_id)?;
        t.values.get(value_idx).copied()
    }
}

/// Parse a SKEL/FRAG/NCX-style INDX (primary record + N data records).
///
/// `primary_idx` is the PalmDB record index of the primary INDX record.
/// The data records are assumed to follow consecutively (record indices
/// `primary_idx+1`, `primary_idx+2`, ...). The number of data records
/// is read from the primary header at offset 24.
pub fn parse_indx(parsed: &ParsedMobi, primary_idx: usize) -> Result<ParsedIndx, String> {
    if primary_idx >= parsed.palmdb.num_records {
        return Err(format!("primary_idx {primary_idx} out of bounds"));
    }
    let primary = parsed.palmdb.record(&parsed.raw, primary_idx);
    if primary.len() < 192 || &primary[0..4] != b"INDX" {
        return Err(format!(
            "record {primary_idx} is not an INDX primary (got magic {:?})",
            &primary[..4.min(primary.len())]
        ));
    }
    let num_data_records = u32_be(primary, 24) as usize;
    let total_entries = u32_be(primary, 36);
    let tagx_offset = u32_be(primary, 180) as usize;
    if tagx_offset + 12 > primary.len() || &primary[tagx_offset..tagx_offset + 4] != b"TAGX" {
        return Err(format!(
            "INDX primary {primary_idx}: TAGX magic missing at offset {tagx_offset}"
        ));
    }
    let tagx_total = u32_be(primary, tagx_offset + 4) as usize;
    let control_byte_count = u32_be(primary, tagx_offset + 8) as usize;
    if tagx_total < 12 || tagx_offset + tagx_total > primary.len() {
        return Err(format!(
            "INDX primary {primary_idx}: TAGX length {tagx_total} out of range"
        ));
    }
    let tagx_data_len = (tagx_total - 12) / 4;
    let mut tagx = Vec::with_capacity(tagx_data_len);
    for i in 0..tagx_data_len {
        let off = tagx_offset + 12 + i * 4;
        tagx.push(TagxTag {
            tag: primary[off],
            values_per_entry: primary[off + 1],
            bitmask: primary[off + 2],
            control_byte: primary[off + 3],
        });
    }
    // Strip the (0,0,0,1) sentinel that marks the end of the table.
    if let Some(last) = tagx.last() {
        if last.tag == 0
            && last.values_per_entry == 0
            && last.bitmask == 0
            && last.control_byte == 1
        {
            tagx.pop();
        }
    }

    let mut entries = Vec::with_capacity(total_entries as usize);
    for di in 0..num_data_records {
        let rec_idx = primary_idx + 1 + di;
        if rec_idx >= parsed.palmdb.num_records {
            return Err(format!(
                "INDX data record {rec_idx} (primary {primary_idx} + {di}) out of bounds"
            ));
        }
        let rec = parsed.palmdb.record(&parsed.raw, rec_idx);
        parse_indx_data_record(rec, &tagx, control_byte_count, &mut entries)
            .map_err(|e| format!("INDX data record {rec_idx}: {e}"))?;
    }

    Ok(ParsedIndx {
        tagx,
        control_byte_count,
        total_entries,
        entries,
    })
}

fn parse_indx_data_record(
    rec: &[u8],
    tagx: &[TagxTag],
    control_byte_count: usize,
    out: &mut Vec<IndxEntry>,
) -> Result<(), String> {
    if rec.len() < 28 || &rec[0..4] != b"INDX" {
        return Err(format!(
            "not an INDX data record (got {:?})",
            &rec[..4.min(rec.len())]
        ));
    }
    let idxt_off = u32_be(rec, 20) as usize;
    let entries_count = u32_be(rec, 24) as usize;
    if idxt_off + 4 + 2 * entries_count > rec.len() {
        return Err(format!(
            "IDXT footer overflow: idxt_off={idxt_off}, count={entries_count}, rec_len={}",
            rec.len()
        ));
    }
    if &rec[idxt_off..idxt_off + 4] != b"IDXT" {
        return Err(format!("IDXT magic missing at offset {idxt_off}"));
    }
    let mut offsets: Vec<usize> = Vec::with_capacity(entries_count + 1);
    for i in 0..entries_count {
        offsets.push(u16_be(rec, idxt_off + 4 + i * 2) as usize);
    }
    // Sentinel: end of last entry == start of IDXT magic.
    offsets.push(idxt_off);

    for i in 0..entries_count {
        let entry_start = offsets[i];
        let entry_end = offsets[i + 1];
        if entry_end <= entry_start || entry_end > rec.len() {
            return Err(format!(
                "entry {i}: offset window [{entry_start}..{entry_end}] invalid (rec_len={})",
                rec.len()
            ));
        }
        let entry = parse_indx_entry(&rec[entry_start..entry_end], tagx, control_byte_count)
            .map_err(|e| format!("entry {i}: {e}"))?;
        out.push(entry);
    }
    Ok(())
}

fn parse_indx_entry(
    entry: &[u8],
    tagx: &[TagxTag],
    control_byte_count: usize,
) -> Result<IndxEntry, String> {
    if entry.is_empty() {
        return Err("empty entry".to_string());
    }
    let label_len = entry[0] as usize;
    if 1 + label_len + control_byte_count > entry.len() {
        return Err(format!(
            "entry truncated: label_len={label_len}, control_byte_count={control_byte_count}, entry_len={}",
            entry.len()
        ));
    }
    let label = entry[1..1 + label_len].to_vec();
    let mut cursor = 1 + label_len;
    let control_bytes = entry[cursor..cursor + control_byte_count].to_vec();
    cursor += control_byte_count;
    let mut control_idx = 0usize;

    // First pass: walk TAGX, build a per-tag (value_count, value_bytes)
    // ptagx list the way libmobi mobi_parse_index_entry does.
    struct PtagxRow {
        tag: u8,
        values_per_entry: u8,
        // Some(n) => the entry stores n*values_per_entry varlens.
        // None    => the entry stores `value_bytes` worth of varlens
        //            (read until consumed).
        value_count: Option<u32>,
        value_bytes: Option<u32>,
    }
    let mut ptagx: Vec<PtagxRow> = Vec::with_capacity(tagx.len());
    for t in tagx {
        if t.control_byte == 1 {
            control_idx += 1;
            continue;
        }
        if control_idx >= control_bytes.len() {
            break;
        }
        let cb = control_bytes[control_idx];
        let masked = cb & t.bitmask;
        if masked == 0 {
            continue;
        }
        if masked == t.bitmask {
            // All mask bits set.
            if t.bitmask.count_ones() > 1 {
                // Multi-bit mask "all set" => read value_bytes from the entry.
                let (vbytes, n) = read_forward_varlen(entry, cursor)
                    .ok_or_else(|| format!("varlen read failed at {cursor} (tag={})", t.tag))?;
                cursor += n;
                ptagx.push(PtagxRow {
                    tag: t.tag,
                    values_per_entry: t.values_per_entry,
                    value_count: None,
                    value_bytes: Some(vbytes),
                });
            } else {
                ptagx.push(PtagxRow {
                    tag: t.tag,
                    values_per_entry: t.values_per_entry,
                    value_count: Some(1),
                    value_bytes: None,
                });
            }
        } else {
            // Some bits set: shift mask down to recover the count.
            let mut shifted = masked;
            let mut mask = t.bitmask;
            while mask & 1 == 0 {
                mask >>= 1;
                shifted >>= 1;
            }
            ptagx.push(PtagxRow {
                tag: t.tag,
                values_per_entry: t.values_per_entry,
                value_count: Some(shifted as u32),
                value_bytes: None,
            });
        }
    }

    // Second pass: actually read the varlen values from `cursor` onward.
    let mut tags: Vec<IndxEntryTag> = Vec::with_capacity(ptagx.len());
    for row in &ptagx {
        let mut values: Vec<u32> = Vec::new();
        if let Some(vc) = row.value_count {
            let total = vc as usize * row.values_per_entry as usize;
            for _ in 0..total {
                let (v, n) = read_forward_varlen(entry, cursor).ok_or_else(|| {
                    format!("varlen read failed mid-tag {} at {cursor}", row.tag)
                })?;
                cursor += n;
                values.push(v);
            }
        } else {
            let target = row.value_bytes.unwrap_or(0) as usize;
            let mut consumed = 0usize;
            while consumed < target {
                let (v, n) = read_forward_varlen(entry, cursor).ok_or_else(|| {
                    format!("varlen-bytes read failed mid-tag {} at {cursor}", row.tag)
                })?;
                cursor += n;
                consumed += n;
                values.push(v);
            }
        }
        tags.push(IndxEntryTag {
            tag_id: row.tag,
            values,
        });
    }
    Ok(IndxEntry { label, tags })
}

// ---------------------------------------------------------------------------
// FDST parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct FdstFlow {
    pub start: u32,
    pub end: u32,
}

/// Parse the FDST record. Layout: `FDST` magic + u32 entry-table offset
/// (kindling and kindlegen both write 12) + u32 flow_count + flow_count
/// * (u32 start, u32 end). See kindling::kf8::build_fdst.
pub fn parse_fdst(rec: &[u8]) -> Result<Vec<FdstFlow>, String> {
    if rec.len() < 12 || &rec[0..4] != b"FDST" {
        return Err(format!(
            "not an FDST record (got {:?})",
            &rec[..4.min(rec.len())]
        ));
    }
    let entries_off = u32_be(rec, 4) as usize;
    let flow_count = u32_be(rec, 8) as usize;
    if entries_off + flow_count * 8 > rec.len() {
        return Err(format!(
            "FDST truncated: entries_off={entries_off}, flow_count={flow_count}, rec_len={}",
            rec.len()
        ));
    }
    let mut flows = Vec::with_capacity(flow_count);
    for i in 0..flow_count {
        let off = entries_off + i * 8;
        flows.push(FdstFlow {
            start: u32_be(rec, off),
            end: u32_be(rec, off + 4),
        });
    }
    Ok(flows)
}

// ---------------------------------------------------------------------------
// KF8 skeleton/fragment reconstruction
// ---------------------------------------------------------------------------
//
// Port of libmobi `mobi_reconstruct_parts` (src/parse_rawml.c:784).
// Walks the SKEL and FRAG indexes, splices fragment bytes into skeleton
// shells at the byte offsets encoded in the FRAG entry labels, and
// returns one Vec<u8> per skeleton (one HTML file per spine entry).
//
// Tag layout (kindling::kf8 writer / Calibre writer8/index.py):
//   SKEL: tag 1 = chunk_count            (vpe=1, mask=3)
//         tag 6 = (start_pos, length)    (vpe=2, mask=12)
//   FRAG: tag 2 = aid_cncx_offset        (vpe=1, mask=1)
//         tag 3 = file_number            (vpe=1, mask=2)
//         tag 4 = sequence_number        (vpe=1, mask=4)
//         tag 6 = (frag_position, frag_length)  (vpe=2, mask=8)
//
// The FRAG entry's LABEL is a decimal ASCII string giving the GLOBAL
// byte offset in the reconstructed flow at which to splice the
// fragment. Within each skeleton, fragments are stored sequentially in
// the rawml flow immediately after the skeleton bytes, in the order
// they will be spliced (libmobi consumes them via mobi_buffer_getpointer
// after a single setpos at skel_position).
pub fn reconstruct_kf8_parts(
    flow0: &[u8],
    skel: &ParsedIndx,
    frag: &ParsedIndx,
) -> Result<Vec<Vec<u8>>, String> {
    let mut parts: Vec<Vec<u8>> = Vec::with_capacity(skel.entries.len());
    let mut buf_pos: usize = 0;
    let mut frag_idx: usize = 0;
    let mut curr_position: usize = 0;
    let total_frags = frag.entries.len();
    for (i, sk) in skel.entries.iter().enumerate() {
        let chunk_count = sk
            .tag_value(1, 0)
            .ok_or_else(|| format!("SKEL entry {i}: missing tag 1 (chunk_count)"))?;
        let skel_pos = sk
            .tag_value(6, 0)
            .ok_or_else(|| format!("SKEL entry {i}: missing tag 6 idx 0 (position)"))?
            as usize;
        let mut skel_len = sk
            .tag_value(6, 1)
            .ok_or_else(|| format!("SKEL entry {i}: missing tag 6 idx 1 (length)"))?
            as usize;
        if skel_pos + skel_len > flow0.len() {
            return Err(format!(
                "SKEL entry {i}: skeleton bytes [{skel_pos}..{}) overflow flow0 ({} bytes)",
                skel_pos + skel_len,
                flow0.len()
            ));
        }
        let mut assembled = flow0[skel_pos..skel_pos + skel_len].to_vec();
        // libmobi: mobi_buffer_setpos(buf, skel_position); then read
        // skel_length, then for each fragment read frag_length more.
        buf_pos = skel_pos + skel_len;

        for f in 0..chunk_count as usize {
            if frag_idx >= total_frags {
                return Err(format!(
                    "SKEL entry {i} fragment {f}: out of FRAG entries ({frag_idx}/{total_frags})"
                ));
            }
            let fe = &frag.entries[frag_idx];
            let label_str = std::str::from_utf8(&fe.label)
                .map_err(|e| format!("FRAG entry {frag_idx}: label not UTF-8: {e}"))?;
            let global_insert: usize = label_str.trim().parse().map_err(|e| {
                format!("FRAG entry {frag_idx}: label {label_str:?} not decimal: {e}")
            })?;
            if global_insert < curr_position {
                return Err(format!(
                    "FRAG entry {frag_idx}: insert position {global_insert} < part start {curr_position}"
                ));
            }
            let file_number = fe
                .tag_value(3, 0)
                .ok_or_else(|| format!("FRAG entry {frag_idx}: missing tag 3 (file_number)"))?;
            if file_number as usize != i {
                return Err(format!(
                    "FRAG entry {frag_idx}: file_number={file_number} but expected SKEL idx {i}"
                ));
            }
            let frag_len = fe
                .tag_value(6, 1)
                .ok_or_else(|| format!("FRAG entry {frag_idx}: missing tag 6 idx 1 (length)"))?
                as usize;

            // libmobi clamps over-the-end inserts to current grown skel_len.
            let mut local_insert = global_insert - curr_position;
            if local_insert > skel_len {
                local_insert = skel_len;
            }
            if buf_pos + frag_len > flow0.len() {
                return Err(format!(
                    "FRAG entry {frag_idx}: fragment bytes [{buf_pos}..{}) overflow flow0 ({} bytes)",
                    buf_pos + frag_len,
                    flow0.len()
                ));
            }
            let frag_bytes = flow0[buf_pos..buf_pos + frag_len].to_vec();
            buf_pos += frag_len;

            assembled.splice(local_insert..local_insert, frag_bytes.iter().copied());
            skel_len += frag_len;
            frag_idx += 1;
        }

        parts.push(assembled);
        curr_position += skel_len;
    }
    if frag_idx != total_frags {
        return Err(format!(
            "consumed {frag_idx} of {total_frags} FRAG entries; some fragments unattached"
        ));
    }
    let _ = buf_pos;
    Ok(parts)
}

/// Convenience: extract decompressed text from the KF8 section, parse
/// the FDST + SKEL + FRAG indexes, and run reconstruct_kf8_parts. Used
/// by parity tests that want to compare reassembled per-page HTML
/// across two MOBI files.
///
/// Two layout quirks this helper hides:
///   1. The KF8-section MOBI header writes SKEL/FRAG/FDST/etc record
///      indices RELATIVE TO the KF8 boundary, not as absolute PalmDB
///      indices. For dual KF7+KF8 files we add `kf8.record_idx`. For
///      KF8-only files `record_idx == 0` so this is a no-op.
///   2. Comic-style KF8 builds emit only one flow (HTML) and may omit
///      the FDST record entirely. When the FDST pointer doesn't land on
///      a real FDST magic we treat the whole decompressed text as flow 0.
pub fn reconstruct_parts_from_mobi(parsed: &ParsedMobi) -> Result<Vec<Vec<u8>>, String> {
    let kf8 = parsed.kf8_or_kf7();
    let section_base = kf8.record_idx;
    let text = extract_text_blob(parsed, kf8);

    // Resolve flow 0. If FDST is present and valid, slice it; otherwise
    // fall back to the entire decompressed text (1-flow comic case).
    let fdst_raw = kf8.header.fdst_index;
    let mut flow0_owned: Vec<u8> = text.clone();
    if fdst_raw != 0xFFFFFFFF {
        let fdst_abs = section_base + fdst_raw as usize;
        if fdst_abs < parsed.palmdb.num_records {
            let rec = parsed.palmdb.record(&parsed.raw, fdst_abs);
            if rec.len() >= 4 && &rec[..4] == b"FDST" {
                let flows = parse_fdst(rec)?;
                if let Some(f0) = flows.first() {
                    let start = f0.start as usize;
                    let end = f0.end as usize;
                    if end <= text.len() && start <= end {
                        flow0_owned = text[start..end].to_vec();
                    } else {
                        return Err(format!(
                            "FDST flow 0 [{start}..{end}) out of range for text ({} bytes)",
                            text.len()
                        ));
                    }
                }
            }
        }
    }

    let skel_raw = kf8.header.skeleton_index;
    let frag_raw = kf8.header.fragment_index;
    if skel_raw == 0xFFFFFFFF || frag_raw == 0xFFFFFFFF {
        return Err(format!(
            "missing skeleton/fragment index in KF8: skel={skel_raw} frag={frag_raw}"
        ));
    }
    let skel_abs = section_base + skel_raw as usize;
    let frag_abs = section_base + frag_raw as usize;
    let skel = parse_indx(parsed, skel_abs)?;
    let frag = parse_indx(parsed, frag_abs)?;

    reconstruct_kf8_parts(&flow0_owned, &skel, &frag)
}

// ---------------------------------------------------------------------------
// Tests for the ported ORDT2 decode path
// ---------------------------------------------------------------------------
//
// These `#[test]` functions are compiled into every integration-test
// binary that includes `mod common;` (currently roundtrip.rs and
// kindlegen_parity.rs). That means they run twice per cargo test
// invocation, which is harmless: each copy asserts the same thing and
// they don't share state. The alternative (a dedicated tests/ binary)
// would violate the "edit exactly one file" scope constraint.

#[test]
fn ordt2_roundtrip_latin_labels() {
    // Tiny end-to-end: encode three ASCII labels with the inline
    // encoder, build a synthetic ORDT2 info, decode each, assert equal.
    let labels = ["alpha", "bravo", "charlie"];
    let (codepoints, encoded) =
        test_encode_ordt2_labels(&labels).expect("encode must succeed for ASCII BMP input");
    let info = Ordt2Info {
        ordt_type: 1,
        codepoints,
    };
    for (i, label) in labels.iter().enumerate() {
        let decoded = decode_indx_label_ordt2(&encoded[i], &info);
        assert_eq!(
            &decoded, label,
            "ordt2 roundtrip failed for label {label:?} (encoded={:?}, decoded={decoded:?})",
            encoded[i]
        );
    }
}

#[test]
fn ordt2_roundtrip_greek_bmp_labels() {
    // Round-trip the Greek popup-bug case. All characters are escaped
    // codepoints (the source file stays 7-bit ASCII so logs and PR
    // diffs don't mangle them). This mirrors the set of codepoints
    // kindling's encoder sees for the Greek dictionary build that
    // originally flagged the popup-routing bug.
    //
    //   \u{03B1} a (small alpha)
    //   \u{03B2} b (small beta)
    //   \u{03B3} g (small gamma)
    //   \u{03C3} s (small sigma)
    //   \u{03C2} s (small final sigma)
    let labels = [
        "\u{03B1}\u{03BB}\u{03C6}\u{03B1}",             // alpha
        "\u{03B2}\u{03AE}\u{03C4}\u{03B1}",             // accented vowels included
        "\u{03B3}\u{03AC}\u{03BC}\u{03BC}\u{03B1}",     //
        "\u{03C3}\u{03AF}\u{03B3}\u{03BC}\u{03B1}",     //
        "\u{03BB}\u{03CC}\u{03B3}\u{03BF}\u{03C2}",     // ends in final sigma
    ];
    let (codepoints, encoded) = test_encode_ordt2_labels(&labels)
        .expect("inline encoder must accept BMP Greek labels");
    assert!(
        codepoints.iter().all(|&cp| cp <= 0xFFFF),
        "all codepoints must be BMP"
    );
    let info = Ordt2Info {
        ordt_type: 1,
        codepoints,
    };
    for (i, label) in labels.iter().enumerate() {
        let decoded = decode_indx_label_ordt2(&encoded[i], &info);
        assert_eq!(
            &decoded, label,
            "greek ordt2 roundtrip failed at index {i}: encoded={:?} decoded={decoded:?}",
            encoded[i]
        );
    }
}

#[test]
fn ordt2_decode_2byte_path() {
    // Synthetic 2-byte index path: hand-build a codepoint table with
    // more than 256 entries so indices must be encoded as u16 BE.
    let codepoints: Vec<u32> = (0x3000..0x3120).map(|cp| cp as u32).collect();
    assert!(codepoints.len() > 256);
    let info = Ordt2Info {
        ordt_type: 2,
        codepoints: codepoints.clone(),
    };
    // Encode 3 codepoints at indices 0, 10, 0x11F.
    let mut bytes = Vec::new();
    for &idx in &[0u16, 10, 0x11F] {
        bytes.extend_from_slice(&idx.to_be_bytes());
    }
    let decoded = decode_indx_label_ordt2(&bytes, &info);
    let expected: String = [codepoints[0], codepoints[10], codepoints[0x11F]]
        .iter()
        .map(|&cp| char::from_u32(cp).unwrap())
        .collect();
    assert_eq!(decoded, expected);
}
