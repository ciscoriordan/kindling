//! Dictionary lookup simulator: what a Kindle opens when a word is tapped.
//!
//! Given a built dictionary MOBI and a tapped word, this predicts which entry
//! the Paperwhite 5 (firmware 5.19.2) opens, or that it opens nothing. It
//! follows the steps the Kindle 4 (firmware 4.1.4), the Paperwhite 4
//! (firmware 5.16.5) and the Paperwhite 5 take, as measured on each:
//!
//! 1. The word is formatted for the language of the book it was tapped in
//!    (see [`format`]): a curly apostrophe becomes a straight one, leading and
//!    trailing digits and punctuation go, and Italian and French elided
//!    words, the English `'s` and French and Portuguese clitics come off.
//!    A word that formats to nothing is not looked up. The simulator takes
//!    the book language to be the dictionary's input language. The Paperwhite
//!    4 differs from the Paperwhite 5 only in this step, for the few
//!    characters whose letter properties changed between the Unicode versions
//!    the two follow.
//! 2. On the Paperwhite 4 and 5, a word looked up in a Russian dictionary is
//!    lowercased (see [`lowercase_for_russian`]). No other language is, and
//!    the Kindle 4 lowercases nothing.
//! 3. `ß` is searched as `ss`, `æ` and `Æ` as `ae`, `œ` and `Œ` as `oe`, and a
//!    prolonged sound mark as the vowel of the last kana before it that has
//!    one (see [`searched_units`]).
//! 4. The word and every stored label are weighed character by character
//!    ([`crate::ordt::device_weight`]): Latin letters fold case and accents,
//!    punctuation and some letters weigh nothing, and Greek, Cyrillic and
//!    most other scripts weigh as they are. Only the tables that turn stored
//!    bytes into characters are read from the file; its collation weights are
//!    not used. A Mobipocket Creator index is the exception: its label bytes
//!    are weighed through the byte weight table it carries, and the word is
//!    written in cp1252 to be weighed the same way (see [`CreatorOrder`]).
//! 5. The index is binary-searched in its stored order, first over the label
//!    that ends each index record and then within the record, and the search
//!    returns the whole run of adjacent labels that weigh the same as the
//!    word, across record boundaries. Nothing is matched by prefix. A label
//!    stored where the order does not put it can be missed, and so can the
//!    labels the search steps over on its way, and a label that weighs the
//!    same but is not adjacent to the run is not returned; the report names
//!    such labels.
//! 6. One label of the run is opened: the only one; else the first spelled
//!    exactly like the formatted word; else the first equal to it under the
//!    book language's collation rules ([`collator`]), which ignore case but
//!    not accents or punctuation, and in German books compare case too; else
//!    the longest label (a trailing "..." removed) equal to the word's
//!    beginning; else the first label that begins with the word; else the
//!    first label of the run. Matching the whole word, and the word against
//!    the beginning of a longer label, compares sort keys; matching the
//!    beginning of the word against a shorter label compares the two
//!    directly.
//!
//! Firmware 5.19.6 differs in step 6: it compares by the default collation
//! rules whatever the book's language, so in a Polish book `WYKŁADNIĘ` opens
//! `wykładnie` rather than `wykładnię`, in a Turkish book `ABIŞLERINDE`
//! opens `abişlerinde` rather than `abislerinde`, and in a Russian dictionary
//! `Абду-Салам` opens `абдусалам` rather than `абду-салам`. The unit tests
//! simulate it; [`report`] and the CLI simulate the Paperwhite 5.
//!
//! Not modeled: the Japanese dictionary form and raw-word attempt, and the
//! retries with up to three trailing characters removed, that the Paperwhite
//! 4 and 5 apply in Japanese, Chinese and Korean books; the weights of the
//! Hangul jamo; the collation rules beyond those [`collator`] lists; which
//! dictionary a device picks for a book; that the Kindle 4 opens nothing in a
//! dictionary whose header declares no input language, where the Paperwhite 4
//! and 5 look words up all the same; and a separate inflection index, which
//! kindling does not write. Only the unit tests simulate a Kindle 4 lookup,
//! with its narrower letter test at the edges of a word, its stop words that
//! show no popup, and no Russian lowercasing. A dictionary build uses the first
//! two to choose the spellings it adds as aliases for the Kindle 4 (see
//! [`kindle_spellings`]).
//!
//! Finding the index. The MOBI header names the orth index record at offset
//! 0x18, but that pointer cannot be trusted on files kindling did not write:
//! a record number that was not adjusted for records inserted ahead of it
//! lands on something else entirely, and every query then misses with nothing
//! to say why (issue #49). So the index is found by its own signature too
//! (see [`orth_index_name`]). A Kindle does not look past the header: in a
//! file whose header names another record, or none, it opens nothing. Such a
//! lookup has no result, and [`report`] says which record the index is in,
//! what the header claimed and what the index would open, so the cause is
//! visible rather than silent.

mod collator;
mod format;

use std::cmp::Ordering;
use std::ops::Range;

use crate::huffcdic::COMPRESSION_HUFFDIC;
use crate::ordt::{
    cp1252_char, device_collation_key, device_weight, expansion_char, expansion_of, kana_vowel,
};
use collator::Collator;

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
/// has no dictionary index at all, one whose index pointer is wrong, and one
/// whose labels are stored out of order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LookupReport {
    /// The label opened, when the query resolved.
    pub result: Option<LookupResult>,
    /// What the search picks in the index that was found, when a Kindle does
    /// not search that index at all because the MOBI header names another
    /// record or none. `result` is then `None`, and this says what the index
    /// would open once the header is fixed.
    pub would_open: Option<LookupResult>,
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
    /// The first few headwords in index order, decoded.
    ///
    /// A miss on a dictionary that plainly contains the word is nearly always
    /// one of two things, and these tell them apart at a glance: if the
    /// labels come back as mojibake then the label bytes were decoded wrong
    /// and no query could ever match, and if they read as ordinary words then
    /// the index is fine and the search is where to look.
    pub first_labels: Vec<String>,
    /// The stored labels either side of where the search ended, decoded.
    ///
    /// Says where the query landed in the stored order. A word that is missing
    /// from a run of otherwise-sensible neighbors is a different problem from
    /// one whose neighbors are also absent.
    pub nearest: Vec<String>,
    /// The language the word was formatted and the labels compared for: the
    /// dictionary's input language, as a code such as `it`.
    pub book_language: String,
    /// The word as the Kindle looks it up, after formatting. Empty when it
    /// formats to nothing, and then nothing is looked up.
    pub formatted: String,
    /// The word the index is searched for: the formatted word, lowercased in
    /// a Russian dictionary (see [`lowercase_for_russian`]).
    pub searched: String,
    /// Every label the index search returned, in stored order. The opened
    /// label is one of them.
    pub run: Vec<String>,
    /// Labels that weigh the same as the searched word but that the search
    /// does not return, because the index is not stored in the order of the
    /// Kindle's weights, whether these labels or others are the misplaced
    /// ones.
    pub unreachable: Vec<String>,
    /// How many adjacent pairs of stored labels are in the wrong order for the
    /// Kindle's weights. Any at all can hide labels, though which ones only a
    /// search for each label shows.
    pub order_breaks: usize,
    /// How many stored labels contain a Hangul jamo whose weight is not
    /// modeled (see [`crate::ordt::is_unmodeled_hangul_jamo`]). When there are
    /// any, a device can miss those labels and labels near them, whatever the
    /// result says.
    pub hangul_jamo_labels: usize,
    /// Set when the Kindle 4 shows no popup for the formatted word. The
    /// Paperwhite 4 and 5 have no such words, so this is always false in
    /// [`report`].
    pub(crate) stop_word: bool,
}

impl LookupReport {
    /// Whether the orth index sits somewhere other than where the MOBI header
    /// says it does, or the header names no index at all. True for a file
    /// whose index pointer was not adjusted for records inserted ahead of it.
    /// A Kindle opens nothing in such a file.
    pub fn index_pointer_is_stale(&self) -> bool {
        match self.index_record {
            Some(used) => self.declared_index_record != Some(used as u32),
            None => false,
        }
    }

    /// Whether a Kindle leaves the index that was found unsearched, because
    /// the header does not name it.
    pub fn kindle_skips_index(&self) -> bool {
        self.index_record.is_some() && self.index_pointer_is_stale()
    }

    /// Whether the text records use HUFF/CDIC compression.
    pub fn is_huffdic(&self) -> bool {
        self.compression == COMPRESSION_HUFFDIC
    }
}

/// Which Kindle to simulate. The CLI and [`report`] simulate the Paperwhite 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Device {
    /// The Kindle 4, firmware 4.1.4. Only the unit tests simulate it; a
    /// dictionary build also adds the spellings it rewrites words to.
    Kindle4,
    /// The Paperwhite 5, firmware 5.19.2. The Paperwhite 4 (firmware 5.16.5)
    /// looks words up the same way, except for the few characters the
    /// [`format`] module doc lists.
    Paperwhite5,
    /// Firmware 5.19.6, as on the Paperwhite 12th generation, which picks
    /// among the labels by the default collation rules in every language.
    /// Only the unit tests simulate it.
    #[cfg_attr(not(test), allow(dead_code))]
    Firmware5_19_6,
}

/// One stored label, as a Kindle reads it.
struct Label {
    /// The UTF-16 code units the stored bytes stand for.
    units: Vec<u16>,
    /// [`device_collation_key`] of `units`.
    key: Vec<u16>,
    /// The text position the entry points at.
    position: u32,
}

struct OrthIndex {
    /// Every label, in stored order across the data records.
    labels: Vec<Label>,
    /// How a Mobipocket Creator index weighs its label bytes, when it is one.
    creator: Option<CreatorOrder>,
    /// Per data record: the key of its routing label in the primary record,
    /// and the range of `labels` it holds.
    records: Vec<(Vec<u16>, Range<usize>)>,
    /// PalmDB record number the primary INDX was read from.
    record: usize,
    /// The primary language id of the dictionary's input language, the low
    /// byte of the Windows LCID in the MOBI header.
    input_language: u8,
}

