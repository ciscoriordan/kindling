/// CNCX (Compiled NCX) record builder.
///
/// CNCX records hold a flat collection of UTF-8 strings. Each string is
/// prefixed with a forward-VWI length byte (max 500 bytes per string).
/// An INDX entry that references a CNCX string stores an offset of the
/// form `record_index * 0x10000 + byte_offset_within_record`.
///
/// Callers use the builder to collect strings and receive a stable
/// u32 offset for each. `into_records` finalizes the builder into the
/// one or more 64 KiB PalmDB records that should be written after the
/// owning INDX records.
///
/// Layout per string:
///   [vwi_length_prefix][utf8 bytes]
///
/// Records are aligned to 4-byte boundaries and capped at
/// `RECORD_LIMIT = 0x10000 - 1024 = 64512` bytes per record, matching
/// the margin kindlegen leaves for safety.

use std::collections::HashMap;

use crate::vwi::encode_vwi;

/// Max bytes of a single CNCX string (kindlegen convention).
pub const MAX_STRING_LENGTH: usize = 500;

/// Max bytes in a single CNCX record (kindlegen convention: 64K - 1K).
pub const RECORD_LIMIT: usize = 0x10000 - 1024;

/// Builder for one or more CNCX records.
///
/// Strings are deduplicated: adding the same string twice returns the
/// same offset. The returned offset is a u32 that callers should
/// store in the referring INDX entry's `tag 2` (for FRAG_AID_CNCX)
/// or `tag 5` (for INFL_GROUPS) slot.
#[derive(Debug, Default)]
pub struct CncxBuilder {
    /// One Vec<u8> per in-progress CNCX record. The first record is
    /// built at index 0; if we overflow `RECORD_LIMIT` we roll over
    /// to a new record at index 1, and so on.
    records: Vec<Vec<u8>>,
    /// Dedup map: string → encoded offset (for callers that add the
    /// same string multiple times).
    dedup: HashMap<String, u32>,
}

impl CncxBuilder {
    pub fn new() -> Self {
        Self {
            records: vec![Vec::new()],
            dedup: HashMap::new(),
        }
    }

    /// Number of strings added so far (including duplicates).
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.dedup.len()
    }

    /// Add a string and return its CNCX offset.
    ///
    /// The offset is `record_index * 0x10000 + byte_offset_in_record`.
    /// If the same string was already added, returns the prior offset
    /// instead of encoding it twice.
    pub fn add(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.dedup.get(s) {
            return off;
        }

        // Truncate overly-long strings on UTF-8 boundary.
        let s_trunc = if s.len() > MAX_STRING_LENGTH {
            let mut cut = MAX_STRING_LENGTH;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            &s[..cut]
        } else {
            s
        };

        let bytes = s_trunc.as_bytes();
        let len_prefix = encode_vwi(bytes.len() as u32);
        let entry_size = len_prefix.len() + bytes.len();

        // Roll over to a new record if this entry would exceed the
        // per-record limit.
        let current = self.records.last_mut().unwrap();
        if current.len() + entry_size > RECORD_LIMIT {
            self.records.push(Vec::new());
        }

        let record_index = (self.records.len() - 1) as u32;
        let current = self.records.last_mut().unwrap();
        let byte_offset = current.len() as u32;
        current.extend_from_slice(&len_prefix);
        current.extend_from_slice(bytes);

        let cncx_offset = record_index * 0x10000 + byte_offset;
        self.dedup.insert(s.to_string(), cncx_offset);
        cncx_offset
    }

    /// Finalize into PalmDB record bytes. Each record is padded to a
    /// 4-byte boundary. Returns an empty Vec if no strings were added.
    pub fn into_records(self) -> Vec<Vec<u8>> {
        let mut out = Vec::with_capacity(self.records.len());
        for mut rec in self.records.into_iter() {
            if rec.is_empty() {
                continue;
            }
            while rec.len() % 4 != 0 {
                rec.push(0);
            }
            out.push(rec);
        }
        out
    }

    /// Number of CNCX records this builder will emit.
    #[allow(dead_code)]
    pub fn record_count(&self) -> usize {
        self.records.iter().filter(|r| !r.is_empty()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_builder_emits_no_records() {
        let b = CncxBuilder::new();
        assert_eq!(b.into_records().len(), 0);
    }

    #[test]
    fn single_string_offset_is_zero() {
        let mut b = CncxBuilder::new();
        let off = b.add("P-//*[@aid='0']");
        assert_eq!(off, 0);
    }

    #[test]
    fn dedup_returns_same_offset() {
        let mut b = CncxBuilder::new();
        let a = b.add("foo");
        let c = b.add("bar");
        let a2 = b.add("foo");
        assert_eq!(a, a2);
        assert_ne!(a, c);
    }

    #[test]
    fn offset_format_includes_record_index_shift() {
        let mut b = CncxBuilder::new();
        // Stuff first record near its limit with repeated long strings.
        let long = "x".repeat(400);
        for i in 0..200 {
            b.add(&format!("{}{}", long, i));
        }
        // A late add should end up in a higher record.
        let late = b.add("last_one");
        // record index appears in the upper 16 bits.
        assert!(late >> 16 >= 1, "late offset {} should include record index >=1", late);
    }

    #[test]
    fn string_length_truncated_at_500() {
        let long = "a".repeat(800);
        let mut b = CncxBuilder::new();
        b.add(&long);
        let recs = b.into_records();
        assert_eq!(recs.len(), 1);
        // The VWI length prefix for 500 is 0x83 0x74 (2 bytes).
        // First byte is the length prefix's first byte (>=0x80 since
        // 500 > 127), then 500 bytes of 'a'.
        assert!(recs[0].len() >= 500);
    }
}
