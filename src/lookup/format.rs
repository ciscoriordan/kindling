//! How a Kindle rewrites a tapped word before it looks the word up.
//!
//! The rules follow the language of the book the word was tapped in, whatever
//! dictionary answers, and are the same on the Kindle 4 (firmware 4.1.4), the
//! Paperwhite 4 (firmware 5.16.5), the Paperwhite 5 (firmware 5.19.2) and
//! firmware 5.19.6 except where [`Device`] and the notes below say. They run
//! once each, in this order, on the word's UTF-16 code units:
//!
//! 1. A right single quotation mark U+2019 becomes an ASCII apostrophe. No
//!    other apostrophe-like character changes: U+2018, U+02BC, U+0060, U+00B4
//!    and U+FF07 stay as they are.
//! 2. Characters before the first letter or apostrophe are dropped, and so are
//!    the characters after the last letter: `MP3` becomes `MP`, `km³` `km`,
//!    `(casa)` `casa`, `ex-` `ex`. Interior characters stay (`H2O`, `New York`,
//!    `rock'n'roll`), and so do doubled spaces. When an apostrophe was among
//!    the dropped trailing characters and the word starts with one, that one
//!    leading apostrophe goes too, so `'tis'` becomes `tis` while `'tis` stays.
//! 3. Italian and French books drop an elided word up to and including its
//!    apostrophe, when the part before the apostrophe is on the language's
//!    list, ignoring case but not accents: `l'altro` becomes `altro` and
//!    `quest'anno` `anno`, while `tutt'al più` and `aujourd'hui` stay whole.
//!    One elision at most.
//! 4. English books drop a final `'s` or `'S`: `John's` becomes `John`.
//! 5. French and Portuguese books repeatedly drop a final clitic pronoun
//!    joined by a hyphen, U+2010 or U+2011 (not an en dash): `rendez-vous`
//!    becomes `rendez`, `va-t-il` `va-t`, `dá-me` `dá`. Portuguese books then
//!    remove a pronoun between two hyphens before a verb ending: `dar-te-ei`
//!    becomes `darei`.
//!
//! A word that comes out empty (`123`, `...`, English `'s`) is not looked up.
//! The case is never changed here. Only the rewritten word is looked up,
//! except in a Japanese book on the Paperwhite 4 and 5: they look up the word
//! as tapped first and, when that finds nothing, the rewritten word turned into
//! its dictionary form (`厳しく` finds `厳しい`); that step is not modeled. The
//! Kindle 4 looks up only the rewritten word in a Japanese book too.
//!
//! "Letter" and "combining mark" follow the Unicode 12.1 character
//! properties, as on the Paperwhite 5 (firmware 5.19.2) and firmware 5.19.6.
//! The Paperwhite 4 (firmware 5.16.5) follows Unicode 9.0 instead, so there a
//! letter or mark added to Unicode since then is dropped at either edge
//! (Georgian capital letters, U+1C90 to U+1CBF, among them), and so are the
//! Myanmar tone marks U+1063 and U+1064, U+1069 to U+106D, U+1087 to U+108C,
//! U+108F, U+109A, U+109B and U+AA7B to U+AA7D, and the signs U+1C36, U+A802,
//! U+A80B and U+A9E5, while U+135F is kept at the start of a word. The Kindle
//! 4's letter test is narrower (see [`is_letter`]).

use super::Device;
use super::collator::Collator;

const APOSTROPHE: u16 = 0x27;

/// The elided forms an Italian book drops before an apostrophe.
const ITALIAN_ELISIONS: &[&str] = &[
    "l", "dell", "all", "dall", "nell", "sull", "gl", "un", "c", "grand", "sant", "pover", "bell",
    "buen", "quell", "nessun", "qualcun", "ciascun", "alcun", "d", "anch", "sul", "sar", "mi",
    "cos", "degl", "agl", "sugl", "quegl", "negl", "dagl", "quest", "su", "tu", "vostr", "nostr",
    "lor", "m", "t", "v", "s", "senz", "qualcos", "buon", "cent",
];

/// The elided forms a French book drops before an apostrophe.
const FRENCH_ELISIONS: &[&str] = &["l", "j", "c", "m", "t", "s", "d", "qu", "n"];

/// The clitic pronouns a French book drops after a hyphen.
const FRENCH_CLITICS: &[&str] = &[
    "le", "la", "les", "lui", "leur", "moi", "toi", "nous", "vous", "y", "en", "il", "elle", "ils",
    "elles", "je", "z", "m'en", "t'en",
];

/// The clitic pronouns a Portuguese book drops after a hyphen, or from
/// between two hyphens.
const PORTUGUESE_CLITICS: &[&str] = &[
    "me", "te", "o", "a", "lhe", "nos", "vos", "os", "as", "lhes", "se", "lo", "la", "los", "las",
    "no", "na", "nas",
];

/// The words for which a Kindle 4 shows no popup at all, in a book of
/// `language`, as far as they are modeled here: in Italian, Spanish, French,
/// German, English, Portuguese and Turkish books every single letter, and in
/// those and Danish and Swedish books the words seen to do it. The Kindle 4
/// does this for more words than these (`di`, `per` and `non` in Italian,
/// `los` and `por` in Spanish), and also in Dutch (`de`, `het`), Romanian
/// (`la`, `în`), Finnish (`on`, `se`) and Hungarian (`az`, `nem`) books,
/// while a Norwegian book gets none; only the words above are modeled. The
/// case of the word is ignored, its accents are not. The Paperwhite 4 and 5
/// have no such lists.
pub(super) fn kindle4_stop_word(word: &[u16], language: &str) -> bool {
    let (letters, seen): (bool, &[&str]) = match language {
        "it" => (true, &["il", "io", "è", "ò"]),
        "fr" => (true, &["le", "il", "est", "sur"]),
        "en" => (true, &["the", "am", "it"]),
        "de" => (true, &["der"]),
        "es" => (true, &["el"]),
        "pt" | "tr" => (true, &[]),
        "da" => (false, &["og", "til"]),
        "sv" => (false, &["och"]),
        _ => return false,
    };
    let trimmed = trim_controls(word);
    let collator = Collator::secondary(language, Device::Kindle4);
    let single_letter =
        trimmed.len() == 1 && u8::try_from(trimmed[0]).is_ok_and(|b| b.is_ascii_alphabetic());
    (letters && single_letter) || seen.iter().any(|w| collator.equal(trimmed, &units(w)))
}

