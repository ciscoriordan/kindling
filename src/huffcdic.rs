//! HUFF/CDIC ("huffdic") decompression, PalmDOC compression type 17480.
//!
//! kindling only ever *writes* PalmDOC LZ77 (compression 2), but it reads
//! files other tools produced: `kindlegen -c2` compresses text with a static
//! Huffman code over a shared phrase dictionary instead, and every
//! Amazon-published dictionary in the store is built that way. Without this
//! module `dump`, `check` and the test harness silently treated those text
//! records as opaque bytes (issue #49).
//!
//! Two record kinds carry the model, both sitting in the PalmDB right after
//! the text records and located through the MOBI header (`huff_rec_index` at
//! MOBI offset 0x60, `huff_rec_count` at 0x64; both are record numbers
//! relative to the section's record 0, which matters for the KF8 half of a
//! dual-format file):
//!
//!   - one `HUFF` record holding the code tables: a 256-entry table indexed by
//!     the top 8 bits of the bit window, plus per-code-length `mincode` and
//!     `maxcode` bounds for the codes too long to resolve from those 8 bits;
//!   - `huff_rec_count - 1` `CDIC` records holding the phrase dictionary the
//!     codes index into. A phrase is normally stored literally, but may itself
//!     be stored as a compressed bitstream (the 0x8000 flag clear), in which
//!     case it expands through the same decoder and is memoized.
//!
//! Codes are read most-significant-bit first out of a 32-bit window carried
//! inside a 64-bit accumulator. A code's dictionary index is `maxcode - code`
//! for its length, so symbols run *backwards* through each length's code
//! range; the tables are stored pre-shifted (`(maxcode + 1) << (32 - codelen)`)
//! so the decoder never has to normalize. Those shifted bounds do not fit in
//! 32 bits at short code lengths, so everything here is computed in `u64`;
//! the Python reference implementations get away with `int`.
//!
//! Cross-checked against KindleUnpack's `mobi_uncompress.HuffcdicReader`,
//! libmobi's `compression.c`, and calibre's `huffcdic.py`, which agree.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

/// PalmDOC header compression value for HUFF/CDIC. Spelled `0x4448` in the
/// wild, which is why some tools print it as "DH".
pub const COMPRESSION_HUFFDIC: u16 = 17480;

/// Longest phrase expansion chain we will follow. Real files nest one level
/// deep at most; the cap exists so a corrupt dictionary cannot recurse away.
const MAX_PHRASE_DEPTH: u32 = 8;

/// Ceiling on the bytes one call may produce. A MOBI text record decompresses
/// to `text_record_size` (4096, or 8192/16384 for very large books), so this
/// is orders of magnitude of headroom; it only stops a decompression bomb.
const MAX_OUTPUT: usize = 16 * 1024 * 1024;

/// Why a huffdic record could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HuffdicError {
    /// The MOBI header says the file is huffdic but names fewer than two
    /// records (a `HUFF` needs at least one `CDIC` behind it), or the records
    /// it names do not exist.
    MissingRecords {
        index: u32,
        count: u32,
        available: usize,
    },
    /// A record that should have been `HUFF` or `CDIC` was not.
    BadMagic {
        expected: &'static str,
        record: usize,
        found: String,
    },
    /// A header offset or length ran past the end of its record.
    Truncated {
        record: &'static str,
        detail: String,
    },
    /// The code tables are self-inconsistent (a zero code length, a non-terminal
    /// entry at 8 bits or fewer, or a code that resolves past 32 bits).
    BadTables(String),
    /// A code resolved to a phrase index the dictionary does not have.
    BadPhraseIndex { index: usize, len: usize },
    /// A compressed phrase expands to itself, directly or through a chain.
    PhraseCycle(usize),
    /// Output exceeded [`MAX_OUTPUT`].
    TooLarge,
}

