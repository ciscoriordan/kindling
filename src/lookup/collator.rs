//! Whether two words are "the same word" in a book's language, the way a
//! Kindle decides it when a lookup returns several labels and when it trims
//! an elided word or a clitic off a tapped word.
//!
//! The Paperwhite 4 (firmware 5.16.5) and the Paperwhite 5 (firmware 5.19.2)
//! compare the two words with the collation rules of the book's language,
//! ignoring differences of case for every language but German, which compares
//! case as well. The Kindle 4 (firmware 4.1.4) does the same, except that its
//! Danish, Swedish and Turkish rules differ in ways not modeled here: a Danish
//! `Og` equals `og`, a Swedish `aa` equals `å`, and a Turkish `I` equals `i`.
//! Firmware 5.19.6 compares by the default rules whatever the book's language,
//! still with case in German books. By default:
//!
//! - Case is ignored for the ASCII letters and for the accented Latin letters
//!   built from them (`É` equals `é`), but not for `Ø`, `Ł`, `Đ`, `Ħ`, `Ĳ`,
//!   `Ŋ`, most of Latin Extended-B, Greek or Cyrillic. `İ` is `I` followed by
//!   a combining dot above, so it equals `i` followed by U+0307 but not `i`.
//! - Accents always count (`résumé` differs from `resume`).
//! - Hyphens, dashes, the soft hyphen, the minus sign, the spaces, tab and
//!   newline count (`E-mail` differs from `email`, a no-break space from a
//!   space), and so do the apostrophe, the period, every other punctuation
//!   character and the digits.
//! - The zero-width characters U+200B to U+200F and the control characters
//!   are passed over, except that one lined up with a hyphen, a space or a
//!   combining mark in the other word counts as a difference when the
//!   beginning of the tapped word is matched against a shorter label.
//! - `ß` equals `ss`, `æ` `ae`, `œ` `oe` and `þ` `th`; in a German book they
//!   still differ, as case does.
//!
//! Languages that change this:
//!
//! - Greek books ignore the case of Greek letters, and `µ` equals `μ`.
//!   Russian, Ukrainian, Bulgarian, Belarusian, Macedonian and Serbian books
//!   ignore the case of Cyrillic letters, except a few letters of other
//!   Cyrillic alphabets.
//! - Danish books keep the case of `A C D E I O Y` and their accented forms,
//!   other than `Ä`, `Ö` and `Å`; treat `æ` and `ß` as letters of their own;
//!   ignore the case of `ø`; read `aa` in any case as one letter; and read a
//!   capital `Ü` as `y` and a capital `Â`, `Ç`, `Đ`, `Ë`, `Í` or `Ô` as `a`,
//!   `c`, `d`, `e`, `i` or `o`. Swedish books treat `æ` as a letter of its own,
//!   ignore the case of `ø` and read `ü` as `y`; Finnish books ignore the
//!   case of `ø`; Icelandic books treat `æ`, `œ` and `þ` as letters of their
//!   own and ignore the case of `ø`. A Norwegian dictionary's language reads
//!   as Norwegian Bokmål, which the Paperwhite 4 compares by the default
//!   rules.
//! - Turkish books pair `I` with `ı` and `İ` with `i`, so `I` differs from
//!   `i`, and treat `ß`, `æ` and `œ` as letters of their own.
//! - Polish and Slovak books treat `ß`, `æ` and `œ` as letters of their own
//!   and ignore the case of `Ł` and `Đ`; Slovenian books treat `ß` as one and
//!   ignore the case of `Ł`, `Đ` and the DŽ and DZ letters; Croatian books,
//!   and Serbian books, whose language reads as Croatian, treat `ß` as one,
//!   ignore the case of `Đ` and the DŽ and DZ letters, and ignore the caron of
//!   `ď ě ľ ň ř ť ǎ ǐ ǒ ǔ ǚ ǧ ǩ ǰ` and their capitals (`ň` equals `n`, `ǚ`
//!   equals `ü`), though not of `č`, `š` and `ž`; Hungarian books treat `ß`
//!   and `œ` as letters of their own and Romanian books `ß` and `æ`, and both
//!   ignore the case of `Đ`; Lithuanian, Latvian and Estonian books treat `ß`
//!   as a letter of its own, Lithuanian books read `ı` and `İ` as `y`, and
//!   Estonian books ignore the case of `Ƶ`. A Czech book's language reads as
//!   one with no rules of its own, so Czech books compare by the default
//!   rules, where `ß` equals `ss`.
//!
//! Only whether two words are equal is modeled, never which one sorts first.
//! One difference from the devices is known: a precomposed letter other than
//! `İ` does not equal the same letter spelled with a combining mark here (`é`
//! against `e` followed by U+0301), while on the devices it does. Inside one
//! lookup this never matters, since a combining mark weighs as itself in the
//! search and the two spellings are never returned together. `İ` is the
//! exception, and is modeled: a Russian dictionary searches it lowercased, as
//! `i` and U+0307, and then compares the labels found with the word as it was
//! tapped.

