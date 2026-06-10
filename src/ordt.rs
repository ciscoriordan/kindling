//! Generated ORDT collation tables for Japanese dictionary indexes.
//!
//! Kindle firmware resolves dictionary lookups by searching the orth INDX
//! with a collation defined inside the dictionary itself, via a pair of
//! ORDT tables embedded in the primary INDX record. kindlegen generates
//! these tables for Japanese input; an index without them (or sorted in a
//! different order than they imply) fails to resolve lookups on device.
//! See issue #11.
//!
//! This module replicates kindlegen's scheme, reverse-engineered from
//! kindlegen 2.9 output across four corpora (12-entry mixed-script,
//! 173-entry single-kana with two different source orders, 5961-entry
//! shuffled; all validated with zero sort violations):
//!
//! * Labels are stored as sequences of symbols, not raw text. With
//!   `ordt_type = 0` each symbol is a big-endian u16 in the label bytes.
//!   (kindlegen emits 1-byte symbols with `ordt_type = 1` for small
//!   alphabets; we always emit the u16 form, which kindlegen itself uses
//!   for any real-sized Japanese dictionary.)
//! * Symbols encode the UTF-8 bytes of the headword, one symbol per byte
//!   (with the exceptions below). `ORDT2[sym]` holds the symbol's value:
//!   the Windows-1252 interpretation of the byte as a BMP code point
//!   (e.g. byte 0x82 is stored as U+201A), or the raw byte value for the
//!   five cp1252-undefined bytes (0x81, 0x8D, 0x8F, 0x90, 0x9D).
//! * `ORDT1[sym]` holds the symbol's collation weight. Weight 0 means
//!   ignorable: the symbol is skipped during comparison. All UTF-8
//!   continuation bytes that map to cp1252 punctuation are ignorable,
//!   which is what folds kana variants together: the firmware finds the
//!   equal-weight range by binary search and disambiguates by scanning.
//! * Nonzero weights follow cp1252 primary-strength folding of the byte:
//!   space < digits (with ¹²³ folded into 1 2 3) < letters a..z (case
//!   and accents folded, so the kana/kanji lead bytes 0xE3..0xE5 land in
//!   the "a" group, 0xE7 in "c", 0xE8..0xE9 in "e", and so on) < the
//!   hiragana block. Only the relative order and the ties matter to the
//!   firmware, not the absolute weight values, so we assign dense group
//!   ranks rather than kindlegen's exact numbers.
//! * Three bytes whose cp1252 characters expand to letter pairs are
//!   encoded as two-symbol escape sequences headed by a control-valued
//!   symbol: 0x8C (Œ) becomes [U+0001-symbol, 'E'], 0x9C (œ) becomes
//!   [U+0002-symbol, 'e'], 0xE6 (æ) becomes [U+0004-symbol, 'e']. The
//!   head symbol carries the first letter's weight ('o', 'o', 'a').
//! * Two bytes are stored as out-of-table literal u16 values: 0x80 as
//!   U+20AC (€) and 0x99 as U+2122 (™). A literal compares after every
//!   weighted symbol, by raw value.
//! * The full hiragana block U+3041..=U+3093 is always present as
//!   character symbols with individually increasing weights after the
//!   letter groups. Labels never reference them, but kindlegen always
//!   emits them for Japanese and the firmware may consult them when
//!   normalizing queries, so we mirror that.
//!
//! Entries in the orth INDX must be sorted by their zero-skipped weight
//! sequences (see `sort_key`); ties may appear in any order because the
//! firmware scans equal-weight ranges.

const HIRAGANA_FIRST: u16 = 0x3041; // ぁ
const HIRAGANA_LAST: u16 = 0x3093; // ん

/// Maximum label size in bytes (the INDX entry length field kindling
/// writes is 5 bits wide, so 31; u16 symbols need an even count).
const MAX_LABEL_BYTES: usize = 30;

/// How a UTF-8 byte is represented in an encoded label.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ByteEnc {
    /// Single table symbol.
    Sym(u16),
    /// Out-of-table literal u16 value (0x80 -> U+20AC, 0x99 -> U+2122).
    Literal(u16),
    /// Two-symbol expansion escape (head control symbol, tail letter symbol).
    Esc(u16, u16),
    /// Byte does not occur in any label; never hit during encoding.
    Unmapped,
}