impl fmt::Display for HuffdicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HuffdicError::MissingRecords {
                index,
                count,
                available,
            } => write!(
                f,
                "huffdic tables missing: header points at record {index} for {count} record(s), \
                 file has {available}"
            ),
            HuffdicError::BadMagic {
                expected,
                record,
                found,
            } => write!(
                f,
                "record {record} should start with {expected:?} but starts with {found:?}"
            ),
            HuffdicError::Truncated { record, detail } => {
                write!(f, "{record} record truncated: {detail}")
            }
            HuffdicError::BadTables(why) => write!(f, "huffdic code tables are invalid: {why}"),
            HuffdicError::BadPhraseIndex { index, len } => write!(
                f,
                "code resolved to phrase {index} but the dictionary holds {len}"
            ),
            HuffdicError::PhraseCycle(i) => {
                write!(f, "compressed phrase {i} expands to itself")
            }
            HuffdicError::TooLarge => {
                write!(f, "decompressed output exceeded {} bytes", MAX_OUTPUT)
            }
        }
    }
}

impl std::error::Error for HuffdicError {}

/// One entry of the 256-entry `HUFF` lookup table, keyed by the top 8 bits of
/// the code window.
#[derive(Clone, Copy)]
struct Dict1 {
    /// For a terminal entry, the code's length. For a non-terminal one, a
    /// lower bound to start the code-length walk from.
    codelen: u8,
    /// Set when those 8 bits already identify the code, so no walk is needed.
    term: bool,
    /// Pre-shifted upper bound, only meaningful when `term` is set.
    maxcode: u64,
}

/// A phrase dictionary slot. Compressed slots expand on first use and are
/// replaced by their expansion.
enum Phrase {
    Plain(Rc<[u8]>),
    Packed(Rc<[u8]>),
    /// Currently expanding: seeing this again means the file references itself.
    Expanding,
}

/// A loaded huffdic model: the code tables plus the phrase dictionary.
///
/// Decompression takes `&self` (phrase memoization happens behind a
/// `RefCell`), so one loaded model decompresses every text record of a file.
pub struct Huffdic {
    dict1: Vec<Dict1>,
    /// Pre-shifted lower bound per code length, index 0..=32. Non-increasing,
    /// which is what makes the "walk the length up" loop terminate on the
    /// right length.
    mincode: [u64; 33],
    /// Pre-shifted upper bound per code length, index 0..=32.
    maxcode: [u64; 33],
    phrases: RefCell<Vec<Phrase>>,
    /// Number of phrases the `CDIC` headers declared, kept for reporting.
    declared_phrases: u32,
}

/// Size of the trailing data regions at the end of a text record.
///
/// `extra_record_flags` (the u32 at record 0 offset 240) names them: bits
/// 15..1 each add a region whose own total size is a varint at its end, and
/// bit 0 adds the multibyte overlap byte, which must come off last. The
/// regions sit outside the compressed stream whatever the compression is, so
/// they come off before decompression rather than after.
pub fn trailing_data_len(record: &[u8], extra_record_flags: u32) -> usize {
    let mut end = record.len();
    for bit in (1..16).rev() {
        if extra_record_flags & (1 << bit) == 0 || end == 0 {
            continue;
        }
        // Most significant byte first, and it is the first byte that carries
        // the high-bit marker: `81 20` is 160, not 4097.
        let window = &record[end.saturating_sub(4)..end];
        let mut size = 0usize;
        for &b in window {
            if b & 0x80 != 0 {
                size = 0;
            }
            size = (size << 7) | (b & 0x7F) as usize;
        }
        end -= size.min(end);
    }
    if extra_record_flags & 1 != 0 && end > 0 {
        end -= ((record[end - 1] & 3) as usize + 1).min(end);
    }
    record.len() - end
}

/// The compressed body of a text record: everything before its trailing data
/// regions. What [`Huffdic::decompress`] expects to be handed.
pub fn text_body(record: &[u8], extra_record_flags: u32) -> &[u8] {
    let trailing = trailing_data_len(record, extra_record_flags);
    &record[..record.len() - trailing]
}

/// Summarizes rather than dumps: the code tables are 289 numbers nobody reads.
impl fmt::Debug for Huffdic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Huffdic")
            .field("phrases", &self.phrases.borrow().len())
            .field("declared_phrases", &self.declared_phrases)
            .finish_non_exhaustive()
    }
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