use super::Device;
use crate::ordt::device_weight;

/// One collation element: a base-letter weight, an accent or variant weight,
/// and a case weight. An element whose primary weight is 0 counts only as a
/// secondary difference (a space, a hyphen, a combining mark), and the all-zero
/// element is passed over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Element {
    primary: u32,
    secondary: u32,
    tertiary: u8,
}

const IGNORABLE: Element = Element {
    primary: 0,
    secondary: 0,
    tertiary: 0,
};

fn letter(primary: u32, tertiary: u8) -> Element {
    Element {
        primary,
        secondary: 0,
        tertiary,
    }
}

fn secondary(unit: u16) -> Element {
    Element {
        primary: 0,
        secondary: u32::from(unit),
        tertiary: 0,
    }
}

/// How one book language's rules differ from the default ones, as far as
/// this model covers.
#[derive(Clone, Copy, Debug, Default)]
struct Tailoring {
    /// `ß`, `æ`, `œ` and `þ` compare as letters of their own, not as `ss`,
    /// `ae`, `oe` and `th`.
    own_sharp_s: bool,
    own_ae: bool,
    own_oe: bool,
    own_thorn: bool,
    /// The base letters whose capitals, with or without accents, differ from
    /// their lowercase forms.
    capital_bases: &'static [u8],
    /// Greek and Cyrillic letters ignore case.
    greek_case: bool,
    cyrillic_case: bool,
    /// `ø`, `Ł`, `Đ`, `Ƶ` and the DŽ and DZ digraph letters ignore case.
    fold_o_stroke: bool,
    fold_l_stroke: bool,
    fold_d_stroke: bool,
    fold_z_stroke: bool,
    fold_digraphs: bool,
    /// Danish: `aa` in any case is `ǻ`, a capital `Ü` is `y`, and `Ð` keeps
    /// its case.
    danish: bool,
    /// Swedish: `ü` in either case is `y`.
    swedish: bool,
    /// Turkish: `I` pairs with `ı` and `İ` with `i`, and `Ĳ` ignores case.
    turkish: bool,
    /// Croatian: the caron of the letters [`caron_base`] lists is ignored.
    ignore_caron: bool,
    /// Lithuanian: `ı` and `İ` are `y`.
    lithuanian: bool,
}

impl Tailoring {
    fn of(language: &str) -> Tailoring {
        let base = Tailoring::default();
        match language {
            "el" => Tailoring {
                greek_case: true,
                ..base
            },
            "ru" | "uk" | "bg" | "be" | "mk" | "sr" => Tailoring {
                cyrillic_case: true,
                ..base
            },
            "da" => Tailoring {
                own_sharp_s: true,
                own_ae: true,
                capital_bases: b"acdeioy",
                fold_o_stroke: true,
                danish: true,
                ..base
            },
            "sv" => Tailoring {
                own_ae: true,
                fold_o_stroke: true,
                swedish: true,
                ..base
            },
            "fi" => Tailoring {
                fold_o_stroke: true,
                ..base
            },
            "is" => Tailoring {
                own_ae: true,
                own_oe: true,
                own_thorn: true,
                fold_o_stroke: true,
                ..base
            },
            "tr" => Tailoring {
                own_sharp_s: true,
                own_ae: true,
                own_oe: true,
                capital_bases: b"i",
                turkish: true,
                ..base
            },
            "pl" | "sk" => Tailoring {
                own_sharp_s: true,
                own_ae: true,
                own_oe: true,
                fold_l_stroke: true,
                fold_d_stroke: true,
                ..base
            },
            "sl" => Tailoring {
                own_sharp_s: true,
                fold_l_stroke: true,
                fold_d_stroke: true,
                fold_digraphs: true,
                ..base
            },
            "hr" => Tailoring {
                own_sharp_s: true,
                fold_d_stroke: true,
                fold_digraphs: true,
                ignore_caron: true,
                ..base
            },
            "hu" => Tailoring {
                own_sharp_s: true,
                own_oe: true,
                fold_d_stroke: true,
                ..base
            },
            "ro" => Tailoring {
                own_sharp_s: true,
                own_ae: true,
                fold_d_stroke: true,
                ..base
            },
            "lv" => Tailoring {
                own_sharp_s: true,
                ..base
            },
            "lt" => Tailoring {
                own_sharp_s: true,
                lithuanian: true,
                ..base
            },
            "et" => Tailoring {
                own_sharp_s: true,
                fold_z_stroke: true,
                ..base
            },
            _ => base,
        }
    }
}