/// Primary-strength weight class of a byte under cp1252 folding.
#[derive(Clone, Copy, Debug, PartialEq)]
enum WeightClass {
    Ignorable,
    Space,
    /// 0..=9
    Digit(u8),
    /// 0..=25 for a..z
    Letter(u8),
}

/// Generated ORDT table pair plus the byte-level encoder state.
pub struct JaOrdt {
    /// Symbol -> collation weight (0 = ignorable).
    ordt1: Vec<u16>,
    /// Symbol -> value (cp1252 code point of the byte, raw byte for
    /// cp1252-undefined bytes, control code for escape heads, or the
    /// code point for hiragana character symbols).
    ordt2: Vec<u16>,
    /// Byte -> label encoding action.
    byte_enc: [ByteEnc; 256],
}

/// Weight of a letter index (0..=25 for a..=z).
fn letter_weight(l: u8) -> u16 {
    12 + l as u16
}

fn class_weight(c: WeightClass) -> u16 {
    match c {
        WeightClass::Ignorable => 0,
        WeightClass::Space => 1,
        WeightClass::Digit(d) => 2 + d as u16,
        WeightClass::Letter(l) => letter_weight(l),
    }
}

/// Primary weight class of a byte, following kindlegen's cp1252 folding
/// (observed in kindlegen output: å ã -> a, é -> e, ƒ -> f, Š š -> s,
/// Ž ž -> z, Ÿ -> y, µ -> m, ¹ ² ³ -> 1 2 3, NBSP -> space, all cp1252
/// punctuation and undefined bytes ignorable).
fn byte_class(b: u8) -> WeightClass {
    use WeightClass::*;
    match b {
        0x20 | 0xA0 => Space,
        b'0'..=b'9' => Digit(b - b'0'),
        b'a'..=b'z' => Letter(b - b'a'),
        b'A'..=b'Z' => Letter(b - b'A'),
        0x83 => Letter(5),                        // ƒ -> f
        0x8A | 0x9A => Letter(18),                // Š š -> s
        0x8E | 0x9E => Letter(25),                // Ž ž -> z
        0x9F => Letter(24),                       // Ÿ -> y
        0xB2 => Digit(2),                         // ²
        0xB3 => Digit(3),                         // ³
        0xB5 => Letter(12),                       // µ -> m
        0xB9 => Digit(1),                         // ¹
        0xC0..=0xC6 | 0xE0..=0xE5 => Letter(0),   // À..Æ à..å -> a
        0xC7 | 0xE7 => Letter(2),                 // Ç ç -> c
        0xC8..=0xCB | 0xE8..=0xEB => Letter(4),   // È..Ë è..ë -> e
        0xCC..=0xCF | 0xEC..=0xEF => Letter(8),   // Ì..Ï ì..ï -> i
        0xD0 | 0xF0 => Letter(3),                 // Ð ð -> d
        0xD1 | 0xF1 => Letter(13),                // Ñ ñ -> n
        0xD2..=0xD6 | 0xD8 | 0xF2..=0xF6 | 0xF8 => Letter(14), // Ò..Ø ò..ø -> o
        0xD9..=0xDC | 0xF9..=0xFC => Letter(20),  // Ù..Ü ù..ü -> u
        0xDD | 0xFD | 0xFF => Letter(24),         // Ý ý ÿ -> y
        0xDE | 0xFE => Letter(19),                // Þ þ -> t
        0xDF => Letter(18),                       // ß -> s
        _ => Ignorable,
    }
}

/// cp1252 code point of a byte (raw byte value for the five undefined
/// bytes 0x81, 0x8D, 0x8F, 0x90, 0x9D, matching kindlegen's tables).
fn cp1252_value(b: u8) -> u16 {
    match b {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        _ => b as u16,
    }
}

