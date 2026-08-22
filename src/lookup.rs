//! Firmware-faithful dictionary lookup simulator (Tier 1).
//!
//! Given a built dictionary MOBI and a tapped query string, this reproduces
//! the on-device lookup as closely as we understand it: parse the orth INDX,
//! read the collation the firmware would use from the primary header, apply
//! the same query normalization the firmware applies, and report which stored
//! label resolves (and to what text position) or that nothing does.
//!
//! What it is and is not. This is a regression harness for kindling's own
//! index construction, not a hardware oracle. Its fidelity is bounded by our
//! understanding of the firmware, so it can only catch encode-side mistakes
//! (label sort order, missing aliases, ORDT symbol numbering), never discover
//! unknown firmware behavior. Where the model encodes a firmware assumption it
//! is called out inline. The collation weights and fold classes it relies on
//! are lifted from Amazon's own tables (`crate::ordt::fold_base`, validated
//! against the SPL1 blob; the ORDT tables embedded in the file itself), so the
//! normalization is grounded in Amazon data rather than invented here.
//!
//! Modes, keyed off the orth primary header:
//!   - Greek/Latin fold (spl_count > 0): the query and every label fold through
//!     the firmware's accent+case fold (`crate::ordt::folded_sort_key`), so
//!     `meme` resolves `même`.
//!   - Generated / exact ORDT (oentries > 0, spl_count 0): labels are ORDT
//!     symbol sequences decoded through the embedded ORDT2 table. Latin-script
//!     labels still fold (exact-accent default sorts folded); CJK/Arabic labels
//!     match by literal code point.
//!   - Plain UTF-16BE (oentries 0): Cyrillic and friends. The firmware folds
//!     case on the query side but not the label side, which is exactly why an
//!     all-caps ФСБ needs a lowercase alias and a stressed form needs its bare
//!     spelling (issues #8 and #17). Modeled by matching the query's lowercased
//!     and stress-stripped forms against the labels as stored.
//!
//! Finding the index. The MOBI header names the orth index record at offset
//! 0x18, but that pointer cannot be trusted on files kindling did not write:
//! a record number that was not adjusted for records inserted ahead of it
//! lands on something else entirely, and every query then misses with nothing
//! to say why (issue #49). So the pointer is verified before it is used, and
//! the index is otherwise found by its own signature (see
//! [`orth_index_name`]). [`report`] says which record was used and what the
//! header claimed, so a stale pointer is visible rather than silent.

use crate::huffcdic::COMPRESSION_HUFFDIC;
use crate::ordt::folded_sort_key;

/// A resolved lookup: the stored label that matched and the text position its
/// entry points at (the start of the headword's record text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupResult {
    pub matched_label: String,
    pub position: u32,
}

/// What the simulator found, including why a miss missed.
///
/// [`lookup`] answers only "did it resolve"; this carries the detail the CLI
/// needs to tell a dictionary with no matching headword apart from a file that
/// has no dictionary index at all, or one whose index pointer is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupReport {
    /// The match, when the query resolved.
    pub result: Option<LookupResult>,
    /// Record number of the orth index actually used, if one was found.
    pub index_record: Option<usize>,
    /// Record number the MOBI header declared for the orth index. `None` when
    /// the header says there is no dictionary (`0xFFFFFFFF`) or the file could
    /// not be parsed.
    pub declared_index_record: Option<u32>,
    /// Number of headword labels the index holds.
    pub entries: usize,
    /// PalmDOC compression type from record 0: 1 uncompressed, 2 PalmDOC LZ77,
    /// 17480 HUFF/CDIC.
    pub compression: u16,
    /// Set when the file is not a readable PalmDB at all.
    pub unreadable: bool,
    /// Why the HUFF/CDIC tables could not be read, on a huffdic file whose
    /// compression model is broken. The lookup index is never compressed, so
    /// this never explains a miss - it is here so a report on a huffdic file
    /// can say the compression was understood, or say plainly that it was not.
    pub huffdic_error: Option<String>,
}

impl LookupReport {
    /// Whether the orth index sits somewhere other than where the MOBI header
    /// says it does. True for a file whose index pointer was not adjusted for
    /// records inserted ahead of it.
    pub fn index_pointer_is_stale(&self) -> bool {
        match (self.index_record, self.declared_index_record) {
            (Some(used), Some(declared)) => used as u32 != declared,
            _ => false,
        }
    }