/// Word equality under the collation rules of one book language.
#[derive(Clone, Copy, Debug)]
pub(super) struct Collator {
    tailoring: Tailoring,
    tertiary: bool,
}

impl Collator {
    /// The collator a Kindle uses to pick among the labels a lookup returned:
    /// tertiary strength for German books, secondary for every other
    /// language.
    pub(super) fn for_choice(language: &str, device: Device) -> Collator {
        let mut collator = Collator::secondary(language, device);
        collator.tertiary = language == "de";
        collator
    }

    /// The same rules at secondary strength, which is how a Kindle compares a
    /// word with its lists of elided words and clitics whatever the
    /// language. Firmware 5.19.6 takes the default rules for every language.
    pub(super) fn secondary(language: &str, device: Device) -> Collator {
        let rules = match device {
            Device::Firmware5_19_6 => "",
            Device::Kindle4 | Device::Paperwhite5 => language,
        };
        Collator {
            tailoring: Tailoring::of(rules),
            tertiary: false,
        }
    }

    /// Whether `a` and `b` have equal sort keys: the comparison a Kindle
    /// makes when it matches a word against a whole label, or against the
    /// beginning of a longer label. Here a zero-width or control character
    /// is left out entirely, even next to a hyphen, a space or a combining
    /// mark, where [`Collator::equal`] counts it as a difference.
    pub(super) fn keys_equal(&self, a: &[u16], b: &[u16]) -> bool {
        if a == b {
            return true;
        }
        let kept = |units| {
            self.elements(units)
                .into_iter()
                .filter(|&element| element != IGNORABLE)
        };
        kept(a).eq(kept(b))
    }

    /// Whether `a` and `b`, as UTF-16 code units, compare equal.
    pub(super) fn equal(&self, a: &[u16], b: &[u16]) -> bool {
        if a == b {
            return true;
        }
        let a = self.elements(a);
        let b = self.elements(b);
        elements_equal(&a, &b)
    }

    fn elements(&self, units: &[u16]) -> Vec<Element> {
        let mut out = Vec::with_capacity(units.len() + 2);
        let mut i = 0;
        while i < units.len() {
            let is_a = |u: u16| matches!(u, 0x41 | 0x61);
            if self.tailoring.danish && is_a(units[i]) && units.get(i + 1).is_some_and(|&u| is_a(u))
            {
                out.push(letter(0x1FB, 0));
                i += 2;
                continue;
            }
            self.push_elements(units[i], &mut out);
            i += 1;
        }
        if !self.tertiary {
            for element in &mut out {
                element.tertiary = 0;
            }
        }
        out
    }

