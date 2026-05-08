//! Round-trip test for the StarDict builder.
//!
//! Builds the parity `simple_dict` fixture as a StarDict bundle, then
//! reparses the four files (`.ifo`, `.idx`, `.dict`, `.syn`) by hand and
//! asserts the bundle has the shape every StarDict reader expects:
//!
//! * `.ifo` carries the magic line, wordcount, idxfilesize, synwordcount
//!   and `sametypesequence=h`.
//! * `.idx` is sorted by `g_ascii_strcasecmp` and every offset/size pair
//!   addresses a valid slice of the `.dict` body.
//! * `.dict` payloads contain the original definitions and no Kindle-only
//!   `<idx:*>` markup leaks through.
//! * `.syn` maps every inflection to the index of its lemma in `.idx`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn kindling_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kindling-cli")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join("simple_dict")
}

fn build_stardict(out_dir: &Path) {
    let opf = fixture_dir().join("simple_dict.opf");
    let output = Command::new(kindling_bin())
        .arg("stardict")
        .arg(&opf)
        .arg("-o")
        .arg(out_dir)
        .output()
        .expect("failed to spawn kindling-cli stardict");
    assert!(
        output.status.success(),
        "kindling-cli stardict failed: {:?}\n--stdout--\n{}\n--stderr--\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Parse `(word, offset, size)` tuples from a StarDict `.idx` blob.
fn parse_idx(bytes: &[u8]) -> Vec<(String, u32, u32)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let nul = bytes[i..]
            .iter()
            .position(|&b| b == 0)
            .expect("missing NUL terminator in .idx");
        let word = std::str::from_utf8(&bytes[i..i + nul])
            .expect("non-UTF-8 word in .idx")
            .to_string();
        let p = i + nul + 1;
        assert!(p + 8 <= bytes.len(), "truncated .idx record");
        let offset = u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap());
        let size = u32::from_be_bytes(bytes[p + 4..p + 8].try_into().unwrap());
        out.push((word, offset, size));
        i = p + 8;
    }
    out
}

/// Parse `(word, original_word_index)` tuples from a StarDict `.syn` blob.
fn parse_syn(bytes: &[u8]) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let nul = bytes[i..]
            .iter()
            .position(|&b| b == 0)
            .expect("missing NUL terminator in .syn");
        let word = std::str::from_utf8(&bytes[i..i + nul])
            .expect("non-UTF-8 word in .syn")
            .to_string();
        let p = i + nul + 1;
        assert!(p + 4 <= bytes.len(), "truncated .syn record");
        let idx = u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap());
        out.push((word, idx));
        i = p + 4;
    }
    out
}

/// `g_ascii_strcasecmp` clone, mirroring the writer.
fn ascii_case_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let n = a.len().min(b.len());
    for i in 0..n {
        let aa = if a[i].is_ascii_uppercase() { a[i] + 32 } else { a[i] };
        let bb = if b[i].is_ascii_uppercase() { b[i] + 32 } else { b[i] };
        if aa != bb {
            return aa.cmp(&bb);
        }
    }
    a.len().cmp(&b.len())
}

fn ifo_get<'a>(ifo: &'a str, key: &str) -> Option<&'a str> {
    for line in ifo.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{}=", key)) {
            return Some(rest);
        }
    }
    None
}