/// The order of a Mobipocket Creator index, whose labels are cp1252 or UTF-8
/// bytes. A Kindle weighs each stored byte through the 256-entry byte weight
/// table the index carries (an `ORDT` block at the offset in header field
/// 0x28), and compares the bytes as they are when there is no table. The word
/// is written in cp1252 first, the non-breaking hyphen U+2011 as `-` and any
/// other character cp1252 lacks as a space, and each byte the index's `LIGT`
/// block lists (Creator lists `Œ`, `œ`, `Æ`, `æ` and `ß`) is replaced by the
/// two bytes given for it. So the weights of [`crate::ordt::device_weight`]
/// play no part in such an index: under Creator's own table `€`, `™`, the
/// no-break space and `¹²³` weigh nothing, and "Dvořák" is searched as
/// "Dvo ák", which "Dvorak" is not.
struct CreatorOrder {
    weights: Option<Vec<u8>>,
    ligatures: Vec<(u8, [u8; 2])>,
}

impl CreatorOrder {
    fn read(primary: &[u8]) -> Self {
        let block = |field: usize, magic: &[u8], len: usize| {
            let at = u32_be(primary, field)
                .map(|v| v as usize)
                .filter(|&v| v > 0)?;
            (primary.get(at..at + 4)? == magic)
                .then(|| primary.get(at + 4..at + 4 + len))
                .flatten()
        };
        let count = u32_be(primary, 0x30).unwrap_or(0) as usize;
        let ligatures = block(0x2C, b"LIGT", count.min(256) * 4)
            .map(|entries| {
                entries
                    .chunks_exact(4)
                    .map(|e| (e[0], [e[2], e[3]]))
                    .collect()
            })
            .unwrap_or_default();
        CreatorOrder {
            weights: block(0x28, b"ORDT", 256).map(<[u8]>::to_vec),
            ligatures,
        }
    }

    /// The weights of stored bytes, the bytes that weigh nothing left out.
    fn key(&self, bytes: &[u8]) -> Vec<u16> {
        bytes
            .iter()
            .map(|&b| self.weights.as_ref().map_or(b, |t| t[usize::from(b)]))
            .filter(|&w| w != 0)
            .map(u16::from)
            .collect()
    }

    /// The key of a searched word, written in cp1252 as a Kindle writes it.
    fn word_key(&self, word: &[u16]) -> Vec<u16> {
        let mut bytes = Vec::with_capacity(word.len() + 2);
        for c in char::decode_utf16(word.iter().copied()) {
            let byte = match c {
                Ok('\u{2011}') => b'-',
                Ok(c) => (0..=255u8).find(|&b| cp1252_char(b) == c).unwrap_or(b' '),
                Err(_) => b' ',
            };
            match self.ligatures.iter().find(|(from, _)| *from == byte) {
                Some((_, pair)) => bytes.extend_from_slice(pair),
                None => bytes.push(byte),
            }
        }
        self.key(&bytes)
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

/// How the label bytes of one orth index are stored.
struct LabelCoding<'a> {
    encoding: u32,
    /// The ORDT2 values, when the labels are ORDT symbol sequences.
    ordt2: Option<&'a [u8]>,
    two_byte: bool,
}

impl LabelCoding<'_> {
    /// The UTF-16 code units a label's bytes stand for, as a Kindle reads
    /// them. In a one-byte table every byte is a symbol; in a two-byte table a
    /// big-endian unit below the table size is a symbol and any other unit is
    /// itself. Plain labels are UTF-16BE, except Mobipocket Creator's, which
    /// are cp1252 or UTF-8 (read as UTF-16, "keep" came back as "步数").
    fn units(&self, bytes: &[u8]) -> Vec<u16> {
        let symbol =
            |ordt2: &[u8], value: u16| u16_be(ordt2, usize::from(value) * 2).unwrap_or(value);
        match (self.ordt2, self.encoding) {
            (Some(ordt2), _) if !self.two_byte => {
                bytes.iter().map(|&b| symbol(ordt2, u16::from(b))).collect()
            }
            (Some(ordt2), _) => bytes
                .chunks_exact(2)
                .map(|c| symbol(ordt2, u16::from_be_bytes([c[0], c[1]])))
                .collect(),
            (None, CP1252_INDEX_ENCODING) => bytes
                .iter()
                .flat_map(|&b| {
                    let mut buf = [0u16; 2];
                    cp1252_char(b).encode_utf16(&mut buf).to_vec()
                })
                .collect(),
            (None, UTF8_INDEX_ENCODING) => String::from_utf8_lossy(bytes).encode_utf16().collect(),
            (None, _) => bytes
                .chunks_exact(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect(),
        }
    }
}

/// A label as a Kindle shows it, and compares it when it picks one label of
/// several: a marker value (see [`expansion_char`]) followed by its second
/// letter is the one letter `ß`, `æ`, `Æ`, `œ` or `Œ`, in one-byte and
/// two-byte tables and in plain UTF-16 labels alike. The markers a stored
/// label writes for a prolonged sound mark after a vowel, U+3095 to U+3098
/// and U+309F, show as the mark `ー` itself, and U+0010 to U+0014 as U+FF70.
/// A marker not followed by its second letter stays as it is.
fn title_units(units: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if let Some(letter) = expansion_char(u32::from(unit)) {
            let (_, _, second) = expansion_of(letter).expect("an expansion letter");
            if units.get(i + 1).copied() == Some(second as u16) {
                out.push(letter as u16);
                i += 2;
                continue;
            }
        }
        out.push(match unit {
            0x10..=0x14 => 0xFF70,
            0x3095..=0x3098 | 0x309F => 0x30FC,
            _ => unit,
        });
        i += 1;
    }
    out
}

/// A label or word for display, with any unpaired surrogate replaced.
fn text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// How many control bytes each entry carries, from the TAGX that follows the
/// primary's header. One when there is no TAGX there to ask.
fn tagx_control_bytes(primary: &[u8], header_len: usize) -> usize {
    if primary.get(header_len..header_len + 4) != Some(b"TAGX".as_slice()) {
        return 1;
    }
    u32_be(primary, header_len + 8)
        .map(|n| n as usize)
        .filter(|n| (1..=8).contains(n))
        .unwrap_or(1)
}