    fn push_elements(&self, unit: u16, out: &mut Vec<Element>) {
        let t = &self.tailoring;
        let upper = |u: u16| u8::from(char::from_u32(u32::from(u)).is_some_and(char::is_uppercase));
        let own = |lower: u16| letter(u32::from(lower), upper(unit));
        let pair = |first: u8, second: u8, case: u8| {
            [
                letter(u32::from(first), case),
                letter(u32::from(second), case),
            ]
        };
        match unit {
            0x0000..=0x0008 | 0x000E..=0x001F | 0x007F..=0x009F | 0x200B..=0x200F => {
                out.push(IGNORABLE)
            }
            // Spaces, tab and newline, the combining marks and the hyphens
            // weigh only as secondary differences. The hyphen-minus is a case
            // variant of U+20E1, so at secondary strength the two are equal.
            0x0009..=0x000D | 0x0020 | 0x00A0 | 0x2000..=0x200A | 0x3000 | 0xFEFF => {
                out.push(secondary(unit))
            }
            0x0300..=0x033F | 0x0342 | 0x0344 | 0x0345 | 0x0360 | 0x0361 => {
                out.push(secondary(unit))
            }
            0x0340 => out.push(secondary(0x0300)),
            0x0341 => out.push(secondary(0x0301)),
            0x0343 => out.push(secondary(0x0313)),
            0x0483..=0x0486 | 0x20D0..=0x20E1 => out.push(secondary(unit)),
            0x002D => out.push(Element {
                tertiary: 1,
                ..secondary(0x20E1)
            }),
            0x00AD | 0x2010..=0x2015 | 0x2212 => out.push(secondary(unit)),
            // ß, æ, œ and þ are two letters with a case variant of their
            // own, except where a language makes one a letter of its own.
            0x00DF if t.own_sharp_s => out.push(letter(0xDF, 0)),
            0x00DF => out.extend(pair(b's', b's', 2)),
            0x00C6 | 0x00E6 if t.own_ae => out.push(own(0xE6)),
            0x00E6 => out.extend(pair(b'a', b'e', 2)),
            0x00C6 => out.extend(pair(b'a', b'e', 3)),
            0x0152 | 0x0153 if t.own_oe => out.push(own(0x153)),
            0x0153 => out.extend(pair(b'o', b'e', 2)),
            0x0152 => out.extend(pair(b'o', b'e', 3)),
            0x00DE | 0x00FE if t.own_thorn => out.push(own(0xFE)),
            0x00FE => out.extend(pair(b't', b'h', 2)),
            0x00DE => out.extend(pair(b't', b'h', 3)),
            0x00D0 if t.danish => out.push(letter(0xD0, 0)),
            0x00D0 | 0x00F0 => out.push(own(0xF0)),
            0x00B5 if t.greek_case => out.push(letter(0x3BC, 0)),
            // `İ` is `I` followed by a combining dot above, so `i` followed by
            // U+0307 equals it wherever the case of `I` is ignored.
            0x0130 if !t.turkish && !t.lithuanian => {
                out.push(self.letter_or_other(0x49));
                out.push(secondary(0x0307));
            }
            // A caron the rules ignore still passes over like a zero-width
            // character, so it counts next to a combining mark in a direct
            // comparison.
            _ => match caron_base(unit).filter(|_| t.ignore_caron) {
                Some(base) => {
                    self.push_elements(base, out);
                    out.push(IGNORABLE);
                }
                None => out.push(self.letter_or_other(unit)),
            },
        }
    }

