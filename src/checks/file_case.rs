// Section 6.5: manifest file references must match case on disk.

use std::fs;
use std::path::{Path, PathBuf};

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct FileCaseChecks;

impl Check for FileCaseChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R6.5"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;
        for (_id, (href, _mt)) in &opf.manifest {
            let rel = Path::new(href);
            let parent = opf.base_dir.join(rel.parent().unwrap_or(Path::new("")));
            let file_name = match rel.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };

            let entries = match fs::read_dir(&parent) {
                Ok(e) => e,
                Err(_) => continue,
            };

            let mut case_insensitive_match: Option<String> = None;
            let mut exact_match = false;
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name == file_name {
                        exact_match = true;
                        break;
                    }
                    if name.eq_ignore_ascii_case(file_name) {
                        case_insensitive_match = Some(name.to_string());
                    }
                }
            }

            if !exact_match {
                if let Some(actual) = case_insensitive_match {
                    report.emit_at(
                        "R6.5",
                        format!(
                            "Manifest references '{}' but file on disk is '{}'.",
                            file_name, actual
                        ),
                        Some(PathBuf::from(href)),
                        None,
                    );
                }
            }
        }
    }
}