#[test]
fn builds_stardict_from_simple_dict_fixture() {
    let tmp = std::env::temp_dir().join("kindling_stardict_simple");
    let _ = std::fs::remove_dir_all(&tmp);
    build_stardict(&tmp);

    let stem = tmp.file_name().unwrap().to_string_lossy().into_owned();
    let ifo_path = tmp.join(format!("{}.ifo", stem));
    let idx_path = tmp.join(format!("{}.idx", stem));
    let dict_path = tmp.join(format!("{}.dict", stem));
    let syn_path = tmp.join(format!("{}.syn", stem));

    // .ifo: magic line + the keys we promise to emit.
    let ifo = std::fs::read_to_string(&ifo_path).unwrap();
    let mut lines = ifo.lines();
    assert_eq!(lines.next().unwrap(), "StarDict's dict ifo file");
    assert_eq!(ifo_get(&ifo, "version"), Some("2.4.2"));
    assert_eq!(ifo_get(&ifo, "wordcount"), Some("5"));
    assert_eq!(ifo_get(&ifo, "synwordcount"), Some("9"));
    assert_eq!(ifo_get(&ifo, "sametypesequence"), Some("h"));
    assert_eq!(
        ifo_get(&ifo, "bookname"),
        Some("Parity Test Dictionary"),
        "bookname should fall back to dc:title"
    );
    assert_eq!(
        ifo_get(&ifo, "author"),
        Some("Kindling Parity Suite"),
        "author should fall back to dc:creator"
    );
    let idxfilesize: u64 = ifo_get(&ifo, "idxfilesize").unwrap().parse().unwrap();
    let actual_idxfilesize = std::fs::metadata(&idx_path).unwrap().len();
    assert_eq!(
        idxfilesize, actual_idxfilesize,
        "idxfilesize in .ifo must match real .idx size"
    );

    // .idx: 5 headwords, sorted by g_ascii_strcasecmp.
    let idx_bytes = std::fs::read(&idx_path).unwrap();
    let idx = parse_idx(&idx_bytes);
    let words: Vec<&str> = idx.iter().map(|(w, _, _)| w.as_str()).collect();
    assert_eq!(words, vec!["alpha", "bravo", "charlie", "delta", "echo"]);
    for window in idx.windows(2) {
        assert!(
            ascii_case_cmp(window[0].0.as_bytes(), window[1].0.as_bytes())
                != std::cmp::Ordering::Greater,
            ".idx must be sorted by g_ascii_strcasecmp: {:?} >= {:?}",
            window[0].0,
            window[1].0
        );
    }

    // .dict: every (offset, size) addresses a slice that contains the
    // expected definition text, with no idx:* markup leaking through.
    let dict_bytes = std::fs::read(&dict_path).unwrap();
    let total: u64 = idx.iter().map(|(_, _, s)| *s as u64).sum();
    assert_eq!(
        total,
        dict_bytes.len() as u64,
        "sum of .idx sizes must equal .dict length"
    );
    for (word, offset, size) in &idx {
        let slice = &dict_bytes[*offset as usize..*offset as usize + *size as usize];
        let text = std::str::from_utf8(slice).expect(".dict slice must be UTF-8");
        assert!(!text.contains("idx:"), "idx markup leaked for {}: {}", word, text);
        assert!(
            text.contains(&format!("<b>{}</b>", word)),
            "expected <b>{}</b> in entry: {}",
            word,
            text
        );
        // Each fixture entry has a sentence containing "placeholder"; that's
        // a cheap proof the definition text survived cleanup.
        assert!(text.contains("placeholder"), "definition stripped for {}: {}", word, text);
    }

    // .syn: 9 inflections, each pointing at the right lemma index in .idx.
    let syn_bytes = std::fs::read(&syn_path).unwrap();
    let syn = parse_syn(&syn_bytes);
    assert_eq!(syn.len(), 9);
    for window in syn.windows(2) {
        assert!(
            ascii_case_cmp(window[0].0.as_bytes(), window[1].0.as_bytes())
                != std::cmp::Ordering::Greater,
            ".syn must be sorted by g_ascii_strcasecmp"
        );
    }

    // Build the inflection -> lemma map the fixture promises.
    let expected: &[(&str, &str)] = &[
        ("alphas", "alpha"),
        ("alphae", "alpha"),
        ("bravos", "bravo"),
        ("charlies", "charlie"),
        ("charliest", "charlie"),
        ("charlying", "charlie"),
        ("deltas", "delta"),
        ("echoes", "echo"),
        ("echoing", "echo"),
    ];
    let lemma_index: std::collections::HashMap<&str, u32> = idx
        .iter()
        .enumerate()
        .map(|(i, (w, _, _))| (w.as_str(), i as u32))
        .collect();
    let syn_map: std::collections::HashMap<&str, u32> =
        syn.iter().map(|(w, i)| (w.as_str(), *i)).collect();
    for (form, lemma) in expected {
        let got = syn_map
            .get(form)
            .unwrap_or_else(|| panic!("missing syn entry for {}", form));
        let want = lemma_index
            .get(lemma)
            .unwrap_or_else(|| panic!("missing lemma {}", lemma));
        assert_eq!(got, want, "syn redirect for {} should point at {}", form, lemma);
    }
}

#[test]
fn library_api_returns_consistent_report() {
    use kindling::stardict;

    let tmp = std::env::temp_dir().join("kindling_stardict_api");
    let _ = std::fs::remove_dir_all(&tmp);
    let opf = fixture_dir().join("simple_dict.opf");
    let report = stardict::build_stardict(&opf, &tmp, &stardict::StarDictOptions::default())
        .expect("build_stardict failed");

    assert_eq!(report.wordcount, 5);
    assert_eq!(report.synwordcount, 9);
    let on_disk = std::fs::metadata(&report.idx_path).unwrap().len();
    let idx_bytes = std::fs::read(&report.idx_path).unwrap();
    let parsed_count = parse_idx(&idx_bytes).len();
    assert_eq!(parsed_count, report.wordcount);
    assert!(on_disk > 0);
}

#[test]
fn ifo_emits_website_email_description_when_provided() {
    use kindling::stardict;

    let tmp = std::env::temp_dir().join("kindling_stardict_metadata");
    let _ = std::fs::remove_dir_all(&tmp);
    let opf = fixture_dir().join("simple_dict.opf");
    let options = stardict::StarDictOptions {
        website: Some("https://example.com/dict".to_string()),
        email: Some("dict@example.com".to_string()),
        description: Some("Public-domain test dictionary. License: MIT.".to_string()),
        ..Default::default()
    };
    let report = stardict::build_stardict(&opf, &tmp, &options).expect("build_stardict failed");
    let ifo = std::fs::read_to_string(&report.ifo_path).unwrap();

    assert_eq!(ifo_get(&ifo, "website"), Some("https://example.com/dict"));
    assert_eq!(ifo_get(&ifo, "email"), Some("dict@example.com"));
    assert_eq!(
        ifo_get(&ifo, "description"),
        Some("Public-domain test dictionary. License: MIT.")
    );
}

#[test]
fn ifo_omits_metadata_keys_when_blank() {
    use kindling::stardict;

    let tmp = std::env::temp_dir().join("kindling_stardict_blank_meta");
    let _ = std::fs::remove_dir_all(&tmp);
    let opf = fixture_dir().join("simple_dict.opf");
    let options = stardict::StarDictOptions {
        website: Some(String::new()),
        email: None,
        ..Default::default()
    };
    let report = stardict::build_stardict(&opf, &tmp, &options).expect("build_stardict failed");
    let ifo = std::fs::read_to_string(&report.ifo_path).unwrap();

    assert!(!ifo.contains("\nwebsite="), "blank website should be omitted: {}", ifo);
    assert!(!ifo.contains("\nemail="), "missing email should be omitted: {}", ifo);
}
