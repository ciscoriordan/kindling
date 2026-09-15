//! Generated ORDT collation tables for dictionary indexes (per-character).
//!
//! Kindle firmware resolves dictionary lookups by encoding the tapped word
//! one **character** at a time and binary-searching the orth INDX, whose
//! entry labels must be encoded the same way. The mapping is defined by a
//! pair of ORDT tables embedded in the primary INDX record. See issue #11.
//!
//! kindling routes Japanese, Chinese, Korean, and the Arabic-script
//! languages (`ar`, `fa`, `ur`, `ps`, `ug`, `sd`, `ckb`) through this module
//! (see `uses_generated_ordt`). Latin dictionaries, and any dictionary built
//! with `--strict-accents`, get a per-character ORDT (see `new_exact`) that
//! keeps each character a distinct symbol. `--fold-accents` instead stores
//! Latin labels as plain UTF-16 with the ORDT/SPL blob taken from a kindlegen
//! Greek dictionary, which Greek dictionaries get by default. Cyrillic and
//! other scripts store plain UTF-16 labels.
//! Outside the generated tables every index is sorted by
//! `device_collation_key`, the weights a Kindle lookup gives each character.
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
//! matching hiragana weight (ア and あ share weight); ヴ ヵ ヶ collate as
//! their base kana う か け. Collation weights are kindlegen's gojuon order
//! (see `HIRA_WEIGHTS`).
//!
//! # The prolonged sound mark ー (U+30FC)
//!
//! ー is not a literal: the firmware normalizes a tapped ー onto the
//! preceding vowel before searching, and kindlegen does the same when it
//! builds the index, so the stored label must match. ー folds to a
//! vowel-specific marker (a→U+3095, i→U+3096, u→U+3097, e→U+3098,
//! o→U+309F) carrying that vowel's collation weight; the fold propagates
//! across consecutive ー. With no preceding vowel (word start, after ん/ン)
//! it stays as the ignorable mark. The middle dot ・ and the iteration
//! marks are likewise kept as weight-0 (ignorable) symbols. See
//! `label_codepoints`. A raw-ー label (kindling through 0.18.0) never
//! resolved on device; this is the post-0.18.0 follow-up to issue #11.
//!
//! # Label width (`ordt_type`)
//!
//! Labels are two-byte big-endian elements (`ordt_type = 0`) whenever a
//! literal is present (a literal code point exceeds one byte) or the table
//! exceeds 256 symbols; otherwise one byte (`ordt_type = 1`). A dictionary
//! with kanji/Hangul/Arabic comes out two-byte; a pure-kana dictionary
//! (every ー folded, so no literals remain) comes out one-byte, matching
//! kindlegen.
//!
//! Entries in the orth INDX must be sorted by their zero-skipped weight
//! sequences (see `sort_key`); ties may appear in any order because the
//! firmware scans equal-weight ranges.

use std::borrow::Cow;
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

/// Maximum encoded label size in bytes. The entry writer's length field is a
/// full byte capped at 254 (`encode_indx_entry`), and kindlegen stores long
/// dictionary labels in full (a 34-byte Korean phrase label was verified
/// intact in a kindlegen production-scale build), so the encoder must not cut
/// earlier: an earlier 30-byte cap (a leftover of the removed 5-bit length
/// field) silently truncated 15+ character Korean headwords, which then could
/// never exact-match a query (issue #22). 254 keeps two-byte elements on an
/// even boundary.
const MAX_LABEL_BYTES: usize = 254;

/// Collation weight for a kana code point, folding katakana onto hiragana.
/// Returns None for non-kana. ヴ ヵ ヶ (and the hiragana ゔ ゕ ゖ) sit just
/// past ん in Unicode; kindlegen collates them as their base kana う か け,
/// so they do too (verified on device via the kindlegen reference).
fn kana_weight(cp: u32) -> Option<u16> {
    // Fold katakana onto hiragana (ヴ→ゔ, ヵ→ゕ, ヶ→ゖ, ン→ん all land here).
    let h = if (HIRAGANA_FIRST..=0x3096).contains(&cp) {
        cp
    } else if (KATAKANA_FIRST..=KATAKANA_LAST).contains(&cp) {
        cp - 0x60
    } else {
        return None;
    };
    let weight_of = |base: u32| HIRA_WEIGHTS[(base - HIRAGANA_FIRST) as usize];
    match h {
        _ if (HIRAGANA_FIRST..=HIRAGANA_LAST).contains(&h) => Some(weight_of(h)),
        0x3094 => Some(weight_of(0x3046)), // ゔ / ヴ collate as う
        0x3095 => Some(weight_of(0x304B)), // ゕ / ヵ collate as か
        0x3096 => Some(weight_of(0x3051)), // ゖ / ヶ collate as け
        _ => None,
    }
}

/// The katakana-hiragana prolonged sound mark ー.
const PROLONGED: u32 = 0x30FC;

/// kindlegen's vowel-specific markers for a folded prolonged sound mark,
/// indexed by vowel class (0=a, 1=i, 2=u, 3=e, 4=o). The firmware
/// normalizes a tapped ー to one of these before searching the index, so
/// the stored label must use the same marker (a raw ー never resolves).
/// U+3097/U+3098 are unassigned Unicode scalars but still valid `char`s,
/// so they round-trip through the table like any other symbol.
const LONG_FOLD: [u32; 5] = [0x3095, 0x3096, 0x3097, 0x3098, 0x309F];

/// Katakana marks that collate as ignorable (weight 0) yet stay in the
/// label as in-table symbols, matching kindlegen: an unfoldable prolonged
/// mark (word-initial or after ん/ン), the middle dot, and the katakana/
/// hiragana iteration marks.
const IGNORABLE_MARKS: [u32; 6] = [0x30FC, 0x30FB, 0x30FD, 0x30FE, 0x309D, 0x309E];

/// Vowel class (0=a, 1=i, 2=u, 3=e, 4=o) of each hiragana in
/// U+3041..=U+3096, or 255 for ん (no inherent vowel). Katakana fold onto
/// this table by code point. Small tsu っ/ッ takes vowel u (it is small
/// つ); ゔ/ヴ take u; ゕ/ヵ take a; ゖ/ヶ take e.
const HIRA_VOWEL: [u8; 86] = [
    0, 0, 1, 1, 2, 2, 3, 3, 4, 4, // ぁあぃいぅうぇえぉお
    0, 0, 1, 1, 2, 2, 3, 3, 4, 4, // かがきぎくぐけげこご
    0, 0, 1, 1, 2, 2, 3, 3, 4, 4, // さざしじすずせぜそぞ
    0, 0, 1, 1, 2, 2, 2, 3, 3, 4, 4, // ただちぢっつづてでとど
    0, 1, 2, 3, 4, // なにぬねの
    0, 0, 0, 1, 1, 1, 2, 2, 2, 3, 3, 3, 4, 4,
    4, // はばぱひびぴふぶぷへべぺほぼぽ
    0, 1, 2, 3, 4, // まみむめも
    0, 0, 2, 2, 4, 4, // ゃやゅゆょよ
    0, 1, 2, 3, 4, // らりるれろ
    0, 0, 1, 3, 4, 255, 2, 0, 3, // ゎわゐゑをん ゔゕゖ
];

/// Vowel class of a kana code point, folding katakana onto hiragana.
/// `None` for ん/ン, the prolonged mark, and non-kana. Drives the fold of
/// a following prolonged sound mark ー onto the preceding vowel.
pub(crate) fn kana_vowel(cp: u32) -> Option<usize> {
    // ヴ→ゔ, ヵ→ゕ, ヶ→ゖ, ン→ん all land inside the hiragana range.
    let h = match cp {
        KATAKANA_FIRST..=KATAKANA_LAST => cp - 0x60,
        // ヷヸヹヺ have no hiragana of their own; they weigh as わゐゑを.
        0x30F7..=0x30FA => cp - 0x68,
        _ => cp,
    };
    if (HIRAGANA_FIRST..=0x3096).contains(&h) {
        let v = HIRA_VOWEL[(h - HIRAGANA_FIRST) as usize];
        if v != 255 {
            return Some(v as usize);
        }
    }
    None
}

/// Plain-vowel (あいうえお) collation weight for vowel class `v`; the
/// long-vowel fold markers carry it so ー sorts as the long vowel.
fn vowel_weight(v: usize) -> u16 {
    const VOWEL_HIRA: [u32; 5] = [0x3042, 0x3044, 0x3046, 0x3048, 0x304A];
    HIRA_WEIGHTS[(VOWEL_HIRA[v] - HIRAGANA_FIRST) as usize]
}

/// Rewrite a headword into the code-point sequence actually encoded into
/// its index label. The prolonged sound mark ー (U+30FC) folds to a
/// vowel-specific marker based on the preceding vowel sound, propagating
/// across consecutive ー; with no preceding vowel it stays as the literal
/// mark (an ignorable symbol). Every other character passes through. This
/// mirrors kindlegen, whose folded labels resolve on real devices while
/// raw-ー labels do not (the firmware normalizes a tapped ー the same way).
pub(crate) fn label_codepoints(text: &str) -> Vec<u32> {
    let mut out = Vec::with_capacity(text.len());
    let mut vowel: Option<usize> = None;
    for ch in text.chars() {
        let cp = ch as u32;
        if cp == PROLONGED {
            // A following ー keeps folding onto the same vowel, so leave
            // `vowel` untouched here.
            out.push(match vowel {
                Some(v) => LONG_FOLD[v],
                None => PROLONGED,
            });
        } else {
            out.push(cp);
            vowel = kana_vowel(cp);
        }
    }
    out
}

/// Generated ORDT table pair plus the character-level encoder state.
pub struct OrdtTables {
    /// Symbol -> collation weight (0 = ignorable).
    ordt1: Vec<u16>,
    /// Symbol -> Unicode code point value.
    ordt2: Vec<u16>,
    /// Character -> symbol index, for the characters that have a symbol.
    sym_of: HashMap<char, u16>,
    /// Surrogate unit -> symbol index, for the characters outside the BMP
    /// in an exact table ([`OrdtTables::new_exact`]), which stores both
    /// units of such a character as symbols. Empty in the generated tables,
    /// which store them as literals.
    surrogate_sym: HashMap<u16, u16>,
    /// True when labels are two-byte BE elements (`ordt_type` 0); false
    /// for single-byte elements (`ordt_type` 1).
    two_byte: bool,
    /// True when labels are stored as [`stored_label_text`] writes them
    /// ([`OrdtTables::new_exact`]); false for the generated kana and literal
    /// tables, which store every character as it is.
    stores_letter_pairs: bool,
}

