// Section 4.2 cover image checks.

use std::path::PathBuf;

use super::helpers::looks_like_html_cover_page;
use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct CoverChecks;

impl Check for CoverChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R4.2.1", "R4.2.2", "R4.2.3", "R4.2.4"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        let cover_href = opf.get_cover_image_href();
        if cover_href.is_none() {
            report.emit("R4.2.1", "");
            return;
        }
        let cover_href = cover_href.unwrap();

        let cover_path = opf.base_dir.join(&cover_href);
        if !cover_path.exists() {
            report.emit_at(
                "R4.2.2",
                format!("File: {}", cover_href),
                Some(PathBuf::from(&cover_href)),
                None,
            );
        } else if let Ok((w, h)) = image::image_dimensions(&cover_path) {
            let shortest = w.min(h);
            if shortest < 500 {
                report.emit_at(
                    "R4.2.3",
                    format!("Image is {}x{} px (shortest side {} px).", w, h, shortest),
                    Some(PathBuf::from(&cover_href)),
                    None,
                );
            }
        }

        for (idref, href) in &opf.spine_items {
            if looks_like_html_cover_page(&opf.base_dir, idref, href, &cover_href) {
                report.emit_at(
                    "R4.2.4",
                    format!("Spine entry '{}' ({}) matches the cover page pattern.", idref, href),
                    Some(PathBuf::from(href)),
                    None,
                );
            }
        }
    }
}