fn magic_of(d: &[u8]) -> String {
    String::from_utf8_lossy(&d[..d.len().min(4)]).into_owned()
}

impl Huffdic {
    /// Load the model for a MOBI section.
    ///
    /// `records` is the whole PalmDB record list and `section_start` the record
    /// number of the section's record 0 (0 for a MOBI6 file, the EXTH 121
    /// boundary for the KF8 half of a dual-format one). `huff_rec_index` is
    /// stored relative to that, matching KindleUnpack's `huffoff + self.start`.
    ///
    /// Returns `Ok(None)` when the header declares no huffdic records at all,
    /// which is the normal state of a PalmDOC file.
    pub fn load(records: &[&[u8]], section_start: usize) -> Result<Option<Self>, HuffdicError> {
        let record0 = match records.get(section_start) {
            Some(r) => *r,
            None => return Ok(None),
        };
        // record0 offsets 112/116 = MOBI header 0x60/0x64.
        let index = u32_be(record0, 112).unwrap_or(0);
        let count = u32_be(record0, 116).unwrap_or(0);
        if index == 0 || index == u32::MAX || count == 0 {
            return Ok(None);
        }
        Self::from_records(records, section_start, index, count).map(Some)
    }

    /// Load from explicit record numbers, for callers that already read
    /// `huff_rec_index` / `huff_rec_count` themselves.
    pub fn from_records(
        records: &[&[u8]],
        section_start: usize,
        huff_rec_index: u32,
        huff_rec_count: u32,
    ) -> Result<Self, HuffdicError> {
        let first = section_start.saturating_add(huff_rec_index as usize);
        let last = first.saturating_add(huff_rec_count as usize);
        // The count covers the HUFF record plus its CDICs, so anything under
        // two means the phrase dictionary is not there at all. Caught here
        // rather than as a phrase-index error on the first code.
        if huff_rec_count < 2 || last > records.len() {
            return Err(HuffdicError::MissingRecords {
                index: huff_rec_index,
                count: huff_rec_count,
                available: records.len(),
            });
        }
        let mut model = Self::from_huff(records[first], first)?;
        for (i, cdic) in records[first + 1..last].iter().enumerate() {
            model.push_cdic(cdic, first + 1 + i)?;
        }
        Ok(model)
    }

    /// Parse a `HUFF` record into the code tables.
    fn from_huff(huff: &[u8], record: usize) -> Result<Self, HuffdicError> {
        if huff.len() < 24 || &huff[..4] != b"HUFF" {
            return Err(HuffdicError::BadMagic {
                expected: "HUFF",
                record,
                found: magic_of(huff),
            });
        }
        let off1 = u32_be(huff, 8).unwrap_or(0) as usize;
        let off2 = u32_be(huff, 12).unwrap_or(0) as usize;
        if off1.saturating_add(256 * 4) > huff.len() {
            return Err(HuffdicError::Truncated {
                record: "HUFF",
                detail: format!(
                    "256-entry table at offset {off1} needs {} bytes, record is {}",
                    256 * 4,
                    huff.len()
                ),
            });
        }
        if off2.saturating_add(64 * 4) > huff.len() {
            return Err(HuffdicError::Truncated {
                record: "HUFF",
                detail: format!(
                    "code-length table at offset {off2} needs 256 bytes, record is {}",
                    huff.len()
                ),
            });
        }

        let mut dict1 = Vec::with_capacity(256);
        for i in 0..256 {
            let v = u32_be(huff, off1 + i * 4).unwrap_or(0);
            let codelen = (v & 0x1F) as u8;
            let term = v & 0x80 != 0;
            if codelen == 0 {
                return Err(HuffdicError::BadTables(format!(
                    "entry {i} has code length 0"
                )));
            }
            if codelen <= 8 && !term {
                // A code of 8 bits or fewer is fully determined by the 8 bits
                // used to index this table, so it must be marked terminal.
                // Every reference implementation asserts this.
                return Err(HuffdicError::BadTables(format!(
                    "entry {i} has code length {codelen} but is not marked terminal"
                )));
            }
            // Stored pre-shift; widen before shifting because the result
            // overruns 32 bits for short codes.
            let maxcode = (((v >> 8) as u64 + 1) << (32 - codelen as u32)) - 1;
            dict1.push(Dict1 {
                codelen,
                term,
                maxcode,
            });
        }

        // 32 (mincode, maxcode) pairs for code lengths 1..=32, with a dummy
        // entry prepended so the arrays index by code length directly.
        let mut mincode = [0u64; 33];
        let mut maxcode = [0u64; 33];
        mincode[0] = 0;
        maxcode[0] = (1u64 << 32) - 1;
        for len in 1..=32usize {
            let lo = u32_be(huff, off2 + (len - 1) * 8).unwrap_or(0) as u64;
            let hi = u32_be(huff, off2 + (len - 1) * 8 + 4).unwrap_or(0) as u64;
            let shift = 32 - len as u32;
            mincode[len] = lo << shift;
            maxcode[len] = ((hi + 1) << shift) - 1;
        }

        Ok(Huffdic {
            dict1,
            mincode,
            maxcode,
            phrases: RefCell::new(Vec::new()),
            declared_phrases: 0,
        })
    }