/// The base letter a Latin letter below U+0300 weighs as in a Kindle
/// dictionary lookup, as an ASCII lowercase letter, or `None` for any other
/// code unit.
///
/// The Kindle 4 (firmware 4.1.4) and the Paperwhite 4 (firmware 5.16.5) fold
/// case and accents for these letters and no others: every letter from U+00C0
/// to U+017F except `ß`, `æ`, `Æ`, `œ`, `Œ`, `ĳ` and `Ĳ`, the micro sign `µ`,
/// about half of Latin Extended-B, and most of the IPA letters. `ð` and `Đ`
/// weigh as d, `þ` and `Þ` as t, `ı` and `İ` as i, `ĸ` as k, `ŉ` and `ŋ` as n,
/// `ſ` as s, the Vietnamese `ơ` and `ư` as o and u, and `ɐ` as a. The Latin
/// letters this leaves out, among them Romanian `ș` and `ț`, weigh nothing (see
/// [`device_weight`]).
fn latin_base_letter(unit: u16) -> Option<u8> {
    let base = match unit {
        0x41..=0x5A => unit as u8 + 0x20,
        0x61..=0x7A => unit as u8,
        0xC0..=0xC5
        | 0xE0..=0xE5
        | 0x100..=0x105
        | 0x1CD..=0x1CE
        | 0x1DE..=0x1E1
        | 0x1FA..=0x1FC
        | 0x200..=0x203
        | 0x250 => b'a',
        0x180..=0x183 | 0x253 | 0x299 => b'b',
        0xC7 | 0xE7 | 0x106..=0x10D | 0x187..=0x188 | 0x255 | 0x297 => b'c',
        0xD0 | 0xF0 | 0x10E..=0x111 | 0x189..=0x18C | 0x256..=0x257 => b'd',
        0xC8..=0xCB
        | 0xE8..=0xEB
        | 0x112..=0x11B
        | 0x18E..=0x190
        | 0x1DD
        | 0x204..=0x207
        | 0x258..=0x25A => b'e',
        0x191..=0x192 => b'f',
        0x11C..=0x123 | 0x193..=0x194 | 0x1E4..=0x1E7 | 0x1F4..=0x1F5 | 0x260..=0x262 | 0x29B => {
            b'g'
        }
        0x124..=0x127 | 0x265..=0x267 | 0x29C => b'h',
        0xCC..=0xCF
        | 0xEC..=0xEF
        | 0x128..=0x131
        | 0x197
        | 0x1CF..=0x1D0
        | 0x208..=0x20B
        | 0x268
        | 0x26A => b'i',
        0x134..=0x135 | 0x1F0 | 0x25F | 0x284 | 0x29D => b'j',
        0x136..=0x138 | 0x198..=0x199 | 0x1E8..=0x1E9 | 0x29E => b'k',
        0x139..=0x142 | 0x19A | 0x26B..=0x26D | 0x29F => b'l',
        0xB5 | 0x19C | 0x26F..=0x271 => b'm',
        0xD1 | 0xF1 | 0x143..=0x14B | 0x19D..=0x19E | 0x272..=0x274 => b'n',
        0xD2..=0xD6
        | 0xD8
        | 0xF2..=0xF6
        | 0xF8
        | 0x14C..=0x151
        | 0x186
        | 0x19F..=0x1A1
        | 0x1D1..=0x1D2
        | 0x1EA..=0x1ED
        | 0x1FE..=0x1FF
        | 0x20C..=0x20F
        | 0x254
        | 0x275 => b'o',
        0x1A4..=0x1A5 => b'p',
        0x2A0 => b'q',
        0x154..=0x159 | 0x1A6 | 0x210..=0x213 | 0x279..=0x281 => b'r',
        0x15A..=0x161 | 0x17F | 0x282 => b's',
        0xDE | 0xFE | 0x162..=0x167 | 0x1AB..=0x1AE | 0x287..=0x288 => b't',
        0xD9..=0xDC
        | 0xF9..=0xFC
        | 0x168..=0x173
        | 0x1AF..=0x1B0
        | 0x1D3..=0x1DC
        | 0x214..=0x217
        | 0x289 => b'u',
        0x1B2 | 0x28B..=0x28C => b'v',
        0x174..=0x175 | 0x28D => b'w',
        0xDD | 0xFD | 0xFF | 0x176..=0x178 | 0x1B3..=0x1B4 | 0x28E..=0x28F => b'y',
        0x179..=0x17E | 0x1B5..=0x1B6 | 0x290..=0x291 => b'z',
        _ => return None,
    };
    Some(base)
}

/// The hiragana each halfwidth katakana from U+FF66 to U+FF9D weighs as,
/// with the prolonged sound mark U+FF70, which weighs nothing, as `ー`.
const HALFWIDTH_KANA_BASES: &str = "をぁぃぅぇぉゃゅょっーあいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわん";

/// The weight of one UTF-16 code unit of a stored label, or of a tapped word,
/// in a Kindle dictionary lookup; 0 means the unit is passed over.
///
/// The Kindle 4 (firmware 4.1.4) and the Paperwhite 4 (firmware 5.16.5)
/// weigh every unit this way, whatever tables the dictionary embeds: in
/// kindling's exact-accent, `--fold-accents` and `--strict-accents` builds,
/// in plain UTF-16 Greek and Cyrillic builds, and in kindlegen's. An index
/// stored out of the order of these weights can hide labels, the misplaced one
/// or its neighbors, so every index kindling writes outside the generated
/// tables of [`uses_generated_ordt`] is sorted by [`device_collation_key`].
///
/// - Weight 0: the control characters (U+0000, U+0006 to U+000F, U+0015 to
///   U+001F, U+007F to U+009F), all ASCII punctuation, U+00A1 to U+00BF
///   except `µ` and `¹²³`, `×` and `÷`, the Latin letters below U+0300 that
///   [`latin_base_letter`] leaves out (`ß`, `æ`, `œ`, `ĳ`, the DŽ, LJ and NJ
///   digraphs, Romanian `ș` and `ț` and the rest of U+0218 to U+024F, and
///   letters such as `ǽ`, `ɑ`, `ɛ`, `ɣ`, `ʃ` and `ʒ`), all of U+02B0 to U+02FF
///   (the modifier letters, U+02BC included), General Punctuation U+2000 to
///   U+206F, the arrows, mathematical operators, technical symbols, box
///   drawing, geometric shapes, miscellaneous symbols, dingbats and
///   supplemental arrows (U+2190 to U+23FF, U+2500 to U+27FF, U+2900 to
///   U+2BFF), the CJK punctuation U+3001 to U+3040, the kana voicing and
///   iteration marks, the middle dot and the prolonged sound marks, every
///   Hangul syllable, U+115A to U+1160, U+11A3 to U+11A7, the Hangul filler
///   U+3164, and the fullwidth and halfwidth punctuation.
/// - The markers kindling and kindlegen store `ß`, `æ`, `Æ`, `œ` and `Œ` as
///   (see [`stored_label_text`]): U+0001 and U+0002 weigh as o, U+0003 and
///   U+0004 as a, U+0005 as s.
/// - Latin letters below U+0300 and the fullwidth Latin letters: their ASCII
///   lowercase base letter.
/// - U+0020, the no-break space and the ideographic space: a space.
/// - The digits, `¹²³` and the fullwidth digits: the digit.
/// - Katakana and halfwidth katakana: the matching hiragana.
/// - Every other unit weighs as itself, so it keeps its case and accents: the
///   combining marks, Greek, Cyrillic, Armenian, Hebrew, Arabic, Latin
///   Extended Additional (`ạ`, `Ấ`, `ẞ`), the superscripts from U+2070, the
///   ideographs, and each surrogate of a character outside the BMP, so such a
///   character sorts after U+D7FF and before U+E000.
///
/// The Hangul jamo are the exception: this gives them their code units, but
/// the devices weigh them differently (see [`is_unmodeled_hangul_jamo`]). A
/// label holding one is still stored and sorted as if by its code units, so
/// in a dictionary sorted this way the Kindle can open nothing for it, and
/// nothing for some labels stored near it.
pub(crate) fn device_weight(unit: u16) -> u16 {
    match unit {
        0x01 | 0x02 => u16::from(b'o'),
        0x03 | 0x04 => u16::from(b'a'),
        0x05 => u16::from(b's'),
        // Vowel markers with the weight of あいうえお; nothing kindling sorts
        // this way stores them.
        0x10..=0x14 => [0x3042, 0x3044, 0x3046, 0x3048, 0x304A][usize::from(unit - 0x10)],
        0x20 | 0xA0 | 0x3000 => 0x20,
        0x30..=0x39 => unit,
        0xB9 => u16::from(b'1'),
        0xB2 => u16::from(b'2'),
        0xB3 => u16::from(b'3'),
        0x00..=0x2FF => latin_base_letter(unit).map_or(0, u16::from),
        0x115A..=0x1160 | 0x11A3..=0x11A7 => 0,
        0x2000..=0x206F | 0x2190..=0x23FF | 0x2500..=0x27FF | 0x2900..=0x2BFF => 0,
        0x3001..=0x3040 | 0x3099..=0x309E | 0x30A0 | 0x30FB..=0x30FF | 0x3164 => 0,
        0x3094 | 0x3097 => 0x3046,
        0x3095 => 0x3042,
        0x3096 => 0x3044,
        0x3098 => 0x3048,
        0x309F => 0x304A,
        0x30A1..=0x30F3 => unit - 0x60,
        0x30F4 => 0x3046,
        0x30F5 => 0x304B,
        0x30F6 => 0x3051,
        0x30F7..=0x30FA => unit - 0x68,
        0xAC00..=0xD7A3 => 0,
        0xFF00..=0xFF0F | 0xFF1A..=0xFF20 | 0xFF3B..=0xFF40 | 0xFF5B..=0xFF65 => 0,
        0xFF70 | 0xFF9E..=0xFF9F => 0,
        0xFF10..=0xFF19 => unit - 0xFF10 + u16::from(b'0'),
        0xFF21..=0xFF3A => unit - 0xFF21 + u16::from(b'a'),
        0xFF41..=0xFF5A => unit - 0xFF41 + u16::from(b'a'),
        0xFF66..=0xFF9D => HALFWIDTH_KANA_BASES
            .chars()
            .nth(usize::from(unit - 0xFF66))
            .map_or(unit, |c| c as u16),
        _ => unit,
    }
}

/// Whether `unit` is a Hangul jamo that a Kindle weighs other than
/// [`device_weight`] says: a conjoining jamo from U+1100 to U+11F9 or a
/// compatibility jamo from U+3131 to U+318E, other than those that weigh
/// nothing, such as the fillers.
///
/// The Kindle 4 and the Paperwhite 4 weigh the leading, trailing and
/// compatibility forms of one consonant the same (`ᄀ`, `ᆨ` and `ㄱ`), every
/// vowel the same, and all of them after the CJK ideographs. kindling does not
/// reproduce those weights, so outside the generated tables of
/// [`uses_generated_ordt`] a label holding one of these units is stored where
/// a lookup may not reach it, and it can hide labels stored near it.
pub(crate) fn is_unmodeled_hangul_jamo(unit: u16) -> bool {
    matches!(unit, 0x1100..=0x11F9 | 0x3131..=0x318E) && device_weight(unit) != 0
}