/// Whether books in `language` get rules of their own (steps 3 to 5 above).
/// Books in any other language rewrite a word as a book in a language without
/// a code does.
pub(super) fn has_language_rules(language: &str) -> bool {
    matches!(language, "it" | "fr" | "en" | "pt")
}

/// The word as a Kindle of `device` looks it up from a book in `language`
/// (a primary language code such as `it`), or an empty word when it is not
/// looked up at all.
pub(super) fn format_word(word: &[u16], language: &str, device: Device) -> Vec<u16> {
    let mut s: Vec<u16> = word
        .iter()
        .map(|&u| if u == 0x2019 { APOSTROPHE } else { u })
        .collect();

    let start = s
        .iter()
        .position(|&u| u == APOSTROPHE || is_letter(u, device))
        .unwrap_or(s.len());
    s.drain(..start);
    let mut end = s.len();
    while end > 0 && !ends_trailing_trim(s[end - 1], device) {
        end -= 1;
    }
    let dropped_apostrophe = s[end..].contains(&APOSTROPHE);
    s.truncate(end);
    if dropped_apostrophe && s.first() == Some(&APOSTROPHE) {
        s.remove(0);
    }

    let collator = Collator::secondary(language, device);
    let on_list =
        |part: &[u16], list: &[&str]| list.iter().any(|w| collator.equal(part, &units(w)));
    let elisions = match language {
        "it" => ITALIAN_ELISIONS,
        "fr" => FRENCH_ELISIONS,
        _ => &[],
    };
    if !elisions.is_empty() {
        let cut = (0..s.len().saturating_sub(1))
            .find(|&i| s[i] == APOSTROPHE && on_list(trim_controls(&s[..i]), elisions));
        if let Some(i) = cut {
            s.drain(..=i);
        }
    }

    if language == "en"
        && s.len() >= 2
        && s[s.len() - 2] == APOSTROPHE
        && matches!(s[s.len() - 1], 0x73 | 0x53)
    {
        s.truncate(s.len() - 2);
    }

    let clitics = match language {
        "fr" => FRENCH_CLITICS,
        "pt" => PORTUGUESE_CLITICS,
        _ => &[],
    };
    if !clitics.is_empty() {
        while let Some(k) = s.iter().rposition(|&u| is_clitic_hyphen(u)) {
            if k == 0 || k + 1 == s.len() || !on_list(&s[k + 1..], clitics) {
                break;
            }
            s.truncate(k);
        }
    }
    if language == "pt" {
        if let Some(a) = s.iter().position(|&u| is_clitic_hyphen(u)) {
            let b = a + 1;
            let second = s[b..].iter().position(|&u| is_clitic_hyphen(u));
            if let Some(c) = second.map(|offset| b + offset) {
                let d = c + 1;
                if a > 0 && b + 3 <= s.len() && c > b && d < s.len() && on_list(&s[b..c], clitics) {
                    s.drain(a..d);
                }
            }
        }
    }
    s
}

