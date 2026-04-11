//! End-to-end CI test for the `kindling repair` subcommand.
//!
//! Builds synthetic EPUBs with the `zip` crate inside the test, runs the
//! compiled `kindling-cli` binary against them, and asserts exit codes,
//! stderr contents, report shape, and output-file correctness. Catches
//! regressions in CLI wiring that unit tests in `src/repair.rs` would miss.

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
