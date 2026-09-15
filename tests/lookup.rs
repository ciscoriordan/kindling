//! End-to-end tests for the lookup simulator (`kindling::lookup`).
//!
//! Each test builds a dictionary with kindling and asserts that the simulator
//! opens the label the Paperwhite 4 (firmware 5.16.5) opens for each tapped
//! word, or nothing where it opens nothing. Every expectation here was
//! measured on the Paperwhite 4, and on the Kindle 4 (firmware 4.1.4), which
//! opens the same labels for these words except where a test says otherwise.

mod common;

use std::path::Path;
use std::process::Command;

use common::{kindling_bin, kindling_build};
use kindling::lookup::{lookup, report};

fn build_ru_strict(out: &Path) {
    let opf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/langs/ru/src/dict.opf");
    let status = Command::new(kindling_bin())
        .arg("build")
        .arg(&opf)
        .arg("-o")
        .arg(out)
        .arg("--no-validate")
        .env("KINDLING_STRICT_ACCENTS", "1")
        .output()
        .expect("spawn kindling-cli");
    assert!(
        status.status.success(),
        "strict build failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("kindling_lookup_test");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn build(code: &str, out: &Path) {
    let opf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/langs")
        .join(code)
        .join("src/dict.opf");
    kindling_build(&opf, out);
}

/// Builds a small dictionary from `words` (one entry each) and returns its bytes.
fn build_words(tag: &str, lang: &str, words: &[&str], extra_args: &[&str]) -> Vec<u8> {
    build_words_logged(tag, lang, words, extra_args).0
}

/// [`build_words`], also returning what the build wrote to stderr.
fn build_words_logged(
    tag: &str,
    lang: &str,
    words: &[&str],
    extra_args: &[&str],
) -> (Vec<u8>, String) {
    let dir = std::env::temp_dir().join(format!("kindling_lookup_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let entries: String = words
        .iter()
        .map(|w| {
            format!(
                "<idx:entry name=\"default\" scriptable=\"yes\"><idx:orth value=\"{w}\"><b>{w}</b></idx:orth><p>gloss of {w}</p></idx:entry><mbp:pagebreak/>\n"
            )
        })
        .collect();
    std::fs::write(
        dir.join("content.html"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:idx=\"http://www.mobipocket.com/idx\" xmlns:mbp=\"http://www.mobipocket.com\"><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"/><title>{tag}</title></head><body><mbp:frameset>\n{entries}</mbp:frameset></body></html>"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("dict.opf"),
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<package version=\"2.0\" xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"BookId\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:title>{tag}</dc:title><dc:language>{lang}</dc:language><dc:identifier id=\"BookId\">{tag}</dc:identifier><x-metadata><DictionaryInLanguage>{lang}</DictionaryInLanguage><DictionaryOutLanguage>en</DictionaryOutLanguage><DefaultLookupIndex>default</DefaultLookupIndex></x-metadata></metadata><manifest><item id=\"content\" href=\"content.html\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"content\"/></spine></package>"),
    )
    .unwrap();
    let out = dir.join("out.mobi");
    let status = Command::new(kindling_bin())
        .arg("build")
        .arg(dir.join("dict.opf"))
        .arg("-o")
        .arg(&out)
        .arg("--no-validate")
        .args(extra_args)
        .output()
        .expect("spawn kindling-cli");
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let data = std::fs::read(&out).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    (data, String::from_utf8_lossy(&status.stderr).into_owned())
}

/// `kindling lookup` on `data`: its stdout and stderr.
fn cli_lookup(tag: &str, data: &[u8], word: &str) -> (String, String) {
    let path = tmp(&format!("{tag}_{}.mobi", std::process::id()));
    std::fs::write(&path, data).unwrap();
    let out = Command::new(kindling_bin())
        .arg("lookup")
        .arg(&path)
        .arg("--")
        .arg(word)
        .output()
        .expect("spawn kindling-cli");
    std::fs::remove_file(&path).ok();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The label the simulator opens for `word`, if any.
fn opens(data: &[u8], word: &str) -> Option<String> {
    lookup(data, word).map(|r| r.matched_label)
}

#[test]
fn russian_alias_resolves_lowercase_headword() {
    // The ru fixture has the mixed-case headword "Москва"; issue #17 adds the
    // lowercased "москва" as an alias pointing at the same entry. The
    // Paperwhite 4 lowercases every word looked up in a Russian dictionary,
    // so both spellings open the alias. The Kindle 4 does not: there
    // "Москва" opens the headword and "ВОДА" opens nothing.
    let out = tmp("ru.mobi");
    build("ru", &out);
    let data = std::fs::read(&out).unwrap();

    let upper = lookup(&data, "Москва").expect("headword resolves");
    let lower = lookup(&data, "москва").expect("lowercase alias resolves");
    assert_eq!(upper.matched_label, "москва");
    assert_eq!(
        upper.position, lower.position,
        "alias must point at the same entry as the headword"
    );
    assert!(lookup(&data, "ВОДА").is_some(), "вода reachable from ВОДА");
    assert!(lookup(&data, "несуществующее").is_none(), "nonsense misses");
}

#[test]
fn russian_strict_accents_keeps_only_the_lowercase_alias() {
    // Same fixture, built with --strict-accents. The Paperwhite 4 (firmware
    // 5.16.5) lowercases every word looked up in a Russian dictionary, so the
    // lowercase alias stays and points at the headword's entry.
    let out = tmp("ru-strict.mobi");
    build_ru_strict(&out);
    let data = std::fs::read(&out).unwrap();
    let upper = lookup(&data, "Москва").expect("headword still resolves");
    let lower = lookup(&data, "москва").expect("lowercase alias kept");
    assert_eq!(upper.position, lower.position);

    // The stress-stripped alias is still left out, and so is every alias of a
    // Ukrainian dictionary, which no Kindle lowercases.
    let stressed = ["за́мок", "дом"];
    let strict = build_words("ru_stress_strict", "ru", &stressed, &["--strict-accents"]);
    assert!(
        lookup(&strict, "за́мок").is_some(),
        "stressed headword resolves"
    );
    assert!(
        lookup(&strict, "замок").is_none(),
        "without the stress alias, the bare form must miss"
    );
    let default = build_words("ru_stress_default", "ru", &stressed, &[]);
    assert!(
        lookup(&default, "замок").is_some(),
        "the default build adds it"
    );
    let uk = build_words("uk_strict", "uk", &["Київ", "місто"], &["--strict-accents"]);
    assert!(lookup(&uk, "Київ").is_some());
    assert!(
        lookup(&uk, "київ").is_none(),
        "no lowercase alias for Ukrainian"
    );
}

#[test]
fn russian_lowercases_the_word_and_ukrainian_does_not() {
    // On the Paperwhite 4 only; the Kindle 4 opens nothing for "ДОМ" or "Дом"
    // in the Russian dictionary.
    let ru = build_words("ru_case", "ru", &["дом", "изба"], &[]);
    assert_eq!(opens(&ru, "ДОМ").as_deref(), Some("дом"));
    assert_eq!(opens(&ru, "Дом").as_deref(), Some("дом"));
    let uk = build_words("uk_case", "uk", &["Дом", "дом"], &[]);
    assert_eq!(opens(&uk, "ДОМ"), None);
    assert_eq!(opens(&uk, "Дом").as_deref(), Some("Дом"));
}

#[test]
fn french_accent_and_case_fold() {
    // Latin letters fold case and accents in the search, so an unaccented or
    // uppercased word opens the accented headword, and a non-headword misses.
    let out = tmp("fr.mobi");
    build("fr", &out);
    let data = std::fs::read(&out).unwrap();

    let exact = lookup(&data, "rivière").expect("exact accented headword");
    assert_eq!(exact.matched_label, "rivière");
    let folded = lookup(&data, "riviere").expect("unaccented folds to accented");
    assert_eq!(folded.matched_label, "rivière");
    let cased = lookup(&data, "RIVIÈRE").expect("uppercase folds to headword");
    assert_eq!(cased.matched_label, "rivière");
    assert!(lookup(&data, "zzzzz").is_none(), "non-headword misses");
}

#[test]
fn italian_words_are_formatted_and_twins_chosen_by_case() {
    let words = [
        "T-shirt",
        "tshirt",
        "E-mail",
        "email",
        "can't",
        "cant",
        "résumé",
        "resume",
        "tabacco",
        "tutt'al più",
        "ex",
        "ex-",
        "posta",
        "altro",
        "l'altro",
        "anno",
        "quest'anno",
        "casa",
        "MP",
        "MP3",
    ];
    let data = build_words("it_twins", "it", &words, &[]);

    // Among labels that weigh the same, the one equal to the word ignoring
    // case, but not punctuation or accents, opens.
    for (word, want) in [
        ("T-SHIRT", "T-shirt"),
        ("TSHIRT", "tshirt"),
        ("tshirt", "tshirt"),
        ("CAN'T", "can't"),
        ("E-MAIL", "E-mail"),
        ("RÉSUMÉ", "résumé"),
        ("Resumé", "resume"),
        ("tuttal più", "tutt'al più"),
    ] {
        assert_eq!(opens(&data, word).as_deref(), Some(want), "{word}");
    }
    let r = report(&data, "T-SHIRT");
    assert_eq!(r.run, ["tshirt", "T-shirt"], "stored order of the twins");

    // The tapped word is formatted for an Italian book first: trailing
    // punctuation and digits go, and so does an elided word.
    for (word, want) in [
        ("ex-", "ex"),
        ("ex", "ex"),
        ("l'altro", "altro"),
        ("quest'anno", "anno"),
        ("(casa)", "casa"),
        ("MP3", "MP"),
    ] {
        assert_eq!(opens(&data, word).as_deref(), Some(want), "{word}");
    }
    assert_eq!(report(&data, "l’altro").formatted, "altro");
    for word in ["123", "...", "-", "tsh", "tabacchi"] {
        assert_eq!(opens(&data, word), None, "{word}");
    }
    assert_eq!(report(&data, "...").formatted, "");

    // The nearest-headword hint follows the stored order: "tr" lands between
    // "tabacco" and "T-shirt" read as "tshirt".
    assert_eq!(
        report(&data, "tr").nearest,
        ["résumé", "tabacco", "tshirt", "T-shirt"]
    );
}

#[test]
fn german_books_compare_case_too() {
    // A German book matches the word to a label only when case agrees as
    // well, so a word that matches no label exactly opens the first label
    // of the ones that weigh the same: the one with more apostrophes, and
    // after that the one with fewer characters that weigh nothing.
    let data = build_words("de_twins", "de", &["Email", "E-Mail", "cant", "can't"], &[]);
    assert_eq!(opens(&data, "E-MAIL").as_deref(), Some("Email"));
    assert_eq!(opens(&data, "E-Mail").as_deref(), Some("E-Mail"));
    assert_eq!(opens(&data, "CAN'T").as_deref(), Some("can't"));
}

#[test]
fn twins_and_rewritten_spellings_open_the_word_as_written() {
    // A sentence-initial capital opens the lowercase twin, and an apostrophe
    // the formatting does not straighten opens the apostrophe spelling.
    let words = [
        "if", "IF", "dont", "don't", "COVID-19", "MP3", "MP4", "people's",
    ];
    let data = build_words("en_tie", "en", &words, &[]);
    for (word, want) in [
        ("If", Some("if")),
        ("IF", Some("IF")),
        ("donʼt", Some("don't")),
        ("DON'T", Some("don't")),
        // Looked up as "COVID" and "people", which the build adds as aliases
        // for those entries.
        ("COVID-19", Some("COVID")),
        ("People's", Some("people")),
        // "MP3" and "MP4" would share "MP", so neither gets it.
        ("MP3", None),
    ] {
        assert_eq!(opens(&data, word).as_deref(), want, "{word}");
    }

    let data = build_words("uk_tie", "uk", &["Демянюка", "Дем’янюка"], &[]);
    for word in ["Дем’янюка", "Дем'янюка", "Демʼянюка"] {
        assert_eq!(opens(&data, word).as_deref(), Some("Дем’янюка"), "{word}");
    }
    assert_eq!(opens(&data, "Демянюка").as_deref(), Some("Демянюка"));
}

#[test]
fn greek_case_and_accents_count() {
    let data = build_words(
        "el_case",
        "el",
        &["κάτι", "άλλος", "αλλος", "π.χ.", "πχ"],
        &[],
    );
    assert_eq!(opens(&data, "Κάτι"), None);
    assert_eq!(opens(&data, "ΑΛΛΟΣ"), None);
    assert_eq!(opens(&data, "αλλος").as_deref(), Some("αλλος"));
    assert_eq!(opens(&data, "άλλος").as_deref(), Some("άλλος"));
    assert_eq!(opens(&data, "π.χ.").as_deref(), Some("π.χ."));
    assert_eq!(opens(&data, "πχ").as_deref(), Some("πχ"));
}

#[test]
fn cyrillic_query_without_punctuation_resolves_punctuated_headword() {
    // Punctuation weighs nothing, so "изпод" opens "из-под".
    let data = build_words(
        "ru_punct",
        "ru",
        &["из-под", "изба", "т.к.", "те", "театр"],
        &[],
    );
    assert_eq!(opens(&data, "изпод").as_deref(), Some("из-под"));
    assert_eq!(opens(&data, "тк").as_deref(), Some("т.к."));
    assert_eq!(opens(&data, "те").as_deref(), Some("те"));
    assert_eq!(report(&data, "изв").nearest, ["изба", "из-под", "те"]);
}

#[test]
fn sharp_s_and_ligature_headwords_resolve_by_every_spelling() {
    // These words open these entries in a dictionary built this way, in the
    // exact-accent default, with --fold-accents, and in a Russian dictionary
    // that holds Latin headwords.
    let de = [
        "da", "dass", "daß", "Fuß", "Futter", "Maß", "Masse", "Straße", "Strasse",
    ];
    for (tag, args) in [("de_exact", &[][..]), ("de_fold", &["--fold-accents"][..])] {
        let data = build_words(tag, "de", &de, args);
        let opens = |q: &str| opens(&data, q);
        for q in ["Fuß", "FUSS", "Fuss", "fuß"] {
            assert_eq!(opens(q).as_deref(), Some("Fuß"), "{tag}: {q}");
        }
        assert_eq!(opens("Straße").as_deref(), Some("Straße"), "{tag}");
        assert_eq!(opens("Strasse").as_deref(), Some("Strasse"), "{tag}");
        assert_eq!(opens("STRASSE").as_deref(), Some("Strasse"), "{tag}");
        assert_eq!(opens("daß").as_deref(), Some("daß"), "{tag}");
        assert_eq!(opens("Daß").as_deref(), Some("dass"), "{tag}");
        assert_eq!(opens("MASS").as_deref(), Some("Maß"), "{tag}");
        assert_eq!(opens("Masse").as_deref(), Some("Masse"), "{tag}");
        assert_eq!(opens("Fu"), None, "{tag}: the letter is not skipped");
        assert_eq!(opens("Strae"), None, "{tag}");
    }
    let fr = build_words("fr_lig", "fr", &["cœur", "sur", "Œdipe", "œil", "il"], &[]);
    assert_eq!(opens(&fr, "coeur").as_deref(), Some("cœur"));
    assert_eq!(opens(&fr, "CŒUR").as_deref(), Some("cœur"));
    assert_eq!(opens(&fr, "OEDIPE").as_deref(), Some("Œdipe"));
    // The Kindle 4 shows no popup for "sur", one of its French stop words;
    // the Paperwhite 4 opens it.
    assert_eq!(opens(&fr, "sur").as_deref(), Some("sur"));
    assert_eq!(opens(&fr, "cur"), None);
    let nl = build_words("nl_lig", "nl", &["ĳs", "ĳzer", "is"], &[]);
    assert_eq!(
        opens(&nl, "ijs").as_deref(),
        Some("ijs"),
        "stored as ij, like kindlegen"
    );
    assert_eq!(opens(&nl, "IJzer").as_deref(), Some("ijzer"));
    assert_eq!(opens(&nl, "zer"), None);
    let ru = build_words(
        "ru_lig",
        "ru",
        &["из", "изба", "Straße", "Strasse", "apple", "cœur", "Zebra"],
        &[],
    );
    assert_eq!(opens(&ru, "Straße").as_deref(), Some("Straße"));
    assert_eq!(opens(&ru, "Strasse").as_deref(), Some("Strasse"));
    assert_eq!(opens(&ru, "coeur").as_deref(), Some("cœur"));
    assert_eq!(opens(&ru, "apple").as_deref(), Some("apple"));
}

#[test]
fn japanese_literal_match() {
    // A kanji headword in a generated table is found by its code point.
    let out = tmp("ja.mobi");
    build("ja", &out);
    let data = std::fs::read(&out).unwrap();
    assert!(lookup(&data, "水").is_some(), "kanji headword resolves");
    assert!(
        lookup(&data, "存在しない語").is_none(),
        "non-headword misses"
    );
}

/// The simulator is used on kindling's own output all day; this is the only
/// test that points it at kindlegen's. It also pins the index-pointer
/// behavior from issue #49: on a well-formed dictionary the record the header
/// names must be the record the index is in, so the recovery path stays a
/// recovery path and does not quietly become the normal one.
#[test]
fn kindlegen_reference_dictionaries_resolve() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let cases: [(&str, &str); 6] = [
        ("langs/en/en-kindlegen.mobi", "book"),
        ("langs/fr/fr-kindlegen.mobi", "rivière"),
        ("langs/ru/ru-kindlegen.mobi", "река"),
        ("langs/el/el-kindlegen.mobi", "νερό"),
        ("langs/ja/ja-kindlegen.mobi", "水"),
        ("parity/simple_dict/kindlegen_reference.mobi", "alpha"),
    ];
    for (path, word) in cases {
        let data = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let r = report(&data, word);
        assert!(
            r.result.is_some(),
            "{path}: {word:?} should resolve, index at {:?} with {} headwords",
            r.index_record,
            r.entries
        );
        assert_eq!(
            r.index_record.map(|i| i as u32),
            r.declared_index_record,
            "{path}: the header should name the record the orth index is actually in"
        );
        println!(
            "  \u{2713} {path}: {word:?} resolves via record {:?}",
            r.index_record
        );
    }

    // kindlegen sorts a Russian dictionary without regard to case, but the Kindle
    // weighs a Cyrillic capital before every lowercase letter. The capitalized
    // "Москва" sits in the middle of this index, so the search for "вода" turns
    // the wrong way there and never reaches the stored label. "Москва" itself is
    // missed on the Paperwhite 4 for another reason: in a Russian dictionary it
    // looks the word up in lowercase, and kindlegen writes no lowercase label.
    let data = std::fs::read(root.join("langs/ru/ru-kindlegen.mobi")).unwrap();
    let r = report(&data, "вода");
    assert_eq!(r.result, None);
    assert_eq!(r.unreachable, ["вода"]);
    assert!(r.order_breaks > 0);
}

/// The notes of `kindling lookup`. On the Kindle 4 (firmware 4.1.4) and the
/// Paperwhite 4 (firmware 5.16.5), a search for one Hangul syllable in this
/// dictionary returns all 25 syllables, so the note names the first 20 and
/// counts the rest. A label that contains a Hangul jamo is named when the
/// dictionary is built and when it is searched. In a Russian dictionary the
/// notes name the lowercased word that is searched.
#[test]
fn lookup_notes_bound_their_lists_and_name_what_is_searched() {
    let syllables: Vec<String> = (0..25)
        .map(|i| char::from_u32(0xAC00 + 7 * i).unwrap().to_string())
        .collect();
    let mut words: Vec<&str> = syllables.iter().map(String::as_str).collect();
    words.extend(["cat", "ㄱa"]);
    let (data, built) = build_words_logged("notes25", "en", &words, &[]);
    assert!(
        built.contains("1 lookup term contains Hangul jamo"),
        "{built}"
    );
    assert_eq!(report(&data, "cat").hangul_jamo_labels, 1);
    let (_, notes) = cli_lookup("notes25", &data, "가");
    assert!(
        notes.contains("returned 25 labels:") && notes.contains(" and 5 more."),
        "{notes}"
    );
    // The run is in code point order, so the last five are the ones counted.
    assert!(
        notes.contains(&format!("\"{}\"", syllables[19]))
            && !notes.contains(&format!("\"{}\"", syllables[20])),
        "{notes}"
    );
    assert!(
        notes.contains("1 label in this index contains Hangul jamo"),
        "{notes}"
    );
    let (data, _) = build_words_logged("notes20", "en", &words[5..25], &[]);
    let (_, notes) = cli_lookup("notes20", &data, "가");
    assert!(
        notes.contains("returned 20 labels:") && !notes.contains(" more"),
        "{notes}"
    );

    let data = build_words("notesru", "ru", &["дом", "кот", "мир"], &[]);
    let (_, notes) = cli_lookup("notesru", &data, "ДОМ");
    assert!(notes.contains("so it searches for \"дом\""), "{notes}");
    let (_, notes) = cli_lookup("notesru", &data, "ЛЕС");
    assert!(
        notes.contains("the search for \"лес\" ended among"),
        "{notes}"
    );
}

/// In a dictionary whose language kindling has no code for, here Swahili, the
/// note of `kindling lookup` names the book language without an empty code.
#[test]
fn lookup_note_leaves_out_a_missing_language_code() {
    let data = build_words("notessw", "sw", &["habari", "jambo"], &[]);
    let (_, notes) = cli_lookup("notessw", &data, "habari.");
    assert!(
        notes.contains("formatted for a book in the dictionary's language."),
        "{notes}"
    );
    assert!(!notes.contains("(\"\")"), "{notes}");
}
