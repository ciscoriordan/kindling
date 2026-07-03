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

use crate::ordt::folded_sort_key;

/// A resolved lookup: the stored label that matched and the text position its
/// entry points at (the start of the headword's record text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupResult {
    pub matched_label: String,
    pub position: u32,
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

/// Parse the orth index of a dictionary MOBI: decode every label and its text
/// position, and determine the collation the firmware would apply.
fn parse_orth_index(data: &[u8]) -> Option<OrthIndex> {
    let recs = palmdb_records(data)?;
    let (r0s, r0e) = *recs.first()?;
    let rec0 = &data[r0s..r0e];
    let orth_idx = u32_be(rec0, 40)? as usize;
    if orth_idx >= recs.len() {
        return None;
    }
    let (ps, pe) = recs[orth_idx];
    let primary = &data[ps..pe];
    if primary.get(0..4)? != b"INDX".as_slice() {
        return None;
    }

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
        let idxt_off = u32_be(rec, 20)? as usize;
        let count = u32_be(rec, 24)? as usize;
        if rec.get(idxt_off..idxt_off + 4) != Some(b"IDXT".as_slice()) {
            continue;
        }
        let mut offs: Vec<usize> = Vec::with_capacity(count + 1);
        for i in 0..count {
            offs.push(u16_be(rec, idxt_off + 4 + i * 2)? as usize);
        }
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

    Some(OrthIndex { entries, collation })
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
    let index = parse_orth_index(mobi)?;
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
