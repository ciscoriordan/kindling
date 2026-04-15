//! Integration tests for the `kindling-cli` binary.
//!
//! Each subcommand lives in its own module below so helper names like
//! `kindling_bin` and `tmp_path` do not collide. This file replaces three
//! earlier per-subcommand files (`cli_validate.rs`, `cli_repair.rs`,
//! `cli_rewrite_metadata.rs`) without changing any test logic.

mod validate {
    
    use std::path::PathBuf;
    use std::process::{Command, Output};
    
    /// Path to the `kindling-cli` binary, resolved at compile time by Cargo.
    fn kindling_bin() -> &'static str {
        env!("CARGO_BIN_EXE_kindling-cli")
    }
    
    /// Absolute path to `tests/fixtures/<name>`.
    fn fixture_dir(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }
    
    /// Run `kindling-cli validate <args...>` and return the full `Output`.
    /// Panics if the binary cannot be spawned.
    fn run_validate(args: &[&str]) -> Output {
        Command::new(kindling_bin())
            .arg("validate")
            .args(args)
            .output()
            .expect("failed to spawn kindling-cli validate")
    }
    
    /// Pretty-print stdout+stderr on failure.
    fn dump(output: &Output) -> String {
        format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    }
    
    // ---------------------------------------------------------------------------
    // Clean fixtures: should exit 0 with no error findings
    // ---------------------------------------------------------------------------
    
    #[test]
    fn validate_clean_book_exits_zero() {
        let opf = fixture_dir("clean_book").join("clean_book.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
    
        assert!(
            out.status.success(),
            "clean_book should validate cleanly\n{}",
            dump(&out)
        );
        assert_eq!(
            out.status.code(),
            Some(0),
            "clean_book should exit 0\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("Kindle Publishing Guidelines v"),
            "header missing KPG version line\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("0 errors"),
            "clean_book should have 0 errors\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("0 warnings"),
            "clean_book should have 0 warnings\n{}",
            dump(&out)
        );
    }
    
    #[test]
    fn validate_clean_dict_exits_zero() {
        let opf = fixture_dir("clean_dict").join("clean_dict.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
    
        assert!(
            out.status.success(),
            "clean_dict should validate cleanly\n{}",
            dump(&out)
        );
        assert_eq!(out.status.code(), Some(0));
        assert!(
            stdout.contains("Kindle Publishing Guidelines v"),
            "header missing KPG version line\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("0 errors"),
            "clean_dict should have 0 errors\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("0 warnings"),
            "clean_dict should have 0 warnings\n{}",
            dump(&out)
        );
    }
    
    #[test]
    fn validate_clean_book_strict_still_exits_zero() {
        // --strict should not flip a clean fixture to non-zero.
        let opf = fixture_dir("clean_book").join("clean_book.opf");
        let out = run_validate(&["--strict", opf.to_str().unwrap()]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "clean_book --strict should still exit 0\n{}",
            dump(&out)
        );
    }
    
    // ---------------------------------------------------------------------------
    // Error fixture: must flag the rules it was constructed to trigger
    // ---------------------------------------------------------------------------
    
    #[test]
    fn validate_book_with_errors_exits_one() {
        let opf = fixture_dir("book_with_errors").join("book_with_errors.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert_eq!(
            out.status.code(),
            Some(1),
            "book_with_errors should exit 1\n{}",
            dump(&out)
        );
        assert!(
            !stdout.contains("0 errors"),
            "book_with_errors should report at least one error\n{}",
            dump(&out)
        );

        // Each rule id below corresponds to a deliberately-planted defect in
        // the fixture. Keep these assertions explicit so the test fails loudly
        // if a rule id is renumbered or a check is accidentally removed.
        for rule_id in &["R4.2.1", "R6.3", "R6.4", "R17.1"] {
            assert!(
                stdout.contains(rule_id),
                "expected rule id {} in output\n{}",
                rule_id,
                dump(&out)
            );
        }
    }

    #[test]
    fn validate_legacy_dict_errors_reports_section_15_rules() {
        // legacy_dict_errors is an EPUB 2.0 dict fixture with deliberately-planted
        // defects that should trip R15.1, R15.2, R15.3, R15.5, R15.6 and R15.7
        // without firing any R15.e* (EPUB 3 DICT) rule.
        let opf = fixture_dir("legacy_dict_errors").join("legacy_dict_errors.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert_eq!(
            out.status.code(),
            Some(1),
            "legacy_dict_errors should exit 1\n{}",
            dump(&out)
        );
        for rule_id in &["R15.1", "R15.2", "R15.3", "R15.5", "R15.6", "R15.7"] {
            assert!(
                stdout.contains(rule_id),
                "expected rule id {} in output\n{}",
                rule_id,
                dump(&out)
            );
        }
        // EPUB 3 DICT rules must not fire on a package version="2.0" fixture.
        for rule_id in &["R15.e1", "R15.e2", "R15.e3", "R15.e4", "R15.e5", "R15.e6", "R15.e7"] {
            assert!(
                !stdout.contains(rule_id),
                "rule id {} must not fire on EPUB 2.0 legacy dict\n{}",
                rule_id,
                dump(&out)
            );
        }
    }
    
    // ---------------------------------------------------------------------------
    // --strict mode: warnings-only fixture
    // ---------------------------------------------------------------------------
    
    #[test]
    fn validate_parse_encoding_errors_flags_all_seven_rules() {
        // Cluster B (parse-time / DOCTYPE / encoding) must light up R6.6 through
        // R6.12 on this fixture. Each rule id is checked explicitly so a future
        // refactor can't silently drop one of them.
        let opf = fixture_dir("parse_encoding_errors").join("parse_encoding_errors.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert_eq!(
            out.status.code(),
            Some(1),
            "parse_encoding_errors should exit 1\n{}",
            dump(&out)
        );
        for rule_id in &[
            "R6.6", "R6.7", "R6.8", "R6.9", "R6.10", "R6.11", "R6.12",
        ] {
            assert!(
                stdout.contains(rule_id),
                "expected rule id {} in parse_encoding_errors output\n{}",
                rule_id,
                dump(&out)
            );
        }
    }

    #[test]
    fn validate_book_with_warnings_default_mode_exits_zero() {
        // Without --strict a warning alone should not fail the run.
        let opf = fixture_dir("book_with_warnings").join("book_with_warnings.opf");
        let out = run_validate(&[opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
    
        assert_eq!(
            out.status.code(),
            Some(0),
            "warnings-only fixture should exit 0 without --strict\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("R4.2.3"),
            "expected the cover-too-small warning R4.2.3 in output\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("0 errors"),
            "warnings-only fixture should report 0 errors\n{}",
            dump(&out)
        );
    }
    
    #[test]
    fn validate_book_with_warnings_strict_exits_one() {
        let opf = fixture_dir("book_with_warnings").join("book_with_warnings.opf");
        let out = run_validate(&["--strict", opf.to_str().unwrap()]);
        let stdout = String::from_utf8_lossy(&out.stdout);
    
        assert_eq!(
            out.status.code(),
            Some(1),
            "--strict should promote warnings to a non-zero exit\n{}",
            dump(&out)
        );
        assert!(
            stdout.contains("R4.2.3"),
            "expected R4.2.3 in --strict output\n{}",
            dump(&out)
        );
    }
    
    // ---------------------------------------------------------------------------
    // Default output extension: KF8-only (.azw3) for non-dict build,
    // legacy MOBI7+KF8 (.mobi) for dict build, KF8-only (.azw3) for comics.
    //
    // These tests run the actual `kindling-cli build` / `kindling-cli comic`
    // binary with no `-o` flag and assert the default output path that
    // kindling picks.
    // ---------------------------------------------------------------------------
    
    /// Run `kindling-cli build <args...>` and return the full `Output`.
    fn run_build(args: &[&str]) -> Output {
        Command::new(kindling_bin())
            .arg("build")
            .args(args)
            .output()
            .expect("failed to spawn kindling-cli build")
    }
    
    /// Run `kindling-cli comic <args...>` and return the full `Output`.
    fn run_comic(args: &[&str]) -> Output {
        Command::new(kindling_bin())
            .arg("comic")
            .args(args)
            .output()
            .expect("failed to spawn kindling-cli comic")
    }
    
    /// Tiny RAII temp dir that creates a unique subdirectory under
    /// `std::env::temp_dir()` and removes it on drop. The repo intentionally
    /// does not pull in the `tempfile` crate, so this is a minimal stand-in
    /// just for CLI integration tests.
    struct TempDir {
        path: PathBuf,
    }
    
    impl TempDir {
        fn new(label: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "kindling-cli-{}-{}-{}-{}",
                label,
                std::process::id(),
                nanos,
                n
            ));
            std::fs::create_dir_all(&path).expect("create tempdir");
            TempDir { path }
        }
        fn path(&self) -> &std::path::Path {
            &self.path
        }
    }
    
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
    
    /// Copy a fixture directory into a fresh temp dir so tests do not litter
    /// the source tree with generated `.azw3` / `.mobi` artifacts. Returns the
    /// path to the copied OPF inside the temp dir, plus the TempDir guard.
    fn stage_fixture(name: &str, opf_name: &str) -> (TempDir, PathBuf) {
        let src = fixture_dir(name);
        let tmp = TempDir::new(name);
        for entry in std::fs::read_dir(&src).expect("read fixture dir") {
            let entry = entry.expect("dir entry");
            let src_path = entry.path();
            if src_path.is_file() {
                let dst = tmp.path().join(entry.file_name());
                std::fs::copy(&src_path, &dst).expect("copy fixture file");
            }
        }
        let opf = tmp.path().join(opf_name);
        (tmp, opf)
    }
    
    #[test]
    fn build_non_dict_defaults_to_azw3() {
        // `clean_book` has no DictionaryInLanguage metadata, so is_dictionary()
        // is false and kindling should default to KF8-only `.azw3`.
        let (tmp, opf) = stage_fixture("clean_book", "clean_book.opf");
        let out = run_build(&[opf.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "build clean_book should succeed\n{}",
            dump(&out)
        );
    
        let expected = tmp.path().join("clean_book.azw3");
        let unexpected = tmp.path().join("clean_book.mobi");
        assert!(
            expected.exists(),
            "expected default output at {:?}\n{}",
            expected,
            dump(&out)
        );
        assert!(
            !unexpected.exists(),
            "did not expect legacy .mobi at {:?}\n{}",
            unexpected,
            dump(&out)
        );
    }
    
    #[test]
    fn build_dict_defaults_to_mobi() {
        // `clean_dict` has DictionaryInLanguage set, so is_dictionary() is true
        // and kindling must keep defaulting to dual-format MOBI7+KF8 `.mobi`
        // because Kindle's lookup popup requires the MOBI7 INDX format.
        let (tmp, opf) = stage_fixture("clean_dict", "clean_dict.opf");
        let out = run_build(&[opf.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "build clean_dict should succeed\n{}",
            dump(&out)
        );
    
        let expected = tmp.path().join("clean_dict.mobi");
        let unexpected = tmp.path().join("clean_dict.azw3");
        assert!(
            expected.exists(),
            "expected dict default output at {:?}\n{}",
            expected,
            dump(&out)
        );
        assert!(
            !unexpected.exists(),
            "did not expect KF8-only .azw3 for dict at {:?}\n{}",
            unexpected,
            dump(&out)
        );
    }
    
    #[test]
    fn build_non_dict_legacy_mobi_flag_produces_mobi() {
        // `--legacy-mobi` is the escape hatch: even on a non-dict book, this
        // should produce dual-format MOBI7+KF8 `.mobi` and pick the `.mobi`
        // default extension.
        let (tmp, opf) = stage_fixture("clean_book", "clean_book.opf");
        let out = run_build(&["--legacy-mobi", opf.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "build clean_book --legacy-mobi should succeed\n{}",
            dump(&out)
        );
    
        let expected = tmp.path().join("clean_book.mobi");
        assert!(
            expected.exists(),
            "expected legacy dual-format output at {:?}\n{}",
            expected,
            dump(&out)
        );
    }
    
    #[test]
    fn comic_defaults_to_azw3_from_cbr_fixture() {
        // The repo ships a small CBR fixture at tests/fixtures/test_comic.cbr.
        // Copy it into a temp dir and build with no -o to assert the default
        // extension is `.azw3`.
        let src = fixture_dir("test_comic.cbr");
        if !src.exists() {
            eprintln!("skipping comic default-extension test: no CBR fixture");
            return;
        }
        let tmp = TempDir::new("test_comic");
        let cbr = tmp.path().join("test_comic.cbr");
        std::fs::copy(&src, &cbr).expect("copy cbr fixture");
    
        let out = run_comic(&[cbr.to_str().unwrap()]);
        assert!(
            out.status.success(),
            "comic build should succeed\n{}",
            dump(&out)
        );
    
        let expected = tmp.path().join("test_comic.azw3");
        let unexpected = tmp.path().join("test_comic.mobi");
        assert!(
            expected.exists(),
            "expected comic default output at {:?}\n{}",
            expected,
            dump(&out)
        );
        assert!(
            !unexpected.exists(),
            "did not expect legacy .mobi at {:?}\n{}",
            unexpected,
            dump(&out)
        );
    }
}

mod repair {
    
    use std::io::{Cursor, Read, Write};
    use std::path::PathBuf;
    use std::process::{Command, Output};
    
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;
    
    fn kindling_bin() -> &'static str {
        env!("CARGO_BIN_EXE_kindling-cli")
    }
    
    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kindling_repair_cli_{}_{}",
            std::process::id(),
            name
        ));
        p
    }
    
    fn dump(out: &Output) -> String {
        format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    }
    
    fn build_epub(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut w = zip::ZipWriter::new(cursor);
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    
            if !entries.iter().any(|(n, _)| *n == "mimetype") {
                w.start_file("mimetype", stored).unwrap();
                w.write_all(b"application/epub+zip").unwrap();
            }
            for (name, bytes) in entries {
                let opts = if *name == "mimetype" { stored } else { deflate };
                w.start_file(*name, opts).unwrap();
                w.write_all(bytes).unwrap();
            }
            w.finish().unwrap();
        }
        buf
    }
    
    const CONTAINER_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
    <container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
      <rootfiles>
        <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
      </rootfiles>
    </container>"#;
    
    const OPF_WITH_LANG: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
    <package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
      <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
        <dc:title>CLI Repair Test</dc:title>
        <dc:identifier id="uid">urn:uuid:cli-repair-test</dc:identifier>
        <dc:language>en</dc:language>
      </metadata>
      <manifest>
        <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
      </manifest>
      <spine><itemref idref="ch1"/></spine>
    </package>"#;
    
    const CLEAN_XHTML: &[u8] = br#"<?xml version="1.0" encoding="utf-8"?>
    <html xmlns="http://www.w3.org/1999/xhtml"><head><title>X</title></head>
    <body><p>hello</p></body></html>"#;
    
    const BROKEN_XHTML_NO_DECL: &[u8] =
        br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>hi</p><img alt="bad"/></body></html>"#;
    
    fn run_repair(args: &[&str]) -> Output {
        Command::new(kindling_bin())
            .arg("repair")
            .args(args)
            .output()
            .expect("failed to spawn kindling-cli repair")
    }
    
    #[test]
    fn cli_repair_clean_input_is_byte_identical() {
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", CLEAN_XHTML),
        ]);
        let input = tmp_path("clean_in.epub");
        let output = tmp_path("clean_out.epub");
        std::fs::write(&input, &epub).unwrap();
        let _ = std::fs::remove_file(&output);
    
        let out = run_repair(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "clean repair should exit 0\n{}", dump(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("No repairs needed"),
            "clean repair should say no repairs needed\n{}",
            dump(&out)
        );
    
        let in_bytes = std::fs::read(&input).unwrap();
        let out_bytes = std::fs::read(&output).unwrap();
        assert_eq!(
            in_bytes, out_bytes,
            "clean input must be copied byte-identically"
        );
    }
    
    #[test]
    fn cli_repair_broken_input_applies_fixes() {
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", BROKEN_XHTML_NO_DECL),
        ]);
        let input = tmp_path("broken_in.epub");
        let output = tmp_path("broken_out.epub");
        std::fs::write(&input, &epub).unwrap();
        let _ = std::fs::remove_file(&output);
    
        let out = run_repair(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ]);
        assert!(out.status.success(), "repair should still exit 0\n{}", dump(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("Repaired"), "should report repairs\n{}", dump(&out));
        assert!(
            stderr.contains("added XML declaration"),
            "should mention the XML declaration fix\n{}",
            dump(&out)
        );
        assert!(
            stderr.contains("removed 1 stray img"),
            "should mention the stray img fix\n{}",
            dump(&out)
        );
    
        // Output should exist and be a valid EPUB with a fixed ch1.xhtml.
        let bytes = std::fs::read(&output).unwrap();
        let mut a = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut e = a.by_name("OEBPS/ch1.xhtml").unwrap();
        let mut s = String::new();
        e.read_to_string(&mut s).unwrap();
        assert!(s.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>"));
        assert!(!s.contains("<img"));
    }
    
    #[test]
    fn cli_repair_dry_run_does_not_write_output() {
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", BROKEN_XHTML_NO_DECL),
        ]);
        let input = tmp_path("dry_in.epub");
        let output = tmp_path("dry_out_must_not_exist.epub");
        std::fs::write(&input, &epub).unwrap();
        let _ = std::fs::remove_file(&output);
    
        let out = run_repair(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--dry-run",
        ]);
        assert!(out.status.success(), "dry-run should exit 0\n{}", dump(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("(dry-run)"), "dry-run prefix expected\n{}", dump(&out));
        assert!(stderr.contains("Repaired"), "should report repair count\n{}", dump(&out));
        assert!(
            !output.exists(),
            "dry-run must not create the output file: {}",
            output.display()
        );
    }
    
    #[test]
    fn cli_repair_report_json_emits_json_on_stdout() {
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", BROKEN_XHTML_NO_DECL),
        ]);
        let input = tmp_path("json_in.epub");
        let output = tmp_path("json_out.epub");
        std::fs::write(&input, &epub).unwrap();
        let _ = std::fs::remove_file(&output);
    
        let out = run_repair(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--report-json",
        ]);
        assert!(out.status.success(), "report-json should exit 0\n{}", dump(&out));
        let stdout = String::from_utf8_lossy(&out.stdout);
        let trimmed = stdout.trim();
        assert!(trimmed.starts_with('{'), "stdout should be JSON\n{}", dump(&out));
        assert!(trimmed.ends_with('}'));
        assert!(trimmed.contains("\"fixes_applied\""));
        assert!(trimmed.contains("added_xml_declaration"));
        assert!(trimmed.contains("removed_stray_img"));
    }
    
    #[test]
    fn cli_repair_drm_input_exits_one() {
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            (
                "META-INF/encryption.xml",
                br#"<?xml version="1.0"?><encryption/>"#,
            ),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", CLEAN_XHTML),
        ]);
        let input = tmp_path("drm_cli_in.epub");
        let output = tmp_path("drm_cli_out.epub");
        std::fs::write(&input, &epub).unwrap();
        let _ = std::fs::remove_file(&output);
    
        let out = run_repair(&[
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
        ]);
        assert_eq!(
            out.status.code(),
            Some(1),
            "DRM input should exit 1\n{}",
            dump(&out)
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("DRM"),
            "should mention DRM in error\n{}",
            dump(&out)
        );
        assert!(
            !output.exists(),
            "must not write output for DRM-protected input"
        );
    }
    
    #[test]
    fn cli_repair_default_output_filename() {
        // Without `-o`, repair should write next to the input as
        // `<stem>-fixed.epub`.
        let epub = build_epub(&[
            ("META-INF/container.xml", CONTAINER_XML),
            ("OEBPS/content.opf", OPF_WITH_LANG),
            ("OEBPS/ch1.xhtml", BROKEN_XHTML_NO_DECL),
        ]);
        let input = tmp_path("defaultout.epub");
        std::fs::write(&input, &epub).unwrap();
        let default_out = input.with_file_name(format!(
            "{}-fixed.epub",
            input.file_stem().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_file(&default_out);
    
        let out = run_repair(&[input.to_str().unwrap()]);
        assert!(out.status.success(), "repair should exit 0\n{}", dump(&out));
        assert!(
            default_out.exists(),
            "default output path should exist: {}",
            default_out.display()
        );
    }
}

mod rewrite_metadata {
    
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Command, Output};
    
    fn kindling_bin() -> &'static str {
        env!("CARGO_BIN_EXE_kindling-cli")
    }
    
    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kindling_rewrite_metadata_cli_{}_{}",
            std::process::id(),
            name
        ));
        p
    }
    
    fn dump(out: &Output) -> String {
        format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    }
    
    fn put_u32_be(buf: &mut [u8], offset: usize, value: u32) {
        buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    
    fn read_u32_be(data: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }
    
    fn read_u16_be(data: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([data[offset], data[offset + 1]])
    }
    
    /// Encode a single EXTH record: type(u32 BE) + length(u32 BE) + data.
    fn exth_record(rtype: u32, data: &[u8]) -> Vec<u8> {
        let mut rec = Vec::with_capacity(8 + data.len());
        rec.extend_from_slice(&rtype.to_be_bytes());
        rec.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
        rec.extend_from_slice(data);
        rec
    }
    
    /// Serialize an EXTH block (header + records + 4-byte-alignment padding).
    fn serialize_exth_block(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let record_bytes: Vec<Vec<u8>> = records
            .iter()
            .map(|(t, d)| exth_record(*t, d))
            .collect();
        let record_total: usize = record_bytes.iter().map(|r| r.len()).sum();
        let unpadded_len = 12 + record_total;
        let padding = (4 - (unpadded_len % 4)) % 4;
        let padded_len = unpadded_len + padding;
    
        let mut out = Vec::with_capacity(padded_len);
        out.extend_from_slice(b"EXTH");
        out.extend_from_slice(&(padded_len as u32).to_be_bytes());
        out.extend_from_slice(&(records.len() as u32).to_be_bytes());
        for rec in &record_bytes {
            out.extend_from_slice(rec);
        }
        for _ in 0..padding {
            out.push(0);
        }
        out
    }
    
    /// Build a minimal synthetic MOBI with the given title and EXTH records.
    /// The file has three PalmDB records: record 0 (PalmDOC + MOBI header +
    /// EXTH + full_name), a dummy text record, and a dummy image record used
    /// as the cover target.
    fn build_synthetic_mobi(title: &str, exth_records: &[(u32, Vec<u8>)]) -> Vec<u8> {
        const MOBI_HEADER_LENGTH: usize = 264;
        let full_name = title.as_bytes();
    
        let mut mobi_header = vec![0u8; MOBI_HEADER_LENGTH];
        mobi_header[0..4].copy_from_slice(b"MOBI");
        mobi_header[4..8].copy_from_slice(&(MOBI_HEADER_LENGTH as u32).to_be_bytes());
        mobi_header[8..12].copy_from_slice(&2u32.to_be_bytes()); // type = 2 (book)
        mobi_header[12..16].copy_from_slice(&65001u32.to_be_bytes()); // UTF-8
        mobi_header[20..24].copy_from_slice(&6u32.to_be_bytes()); // file version
        mobi_header[112..116].copy_from_slice(&0x40u32.to_be_bytes()); // EXTH flag
        mobi_header[92..96].copy_from_slice(&2u32.to_be_bytes()); // first_image_record
    
        let exth_block = serialize_exth_block(exth_records);
    
        let mut record0 = Vec::new();
        // PalmDOC header: compression=1, reserved, text_length=1024, text_rec_count=1,
        // record_size=4096, encryption_type=0, unknown=0.
        record0.extend_from_slice(&1u16.to_be_bytes());
        record0.extend_from_slice(&0u16.to_be_bytes());
        record0.extend_from_slice(&1024u32.to_be_bytes());
        record0.extend_from_slice(&1u16.to_be_bytes());
        record0.extend_from_slice(&4096u16.to_be_bytes());
        record0.extend_from_slice(&0u16.to_be_bytes());
        record0.extend_from_slice(&0u16.to_be_bytes());
        record0.extend_from_slice(&mobi_header);
        record0.extend_from_slice(&exth_block);
        let full_name_offset = record0.len();
        record0.extend_from_slice(full_name);
        while record0.len() % 4 != 0 {
            record0.push(0);
        }
        // full_name_offset / full_name_length live at MOBI header +68/+72
        // which is record0 +84/+88.
        put_u32_be(&mut record0, 84, full_name_offset as u32);
        put_u32_be(&mut record0, 88, full_name.len() as u32);
    
        let dummy_text = vec![0u8; 128];
        // JPEG-magic-prefixed "cover" record.
        let mut cover = vec![0xFFu8, 0xD8, 0xFF, 0xE0];
        cover.extend(std::iter::repeat(0x11).take(256));
        cover.extend_from_slice(&[0xFF, 0xD9]);
    
        let records: Vec<Vec<u8>> = vec![record0, dummy_text, cover];
    
        let num_records = records.len();
        let record_info_len = num_records * 8;
        let gap_len = 2;
        let mut offsets: Vec<u32> = Vec::with_capacity(num_records);
        let mut cursor = 78 + record_info_len + gap_len;
        for rec in &records {
            offsets.push(cursor as u32);
            cursor += rec.len();
        }
    
        let mut out = Vec::with_capacity(cursor);
        // PalmDB header (78 bytes).
        let mut name = [0u8; 32];
        let tn = b"TestBook";
        name[..tn.len()].copy_from_slice(tn);
        out.extend_from_slice(&name);
        out.extend_from_slice(&[0u8; 12]); // attrs, ver, dates
        out.extend_from_slice(&[0u8; 12]); // backup, modnum, appinfo
        out.extend_from_slice(&[0u8; 4]); // sort info
        out.extend_from_slice(b"BOOK");
        out.extend_from_slice(b"MOBI");
        out.extend_from_slice(&[0u8; 4]); // uid seed
        out.extend_from_slice(&[0u8; 4]); // next record list
        out.extend_from_slice(&(num_records as u16).to_be_bytes());
        assert_eq!(out.len(), 78);
    
        for (i, off) in offsets.iter().enumerate() {
            out.extend_from_slice(&off.to_be_bytes());
            out.push(0); // attributes
            out.extend_from_slice(&[0u8, 0, i as u8]); // 3-byte unique id
        }
        out.extend_from_slice(&[0u8, 0]); // 2-byte gap
    
        for rec in &records {
            out.extend_from_slice(rec);
        }
        out
    }
    
    /// Find the EXTH block inside a MOBI file and return parsed (type, data)
    /// records. Used to verify rewrite output.
    fn parse_exth_records(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
        // PalmDB record 0 offset.
        let record0_start = read_u32_be(data, 78) as usize;
        let record0_end = if read_u16_be(data, 76) > 1 {
            read_u32_be(data, 78 + 8) as usize
        } else {
            data.len()
        };
        let record0 = &data[record0_start..record0_end];
        // MOBI header length at record0 offset 20.
        let mobi_header_length = read_u32_be(record0, 20) as usize;
        // EXTH starts at 16 + mobi_header_length.
        let exth_start = 16 + mobi_header_length;
        assert_eq!(&record0[exth_start..exth_start + 4], b"EXTH", "expected EXTH magic");
        let padded_len = read_u32_be(record0, exth_start + 4) as usize;
        let count = read_u32_be(record0, exth_start + 8) as usize;
    
        let mut records = Vec::with_capacity(count);
        let mut pos = exth_start + 12;
        let end = exth_start + padded_len;
        for _ in 0..count {
            let rtype = read_u32_be(record0, pos);
            let rlen = read_u32_be(record0, pos + 4) as usize;
            assert!(rlen >= 8);
            assert!(pos + rlen <= end);
            let payload = record0[pos + 8..pos + rlen].to_vec();
            records.push((rtype, payload));
            pos += rlen;
        }
        records
    }
    
    fn default_exth() -> Vec<(u32, Vec<u8>)> {
        vec![
            (100, b"Jane Doe".to_vec()),                    // author
            (503, b"Original Title".to_vec()),              // updated title
            (524, b"en".to_vec()),                          // language
            (103, b"An original description.".to_vec()),    // description
            (201, 0u32.to_be_bytes().to_vec()),             // cover offset
        ]
    }
    
    fn write_synthetic(name: &str, title: &str, exth: &[(u32, Vec<u8>)]) -> PathBuf {
        let bytes = build_synthetic_mobi(title, exth);
        let p = tmp_path(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(&bytes).unwrap();
        p
    }
    
    // ---------------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------------
    
    #[test]
    fn cli_rewrite_title_updates_exth_503() {
        let input = write_synthetic("title_in.mobi", "Original Title", &default_exth());
        let output = tmp_path("title_out.mobi");
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--title",
                "Brand New Title",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        let out_bytes = std::fs::read(&output).unwrap();
        let records = parse_exth_records(&out_bytes);
        assert_eq!(
            records
                .iter()
                .find(|(t, _)| *t == 503)
                .map(|(_, d)| d.as_slice()),
            Some(b"Brand New Title".as_slice())
        );
    }
    
    #[test]
    fn cli_rewrite_multiple_authors() {
        let input = write_synthetic("multi_author_in.mobi", "T", &default_exth());
        let output = tmp_path("multi_author_out.mobi");
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--author",
                "Alice",
                "--author",
                "Bob",
                "--author",
                "Carol",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        let records = parse_exth_records(&std::fs::read(&output).unwrap());
        let authors: Vec<&Vec<u8>> = records
            .iter()
            .filter(|(t, _)| *t == 100)
            .map(|(_, d)| d)
            .collect();
        assert_eq!(authors.len(), 3);
        assert_eq!(authors[0], b"Alice");
        assert_eq!(authors[1], b"Bob");
        assert_eq!(authors[2], b"Carol");
    }
    
    #[test]
    fn cli_report_json_emits_structured_output_on_stdout() {
        let input = write_synthetic("json_in.mobi", "Original Title", &default_exth());
        let output = tmp_path("json_out.mobi");
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--language",
                "fr",
                "--report-json",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Must be a JSON object with the expected top-level keys.
        assert!(stdout.trim().starts_with('{'));
        assert!(stdout.contains("\"input_path\""));
        assert!(stdout.contains("\"output_path\""));
        assert!(stdout.contains("\"no_op\":false"));
        assert!(stdout.contains("\"changes\":["));
        assert!(stdout.contains("\"exth_type\":524"));
    }
    
    #[test]
    fn cli_dry_run_does_not_write_output() {
        let input = write_synthetic("dry_in.mobi", "Original Title", &default_exth());
        let output = tmp_path("dry_out.mobi");
        assert!(!output.exists());
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--title",
                "Something Totally Different",
                "--dry-run",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        assert!(!output.exists(), "dry-run must not write output file");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("(dry-run)"), "stderr should flag dry-run: {}", stderr);
    }
    
    #[test]
    fn cli_no_changes_needed_is_noop() {
        let input = write_synthetic("noop_in.mobi", "Original Title", &default_exth());
        let output = tmp_path("noop_out.mobi");
        // Pass the same title and language that are already in the file.
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--title",
                "Original Title",
                "--language",
                "en",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("No metadata changes needed"), "stderr: {}", stderr);
        // Output file must be byte-identical to input.
        let in_bytes = std::fs::read(&input).unwrap();
        let out_bytes = std::fs::read(&output).unwrap();
        assert_eq!(in_bytes, out_bytes, "byte-stable no-op must copy input verbatim");
    }
    
    #[test]
    fn cli_cover_replacement_via_file_path() {
        let input = write_synthetic("cover_in.mobi", "T", &default_exth());
        let output = tmp_path("cover_out.mobi");
        // Write a fake JPEG to a temp path.
        let mut cover_bytes = vec![0xFFu8, 0xD8, 0xFF, 0xE0];
        cover_bytes.extend(std::iter::repeat(0xAA).take(512));
        cover_bytes.extend_from_slice(&[0xFF, 0xD9]);
        let cover_path = tmp_path("cover.jpg");
        std::fs::write(&cover_path, &cover_bytes).unwrap();
    
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--cover",
                cover_path.to_str().unwrap(),
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(out.status.success(), "{}", dump(&out));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("Replaced cover image record"), "stderr: {}", stderr);
        // Verify: cover record in output file contains our new bytes.
        let out_bytes = std::fs::read(&output).unwrap();
        // Record 2 is the cover in our synthetic layout. Read its offset and length.
        let off2 = read_u32_be(&out_bytes, 78 + 2 * 8) as usize;
        let end2 = out_bytes.len();
        assert_eq!(&out_bytes[off2..end2], &cover_bytes[..]);
    }
    
    #[test]
    fn cli_drm_rejection_exits_nonzero() {
        // Build a synthetic MOBI with a DRM EXTH record (401) present.
        let mut exth = default_exth();
        exth.push((401, vec![0, 0, 0, 1]));
        let input = write_synthetic("drm_in.mobi", "T", &exth);
        let output = tmp_path("drm_out.mobi");
        let out = Command::new(kindling_bin())
            .args([
                "rewrite-metadata",
                input.to_str().unwrap(),
                "-o",
                output.to_str().unwrap(),
                "--title",
                "Pwned",
            ])
            .output()
            .expect("failed to run kindling-cli");
        assert!(!out.status.success(), "expected DRM rejection: {}", dump(&out));
        assert!(!output.exists(), "DRM-rejected files must not produce output");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.to_lowercase().contains("drm"), "stderr should mention DRM: {}", stderr);
    }
}

