//! Generated ORDT collation tables for dictionary indexes (per-character).
//!
//! Kindle firmware resolves dictionary lookups by encoding the tapped word
//! one **character** at a time and binary-searching the orth INDX, whose
//! entry labels must be encoded the same way. The mapping is defined by a
//! pair of ORDT tables embedded in the primary INDX record. See issue #11.
//!
//! kindling routes Japanese, Chinese, Korean, and Arabic through this
//! module (see `uses_generated_ordt`). Latin/Greek/Cyrillic dictionaries
//! keep the UTF-16BE label scheme with the static Greek ORDT/SPL blob,
//! verified working on real devices.
//!
//! # The per-character scheme (validated on hardware)
//!
//! Each character of a headword becomes one label element:
//!
//! * A character that has a table symbol (kana, space, and a few ASCII
//!   markers) is stored as that symbol's index. `ORDT2[sym]` is the
//!   character's Unicode code point; `ORDT1[sym]` is its collation weight.
//! * Any other character (kanji, Hangul, Arabic letters, Latin, ...) is
//!   stored as an out-of-table **literal**: the raw Unicode code point.
//!   A label element is a literal exactly when its value is `>= oentries`
//!   (the table size), so every literal code point must be `>= oentries`
//!   (kanji at U+4E00+ always are; the rare low-code-point characters that
//!   appear are pulled into the table as symbols, see `new`).
//!
//! The firmware encodes a query the same way: kana characters are looked
//! up in ORDT2 by code point to get a symbol; everything else is a literal
//! code point. So hiragana/katakana fold (they share collation weights)
//! while kanji match exactly. This is why an index built one-symbol-per-
//! UTF-8-byte (kindling 0.16.0..0.18.0) resolved nothing on device: the
//! firmware's per-character query never matched the per-byte labels.
//!
//! The hiragana block U+3041..=U+3093 and the katakana block
//! U+30A1..=U+30F6 are always present as character symbols, so the firmware
//! can encode and fold arbitrary kana queries. Katakana fold onto the
//! matching hiragana weight (ア and あ share weight). Collation weights are
//! kindlegen's gojuon order (see `HIRA_WEIGHTS`).
//!
//! # Label width (`ordt_type`)
//!
//! Labels are two-byte big-endian elements (`ordt_type = 0`) whenever a
//! literal is present (a literal code point exceeds one byte) or the table
//! exceeds 256 symbols; otherwise one byte (`ordt_type = 1`). Any
//! dictionary with kanji/Hangul/Arabic, and any with the kana blocks
//! present, comes out two-byte.
//!
//! Entries in the orth INDX must be sorted by their zero-skipped weight
//! sequences (see `sort_key`); ties may appear in any order because the
//! firmware scans equal-weight ranges.

use std::collections::HashMap;

const HIRAGANA_FIRST: u32 = 0x3041; // ぁ
const HIRAGANA_LAST: u32 = 0x3093; // ん
const KATAKANA_FIRST: u32 = 0x30A1; // ァ
const KATAKANA_LAST: u32 = 0x30F6; // ヶ

/// kindlegen's gojuon collation weights for hiragana U+3041..=U+3093.
/// Katakana fold onto these (the matching hiragana weight); voiced and
/// small kana get distinct adjacent weights, leaving gaps.
const HIRA_WEIGHTS: [u16; 83] = [
    4, 5, 7, 8, 11, 12, 13, 14, 16, 17, 19, 20, 22, 23, 24, 26, 27, 29, 30, 32, 33, 35, 36, 37, 38,
    40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89,
    91, 92, 93, 94, 95, 96, 97, 98, 99, 100,
];

/// Collation weight of space (kindlegen value); below every kana weight.
const SPACE_WEIGHT: u16 = 3;
/// Weight given to in-table ASCII alphanumerics pulled in for safety; sorts
/// just above space and below kana. Punctuation pulled in is ignorable (0).
const ASCII_ALNUM_WEIGHT: u16 = 2;

/// Maximum encoded label size in bytes (5-bit length field, max 31; two-
/// byte elements need an even count, so 30).
const MAX_LABEL_BYTES_2: usize = 30;
const MAX_LABEL_BYTES_1: usize = 31;

/// Collation weight for a kana code point, folding katakana onto hiragana.
/// Returns None for non-kana (and the few katakana with no hiragana form,
/// which `new` weights separately).
fn kana_weight(cp: u32) -> Option<u16> {
    if (HIRAGANA_FIRST..=HIRAGANA_LAST).contains(&cp) {
        return Some(HIRA_WEIGHTS[(cp - HIRAGANA_FIRST) as usize]);
    }
    if (KATAKANA_FIRST..=KATAKANA_LAST).contains(&cp) {
        let folded = cp - 0x60; // katakana -> hiragana
        if (HIRAGANA_FIRST..=HIRAGANA_LAST).contains(&folded) {
            return Some(HIRA_WEIGHTS[(folded - HIRAGANA_FIRST) as usize]);
        }
    }
    None
}

