// Section 5: navigation (TOC, NCX presence and spine-toc reference).

use std::fs;

use super::helpers::strip_tags_len;
use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct NavigationChecks;

impl Check for NavigationChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R5.1", "R5.2.1", "R5.2.2"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        let ncx_id: Option<String> = opf
            .manifest
            .iter()
            .find(|(_, (_, mt))| mt == "application/x-dtbncx+xml")
            .map(|(id, _)| id.clone());

        if ncx_id.is_none() {
            report.emit("R5.2.1", "");
        }

        if let Ok(raw) = fs::read_to_string(&epub.opf_path) {
            let has_spine_toc = raw.lines().any(|l| {
                let lt = l.trim();
                lt.contains("<spine") && lt.contains("toc=")
            });
            if let Some(ref id) = ncx_id {
                if !has_spine_toc {
                    report.emit("R5.2.2", format!("NCX id: '{}'.", id));
                }
            }
        }

        let mut total_chars: usize = 0;
        for (_, href) in &opf.spine_items {
            let full = opf.base_dir.join(href);
            if let Ok(content) = fs::read_to_string(&full) {
                total_chars += strip_tags_len(&content);
            }
        }
        let approx_pages = total_chars / 1800;
        if approx_pages > 20 && ncx_id.is_none() {
            report.emit("R5.1", format!("Approximately {} pages.", approx_pages));
        }
    }
}