impl JaOrdt {
    /// Build tables for a dictionary whose labels use the given set of
    /// byte values (`used_bytes[b]` true if byte `b` occurs in any
    /// label's UTF-8 encoding). The printable ASCII range and the
    /// hiragana block are always included so the firmware can encode
    /// arbitrary Latin queries against the same tables.
    pub fn new(used_bytes: &[bool; 256]) -> JaOrdt {
        let mut ordt1: Vec<u16> = Vec::with_capacity(384);
        let mut ordt2: Vec<u16> = Vec::with_capacity(384);
        let mut byte_enc = [ByteEnc::Unmapped; 256];

        let add = |value: u16, weight: u16, ordt1: &mut Vec<u16>, ordt2: &mut Vec<u16>| -> u16 {
            let sym = ordt2.len() as u16;
            ordt2.push(value);
            ordt1.push(weight);
            sym
        };

        // Symbol 0 is always NUL (kindlegen invariant).
        byte_enc[0] = ByteEnc::Sym(add(0, 0, &mut ordt1, &mut ordt2));

        // Printable ASCII, always present.
        for b in 0x20u8..=0x7E {
            let sym = add(b as u16, class_weight(byte_class(b)), &mut ordt1, &mut ordt2);
            byte_enc[b as usize] = ByteEnc::Sym(sym);
        }

        // Any other low bytes that somehow appear in labels: raw value,
        // ignorable. (XML text content cannot normally contain these.)
        for b in 0x01u8..=0x1F {
            if used_bytes[b as usize] {
                let sym = add(b as u16, 0, &mut ordt1, &mut ordt2);
                byte_enc[b as usize] = ByteEnc::Sym(sym);
            }
        }
        if used_bytes[0x7F] {
            let sym = add(0x7F, 0, &mut ordt1, &mut ordt2);
            byte_enc[0x7F] = ByteEnc::Sym(sym);
        }

        // High bytes actually used by labels. 0x80 and 0x99 become
        // literals; 0x8C, 0x9C, 0xE6 become escapes (resolved below).
        for b in 0x80u8..=0xFF {
            if !used_bytes[b as usize] {
                continue;
            }
            match b {
                0x80 => byte_enc[b as usize] = ByteEnc::Literal(0x20AC),
                0x99 => byte_enc[b as usize] = ByteEnc::Literal(0x2122),
                0x8C | 0x9C | 0xE6 => {}
                _ => {
                    let sym = add(
                        cp1252_value(b),
                        class_weight(byte_class(b)),
                        &mut ordt1,
                        &mut ordt2,
                    );
                    byte_enc[b as usize] = ByteEnc::Sym(sym);
                }
            }
        }

        // Expansion escape heads. The control value identifies the
        // expansion (U+0001 = Œ, U+0002 = œ, U+0004 = æ); the head
        // symbol carries the first expansion letter's weight and the
        // tail is the plain second-letter symbol.
        let sym_of = |byte_enc: &[ByteEnc; 256], b: u8| -> u16 {
            match byte_enc[b as usize] {
                ByteEnc::Sym(s) => s,
                _ => unreachable!("ASCII letters are always table symbols"),
            }
        };
        if used_bytes[0x8C] {
            let head = add(0x0001, letter_weight(14), &mut ordt1, &mut ordt2); // 'o'
            byte_enc[0x8C] = ByteEnc::Esc(head, sym_of(&byte_enc, b'E'));
        }
        if used_bytes[0x9C] {
            let head = add(0x0002, letter_weight(14), &mut ordt1, &mut ordt2); // 'o'
            byte_enc[0x9C] = ByteEnc::Esc(head, sym_of(&byte_enc, b'e'));
        }
        if used_bytes[0xE6] {
            let head = add(0x0004, letter_weight(0), &mut ordt1, &mut ordt2); // 'a'
            byte_enc[0xE6] = ByteEnc::Esc(head, sym_of(&byte_enc, b'e'));
        }

        // Hiragana character symbols, always present, weights strictly
        // increasing after the letter groups.
        let hira_base = letter_weight(25) + 1; // first weight after 'z'
        for cp in HIRAGANA_FIRST..=HIRAGANA_LAST {
            add(cp, hira_base + (cp - HIRAGANA_FIRST), &mut ordt1, &mut ordt2);
        }

        JaOrdt {
            ordt1,
            ordt2,
            byte_enc,
        }
    }

    /// Number of table entries (the `oentries` INDX header field).
    pub fn count(&self) -> u32 {
        self.ordt2.len() as u32
    }

