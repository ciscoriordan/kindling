// Section 10.4: image format, size, dimensions (plus R4.2.2 for missing files).

use std::fs;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
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