    /// Append the phrases held by one `CDIC` record.
    fn push_cdic(&mut self, cdic: &[u8], record: usize) -> Result<(), HuffdicError> {
        if cdic.len() < 16 || &cdic[..4] != b"CDIC" {
            return Err(HuffdicError::BadMagic {
                expected: "CDIC",
                record,
                found: magic_of(cdic),
            });
        }
        let declared = u32_be(cdic, 8).unwrap_or(0);
        let bits = u32_be(cdic, 12).unwrap_or(0);
        if bits > 16 {
            return Err(HuffdicError::BadTables(format!(
                "CDIC record {record} declares {bits} index bits"
            )));
        }
        self.declared_phrases = declared;

        let phrases = self.phrases.get_mut();
        // Each record holds up to 1 << bits phrases, and the last one is short.
        let already = phrases.len();
        let remaining = (declared as usize).saturating_sub(already);
        let n = remaining.min(1usize << bits);
        // The offset table starts at 0x10 and its entries are relative to it.
        if 16 + n * 2 > cdic.len() {
            return Err(HuffdicError::Truncated {
                record: "CDIC",
                detail: format!(
                    "offset table for {n} phrases needs {} bytes, record is {}",
                    16 + n * 2,
                    cdic.len()
                ),
            });
        }
        for i in 0..n {
            let off = u16_be(cdic, 16 + i * 2).unwrap_or(0) as usize;
            let len_at = 16 + off;
            let blen = u16_be(cdic, len_at).ok_or_else(|| HuffdicError::Truncated {
                record: "CDIC",
                detail: format!("phrase {i} length at offset {len_at} is past the record end"),
            })? as usize;
            let start = len_at + 2;
            let end = start + (blen & 0x7FFF);
            if end > cdic.len() {
                return Err(HuffdicError::Truncated {
                    record: "CDIC",
                    detail: format!("phrase {i} runs to offset {end}, record is {}", cdic.len()),
                });
            }
            let bytes: Rc<[u8]> = Rc::from(&cdic[start..end]);
            phrases.push(if blen & 0x8000 != 0 {
                Phrase::Plain(bytes)
            } else {
                Phrase::Packed(bytes)
            });
        }
        Ok(())
    }

    /// Number of phrases actually loaded.
    pub fn phrase_count(&self) -> usize {
        self.phrases.borrow().len()
    }

    /// Number of phrases the `CDIC` headers said the dictionary has. Differs
    /// from [`Huffdic::phrase_count`] only when a `CDIC` record is missing.
    pub fn declared_phrase_count(&self) -> u32 {
        self.declared_phrases
    }