/// The weights of `units` with the units that weigh nothing left out: the key
/// a Kindle dictionary lookup compares a stored label and a tapped word by,
/// lexicographically, a key that is a prefix of another comparing smaller.
///
/// The Kindle 4 and the Paperwhite 4 binary-search the stored labels with
/// this key and then gather the adjacent labels whose key equals the word's,
/// so an index out of this order can hide labels. So "T-shirt"
/// sorts as "tshirt", "școală" as "coala", "Ω" before "α", "x😀y" before
/// "xﬁy", and in a Russian dictionary "apple" before "Zebra".
pub(crate) fn device_collation_key(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .map(|&unit| device_weight(unit))
        .filter(|&weight| weight != 0)
        .collect()
}

/// The units of `units` that weigh nothing (see [`device_weight`]), in
/// order. Among labels with the same [`device_collation_key`] that the
/// earlier tie rules do not separate, the one with fewer comes first, and
/// after the letter-pair rule these units are compared in code order.
pub(crate) fn skipped_units(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .copied()
        .filter(|&unit| device_weight(unit) == 0)
        .collect()
}

/// How many of `ß`, `æ`, `Æ`, `œ`, `Œ`, `ĳ` and `Ĳ` a headword spells as one
/// letter. [`stored_label_text`] writes each as two, so "Straße" and
/// "Strasse" have the same key; when the earlier tie rules do not separate
/// them, the spelling without the letter comes first.
/// A marker followed by its second letter counts as the letter, as
/// [`stored_label_text`] reads it.
pub(crate) fn one_letter_pair_count(label: &str) -> usize {
    marker_pairs_joined(label)
        .chars()
        .filter(|&c| matches!(c, 'ĳ' | 'Ĳ') || expansion_of(c).is_some())
        .count()
}

/// The character a cp1252 byte stands for.
///
/// Only 0x80..=0x9F differ from Latin-1. The five slots cp1252 leaves
/// undefined map to the C1 control at the same position, which is what
/// Windows' own decoder does with them.
pub(crate) fn cp1252_char(b: u8) -> char {
    let cp: u32 = match b {
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
        other => other as u32,
    };
    char::from_u32(cp).unwrap_or('\u{FFFD}')
}

/// The character an ORDT expansion marker stands for.
///
/// Five characters collate as two letters, and kindlegen stores each as a
/// symbol whose ORDT2 value is a small control number, followed by one more
/// symbol for the letter's second half. So "Straße" is `S t r a [5] s e`: the
/// marker is the ß and the `s` after it belongs to the ß. The values 1 for Œ
/// (Œuvre), 2 for œ (Bœuf), 3 for Æ (Ærø) and 5 for ß (Aasfraß) were read off
/// a production German dictionary, and 4 is æ: a label stored with it opens
/// for æble, aeble and AEBLE. kindling writes the same form (see
/// [`stored_label_text`]), in one-byte and two-byte tables and in plain
/// UTF-16 labels.
pub(crate) fn expansion_char(value: u32) -> Option<char> {
    match value {
        1 => Some('Œ'),
        2 => Some('œ'),
        3 => Some('Æ'),
        4 => Some('æ'),
        5 => Some('ß'),
        _ => None,
    }
}

/// How a Kindle dictionary search reads `ß`, `æ`, `Æ`, `œ` and `Œ`.
///
/// A typed word is searched with each of these letters written as two: `ß` as
/// "ss", `æ` and `Æ` as "ae", `œ` and `Œ` as "oe", in any case, so on the
/// Kindle 4 and the Paperwhite 4 "Fuß", "FUSS" and "Fuss" all find the same
/// labels. A label is found by those spellings only when it stores the letter
/// as its marker (the value [`expansion_char`] maps back to the letter)
/// followed by the letter's second half. The pair is compared as the two
/// letters and shown as the one letter, and since the Kindle opens the label
/// shown exactly as the word was typed, "Straße" and "Strasse" in one
/// dictionary each open their own entry.
///
/// Returns the marker value, the letter the marker is compared as, and the
/// letter stored after it. That second letter is exactly `s`, `e` or `E` as
/// listed; with any other letter after it the Kindle shows the marker as it
/// is, not as the letter.
pub(crate) fn expansion_of(c: char) -> Option<(u32, char, char)> {
    match c {
        'Œ' => Some((1, 'o', 'E')),
        'œ' => Some((2, 'o', 'e')),
        'Æ' => Some((3, 'a', 'E')),
        'æ' => Some((4, 'a', 'e')),
        'ß' => Some((5, 's', 's')),
        _ => None,
    }
}

/// A headword as kindling stores it in an index label, so that a Kindle
/// search finds it by its usual spellings.
///
/// - `ß`, `æ`, `Æ`, `œ` and `Œ` become their marker value followed by their
///   second letter (see [`expansion_of`]), which kindlegen writes too. "Fuß"
///   is then found by "Fuß", "FUSS" and "Fuss", and no longer by "Fu".
/// - `ĳ` and `Ĳ` become the two letters `ij` and `IJ`, as kindlegen stores
///   them. The Kindle passes over a typed `ĳ` as well as a stored one, so no
///   label can be found both by "ĳs" and by "ijs", and a reader's text spells
///   the word "ijs" nearly always.
/// - A control character U+0001 to U+0005 followed by the second letter of
///   the letter it marks is read as that letter (see [`marker_pairs_joined`]),
///   since a dictionary unpacked from a Kindle file spells `ß`, `æ` and `œ`
///   that way. Any other U+0001 to U+0005 is left out, since a label holding
///   one would show it as one of the letters above.
pub(crate) fn stored_label_text(label: &str) -> String {
    let mut out = String::with_capacity(label.len() + 4);
    for c in marker_pairs_joined(label).chars() {
        match c {
            '\u{1}'..='\u{5}' => {}
            'ĳ' => out.push_str("ij"),
            'Ĳ' => out.push_str("IJ"),
            _ => match expansion_of(c) {
                Some((marker, _, second)) => {
                    out.extend(char::from_u32(marker));
                    out.push(second);
                }
                None => out.push(c),
            },
        }
    }
    out
}