    /// The element of a unit that is not one of the fixed cases above: a
    /// letter whose case the rules ignore weighs as its lowercase form with
    /// the case as a tertiary difference, and every other unit weighs as
    /// itself.
    fn letter_or_other(&self, unit: u16) -> Element {
        let t = &self.tailoring;
        let own = letter(u32::from(unit), 0);
        let Some(c) = char::from_u32(u32::from(unit)) else {
            return own;
        };
        match c {
            'İ' if t.turkish => return letter(u32::from(b'i'), 1),
            'I' if t.turkish => return letter(0x131, 1),
            'ı' if t.turkish => return letter(0x131, 0),
            'ı' if t.lithuanian => return letter(u32::from(b'y'), 0),
            'İ' if t.lithuanian => return letter(u32::from(b'y'), 1),
            // A Danish capital Â, Ç, Đ, Ë, Í or Ô is its lowercase base letter.
            'Â' | 'Ç' | 'Đ' | 'Ë' | 'Í' | 'Ô' if t.danish => {
                let base = match c {
                    'Â' => b'a',
                    'Ç' => b'c',
                    'Đ' => b'd',
                    'Ë' => b'e',
                    'Í' => b'i',
                    _ => b'o',
                };
                return letter(u32::from(base), 1);
            }
            _ => {}
        }
        let mut lower = c.to_lowercase();
        let (Some(lowered), None) = (lower.next(), lower.next()) else {
            return own;
        };
        let case = u8::from(lowered != c);
        let folded = letter(lowered as u32, case);
        let base = latin_base_letter(unit);
        if case == 1 && base.is_some_and(|b| t.capital_bases.contains(&b)) {
            // Danish Ä, Ö, Ø and Å are variants of its letters Æ, Ø and Å,
            // whose case is ignored, and Danish capital Ü is a y.
            let danish_vowel = matches!(c, 'Ä' | 'Ö' | 'Å' | 'Ő' | 'Ǻ' | 'Ǣ' | 'Ǽ');
            if !(t.danish && danish_vowel) {
                return own;
            }
        }
        let case_ignored = match c {
            'Ü' if t.danish => return letter(u32::from(b'y'), 1),
            'Ü' | 'ü' if t.swedish => return letter(u32::from(b'y'), case),
            'Ĳ' | 'ĳ' => t.turkish,
            'Ø' | 'ø' | 'Ǿ' | 'ǿ' => t.fold_o_stroke,
            'Ł' | 'ł' => t.fold_l_stroke,
            'Đ' | 'đ' => t.fold_d_stroke,
            'Ƶ' | 'ƶ' => t.fold_z_stroke,
            'Ǆ' | 'ǅ' | 'ǆ' | 'Ǳ' | 'ǲ' | 'ǳ' => t.fold_digraphs,
            _ => {
                (t.greek_case && greek_letter_ignores_case(unit))
                    || (t.cyrillic_case && cyrillic_letter_ignores_case(unit))
                    || base.is_some()
            }
        };
        match c {
            'ǅ' | 'ǲ' if case_ignored => letter(u32::from(unit) + 1, 1),
            'ς' | 'Σ' if case_ignored => letter(0x3C3, case),
            _ if case_ignored => folded,
            _ => own,
        }
    }
}

/// The letter without its caron, for the letters whose caron a Croatian book
/// ignores: `ď ě ľ ň ř ť ǎ ǐ ǒ ǔ ǚ ǧ ǩ ǰ` and their capitals (`Ň` is `N`,
/// `ǚ` is `ü`). `č`, `š` and `ž` are letters of their own there.
fn caron_base(unit: u16) -> Option<u16> {
    let (first, capital_base) = match unit {
        0x010E..=0x010F => (0x010E, b'D'),
        0x011A..=0x011B => (0x011A, b'E'),
        0x013D..=0x013E => (0x013D, b'L'),
        0x0147..=0x0148 => (0x0147, b'N'),
        0x0158..=0x0159 => (0x0158, b'R'),
        0x0164..=0x0165 => (0x0164, b'T'),
        0x01CD..=0x01CE => (0x01CD, b'A'),
        0x01CF..=0x01D0 => (0x01CF, b'I'),
        0x01D1..=0x01D2 => (0x01D1, b'O'),
        0x01D3..=0x01D4 => (0x01D3, b'U'),
        0x01D9..=0x01DA => (0x01D9, 0xDC),
        0x01E6..=0x01E7 => (0x01E6, b'G'),
        0x01E8..=0x01E9 => (0x01E8, b'K'),
        0x01F0 => (0x01F0, b'j'),
        _ => return None,
    };
    Some(u16::from(capital_base) + 0x20 * (unit - first))
}