/// Whether an orth primary's labels are ORDT symbol sequences: it names an
/// ORDT2 table and the table is really there. An SPL fold blob beside it
/// changes nothing about how the labels are stored.
fn labels_are_ordt(primary: &[u8], oentries: u32, ordt2_off: usize) -> bool {
    oentries > 0
        && ordt2_off > 0
        && primary.get(ordt2_off..ordt2_off + 4) == Some(b"ORDT".as_slice())
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

/// The label encodings Mobipocket Creator wrote before kindlegen existed:
/// cp1252, one byte per character, or UTF-8. Its orth primaries carry no
/// index name either, so the encoding is the only thing that marks them.
const CP1252_INDEX_ENCODING: u32 = 1252;
const UTF8_INDEX_ENCODING: u32 = 65001;

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
fn declares_orth_encoding(
    data: &[u8],
    recs: &[(usize, usize)],
    idx: usize,
    file_is_dictionary: bool,
) -> bool {
    let (s, e) = match recs.get(idx) {
        Some(r) => *r,
        None => return false,
    };
    let rec = &data[s..e];
    if rec.get(0..4) != Some(b"INDX".as_slice())
        || u32_be(rec, 8) != Some(0)
        || u32_be(rec, 12) != Some(0)
    {
        return false;
    }
    match u32_be(rec, 28) {
        Some(ORTH_INDEX_ENCODING) => true,
        // cp1252 is what Mobipocket Creator's orth primaries declare, and it
        // is also something an ordinary index can declare, so it counts only
        // in a file whose header says it is a dictionary, which Creator's
        // always do. UTF-8 does not count at all: it is what kindlegen writes
        // for a book's table of contents, and accepting it turned a book's
        // three chapter entries into a dictionary of "0", "1" and "2".
        Some(CP1252_INDEX_ENCODING) => file_is_dictionary,
        _ => false,
    }
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
        .filter(|&i| declares_orth_encoding(data, recs, i, declared.is_some()))
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

/// The entries of one INDX record's IDXT, as byte slices of the record.
fn idxt_entries(rec: &[u8]) -> Vec<&[u8]> {
    let (Some(idxt_off), Some(count)) = (u32_be(rec, 20), u32_be(rec, 24)) else {
        return Vec::new();
    };
    let (idxt_off, count) = (idxt_off as usize, count as usize);
    if rec.get(idxt_off..idxt_off + 4) != Some(b"IDXT".as_slice()) {
        return Vec::new();
    }
    let mut offs: Vec<usize> = (0..count)
        .map_while(|i| u16_be(rec, idxt_off + 4 + i * 2).map(usize::from))
        .collect();
    offs.push(idxt_off);
    offs.windows(2)
        .filter(|w| w[0] < w[1] && w[1] <= rec.len())
        .map(|w| &rec[w[0]..w[1]])
        .collect()
}

/// Parse the orth index of a dictionary MOBI: decode every label and its text
/// position, in stored order, and the routing label of each data record.
fn parse_orth_index(data: &[u8]) -> Option<OrthIndex> {
    let recs = palmdb_records(data)?;
    let (r0s, r0e) = *recs.first()?;
    let rec0 = &data[r0s..r0e];
    let declared = u32_be(rec0, 40).filter(|&v| v != u32::MAX);
    let orth_idx = find_orth_primary(data, &recs, declared)?;
    let (ps, pe) = recs[orth_idx];
    let primary = &data[ps..pe];

    let num_data = u32_be(primary, 24)? as usize;
    let header_len = u32_be(primary, 4).unwrap_or(0) as usize;
    let encoding = u32_be(primary, 28).unwrap_or(ORTH_INDEX_ENCODING);
    // The ORDT fields sit at 164..180. Mobipocket Creator wrote headers as
    // short as 164 bytes, where those offsets already hold its TAGX, so they
    // are only read from a header long enough to contain them.
    let (oentries, ordt_type, ordt2_off) = if header_len >= 180 {
        (
            u32_be(primary, 168).unwrap_or(0),
            u32_be(primary, 164).unwrap_or(0), // 0 = two-byte, 1 = one-byte
            u32_be(primary, 176).unwrap_or(0) as usize,
        )
    } else {
        (0, 0, 0)
    };
    // Control bytes per entry, from the TAGX after the header. kindling and
    // kindlegen always write one; Mobipocket Creator wrote two once an index
    // carried enough tags, and assuming one read every text position from
    // the wrong byte.
    let control_bytes = tagx_control_bytes(primary, header_len);

    // Labels are ORDT symbol sequences whenever the primary carries a real
    // ORDT2 table, whether or not it also carries an SPL fold blob.
    //
    // This used to require the fold blob to be absent, to keep kindling's own
    // Greek dictionaries (fold blob plus a 7-entry seed table, UTF-16BE labels)
    // from being read as ORDT. That protected nothing: every Greek code point
    // is far above 7, so it is a literal and decodes to itself either way. And
    // it broke every production kindlegen dictionary, which carries both. A
    // 174685-headword German one resolved no query at all, because its labels
    // were read as UTF-16 and came out as the raw symbol numbers (issue #49).
    //
    // The ORDT2 table is written as its 4-byte "ORDT" magic followed by
    // `oentries` big-endian u16 values (see OrdtTables::serialize); the header
    // offset points at the magic, so the values start 4 bytes later.
    let ordt2 = labels_are_ordt(primary, oentries, ordt2_off).then(|| {
        let start = ordt2_off + 4;
        let end = (start + oentries as usize * 2).min(primary.len());
        &primary[start..end]
    });
    let creator = (ordt2.is_none()
        && matches!(encoding, CP1252_INDEX_ENCODING | UTF8_INDEX_ENCODING))
    .then(|| CreatorOrder::read(primary));
    let label_key = |bytes: &[u8], units: &[u16]| match &creator {
        Some(order) => order.key(bytes),
        None => device_collation_key(units),
    };
    let coding = LabelCoding {
        encoding,
        ordt2,
        two_byte: ordt_type == 0,
    };

    let mut labels: Vec<Label> = Vec::new();
    let mut ranges: Vec<Range<usize>> = Vec::new();
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
        // A truncated leaf costs its own entries and no more.
        let start = labels.len();
        for entry in idxt_entries(rec) {
            let label_len = usize::from(entry[0]);
            if 1 + label_len >= entry.len() {
                continue;
            }
            let bytes = &entry[1..1 + label_len];
            let units = coding.units(bytes);
            // First tag value after the control bytes is the text position.
            let position = read_vwi_inv(entry, 1 + label_len + control_bytes).unwrap_or(0);
            labels.push(Label {
                key: label_key(bytes, &units),
                units,
                position,
            });
        }
        ranges.push(start..labels.len());
    }

    // The routing label of each data record is the label in the primary's
    // own entry for it, which kindling and kindlegen make the record's last
    // label. Where the primary carries no such entries (Mobipocket Creator's
    // do not), the last label of the record stands in.
    let routing: Vec<Vec<u16>> = idxt_entries(primary)
        .into_iter()
        .filter_map(|entry| {
            let len = usize::from(*entry.first()?);
            entry
                .get(1..1 + len)
                .map(|bytes| label_key(bytes, &coding.units(bytes)))
        })
        .collect();
    let records = ranges
        .into_iter()
        .enumerate()
        .map(|(i, range)| {
            let key = match routing.get(i) {
                Some(key) if routing.len() == num_data => key.clone(),
                _ => range
                    .clone()
                    .last()
                    .map(|last| labels[last].key.clone())
                    .unwrap_or_default(),
            };
            (key, range)
        })
        .collect();

    Some(OrthIndex {
        labels,
        creator,
        records,
        record: orth_idx,
        input_language: u32_be(rec0, 0x60).map_or(0, |v| (v & 0xFF) as u8),
    })
}

/// A language code for the primary language id of a Windows LCID, for the
/// languages whose lookup rules differ and the others kindling writes.
fn language_code(id: u8) -> &'static str {
    match id {
        0x01 => "ar",
        0x02 => "bg",
        0x03 => "ca",
        0x04 => "zh",
        0x05 => "cs",
        0x06 => "da",
        0x07 => "de",
        0x08 => "el",
        0x09 => "en",
        0x0A => "es",
        0x0B => "fi",
        0x0C => "fr",
        0x0D => "he",
        0x0E => "hu",
        0x0F => "is",
        0x10 => "it",
        0x11 => "ja",
        0x12 => "ko",
        0x13 => "nl",
        0x14 => "nb",
        0x15 => "pl",
        0x16 => "pt",
        0x18 => "ro",
        0x19 => "ru",
        0x1A => "hr",
        0x1B => "sk",
        0x1C => "sq",
        0x1D => "sv",
        0x1E => "th",
        0x1F => "tr",
        0x20 => "ur",
        0x21 => "id",
        0x22 => "uk",
        0x23 => "be",
        0x24 => "sl",
        0x25 => "et",
        0x26 => "lv",
        0x27 => "lt",
        0x28 => "tg",
        0x29 => "fa",
        0x2A => "vi",
        0x2B => "hy",
        0x2D => "eu",
        0x2F => "mk",
        _ => "",
    }
}

/// A spelling a Kindle looks a word up by (see [`kindle_spellings`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KindleSpelling {
    pub(crate) text: String,
    /// Whether only the Kindle 4 (firmware 4.1.4) looks the word up this way,
    /// and not the Paperwhite 5 (firmware 5.19.2).
    pub(crate) kindle4_only: bool,
}

/// The spellings the Paperwhite 5 (firmware 5.19.2) and the Kindle 4
/// (firmware 4.1.4) look `word` up by, when they differ from `word`: tapped in
/// a book in the language with the Windows primary language id `language_id`,
/// and, on the Paperwhite 5, in a book in a language with no rules of its own
/// (see [`format`]). The Paperwhite 5's spellings come first. An empty
/// spelling means nothing would be looked up. A rewrite that leaves half of a
/// surrogate pair is left out, and so is a Kindle 4 spelling for which the
/// Kindle 4 shows no popup.
pub(crate) fn kindle_spellings(word: &str, language_id: u8) -> Vec<KindleSpelling> {
    let units: Vec<u16> = word.encode_utf16().collect();
    // A word made only of letters is looked up as written: every rule starts
    // from a character that is not a letter. The Kindle 4 counts fewer
    // characters as letters than the Paperwhite 5 does.
    if units.iter().all(|&u| format::is_letter(u, Device::Kindle4)) {
        return Vec::new();
    }
    let language = language_code(language_id);
    let mut book_languages = vec![language];
    if format::has_language_rules(language) {
        book_languages.push("");
    }
    let mut spellings: Vec<KindleSpelling> = Vec::new();
    for device in [Device::Paperwhite5, Device::Kindle4] {
        let kindle4_only = device == Device::Kindle4;
        // The Kindle 4 looks a word up only in a dictionary of the book's
        // language.
        let languages = if kindle4_only {
            &book_languages[..1]
        } else {
            &book_languages[..]
        };
        for &book_language in languages {
            let spelling = format::format_word(&units, book_language, device);
            if spelling == units
                || (kindle4_only && format::kindle4_stop_word(&spelling, book_language))
            {
                continue;
            }
            if let Ok(text) = String::from_utf16(&spelling) {
                if !spellings.iter().any(|s| s.text == text) {
                    spellings.push(KindleSpelling { text, kindle4_only });
                }
            }
        }
    }
    spellings
}

/// Whether the Paperwhite 5 looks up the stored label `units`, tapped in a
/// book in the language with the Windows primary language id `language_id`,
/// by a spelling other than the label (see [`format`]): "-aren" as "aren",
/// "A.I." as "A.I", and in an English book "people's" as "people". Such a
/// label never equals the word it is looked up by.
pub(crate) fn kindle_rewrites_label(units: &[u16], language_id: u8) -> bool {
    let title = title_units(units);
    format::format_word(&title, language_code(language_id), Device::Paperwhite5) != title
}

/// The primary language id of Russian.
const RUSSIAN: u8 = 0x19;

/// The word a Kindle searches the index for: the formatted word, lowercased
/// when `lowercase` (the Paperwhite 4 and 5 in a Russian dictionary).
fn searched_word(formatted: &[u16], lowercase: bool) -> Vec<u16> {
    match String::from_utf16(formatted) {
        Ok(word) if lowercase => lowercase_for_russian(&word).encode_utf16().collect(),
        _ => formatted.to_vec(),
    }
}