    /// Whether the text records use HUFF/CDIC compression.
    pub fn is_huffdic(&self) -> bool {
        self.compression == COMPRESSION_HUFFDIC
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Collation {
    /// Accent+case folding (Greek SPL blob, or Latin exact/fold ORDT).
    Fold,
    /// Literal per-character match (CJK / Arabic generated ORDT).
    Literal,
    /// Case-folded query against as-stored labels (Cyrillic and other plain
    /// UTF-16BE dictionaries).
    Plain,
}

struct OrthIndex {
    entries: Vec<(String, u32)>, // (decoded label, text position)
    collation: Collation,
    /// PalmDB record number the primary INDX was read from.
    record: usize,
}

fn u16_be(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn u32_be(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

/// PalmDB record offset table: returns the byte ranges of every record.
fn palmdb_records(data: &[u8]) -> Option<Vec<(usize, usize)>> {
    let count = u16_be(data, 76)? as usize;
    let mut starts = Vec::with_capacity(count);
    for i in 0..count {
        starts.push(u32_be(data, 78 + i * 8)? as usize);
    }
    let mut ranges = Vec::with_capacity(count);
    for i in 0..count {
        let start = starts[i];
        let end = if i + 1 < count {
            starts[i + 1]
        } else {
            data.len()
        };
        if start > data.len() || end > data.len() || start > end {
            return None;
        }
        ranges.push((start, end));
    }
    Some(ranges)
}

/// Decode a UTF-16BE label to a String (lossy on unpaired surrogates).
fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decode an ORDT symbol-sequence label through the embedded ORDT2 table.
/// A table symbol maps to `ordt2[sym]`; an out-of-table value (>= oentries)
/// is a literal code point.
fn decode_ordt_label(bytes: &[u8], ordt2: &[u16], oentries: u32, two_byte: bool) -> String {
    let elems: Vec<u32> = if two_byte {
        bytes
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]) as u32)
            .collect()
    } else {
        bytes.iter().map(|&b| b as u32).collect()
    };
    let mut out = String::with_capacity(elems.len());
    for e in elems {
        let cp = if e < oentries {
            *ordt2.get(e as usize).unwrap_or(&0) as u32
        } else {
            e
        };
        if let Some(c) = char::from_u32(cp) {
            out.push(c);
        }
    }
    out
}

/// Read one inverted VWI (high bit set marks the last byte) starting at `pos`.
fn read_vwi_inv(entry: &[u8], mut pos: usize) -> Option<u32> {
    let mut value: u32 = 0;
    for _ in 0..5 {
        let b = *entry.get(pos)?;
        value = (value << 7) | (b & 0x7F) as u32;
        pos += 1;
        if b & 0x80 != 0 {
            return Some(value);
        }
    }
    None
}

/// Index encoding declared by an orth primary INDX header (0xFDEA). Weaker
/// than [`orth_index_name`] as a signature - kindling stamps it on all three
/// of the primaries it writes for one dictionary - so it is only the fallback.
const ORTH_INDEX_ENCODING: u32 = 65002;

/// The index name in an orth primary INDX header, if the record is one.
///
/// Every INDX header is 192 bytes and TAGX starts right after it, except the
/// orth primary, which carries its index name (`default`, from
/// `<idx:entry name="default">`) in between and declares a header length past
/// 192 to cover it. Measured across every committed kindlegen and kindling
/// dictionary, that is exactly one record per file, and nothing at all in a
/// book or a comic - which matters, because a kindling comic's orth index
/// field points at a fragment INDX that parses into a handful of mojibake
/// entries if it is taken at face value.
fn orth_index_name(rec: &[u8]) -> Option<&[u8]> {
    if rec.get(0..4) != Some(b"INDX".as_slice()) {
        return None;
    }
    let header_len = u32_be(rec, 4)? as usize;
    // index type at 8, generation at 12: an orth primary is type 0 and the
    // primary rather than one of its data records.
    if u32_be(rec, 8)? != 0 || u32_be(rec, 12)? != 0 {
        return None;
    }
    if header_len <= 192 || header_len > rec.len() {
        return None;
    }
    let name = &rec[192..header_len];
    name.iter().all(|b| b.is_ascii_graphic()).then_some(name)
}

/// Whether record `idx` at least declares the orth index encoding. The loose
/// test, for a dictionary whose primary header carries no index name.
fn declares_orth_encoding(data: &[u8], recs: &[(usize, usize)], idx: usize) -> bool {
    let (s, e) = match recs.get(idx) {
        Some(r) => *r,
        None => return false,
    };
    let rec = &data[s..e];
    rec.get(0..4) == Some(b"INDX".as_slice())
        && u32_be(rec, 12) == Some(0)
        && u32_be(rec, 28) == Some(ORTH_INDEX_ENCODING)
}

/// Locate the orth primary INDX.
///
/// Prefers the record the MOBI header names, but only once it has been
/// confirmed to be one. Otherwise takes the record that looks like one:
/// preferring the `default` index the firmware searches, then the index with
/// the most entries, since a dictionary can carry several.
fn find_orth_primary(data: &[u8], recs: &[(usize, usize)], declared: Option<u32>) -> Option<usize> {
    let named: Vec<usize> = (0..recs.len())
        .filter(|&i| {
            let (s, e) = recs[i];
            orth_index_name(&data[s..e]).is_some()
        })
        .collect();
    let pick = |candidates: &[usize]| -> Option<usize> {
        if let Some(d) = declared {
            if candidates.contains(&(d as usize)) {
                return Some(d as usize);
            }
        }
        candidates.iter().copied().max_by_key(|&i| {
            let (s, e) = recs[i];
            let rec = &data[s..e];
            let is_default = orth_index_name(rec) == Some(b"default".as_slice());
            // Total entry count at header offset 36. `Reverse(i)` breaks a tie
            // toward the earlier record, so the choice does not depend on
            // iteration order.
            (
                is_default,
                u32_be(rec, 36).unwrap_or(0),
                std::cmp::Reverse(i),
            )
        })
    };
    if !named.is_empty() {
        return pick(&named);
    }
    let encoded: Vec<usize> = (0..recs.len())
        .filter(|&i| declares_orth_encoding(data, recs, i))
        .collect();
    pick(&encoded)
}

/// PalmDB record number of the dictionary's orth primary INDX, found the way
/// [`lookup`] finds it: the record the MOBI header names when that record
/// really is one, and otherwise the record that looks like one.
///
/// Returns `None` for a file with no dictionary index at all.
pub fn orth_index_record(mobi: &[u8]) -> Option<usize> {
    let recs = palmdb_records(mobi)?;
    let (r0s, r0e) = *recs.first()?;
    let declared = u32_be(&mobi[r0s..r0e], 40).filter(|&v| v != u32::MAX);
    find_orth_primary(mobi, &recs, declared)
}

/// Parse the orth index of a dictionary MOBI: decode every label and its text
/// position, and determine the collation the firmware would apply.
fn parse_orth_index(data: &[u8]) -> Option<OrthIndex> {
    let recs = palmdb_records(data)?;
    let (r0s, r0e) = *recs.first()?;
    let rec0 = &data[r0s..r0e];
    let declared = u32_be(rec0, 40).filter(|&v| v != u32::MAX);
    let orth_idx = find_orth_primary(data, &recs, declared)?;
    let (ps, pe) = recs[orth_idx];
    let primary = &data[ps..pe];

    let num_data = u32_be(primary, 24)? as usize;
    let spl_count = u32_be(primary, 56).unwrap_or(0);
    let oentries = u32_be(primary, 168).unwrap_or(0);
    let ordt_type = u32_be(primary, 164).unwrap_or(0); // 0 = two-byte, 1 = one-byte
    let ordt2_off = u32_be(primary, 176).unwrap_or(0) as usize;

    // ORDT2 is meaningful only for the generated/exact path (spl_count 0). The
    // Greek fold blob also sets oentries (a 7-symbol seed table) but keeps
    // UTF-16BE labels, so it must not be read as ORDT-encoded.
    let ordt_labels = spl_count == 0 && oentries > 0 && ordt2_off > 0;
    let two_byte = ordt_type == 0;
    // The ORDT2 table is written as its 4-byte "ORDT" magic followed by
    // `oentries` big-endian u16 values (see OrdtTables::serialize); the header
    // offset points at the magic, so skip it before reading symbols.
    let ordt2: Vec<u16> = if ordt_labels {
        let base = if primary.get(ordt2_off..ordt2_off + 4) == Some(b"ORDT".as_slice()) {
            ordt2_off + 4
        } else {
            ordt2_off
        };
        (0..oentries as usize)
            .filter_map(|i| u16_be(primary, base + i * 2))
            .collect()
    } else {
        Vec::new()
    };

    let mut entries: Vec<(String, u32)> = Vec::new();
    for di in 0..num_data {
        let ri = orth_idx + 1 + di;
        if ri >= recs.len() {
            break;
        }
        let (rs, re) = recs[ri];
        let rec = &data[rs..re];
        if rec.get(0..4) != Some(b"INDX".as_slice()) {
            continue;
        }
        // A truncated leaf costs its own entries and no more. These used to
        // be `?`, which threw away every label in the dictionary because one
        // record ran short.
        let (idxt_off, count) = match (u32_be(rec, 20), u32_be(rec, 24)) {
            (Some(o), Some(c)) => (o as usize, c as usize),
            _ => continue,
        };
        if rec.get(idxt_off..idxt_off + 4) != Some(b"IDXT".as_slice()) {
            continue;
        }
        let mut offs: Vec<usize> = Vec::with_capacity(count + 1);
        for i in 0..count {
            match u16_be(rec, idxt_off + 4 + i * 2) {
                Some(o) => offs.push(o as usize),
                None => break,
            }
        }
        let count = offs.len();
        offs.push(idxt_off);
        for i in 0..count {
            let (a, b) = (offs[i], offs[i + 1]);
            if b <= a || b > rec.len() {
                continue;
            }
            let entry = &rec[a..b];
            let label_len = entry[0] as usize;
            if 1 + label_len >= entry.len() {
                continue;
            }
            let label_bytes = &entry[1..1 + label_len];
            let control_pos = 1 + label_len;
            // First tag value after the control byte is the text position.
            let position = read_vwi_inv(entry, control_pos + 1).unwrap_or(0);
            let label = if ordt_labels {
                decode_ordt_label(label_bytes, &ordt2, oentries, two_byte)
            } else {
                decode_utf16be(label_bytes)
            };
            entries.push((label, position));
        }
    }

    let collation = if spl_count > 0 {
        Collation::Fold
    } else if ordt_labels {
        // Latin-script labels fold (exact-accent default sorts folded); other
        // scripts on the generated ORDT (CJK/Arabic) match by literal.
        if entries.iter().any(|(l, _)| is_latin_label(l)) {
            Collation::Fold
        } else {
            Collation::Literal
        }
    } else {
        Collation::Plain
    };

    Some(OrthIndex {
        entries,
        collation,
        record: orth_idx,
    })
}

fn is_latin_label(label: &str) -> bool {
    label.chars().any(|c| {
        let cp = c as u32;
        matches!(cp, 0x41..=0x5A | 0x61..=0x7A | 0x00C0..=0x024F | 0x1E00..=0x1EFF)
    })
}

fn strip_stress(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '\u{0300}' && c != '\u{0301}')
        .collect()
}

