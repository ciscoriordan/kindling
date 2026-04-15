// R5.2.3, R5.3.1: NCX content/guide reference targets must exist in the manifest.

use std::collections::HashSet;
use std::fs;

use super::helpers::extract_attr;
use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct NavLinksChecks;

impl Check for NavLinksChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R5.2.3", "R5.3.1"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        let manifest_hrefs: HashSet<String> = opf
            .manifest
            .values()
            .map(|(href, _)| href.clone())
            .collect();

        let strip_fragment = |href: &str| -> String {
            match href.find('#') {
                Some(i) => href[..i].to_string(),
                None => href.to_string(),
            }
        };

        // NCX <content src="..."/>
        let ncx_href: Option<String> = opf
            .manifest
            .values()
            .find(|(_, mt)| mt == "application/x-dtbncx+xml")
            .map(|(href, _)| href.clone());
        if let Some(ncx_href) = ncx_href {
            let ncx_path = opf.base_dir.join(&ncx_href);
            if let Ok(ncx_content) = fs::read_to_string(&ncx_path) {
                let mut rest = ncx_content.as_str();
                while let Some(idx) = rest.find("<content") {
                    rest = &rest[idx + "<content".len()..];
                    let Some(end) = rest.find('>') else { break };
                    let tag = &rest[..end];
                    if let Some(src) = extract_attr(tag, "src") {
                        let file_part = strip_fragment(&src);
                        if !file_part.is_empty() && !manifest_hrefs.contains(&file_part) {
                            report.emit_at(
                                "R5.2.3",
                                format!("NCX references '{}'.", src),
                                Some(ncx_path.clone()),
                                None,
                            );
                        }
                    }
                    rest = &rest[end..];
                }
            }
        }

        // OPF <guide><reference href="..."/>
        if let Ok(opf_content) = fs::read_to_string(&epub.opf_path) {
            let mut rest = opf_content.as_str();
            while let Some(idx) = rest.find("<reference") {
                rest = &rest[idx + "<reference".len()..];
                let Some(end) = rest.find('>') else { break };
                let tag = &rest[..end];
                if let Some(href) = extract_attr(tag, "href") {
                    let file_part = strip_fragment(&href);
                    if !file_part.is_empty() && !manifest_hrefs.contains(&file_part) {
                        report.emit_at(
                            "R5.3.1",
                            format!("Guide references '{}'.", href),
                            Some(epub.opf_path.clone()),
                            None,
                        );
                    }
                }
                rest = &rest[end..];
            }
        }
    }
}