/// The ASCII letter a Latin letter is built from by adding accents, for the
/// ASCII letters themselves and the Latin letters with a canonical
/// decomposition whose base letter is ASCII (`é`, `Ǻ`, `ạ`), or `None`. `İ`
/// is left out: its lowercase form is `i` with a dot above, which is not `i`.
fn latin_base_letter(unit: u16) -> Option<u8> {
    /// The base letters of U+1E00 to U+1EF9, one per code point, with `.`
    /// for the letters that are not built on an ASCII letter.
    const EXTENDED_ADDITIONAL: &[u8; 250] = b"\
        aabbbbbbccddddddddddeeeeeeeeeeffgghhhhhhhhhhiiiikkkkkkllllllllmm\
        mmmmnnnnnnnnoooooooopppprrrrrrrrssssssssssttttttttuuuuuuuuuuvvvv\
        wwwwwwwwwwxxxxyyzzzzzzhtwy......aaaaaaaaaaaaaaaaaaaaaaaaeeeeeeee\
        eeeeeeeeiiiioooooooooooooooooooooooouuuuuuuuuuuuuuyyyyyyyy";
    let base = match unit {
        0x0041..=0x005A | 0x0061..=0x007A => unit as u8 | 0x20,
        0x01E2..=0x01E3 | 0x01FD => b'a',
        0x01F8..=0x01F9 => b'n',
        0x0218..=0x0219 => b's',
        0x021A..=0x021B => b't',
        0x021E..=0x021F => b'h',
        0x0226..=0x0233 => b"aaeeooooooooyy"[usize::from(unit - 0x0226)],
        0x1E00..=0x1EF9 => EXTENDED_ADDITIONAL[usize::from(unit - 0x1E00)],
        _ if builds_on_an_ascii_letter(unit) => u8::try_from(device_weight(unit)).ok()?,
        _ => return None,
    };
    base.is_ascii_lowercase().then_some(base)
}

/// Whether a Latin letter below U+0226 is built from an ASCII letter by adding
/// accents (or from `æ`, whose base letter is taken to be `a`).
fn builds_on_an_ascii_letter(unit: u16) -> bool {
    matches!(
        unit,
        0x00C0..=0x00C5
            | 0x00C7..=0x00CF
            | 0x00D1..=0x00D6
            | 0x00D9..=0x00DD
            | 0x00E0..=0x00E5
            | 0x00E7..=0x00EF
            | 0x00F1..=0x00F6
            | 0x00F9..=0x00FD
            | 0x00FF..=0x010F
            | 0x0112..=0x0125
            | 0x0128..=0x012F
            | 0x0134..=0x0137
            | 0x0139..=0x013E
            | 0x0143..=0x0148
            | 0x014C..=0x0151
            | 0x0154..=0x0165
            | 0x0168..=0x017E
            | 0x01A0..=0x01A1
            | 0x01AF..=0x01B0
            | 0x01CD..=0x01DC
            | 0x01DE..=0x01E3
            | 0x01E6..=0x01ED
            | 0x01F0
            | 0x01F4..=0x01F5
            | 0x01F8..=0x01FD
            | 0x0200..=0x021B
            | 0x021E..=0x021F
    )
}

/// The Greek letters whose case a Greek book ignores: the modern alphabet
/// with its accented forms, the Coptic letters U+03E2 to U+03EF and the
/// polytonic letters. The archaic and variant letters keep their case.
fn greek_letter_ignores_case(unit: u16) -> bool {
    matches!(
        unit,
        0x0386 | 0x0388..=0x038A | 0x038C | 0x038E..=0x03A1 | 0x03A3..=0x03CE | 0x03E2..=0x03EF
            | 0x1F00..=0x1FFF
    )
}

/// The Cyrillic letters whose case a Russian, Ukrainian, Bulgarian,
/// Belarusian, Macedonian or Serbian book ignores: all of U+0400 to U+04F9
/// except a few letters of other Cyrillic alphabets.
fn cyrillic_letter_ignores_case(unit: u16) -> bool {
    matches!(unit, 0x0400..=0x04F9)
        && !matches!(
            unit,
            0x0462..=0x0463
                | 0x048A..=0x048F
                | 0x04B6..=0x04B7
                | 0x04C0
                | 0x04C5..=0x04C6
                | 0x04C9..=0x04CA
                | 0x04CD..=0x04CF
                | 0x04D4..=0x04D5
                | 0x04D8..=0x04DB
                | 0x04E0..=0x04E1
                | 0x04E8..=0x04EB
                | 0x04F6..=0x04F7
        )
}