fn fold_key(s: &str) -> String {
    folded_sort_key(s).into_iter().collect()
}

/// Resolve `query` against the dictionary in `mobi`, returning the matched
/// label and its text position, or `None` if the firmware would find nothing.
pub fn lookup(mobi: &[u8], query: &str) -> Option<LookupResult> {
    report(mobi, query).result
}

/// Resolve `query` and report what the file looked like while doing it.
///
/// A miss is not one thing: the file may hold no dictionary index, or hold one
/// that simply has no matching headword. The CLI needs to say which, so this
/// returns both the outcome and the shape of the file behind it.
pub fn report(mobi: &[u8], query: &str) -> LookupReport {
    let mut out = LookupReport {
        result: None,
        index_record: None,
        declared_index_record: None,
        entries: 0,
        compression: 0,
        unreadable: false,
        huffdic_error: None,
    };

    let recs = match palmdb_records(mobi) {
        Some(r) if !r.is_empty() => r,
        _ => {
            out.unreadable = true;
            return out;
        }
    };
    let (r0s, r0e) = recs[0];
    let rec0 = &mobi[r0s..r0e];
    out.compression = u16_be(rec0, 0).unwrap_or(0);
    out.declared_index_record = u32_be(rec0, 40).filter(|&v| v != u32::MAX);

    if out.compression == COMPRESSION_HUFFDIC {
        let records: Vec<&[u8]> = recs.iter().map(|&(s, e)| &mobi[s..e]).collect();
        if let Err(e) = crate::huffcdic::Huffdic::load(&records, 0) {
            out.huffdic_error = Some(e.to_string());
        }
    }

    let index = match parse_orth_index(mobi) {
        Some(i) => i,
        None => return out,
    };
    out.index_record = Some(index.record);
    out.entries = index.entries.len();
    out.result = resolve(&index, query);
    out
}

