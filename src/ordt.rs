//! Generated ORDT collation tables for dictionary indexes (per-character).
//!
//! Kindle firmware resolves dictionary lookups by encoding the tapped word
//! one **character** at a time and binary-searching the orth INDX, whose
//! entry labels must be encoded the same way. The mapping is defined by a
//! pair of ORDT tables embedded in the primary INDX record. See issue #11.
//!
//! kindling routes Japanese, Chinese, Korean, and the Arabic-script
//! languages (`ar`, `fa`, `ur`, `ps`, `ug`, `sd`, `ckb`) through this module
//! (see `uses_generated_ordt`). Latin dictionaries default to a full
//! per-letter ORDT (see `new_exact`) that folds accent/case collation weights
//! while keeping each character a distinct symbol, for exact-accent matching
//! (issue #8); `--fold-accents` gives them the diacritic-folding Greek
//! ORDT/SPL blob instead. Greek and Cyrillic keep the UTF-16BE label scheme.
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
fn kana_vowel(cp: u32) -> Option<usize> {
    // ヴ→ゔ, ヵ→ゕ, ヶ→ゖ, ン→ん all land inside the hiragana range.
    let h = if (KATAKANA_FIRST..=KATAKANA_LAST).contains(&cp) {
        cp - 0x60
    } else {
        cp
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
fn label_codepoints(text: &str) -> Vec<u32> {
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
    /// True when labels are two-byte BE elements (`ordt_type` 0); false
    /// for single-byte elements (`ordt_type` 1).
    two_byte: bool,
}

/// Fold a character to its base letter for accent-strict collation weights:
/// lowercase it and strip Latin diacritics, so every accent/case variant of a
/// letter (é/è/ê/ë/É → e, à/â/ä/À → a, ç/Ç → c, ...) collates to one weight.
/// Used by [`OrdtTables::new_exact`]; characters with no Latin base (digits,
/// punctuation, non-Latin scripts) fold only by case. See issue #8.
fn fold_base(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => 'a',
        'ç' | 'ć' | 'č' | 'ċ' | 'ĉ' => 'c',
        'ð' | 'ď' | 'đ' => 'd',
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => 'e',
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => 'g',
        'ĥ' | 'ħ' => 'h',
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => 'i',
        'ĵ' => 'j',
        'ķ' => 'k',
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => 'l',
        'ñ' | 'ń' | 'ņ' | 'ň' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => 'o',
        'ŕ' | 'ŗ' | 'ř' => 'r',
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => 's',
        'ţ' | 'ť' | 'ŧ' | 'ț' => 't',
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => 'u',
        'ŵ' => 'w',
        'ý' | 'ÿ' | 'ŷ' => 'y',
        'ź' | 'ż' | 'ž' => 'z',
        other => other,
    }
}

/// Folded collation key for a Latin headword label: each character mapped
/// through `fold_base` (case + Latin diacritic folding). Latin dictionaries
/// must be sorted in this order, not raw UTF-16BE byte order, so the
/// firmware's folding collation can find accent-initial headwords. Under raw
/// byte order `świat` (ś = U+015B) sorts after `z`, where the firmware's
/// Polish search never looks, so the lookup misses (issue #8). This mirrors
/// the folded weights `new_exact` assigns, so the exact and fold paths order
/// labels the same way.
pub(crate) fn folded_sort_key(label: &str) -> Vec<char> {
    label.chars().map(fold_base).collect()
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
            two_byte,
        }
    }

    /// Build a full accent-strict ORDT for a Latin/Greek/Cyrillic dictionary
    /// (the `--strict-accents` path).
    ///
    /// Every character the dictionary uses becomes its own ORDT symbol so the
    /// encoded labels keep ê distinct from e, but accented and case variants
    /// of a letter share a single collation WEIGHT (é=è=ê=ë=e, à=â=ä=a, A=a,
    /// ...), exactly like kindlegen's Latin dictionary ORDT. This is the
    /// counter-intuitive key (issue #8): the Kindle firmware does not collate a
    /// Latin/French dictionary with the dict's own per-letter weights - it
    /// binary-searches with its built-in accent+case-folding French collation
    /// and requires the stored headword labels to be PRE-SORTED in that folded
    /// order. Folding the weights makes accented headwords sort adjacent to
    /// their base form (so the firmware's folded search lands in the right
    /// neighborhood), while the distinct symbols let the final byte comparison
    /// pick the exact headword. Assigning every character a DISTINCT weight (an
    /// earlier attempt) scattered accented headwords far from their base, so
    /// the folded search mis-landed `meme`->`même` and never reached `mère`.
    pub fn new_exact(labels: &[&str]) -> OrdtTables {
        let mut ordt1: Vec<u16> = Vec::new();
        let mut ordt2: Vec<u16> = Vec::new();
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

        // Fixed seed: NUL, %, _ (kindlegen always emits these as ignorable
        // weight-0 symbols).
        add(0x0000, 0, &mut ordt1, &mut ordt2, &mut sym_of);
        add(0x0025, 0, &mut ordt1, &mut ordt2, &mut sym_of); // %
        add(0x005F, 0, &mut ordt1, &mut ordt2, &mut sym_of); // _

        let mut chars: Vec<char> = labels.iter().flat_map(|s| s.chars()).collect();
        chars.sort_unstable();
        chars.dedup();

        // Assign each fold-base letter a collation weight in alphabetical
        // order; every accent/case variant of that letter shares the weight.
        let mut bases: Vec<char> = chars.iter().map(|&c| fold_base(c)).collect();
        bases.sort_unstable();
        bases.dedup();
        let mut base_weight: HashMap<char, u16> = HashMap::new();
        for (i, b) in bases.iter().enumerate() {
            base_weight.insert(*b, (i as u16).saturating_add(1));
        }

        // Each unique BMP character becomes its own symbol carrying its
        // fold-base's weight. Code points above U+FFFF cannot index the u16
        // symbol table and are left out (none occur in accented Latin).
        for c in chars {
            let cp = c as u32;
            if cp <= 0xFFFF && !sym_of.contains_key(&c) {
                let weight = *base_weight.get(&fold_base(c)).unwrap_or(&0);
                add(cp, weight, &mut ordt1, &mut ordt2, &mut sym_of);
            }
        }

        // Single-byte labels (ordt_type = 1, like kindlegen) when the table
        // fits a byte index; every label character is in the table, so there
        // are no literals to force two-byte elements unless the table itself
        // exceeds 256 symbols.
        let two_byte = ordt2.len() > 256;

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
    /// for table characters and the raw code point for literals. The
    /// prolonged sound mark ー is folded onto the preceding vowel first (see
    /// `label_codepoints`). Truncated to the 5-bit length limit at a
    /// character boundary.
    pub fn encode_label(&self, text: &str) -> Vec<u8> {
        let max_bytes = if self.two_byte {
            MAX_LABEL_BYTES_2
        } else {
            MAX_LABEL_BYTES_1
        };
        let elem_bytes = if self.two_byte { 2 } else { 1 };
        let mut out: Vec<u8> = Vec::new();
        for cp in label_codepoints(text) {
            if out.len() + elem_bytes > max_bytes {
                break;
            }
            match char::from_u32(cp).and_then(|c| self.sym_of.get(&c)) {
                Some(&sym) => self.push_elem(&mut out, sym),
                None => {
                    // Out-of-table literal code point. Representable only in
                    // two-byte labels (a one-byte table is built only when no
                    // literals are present); BMP only, and skipped if it would
                    // collide with the symbol-index range.
                    if self.two_byte && cp <= 0xFFFF && cp >= self.count() {
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
        // That folded-weight, distinct-symbol layout is what makes the
        // firmware match accents exactly on device.
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