/// Whether two element sequences are equal: the base weights must agree
/// element for element, and so must the secondary (and, when kept, tertiary)
/// weights. An element that counts only as a secondary difference, on one
/// side only, makes the words differ; an ignorable element on one side is
/// passed over, unless it lines up with such a secondary-only element on the
/// other, where its missing secondary weight counts as a difference.
fn elements_equal(a: &[Element], b: &[Element]) -> bool {
    let (mut i, mut j) = (0, 0);
    let mut differ = false;
    while i < a.len() && j < b.len() {
        let (x, y) = (a[i], b[j]);
        if x == y {
            i += 1;
            j += 1;
        } else if x.primary != y.primary {
            if x == IGNORABLE {
                i += 1;
            } else if y == IGNORABLE {
                j += 1;
            } else if x.primary == 0 {
                differ = true;
                i += 1;
            } else if y.primary == 0 {
                differ = true;
                j += 1;
            } else {
                return false;
            }
        } else {
            differ = true;
            i += 1;
            j += 1;
        }
    }
    for rest in [&a[i..], &b[j..]] {
        for element in rest {
            if element.primary != 0 {
                return false;
            }
            if element.secondary != 0 {
                differ = true;
            }
        }
    }
    !differ
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(language: &str, a: &str, b: &str) -> bool {
        eq_on(language, a, b, Device::Paperwhite5)
    }

    fn eq_on(language: &str, a: &str, b: &str, device: Device) -> bool {
        let a: Vec<u16> = a.encode_utf16().collect();
        let b: Vec<u16> = b.encode_utf16().collect();
        Collator::for_choice(language, device).equal(&a, &b)
    }

    /// The pairs measured on both devices, at secondary strength (English)
    /// and tertiary strength (German).
    #[test]
    fn case_accents_and_punctuation() {
        assert!(eq("en", "CAN'T", "can't"));
        assert!(!eq("en", "CAN'T", "cant"));
        assert!(!eq("en", "E-MAIL", "email"));
        assert!(eq("en", "E-MAIL", "e-mail"));
        assert!(eq("en", "FUSS", "Fuß"));
        assert!(!eq("en", "a\u{2010}b", "ab"));
        assert!(eq("en", "a\u{200B}b", "ab"));
        assert!(eq("it", "RÉSUMÉ", "résumé"));
        assert!(!eq("it", "Resumé", "résumé"));
        assert!(!eq("en", "ice\u{A0}cream", "ice cream"));
        assert!(eq("en", "OEUF", "œuf"));
        assert!(eq("en", "ÆSC", "aesc"));
        assert!(!eq("de", "CAN'T", "can't"));
        assert!(!eq("de", "FUSS", "Fuß"));
        assert!(eq("de", "Fuß", "Fuß"));
        assert!(!eq("de", "ÜBER", "Über"));
    }

    /// The case pairs that still differ, and the languages that fold them.
    #[test]
    fn case_that_counts_by_language() {
        for (a, b) in [("Ø", "ø"), ("Ł", "ł"), ("Đ", "đ"), ("Ħ", "ħ"), ("İ", "i")] {
            assert!(!eq("en", a, b), "{a} {b}");
        }
        assert!(eq("pl", "Łódź", "łódź"));
        assert!(!eq("en", "ΑΛΛΟΣ", "αλλος"));
        assert!(eq("el", "ΑΛΛΟΣ", "αλλος"));
        assert!(eq("el", "ΆΛΛΟΣ", "άλλοσ"));
        assert!(!eq("el", "ΑΛΛΟΣ", "άλλος"));
        assert!(!eq("en", "ДОМ", "дом"));
        assert!(eq("ru", "ДОМ", "дом"));
        assert!(eq("uk", "КИЇВ", "київ"));
        assert!(!eq("da", "CAN'T", "can't"));
        assert!(!eq("da", "BOG", "bog"));
        assert!(eq("da", "BURG", "burg"));
        assert!(!eq("da", "aeble", "æble"));
        assert!(eq("en", "aeble", "æble"));
        assert!(eq("da", "AARHUS", "aarhus"));
        assert!(
            eq("nb", "ÆRE", "aere"),
            "a Norwegian book compares by the default rules"
        );
        assert!(!eq("sv", "aeble", "æble"));
        assert!(!eq("pl", "straße", "strasse"));
        assert!(
            eq("cs", "straße", "strasse"),
            "a Czech book compares by the default rules"
        );
        assert!(eq("pl", "straße", "Straße"));
        assert!(!eq("tr", "I", "i"));
        assert!(eq("tr", "I", "ı"));
        assert!(eq("tr", "İ", "i"));
        assert!(eq("hr", "PLZEŇ", "Plzen"));
        let hr = Collator::for_choice("hr", Device::Paperwhite5);
        let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert!(!hr.equal(&units("ǰ\u{301}"), &units("J\u{301}")));
        assert!(hr.keys_equal(&units("ǰ\u{301}"), &units("J\u{301}")));
        assert!(eq("hr", "kǚm", "KÜM"));
        assert!(!eq("hr", "ČAS", "cas"));
        assert!(eq("da", "Ë-le", "e-le"));
        assert!(!eq("da", "É-le", "e-le"));
        assert!(!eq("da", "Ë-le", "ë-le"));
        assert!(eq("lt", "İA", "ıa"));
        // Every caron a Croatian book ignores, in either case.
        for (a, b) in [
            ("ĎQ", "dq"),
            ("ěq", "eq"),
            ("ĽQ", "lq"),
            ("ňq", "nq"),
            ("Řq", "rq"),
            ("ťq", "tq"),
            ("ŤAK", "tak"),
            ("ǎq", "aq"),
            ("Ǐq", "iq"),
            ("ǒq", "oq"),
            ("Ǔq", "uq"),
            ("ǚq", "üq"),
            ("ǧq", "gq"),
            ("Ǧak", "gak"),
            ("Ǩq", "kq"),
            ("ǰq", "jq"),
        ] {
            assert!(eq("hr", a, b), "{a} {b}");
        }
        assert!(!eq("hr", "šq", "sq"));
        // Each Danish capital that reads as its base letter.
        for (a, b) in [
            ("Âb", "ab"),
            ("Çb", "cb"),
            ("Đb", "db"),
            ("Ëb", "eb"),
            ("Íb", "ib"),
            ("Ôb", "ob"),
        ] {
            assert!(eq("da", a, b), "{a} {b}");
        }
        assert!(!eq("da", "Ôb", "eb"));
        assert!(eq("et", "Ƶa", "ƶa"));
        for (a, b) in [("Ǆa", "ǆa"), ("ǅa", "ǆa"), ("Ǳe", "ǳe"), ("ǆo", "ǅo")] {
            assert!(eq("hr", a, b), "{a} {b}");
        }
        assert!(eq("hu", "Œuf", "œuf"));
        assert!(!eq("hu", "Œuf", "oeuf"));
        assert!(!eq("ru", "ӘБ", "әб"));
        assert!(!eq("ru", "ӨБ", "өб"));
    }

    /// `İ` is `I` and a combining dot above: equal to `i` and U+0307 where
    /// the case of `I` is ignored, and never equal to `i`.
    #[test]
    fn dotted_capital_i_is_i_and_a_dot() {
        for language in ["en", "ru", "el", "fr"] {
            assert!(eq(language, "Z³İS", "z³i\u{307}s"), "{language}");
            assert!(eq(language, "İ", "I\u{307}"), "{language}");
            assert!(!eq(language, "z³İs", "z3i\u{307}s"), "{language}");
        }
        assert!(!eq("da", "İ", "i\u{307}"));
        assert!(eq("da", "İ", "I\u{307}"));
        assert!(eq("de", "İ", "I\u{307}"));
        assert!(!eq("de", "İ", "i\u{307}"));
    }

    /// Firmware 5.19.6 compares by the default rules in every language, with
    /// case still counting in German books.
    #[test]
    fn firmware_5_19_6_compares_by_the_default_rules() {
        let old = |l: &str, a: &str, b: &str| eq_on(l, a, b, Device::Paperwhite5);
        let new = |l: &str, a: &str, b: &str| eq_on(l, a, b, Device::Firmware5_19_6);
        assert!(old("pl", "WYKŁADNIĘ", "wykładnię"));
        assert!(!new("pl", "WYKŁADNIĘ", "wykładnię"));
        assert!(old("ru", "Абду-Салам", "абду-салам"));
        assert!(!new("ru", "Абду-Салам", "абду-салам"));
        assert!(!old("tr", "ABIŞLERINDE", "abişlerinde"));
        assert!(new("tr", "ABIŞLERINDE", "abişlerinde"));
        assert!(!old("da", "HØST", "host"));
        assert!(old("da", "HØST", "høst"));
        assert!(!new("da", "HØST", "høst"));
        assert!(!new("de", "E-MAIL", "e-mail"));
    }
}