    /// Encode a label as big-endian u16 symbol bytes, truncated to
    /// `MAX_LABEL_BYTES` at a character boundary (escape pairs are never
    /// split).
    pub fn encode_label(&self, text: &str) -> Vec<u8> {
        let max_syms = MAX_LABEL_BYTES / 2;
        let mut syms: Vec<u16> = Vec::with_capacity(max_syms);
        let mut buf = [0u8; 4];
        for ch in text.chars() {
            let mut ch_syms: [u16; 8] = [0; 8];
            let mut n = 0;
            for &b in ch.encode_utf8(&mut buf).as_bytes() {
                match self.byte_enc[b as usize] {
                    ByteEnc::Sym(s) => {
                        ch_syms[n] = s;
                        n += 1;
                    }
                    ByteEnc::Literal(v) => {
                        ch_syms[n] = v;
                        n += 1;
                    }
                    ByteEnc::Esc(head, tail) => {
                        ch_syms[n] = head;
                        ch_syms[n + 1] = tail;
                        n += 2;
                    }
                    ByteEnc::Unmapped => {
                        // Byte missing from the table this JaOrdt was
                        // built from. Degrade to a large literal so the
                        // label still encodes deterministically.
                        debug_assert!(false, "byte 0x{b:02X} not in ORDT alphabet");
                        ch_syms[n] = 0xFFFD;
                        n += 1;
                    }
                }
            }
            if syms.len() + n > max_syms {
                break;
            }
            syms.extend_from_slice(&ch_syms[..n]);
        }
        let mut out = Vec::with_capacity(syms.len() * 2);
        for s in syms {
            out.extend_from_slice(&s.to_be_bytes());
        }
        out
    }