/// `word` lowercased the way the Paperwhite 4 and 5 lowercase a word they
/// look up in a Russian dictionary. Every character lowers as in Unicode
/// except `Σ`, which becomes `ς` only at the end of a word part: a cased
/// letter comes before it in the part and none after. A part is a stretch of
/// letters and digits, joined by combining marks, by invisible format
/// characters such as U+200B, by one hyphen, dash, connector, soft hyphen,
/// apostrophe, quotation mark, period or U+2027 between two letters, by one
/// apostrophe, quotation mark, period, comma or U+066B between two digits,
/// and by a danda between a letter and a digit. Kana and ideographs are not
/// letters here. So `ΑΣ-Α`, `ΑΣ1Α` and `ΑΣ.Α` keep `σ`, while `ΑΣ`, `ΑΣ-`,
/// `ΑΣ:Α`, `ΑΣ,Α` and `ΑΣ Α` take `ς`, which is not Unicode's own rule.
pub(crate) fn lowercase_for_russian(word: &str) -> String {
    // Only `Σ` lowers by a rule of its own, so a word without it lowers as in
    // Unicode, which is what `str::to_lowercase` does for every other letter.
    if !word.contains('Σ') {
        return word.to_lowercase();
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        Letter,
        Digit,
        Mark,
        Invisible,
        Other,
    }
    let kind = |c: char| match c {
        '\u{600}'..='\u{604}'
        | '\u{6DD}'
        | '\u{70F}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{206A}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}' => Kind::Invisible,
        '\u{3005}'
        | '\u{3041}'..='\u{30FE}'
        | '\u{4E00}'..='\u{9FA5}'
        | '\u{F900}'..='\u{FA2D}' => Kind::Other,
        '\u{488}' | '\u{489}' | '\u{20DD}'..='\u{20E0}' | '\u{20E2}'..='\u{20E4}' => Kind::Mark,
        _ if u16::try_from(u32::from(c)).is_ok_and(format::has_combining_class) => Kind::Mark,
        _ if c.is_numeric() => Kind::Digit,
        '\u{24B6}'..='\u{24E9}' => Kind::Other,
        _ if c.is_alphabetic() => Kind::Letter,
        _ => Kind::Other,
    };
    let joins_letters = |c: char| {
        matches!(
            c,
            '-' | '_'
                | '"'
                | '\''
                | '.'
                | '\u{AD}'
                | '\u{58A}'
                | '\u{5BE}'
                | '\u{1400}'
                | '\u{1806}'
                | '\u{2010}'..='\u{2015}'
                | '\u{2027}'
                | '\u{203F}'
                | '\u{2040}'
                | '\u{2054}'
                | '\u{2E17}'
                | '\u{2E1A}'
                | '\u{2E3A}'
                | '\u{2E3B}'
                | '\u{301C}'
                | '\u{3030}'
                | '\u{30A0}'
                | '\u{FE31}'..='\u{FE34}'
                | '\u{FE4D}'..='\u{FE4F}'
                | '\u{FE58}'
                | '\u{FE63}'
                | '\u{FF0D}'
                | '\u{FF3F}'
        )
    };
    let joins_digits = |c: char| matches!(c, '"' | '\'' | '.' | ',' | '\u{66B}');
    // Superscript and subscript letters and the ordinal indicators `ª` and
    // `º` are not cased letters here, and the titlecase letters are.
    let cased = |c: char| {
        let uncased = matches!(
            c,
            '\u{AA}'
                | '\u{BA}'
                | '\u{10FC}'
                | '\u{1D62}'..='\u{1D6A}'
                | '\u{1D78}'
                | '\u{1D9B}'..='\u{1DBF}'
                | '\u{2071}'
                | '\u{207F}'
                | '\u{2090}'..='\u{209C}'
                | '\u{2C7C}'
                | '\u{2C7D}'
                | '\u{A69C}'
                | '\u{A69D}'
                | '\u{A770}'
                | '\u{A7F2}'..='\u{A7F4}'
                | '\u{A7F8}'
                | '\u{A7F9}'
                | '\u{AB5C}'..='\u{AB5F}'
                | '\u{AB69}'
        );
        let titlecase = matches!(
            c,
            '\u{1C5}'
                | '\u{1C8}'
                | '\u{1CB}'
                | '\u{1F2}'
                | '\u{1F88}'..='\u{1F8F}'
                | '\u{1F98}'..='\u{1F9F}'
                | '\u{1FA8}'..='\u{1FAF}'
                | '\u{1FBC}'
                | '\u{1FCC}'
                | '\u{1FFC}'
        );
        ((c.is_lowercase() || c.is_uppercase()) && !uncased) || titlecase
    };

    let chars: Vec<char> = word.chars().collect();
    // The word parts, as the range of `chars` each covers, in order.
    let mut parts: Vec<Range<usize>> = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start + 1;
        let mut last = kind(chars[start]);
        if matches!(last, Kind::Letter | Kind::Digit) {
            for i in start + 1..chars.len() {
                let here = kind(chars[i]);
                let next = chars[i + 1..]
                    .iter()
                    .map(|&c| kind(c))
                    .find(|&k| k != Kind::Invisible);
                let joined = match here {
                    Kind::Letter | Kind::Digit | Kind::Invisible => true,
                    Kind::Mark => last != Kind::Other,
                    Kind::Other
                        if last == Kind::Letter && matches!(chars[i], '\u{964}' | '\u{965}') =>
                    {
                        next == Some(Kind::Digit)
                    }
                    Kind::Other if last == Kind::Letter && joins_letters(chars[i]) => {
                        next == Some(Kind::Letter)
                    }
                    Kind::Other if last == Kind::Digit && joins_digits(chars[i]) => {
                        next == Some(Kind::Digit)
                    }
                    Kind::Other => false,
                };
                if !joined {
                    break;
                }
                if !matches!(here, Kind::Invisible | Kind::Mark) {
                    last = here;
                }
                end = i + 1;
            }
        }
        parts.push(start..end);
        start = end;
    }

    let mut out = String::with_capacity(word.len());
    for part in parts {
        for i in part.clone() {
            let c = chars[i];
            if c == 'Σ' {
                let before = chars[part.start..i].iter().any(|&c| cased(c));
                let after = chars[i + 1..part.end].iter().any(|&c| cased(c));
                out.push(if before && !after { 'ς' } else { 'σ' });
            } else {
                out.extend(c.to_lowercase());
            }
        }
    }
    out
}

/// The UTF-16 units a Kindle weighs for a searched word: U+0000 left out,
/// `ß`, `æ`, `Æ`, `œ` and `Œ` written as their marker and second letter, and a
/// prolonged sound mark `ー` or `ｰ` written as the vowel marker of the last
/// kana before it that has a vowel (see [`carried_vowel`]), however many other
/// characters come between. A mark with no such kana before it stays as it is
/// and weighs nothing.
fn searched_units(word: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(word.len() + 2);
    let mut vowel = None;
    for &unit in word {
        let expansion = char::from_u32(u32::from(unit)).and_then(expansion_of);
        match (unit, vowel, expansion) {
            (0, _, _) => {}
            (0x30FC, Some(v), _) => out.push(VOWEL_MARKERS[v]),
            (0xFF70, Some(v), _) => out.push(HALFWIDTH_VOWEL_MARKERS[v]),
            (_, _, Some((marker, _, second))) => out.extend([marker as u16, second as u16]),
            _ => out.push(unit),
        }
        vowel = carried_vowel(unit).or(vowel);
    }
    out
}

/// The markers a prolonged sound mark `ー` after the vowels a, i, u, e and o
/// is stored and searched as, which weigh as `あ`, `い`, `う`, `え` and `お`.
const VOWEL_MARKERS: [u16; 5] = [0x3095, 0x3096, 0x3097, 0x3098, 0x309F];

/// The markers a halfwidth prolonged sound mark `ｰ` is searched as, which
/// weigh as the same vowels.
const HALFWIDTH_VOWEL_MARKERS: [u16; 5] = [0x10, 0x11, 0x12, 0x13, 0x14];

/// The vowel (0 to 4 for a, i, u, e, o) a kana gives a later prolonged sound
/// mark in a searched word: hiragana `ぁ` to `ゔ`, katakana `ァ` to `ヺ` and
/// halfwidth katakana `ｦ` to `ﾜ`, by the hiragana each weighs as. `ん`, `ン`,
/// `ﾝ`, `ゕ` and `ゖ` give none, and neither does anything else, so they leave
/// the vowel of an earlier kana in place: "カンー" is searched as "カンア".
fn carried_vowel(unit: u16) -> Option<usize> {
    match unit {
        0x3041..=0x3094 | 0x30A1..=0x30FA | 0xFF66..=0xFF6F | 0xFF71..=0xFF9D => {
            kana_vowel(u32::from(device_weight(unit)))
        }
        _ => None,
    }
}

/// A Kindle's binary search over `n` sorted items, where `compare(i)` says
/// how the searched word compares with item `i`. Returns `None` for no items,
/// and otherwise where the search ends: the index of an equal item, or of the
/// item it stopped at, or `n` past the last.
fn bisect(n: usize, compare: impl Fn(usize) -> Ordering) -> Option<usize> {
    if n == 0 {
        return None;
    }
    let (mut lo, mut hi) = (0, n - 1);
    let mut mid = (lo + hi) / 2;
    loop {
        match compare(mid) {
            Ordering::Equal => return Some(mid),
            Ordering::Less => hi = mid,
            Ordering::Greater => lo = mid,
        }
        mid = (lo + hi) / 2;
        if mid == lo || mid == hi {
            if lo == 0 {
                if compare(0) != Ordering::Greater {
                    return Some(0);
                }
            } else if hi == n - 1 && compare(n - 1) == Ordering::Greater {
                return Some(n);
            }
            return Some(hi);
        }
    }
}

impl OrthIndex {
    /// The labels a Kindle's search returns for `key`, as a range of
    /// `self.labels`, and the position in `self.labels` where the search
    /// ended either way.
    fn search(&self, key: &[u16]) -> (Option<Range<usize>>, usize) {
        let Some(m) = bisect(self.records.len(), |i| key.cmp(&self.records[i].0)) else {
            return (None, 0);
        };
        let range = self.records[m.min(self.records.len() - 1)].1.clone();
        let Some(e) = bisect(range.len(), |i| key.cmp(&self.labels[range.start + i].key)) else {
            return (None, range.start);
        };
        let found = range.start + e;
        if e >= range.len() || self.labels[found].key != key {
            return (None, found);
        }
        let mut first = found;
        while first > 0 && self.labels[first - 1].key == self.labels[first].key {
            first -= 1;
        }
        let mut end = found + 1;
        while end < self.labels.len() && self.labels[end].key == self.labels[end - 1].key {
            end += 1;
        }
        (Some(first..end), found)
    }

    fn order_breaks(&self) -> usize {
        self.labels
            .windows(2)
            .filter(|pair| pair[0].key > pair[1].key)
            .count()
    }
}

/// The label a Kindle opens among `titles`, the run a search returned, for
/// the formatted word `word`. Matching the whole word, and the word against
/// the beginning of a longer label, compares sort keys; matching the
/// beginning of the word against a shorter label compares the two directly.
fn choose(titles: &[Vec<u16>], word: &[u16], collator: &Collator) -> usize {
    if titles.len() == 1 {
        return 0;
    }
    if let Some(i) = titles.iter().position(|t| t == word) {
        return i;
    }
    if let Some(i) = titles.iter().position(|t| collator.keys_equal(word, t)) {
        return i;
    }
    // A label that is nothing but "..." leaves nothing to match, and is
    // never taken as the beginning of the word.
    let mut longest: Option<(usize, usize)> = None;
    for (i, title) in titles.iter().enumerate() {
        let title = title.strip_suffix(&[0x2E, 0x2E, 0x2E]).unwrap_or(title);
        if title.len() < word.len()
            && title.len() > longest.map_or(0, |(len, _)| len)
            && collator.equal(title, &word[..title.len()])
        {
            longest = Some((title.len(), i));
        }
    }
    if let Some((_, i)) = longest {
        return i;
    }
    titles
        .iter()
        .position(|t| t.len() > word.len() && collator.keys_equal(word, &t[..word.len()]))
        .unwrap_or(0)
}

/// Resolve `query` against the dictionary in `mobi`, returning the label the
/// Paperwhite 5 opens and its text position, or `None` if it opens nothing.
pub fn lookup(mobi: &[u8], query: &str) -> Option<LookupResult> {
    report(mobi, query).result
}

