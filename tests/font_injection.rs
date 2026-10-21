//! Font injection through the CLI, including the emitted KF8 resources.
mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "kindling_injected_fonts_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("source")).unwrap();
        let source = dir.join("source");
        for name in ["one", "two"] {
            fs::write(source.join(format!("{name}.html")), format!("<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{name}</title></head><body><p style=\"font-family: Missing; color: red\">x <b>x</b> <i>x</i></p></body></html>")).unwrap();
        }
        fs::write(source.join("book.opf"), r#"<package version="2.0" xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Font injection</dc:title><dc:language>en</dc:language></metadata><manifest><item id="one" href="one.html" media-type="application/xhtml+xml"/><item id="two" href="two.html" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="one"/><itemref idref="two"/></spine></package>"#).unwrap();
        Self(dir)
    }
    fn opf(&self) -> PathBuf {
        self.0.join("source/book.opf")
    }
    fn build(&self, input: &Path, extra: &[&str]) -> Output {
        Command::new(common::kindling_bin())
            .arg("build")
            .arg(input)
            .args(["--no-validate", "--no-embed-source", "-o"])
            .arg(self.0.join("out.azw3"))
            .args(extra)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn font(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/fonts")
        .join(name)
}

fn assert_ok(result: &Output) {
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn injected_fonts_reach_every_spine_document_in_opf_epub_and_dual_output() {
    for epub in [false, true] {
        for legacy in [false, true] {
            let fixture = Fixture::new();
            let opf_before = fs::read(fixture.opf()).unwrap();
            let html_before = fs::read(fixture.0.join("source/one.html")).unwrap();
            let input = if epub {
                let path = fixture.0.join("input.epub");
                fs::write(
                    &path,
                    kindling::epub::create_epub_from_dir(&fixture.0.join("source")).unwrap(),
                )
                .unwrap();
                path
            } else {
                fixture.opf()
            };
            let input_before = fs::read(&input).unwrap();
            let paths = [font("regular.ttf"), font("bold.ttf"), font("italic.ttf")];
            let mut args = vec![
                "--embed-font",
                paths[0].to_str().unwrap(),
                "--embed-font",
                paths[1].to_str().unwrap(),
                "--embed-font",
                paths[2].to_str().unwrap(),
            ];
            if legacy {
                args.push("--legacy-mobi");
            }
            assert_ok(&fixture.build(&input, &args));
            let data = fs::read(fixture.0.join("out.azw3")).unwrap();
            let parsed = common::parse_mobi_file(&data).unwrap();
            assert_eq!(parsed.is_kf8_only(), !legacy);
            let section = parsed.kf8_or_kf7();
            let text = String::from_utf8(common::extract_text_blob(&parsed, section)).unwrap();
            // These documents had no CSS links. A FONT record and an unlinked
            // stylesheet would pass a byte-presence test but render no font.
            assert_eq!(
                text.matches("href=\"kindle:flow:0001?mime=text/css\"")
                    .count(),
                2
            );
            assert!(text.contains("body, body p { font-family: \"Kindling Test\" !important; }"));
            assert!(text.contains("font-weight: 700; font-style: normal"));
            assert!(text.contains("font-weight: 400; font-style: italic"));
            assert!(text.contains("color: red"));
            let records: Vec<_> = (0..parsed.palmdb.num_records)
                .filter_map(|i| {
                    let record = parsed.palmdb.record(&data, i);
                    record.starts_with(b"FONT").then_some((i, record))
                })
                .collect();
            assert_eq!(records.len(), 3);
            for ((index, record), path) in records.iter().zip(&paths) {
                let mut payload = Vec::new();
                flate2::read::ZlibDecoder::new(&record[24..])
                    .read_to_end(&mut payload)
                    .unwrap();
                assert_eq!(payload, fs::read(path).unwrap());
                // Dual files share the resources addressed by the KF7 header.
                let relative = index + 1 - parsed.kf7.header.first_image_record as usize;
                assert!(text.contains(&format!(
                    "kindle:embed:{relative:04}?mime=application/x-font-ttf"
                )));
            }
            assert_eq!(fs::read(&input).unwrap(), input_before);
            assert_eq!(fs::read(fixture.opf()).unwrap(), opf_before);
            assert_eq!(
                fs::read(fixture.0.join("source/one.html")).unwrap(),
                html_before
            );
            assert_eq!(fs::read_dir(fixture.0.join("source")).unwrap().count(), 3);
        }
    }
}

#[test]
fn metadata_fallback_order_and_family_override_keep_face_styles() {
    let paths = [
        font("regular.ttf"),
        font("bold.ttf"),
        font("italic.ttf"),
        font("fallback.ttf"),
    ];
    let fonts = kindling::font_injection::FontInjection::from_paths(&paths, None).unwrap();
    assert_eq!(fonts.families(), ["Kindling Test", "Kindling Preferred"]);
    let fixture = Fixture::new();
    let override_name = "Custom \"Reading\" \\ Face";
    let regular = font("regular.ttf");
    let italic = font("italic.ttf");
    assert_ok(&fixture.build(
        &fixture.opf(),
        &[
            "--embed-font",
            regular.to_str().unwrap(),
            "--embed-font",
            italic.to_str().unwrap(),
            "--font-family",
            override_name,
        ],
    ));
    let data = fs::read(fixture.0.join("out.azw3")).unwrap();
    let parsed = common::parse_mobi_file(&data).unwrap();
    let text = String::from_utf8(common::extract_text_blob(&parsed, parsed.kf8_or_kf7())).unwrap();
    assert!(text.contains(r#"font-family: "Custom \"Reading\" \\ Face""#));
    assert!(text.contains("font-style: italic"));
}

#[test]
fn injected_resources_follow_existing_images_and_manifest_fonts() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 20]))
        .save(source.join("picture.jpg"))
        .unwrap();
    fs::copy(font("fallback.ttf"), source.join("publisher.ttf")).unwrap();
    fs::write(source.join("style.css"), "@font-face { font-family: Publisher; src: url(publisher.ttf); } body { font-family: Publisher; }").unwrap();
    let opf = fs::read_to_string(fixture.opf()).unwrap().replace("</manifest>", r#"<item id="image" href="picture.jpg" media-type="image/jpeg"/><item id="publisher" href="publisher.ttf" media-type="font/ttf"/><item id="css" href="style.css" media-type="text/css"/></manifest>"#);
    fs::write(fixture.opf(), opf).unwrap();
    fs::write(source.join("one.html"), "<html><head><link rel=\"stylesheet\" href=\"style.css\"/></head><body><p>x</p><img src=\"picture.jpg\"/></body></html>").unwrap();
    let regular = font("regular.ttf");
    assert_ok(&fixture.build(&fixture.opf(), &["--embed-font", regular.to_str().unwrap()]));
    let data = fs::read(fixture.0.join("out.azw3")).unwrap();
    let parsed = common::parse_mobi_file(&data).unwrap();
    let section = parsed.kf8_or_kf7();
    let text = String::from_utf8(common::extract_text_blob(&parsed, section)).unwrap();
    let first = parsed.kf7.header.first_image_record as usize;
    assert!(
        parsed
            .palmdb
            .record(&data, first)
            .starts_with(&[0xff, 0xd8])
    );
    for (offset, expected) in [(1, font("fallback.ttf")), (2, regular)] {
        let record = parsed.palmdb.record(&data, first + offset);
        assert!(record.starts_with(b"FONT"));
        let mut decoded = Vec::new();
        flate2::read::ZlibDecoder::new(&record[24..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, fs::read(expected).unwrap());
        assert!(text.contains(&format!(
            "kindle:embed:{:04}?mime=application/x-font-ttf",
            offset + 1
        )));
    }
    assert!(text.contains("font-family: Publisher"));
    assert!(text.contains("font-family: \"Kindling Test\""));
}

#[test]
fn injection_rejects_conflicts_bad_fonts_and_unsupported_content() {
    let fixture = Fixture::new();
    let regular = font("regular.ttf");
    let path = regular.to_str().unwrap();
    for args in [
        vec!["--embed-font", path, "--force-user-fonts"],
        vec!["--font-family", "Missing path"],
        vec!["--embed-font", path, "--font-family", " "],
        vec!["--embed-font", path, "--embed-font", path],
    ] {
        assert!(!fixture.build(&fixture.opf(), &args).status.success());
        assert!(!fixture.0.join("out.azw3").exists());
    }
    for bytes in [b"wOFFfake".as_slice(), b"ttcfFake", &[0, 1, 0, 0, 0]] {
        let invalid = fixture.0.join("invalid.ttf");
        fs::write(&invalid, bytes).unwrap();
        assert!(
            !fixture
                .build(&fixture.opf(), &["--embed-font", invalid.to_str().unwrap()])
                .status
                .success()
        );
    }
    let old = fs::read_to_string(fixture.opf()).unwrap();
    fs::write(
        fixture.opf(),
        old.replace(
            "</metadata>",
            "<meta name=\"fixed-layout\" content=\"true\"/></metadata>",
        ),
    )
    .unwrap();
    let result = fixture.build(&fixture.opf(), &["--embed-font", path]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("reflowable books only"));
    fs::write(fixture.opf(), old).unwrap();
    fs::write(
        fixture.0.join("source/one.html"),
        "<html><body><idx:entry><idx:orth value=\"x\"/>definition</idx:entry></body></html>",
    )
    .unwrap();
    assert!(
        !fixture
            .build(&fixture.opf(), &["--embed-font", path])
            .status
            .success()
    );
}

#[test]
fn default_and_force_user_fonts_preserve_reader_font_choice() {
    let fixture = Fixture::new();
    for args in [vec![], vec!["--force-user-fonts"]] {
        assert_ok(&fixture.build(&fixture.opf(), &args));
        let data = fs::read(fixture.0.join("out.azw3")).unwrap();
        let parsed = common::parse_mobi_file(&data).unwrap();
        let text =
            String::from_utf8(common::extract_text_blob(&parsed, parsed.kf8_or_kf7())).unwrap();
        assert!(!text.contains("font-family"));
        assert!(text.contains("color: red"));
        assert!(
            !(0..parsed.palmdb.num_records)
                .any(|i| parsed.palmdb.record(&data, i).starts_with(b"FONT"))
        );
    }
}

#[test]
fn cff_opentype_font_uses_the_opentype_resource_type() {
    let fixture = Fixture::new();
    let otf = font("regular.otf");
    assert_ok(&fixture.build(&fixture.opf(), &["--embed-font", otf.to_str().unwrap()]));
    let data = fs::read(fixture.0.join("out.azw3")).unwrap();
    let parsed = common::parse_mobi_file(&data).unwrap();
    let text = String::from_utf8(common::extract_text_blob(&parsed, parsed.kf8_or_kf7())).unwrap();
    assert!(text.contains("kindle:embed:0001?mime=application/vnd.ms-opentype"));
    assert!(text.contains("font-family: \"Kindling CFF\""));
    let record = parsed
        .palmdb
        .record(&data, parsed.kf7.header.first_image_record as usize);
    let mut decoded = Vec::new();
    flate2::read::ZlibDecoder::new(&record[24..])
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, fs::read(otf).unwrap());
}