    /// Collation key for an encoded label: per-symbol ORDT1 weights with
    /// zero (ignorable) weights skipped. Out-of-table literals compare
    /// after every weighted symbol, by raw value.
    pub fn sort_key(&self, label_bytes: &[u8]) -> Vec<u32> {
        let cnt = self.ordt1.len();
        let mut key = Vec::with_capacity(label_bytes.len() / 2);
        for chunk in label_bytes.chunks_exact(2) {
            let s = u16::from_be_bytes([chunk[0], chunk[1]]) as usize;
            if s < cnt {
                let w = self.ordt1[s];
                if w != 0 {
                    key.push(w as u32);
                }
            } else {
                key.push(0x1_0000 + s as u32);
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

/// True if a dictionary input language selects the Japanese ORDT path.
pub fn is_japanese(lang: &str) -> bool {
    lang == "ja" || lang.starts_with("ja-") || lang.starts_with("ja_")
}

/// Collect the set of UTF-8 byte values used across an iterator of
/// label strings.
pub fn used_bytes<'a>(labels: impl Iterator<Item = &'a str>) -> [bool; 256] {
    let mut used = [false; 256];
    for label in labels {
        for &b in label.as_bytes() {
            used[b as usize] = true;
        }
    }
    used
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordt_for(labels: &[&str]) -> JaOrdt {
        JaOrdt::new(&used_bytes(labels.iter().copied()))
    }

    #[test]
    fn kana_fold_hiragana_katakana_equal() {
        // あ (E3 81 82) and ア (E3 82 A2) differ only in ignorable
        // continuation bytes, so their collation keys are equal. This is
        // the equal-weight range the firmware scans for kana folding.
        let o = ordt_for(&["あい", "アイ"]);
        let a = o.encode_label("あい");
        let b = o.encode_label("アイ");
        assert_ne!(a, b, "encodings must stay distinct");
        assert_eq!(o.sort_key(&a), o.sort_key(&b));
    }

    #[test]
    fn lead_byte_folding_orders_kanji_groups() {
        // Kanji lead bytes fold like cp1252 letters: E4/E5 (ä å -> a)
        // before E7 (ç -> c) before E9 (é -> e).
        let o = ordt_for(&["人", "山", "火", "金"]);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert_eq!(k("人"), k("山")); // E4 and E5 both fold to 'a'
        assert!(k("山") < k("火"));
        assert!(k("火") < k("金"));
    }

    #[test]
    fn expansion_escapes() {
        // が is E3 81 8C; 0x8C (Œ) must encode as the two-symbol
        // [U+0001 head, 'E'] expansion. 愛 is E6 84 9B; 0xE6 (æ) must
        // encode as [U+0004 head, 'e'].
        let o = ordt_for(&["がき", "愛"]);
        let ga = o.encode_label("が");
        assert_eq!(ga.len(), 8, "3 bytes -> 4 symbols (one escape pair)");
        let syms: Vec<u16> = ga
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let head = syms[2] as usize;
        assert_eq!(o.ordt2[head], 0x0001, "Œ escape head control value");
        assert_eq!(o.ordt1[head], letter_weight(14), "head carries 'o' weight");

        let ai = o.encode_label("愛");
        assert_eq!(ai.len(), 8);
        let syms: Vec<u16> = ai
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        let head = syms[0] as usize;
        assert_eq!(o.ordt2[head], 0x0004, "æ escape head control value");
        assert_eq!(o.ordt1[head], letter_weight(0), "head carries 'a' weight");
    }

    #[test]
    fn literal_bytes() {
        // す is E3 81 99; byte 0x99 is stored as the literal U+2122.
        // む is E3 82 80; byte 0x80 is stored as the literal U+20AC.
        let o = ordt_for(&["す", "む", "ま"]);
        let su: Vec<u16> = o
            .encode_label("す")
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(su[2], 0x2122);
        assert!(su[2] as u32 >= o.count(), "literal is out of table range");
        let mu: Vec<u16> = o
            .encode_label("む")
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(mu[2], 0x20AC);
        // Literals sort after all weighted symbols.
        let plain = o.sort_key(&o.encode_label("ま")); // E3 81 BE, fully ignorable trail
        let lit = o.sort_key(&o.encode_label("す"));
        assert!(lit > plain);
    }

    #[test]
    fn voiced_kana_with_escape_sorts_after_plain() {
        // かき (trail bytes ignorable) keys shorter than がき (whose 0x8C
        // expands to weighted o+E symbols), so かき sorts first. This
        // matches kindlegen's output ordering.
        let o = ordt_for(&["かき", "がき"]);
        let ka = o.sort_key(&o.encode_label("かき"));
        let ga = o.sort_key(&o.encode_label("がき"));
        assert!(ka < ga);
    }

    #[test]
    fn hiragana_char_symbols_present_with_increasing_weights() {
        let o = ordt_for(&["あ"]);
        let cnt = o.count() as usize;
        // The last 83 symbols are ぁ..ん.
        let first_hira = cnt - (HIRAGANA_LAST - HIRAGANA_FIRST + 1) as usize;
        for (i, sym) in (first_hira..cnt).enumerate() {
            assert_eq!(o.ordt2[sym], HIRAGANA_FIRST + i as u16);
            if i > 0 {
                assert_eq!(o.ordt1[sym], o.ordt1[sym - 1] + 1);
            }
        }
        // Hiragana weights start after the letter groups.
        assert!(o.ordt1[first_hira] > letter_weight(25));
    }

    #[test]
    fn truncation_keeps_char_boundaries_and_even_length() {
        let long: String = "あいうえおかきくけこさしすせそ".chars().cycle().take(40).collect();
        let o = ordt_for(&[long.as_str()]);
        let enc = o.encode_label(&long);
        assert!(enc.len() <= MAX_LABEL_BYTES);
        assert_eq!(enc.len() % 2, 0);
        // 30 bytes = 15 symbols = 5 three-symbol kana.
        assert_eq!(enc.len(), 30);
    }

    #[test]
    fn ascii_weights_fold_case_and_order_after_digits() {
        let o = ordt_for(&["abc", "ABC", "a1"]);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert_eq!(k("abc"), k("ABC"));
        assert!(k("a1") > k("a0"));
        assert!(k("0") < k("a"));
        assert!(k(" ") < k("0"));
    }

    #[test]
    fn serialize_layout() {
        let o = ordt_for(&["あ"]);
        let (t1, t2) = o.serialize();
        assert_eq!(&t1[..4], b"ORDT");
        assert_eq!(&t2[..4], b"ORDT");
        assert_eq!(t1.len(), 4 + o.count() as usize * 2);
        assert_eq!(t2.len(), 4 + o.count() as usize * 2);
        // Symbol 0 is NUL with weight 0.
        assert_eq!(&t2[4..6], &[0, 0]);
        assert_eq!(&t1[4..6], &[0, 0]);
    }

    #[test]
    fn is_japanese_variants() {
        assert!(is_japanese("ja"));
        assert!(is_japanese("ja-JP"));
        assert!(is_japanese("ja_JP"));
        assert!(!is_japanese("el"));
        assert!(!is_japanese("en"));
        assert!(!is_japanese("jam")); // Jamaican Creole, not Japanese
    }
}