/// Generated ORDT table pair plus the character-level encoder state.
pub struct OrdtTables {
    /// Symbol -> collation weight (0 = ignorable).
    ordt1: Vec<u16>,
    /// Symbol -> Unicode code point value.
    ordt2: Vec<u16>,
    /// Character -> symbol index, for the characters that have a symbol.
    sym_of: HashMap<char, u16>,
    /// True when labels are two-byte BE elements (`ordt_type` 0); false
    /// for single-byte elements (`ordt_type` 1).
    two_byte: bool,
}

impl OrdtTables {
    /// Build the per-character collation table for a dictionary whose
    /// lookup labels are `labels`. The hiragana and katakana blocks are
    /// always included as character symbols; any other character with a
    /// code point below the table size is pulled in as a symbol so it can
    /// never be mistaken for a literal; everything else is a literal code
    /// point at encode time.
    pub fn new(labels: &[&str]) -> OrdtTables {
        let mut ordt1: Vec<u16> = Vec::with_capacity(256);
        let mut ordt2: Vec<u16> = Vec::with_capacity(256);
        let mut sym_of: HashMap<char, u16> = HashMap::new();

        let mut add = |cp: u32, weight: u16, o1: &mut Vec<u16>, o2: &mut Vec<u16>, m: &mut HashMap<char, u16>| {
            let sym = o2.len() as u16;
            o2.push(cp as u16);
            o1.push(weight);
            if let Some(c) = char::from_u32(cp) {
                m.entry(c).or_insert(sym);
            }
        };

        // Fixed seed: NUL, %, _ (kindlegen always emits these three).
        add(0x0000, 0, &mut ordt1, &mut ordt2, &mut sym_of);
        add(0x0025, 0, &mut ordt1, &mut ordt2, &mut sym_of); // %
        add(0x005F, 0, &mut ordt1, &mut ordt2, &mut sym_of); // _

        // The kana blocks (and space) are embedded ONLY when the dictionary
        // actually contains kana. A large generated-collation table makes
        // the firmware treat the dictionary as kana-collated: harmless for
        // Japanese, but it breaks lookups for Chinese/Korean/Arabic, whose
        // characters are all literals and which kindlegen leaves with a
        // minimal 3-entry table. See issue #11 (Arabic regression).
        let has_kana = labels.iter().flat_map(|s| s.chars()).any(|c| {
            let cp = c as u32;
            (HIRAGANA_FIRST..=HIRAGANA_LAST).contains(&cp)
                || (KATAKANA_FIRST..=KATAKANA_LAST).contains(&cp)
        });
        if has_kana {
            add(0x0020, SPACE_WEIGHT, &mut ordt1, &mut ordt2, &mut sym_of); // space
            // Full hiragana block.
            for cp in HIRAGANA_FIRST..=HIRAGANA_LAST {
                add(cp, kana_weight(cp).unwrap(), &mut ordt1, &mut ordt2, &mut sym_of);
            }
            // Full katakana block, folded onto hiragana weights. The handful
            // with no hiragana form (ヴ ヵ ヶ) get weights just past the block.
            let mut extra = HIRA_WEIGHTS[HIRA_WEIGHTS.len() - 1] + 1;
            for cp in KATAKANA_FIRST..=KATAKANA_LAST {
                let w = kana_weight(cp).unwrap_or_else(|| {
                    let w = extra;
                    extra += 1;
                    w
                });
                add(cp, w, &mut ordt1, &mut ordt2, &mut sym_of);
            }
        }

        // Safety: any character used by a label whose code point is below
        // the current table size would otherwise be indistinguishable from
        // a symbol index when stored as a literal. Pull such characters in
        // as symbols. Iterate because each addition grows the table. This
        // does not fire for ordinary kana+kanji dictionaries.
        let mut used: Vec<char> = labels
            .iter()
            .flat_map(|s| s.chars())
            .filter(|c| !sym_of.contains_key(c))
            .collect();
        used.sort_unstable();
        used.dedup();
        loop {
            let size = ordt2.len() as u32;
            let mut added = false;
            for &c in &used {
                let cp = c as u32;
                if cp < size && cp <= 0xFFFF && !sym_of.contains_key(&c) {
                    let w = if c.is_ascii_alphanumeric() {
                        ASCII_ALNUM_WEIGHT
                    } else {
                        0
                    };
                    add(cp, w, &mut ordt1, &mut ordt2, &mut sym_of);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }

        // A character is a literal exactly when it has no table symbol;
        // the safety loop above guarantees every such character's code
        // point is >= the table size, so literals never collide with
        // symbol indices. Two-byte labels are needed when any literal is
        // present (literal code points exceed one byte) or the table
        // itself exceeds 256 symbols.
        let has_literal = labels
            .iter()
            .flat_map(|s| s.chars())
            .any(|c| !sym_of.contains_key(&c));
        let two_byte = has_literal || ordt2.len() > 256;

        OrdtTables {
            ordt1,
            ordt2,
            sym_of,
            two_byte,
        }
    }

    /// Number of table entries (the `oentries` INDX header field). Label
    /// elements with a value `>= count()` are literals.
    pub fn count(&self) -> u32 {
        self.ordt2.len() as u32
    }

    /// The `ordt_type` INDX header field: 0 for two-byte elements, 1 for
    /// single-byte elements.
    pub fn ordt_type(&self) -> u32 {
        if self.two_byte {
            0
        } else {
            1
        }
    }

    fn push_elem(&self, out: &mut Vec<u8>, v: u16) {
        if self.two_byte {
            out.extend_from_slice(&v.to_be_bytes());
        } else {
            out.push(v as u8);
        }
    }

    /// Encode a label as ORDT elements: one per character, the symbol index
    /// for table characters and the raw code point for literals. Truncated
    /// to the 5-bit length limit at a character boundary.
    pub fn encode_label(&self, text: &str) -> Vec<u8> {
        let max_bytes = if self.two_byte {
            MAX_LABEL_BYTES_2
        } else {
            MAX_LABEL_BYTES_1
        };
        let elem_bytes = if self.two_byte { 2 } else { 1 };
        let mut out: Vec<u8> = Vec::new();
        for ch in text.chars() {
            if out.len() + elem_bytes > max_bytes {
                break;
            }
            match self.sym_of.get(&ch) {
                Some(&sym) => self.push_elem(&mut out, sym),
                None => {
                    // Literal code point. BMP only (supplementary planes do
                    // not occur in dictionary headwords we target); skip if
                    // it would collide with the symbol-index range.
                    let cp = ch as u32;
                    if cp <= 0xFFFF && cp >= self.count() {
                        self.push_elem(&mut out, cp as u16);
                    }
                    // else: unrepresentable; drop the character.
                }
            }
        }
        out
    }

    /// Collation key for an encoded label: per-element ORDT1 weights with
    /// zero (ignorable) weights skipped. Out-of-table literals compare
    /// after every weighted symbol, by raw code point.
    pub fn sort_key(&self, label_bytes: &[u8]) -> Vec<u32> {
        let cnt = self.ordt1.len();
        let mut key = Vec::with_capacity(label_bytes.len());
        let mut push = |v: usize, key: &mut Vec<u32>| {
            if v < cnt {
                let w = self.ordt1[v];
                if w != 0 {
                    key.push(w as u32);
                }
            } else {
                key.push(0x1_0000 + v as u32);
            }
        };
        if self.two_byte {
            for chunk in label_bytes.chunks_exact(2) {
                push(u16::from_be_bytes([chunk[0], chunk[1]]) as usize, &mut key);
            }
        } else {
            for &b in label_bytes {
                push(b as usize, &mut key);
            }
        }
        key
    }

    /// Serialize the tables as the two `"ORDT" + u16 BE entries` blocks
    /// (ORDT1 weights, ORDT2 values) referenced from the primary INDX
    /// header at offsets 172 and 176.
    pub fn serialize(&self) -> (Vec<u8>, Vec<u8>) {
        let mut t1 = Vec::with_capacity(4 + self.ordt1.len() * 2);
        t1.extend_from_slice(b"ORDT");
        for &w in &self.ordt1 {
            t1.extend_from_slice(&w.to_be_bytes());
        }
        let mut t2 = Vec::with_capacity(4 + self.ordt2.len() * 2);
        t2.extend_from_slice(b"ORDT");
        for &v in &self.ordt2 {
            t2.extend_from_slice(&v.to_be_bytes());
        }
        (t1, t2)
    }
}

/// True if a dictionary input language selects the generated ORDT path.
///
/// Japanese (proven, issue #11), Chinese, Korean, and Arabic: scripts whose
/// firmware lookup encodes queries per character against the embedded ORDT
/// tables. Latin/Greek/Cyrillic dictionaries stay on the UTF-16BE path,
/// which is verified working on real devices.
pub fn uses_generated_ordt(lang: &str) -> bool {
    let primary = lang
        .split(['-', '_'])
        .next()
        .unwrap_or(lang)
        .to_ascii_lowercase();
    matches!(primary.as_str(), "ja" | "zh" | "ko" | "ar")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elems(o: &OrdtTables, bytes: &[u8]) -> Vec<u16> {
        if o.two_byte {
            bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect()
        } else {
            bytes.iter().map(|&b| b as u16).collect()
        }
    }

    #[test]
    fn kana_are_symbols_kanji_are_literals() {
        // 食べる: 食 (kanji) is a literal code point, べ and る are symbols.
        let o = OrdtTables::new(&["食べる", "あい"]);
        assert!(o.two_byte, "kanji literal forces two-byte labels");
        let e = elems(&o, &o.encode_label("食べる"));
        assert_eq!(e.len(), 3);
        assert_eq!(e[0], 0x98DF, "食 stored as its literal code point");
        assert!(e[0] as u32 >= o.count(), "literal is out of table range");
        assert!((e[1] as u32) < o.count(), "べ is a table symbol");
        assert_eq!(o.ordt2[e[1] as usize], 0x3079, "symbol value is べ");
        assert_eq!(o.ordt2[e[2] as usize], 0x308B, "symbol value is る");
    }

    #[test]
    fn hiragana_katakana_fold() {
        // あい and アイ collate equally (kana folding).
        let o = OrdtTables::new(&["あい", "アイ"]);
        let a = o.encode_label("あい");
        let b = o.encode_label("アイ");
        assert_ne!(a, b, "encodings stay distinct (different symbols)");
        assert_eq!(o.sort_key(&a), o.sort_key(&b), "but collation keys fold");
    }

    #[test]
    fn gojuon_order() {
        let o = OrdtTables::new(&["あ", "か", "さ", "ん", "が"]);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert!(k("あ") < k("か"));
        assert!(k("か") < k("さ"));
        assert!(k("さ") < k("ん"));
        // が (voiced) sorts right after か, before き.
        assert!(k("か") < k("が"));
        assert!(k("が") < k("き"));
    }

    #[test]
    fn kanji_sort_after_kana_by_codepoint() {
        let o = OrdtTables::new(&["あ", "山", "川"]);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert!(k("あ") < k("山"), "kana sort before kanji literals");
        // 山 U+5C71 < 川 U+5DDD by code point.
        assert!(k("山") < k("川"));
    }

    #[test]
    fn ordt_type_two_byte_with_literals() {
        let o = OrdtTables::new(&["山"]);
        assert_eq!(o.ordt_type(), 0);
        // Pure-kana, small table: one-byte labels.
        let o = OrdtTables::new(&["あい", "かき"]);
        assert_eq!(o.ordt_type(), 1, "no literals, table <256 -> one byte");
        let e = elems(&o, &o.encode_label("あい"));
        assert_eq!(e.len(), 2);
    }

    #[test]
    fn kana_blocks_only_when_kana_present() {
        // A kana dictionary embeds the full kana blocks so any kana query
        // can be encoded and folded.
        let ja = OrdtTables::new(&["あい", "山"]);
        assert_eq!(ja.ordt2[ja.sym_of[&'あ'] as usize], 0x3042);
        assert_eq!(ja.ordt2[ja.sym_of[&'ア'] as usize], 0x30A2);
        assert_eq!(
            ja.ordt1[ja.sym_of[&'ア'] as usize],
            ja.ordt1[ja.sym_of[&'あ'] as usize],
            "ア folds onto あ"
        );
        // A no-kana dictionary (Chinese/Korean/Arabic) keeps the minimal
        // 3-entry table; every character is a literal, matching kindlegen.
        // A large kana table makes the firmware mis-collate these scripts
        // (the Arabic regression: water resolved to sky). See issue #11.
        let zh = OrdtTables::new(&["山", "水", "爱"]);
        assert_eq!(zh.count(), 3, "no-kana dict keeps only NUL/%/_");
        assert!(!zh.sym_of.contains_key(&'あ'));
    }

    #[test]
    fn serialize_layout() {
        let o = OrdtTables::new(&["あ"]);
        let (t1, t2) = o.serialize();
        assert_eq!(&t1[..4], b"ORDT");
        assert_eq!(&t2[..4], b"ORDT");
        assert_eq!(t1.len(), 4 + o.count() as usize * 2);
        assert_eq!(t2.len(), 4 + o.count() as usize * 2);
        assert_eq!(&t2[4..6], &[0, 0]); // symbol 0 is NUL
    }

    #[test]
    fn generated_ordt_language_gate() {
        assert!(uses_generated_ordt("ja"));
        assert!(uses_generated_ordt("ja-JP"));
        assert!(uses_generated_ordt("zh"));
        assert!(uses_generated_ordt("ko"));
        assert!(uses_generated_ordt("ar"));
        assert!(!uses_generated_ordt("el"));
        assert!(!uses_generated_ordt("en"));
        assert!(!uses_generated_ordt("jam"));
    }
}