/// Match `query` against the decoded labels using the index's collation.
fn resolve(index: &OrthIndex, query: &str) -> Option<LookupResult> {
    match index.collation {
        Collation::Fold => {
            let qk = fold_key(query);
            // Prefer an exact-label match, then any fold-equal label.
            index
                .entries
                .iter()
                .find(|(l, _)| l == query)
                .or_else(|| index.entries.iter().find(|(l, _)| fold_key(l) == qk))
                .map(|(l, p)| LookupResult {
                    matched_label: l.clone(),
                    position: *p,
                })
        }
        Collation::Literal => index
            .entries
            .iter()
            .find(|(l, _)| l == query)
            .map(|(l, p)| LookupResult {
                matched_label: l.clone(),
                position: *p,
            }),
        Collation::Plain => {
            // Firmware folds case on the query side only; labels match as
            // stored. Try the query, its lowercase, and their stress-stripped
            // forms, all against the labels verbatim (issues #8, #17).
            let mut candidates = vec![
                query.to_string(),
                query.to_lowercase(),
                strip_stress(query),
                strip_stress(query).to_lowercase(),
            ];
            candidates.dedup();
            for cand in candidates {
                if let Some((l, p)) = index.entries.iter().find(|(l, _)| *l == cand) {
                    return Some(LookupResult {
                        matched_label: l.clone(),
                        position: *p,
                    });
                }
            }
            None
        }
    }
}