fn units(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// `s` without the characters up to U+0020 at either end.
fn trim_controls(s: &[u16]) -> &[u16] {
    let start = s.iter().position(|&u| u > 0x20).unwrap_or(s.len());
    let end = s.iter().rposition(|&u| u > 0x20).map_or(start, |i| i + 1);
    &s[start..end]
}

fn is_clitic_hyphen(unit: u16) -> bool {
    matches!(unit, 0x2D | 0x2010 | 0x2011)
}

/// Whether a unit counts as a letter at the edges of a word. The Paperwhite 4
/// and 5 take every character with the Unicode Alphabetic property, which
/// includes the letter numbers (`Ⅻ`), the circled letters and many marks (the
/// Thai vowel sign U+0E31, the Devanagari sign U+0903). The Kindle 4 takes only
/// the characters in a letter category, so it drops all of those at either
/// edge; its Unicode tables are older still, so a letter added since may be
/// dropped there too, which is not modeled. A lone surrogate is never a
/// letter, so a letter outside the Basic Multilingual Plane is dropped at
/// either edge.
pub(super) fn is_letter(unit: u16, device: Device) -> bool {
    match device {
        Device::Kindle4 => alphabetic(unit) && !alphabetic_but_not_a_letter(unit),
        Device::Paperwhite5 | Device::Firmware5_19_6 => alphabetic(unit),
    }
}

/// Whether the trailing trim stops at this unit. On the Paperwhite 4 and
/// later it also stops at a combining mark with a nonzero canonical combining
/// class, so `cafe` followed by U+0301 keeps its accent there and loses it on
/// the Kindle 4.
fn ends_trailing_trim(unit: u16, device: Device) -> bool {
    is_letter(unit, device) || (device != Device::Kindle4 && has_combining_class(unit))
}

/// Whether a Basic Multilingual Plane character has the Alphabetic property
/// in Unicode 12.1. Generated from DerivedCoreProperties-12.1.0.txt.
fn alphabetic(unit: u16) -> bool {
    matches!(
        unit,
        0x0041..=0x005A
            | 0x0061..=0x007A
            | 0x00AA
            | 0x00B5
            | 0x00BA
            | 0x00C0..=0x00D6
            | 0x00D8..=0x00F6
            | 0x00F8..=0x02C1
            | 0x02C6..=0x02D1
            | 0x02E0..=0x02E4
            | 0x02EC
            | 0x02EE
            | 0x0345
            | 0x0370..=0x0374
            | 0x0376..=0x0377
            | 0x037A..=0x037D
            | 0x037F
            | 0x0386
            | 0x0388..=0x038A
            | 0x038C
            | 0x038E..=0x03A1
            | 0x03A3..=0x03F5
            | 0x03F7..=0x0481
            | 0x048A..=0x052F
            | 0x0531..=0x0556
            | 0x0559
            | 0x0560..=0x0588
            | 0x05B0..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x05D0..=0x05EA
            | 0x05EF..=0x05F2
            | 0x0610..=0x061A
            | 0x0620..=0x0657
            | 0x0659..=0x065F
            | 0x066E..=0x06D3
            | 0x06D5..=0x06DC
            | 0x06E1..=0x06E8
            | 0x06ED..=0x06EF
            | 0x06FA..=0x06FC
            | 0x06FF
            | 0x0710..=0x073F
            | 0x074D..=0x07B1
            | 0x07CA..=0x07EA
            | 0x07F4..=0x07F5
            | 0x07FA
            | 0x0800..=0x0817
            | 0x081A..=0x082C
            | 0x0840..=0x0858
            | 0x0860..=0x086A
            | 0x08A0..=0x08B4
            | 0x08B6..=0x08BD
            | 0x08D4..=0x08DF
            | 0x08E3..=0x08E9
            | 0x08F0..=0x093B
            | 0x093D..=0x094C
            | 0x094E..=0x0950
            | 0x0955..=0x0963
            | 0x0971..=0x0983
            | 0x0985..=0x098C
            | 0x098F..=0x0990
            | 0x0993..=0x09A8
            | 0x09AA..=0x09B0
            | 0x09B2
            | 0x09B6..=0x09B9
            | 0x09BD..=0x09C4
            | 0x09C7..=0x09C8
            | 0x09CB..=0x09CC
            | 0x09CE
            | 0x09D7
            | 0x09DC..=0x09DD
            | 0x09DF..=0x09E3
            | 0x09F0..=0x09F1
            | 0x09FC
            | 0x0A01..=0x0A03
            | 0x0A05..=0x0A0A
            | 0x0A0F..=0x0A10
            | 0x0A13..=0x0A28
            | 0x0A2A..=0x0A30
            | 0x0A32..=0x0A33
            | 0x0A35..=0x0A36
            | 0x0A38..=0x0A39
            | 0x0A3E..=0x0A42
            | 0x0A47..=0x0A48
            | 0x0A4B..=0x0A4C
            | 0x0A51
            | 0x0A59..=0x0A5C
            | 0x0A5E
            | 0x0A70..=0x0A75
            | 0x0A81..=0x0A83
            | 0x0A85..=0x0A8D
            | 0x0A8F..=0x0A91
            | 0x0A93..=0x0AA8
            | 0x0AAA..=0x0AB0
            | 0x0AB2..=0x0AB3
            | 0x0AB5..=0x0AB9
            | 0x0ABD..=0x0AC5
            | 0x0AC7..=0x0AC9
            | 0x0ACB..=0x0ACC
            | 0x0AD0
            | 0x0AE0..=0x0AE3
            | 0x0AF9..=0x0AFC
            | 0x0B01..=0x0B03
            | 0x0B05..=0x0B0C
            | 0x0B0F..=0x0B10
            | 0x0B13..=0x0B28
            | 0x0B2A..=0x0B30
            | 0x0B32..=0x0B33
            | 0x0B35..=0x0B39
            | 0x0B3D..=0x0B44
            | 0x0B47..=0x0B48
            | 0x0B4B..=0x0B4C
            | 0x0B56..=0x0B57
            | 0x0B5C..=0x0B5D
            | 0x0B5F..=0x0B63
            | 0x0B71
            | 0x0B82..=0x0B83
            | 0x0B85..=0x0B8A
            | 0x0B8E..=0x0B90
            | 0x0B92..=0x0B95
            | 0x0B99..=0x0B9A
            | 0x0B9C
            | 0x0B9E..=0x0B9F
            | 0x0BA3..=0x0BA4
            | 0x0BA8..=0x0BAA
            | 0x0BAE..=0x0BB9
            | 0x0BBE..=0x0BC2
            | 0x0BC6..=0x0BC8
            | 0x0BCA..=0x0BCC
            | 0x0BD0
            | 0x0BD7
            | 0x0C00..=0x0C03
            | 0x0C05..=0x0C0C
            | 0x0C0E..=0x0C10
            | 0x0C12..=0x0C28
            | 0x0C2A..=0x0C39
            | 0x0C3D..=0x0C44
            | 0x0C46..=0x0C48
            | 0x0C4A..=0x0C4C
            | 0x0C55..=0x0C56
            | 0x0C58..=0x0C5A
            | 0x0C60..=0x0C63
            | 0x0C80..=0x0C83
            | 0x0C85..=0x0C8C
            | 0x0C8E..=0x0C90
            | 0x0C92..=0x0CA8
            | 0x0CAA..=0x0CB3
            | 0x0CB5..=0x0CB9
            | 0x0CBD..=0x0CC4
            | 0x0CC6..=0x0CC8
            | 0x0CCA..=0x0CCC
            | 0x0CD5..=0x0CD6
            | 0x0CDE
            | 0x0CE0..=0x0CE3
            | 0x0CF1..=0x0CF2
            | 0x0D00..=0x0D03
            | 0x0D05..=0x0D0C
            | 0x0D0E..=0x0D10
            | 0x0D12..=0x0D3A
            | 0x0D3D..=0x0D44
            | 0x0D46..=0x0D48
            | 0x0D4A..=0x0D4C
            | 0x0D4E
            | 0x0D54..=0x0D57
            | 0x0D5F..=0x0D63
            | 0x0D7A..=0x0D7F
            | 0x0D82..=0x0D83
            | 0x0D85..=0x0D96
            | 0x0D9A..=0x0DB1
            | 0x0DB3..=0x0DBB
            | 0x0DBD
            | 0x0DC0..=0x0DC6
            | 0x0DCF..=0x0DD4
            | 0x0DD6
            | 0x0DD8..=0x0DDF
            | 0x0DF2..=0x0DF3
            | 0x0E01..=0x0E3A
            | 0x0E40..=0x0E46
            | 0x0E4D
            | 0x0E81..=0x0E82
            | 0x0E84
            | 0x0E86..=0x0E8A
            | 0x0E8C..=0x0EA3
            | 0x0EA5
            | 0x0EA7..=0x0EB9
            | 0x0EBB..=0x0EBD
            | 0x0EC0..=0x0EC4
            | 0x0EC6
            | 0x0ECD
            | 0x0EDC..=0x0EDF
            | 0x0F00
            | 0x0F40..=0x0F47
            | 0x0F49..=0x0F6C
            | 0x0F71..=0x0F81
            | 0x0F88..=0x0F97
            | 0x0F99..=0x0FBC
            | 0x1000..=0x1036
            | 0x1038
            | 0x103B..=0x103F
            | 0x1050..=0x108F
            | 0x109A..=0x109D
            | 0x10A0..=0x10C5
            | 0x10C7
            | 0x10CD
            | 0x10D0..=0x10FA
            | 0x10FC..=0x1248
            | 0x124A..=0x124D
            | 0x1250..=0x1256
            | 0x1258
            | 0x125A..=0x125D
            | 0x1260..=0x1288
            | 0x128A..=0x128D
            | 0x1290..=0x12B0
            | 0x12B2..=0x12B5
            | 0x12B8..=0x12BE
            | 0x12C0
            | 0x12C2..=0x12C5
            | 0x12C8..=0x12D6
            | 0x12D8..=0x1310
            | 0x1312..=0x1315
            | 0x1318..=0x135A
            | 0x1380..=0x138F
            | 0x13A0..=0x13F5
            | 0x13F8..=0x13FD
            | 0x1401..=0x166C
            | 0x166F..=0x167F
            | 0x1681..=0x169A
            | 0x16A0..=0x16EA
            | 0x16EE..=0x16F8
            | 0x1700..=0x170C
            | 0x170E..=0x1713
            | 0x1720..=0x1733
            | 0x1740..=0x1753
            | 0x1760..=0x176C
            | 0x176E..=0x1770
            | 0x1772..=0x1773
            | 0x1780..=0x17B3
            | 0x17B6..=0x17C8
            | 0x17D7
            | 0x17DC
            | 0x1820..=0x1878
            | 0x1880..=0x18AA
            | 0x18B0..=0x18F5
            | 0x1900..=0x191E
            | 0x1920..=0x192B
            | 0x1930..=0x1938
            | 0x1950..=0x196D
            | 0x1970..=0x1974
            | 0x1980..=0x19AB
            | 0x19B0..=0x19C9
            | 0x1A00..=0x1A1B
            | 0x1A20..=0x1A5E
            | 0x1A61..=0x1A74
            | 0x1AA7
            | 0x1B00..=0x1B33
            | 0x1B35..=0x1B43
            | 0x1B45..=0x1B4B
            | 0x1B80..=0x1BA9
            | 0x1BAC..=0x1BAF
            | 0x1BBA..=0x1BE5
            | 0x1BE7..=0x1BF1
            | 0x1C00..=0x1C36
            | 0x1C4D..=0x1C4F
            | 0x1C5A..=0x1C7D
            | 0x1C80..=0x1C88
            | 0x1C90..=0x1CBA
            | 0x1CBD..=0x1CBF
            | 0x1CE9..=0x1CEC
            | 0x1CEE..=0x1CF3
            | 0x1CF5..=0x1CF6
            | 0x1CFA
            | 0x1D00..=0x1DBF
            | 0x1DE7..=0x1DF4
            | 0x1E00..=0x1F15
            | 0x1F18..=0x1F1D
            | 0x1F20..=0x1F45
            | 0x1F48..=0x1F4D
            | 0x1F50..=0x1F57
            | 0x1F59
            | 0x1F5B
            | 0x1F5D
            | 0x1F5F..=0x1F7D
            | 0x1F80..=0x1FB4
            | 0x1FB6..=0x1FBC
            | 0x1FBE
            | 0x1FC2..=0x1FC4
            | 0x1FC6..=0x1FCC
            | 0x1FD0..=0x1FD3
            | 0x1FD6..=0x1FDB
            | 0x1FE0..=0x1FEC
            | 0x1FF2..=0x1FF4
            | 0x1FF6..=0x1FFC
            | 0x2071
            | 0x207F
            | 0x2090..=0x209C
            | 0x2102
            | 0x2107
            | 0x210A..=0x2113
            | 0x2115
            | 0x2119..=0x211D
            | 0x2124
            | 0x2126
            | 0x2128
            | 0x212A..=0x212D
            | 0x212F..=0x2139
            | 0x213C..=0x213F
            | 0x2145..=0x2149
            | 0x214E
            | 0x2160..=0x2188
            | 0x24B6..=0x24E9
            | 0x2C00..=0x2C2E
            | 0x2C30..=0x2C5E
            | 0x2C60..=0x2CE4
            | 0x2CEB..=0x2CEE
            | 0x2CF2..=0x2CF3
            | 0x2D00..=0x2D25
            | 0x2D27
            | 0x2D2D
            | 0x2D30..=0x2D67
            | 0x2D6F
            | 0x2D80..=0x2D96
            | 0x2DA0..=0x2DA6
            | 0x2DA8..=0x2DAE
            | 0x2DB0..=0x2DB6
            | 0x2DB8..=0x2DBE
            | 0x2DC0..=0x2DC6
            | 0x2DC8..=0x2DCE
            | 0x2DD0..=0x2DD6
            | 0x2DD8..=0x2DDE
            | 0x2DE0..=0x2DFF
            | 0x2E2F
            | 0x3005..=0x3007
            | 0x3021..=0x3029
            | 0x3031..=0x3035
            | 0x3038..=0x303C
            | 0x3041..=0x3096
            | 0x309D..=0x309F
            | 0x30A1..=0x30FA
            | 0x30FC..=0x30FF
            | 0x3105..=0x312F
            | 0x3131..=0x318E
            | 0x31A0..=0x31BA
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DB5
            | 0x4E00..=0x9FEF
            | 0xA000..=0xA48C
            | 0xA4D0..=0xA4FD
            | 0xA500..=0xA60C
            | 0xA610..=0xA61F
            | 0xA62A..=0xA62B
            | 0xA640..=0xA66E
            | 0xA674..=0xA67B
            | 0xA67F..=0xA6EF
            | 0xA717..=0xA71F
            | 0xA722..=0xA788
            | 0xA78B..=0xA7BF
            | 0xA7C2..=0xA7C6
            | 0xA7F7..=0xA805
            | 0xA807..=0xA827
            | 0xA840..=0xA873
            | 0xA880..=0xA8C3
            | 0xA8C5
            | 0xA8F2..=0xA8F7
            | 0xA8FB
            | 0xA8FD..=0xA8FF
            | 0xA90A..=0xA92A
            | 0xA930..=0xA952
            | 0xA960..=0xA97C
            | 0xA980..=0xA9B2
            | 0xA9B4..=0xA9BF
            | 0xA9CF
            | 0xA9E0..=0xA9EF
            | 0xA9FA..=0xA9FE
            | 0xAA00..=0xAA36
            | 0xAA40..=0xAA4D
            | 0xAA60..=0xAA76
            | 0xAA7A..=0xAABE
            | 0xAAC0
            | 0xAAC2
            | 0xAADB..=0xAADD
            | 0xAAE0..=0xAAEF
            | 0xAAF2..=0xAAF5
            | 0xAB01..=0xAB06
            | 0xAB09..=0xAB0E
            | 0xAB11..=0xAB16
            | 0xAB20..=0xAB26
            | 0xAB28..=0xAB2E
            | 0xAB30..=0xAB5A
            | 0xAB5C..=0xAB67
            | 0xAB70..=0xABEA
            | 0xAC00..=0xD7A3
            | 0xD7B0..=0xD7C6
            | 0xD7CB..=0xD7FB
            | 0xF900..=0xFA6D
            | 0xFA70..=0xFAD9
            | 0xFB00..=0xFB06
            | 0xFB13..=0xFB17
            | 0xFB1D..=0xFB28
            | 0xFB2A..=0xFB36
            | 0xFB38..=0xFB3C
            | 0xFB3E
            | 0xFB40..=0xFB41
            | 0xFB43..=0xFB44
            | 0xFB46..=0xFBB1
            | 0xFBD3..=0xFD3D
            | 0xFD50..=0xFD8F
            | 0xFD92..=0xFDC7
            | 0xFDF0..=0xFDFB
            | 0xFE70..=0xFE74
            | 0xFE76..=0xFEFC
            | 0xFF21..=0xFF3A
            | 0xFF41..=0xFF5A
            | 0xFF66..=0xFFBE
            | 0xFFC2..=0xFFC7
            | 0xFFCA..=0xFFCF
            | 0xFFD2..=0xFFD7
            | 0xFFDA..=0xFFDC
    )
}

/// Whether a Basic Multilingual Plane character has the Alphabetic property
/// but is not in a letter category in Unicode 12.1: the letter numbers, the
/// marks that count as alphabetic and the circled letters. Generated from
/// DerivedCoreProperties-12.1.0.txt and DerivedGeneralCategory-12.1.0.txt.
fn alphabetic_but_not_a_letter(unit: u16) -> bool {
    matches!(
        unit,
        0x0345
            | 0x05B0..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x0657
            | 0x0659..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06E1..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06ED
            | 0x0711
            | 0x0730..=0x073F
            | 0x07A6..=0x07B0
            | 0x0816..=0x0817
            | 0x081B..=0x0823
            | 0x0825..=0x0827
            | 0x0829..=0x082C
            | 0x08D4..=0x08DF
            | 0x08E3..=0x08E9
            | 0x08F0..=0x0903
            | 0x093A..=0x093B
            | 0x093E..=0x094C
            | 0x094E..=0x094F
            | 0x0955..=0x0957
            | 0x0962..=0x0963
            | 0x0981..=0x0983
            | 0x09BE..=0x09C4
            | 0x09C7..=0x09C8
            | 0x09CB..=0x09CC
            | 0x09D7
            | 0x09E2..=0x09E3
            | 0x0A01..=0x0A03
            | 0x0A3E..=0x0A42
            | 0x0A47..=0x0A48
            | 0x0A4B..=0x0A4C
            | 0x0A51
            | 0x0A70..=0x0A71
            | 0x0A75
            | 0x0A81..=0x0A83
            | 0x0ABE..=0x0AC5
            | 0x0AC7..=0x0AC9
            | 0x0ACB..=0x0ACC
            | 0x0AE2..=0x0AE3
            | 0x0AFA..=0x0AFC
            | 0x0B01..=0x0B03
            | 0x0B3E..=0x0B44
            | 0x0B47..=0x0B48
            | 0x0B4B..=0x0B4C
            | 0x0B56..=0x0B57
            | 0x0B62..=0x0B63
            | 0x0B82
            | 0x0BBE..=0x0BC2
            | 0x0BC6..=0x0BC8
            | 0x0BCA..=0x0BCC
            | 0x0BD7
            | 0x0C00..=0x0C03
            | 0x0C3E..=0x0C44
            | 0x0C46..=0x0C48
            | 0x0C4A..=0x0C4C
            | 0x0C55..=0x0C56
            | 0x0C62..=0x0C63
            | 0x0C81..=0x0C83
            | 0x0CBE..=0x0CC4
            | 0x0CC6..=0x0CC8
            | 0x0CCA..=0x0CCC
            | 0x0CD5..=0x0CD6
            | 0x0CE2..=0x0CE3
            | 0x0D00..=0x0D03
            | 0x0D3E..=0x0D44
            | 0x0D46..=0x0D48
            | 0x0D4A..=0x0D4C
            | 0x0D57
            | 0x0D62..=0x0D63
            | 0x0D82..=0x0D83
            | 0x0DCF..=0x0DD4
            | 0x0DD6
            | 0x0DD8..=0x0DDF
            | 0x0DF2..=0x0DF3
            | 0x0E31
            | 0x0E34..=0x0E3A
            | 0x0E4D
            | 0x0EB1
            | 0x0EB4..=0x0EB9
            | 0x0EBB..=0x0EBC
            | 0x0ECD
            | 0x0F71..=0x0F81
            | 0x0F8D..=0x0F97
            | 0x0F99..=0x0FBC
            | 0x102B..=0x1036
            | 0x1038
            | 0x103B..=0x103E
            | 0x1056..=0x1059
            | 0x105E..=0x1060
            | 0x1062..=0x1064
            | 0x1067..=0x106D
            | 0x1071..=0x1074
            | 0x1082..=0x108D
            | 0x108F
            | 0x109A..=0x109D
            | 0x16EE..=0x16F0
            | 0x1712..=0x1713
            | 0x1732..=0x1733
            | 0x1752..=0x1753
            | 0x1772..=0x1773
            | 0x17B6..=0x17C8
            | 0x1885..=0x1886
            | 0x18A9
            | 0x1920..=0x192B
            | 0x1930..=0x1938
            | 0x1A17..=0x1A1B
            | 0x1A55..=0x1A5E
            | 0x1A61..=0x1A74
            | 0x1B00..=0x1B04
            | 0x1B35..=0x1B43
            | 0x1B80..=0x1B82
            | 0x1BA1..=0x1BA9
            | 0x1BAC..=0x1BAD
            | 0x1BE7..=0x1BF1
            | 0x1C24..=0x1C36
            | 0x1DE7..=0x1DF4
            | 0x2160..=0x2182
            | 0x2185..=0x2188
            | 0x24B6..=0x24E9
            | 0x2DE0..=0x2DFF
            | 0x3007
            | 0x3021..=0x3029
            | 0x3038..=0x303A
            | 0xA674..=0xA67B
            | 0xA69E..=0xA69F
            | 0xA6E6..=0xA6EF
            | 0xA802
            | 0xA80B
            | 0xA823..=0xA827
            | 0xA880..=0xA881
            | 0xA8B4..=0xA8C3
            | 0xA8C5
            | 0xA8FF
            | 0xA926..=0xA92A
            | 0xA947..=0xA952
            | 0xA980..=0xA983
            | 0xA9B4..=0xA9BF
            | 0xA9E5
            | 0xAA29..=0xAA36
            | 0xAA43
            | 0xAA4C..=0xAA4D
            | 0xAA7B..=0xAA7D
            | 0xAAB0
            | 0xAAB2..=0xAAB4
            | 0xAAB7..=0xAAB8
            | 0xAABE
            | 0xAAEB..=0xAAEF
            | 0xAAF5
            | 0xABE3..=0xABEA
            | 0xFB1E
    )
}

/// Whether a Basic Multilingual Plane character has a nonzero canonical
/// combining class in Unicode 12.1. Generated from
/// DerivedCombiningClass-12.1.0.txt.
pub(super) fn has_combining_class(unit: u16) -> bool {
    matches!(
        unit,
        0x0300..=0x034E
            | 0x0350..=0x036F
            | 0x0483..=0x0487
            | 0x0591..=0x05BD
            | 0x05BF
            | 0x05C1..=0x05C2
            | 0x05C4..=0x05C5
            | 0x05C7
            | 0x0610..=0x061A
            | 0x064B..=0x065F
            | 0x0670
            | 0x06D6..=0x06DC
            | 0x06DF..=0x06E4
            | 0x06E7..=0x06E8
            | 0x06EA..=0x06ED
            | 0x0711
            | 0x0730..=0x074A
            | 0x07EB..=0x07F3
            | 0x07FD
            | 0x0816..=0x0819
            | 0x081B..=0x0823
            | 0x0825..=0x0827
            | 0x0829..=0x082D
            | 0x0859..=0x085B
            | 0x08D3..=0x08E1
            | 0x08E3..=0x08FF
            | 0x093C
            | 0x094D
            | 0x0951..=0x0954
            | 0x09BC
            | 0x09CD
            | 0x09FE
            | 0x0A3C
            | 0x0A4D
            | 0x0ABC
            | 0x0ACD
            | 0x0B3C
            | 0x0B4D
            | 0x0BCD
            | 0x0C4D
            | 0x0C55..=0x0C56
            | 0x0CBC
            | 0x0CCD
            | 0x0D3B..=0x0D3C
            | 0x0D4D
            | 0x0DCA
            | 0x0E38..=0x0E3A
            | 0x0E48..=0x0E4B
            | 0x0EB8..=0x0EBA
            | 0x0EC8..=0x0ECB
            | 0x0F18..=0x0F19
            | 0x0F35
            | 0x0F37
            | 0x0F39
            | 0x0F71..=0x0F72
            | 0x0F74
            | 0x0F7A..=0x0F7D
            | 0x0F80
            | 0x0F82..=0x0F84
            | 0x0F86..=0x0F87
            | 0x0FC6
            | 0x1037
            | 0x1039..=0x103A
            | 0x108D
            | 0x135D..=0x135F
            | 0x1714
            | 0x1734
            | 0x17D2
            | 0x17DD
            | 0x18A9
            | 0x1939..=0x193B
            | 0x1A17..=0x1A18
            | 0x1A60
            | 0x1A75..=0x1A7C
            | 0x1A7F
            | 0x1AB0..=0x1ABD
            | 0x1B34
            | 0x1B44
            | 0x1B6B..=0x1B73
            | 0x1BAA..=0x1BAB
            | 0x1BE6
            | 0x1BF2..=0x1BF3
            | 0x1C37
            | 0x1CD0..=0x1CD2
            | 0x1CD4..=0x1CE0
            | 0x1CE2..=0x1CE8
            | 0x1CED
            | 0x1CF4
            | 0x1CF8..=0x1CF9
            | 0x1DC0..=0x1DF9
            | 0x1DFB..=0x1DFF
            | 0x20D0..=0x20DC
            | 0x20E1
            | 0x20E5..=0x20F0
            | 0x2CEF..=0x2CF1
            | 0x2D7F
            | 0x2DE0..=0x2DFF
            | 0x302A..=0x302F
            | 0x3099..=0x309A
            | 0xA66F
            | 0xA674..=0xA67D
            | 0xA69E..=0xA69F
            | 0xA6F0..=0xA6F1
            | 0xA806
            | 0xA8C4
            | 0xA8E0..=0xA8F1
            | 0xA92B..=0xA92D
            | 0xA953
            | 0xA9B3
            | 0xA9C0
            | 0xAAB0
            | 0xAAB2..=0xAAB4
            | 0xAAB7..=0xAAB8
            | 0xAABE..=0xAABF
            | 0xAAC1
            | 0xAAF6
            | 0xABED
            | 0xFB1E
            | 0xFE20..=0xFE2F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(word: &str, language: &str) -> String {
        fmt_on(word, language, Device::Paperwhite5)
    }

    fn fmt_on(word: &str, language: &str, device: Device) -> String {
        String::from_utf16(&format_word(&units(word), language, device)).unwrap()
    }

    /// The edge trim and the apostrophe rules, the same in every language.
    #[test]
    fn edges_and_apostrophes() {
        for (word, want) in [
            ("MP3", "MP"),
            ("COVID-19", "COVID"),
            ("4G", "G"),
            ("H2O", "H2O"),
            ("km³", "km"),
            ("¿qué?", "qué"),
            (" word ", "word"),
            ("\u{3000}word\u{3000}", "word"),
            ("New  York", "New  York"),
            ("123", ""),
            ("...", ""),
            ("(casa)", "casa"),
            ("\u{A0}casa\u{A0}", "casa"),
            ("ex-", "ex"),
            ("'tis", "'tis"),
            ("'tis,", "'tis"),
            ("'tis'", "tis"),
            ("’ndrangheta’", "ndrangheta"),
            ("'word\u{201D}", "'word"),
            ("''tis", "''tis"),
            ("‘ndrangheta", "ndrangheta"),
            ("po'.", "po"),
            ("\u{1D400}bc", "bc"),
            ("a\u{20000}b", "a\u{20000}b"),
            ("cafe\u{301}", "cafe\u{301}"),
            ("Ⅻ", "Ⅻ"),
            ("п'", "п"),
            ("пʼ", "пʼ"),
            ("ab\u{1C90}", "ab\u{1C90}"),
            ("\u{1C90}ab", "\u{1C90}ab"),
            ("kaw\u{1063}", "kaw\u{1063}"),
            ("\u{135F}ab", "ab"),
            ("ab\u{135F}", "ab\u{135F}"),
            ("\u{363}ab", "ab"),
            // Letters and marks as of Unicode 12.1, not later: U+0E4D stays,
            // and U+1DFA, U+0C3C and U+9FF0 go.
            ("ข\u{E4D}", "ข\u{E4D}"),
            ("ab\u{1DFA}", "ab"),
            ("cd\u{C3C}", "cd"),
            ("ef\u{9FF0}", "ef"),
        ] {
            assert_eq!(fmt(word, "de"), want, "{word:?}");
        }
    }

    #[test]
    fn only_italian_french_english_and_portuguese_books_have_rules_of_their_own() {
        let words = [
            "l'altro",
            "quest'anno",
            "qu'il",
            "John's",
            "rendez-vous",
            "dá-me",
            "dar-te-ei",
            "MP3",
        ];
        for language in ["it", "fr", "en", "pt"] {
            assert!(has_language_rules(language));
            assert!(
                words.iter().any(|w| fmt(w, language) != fmt(w, "")),
                "{language}"
            );
        }
        for language in [
            "", "de", "es", "nl", "ru", "uk", "tr", "th", "da", "nb", "ro", "ca",
        ] {
            assert!(!has_language_rules(language));
            for word in words {
                assert_eq!(fmt(word, language), fmt(word, ""), "{language} {word}");
            }
        }
    }

    #[test]
    fn italian_and_french_elision() {
        assert_eq!(fmt("quest'anno", "it"), "anno");
        assert_eq!(fmt("quest’anno", "it"), "anno");
        assert_eq!(fmt("questʼanno", "it"), "questʼanno");
        assert_eq!(fmt("Quest'anno", "it"), "anno");
        assert_eq!(fmt("DELL'ANNO", "it"), "ANNO");
        assert_eq!(fmt("dèll'anno", "it"), "dèll'anno");
        assert_eq!(fmt("tutt'al più", "it"), "tutt'al più");
        assert_eq!(fmt("l'un'altra", "it"), "un'altra");
        assert_eq!(fmt("l 'altro", "it"), "altro");
        assert_eq!(fmt("x'l'altro", "it"), "x'l'altro");
        assert_eq!(fmt("T’shirt", "it"), "shirt");
        assert_eq!(fmt("qu'il", "it"), "qu'il");
        assert_eq!(fmt("qu'il", "fr"), "il");
        assert_eq!(fmt("aujourd'hui", "fr"), "aujourd'hui");
        assert_eq!(fmt("l'altro", "en"), "l'altro");
        assert_eq!(fmt("l'altro", "es"), "l'altro");
    }

    #[test]
    fn english_possessive() {
        assert_eq!(fmt("John's", "en"), "John");
        assert_eq!(fmt("JOHN'S", "en"), "JOHN");
        assert_eq!(fmt("it’s", "en"), "it");
        assert_eq!(fmt("O'Brien's", "en"), "O'Brien");
        assert_eq!(fmt("the dog's.", "en"), "the dog");
        assert_eq!(fmt("'s", "en"), "");
        assert_eq!(fmt("John's", "it"), "John's");
    }

    #[test]
    fn french_and_portuguese_clitics() {
        for (word, want) in [
            ("rendez-vous", "rendez"),
            ("rendez\u{2010}vous", "rendez"),
            ("rendez\u{2011}vous", "rendez"),
            ("rendez–vous", "rendez–vous"),
            ("RENDEZ-VOUS", "RENDEZ"),
            ("va-t-il", "va-t"),
            ("donne-le-moi", "donne"),
            ("dis-le-lui", "dis"),
            ("va-t'en", "va"),
            ("allez-y", "allez"),
            ("a-t-elle", "a-t"),
            ("peut-être", "peut-être"),
            ("dit-on", "dit-on"),
            ("celui-ci", "celui-ci"),
            ("c'est-à-dire", "est-à-dire"),
        ] {
            assert_eq!(fmt(word, "fr"), want, "{word:?}");
        }
        for (word, want) in [
            ("dá-me", "dá"),
            ("fazê-lo", "fazê"),
            ("dá-se-lhe", "dá"),
            ("dar-te-ei", "darei"),
            ("falar-lhe-ia", "falaria"),
            ("dar-se-á", "dará"),
            ("dar-te-ei-o", "darei"),
            ("y-a-t-il", "yt-il"),
            ("dar--ei", "dar--ei"),
            ("ab-te-e", "abe"),
            ("dar-lho", "dar-lho"),
        ] {
            assert_eq!(fmt(word, "pt"), want, "{word:?}");
        }
        assert_eq!(fmt("rendez-vous", "it"), "rendez-vous");
    }

    /// Where the Kindle 4 trims differently, and its stop words.
    #[test]
    fn kindle_4_differences() {
        let k4 = |w: &str| fmt_on(w, "it", Device::Kindle4);
        assert_eq!(k4("cafe\u{301}"), "cafe");
        assert_eq!(
            fmt_on("cafe\u{301}", "it", Device::Firmware5_19_6),
            "cafe\u{301}"
        );
        assert_eq!(k4("Ⅻ"), "");
        assert_eq!(k4("casaⅫ"), "casa");
        assert_eq!(k4("ⓐbc"), "bc");
        assert_eq!(fmt("ⓐbc", "it"), "ⓐbc");
        assert_eq!(k4("casa\u{E31}"), "casa");
        assert_eq!(fmt("casa\u{E31}", "it"), "casa\u{E31}");
        assert_eq!(k4("\u{9BE}casa"), "casa");
        assert_eq!(fmt("\u{9BE}casa", "it"), "\u{9BE}casa");
        assert_eq!(k4(" s\u{903}\u{200D}\u{FEFF}"), "s");
        assert_eq!(fmt(" s\u{903}\u{200D}\u{FEFF}", "it"), "s\u{903}");
        assert_eq!(k4("ab\u{135F}"), "ab");
        assert_eq!(k4("كتابٌ"), "كتاب");
        assert_eq!(k4("राम्"), "राम");
        assert_eq!(fmt("राम्", "it"), "राम्");
        assert_eq!(k4("l'altro"), "altro");
        let stop = |w: &str, l: &str| kindle4_stop_word(&units(w), l);
        assert!(stop("IL", "it"));
        assert!(stop("è", "it"));
        assert!(!stop("È", "fr"));
        assert!(stop("Le", "fr"));
        assert!(stop("Sur", "fr"));
        assert!(!stop("sûr", "fr"));
        assert!(stop("x", "it"));
        assert!(stop("og", "da"));
        assert!(!stop("f", "da"));
        assert!(!stop("og", "nb"));
        assert!(!stop("casa", "it"));
    }
}