#[allow(unused_imports)]
mod phase2 {
    #[test]
    fn validate_fixed_layout_errors_reports_r11_rules() {
        use std::path::PathBuf;
        use std::process::Command;

        let opf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fixed_layout_errors")
            .join("fixed_layout_errors.opf");
        let out = Command::new(env!("CARGO_BIN_EXE_kindling-cli"))
            .arg("validate")
            .arg(opf.to_str().unwrap())
            .output()
            .expect("failed to spawn kindling-cli validate");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let dump = format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            stdout,
            stderr,
        );

        assert_eq!(
            out.status.code(),
            Some(1),
            "fixed_layout_errors should exit 1\n{}",
            dump
        );
        // Every R11.* rule except R11.1 (which fires only when OPF is missing
        // the rendition:layout declaration) should be triggered by this
        // fixture. R11.1 has its own dedicated fixture below.
        for rule_id in &["R11.3", "R11.4", "R11.5", "R11.6", "R11.7", "R11.8", "R11.9"] {
            assert!(
                stdout.contains(rule_id),
                "expected rule id {} in fixed_layout_errors output\n{}",
                rule_id,
                dump
            );
        }
    }

    #[test]
    fn validate_fixed_layout_missing_opf_declaration_fires_r11_1() {
        use std::path::PathBuf;
        use std::process::Command;

        let opf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fixed_layout_missing_opf")
            .join("fixed_layout_missing_opf.opf");
        let out = Command::new(env!("CARGO_BIN_EXE_kindling-cli"))
            .arg("validate")
            .arg(opf.to_str().unwrap())
            .output()
            .expect("failed to spawn kindling-cli validate");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let dump = format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            stdout,
            stderr,
        );

        assert!(
            stdout.contains("R11.1"),
            "expected R11.1 on OPF without rendition:layout\n{}",
            dump
        );
    }

    #[test]
    fn validate_clean_book_does_not_fire_r11_rules() {
        use std::path::PathBuf;
        use std::process::Command;

        // clean_book is reflowable; none of R11.* should fire on it.
        let opf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("clean_book")
            .join("clean_book.opf");
        let out = Command::new(env!("CARGO_BIN_EXE_kindling-cli"))
            .arg("validate")
            .arg(opf.to_str().unwrap())
            .output()
            .expect("failed to spawn kindling-cli validate");
        let stdout = String::from_utf8_lossy(&out.stdout);
        for rule_id in &[
            "R11.1", "R11.2", "R11.3", "R11.4", "R11.5", "R11.6", "R11.7",
            "R11.8", "R11.9",
        ] {
            assert!(
                !stdout.contains(rule_id),
                "rule id {} must not fire on reflowable clean_book:\n{}",
                rule_id,
                stdout
            );
        }
    }
    // PHASE2-TEST: C
    // PHASE2-TEST: D
    // PHASE2-TEST: E
    #[test]
    fn validate_cross_refs_errors_reports_r9_rules() {
        use std::path::PathBuf;
        use std::process::Command;

        let opf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("cross_refs_errors")
            .join("cross_refs_errors.opf");
        let out = Command::new(env!("CARGO_BIN_EXE_kindling-cli"))
            .arg("validate")
            .arg(opf.to_str().unwrap())
            .output()
            .expect("failed to spawn kindling-cli validate");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        let dump = format!(
            "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            out.status.code(),
            stdout,
            stderr,
        );

        assert_eq!(
            out.status.code(),
            Some(1),
            "cross_refs_errors should exit 1\n{}",
            dump
        );
        // The fixture is built to fire at least these rules; assert each is
        // present so a future refactor can't silently drop one.
        for rule_id in &[
            "R9.1", "R9.3", "R9.4", "R9.5", "R9.6", "R9.7", "R9.8", "R9.9", "R9.10", "R9.11",
        ] {
            assert!(
                stdout.contains(rule_id),
                "expected rule id {} in cross_refs_errors output\n{}",
                rule_id,
                dump,
            );
        }
    }
    // PHASE2-TEST: G
    // PHASE2-TEST: H
    // PHASE2-TEST: I
    // PHASE2-TEST: K
}