    /// Decompress one text record.
    ///
    /// The record must already have had its trailing data entries stripped
    /// (multibyte overlap byte and TBS region, per `extra_record_flags`), the
    /// same as for PalmDOC: the trailing regions sit outside the compressed
    /// stream and their bits would otherwise be decoded as symbols.
    pub fn decompress(&self, record: &[u8]) -> Result<Vec<u8>, HuffdicError> {
        let mut out = Vec::with_capacity(record.len() * 4);
        self.decode_into(record, &mut out, 0)?;
        Ok(out)
    }

    fn decode_into(&self, data: &[u8], out: &mut Vec<u8>, depth: u32) -> Result<(), HuffdicError> {
        // The reader always has a full 64-bit window in hand, so pad past the
        // end; the padding bits are never emitted because `bitsleft` runs out
        // first.
        let mut buf = Vec::with_capacity(data.len() + 12);
        buf.extend_from_slice(data);
        buf.resize(data.len() + 12, 0);

        let mut bitsleft = data.len() as i64 * 8;
        let mut pos = 0usize;
        let mut window = u64::from_be_bytes(buf[0..8].try_into().unwrap());
        // Bits of `window` still unread, counted down from the top 32.
        let mut n: i32 = 32;

        loop {
            if n <= 0 {
                pos += 4;
                if pos + 8 > buf.len() {
                    buf.resize(pos + 8, 0);
                }
                window = u64::from_be_bytes(buf[pos..pos + 8].try_into().unwrap());
                n += 32;
            }
            let code = (window >> n) & 0xFFFF_FFFF;

            let entry = self.dict1[(code >> 24) as usize];
            let mut codelen = entry.codelen as usize;
            let mut maxcode = entry.maxcode;
            if !entry.term {
                while codelen <= 32 && code < self.mincode[codelen] {
                    codelen += 1;
                }
                if codelen > 32 {
                    return Err(HuffdicError::BadTables(format!(
                        "code {code:#010x} is longer than 32 bits"
                    )));
                }
                maxcode = self.maxcode[codelen];
            }

            n -= codelen as i32;
            bitsleft -= codelen as i64;
            if bitsleft < 0 {
                break;
            }

            let span = maxcode.checked_sub(code).ok_or_else(|| {
                HuffdicError::BadTables(format!(
                    "code {code:#010x} is above the maximum for length {codelen}"
                ))
            })?;
            let index = (span >> (32 - codelen as u32)) as usize;
            self.append_phrase(index, out, depth)?;
            if out.len() > MAX_OUTPUT {
                return Err(HuffdicError::TooLarge);
            }
        }
        Ok(())
    }