/// Resolve `query` as the Paperwhite 5 does, and report what the file looked
/// like while doing it.
///
/// A miss is not one thing: the file may hold no dictionary index, hold one
/// that simply has no matching headword, hold one a Kindle does not search,
/// or hold the headword where the search cannot reach it. The CLI needs to
/// say which, so this returns both the outcome and the shape of the file
/// behind it.
pub fn report(mobi: &[u8], query: &str) -> LookupReport {
    report_on(mobi, query, Device::Paperwhite5, None)
}

/// [`report`] for any device, with the book language given or taken from the
/// dictionary.
fn report_on(
    mobi: &[u8],
    query: &str,
    device: Device,
    book_language: Option<&str>,
) -> LookupReport {
    let (mut out, index) = open(mobi);
    if let Some(index) = &index {
        answer(&mut out, index, query, device, book_language);
    }
    out
}

/// What a lookup reads from `mobi` whatever the word: the fields of the
/// report that describe the file, and the orth index when there is one.
fn open(mobi: &[u8]) -> (LookupReport, Option<OrthIndex>) {
    let mut out = LookupReport {
        result: None,
        would_open: None,
        index_record: None,
        declared_index_record: None,
        entries: 0,
        compression: 0,
        unreadable: false,
        huffdic_error: None,
        first_labels: Vec::new(),
        nearest: Vec::new(),
        book_language: String::new(),
        formatted: String::new(),
        searched: String::new(),
        run: Vec::new(),
        unreachable: Vec::new(),
        order_breaks: 0,
        hangul_jamo_labels: 0,
        stop_word: false,
    };

    let recs = match palmdb_records(mobi) {
        Some(r) if !r.is_empty() => r,
        _ => {
            out.unreadable = true;
            return (out, None);
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

    let Some(index) = parse_orth_index(mobi) else {
        return (out, None);
    };
    out.index_record = Some(index.record);
    out.entries = index.labels.len();
    out.order_breaks = index.order_breaks();
    out.hangul_jamo_labels = index
        .labels
        .iter()
        .filter(|l| {
            l.units
                .iter()
                .any(|&unit| crate::ordt::is_unmodeled_hangul_jamo(unit))
        })
        .count();
    out.first_labels = index
        .labels
        .iter()
        .take(5)
        .map(|l| text(&title_units(&l.units)))
        .collect();
    out.book_language = language_code(index.input_language).to_string();
    (out, Some(index))
}

/// Look `query` up in `index` as `device` does, filling in the fields of
/// `out` that depend on the word.
fn answer(
    out: &mut LookupReport,
    index: &OrthIndex,
    query: &str,
    device: Device,
    book_language: Option<&str>,
) {
    let language = book_language.unwrap_or_else(|| language_code(index.input_language));
    out.book_language = language.to_string();

    let word: Vec<u16> = query.encode_utf16().collect();
    let formatted = format::format_word(&word, language, device);
    out.formatted = text(&formatted);
    if formatted.is_empty() {
        return;
    }
    let lowercase = device != Device::Kindle4 && index.input_language == RUSSIAN;
    let searched = searched_word(&formatted, lowercase);
    out.searched = text(&searched);
    let key = match &index.creator {
        Some(order) => order.word_key(&searched),
        None => device_collation_key(&searched_units(&searched)),
    };
    let (run, landed) = index.search(&key);
    let run = run.unwrap_or(landed..landed);
    let titles: Vec<Vec<u16>> = index.labels[run.clone()]
        .iter()
        .map(|l| title_units(&l.units))
        .collect();
    out.run = titles.iter().map(|t| text(t)).collect();
    out.unreachable = index
        .labels
        .iter()
        .enumerate()
        .filter(|(i, l)| l.key == key && !run.contains(i))
        .map(|(_, l)| text(&title_units(&l.units)))
        .collect();
    out.nearest = index.labels[landed.saturating_sub(2)..(landed + 2).min(index.labels.len())]
        .iter()
        .map(|l| text(&title_units(&l.units)))
        .collect();

    if device == Device::Kindle4 && format::kindle4_stop_word(&formatted, language) {
        out.stop_word = true;
        return;
    }
    if !titles.is_empty() {
        let chosen = choose(&titles, &formatted, &Collator::for_choice(language, device));
        let hit = LookupResult {
            matched_label: text(&titles[chosen]),
            position: index.labels[run.start + chosen].position,
        };
        if out.kindle_skips_index() {
            out.would_open = Some(hit);
        } else {
            out.result = Some(hit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(units: &[u16]) -> Vec<u8> {
        units.iter().flat_map(|u| u.to_be_bytes()).collect()
    }

    /// The German production dictionary this was found on carries an SPL
    /// fold blob and a 617-entry ORDT2 table, and its labels are symbol
    /// sequences. Refusing ORDT whenever the fold blob was present made every
    /// label decode to raw symbol numbers and every lookup miss.
    #[test]
    fn a_fold_blob_does_not_stop_labels_being_ordt() {
        let mut primary = vec![0u8; 400];
        primary[56..60].copy_from_slice(&47u32.to_be_bytes()); // SPL count
        primary[300..304].copy_from_slice(b"ORDT");
        assert!(labels_are_ordt(&primary, 617, 300));
        assert!(
            !labels_are_ordt(&primary, 617, 200),
            "no table at that offset"
        );
        assert!(!labels_are_ordt(&primary, 0, 300), "no entries");
    }

    fn decode(bytes: &[u8], ordt2: &[u16], two_byte: bool) -> String {
        let table = be16(ordt2);
        let coding = LabelCoding {
            encoding: ORTH_INDEX_ENCODING,
            ordt2: Some(&table),
            two_byte,
        };
        text(&title_units(&coding.units(bytes)))
    }

    /// A marker and its second letter show as the letter in either label
    /// width, and are searched as two letters; a marker followed by some
    /// other letter stays as it is. kindlegen writes ß as a marker plus a
    /// collation tail, and "Straße" must come back as six letters.
    #[test]
    fn markers_show_as_their_letter() {
        let ordt2 = [0u16, 0x46, 0x75, 0x0005, 0x73, 0x61];
        assert_eq!(decode(&[1, 2, 3, 4], &ordt2, false), "Fuß");
        assert_eq!(text(&title_units(&[0x46, 0x75, 5, 0x61])), "Fu\u{5}a");
        // A stored vowel marker shows as the prolonged sound mark it stands
        // for: the label "カード" is stored as カ, U+3095, ド.
        assert_eq!(text(&title_units(&[0x30AB, 0x3095, 0x30C9])), "カード");
        let ordt2 = [0u16, 0x53, 0x74, 0x72, 0x61, 0x0005, 0x73, 0x65];
        assert_eq!(
            decode(&be16(&[1, 2, 3, 4, 5, 6, 7]), &ordt2, true),
            "Straße"
        );
        let ordt2 = [0u16, 0x42, 0x0002, 0x65, 0x75, 0x66];
        assert_eq!(decode(&be16(&[1, 2, 3, 4, 5]), &ordt2, true), "Bœuf");
        let fuss: Vec<u16> = "Fuß".encode_utf16().collect();
        assert_eq!(searched_units(&fuss), [0x46, 0x75, 5, 0x73]);
        let oe: Vec<u16> = "Œdipe æble".encode_utf16().collect();
        assert_eq!(text(&searched_units(&oe)), "\u{1}Edipe \u{4}eble");
    }

    /// A prolonged sound mark takes the vowel of the last kana before it that
    /// has one, across `ん`, other characters and halfwidth kana, and `ｰ`
    /// counts as one too. So `ｹｰｷ` finds `ケーキ`, and `カンー` finds `カンア`.
    #[test]
    fn a_prolonged_sound_mark_takes_the_last_kana_vowel() {
        let units = |s: &str| searched_units(&s.encode_utf16().collect::<Vec<_>>());
        assert_eq!(units("ｹｰｷ"), [0xFF79, 0x13, 0xFF77]);
        assert_eq!(units("カンー"), [0x30AB, 0x30F3, 0x3095]);
        assert_eq!(units("カ乂ｰ"), [0x30AB, 0x4E42, 0x10]);
        assert_eq!(units("カxー"), [0x30AB, 0x78, 0x3095]);
        assert_eq!(units("ヷーー"), [0x30F7, 0x3095, 0x3095]);
        assert_eq!(units("ゕー"), [0x3095, 0x30FC]);
        assert_eq!(units("ーカ"), [0x30FC, 0x30AB]);
        let key = |s: &str| device_collation_key(&units(s));
        assert_eq!(key("ｹｰｷ"), key("けえき"));
        assert_eq!(key("ｶﾞｰﾄ"), key("かあと"));
        assert_eq!(key("ﾝｰﾄ"), key("んと"));
        // Each vowel's marker, full width and halfwidth: after o as well.
        assert_eq!(key("ヺー"), key("ヺお"));
        assert_eq!(key("ヺｰ"), key("ヺお"));
        assert_eq!(key("ｺｰ"), key("こお"));
    }

    /// The Paperwhite 4 and 5 write a capital sigma as final only at the end
    /// of a word part, which hyphens, digits, apostrophes and periods do not
    /// end but colons, spaces and commas between letters do. A comma between
    /// digits and a danda before a digit do not end a part either, and the
    /// ordinal indicators are not cased letters while titlecase letters are.
    #[test]
    fn russian_lowercasing_places_final_sigma_by_word_part() {
        for (word, lowered) in [
            ("ΑΣ", "ας"),
            ("ΑΣΑ", "ασα"),
            ("ΑΣ-Α", "ασ-α"),
            ("ΑΣ\u{2010}Α", "ασ\u{2010}α"),
            ("ΑΣ1Α", "ασ1α"),
            ("ΑΣ_Α", "ασ_α"),
            ("ΑΣ'Α", "ασ'α"),
            ("ΑΣ.Α", "ασ.α"),
            ("ΑΣ\u{AD}Α", "ασ\u{AD}α"),
            ("ΑΣ\u{200B}Α", "ασ\u{200B}α"),
            ("ΑΣ:Α", "ας:α"),
            ("ΑΣ,Α", "ας,α"),
            ("ΑΣ Α", "ας α"),
            ("ΑΣ-", "ας-"),
            ("ΑΣ-1", "ας-1"),
            ("Α-Σ", "α-ς"),
            ("Σ-Α", "σ-α"),
            ("ΑΣ\u{301}", "ας\u{301}"),
            ("ΑΣ1,1Α", "ασ1,1α"),
            ("ΑΣ\u{964}Α", "ας\u{964}α"),
            ("ªΣ", "ªσ"),
            ("ǅΣ", "ǆς"),
            ("ИЗБА", "изба"),
            ("İstanbul", "i\u{307}stanbul"),
        ] {
            assert_eq!(lowercase_for_russian(word), lowered, "{word}");
        }
    }

    /// Why the old guard protected nothing. kindling's Greek dictionaries
    /// carry a 7-entry seed table beside their fold blob, and every Greek
    /// code point is far above 7, so each one is a literal and reads back as
    /// itself whether or not the labels are treated as ORDT.
    #[test]
    fn greek_labels_read_the_same_through_a_seed_table() {
        let greek: Vec<u16> = "άνεμος".encode_utf16().collect();
        let seed = [0u16, 0x25, 0x5F, 0x20, 0x21, 0x24, 0x26];
        assert_eq!(decode(&be16(&greek), &seed, true), "άνεμος");
    }

    /// The probe pattern of the search: a word equal to an item lands on it,
    /// a word between two items lands on the later one, a word before the
    /// first lands on the first, and a word after the last lands past it.
    #[test]
    fn bisection_lands_where_the_kindle_lands() {
        let items = [10, 20, 30, 40, 50];
        let find = |q: i32| bisect(items.len(), |i| q.cmp(&items[i]));
        assert_eq!(find(30), Some(2));
        assert_eq!(find(35), Some(3));
        assert_eq!(find(5), Some(0));
        assert_eq!(find(15), Some(1));
        assert_eq!(find(45), Some(4));
        assert_eq!(find(55), Some(5));
        assert_eq!(bisect(0, |_| Ordering::Equal), None);
        assert_eq!(bisect(1, |_| Ordering::Greater), Some(0));
    }

    fn palmdb(records: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0u8; 78];
        out[76..78].copy_from_slice(&(records.len() as u16).to_be_bytes());
        let mut at = 78 + records.len() * 8 + 2;
        for r in records {
            out.extend_from_slice(&(at as u32).to_be_bytes());
            out.extend_from_slice(&[0u8; 4]);
            at += r.len();
        }
        out.extend_from_slice(&[0, 0]);
        for r in records {
            out.extend_from_slice(r);
        }
        out
    }

    fn inverted_vwi(mut v: u32) -> Vec<u8> {
        let mut bytes = vec![(v & 0x7F) as u8 | 0x80];
        v >>= 7;
        while v > 0 {
            bytes.insert(0, (v & 0x7F) as u8);
            v >>= 7;
        }
        bytes
    }

    fn with_idxt(mut record: Vec<u8>, entries: &[Vec<u8>]) -> Vec<u8> {
        let mut offs = Vec::new();
        for entry in entries {
            offs.push(record.len() as u16);
            record.extend_from_slice(entry);
        }
        let idxt = record.len() as u32;
        record[20..24].copy_from_slice(&idxt.to_be_bytes());
        record[24..28].copy_from_slice(&(entries.len() as u32).to_be_bytes());
        record.extend_from_slice(b"IDXT");
        for o in offs {
            record.extend_from_slice(&o.to_be_bytes());
        }
        record
    }

    /// A dictionary whose labels are stored exactly in the order given, as
    /// UTF-16BE, split into data records of the given sizes, with the input
    /// language `lcid`. Text positions are 100 times the label's number.
    fn utf16_dictionary(labels: &[&str], record_sizes: &[usize], lcid: u32) -> Vec<u8> {
        let mut rec0 = vec![0u8; 256];
        rec0[0..2].copy_from_slice(&1u16.to_be_bytes());
        rec0[40..44].copy_from_slice(&1u32.to_be_bytes());
        rec0[0x60..0x64].copy_from_slice(&lcid.to_be_bytes());

        let label_bytes = |l: &str| be16(&l.encode_utf16().collect::<Vec<_>>());
        let mut data_records = Vec::new();
        let mut routing = Vec::new();
        let mut next = 0;
        for &size in record_sizes {
            let chunk = &labels[next..next + size];
            let entries: Vec<Vec<u8>> = chunk
                .iter()
                .enumerate()
                .map(|(k, l)| {
                    let bytes = label_bytes(l);
                    let mut e = vec![bytes.len() as u8];
                    e.extend_from_slice(&bytes);
                    e.push(0x01);
                    e.extend(inverted_vwi(100 * (next + k) as u32));
                    e
                })
                .collect();
            let mut leaf = vec![0u8; 192];
            leaf[0..4].copy_from_slice(b"INDX");
            leaf[4..8].copy_from_slice(&192u32.to_be_bytes());
            leaf[12..16].copy_from_slice(&1u32.to_be_bytes());
            data_records.push(with_idxt(leaf, &entries));
            let last = label_bytes(chunk[size - 1]);
            let mut r = vec![last.len() as u8];
            r.extend_from_slice(&last);
            r.extend_from_slice(&(size as u16).to_be_bytes());
            routing.push(r);
            next += size;
        }
        let mut primary = vec![0u8; 192];
        primary[0..4].copy_from_slice(b"INDX");
        primary[4..8].copy_from_slice(&192u32.to_be_bytes());
        primary[28..32].copy_from_slice(&ORTH_INDEX_ENCODING.to_be_bytes());
        primary[36..40].copy_from_slice(&(labels.len() as u32).to_be_bytes());
        let mut primary = with_idxt(primary, &routing);
        primary[24..28].copy_from_slice(&(record_sizes.len() as u32).to_be_bytes());
        let mut records = vec![rec0, primary];
        records.extend(data_records);
        palmdb(&records)
    }

    fn opened(report: &LookupReport) -> Option<&str> {
        report.result.as_ref().map(|r| r.matched_label.as_str())
    }

    /// An index stored out of order, as older kindling builds wrote reader.dict
    /// Italian: the punctuated "post-" labels sorted as if the hyphen were a
    /// low letter, so they sit before "posta", whose key "posta" is smaller
    /// than theirs. The search for "posta" stops among them, and the report
    /// names the label it cannot reach.
    #[test]
    fn an_index_out_of_order_hides_labels_and_says_so() {
        let labels = [
            "possedibile",
            "possidenti",
            "possono",
            "post-",
            "post-prandiale",
            "post-verità",
            "posta",
            "postare",
            "posteggiarono",
            "potanti",
        ];
        let data = utf16_dictionary(&labels, &[5, 5], 0x10);
        let r = report(&data, "posta");
        assert_eq!(r.result, None);
        assert_eq!(r.unreachable, ["posta"]);
        assert!(r.order_breaks > 0);
        assert_eq!(opened(&report(&data, "potanti")), Some("potanti"));

        let mut sorted = labels;
        sorted.sort_by_key(|l| device_collation_key(&l.encode_utf16().collect::<Vec<_>>()));
        let data = utf16_dictionary(&sorted, &[5, 5], 0x10);
        let r = report(&data, "posta");
        assert_eq!(opened(&r), Some("posta"));
        assert!(r.unreachable.is_empty());
        assert_eq!(r.order_breaks, 0);
    }

    /// A label that weighs the same as the word but is stored away from its
    /// run is not returned, and the run itself crosses record boundaries.
    #[test]
    fn the_run_is_adjacent_labels_across_records() {
        let labels = ["ab", "a-b", "a.b", "a'b", "b", "c", "a+b"];
        let data = utf16_dictionary(&labels, &[2, 3, 2], 0x09);
        let r = report(&data, "A.B");
        assert_eq!(r.run, ["ab", "a-b", "a.b", "a'b"]);
        assert_eq!(r.unreachable, ["a+b"]);
        assert_eq!(opened(&r), Some("a.b"));
        assert_eq!(opened(&report(&data, "AB")), Some("ab"));
    }

    /// A run that spans three records, where the search lands in the middle
    /// one, reaches back into the first: a label spelled like the word opens
    /// wherever the run begins.
    #[test]
    fn the_run_reaches_back_into_earlier_records() {
        let labels = ["a-b", "a.b", "a'b", "a,b", "a;b", "ab"];
        let data = utf16_dictionary(&labels, &[2, 2, 2], 0x09);
        let r = report(&data, "a-b");
        assert_eq!(r.run, labels);
        assert_eq!(opened(&r), Some("a-b"));
    }

    /// The entry choice among a run, by the book's language.
    #[test]
    fn the_choice_follows_the_book_language() {
        let data = utf16_dictionary(&["email", "E-mail"], &[2], 0x10);
        let opens = |q: &str, lang: &str| {
            opened(&report_on(&data, q, Device::Paperwhite5, Some(lang))).map(str::to_string)
        };
        assert_eq!(opens("E-MAIL", "it").as_deref(), Some("E-mail"));
        assert_eq!(opens("E-MAIL", "de").as_deref(), Some("email"));
        assert_eq!(opens("EMAIL", "it").as_deref(), Some("email"));
        let data = utf16_dictionary(&["cant", "can't"], &[2], 0x10);
        let opens = |q: &str, lang: &str| {
            opened(&report_on(&data, q, Device::Paperwhite5, Some(lang))).map(str::to_string)
        };
        assert_eq!(opens("CAN'T", "it").as_deref(), Some("can't"));
        assert_eq!(opens("CAN'T", "de").as_deref(), Some("cant"));
        assert_eq!(opens("CAN'T", "da").as_deref(), Some("cant"));

        let titles = |words: &[&str]| -> Vec<Vec<u16>> {
            words.iter().map(|w| w.encode_utf16().collect()).collect()
        };
        let pick = |words: &[&str], w: &str| {
            let w: Vec<u16> = w.encode_utf16().collect();
            words[choose(
                &titles(words),
                &w,
                &Collator::for_choice("en", Device::Paperwhite5),
            )]
            .to_string()
        };
        let run = ["abc", "a-bc", "abc-", "abc..", "abc..."];
        assert_eq!(pick(&run, "a-bcș"), "a-bc");
        assert_eq!(pick(&run, "A-BC"), "a-bc");
        assert_eq!(pick(&["-dromo", "dromo-"], "DROMO"), "dromo-");
        assert_eq!(pick(&["x-yz", "xyz..."], "xyzș"), "xyz...");
        assert_eq!(pick(&["x-yz", "xyz..."], "xyz"), "xyz...");
        assert_eq!(pick(&["ÅR", "år"], "AR"), "ÅR");
        // Of labels equally long that each equal the beginning of the word,
        // the first opens, whatever its case.
        assert_eq!(pick(&["ABC", "abc"], "abcș"), "ABC");
        assert_eq!(pick(&["abc", "ABC"], "abcș"), "abc");
        assert_eq!(pick(&["POLISH", "Polish", "polish"], "PoLiSh"), "POLISH");
    }

    /// A zero-width or control character is left out when the whole word is
    /// matched and when the word is matched against the beginning of a longer
    /// label, even next to a hyphen; matching the beginning of the word
    /// against a shorter label counts it there as a difference.
    #[test]
    fn a_zero_width_character_next_to_a_hyphen() {
        let pick = |lang: &str, words: &[&str], w: &str| {
            let titles: Vec<Vec<u16>> = words.iter().map(|w| w.encode_utf16().collect()).collect();
            let w: Vec<u16> = w.encode_utf16().collect();
            let collator = Collator::for_choice(lang, Device::Paperwhite5);
            words[choose(&titles, &w, &collator)].to_string()
        };
        assert_eq!(
            pick("it", &["email", "e\u{200B}-mail"], "E-MAIL"),
            "e\u{200B}-mail"
        );
        assert_eq!(
            pick(
                "de",
                &["KinderGarten", "Kinder-Garten"],
                "Kinder\u{200D}-Garten"
            ),
            "Kinder-Garten"
        );
        assert_eq!(
            pick("en", &["email", "e-\u{200B}mail-"], "e\u{200B}-mail"),
            "e-\u{200B}mail-"
        );
        assert_eq!(pick("en", &["a", "ab\u{200B}-"], "ab-\u{200B}cd"), "a");
    }

    /// One adjacent pair stored out of order. The search halves toward the
    /// lower middle, so it tests "d-b" and stops there: "db" opens "d-b", and
    /// "da" is never tested although it is stored.
    #[test]
    fn one_swapped_pair_hides_the_label_the_midpoint_skips() {
        let data = utf16_dictionary(&["a", "b", "c", "d-b", "da", "e"], &[6], 0x09);
        assert_eq!(opened(&report(&data, "db")), Some("d-b"));
        let r = report(&data, "da");
        assert_eq!(r.result, None);
        assert_eq!(r.unreachable, ["da"]);
    }

    /// The search halves toward the lower middle at every step, including the
    /// first: with "b-z" stored before "ba", "ba" is tested first and opens,
    /// while "b-z" and "ca" are stepped past and open nothing.
    #[test]
    fn the_first_midpoint_is_the_lower_middle() {
        let data = utf16_dictionary(&["a", "b", "b-z", "ba", "bb", "c", "c-z", "ca"], &[8], 0x09);
        assert_eq!(opened(&report(&data, "ba")), Some("ba"));
        assert_eq!(opened(&report(&data, "bb")), Some("bb"));
        assert_eq!(opened(&report(&data, "c-z")), Some("c-z"));
        assert_eq!(opened(&report(&data, "ca")), None);
        assert_eq!(opened(&report(&data, "b-z")), None);
    }

    /// Russian dictionaries lowercase the word on the Paperwhite 4 and 5; no
    /// other language does, and the Kindle 4 does not.
    #[test]
    fn russian_lowercasing_is_the_paperwhites() {
        let ru = utf16_dictionary(&["Москва", "дом", "москва"], &[3], 0x19);
        let on = |data: &[u8], q: &str, device| {
            opened(&report_on(data, q, device, None)).map(str::to_string)
        };
        assert_eq!(on(&ru, "ДОМ", Device::Paperwhite5).as_deref(), Some("дом"));
        assert_eq!(on(&ru, "ДОМ", Device::Kindle4), None);
        assert_eq!(
            on(&ru, "Москва", Device::Paperwhite5).as_deref(),
            Some("москва")
        );
        assert_eq!(
            on(&ru, "Москва", Device::Kindle4).as_deref(),
            Some("Москва")
        );
        let r = report(&ru, "ДОМ");
        assert_eq!((r.formatted.as_str(), r.searched.as_str()), ("ДОМ", "дом"));
        // The word is searched as the Kindle lowercases it, not as Unicode
        // does: "ΑΣ-Α" is searched as "ασ-α".
        let sigma = utf16_dictionary(&["ας-α", "ασ-α"], &[2], 0x19);
        assert_eq!(
            on(&sigma, "ΑΣ-Α", Device::Paperwhite5).as_deref(),
            Some("ασ-α")
        );
        let uk = utf16_dictionary(&["Дом", "дом"], &[2], 0x22);
        assert_eq!(on(&uk, "ДОМ", Device::Paperwhite5), None);
        assert_eq!(on(&uk, "Дом", Device::Paperwhite5).as_deref(), Some("Дом"));
    }

    /// Firmware 5.19.6 picks among the run by the default collation rules in
    /// every language, where the Paperwhite 5 uses the book language's.
    #[test]
    fn firmware_5_19_6_picks_by_the_default_rules() {
        let on = |labels: &[&str], lcid: u32, q: &str, device| {
            let data = utf16_dictionary(labels, &[labels.len()], lcid);
            opened(&report_on(&data, q, device, None)).map(str::to_string)
        };
        let both = |labels: &[&str], lcid: u32, q: &str| {
            (
                on(labels, lcid, q, Device::Paperwhite5),
                on(labels, lcid, q, Device::Firmware5_19_6),
            )
        };
        let some = |a: &str, b: &str| (Some(a.to_string()), Some(b.to_string()));
        assert_eq!(
            both(&["wykładnie", "wykładnię"], 0x15, "WYKŁADNIĘ"),
            some("wykładnię", "wykładnie")
        );
        assert_eq!(
            both(&["abislerinde", "abişlerinde"], 0x1F, "ABIŞLERINDE"),
            some("abislerinde", "abişlerinde")
        );
        assert_eq!(
            both(&["hayalarıma", "hayâlarıma"], 0x1F, "HAYÂLARIMA"),
            some("hayâlarıma", "hayalarıma")
        );
        assert_eq!(
            both(&["абдусалам", "абду-салам"], 0x19, "Абду-Салам"),
            some("абду-салам", "абдусалам")
        );
        assert_eq!(both(&["host", "høst"], 0x06, "HØST"), some("høst", "host"));
        assert_eq!(both(&["ad", "åd"], 0x06, "ÅD"), some("ad", "åd"));
    }

    /// Words the Kindle 4 formats differently or shows no popup for.
    #[test]
    fn kindle_4_stop_words_and_trimming() {
        let it = utf16_dictionary(&["casa", "città", "il"], &[3], 0x10);
        let k4 = report_on(&it, "IL", Device::Kindle4, None);
        assert!(k4.stop_word);
        assert_eq!(k4.result, None);
        assert_eq!(k4.run, ["il"]);
        assert_eq!(opened(&report(&it, "IL")), Some("il"));
        let nfd = "citta\u{300}";
        assert_eq!(
            opened(&report_on(&it, nfd, Device::Kindle4, None)),
            Some("città"),
            "the Kindle 4 drops the trailing accent and finds the plain spelling"
        );
        assert_eq!(opened(&report(&it, nfd)), None);
    }

    /// The Kindle 4 also drops marks at the end of a word that the Paperwhite
    /// 5 keeps, so some words have a spelling only the Kindle 4 looks them up
    /// by.
    #[test]
    fn kindle_spellings_mark_the_ones_only_the_kindle_4_uses() {
        let spellings = |word: &str, language_id: u8| -> Vec<(String, bool)> {
            kindle_spellings(word, language_id)
                .into_iter()
                .map(|s| (s.text, s.kindle4_only))
                .collect()
        };
        let both = |text: &str| (text.to_string(), false);
        let kindle4 = |text: &str| (text.to_string(), true);
        // Every Kindle drops U+0E4C; the Kindle 4 also drops the vowel sign
        // U+0E34 before it.
        assert_eq!(spellings("ศักดิ์", 0x1E), [both("ศักดิ"), kindle4("ศักด")]);
        // A vowel sign and a tone mark, a combining accent and a Hebrew point.
        assert_eq!(spellings("กระทู้", 0x1E), [kindle4("กระท")]);
        assert_eq!(spellings("citta\u{300}", 0x10), [kindle4("citta")]);
        assert_eq!(spellings("שלום\u{5B8}", 0x0D), [kindle4("שלום")]);
        // The Kindle 4 looks words up only from a book in the dictionary's
        // language, and an Italian book shows no popup for "il".
        assert_eq!(spellings("il\u{301}", 0x10), []);
        // A word made of letters is looked up as it is.
        assert_eq!(spellings("กิน", 0x1E), []);
    }

    /// The shape Mobipocket Creator wrote: a 164-byte primary header with no
    /// index name and TAGX straight after it, cp1252 labels, and as many
    /// control bytes per entry as the TAGX says. It carries no byte weight
    /// table, so its labels compare as the bytes they are.
    fn creator_dictionary(entries: &[(&[u8], u32)], control: u32) -> Vec<u8> {
        creator_dictionary_with(entries, control, None, &[])
    }

    /// A Creator byte weight table in the shape of the one Creator writes:
    /// letters weigh as their lowercase ASCII letter (`é` as `e`), digits and
    /// the space as themselves, everything else nothing.
    fn creator_weights() -> Vec<u8> {
        (0..=255u8)
            .map(|b| match b {
                b'0'..=b'9' | b' ' | b'a'..=b'z' => b,
                b'A'..=b'Z' => b.to_ascii_lowercase(),
                0xE9 => b'e',
                _ => 0,
            })
            .collect()
    }

    fn creator_dictionary_with(
        entries: &[(&[u8], u32)],
        control: u32,
        weights: Option<&[u8]>,
        ligatures: &[[u8; 4]],
    ) -> Vec<u8> {
        let mut rec0 = vec![0u8; 256];
        rec0[0..2].copy_from_slice(&2u16.to_be_bytes());
        rec0[40..44].copy_from_slice(&1u32.to_be_bytes());
        rec0[0x60..0x64].copy_from_slice(&0x409u32.to_be_bytes());

        let mut primary = vec![0u8; 164];
        primary[0..4].copy_from_slice(b"INDX");
        primary[4..8].copy_from_slice(&164u32.to_be_bytes());
        primary[24..28].copy_from_slice(&1u32.to_be_bytes());
        primary[28..32].copy_from_slice(&1252u32.to_be_bytes());
        primary[36..40].copy_from_slice(&(entries.len() as u32).to_be_bytes());
        primary.extend_from_slice(b"TAGX");
        primary.extend_from_slice(&20u32.to_be_bytes());
        primary.extend_from_slice(&control.to_be_bytes());
        primary.extend_from_slice(&[1, 1, 0x01, 0, 0, 0, 0, 1]);
        if let Some(weights) = weights {
            let at = primary.len() as u32;
            primary[0x28..0x2C].copy_from_slice(&at.to_be_bytes());
            primary.extend_from_slice(b"ORDT");
            primary.extend_from_slice(weights);
        }
        if !ligatures.is_empty() {
            let at = primary.len() as u32;
            primary[0x2C..0x30].copy_from_slice(&at.to_be_bytes());
            primary[0x30..0x34].copy_from_slice(&(ligatures.len() as u32).to_be_bytes());
            primary.extend_from_slice(b"LIGT");
            primary.extend(ligatures.iter().flatten());
        }

        let mut leaf = vec![0u8; 192];
        leaf[0..4].copy_from_slice(b"INDX");
        leaf[4..8].copy_from_slice(&192u32.to_be_bytes());
        leaf[12..16].copy_from_slice(&1u32.to_be_bytes());
        leaf[28..32].copy_from_slice(&u32::MAX.to_be_bytes());
        let encoded: Vec<Vec<u8>> = entries
            .iter()
            .map(|(label, pos)| {
                let mut e = vec![label.len() as u8];
                e.extend_from_slice(label);
                e.extend(std::iter::repeat_n(0x01u8, control as usize));
                e.extend(inverted_vwi(*pos));
                e
            })
            .collect();
        palmdb(&[rec0, primary, with_idxt(leaf, &encoded)])
    }

    /// Mobipocket Creator's dictionaries were unreadable twice over: the
    /// index was never found, because its primary names no index and
    /// declares cp1252, and had it been found every label was read as UTF-16.
    #[test]
    fn a_mobipocket_creator_dictionary_is_found_and_read() {
        let data = creator_dictionary(&[(b"caf\xe9", 200), (b"chair", 100)], 1);
        let r = report(&data, "chair");
        assert_eq!(r.index_record, Some(1), "the cp1252 primary is the index");
        let hit = r.result.expect("chair resolves");
        assert_eq!((hit.matched_label.as_str(), hit.position), ("chair", 100));
        let hit = report(&data, "café").result.expect("café resolves");
        assert_eq!((hit.matched_label.as_str(), hit.position), ("café", 200));
        assert_eq!(opened(&report(&data, "cafe")), None, "no table, no folding");
        assert_eq!(opened(&report(&data, "Chair")), None);
    }

    /// A Creator index is weighed through its own byte weight table, and the
    /// word is written in cp1252 first: a letter cp1252 lacks weighs as a
    /// space rather than as its base letter, and a byte the table weighs
    /// nothing is skipped even where the Unicode weights would count it.
    #[test]
    fn a_creator_index_weighs_bytes_through_its_own_table() {
        let weights = creator_weights();
        let data = creator_dictionary_with(
            &[(b"caf\xe9", 200), (b"ch\xb2air", 100), (b"success", 300)],
            1,
            Some(&weights),
            &[],
        );
        assert_eq!(opened(&report(&data, "CAFE")), Some("café"));
        assert_eq!(opened(&report(&data, "chair")), Some("ch²air"));
        assert_eq!(opened(&report(&data, "ch2air")), None);
        // The non-breaking hyphen is written as "-", which weighs nothing.
        assert_eq!(opened(&report(&data, "ch\u{2011}air")), Some("ch²air"));
        assert_eq!(opened(&report(&data, "success")), Some("success"));
        assert_eq!(opened(&report(&data, "şuccess")), None);
        let data = creator_dictionary_with(&[(b"ch ir", 100)], 1, Some(&weights), &[]);
        assert_eq!(
            opened(&report(&data, "chŗir")),
            Some("ch ir"),
            "ŗ is a space"
        );
        assert_eq!(opened(&report(&data, "chir")), None);
        // Each byte the LIGT block lists is replaced by the two bytes given
        // for it before it is weighed: ß is searched as "ss".
        let data = creator_dictionary_with(
            &[(b"strasse", 100)],
            1,
            Some(&weights),
            &[[0xDF, 2, b's', b's']],
        );
        assert_eq!(opened(&report(&data, "straße")), Some("strasse"));
    }

    /// A Creator index of UTF-8 labels is weighed byte by byte through its
    /// table as well, with the word written in cp1252: "tazçalenkat" is
    /// found by "tazaalenkat" when the table weighs 0xC3 as nothing and 0xA7
    /// as "a".
    #[test]
    fn a_utf8_creator_index_weighs_bytes_through_its_own_table() {
        let mut rec0 = vec![0u8; 256];
        rec0[0..2].copy_from_slice(&2u16.to_be_bytes());
        rec0[40..44].copy_from_slice(&1u32.to_be_bytes());
        rec0[0x60..0x64].copy_from_slice(&0x409u32.to_be_bytes());
        let mut primary = vec![0u8; 192];
        primary[0..4].copy_from_slice(b"INDX");
        primary[4..8].copy_from_slice(&195u32.to_be_bytes());
        primary[24..28].copy_from_slice(&1u32.to_be_bytes());
        primary[28..32].copy_from_slice(&UTF8_INDEX_ENCODING.to_be_bytes());
        primary[36..40].copy_from_slice(&1u32.to_be_bytes());
        primary.extend_from_slice(b"idx");
        primary.extend_from_slice(b"TAGX");
        primary.extend_from_slice(&20u32.to_be_bytes());
        primary.extend_from_slice(&1u32.to_be_bytes());
        primary.extend_from_slice(&[1, 1, 0x01, 0, 0, 0, 0, 1]);
        let mut weights = creator_weights();
        weights[0xA7] = b'a';
        let at = primary.len() as u32;
        primary[0x28..0x2C].copy_from_slice(&at.to_be_bytes());
        primary.extend_from_slice(b"ORDT");
        primary.extend_from_slice(&weights);
        let mut leaf = vec![0u8; 192];
        leaf[0..4].copy_from_slice(b"INDX");
        leaf[4..8].copy_from_slice(&192u32.to_be_bytes());
        leaf[12..16].copy_from_slice(&1u32.to_be_bytes());
        let label = "tazçalenkat".as_bytes();
        let mut entry = vec![label.len() as u8];
        entry.extend_from_slice(label);
        entry.push(0x01);
        entry.extend(inverted_vwi(100));
        let data = palmdb(&[rec0, primary, with_idxt(leaf, &[entry])]);
        assert_eq!(opened(&report(&data, "tazaalenkat")), Some("tazçalenkat"));
        assert_eq!(opened(&report(&data, "tazcalenkat")), None);
    }

    /// A Kindle searches only the index the MOBI header names. Otherwise
    /// nothing opens, and the report says what the index would open. The
    /// dictionary languages in the header do not stop the search: the
    /// Paperwhite 4 and 5 open words in a file that declares neither.
    #[test]
    fn a_kindle_skips_an_index_the_header_does_not_name() {
        let data = utf16_dictionary(&["book", "cloud"], &[2], 0x09);
        let r = report(&data, "book");
        assert!(r.result.is_some() && r.would_open.is_none());
        assert!(!r.kindle_skips_index());

        // rec0 starts after the 78-byte header, three 8-byte record entries
        // and 2 bytes of padding.
        let rec0 = 78 + 3 * 8 + 2;
        let with = |at: usize, value: u32| {
            let mut data = data.clone();
            data[rec0 + at..rec0 + at + 4].copy_from_slice(&value.to_be_bytes());
            data
        };
        for data in [with(40, 2), with(40, u32::MAX)] {
            let r = report(&data, "book");
            assert_eq!(r.index_record, Some(1));
            assert_eq!(r.result, None);
            assert!(r.index_pointer_is_stale());
            assert!(r.kindle_skips_index());
            let hit = r.would_open.expect("the index would open book");
            assert_eq!((hit.matched_label.as_str(), hit.position), ("book", 0));
        }

        // Input language at 0x60, output language at 0x64.
        for (input, output) in [(0u32, 0u32), (0, 0x409), (0x409, 0)] {
            let mut data = with(0x60, input);
            data[rec0 + 0x64..rec0 + 0x68].copy_from_slice(&output.to_be_bytes());
            let r = report(&data, "book");
            assert!(!r.kindle_skips_index());
            assert_eq!(
                opened(&r),
                Some("book"),
                "input language {input:#x}, output language {output:#x}"
            );
        }
    }

    /// Two control bytes per entry, as Creator wrote for a busier index.
    /// Reading one put every text position a byte early.
    #[test]
    fn positions_come_after_every_control_byte() {
        let data = creator_dictionary(&[(b"bed", 300), (b"chair", 5000)], 2);
        assert_eq!(report(&data, "bed").result.map(|h| h.position), Some(300));
        assert_eq!(
            report(&data, "chair").result.map(|h| h.position),
            Some(5000)
        );
    }

    /// The same cp1252 index in a file whose header declares no dictionary
    /// is an ordinary index, not something to look words up in.
    #[test]
    fn a_cp1252_index_in_a_book_is_not_a_dictionary() {
        let mut data = creator_dictionary(&[(b"chair", 100)], 1);
        // rec0 starts after the 78-byte header, three 8-byte record entries
        // and 2 bytes of padding; its orth pointer is at offset 40.
        let rec0 = 78 + 3 * 8 + 2;
        data[rec0 + 40..rec0 + 44].copy_from_slice(&u32::MAX.to_be_bytes());
        let r = report(&data, "chair");
        assert_eq!(r.index_record, None, "a book has no dictionary index");
        assert!(r.result.is_none());
    }

    #[test]
    fn cp1252_labels_use_the_whole_table() {
        let coding = LabelCoding {
            encoding: CP1252_INDEX_ENCODING,
            ordt2: None,
            two_byte: false,
        };
        assert_eq!(text(&coding.units(b"\x80\x8c\x9c\xe9")), "€Œœé");
    }
}
