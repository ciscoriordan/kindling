// Section 10.4: image format, size, dimensions (plus R4.2.2 for missing files).

use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::kdp_rules::Severity;
use crate::profile::Profile;
use crate::validate::ValidationReport;

const SUPPORTED_IMAGE_MEDIA: &[&str] = &[
    "image/jpeg",
    "image/jpg",
    "image/png",
    "image/gif",
    "image/svg+xml",
];

pub struct ImageChecks;

impl Check for ImageChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R10.4.1", "R10.4.2a", "R10.4.2b", "R4.2.2"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        let mut items: Vec<(String, String, String)> = opf
            .manifest
            .iter()
            .filter(|(_, (_, mt))| mt.starts_with("image/"))
            .map(|(id, (href, mt))| (id.clone(), href.clone(), mt.clone()))
            .collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        for (_id, href, media_type) in &items {
            let path = opf.base_dir.join(href);

            if !SUPPORTED_IMAGE_MEDIA.contains(&media_type.as_str()) {
                report.emit_at(
                    "R10.4.1",
                    format!("{} has media-type '{}'.", href, media_type),
                    Some(PathBuf::from(href)),
                    None,
                );
            } else if epub.profile == Profile::Dict && media_type == "image/svg+xml" {
                // A dictionary is Mobi 7 (KPG 16.3.1), which cannot show SVG
                // (KPG 11.4.1), and kindling has no rasterizer, so the image
                // would reach the device as bytes it cannot draw. A warning
                // rather than an error: the rest of the dictionary still works.
                report.emit_at_level(
                    "R10.4.1",
                    Severity::Warning,
                    format!(
                        "{} is SVG, which a dictionary cannot show: dictionaries are Mobi 7, \
                         and Mobi 7 has no SVG.",
                        href
                    ),
                    Some(PathBuf::from(href)),
                    None,
                );
            }

            if !path.exists() {
                report.emit_at(
                    "R4.2.2",
                    format!("Image {} referenced in manifest but file is missing.", href),
                    Some(PathBuf::from(href)),
                    None,
                );
                continue;
            }

            if let Ok(md) = fs::metadata(&path) {
                if md.len() > 127 * 1024 {
                    report.emit_at(
                        "R10.4.2a",
                        format!("{} is {} bytes.", href, md.len()),
                        Some(PathBuf::from(href)),
                        None,
                    );
                }
            }

            if media_type != "image/svg+xml" {
                if let Ok((w, h)) = image::image_dimensions(&path) {
                    let mp = (w as u64) * (h as u64);
                    if mp > 5_000_000 {
                        report.emit_at(
                            "R10.4.2b",
                            format!("{} is {}x{} ({} MP).", href, w, h, mp / 1_000_000),
                            Some(PathBuf::from(href)),
                            None,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validate a one-image source, as a dictionary or as a book.
    fn findings_for_svg(dictionary: bool) -> ValidationReport {
        let dir = std::env::temp_dir().join(format!(
            "kindling_images_svg_{}_{}_{}",
            dictionary,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let x_metadata = if dictionary {
            "<x-metadata><DictionaryInLanguage>en</DictionaryInLanguage>\
             <DictionaryOutLanguage>en</DictionaryOutLanguage></x-metadata>"
        } else {
            ""
        };
        fs::write(
            dir.join("content.opf"),
            format!(
                r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:language>en</dc:language><dc:identifier id="uid">x</dc:identifier>{x_metadata}</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/><item id="i" href="i.svg" media-type="image/svg+xml"/></manifest>
<spine><itemref idref="c"/></spine></package>"#
            ),
        )
        .unwrap();
        fs::write(dir.join("c.xhtml"), "<html><body><p>x</p></body></html>").unwrap();
        fs::write(
            dir.join("i.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
        )
        .unwrap();
        let epub =
            crate::extracted::ExtractedEpub::from_opf_path(&dir.join("content.opf")).unwrap();
        let mut report = ValidationReport::new();
        ImageChecks.run(&epub, &mut report);
        fs::remove_dir_all(&dir).ok();
        report
    }

    #[test]
    fn an_svg_in_a_dictionary_is_a_warning() {
        let report = findings_for_svg(true);
        let svg: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.rule_id == Some("R10.4.1"))
            .collect();
        assert_eq!(svg.len(), 1, "{svg:?}");
        assert_eq!(svg[0].level, Severity::Warning);
    }

    #[test]
    fn an_svg_in_a_book_is_fine() {
        let report = findings_for_svg(false);
        assert!(
            report.findings.iter().all(|f| f.rule_id != Some("R10.4.1")),
            "{:?}",
            report.findings
        );
    }
}