    /// Append phrase `index`, expanding and memoizing it if it is stored
    /// compressed.
    fn append_phrase(
        &self,
        index: usize,
        out: &mut Vec<u8>,
        depth: u32,
    ) -> Result<(), HuffdicError> {
        // Take the slot out before recursing so no borrow is held across the
        // recursive call, and so a self-reference is visible as `Expanding`.
        let packed = {
            let mut phrases = self.phrases.borrow_mut();
            let len = phrases.len();
            match phrases.get_mut(index) {
                None => return Err(HuffdicError::BadPhraseIndex { index, len }),
                Some(Phrase::Expanding) => return Err(HuffdicError::PhraseCycle(index)),
                Some(slot @ Phrase::Plain(_)) => {
                    if let Phrase::Plain(bytes) = slot {
                        out.extend_from_slice(bytes);
                        return Ok(());
                    }
                    unreachable!()
                }
                Some(slot) => match std::mem::replace(slot, Phrase::Expanding) {
                    Phrase::Packed(bytes) => bytes,
                    _ => unreachable!(),
                },
            }
        };

        if depth >= MAX_PHRASE_DEPTH {
            self.phrases.borrow_mut()[index] = Phrase::Packed(packed);
            return Err(HuffdicError::BadTables(format!(
                "phrase {index} nests more than {MAX_PHRASE_DEPTH} levels deep"
            )));
        }

        let mut expanded = Vec::new();
        match self.decode_into(&packed, &mut expanded, depth + 1) {
            Ok(()) => {
                let bytes: Rc<[u8]> = Rc::from(expanded.as_slice());
                out.extend_from_slice(&bytes);
                self.phrases.borrow_mut()[index] = Phrase::Plain(bytes);
                Ok(())
            }
            Err(e) => {
                // Put the slot back so a later record gets the same error
                // rather than a spurious cycle report.
                self.phrases.borrow_mut()[index] = Phrase::Packed(packed);
                Err(e)
            }
        }
    }
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
        p.push("tests/fixtures/huffdic");
        p.push(name);
        p
    }

    /// Split a file into its PalmDB records.
    fn records(data: &[u8]) -> Vec<&[u8]> {
        let n = u16::from_be_bytes([data[76], data[77]]) as usize;
        let offsets: Vec<usize> = (0..n)
            .map(|i| u32_be(data, 78 + i * 8).unwrap() as usize)
            .collect();
        (0..n)
            .map(|i| {
                let end = if i + 1 < n {
                    offsets[i + 1]
                } else {
                    data.len()
                };
                &data[offsets[i]..end]
            })
            .collect()
    }

    #[test]
    fn load_returns_none_for_a_palmdoc_file() {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.push("tests/fixtures/langs/en/en-kindlegen.mobi");
        let data = std::fs::read(&p).expect("read fixture");
        let recs = records(&data);
        assert!(
            Huffdic::load(&recs, 0)
                .expect("load should not error")
                .is_none(),
            "a PalmDOC file declares no huffdic records"
        );
        println!("  \u{2713} a PalmDOC file loads no huffdic model");
    }

    #[test]
    fn loads_the_model_from_a_huffdic_file() {
        let data = std::fs::read(fixture("en_huffdic.mobi")).expect("read fixture");
        let recs = records(&data);
        let model = Huffdic::load(&recs, 0)
            .expect("load should succeed")
            .expect("fixture is huffdic");
        assert_eq!(model.phrase_count(), model.declared_phrase_count() as usize);
        assert!(model.phrase_count() > 256, "byte phrases plus n-grams");
        println!(
            "  \u{2713} loaded {} phrases from the HUFF/CDIC records",
            model.phrase_count()
        );
    }

    #[test]
    fn header_pointing_past_the_end_is_an_error() {
        let empty: [&[u8]; 0] = [];
        let err = Huffdic::from_records(&empty, 0, 5, 2).unwrap_err();
        assert!(matches!(err, HuffdicError::MissingRecords { .. }), "{err}");
        assert!(err.to_string().contains("huffdic tables missing"));
        println!("  \u{2713} a header pointing past the last record reports it");
    }

    #[test]
    fn a_non_huff_record_is_an_error() {
        let recs: [&[u8]; 2] = [b"not a huff record at all........", b"CDIC\x00\x00\x00\x10"];
        let err = Huffdic::from_records(&recs, 0, 0, 2).unwrap_err();
        match err {
            HuffdicError::BadMagic { expected, .. } => assert_eq!(expected, "HUFF"),
            other => panic!("expected BadMagic, got {other}"),
        }
        println!("  \u{2713} a record that is not HUFF is reported as such");
    }

    #[test]
    fn a_truncated_huff_table_is_an_error() {
        // Magic and header length are right, but the table offsets point past
        // the end of a 24-byte record.
        let mut huff = Vec::from(*b"HUFF\x00\x00\x00\x18");
        huff.extend_from_slice(&24u32.to_be_bytes());
        huff.extend_from_slice(&(24u32 + 1024).to_be_bytes());
        huff.resize(24, 0);
        let cdic = tiny_cdic(&[(b"a", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let err = Huffdic::from_records(&recs, 0, 0, 2).unwrap_err();
        assert!(matches!(err, HuffdicError::Truncated { .. }), "{err}");
        println!("  \u{2713} a HUFF record too short for its tables is reported");
    }

    /// The 256-entry table is indexed by the top 8 bits of the code window, so
    /// an entry claiming a code of 8 bits or fewer has already been resolved
    /// and must say so. One that does not would send the decoder into the
    /// code-length walk with no bound to find.
    #[test]
    fn a_short_non_terminal_code_is_rejected() {
        let mut huff = Vec::from(*b"HUFF\x00\x00\x00\x18");
        huff.extend_from_slice(&24u32.to_be_bytes());
        huff.extend_from_slice(&(24u32 + 1024).to_be_bytes());
        huff.resize(24, 0);
        for _ in 0..256 {
            huff.extend_from_slice(&4u32.to_be_bytes()); // codelen 4, term clear
        }
        huff.resize(24 + 1024 + 256, 0);
        let cdic = tiny_cdic(&[(b"a", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let err = Huffdic::from_records(&recs, 0, 0, 2).unwrap_err();
        assert!(matches!(err, HuffdicError::BadTables(_)), "{err}");
        assert!(err.to_string().contains("not marked terminal"));
        println!("  \u{2713} a short code that is not terminal is rejected");
    }

    /// A hand-built model with a single 8-bit code per phrase index.
    ///
    /// Every `dict1` entry is the same terminal entry, so the top 8 bits of
    /// the window *are* the code: byte 0xFF resolves phrase 0, 0xFE phrase 1,
    /// and so on down. That makes the interesting failures reachable in one
    /// byte each, without needing to work out a real dictionary's code
    /// assignment first.
    fn tiny_huff() -> Vec<u8> {
        let mut huff = Vec::from(*b"HUFF\x00\x00\x00\x18");
        huff.extend_from_slice(&24u32.to_be_bytes());
        huff.extend_from_slice(&(24u32 + 1024).to_be_bytes());
        huff.resize(24, 0);
        // codelen 8, terminal, maxcode 255: index = 255 - (window >> 24).
        let entry: u32 = (255 << 8) | 0x80 | 8;
        for _ in 0..256 {
            huff.extend_from_slice(&entry.to_be_bytes());
        }
        // dict2 is never consulted while every entry is terminal.
        huff.resize(24 + 1024 + 256, 0);
        huff
    }

    /// One CDIC holding `phrases`, each `(bytes, already_expanded)`.
    fn tiny_cdic(phrases: &[(&[u8], bool)]) -> Vec<u8> {
        let n = phrases.len();
        let mut offsets = Vec::with_capacity(n);
        let mut body: Vec<u8> = Vec::new();
        for (data, expanded) in phrases {
            offsets.push((2 * n + body.len()) as u16);
            let flagged = data.len() as u16 | if *expanded { 0x8000 } else { 0 };
            body.extend_from_slice(&flagged.to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut rec = Vec::from(*b"CDIC\x00\x00\x00\x10");
        rec.extend_from_slice(&(n as u32).to_be_bytes());
        rec.extend_from_slice(&8u32.to_be_bytes());
        for o in offsets {
            rec.extend_from_slice(&o.to_be_bytes());
        }
        rec.extend_from_slice(&body);
        rec
    }

    #[test]
    fn a_lone_huff_record_is_an_error() {
        let huff = tiny_huff();
        let recs: [&[u8]; 1] = [&huff];
        let err = Huffdic::from_records(&recs, 0, 0, 1).unwrap_err();
        assert!(matches!(err, HuffdicError::MissingRecords { .. }), "{err}");
        println!("  \u{2713} a HUFF record with no CDIC behind it is reported");
    }

    #[test]
    fn a_self_referential_phrase_is_reported_not_recursed() {
        let huff = tiny_huff();
        // Phrase 0 is stored compressed, and its one byte decodes to phrase 0.
        let cdic = tiny_cdic(&[(&[0xFF], false), (b"x", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let model = Huffdic::from_records(&recs, 0, 0, 2).expect("model loads");

        assert_eq!(
            model.decompress(&[0xFE]).unwrap(),
            b"x",
            "phrase 1 is plain"
        );
        let err = model.decompress(&[0xFF]).unwrap_err();
        assert_eq!(err, HuffdicError::PhraseCycle(0));
        // The slot is restored, so a later record gets the same answer rather
        // than a stale in-progress marker.
        assert_eq!(
            model.decompress(&[0xFF]).unwrap_err(),
            HuffdicError::PhraseCycle(0)
        );
        println!("  \u{2713} a phrase that expands to itself is reported twice, not recursed once");
    }

    #[test]
    fn a_phrase_index_past_the_dictionary_is_an_error() {
        let huff = tiny_huff();
        let cdic = tiny_cdic(&[(b"a", true), (b"b", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let model = Huffdic::from_records(&recs, 0, 0, 2).expect("model loads");
        // 0x00 resolves index 255, and the dictionary holds two phrases.
        let err = model.decompress(&[0x00]).unwrap_err();
        assert_eq!(err, HuffdicError::BadPhraseIndex { index: 255, len: 2 });
        println!("  \u{2713} a code resolving past the phrase dictionary is reported");
    }

    #[test]
    fn an_empty_record_decompresses_to_nothing() {
        let huff = tiny_huff();
        let cdic = tiny_cdic(&[(b"a", true), (b"b", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let model = Huffdic::from_records(&recs, 0, 0, 2).expect("model loads");
        assert!(model.decompress(&[]).unwrap().is_empty());
        println!("  \u{2713} an empty text record decodes to nothing");
    }

    /// Whatever a malformed file contains, reading it is an error or a result,
    /// never a panic.
    #[test]
    fn garbage_records_never_panic() {
        let huff = tiny_huff();
        let cdic = tiny_cdic(&[(b"a", true), (b"b", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let model = Huffdic::from_records(&recs, 0, 0, 2).expect("model loads");

        let mut seed = 0x1234_5678u32;
        let mut random = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        };
        let cases: Vec<Vec<u8>> = vec![
            vec![0u8; 64],
            vec![0xFFu8; 64],
            (0..512).map(|_| random()).collect(),
        ];
        for case in &cases {
            let _ = model.decompress(case);
        }

        // And the same for the table records themselves.
        for bad in [vec![0u8; 4096], vec![0xFFu8; 4096]] {
            let recs: [&[u8]; 2] = [&bad, &bad];
            let _ = Huffdic::from_records(&recs, 0, 0, 2);
        }
        println!("  \u{2713} garbage input errors or decodes, never panics");
    }

    #[test]
    fn trailing_data_strips_the_tbs_region_then_the_multibyte_byte() {
        // extra_record_flags = 3: one TBS region plus the multibyte byte.
        // The region's size varint is most significant byte first with the
        // marker on the FIRST byte, so `81 20` is 160 and not 4097.
        let mut rec = vec![b'x'; 200];
        rec[198] = 0x81;
        rec[199] = 0x20; // the region is 160 bytes, so it starts at 40
        rec[39] = 0xAA; // multibyte byte just before it: (0xAA & 3) + 1 = 3
        assert_eq!(trailing_data_len(&rec, 3), 160 + 3);
        // Bit 0 alone strips only the multibyte byte, sized from the last
        // byte of the record: 0x20 & 3 = 0, so one byte.
        assert_eq!(trailing_data_len(&rec, 1), 1);
        // No flags, no trailing data.
        assert_eq!(trailing_data_len(&rec, 0), 0);
        assert_eq!(text_body(&rec, 0).len(), 200);
        println!("  \u{2713} trailing regions come off before the multibyte byte");
    }

    #[test]
    fn a_zero_code_length_is_rejected() {
        let mut huff = Vec::from(*b"HUFF\x00\x00\x00\x18");
        huff.extend_from_slice(&24u32.to_be_bytes());
        huff.extend_from_slice(&(24u32 + 1024).to_be_bytes());
        huff.resize(24, 0);
        huff.resize(24 + 1024 + 256, 0); // every dict1 entry is zero
        let cdic = tiny_cdic(&[(b"a", true)]);
        let recs: [&[u8]; 2] = [&huff, &cdic];
        let err = Huffdic::from_records(&recs, 0, 0, 2).unwrap_err();
        assert!(matches!(err, HuffdicError::BadTables(_)), "{err}");
        assert!(err.to_string().contains("code length 0"));
        println!("  \u{2713} a zero code length is rejected");
    }
}