/// `label` with each marker that is followed by the second letter of its
/// letter written as that letter: `Fu`, U+0005, `s` becomes "Fuß", and
/// U+0001 followed by `Euvre` becomes "Œuvre". A tool that unpacks a Kindle
/// dictionary without decoding the markers writes its headwords that way.
pub(crate) fn marker_pairs_joined(label: &str) -> Cow<'_, str> {
    if !label.contains(['\u{1}', '\u{2}', '\u{3}', '\u{4}', '\u{5}']) {
        return Cow::Borrowed(label);
    }
    let mut out = String::with_capacity(label.len());
    let mut chars = label.chars().peekable();
    while let Some(c) = chars.next() {
        let pair = expansion_char(c as u32)
            .and_then(|letter| expansion_of(letter).map(|(_, _, second)| (letter, second)));
        match pair {
            Some((letter, second)) if chars.peek() == Some(&second) => {
                chars.next();
                out.push(letter);
            }
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// A label as kindling stores it in plain UTF-16, in a dictionary with no
/// table of its own, UTF-16BE bytes ready for the index entry.
///
/// The characters are those an exact table stores ([`OrdtTables::new_exact`]):
/// [`stored_label_text`], with a prolonged sound mark `ー` after kana written
/// as the marker of its vowel (see [`label_codepoints`]), since a Kindle
/// searches for a tapped `ー` that way and weighs a stored one as nothing. The
/// label is cut to the entry writer's 254 bytes before the first character
/// that does not fit whole, so a surrogate pair, or a marker and the letter
/// after it, is never split.
pub(crate) fn encode_plain_label(label: &str) -> Vec<u8> {
    let mut kept = String::new();
    let mut bytes = 0;
    for c in label_codepoints(&stored_label_text(label))
        .into_iter()
        .filter_map(char::from_u32)
    {
        // A marker and the letter after it are cut together or not at all.
        let needed = if expansion_char(c as u32).is_some() {
            4
        } else {
            2 * c.len_utf16()
        };
        if bytes + needed > MAX_LABEL_BYTES {
            break;
        }
        bytes += 2 * c.len_utf16();
        kept.push(c);
    }
    crate::indx::encode_indx_label(&kept)
}

impl OrdtTables {
    /// Build the per-character collation table for a dictionary whose
    /// lookup labels are `labels`. The hiragana and katakana blocks are
    /// always included as character symbols; any other character with a
    /// code point below the table size is pulled in as a symbol so it can
    /// never be mistaken for a literal; everything else is a literal code
    /// point at encode time.
    pub fn new(labels: &[&str]) -> OrdtTables {
        // The code points actually encoded into each label: the prolonged
        // sound mark ー is folded onto the preceding vowel here, so the
        // table is built from (and sized for) the folded forms, exactly
        // like the labels `encode_label` later produces.
        let label_cps: Vec<Vec<u32>> = labels.iter().map(|s| label_codepoints(s)).collect();

        let mut ordt1: Vec<u16> = Vec::with_capacity(256);
        let mut ordt2: Vec<u16> = Vec::with_capacity(256);
        let mut sym_of: HashMap<char, u16> = HashMap::new();

        let add = |cp: u32,
                   weight: u16,
                   o1: &mut Vec<u16>,
                   o2: &mut Vec<u16>,
                   m: &mut HashMap<char, u16>| {
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
                add(
                    cp,
                    kana_weight(cp).unwrap(),
                    &mut ordt1,
                    &mut ordt2,
                    &mut sym_of,
                );
            }
            // Full katakana block, folded onto hiragana weights (ヴ ヵ ヶ
            // collate as う か け, like kindlegen).
            for cp in KATAKANA_FIRST..=KATAKANA_LAST {
                add(
                    cp,
                    kana_weight(cp).unwrap(),
                    &mut ordt1,
                    &mut ordt2,
                    &mut sym_of,
                );
            }

            // Long-vowel fold markers for ー, each carrying its plain
            // vowel's collation weight so ー sorts as a long vowel. Added
            // only for the vowels that folding actually produced.
            for (v, &fold_cp) in LONG_FOLD.iter().enumerate() {
                if label_cps.iter().flatten().any(|&cp| cp == fold_cp) {
                    add(
                        fold_cp,
                        vowel_weight(v),
                        &mut ordt1,
                        &mut ordt2,
                        &mut sym_of,
                    );
                }
            }
            // Ignorable katakana marks (unfoldable ー, middle dot, iteration
            // marks): kept in the label as weight-0 symbols, like kindlegen,
            // so they neither become high-sorting literals nor are dropped.
            for &mark in &IGNORABLE_MARKS {
                let c = char::from_u32(mark).unwrap();
                if !sym_of.contains_key(&c) && label_cps.iter().flatten().any(|&cp| cp == mark) {
                    add(mark, 0, &mut ordt1, &mut ordt2, &mut sym_of);
                }
            }
        } else if labels.iter().any(|s| s.contains(' ')) {
            // kindlegen extends the minimal no-kana table with the space
            // symbol when any headword contains a space (verified on a
            // production-scale Korean build: ORDT2 = [NUL, %, _, space],
            // ORDT1 weights [0, 0, 0, 3]). Multi-word headwords then encode
            // space as symbol 3 rather than the literal 0x0020.
            add(0x0020, SPACE_WEIGHT, &mut ordt1, &mut ordt2, &mut sym_of);
        }

        // Safety: any character used by a label whose code point is below
        // the current table size would otherwise be indistinguishable from
        // a symbol index when stored as a literal. Pull such characters in
        // as symbols. Iterate because each addition grows the table. This
        // does not fire for ordinary kana+kanji dictionaries.
        let mut used: Vec<char> = label_cps
            .iter()
            .flatten()
            .filter_map(|&cp| char::from_u32(cp))
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

        // A code point is a literal exactly when it has no table symbol;
        // the safety loop above guarantees every such code point is >= the
        // table size, so literals never collide with symbol indices.
        // Two-byte labels are needed when any literal is present (literal
        // code points exceed one byte) or the table exceeds 256 symbols.
        // Pure-kana dictionaries have no literals once ー is folded, so
        // they come out one-byte, matching kindlegen.
        let has_literal = label_cps
            .iter()
            .flatten()
            .any(|&cp| match char::from_u32(cp) {
                Some(c) => !sym_of.contains_key(&c),
                None => true,
            });
        let two_byte = has_literal || ordt2.len() > 256;

        OrdtTables {
            ordt1,
            ordt2,
            sym_of,
            surrogate_sym: HashMap::new(),
            two_byte,
            stores_letter_pairs: false,
        }
    }

    /// Build the per-character ORDT for a Latin dictionary (the exact-accent
    /// default) or for any dictionary built with `--strict-accents`.
    ///
    /// Every character the dictionary uses becomes its own ORDT symbol, so the
    /// encoded labels keep ê distinct from e and the Kindle shows each headword
    /// as written. The ORDT1 weight of a symbol is the rank of its
    /// [`device_weight`] among the weights the dictionary uses, 0 for a
    /// character that weighs nothing, so accented and case variants of a Latin
    /// letter share a weight (é=è=ê=ë=e, à=â=ä=a, A=a, ...), while a Greek,
    /// Cyrillic or Armenian capital keeps a weight of its own. Sorting the
    /// labels by these weights gives the order of [`device_collation_key`],
    /// which is the order the Kindle 4 and the Paperwhite 4 search in; they do
    /// not read the ORDT1 weights, which are written to agree with that order.
    /// The folded order is the counter-intuitive part (issue #8): sorted by
    /// distinct per-character weights instead, accented headwords scattered far
    /// from their base, and a folded search for `meme` landed on `même` and
    /// never reached `mère`.
    ///
    /// Labels are encoded from [`stored_label_text`]: `ß`, `æ`, `Æ`, `œ` and
    /// `Œ` as a marker symbol (weighing as `s`, `a` or `o`) followed by the
    /// symbol of its second letter, and `ĳ`, `Ĳ` as `ij`, `IJ`, so the table
    /// carries a symbol for each marker used and never one for these letters
    /// themselves. A character outside the BMP is stored as its two surrogate
    /// units, and each unit gets a symbol too, whose weight ranks where the
    /// device weighs it, after U+D7FF and before U+E000.
    pub fn new_exact(labels: &[&str]) -> OrdtTables {
        let mut ordt1: Vec<u16> = Vec::new();
        let mut ordt2: Vec<u16> = Vec::new();
        let mut sym_of: HashMap<char, u16> = HashMap::new();
        let mut surrogate_sym: HashMap<u16, u16> = HashMap::new();

        // Fixed seed: NUL, %, _ (kindlegen always emits these as ignorable
        // weight-0 symbols, and the Kindle weighs them 0 too).
        for seed in ['\0', '%', '_'] {
            sym_of.insert(seed, ordt2.len() as u16);
            ordt2.push(seed as u16);
            ordt1.push(0);
        }

        let mut units: Vec<u16> = Vec::new();
        let mut supplementary = false;
        for label in labels {
            for cp in label_codepoints(&stored_label_text(label)) {
                supplementary |= cp > 0xFFFF;
                if let Some(c) = char::from_u32(cp) {
                    units.extend_from_slice(c.encode_utf16(&mut [0u16; 2]));
                }
            }
        }
        units.sort_unstable();
        units.dedup();

        // The rank of each weight the dictionary uses, from 1.
        let mut weights: Vec<u16> = units
            .iter()
            .map(|&unit| device_weight(unit))
            .filter(|&weight| weight != 0)
            .collect();
        weights.sort_unstable();
        weights.dedup();
        let rank = |unit: u16| match device_weight(unit) {
            0 => 0,
            weight => weights.binary_search(&weight).map_or(0, |i| i as u16 + 1),
        };

        for unit in units {
            let sym = ordt2.len() as u16;
            match char::from_u32(u32::from(unit)) {
                Some(c) if sym_of.contains_key(&c) => continue,
                Some(c) => {
                    sym_of.insert(c, sym);
                }
                None => {
                    surrogate_sym.insert(unit, sym);
                }
            }
            ordt2.push(unit);
            ordt1.push(rank(unit));
        }

        // Single-byte labels (ordt_type = 1, like kindlegen) when the table
        // fits a byte index. A surrogate pair keeps the labels two-byte, as
        // kindlegen stores such a character.
        let two_byte = ordt2.len() > 256 || supplementary;

        OrdtTables {
            ordt1,
            ordt2,
            sym_of,
            surrogate_sym,
            two_byte,
            stores_letter_pairs: true,
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
        if self.two_byte { 0 } else { 1 }
    }

    fn push_elem(&self, out: &mut Vec<u8>, v: u16) {
        if self.two_byte {
            out.extend_from_slice(&v.to_be_bytes());
        } else {
            out.push(v as u8);
        }
    }

    /// Encode a label as ORDT elements: one per character, the symbol index
    /// for table characters and the raw code point for literals. A
    /// supplementary-plane literal becomes its UTF-16 surrogate pair (two
    /// elements), exactly as kindlegen stores it (verified: kindlegen encodes
    /// a U+200D7 headword as elements D840 DCD7) and as `encode_indx_label`
    /// does on the UTF-16BE path; dropping such characters instead produced
    /// zero-length labels that sorted to the head of the index, a structure
    /// kindlegen never emits (issue #22). An exact table
    /// ([`OrdtTables::new_exact`]) encodes [`stored_label_text`] instead, and
    /// stores the two surrogates as symbols. The prolonged sound mark ー is
    /// folded onto the preceding vowel first (see `label_codepoints`).
    /// Truncated at a character boundary to the entry writer's 254-byte
    /// length cap, never splitting a surrogate pair or a marker pair.
    pub fn encode_label(&self, text: &str) -> Vec<u8> {
        let elem_bytes = if self.two_byte { 2 } else { 1 };
        let mut out: Vec<u8> = Vec::new();
        let stored;
        let text = if self.stores_letter_pairs {
            stored = stored_label_text(text);
            stored.as_str()
        } else {
            text
        };
        for cp in label_codepoints(text) {
            // A marker and the letter after it are cut together or not at all.
            let needed =
                if cp > 0xFFFF || (self.stores_letter_pairs && expansion_char(cp).is_some()) {
                    elem_bytes * 2
                } else {
                    elem_bytes
                };
            if out.len() + needed > MAX_LABEL_BYTES {
                break;
            }
            if self.stores_letter_pairs && cp > 0xFFFF {
                // Both surrogates are symbols in an exact table.
                let mut units = [0u16; 2];
                if let Some(c) = char::from_u32(cp) {
                    for unit in c.encode_utf16(&mut units) {
                        if let Some(&sym) = self.surrogate_sym.get(unit) {
                            self.push_elem(&mut out, sym);
                        }
                    }
                }
                continue;
            }
            match char::from_u32(cp).and_then(|c| self.sym_of.get(&c)) {
                Some(&sym) => self.push_elem(&mut out, sym),
                None => {
                    // Out-of-table literal. Representable only in two-byte
                    // labels (a one-byte table is built only when no literals
                    // are present). Symbol indices occupy the low element
                    // values, so a literal must be >= the table size; BMP
                    // code points below it cannot occur (the safety loop in
                    // `new` pulls them into the table).
                    if self.two_byte && cp <= 0xFFFF && cp >= self.count() {
                        self.push_elem(&mut out, cp as u16);
                    } else if self.two_byte && cp > 0xFFFF {
                        let adjusted = cp - 0x1_0000;
                        self.push_elem(&mut out, (0xD800 + (adjusted >> 10)) as u16);
                        self.push_elem(&mut out, (0xDC00 + (adjusted & 0x3FF)) as u16);
                    }
                    // else (one-byte table): unreachable, no literals exist.
                }
            }
        }
        out
    }

    /// The UTF-16 code units an encoded label stands for, as a Kindle reads
    /// it: each symbol replaced by its ORDT2 value, each literal kept.
    pub fn stored_units(&self, label_bytes: &[u8]) -> Vec<u16> {
        let value = |elem: u16| match self.ordt2.get(usize::from(elem)) {
            Some(&v) => v,
            None => elem,
        };
        if self.two_byte {
            label_bytes
                .chunks_exact(2)
                .map(|c| value(u16::from_be_bytes([c[0], c[1]])))
                .collect()
        } else {
            label_bytes.iter().map(|&b| value(u16::from(b))).collect()
        }
    }

    /// Collation key for an encoded label: per-element ORDT1 weights with
    /// zero (ignorable) weights skipped. Out-of-table literals compare
    /// after every weighted symbol, by raw code point.
    pub fn sort_key(&self, label_bytes: &[u8]) -> Vec<u32> {
        let cnt = self.ordt1.len();
        let mut key = Vec::with_capacity(label_bytes.len());
        let push = |v: usize, key: &mut Vec<u32>| {
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

    /// Serialize the tables as the two `"ORDT" + entries` blocks (ORDT1
    /// weights, ORDT2 values) referenced from the primary INDX header at
    /// offsets 172 and 176.
    ///
    /// ORDT1 element width tracks `ordt_type`: 1 byte/symbol for
    /// `ordt_type = 1` (one-byte labels), 2 bytes/symbol for `ordt_type = 0`.
    /// This matches what the firmware, KindleUnpack, and kindlegen read back:
    /// KindleUnpack uses `'B'` for ORDT1, and a kindlegen `ordt_type = 0`
    /// build (the committed `ja-kindlegen.mobi`) lays ORDT1 out at 2
    /// bytes/symbol (`ordt2_off - ordt1_off == 4 + oentries * 2`). Writing
    /// ORDT1 at a fixed 2 bytes regardless of type scrambled collation on the
    /// `ordt_type = 1` path, because each weight's zero high byte was read as
    /// a separate symbol's weight (issue #13). ORDT2 holds code points and is
    /// always 2 bytes/symbol. Weights are small (0-255), so the one-byte cast
    /// never truncates.
    pub fn serialize(&self) -> (Vec<u8>, Vec<u8>) {
        let mut t1 = Vec::with_capacity(4 + self.ordt1.len() * 2);
        t1.extend_from_slice(b"ORDT");
        if self.two_byte {
            for &w in &self.ordt1 {
                t1.extend_from_slice(&w.to_be_bytes());
            }
        } else {
            for &w in &self.ordt1 {
                t1.push(w as u8);
            }
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
/// Japanese (proven, issue #11), Chinese, and Korean encode queries per
/// character against the embedded ORDT tables. The Arabic-script languages
/// (Arabic, Persian, Urdu, Pashto, Uyghur, Sindhi, Central Kurdish) all store
/// their letters as literal code points through the same all-literal table
/// that `ar` uses (no kana, so `OrdtTables::new` emits the minimal table), so
/// they share this path; only the MOBI locale differs per language. Latin,
/// Greek, and Cyrillic dictionaries stay on the UTF-16BE / exact-ORDT path.
pub fn uses_generated_ordt(lang: &str) -> bool {
    let primary = lang
        .split(['-', '_'])
        .next()
        .unwrap_or(lang)
        .to_ascii_lowercase();
    matches!(
        primary.as_str(),
        // CJK
        "ja" | "zh" | "ko"
        // Arabic-script (all-literal ORDT, like `ar`)
        | "ar" | "fa" | "ur" | "ps" | "ug" | "sd" | "ckb"
    )
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
    fn strict_accents_folds_weights_keeps_symbols_distinct() {
        // Issue #8: --strict-accents routes a Latin dictionary through the
        // full per-letter ORDT (new_exact). Every character is its own symbol
        // (so labels distinguish ê from e), but accent/case variants share a
        // collation WEIGHT so accented headwords sort ADJACENT to their base.
        // The devices do not weigh labels by this table, so this test only
        // checks the layout.
        let o = OrdtTables::new_exact(&["même", "meme", "mère", "mere"]);
        assert!(!o.two_byte, "single-byte symbol labels (ordt_type=1)");
        assert!(
            o.count() > 3,
            "full per-letter table, not the 3-symbol seed"
        );

        // Distinct symbols: meme and même encode to different labels.
        assert_ne!(
            o.encode_label("même"),
            o.encode_label("meme"),
            "accented and base forms must encode to distinct symbols"
        );
        assert_ne!(o.encode_label("mère"), o.encode_label("mere"));

        // Folded weights: meme and même collate EQUAL (sort adjacent), unlike
        // the distinct-symbol labels. Same for mere/mère.
        assert_eq!(
            o.sort_key(&o.encode_label("même")),
            o.sort_key(&o.encode_label("meme")),
            "accent and base must share a collation weight (sort adjacent)"
        );
        assert_eq!(
            o.sort_key(&o.encode_label("mère")),
            o.sort_key(&o.encode_label("mere"))
        );
        // ...but meme's folded key sorts below mere's (e-weight < r-weight),
        // so the groups stay in alphabetical order.
        assert!(o.sort_key(&o.encode_label("meme")) < o.sort_key(&o.encode_label("mere")));
    }

    #[test]
    fn exact_ordt_skips_punctuation_in_collation() {
        // The Kindle 4 skips punctuation when it binary-searches the index, so
        // a punctuated label must collate as its letters alone. Every
        // character keeps its own symbol, so the stored label is unchanged.
        let words = [
            "T-shirt",
            "tshirt",
            "tutt'al più",
            "tutt’al più",
            "tuttal più",
            "a.C.",
            "aC",
            "«ciao»",
            "ciao",
            "5 €",
            "5€",
            "5",
            "sì—no",
            "sìno",
            "n.º",
            "n",
            "Straße",
            "Strasse",
            "Strae",
            "a\u{A0}b",
            "a b",
            "m²",
            "m2",
        ];
        let o = OrdtTables::new_exact(&words);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert_ne!(o.encode_label("T-shirt"), o.encode_label("tshirt"));
        assert_eq!(k("T-shirt"), k("tshirt"));
        assert_eq!(k("tutt'al più"), k("tuttal più"));
        assert_eq!(
            k("tutt’al più"),
            k("tuttal più"),
            "U+2019 is skipped like ASCII '"
        );
        assert_eq!(k("a.C."), k("aC"));
        assert_eq!(k("«ciao»"), k("ciao"));
        assert_eq!(k("sì—no"), k("sìno"), "the em dash is skipped");
        assert_eq!(k("n.º"), k("n"), "the ordinal indicator is skipped");
        assert_eq!(k("Straße"), k("Strasse"), "ß sorts as ss");
        assert_ne!(k("Straße"), k("Strae"), "ß is not skipped");
        assert_ne!(o.encode_label("Straße"), o.encode_label("Strasse"));
        assert_eq!(
            k("a\u{A0}b"),
            k("a b"),
            "a no-break space weighs as a space"
        );
        assert_eq!(k("m²"), k("m2"), "a superscript digit weighs as its digit");
        let lone = OrdtTables::new_exact(&["m", "m²", "ma"]);
        let kl = |s: &str| lone.sort_key(&lone.encode_label(s));
        assert!(
            kl("m") < kl("m²") && kl("m²") < kl("ma"),
            "² weighs as a digit even with no plain 2 in the table"
        );
        assert_ne!(k("5€"), k("5"), "€ keeps its weight");
        assert_ne!(k("5 €"), k("5€"), "space keeps its weight");
        assert_eq!(
            k("T-shirt").len(),
            "tshirt".len(),
            "the hyphen contributes nothing"
        );
    }

    #[test]
    fn exact_ordt_keeps_non_latin_capitals_apart() {
        // Only Latin letters share a weight across case. The Kindle 4 and the
        // Paperwhite 4 missed "πατέρας" when "π.Χ." sorted after it as "πχ",
        // and "Ấn Độ" when it sorted after "ấn".
        let words = [
            "π.Χ.",
            "πάθος",
            "πατέρας",
            "πχ",
            "Лук'ян",
            "лука",
            "Ա.Մ.Ն.",
            "ամիս",
            "Ấn Độ",
            "ấm",
            "T-shirt",
            "tshirt",
            "É",
            "e",
            "Øresund",
            "Paris",
            "Ω",
            "ω",
        ];
        let o = OrdtTables::new_exact(&words);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert!(k("π.Χ.") < k("πάθος"), "Χ U+03A7 weighs below ά U+03AC");
        assert!(k("πατέρας") < k("πχ"));
        assert!(
            k("Лук'ян") < k("лука"),
            "Cyrillic capitals keep their own weight"
        );
        assert!(
            k("Ա.Մ.Ն.") < k("ամիս"),
            "Armenian capitals keep their own weight"
        );
        assert!(k("Ấn Độ") < k("ấm"), "Ấ U+1EA4 weighs below ấ U+1EA5");
        assert!(k("Ω") < k("ω"));
        assert_eq!(k("T-shirt"), k("tshirt"), "Latin still folds case");
        assert_eq!(k("É"), k("e"), "Latin still folds case and accent");
        assert!(k("Øresund") < k("Paris"), "Ø still folds to o");
        // The weights order labels as the device key of the stored units does.
        let dk = |s: &str| device_collation_key(&o.stored_units(&o.encode_label(s)));
        for a in words {
            for b in words {
                assert_eq!(k(a).cmp(&k(b)), dk(a).cmp(&dk(b)), "{a} vs {b}");
            }
        }
    }

    #[test]
    fn exact_ordt_writes_weight_zero_for_skipped_characters() {
        // The serialized table, not only the in-memory one, gives every skipped
        // character weight 0, in both label widths, so the weights written into
        // the file agree with the order the labels are sorted in. kindlegen
        // writes weight 0 for this punctuation too. The Kindle 4 and the
        // Paperwhite 4 do not read these weights for a Latin dictionary:
        // rewriting them in a built file changes no lookup, and only the label
        // order matters.
        let words = [
            "T-shirt",
            "tutt’altro",
            "a.C.",
            "«ciao»",
            "sì—no",
            "5 €",
            "Straße",
            "m²",
            "școală",
            "Hawaiʻi",
            "ǀhoa",
        ];
        for extra in [None, Some("\u{20000}")] {
            let mut all: Vec<&str> = words.to_vec();
            all.extend(extra);
            let o = OrdtTables::new_exact(&all);
            let (t1, t2) = o.serialize();
            let width = if o.ordt_type() == 1 { 1 } else { 2 };
            for i in 0..o.count() as usize {
                let cp = u16::from_be_bytes([t2[4 + 2 * i], t2[5 + 2 * i]]) as u32;
                let w = if width == 1 {
                    t1[4 + i] as u16
                } else {
                    u16::from_be_bytes([t1[4 + 2 * i], t1[5 + 2 * i]])
                };
                let skipped = device_weight(cp as u16) == 0;
                assert_eq!(w == 0, skipped, "U+{cp:04X} has weight {w}");
            }
        }
    }

    #[test]
    fn exact_ordt_stores_two_letter_forms_like_kindlegen() {
        // ß, æ, Æ, œ and Œ are stored as a marker symbol whose ORDT2 value
        // names the letter, followed by the symbol of the letter's second
        // half; ĳ is stored as i and j. A Kindle finds a ß, æ or œ label by
        // the letter and by its two-letter spelling, and an ĳ label only by ij.
        for extra in [None, Some("\u{20000}")] {
            let mut words = vec![
                "Straße", "Strasse", "cœur", "Œdipe", "æble", "Ærø", "ĳs", "Fusion",
            ];
            words.extend(extra);
            let o = OrdtTables::new_exact(&words);
            let (t1, t2) = o.serialize();
            let width = if o.ordt_type() == 1 { 1 } else { 2 };
            let value = |sym: usize| u16::from_be_bytes([t2[4 + 2 * sym], t2[5 + 2 * sym]]) as u32;
            let weight = |sym: usize| {
                if width == 1 {
                    t1[4 + sym] as u16
                } else {
                    u16::from_be_bytes([t1[4 + 2 * sym], t1[5 + 2 * sym]])
                }
            };
            let elems = |s: &str| -> Vec<usize> {
                let b = o.encode_label(s);
                if width == 1 {
                    b.iter().map(|&x| x as usize).collect()
                } else {
                    b.chunks(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]) as usize)
                        .collect()
                }
            };
            let text = |s: &str| -> Vec<u32> { elems(s).into_iter().map(value).collect() };
            assert_eq!(text("Straße"), [0x53, 0x74, 0x72, 0x61, 5, 0x73, 0x65]);
            assert_eq!(text("cœur"), [0x63, 2, 0x65, 0x75, 0x72]);
            assert_eq!(text("Œdipe"), [1, 0x45, 0x64, 0x69, 0x70, 0x65]);
            assert_eq!(text("æble"), [4, 0x65, 0x62, 0x6C, 0x65]);
            assert_eq!(text("Ærø"), [3, 0x45, 0x72, 0xF8]);
            assert_eq!(text("ĳs"), [0x69, 0x6A, 0x73]);
            assert!(
                t2.chunks(2).skip(2).all(|c| {
                    let v = u16::from_be_bytes([c[0], c[1]]);
                    !matches!(v, 0xDF | 0xE6 | 0xC6 | 0x153 | 0x152 | 0x133 | 0x132)
                }),
                "no symbol for the one-character letters"
            );
            let sym = |c: char| elems(&c.to_string())[0];
            let marker = elems("Straße")[4];
            assert_eq!(weight(marker), weight(sym('s')), "the ß marker weighs as s");
            assert_eq!(weight(elems("æble")[0]), weight(sym('a')));
            assert_eq!(weight(elems("cœur")[1]), weight(sym('o')));
            let k = |s: &str| o.sort_key(&o.encode_label(s));
            assert_eq!(k("Straße"), k("Strasse"));
            assert!(k("Fusion") < k("Straße"));
        }
    }

    #[test]
    fn label_texts_spell_out_the_two_letter_forms() {
        assert_eq!(stored_label_text("Fuß"), "Fu\u{5}s");
        assert_eq!(stored_label_text("ŒUVRE"), "\u{1}EUVRE");
        assert_eq!(stored_label_text("ĲSLAND"), "IJSLAND");
        assert_eq!(stored_label_text("a\u{3}b"), "ab");
        // A marker already followed by its second letter is that letter, as a
        // dictionary unpacked from a Kindle file spells it.
        assert_eq!(stored_label_text("Fu\u{5}s"), "Fu\u{5}s");
        assert_eq!(stored_label_text("\u{1}Euvre"), "\u{1}Euvre");
        assert_eq!(stored_label_text("B\u{2}euf"), "B\u{2}euf");
        assert_eq!(stored_label_text("C\u{4}esar"), "C\u{4}esar");
        // The wrong second letter: Œ is followed by E, not e.
        assert_eq!(stored_label_text("\u{1}euvre"), "euvre");
        assert_eq!(stored_label_text("Fu\u{5}\u{5}s"), "Fu\u{5}s");
        // A marker weighs as its letter, so the stored form sorts spelled out.
        let k = |s: &str| {
            let units: Vec<u16> = stored_label_text(s).encode_utf16().collect();
            device_collation_key(&units)
        };
        assert_eq!(k("Fuß"), k("Fuss"));
        assert_eq!(k("Ærø"), k("aerø"));
        assert_eq!(k("ĲSLAND"), k("ijsland"));
        assert!(k("Fusion") < k("Fuß") && k("Fuß") < k("Futter"));
    }

    /// The weight of every BMP code unit, written out as the classes the
    /// Kindle 4 and the Paperwhite 4 were measured to give them, so a moved
    /// range end or a letter filed under the wrong base fails. The Hangul jamo
    /// are left out: the devices give them weights of their own, which are not
    /// modeled (see [`is_unmodeled_hangul_jamo`]).
    #[test]
    fn device_weight_classes() {
        const NOTHING: &[(u16, u16)] = &[
            (0x0000, 0x0000),
            (0x0006, 0x000F),
            (0x0015, 0x001F),
            (0x0021, 0x002F),
            (0x003A, 0x0040),
            (0x005B, 0x0060),
            (0x007B, 0x009F),
            (0x00A1, 0x00B1),
            (0x00B4, 0x00B4),
            (0x00B6, 0x00B8),
            (0x00BA, 0x00BF),
            (0x00C6, 0x00C6),
            (0x00D7, 0x00D7),
            (0x00DF, 0x00DF),
            (0x00E6, 0x00E6),
            (0x00F7, 0x00F7),
            (0x0132, 0x0133),
            (0x0152, 0x0153),
            (0x0184, 0x0185),
            (0x018D, 0x018D),
            (0x0195, 0x0196),
            (0x019B, 0x019B),
            (0x01A2, 0x01A3),
            (0x01A7, 0x01AA),
            (0x01B1, 0x01B1),
            (0x01B7, 0x01CC),
            (0x01E2, 0x01E3),
            (0x01EE, 0x01EF),
            (0x01F1, 0x01F3),
            (0x01F6, 0x01F9),
            (0x01FD, 0x01FD),
            (0x0218, 0x024F),
            (0x0251, 0x0252),
            (0x025B, 0x025E),
            (0x0263, 0x0264),
            (0x0269, 0x0269),
            (0x026E, 0x026E),
            (0x0276, 0x0278),
            (0x0283, 0x0283),
            (0x0285, 0x0286),
            (0x028A, 0x028A),
            (0x0292, 0x0296),
            (0x0298, 0x0298),
            (0x029A, 0x029A),
            (0x02A1, 0x02FF),
            (0x115A, 0x1160),
            (0x11A3, 0x11A7),
            (0x2000, 0x206F),
            (0x2190, 0x23FF),
            (0x2500, 0x27FF),
            (0x2900, 0x2BFF),
            (0x3001, 0x3040),
            (0x3099, 0x309E),
            (0x30A0, 0x30A0),
            (0x30FB, 0x30FF),
            (0x3164, 0x3164),
            (0xAC00, 0xD7A3),
            (0xFF00, 0xFF0F),
            (0xFF1A, 0xFF20),
            (0xFF3B, 0xFF40),
            (0xFF5B, 0xFF65),
            (0xFF70, 0xFF70),
            (0xFF9E, 0xFF9F),
        ];
        // Each base letter with the characters that weigh as it; "X-Y" is a
        // range.
        const LETTERS: &[(char, &str)] = &[
            ('a', "A a À-Å à-å Ā-ą Ǎǎ Ǟ-ǡ Ǻ-Ǽ Ȁ-ȃ ɐ \u{3} \u{4}"),
            ('b', "B b ƀ-ƃ ɓ ʙ"),
            ('c', "C c Ç ç Ć-č Ƈƈ ɕ ʗ"),
            ('d', "D d Ð ð Ď-đ Ɖ-ƌ ɖɗ"),
            ('e', "E e È-Ë è-ë Ē-ě Ǝ-Ɛ ǝ Ȅ-ȇ ɘ-ɚ"),
            ('f', "F f Ƒƒ"),
            ('g', "G g Ĝ-ģ ƓƔ Ǥ-ǧ Ǵǵ ɠ-ɢ ʛ"),
            ('h', "H h Ĥ-ħ ɥ-ɧ ʜ"),
            ('i', "I i Ì-Ï ì-ï Ĩ-ı Ɨ Ǐǐ Ȉ-ȋ ɨ ɪ"),
            ('j', "J j Ĵĵ ǰ ɟ ʄ ʝ"),
            ('k', "K k Ķ-ĸ Ƙƙ Ǩǩ ʞ"),
            ('l', "L l Ĺ-ł ƚ ɫ-ɭ ʟ"),
            ('m', "M m µ Ɯ ɯ-ɱ"),
            ('n', "N n Ñ ñ Ń-ŋ Ɲƞ ɲ-ɴ"),
            (
                'o',
                "O o Ò-Ö Ø ò-ö ø Ō-ő Ɔ Ɵ-ơ Ǒǒ Ǫ-ǭ Ǿǿ Ȍ-ȏ ɔ ɵ \u{1} \u{2}",
            ),
            ('p', "P p Ƥƥ"),
            ('q', "Q q ʠ"),
            ('r', "R r Ŕ-ř Ʀ Ȑ-ȓ ɹ-ʁ"),
            ('s', "S s Ś-š ſ ʂ \u{5}"),
            ('t', "T t Þ þ Ţ-ŧ ƫ-Ʈ ʇʈ"),
            ('u', "U u Ù-Ü ù-ü Ũ-ų Ưư Ǔ-ǜ Ȕ-ȗ ʉ"),
            ('v', "V v Ʋ ʋʌ"),
            ('w', "W w Ŵŵ ʍ"),
            ('x', "X x"),
            ('y', "Y y Ý ý ÿ Ŷ-Ÿ Ƴƴ ʎʏ"),
            ('z', "Z z Ź-ž Ƶƶ ʐʑ"),
        ];
        let mut expected: HashMap<u16, u16> = HashMap::new();
        for &(lo, hi) in NOTHING {
            for unit in lo..=hi {
                expected.insert(unit, 0);
            }
        }
        for &(base, spelled) in LETTERS {
            for token in spelled.split(' ') {
                let chars: Vec<char> = token.chars().collect();
                let units: Vec<u16> = match chars[..] {
                    [lo, '-', hi] => (lo as u16..=hi as u16).collect(),
                    _ => chars.iter().map(|&c| c as u16).collect(),
                };
                for unit in units {
                    assert_eq!(expected.insert(unit, base as u16), None, "U+{unit:04X}");
                }
            }
        }
        for (unit, hiragana) in (0x10..=0x14).zip(['あ', 'い', 'う', 'え', 'お']) {
            expected.insert(unit, hiragana as u16);
        }
        for unit in [0x20, 0xA0, 0x3000] {
            expected.insert(unit, 0x20);
        }
        for (digit, forms) in ('0'..='9').zip(["", "¹", "²", "³", "", "", "", "", "", ""]) {
            for c in forms.chars() {
                expected.insert(c as u16, digit as u16);
            }
            expected.insert(0xFF10 + (digit as u16 - 0x30), digit as u16);
        }
        for (i, letter) in ('a'..='z').enumerate() {
            expected.insert(0xFF21 + i as u16, letter as u16);
            expected.insert(0xFF41 + i as u16, letter as u16);
        }
        let katakana = "ァアィイゥウェエォオカガキギクグケゲコゴサザシジスズセゼソゾタダチヂッツヅテデトドナニヌネノハバパヒビピフブプヘベペホボポマミムメモャヤュユョヨラリルレロヮワヰヱヲン";
        let hiragana = "ぁあぃいぅうぇえぉおかがきぎくぐけげこごさざしじすずせぜそぞただちぢっつづてでとどなにぬねのはばぱひびぴふぶぷへべぺほぼぽまみむめもゃやゅゆょよらりるれろゎわゐゑをん";
        for (k, h) in katakana.chars().zip(hiragana.chars()) {
            expected.insert(k as u16, h as u16);
        }
        for (c, h) in "ヴヵヶヷヸヹヺゔゕゖ\u{3097}\u{3098}ゟ"
            .chars()
            .zip("うかけわゐゑをうあいうえお".chars())
        {
            expected.insert(c as u16, h as u16);
        }
        let halfwidth = "ｦｧｨｩｪｫｬｭｮｯｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘﾙﾚﾛﾜﾝ";
        let full = "をぁぃぅぇぉゃゅょっあいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわん";
        for (c, h) in halfwidth.chars().zip(full.chars()) {
            expected.insert(c as u16, h as u16);
        }
        for unit in 0..=0xFFFFu16 {
            let jamo = (0x1100..=0x11F9).contains(&unit) || (0x3131..=0x318E).contains(&unit);
            if jamo && expected.get(&unit) != Some(&0) {
                continue;
            }
            let want = expected.get(&unit).copied().unwrap_or(unit);
            assert_eq!(device_weight(unit), want, "U+{unit:04X}");
        }
    }

    /// The Latin-1 letters of the SPL1 spelling table in kindlegen's collation
    /// blob fold to the same base letter as [`device_weight`] gives them.
    #[test]
    fn device_weights_agree_with_the_kindlegen_spelling_table() {
        let pairs = crate::indx::amazon_fold_pairs();
        assert!(pairs.len() >= 60);
        for (src, want) in pairs {
            assert_eq!(device_weight(src as u16), want as u16, "{src:?}");
        }
    }

    #[test]
    fn hangul_jamo_are_flagged_as_unmodeled() {
        for unit in [0x1100, 0x1112, 0x11A8, 0x11F9, 0x3131, 0x314F, 0x318E] {
            assert!(is_unmodeled_hangul_jamo(unit), "U+{unit:04X}");
        }
        // The fillers weigh nothing, as modeled; syllables, the extended jamo
        // blocks and neighbors are not flagged.
        for unit in [
            0x115F, 0x1160, 0x3164, 0x10FF, 0x11FA, 0x3130, 0x318F, 0xAC00, 0xA960,
        ] {
            assert!(!is_unmodeled_hangul_jamo(unit), "U+{unit:04X}");
        }
    }

    #[test]
    fn plain_labels_are_stored_like_exact_table_labels() {
        let units = |bytes: Vec<u8>| -> Vec<u16> {
            bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect()
        };
        for word in ["カード", "Straße", "ĳs", "x😀y", "ーカ", "a\u{3}b"] {
            let o = OrdtTables::new_exact(&[word]);
            assert_eq!(
                units(encode_plain_label(word)),
                o.stored_units(&o.encode_label(word)),
                "{word}"
            );
        }
        assert_eq!(
            units(encode_plain_label("カード")),
            [0x30AB, 0x3095, 0x30C9]
        );
        // ヷヸヹヺ weigh as わゐゑを, so a following ー takes their vowel.
        for (word, marker) in [
            ("ヷー", 0x3095),
            ("ヸー", 0x3096),
            ("ヹー", 0x3098),
            ("ヺー", 0x309F),
        ] {
            let first = word.chars().next().unwrap() as u16;
            assert_eq!(units(encode_plain_label(word)), [first, marker], "{word}");
        }
        // Cut to 254 bytes before a pair that does not fit whole.
        for tail in ["ß", "😀", "æ"] {
            let label = format!("{}{tail}", "д".repeat(126));
            let bytes = encode_plain_label(&label);
            assert_eq!(bytes.len(), 252, "{tail}");
            assert_eq!(units(bytes).last(), Some(&0x0434));
        }
        let label = format!("{}ßx", "д".repeat(125));
        assert_eq!(encode_plain_label(&label).len(), 254);
    }

    /// Labels in the order the Kindle 4 and the Paperwhite 4 search them.
    #[test]
    fn device_key_orders_romanian_mixed_scripts_and_supplementary_characters() {
        let k = |s: &str| {
            let units: Vec<u16> = stored_label_text(s).encode_utf16().collect();
            device_collation_key(&units)
        };
        let ordered = |labels: &[&str]| {
            for pair in labels.windows(2) {
                assert!(k(pair[0]) < k(pair[1]), "{:?} < {:?}", pair[0], pair[1]);
            }
        };
        // Romanian ș and ț weigh nothing, so "școală" sorts as "coala" and
        // "țară" as "ară"; ş with a cedilla is an s.
        ordered(&[
            "acasă", "țară", "arc", "școală", "copil", "sat", "şcoală", "stat", "tară",
        ]);
        assert_eq!(k("țară"), k("ară"));
        assert_eq!(k("ȘCOALĂ"), k("coală"));
        // Modifier letters and the skipped Extended-B and IPA letters weigh
        // nothing; ư and ơ weigh as u and o, ſ as s, ŋ as n.
        assert_eq!(k("Hawaiʻi"), k("hawaii"));
        assert_eq!(k("pʰa"), k("pa"));
        assert_eq!(k("ǆep"), k("ep"));
        assert_eq!(k("ʃa"), k("a"));
        assert_eq!(k("ưa"), k("ua"));
        assert_eq!(k("ơi"), k("oi"));
        assert_eq!(k("ſein"), k("sein"));
        assert_eq!(k("ŋa"), k("na"));
        // Latin folds; Greek, Cyrillic, Armenian, Latin Extended Additional
        // and combining marks keep their code units, capitals first.
        ordered(&[
            "apple",
            "café",
            "cafe\u{301}",
            "Zebra",
            "zebu",
            "Ω",
            "α",
            "Москва",
            "москва",
        ]);
        ordered(&["Ա.Մ.Ն.", "ամիս", "ẞa", "Ấn Độ", "ấm"]);
        // A character outside the BMP compares as its surrogates, after
        // U+D7FF and before U+E000.
        ordered(&[
            "x\u{D7FB}y",
            "x😀y",
            "x😁y",
            "x\u{20000}y",
            "x\u{E000}y",
            "xﬁy",
            "x\u{FFFD}y",
        ]);
        // The no-break and ideographic spaces weigh as a space, the thin space
        // nothing; a Hangul syllable nothing, katakana as hiragana; fullwidth
        // letters and digits as ASCII.
        assert_eq!(k("ice\u{A0}cream"), k("ice cream"));
        assert_eq!(k("ice\u{3000}cream"), k("ice cream"));
        assert_eq!(k("ice\u{2009}cream"), k("icecream"));
        assert_eq!(k("a한국b"), k("ab"));
        assert_eq!(k("カラオケ"), k("からおけ"));
        assert_eq!(k("ＡＢＣ１"), k("abc1"));
        assert_eq!(k("m²"), k("m2"));
        assert_ne!(k("m⁴"), k("m4"), "U+2074 keeps its code unit");
        assert!(k("") < k("a"));
    }

    /// The counts that order labels with equal keys.
    #[test]
    fn tie_counts() {
        let units = |s: &str| -> Vec<u16> { stored_label_text(s).encode_utf16().collect() };
        assert_eq!(skipped_units(&units("T-shirt")), [0x2D]);
        assert_eq!(skipped_units(&units("școală")), [0x0219]);
        assert!(
            skipped_units(&units("Straße")).is_empty(),
            "a marker weighs"
        );
        assert_eq!(one_letter_pair_count("Straße"), 1);
        assert_eq!(one_letter_pair_count("ĳsœ"), 2);
        assert_eq!(one_letter_pair_count("Strasse"), 0);
        assert_eq!(one_letter_pair_count("Stra\u{5}se"), 1);
        assert_eq!(one_letter_pair_count("\u{1}euvre"), 0);
    }

    /// The ORDT1 weights an exact table writes give its labels the order of
    /// the device key, in one-byte and two-byte tables, with characters
    /// outside the BMP and above U+E000 in the same dictionary.
    #[test]
    fn exact_ordt_weights_agree_with_the_device_order() {
        let words = [
            "x😀y",
            "xﬁy",
            "x\u{FFFD}y",
            "xy",
            "xay",
            "Ωx",
            "ωx",
            "școală",
            "coala",
            "scoala",
            "Straße",
            "Strasse",
            "Strase",
            "ǽsc",
            "sc",
            "Hawaiʻi",
            "hawaii",
            "ưa",
            "ua",
            "Ấn Độ",
            "ấm",
            "カラオケ",
            "からおけ",
            "a→b",
            "ab",
        ];
        for split in [words.len(), 4] {
            let o = OrdtTables::new_exact(&words[words.len() - split..]);
            // The weights as the file carries them: read back from the
            // serialized ORDT1 block, one or two bytes per symbol, so a weight
            // that does not fit its width fails.
            let (t1, _) = o.serialize();
            let ordt1 = |s: &str| -> Vec<u16> {
                let label = o.encode_label(s);
                let weights: Vec<u16> = if o.ordt_type() == 1 {
                    label
                        .iter()
                        .map(|&b| u16::from(t1[4 + usize::from(b)]))
                        .collect()
                } else {
                    label
                        .chunks_exact(2)
                        .map(|c| {
                            let at = 4 + 2 * usize::from(u16::from_be_bytes([c[0], c[1]]));
                            u16::from_be_bytes([t1[at], t1[at + 1]])
                        })
                        .collect()
                };
                weights.into_iter().filter(|&w| w != 0).collect()
            };
            let device = |s: &str| device_collation_key(&o.stored_units(&o.encode_label(s)));
            for a in &words[words.len() - split..] {
                assert_eq!(
                    String::from_utf16(&o.stored_units(&o.encode_label(a))).unwrap(),
                    stored_label_text(a),
                    "{a} decodes to its stored form"
                );
                for b in &words[words.len() - split..] {
                    assert_eq!(
                        ordt1(a).cmp(&ordt1(b)),
                        device(a).cmp(&device(b)),
                        "{a} vs {b}"
                    );
                }
            }
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
    fn prolonged_mark_folds_onto_preceding_vowel() {
        // The post-0.18.0 report: katakana names with ー did not resolve on
        // device because kindling stored ー as an out-of-table literal. It
        // now folds onto the preceding vowel, like kindlegen.
        let o = OrdtTables::new(&["ローゼマイン", "ヴィルフリート"]);
        assert!(
            !o.sym_of.contains_key(&'ー'),
            "foldable ー is never a symbol"
        );

        // ローゼマイン: ロ ー ゼ マ イ ン — ー after ロ (o) -> U+309F.
        let e = elems(&o, &o.encode_label("ローゼマイン"));
        assert_eq!(e.len(), 6, "ー folds in place; no literal expansion");
        assert!(
            e.iter().all(|&s| (s as u32) < o.count()),
            "all table symbols"
        );
        assert_eq!(o.ordt2[e[1] as usize], 0x309F, "ー after ロ -> o-marker");
        assert_eq!(
            o.ordt1[e[1] as usize], o.ordt1[o.sym_of[&'お'] as usize],
            "the o-marker carries お's weight"
        );

        // ヴィルフリート: ヴ ィ ル フ リ ー ト — ー after リ (i) -> U+3096.
        let e = elems(&o, &o.encode_label("ヴィルフリート"));
        assert_eq!(o.ordt2[e[5] as usize], 0x3096, "ー after リ -> i-marker");
        assert_eq!(o.ordt1[e[5] as usize], o.ordt1[o.sym_of[&'い'] as usize]);
    }

    #[test]
    fn prolonged_fold_collates_as_long_vowel() {
        let o = OrdtTables::new(&["カ", "カー", "カイ", "キー"]);
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert!(k("カ") < k("カー"), "カ is a prefix of カー");
        assert!(k("カー") < k("カイ"), "long-a (あ weight) sorts before イ");
        assert!(k("カー") < k("キー"), "fold by vowel: カー (a) < キー (i)");
    }

    #[test]
    fn vu_collates_as_u() {
        // ヴ (U+30F4) collates as ウ rather than sorting past the kana block,
        // matching kindlegen; this is why ヴィルフリート now lands early.
        let o = OrdtTables::new(&["ア", "ウ", "ヴ", "エ"]);
        assert_eq!(
            o.ordt1[o.sym_of[&'ヴ'] as usize], o.ordt1[o.sym_of[&'ウ'] as usize],
            "ヴ has ウ's weight"
        );
        let k = |s: &str| o.sort_key(&o.encode_label(s));
        assert!(k("ア") < k("ヴ") && k("ヴ") < k("エ"), "あ < ヴ(=う) < え");
    }

    #[test]
    fn unfoldable_prolonged_mark_is_ignorable() {
        // ー with no preceding vowel (word start or after ン) stays as the
        // literal mark with weight 0: kept in the label but ignored in
        // collation, like kindlegen.
        let o = OrdtTables::new(&["ンー", "ン"]);
        assert!(
            o.sym_of.contains_key(&'ー'),
            "unfoldable ー is an in-table symbol"
        );
        assert_eq!(
            o.ordt1[o.sym_of[&'ー'] as usize], 0,
            "ー is collation-ignorable"
        );
        assert_eq!(
            o.sort_key(&o.encode_label("ンー")),
            o.sort_key(&o.encode_label("ン")),
            "trailing ー contributes nothing to the key"
        );
    }

    #[test]
    fn sokuon_and_double_mark_fold() {
        // Small tsu ッ collates as u (it is small つ), so a following ー folds
        // to the u-marker; and consecutive ー keep the first one's vowel.
        let o = OrdtTables::new(&["ッー", "カーー"]);
        let e = elems(&o, &o.encode_label("ッー"));
        assert_eq!(o.ordt2[e[1] as usize], 0x3097, "ー after ッ -> u-marker");
        let e = elems(&o, &o.encode_label("カーー"));
        assert_eq!(o.ordt2[e[1] as usize], 0x3095, "first ー -> a-marker");
        assert_eq!(
            o.ordt2[e[2] as usize], 0x3095,
            "second ー keeps the a-marker"
        );
    }

    #[test]
    fn middle_dot_is_ignorable() {
        // ・ (U+30FB) stays in the label as a weight-0 symbol, like kindlegen,
        // so a name like ナカ・マ collates as ナカマ instead of sorting past
        // every kana (which a high-weight literal would do).
        let o = OrdtTables::new(&["ナカ・マ", "ナカマ"]);
        assert!(!o.two_byte, "・ is an in-table symbol, not a literal");
        assert_eq!(
            o.ordt1[o.sym_of[&'・'] as usize], 0,
            "・ is collation-ignorable"
        );
        assert_eq!(
            o.sort_key(&o.encode_label("ナカ・マ")),
            o.sort_key(&o.encode_label("ナカマ")),
        );
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
            ja.ordt1[ja.sym_of[&'ア'] as usize], ja.ordt1[ja.sym_of[&'あ'] as usize],
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
    fn no_kana_space_headwords_add_space_symbol() {
        // kindlegen's no-kana table gains the space symbol (weight 3) when a
        // headword contains a space: ORDT2 [NUL, %, _, space], ORDT1
        // [0, 0, 0, 3], and the space encodes as symbol 3, not literal 0x20.
        let ko = OrdtTables::new(&["빅 벤", "빅맥"]);
        assert_eq!(ko.count(), 4);
        assert_eq!(ko.ordt2, vec![0x0000, 0x0025, 0x005F, 0x0020]);
        assert_eq!(ko.ordt1, vec![0, 0, 0, SPACE_WEIGHT]);
        let e = elems(&ko, &ko.encode_label("빅 벤"));
        assert_eq!(e, vec![0xBE45, 0x0003, 0xBCA4]);
        // Space stays weighted (not ignorable), so "빅 벤" keeps sorting
        // before "빅맥" exactly as it does with a literal 0x20.
        assert!(ko.sort_key(&ko.encode_label("빅 벤")) < ko.sort_key(&ko.encode_label("빅맥")));
        // Space-free no-kana dictionaries keep the minimal 3-entry table.
        let ko2 = OrdtTables::new(&["빅맥"]);
        assert_eq!(ko2.count(), 3);
    }

    #[test]
    fn astral_literal_encodes_surrogate_pair() {
        // U+200D7 (rare hanja used as a Korean lookup term). kindlegen stores
        // it as the surrogate pair D840 DCD7; dropping it produced an empty
        // label at the head of the index (issue #22).
        let o = OrdtTables::new(&["\u{200D7}", "빅맥"]);
        assert!(o.two_byte);
        let e = elems(&o, &o.encode_label("\u{200D7}"));
        assert_eq!(e, vec![0xD840, 0xDCD7]);
        assert!(
            !o.encode_label("\u{200D7}").is_empty(),
            "astral-only labels must never encode to zero length"
        );
        // Matches the UTF-16BE path byte-for-byte for the same string.
        assert_eq!(
            o.encode_label("\u{200D7}"),
            crate::indx::encode_indx_label("\u{200D7}")
        );
        // Sort key: literal surrogate units, after every BMP Hangul label.
        assert!(o.sort_key(&o.encode_label("빅맥")) < o.sort_key(&o.encode_label("\u{200D7}")));
    }

    #[test]
    fn long_labels_kept_to_entry_cap() {
        // 17-char Korean phrase labels are stored in full by kindlegen; the
        // old 30-byte cap truncated at 15 chars and the entry could then
        // never exact-match its own headword (issue #22).
        let long = "소비에트 사회주의 공화국 연방";
        let n = long.chars().count();
        assert!(n > 15, "must exceed the old 15-char truncation point");
        let o = OrdtTables::new(&[long]);
        let e = elems(&o, &o.encode_label(long));
        assert_eq!(e.len(), n, "no truncation below the 254-byte entry cap");
        // Truncation still applies at 254 bytes (127 two-byte elements), at a
        // character boundary.
        let very_long: String = "가".repeat(200);
        let bytes = o.encode_label(&very_long);
        assert_eq!(bytes.len(), 254);
        // A surrogate pair is never split at the cap: 126 BMP chars (252
        // bytes) followed by an astral char (4 bytes needed, 2 remaining).
        let mut tail = "나".repeat(126);
        tail.push('\u{200D7}');
        let t = OrdtTables::new(&[tail.as_str()]);
        let b = t.encode_label(&tail);
        assert_eq!(b.len(), 252, "pair dropped whole, not split");
    }

    #[test]
    fn serialize_layout() {
        // Two-byte table (a literal forces ordt_type=0): ORDT1 and ORDT2 are
        // both 2 bytes/symbol.
        let o = OrdtTables::new(&["山"]);
        assert_eq!(o.ordt_type(), 0);
        let (t1, t2) = o.serialize();
        assert_eq!(&t1[..4], b"ORDT");
        assert_eq!(&t2[..4], b"ORDT");
        assert_eq!(t1.len(), 4 + o.count() as usize * 2);
        assert_eq!(t2.len(), 4 + o.count() as usize * 2);
        assert_eq!(&t2[4..6], &[0, 0]); // symbol 0 is NUL
    }

    #[test]
    fn serialize_ordt1_width_tracks_ordt_type() {
        // Issue #13: ORDT1 must be 1 byte/symbol for ordt_type=1 and 2
        // bytes/symbol for ordt_type=0, matching what the firmware and
        // KindleUnpack read back. Writing a fixed 2 bytes for the one-byte
        // path scrambled collation (zero high bytes were read as weights).

        // Pure-kana, small table -> ordt_type=1 -> ORDT1 is 1 byte/symbol.
        let one = OrdtTables::new(&["あい", "かき"]);
        assert_eq!(one.ordt_type(), 1);
        let (t1, t2) = one.serialize();
        assert_eq!(&t1[..4], b"ORDT");
        assert_eq!(t1.len(), 4 + one.count() as usize, "ORDT1 is 1 byte/symbol");
        assert_eq!(
            t2.len(),
            4 + one.count() as usize * 2,
            "ORDT2 stays 2 bytes"
        );
        // Each weight survives the one-byte cast and lands at byte N.
        for (i, &w) in one.ordt1.iter().enumerate() {
            assert!(w <= 0xFF, "weights fit in a byte");
            assert_eq!(t1[4 + i], w as u8);
        }

        // A literal forces ordt_type=0 -> ORDT1 is 2 bytes/symbol.
        let two = OrdtTables::new(&["山", "あ"]);
        assert_eq!(two.ordt_type(), 0);
        let (t1, _) = two.serialize();
        assert_eq!(
            t1.len(),
            4 + two.count() as usize * 2,
            "ORDT1 is 2 bytes/symbol"
        );
    }

    #[test]
    fn generated_ordt_language_gate() {
        assert!(uses_generated_ordt("ja"));
        assert!(uses_generated_ordt("ja-JP"));
        assert!(uses_generated_ordt("zh"));
        assert!(uses_generated_ordt("ko"));
        assert!(uses_generated_ordt("ar"));
        // Arabic-script languages share the all-literal `ar` path.
        for code in ["fa", "ur", "ps", "ug", "sd", "ckb", "fa-IR", "ur_PK"] {
            assert!(
                uses_generated_ordt(code),
                "{code} should use generated ORDT"
            );
        }
        assert!(!uses_generated_ordt("el"));
        assert!(!uses_generated_ordt("en"));
        // Latin Kurdish (Kurmanji) and other non-Arabic codes stay off the
        // generated path; only the Arabic-script Kurdish code ckb is on it.
        assert!(!uses_generated_ordt("ku"));
        assert!(!uses_generated_ordt("kmr"));
        assert!(!uses_generated_ordt("jam"));
    }
}
