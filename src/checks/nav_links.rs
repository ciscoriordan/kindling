// R5.2.3, R5.3.1: NCX content/guide reference targets must exist in the manifest.

use std::collections::HashSet;
use std::fs;

use super::Check;
use super::helpers::extract_attr;
use crate::extracted::ExtractedEpub;
use crate::links::percent_decode;
use crate::validate::ValidationReport;

pub struct NavLinksChecks;

impl Check for NavLinksChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R5.2.3", "R5.3.1"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // Every href is compared as the path it names from the OPF's
        // directory, percent-decoded, the way the TOC builder reads it
        // (nav.rs), so `./a.xhtml`, `a%20b.xhtml` and `a b.xhtml` agree.
        let manifest_hrefs: HashSet<String> = opf
            .manifest
            .values()
            .filter_map(|(href, _)| resolve("", href))
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
            // An NCX src is relative to the NCX, and manifest hrefs are
            // relative to the OPF, so an NCX in a subdirectory has to have its
            // links resolved before they are compared. They were compared as
            // written, which called every link in such an NCX broken.
            let ncx_dir = ncx_href.rfind('/').map_or("", |i| &ncx_href[..=i]);
            if let Ok(ncx_content) = fs::read_to_string(&ncx_path) {
                let mut rest = ncx_content.as_str();
                while let Some(idx) = rest.find("<content") {
                    rest = &rest[idx + "<content".len()..];
                    let Some(end) = rest.find('>') else { break };
                    let tag = &rest[..end];
                    if let Some(src) = extract_attr(tag, "src") {
                        let file_part = strip_fragment(&src);
                        if !file_part.is_empty()
                            && !resolve(ncx_dir, &file_part)
                                .is_some_and(|path| manifest_hrefs.contains(&path))
                        {
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
                    if !file_part.is_empty()
                        && !resolve("", &file_part)
                            .is_some_and(|path| manifest_hrefs.contains(&path))
                    {
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

/// Resolve `href`, relative to the directory `base` (empty, or ending in
/// `/`), to a percent-decoded path from the OPF's directory, the way the TOC
/// builder reads it (nav.rs). None when a `..` climbs out of the OPF's
/// directory: kindlegen resolves such a link to a file above it and refuses to
/// build the TOC, where `links::normalize_path` would quietly keep it inside.
fn resolve(base: &str, href: &str) -> Option<String> {
    let joined = format!("{base}{}", percent_decode(href.trim()));
    let mut segments: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            s => segments.push(s),
        }
    }
    Some(segments.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validate a one-page book whose NCX is at `ncx_href`, links to
    /// `ncx_src`, and whose page is `content_href`, with an optional guide
    /// link, and return the findings as "id: message" strings.
    fn nav_findings(
        ncx_href: &str,
        ncx_src: &str,
        guide_href: Option<&str>,
        content_href: &str,
    ) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!(
            "kindling_nav_links_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let guide = guide_href
            .map(|h| format!(r#"<guide><reference type="text" title="Start" href="{h}"/></guide>"#))
            .unwrap_or_default();
        let ncx_path = dir.join(ncx_href);
        fs::create_dir_all(ncx_path.parent().unwrap()).unwrap();
        fs::write(
            dir.join("content.opf"),
            format!(
                r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:language>en</dc:language><dc:identifier id="uid">x</dc:identifier></metadata>
<manifest><item id="ncx" href="{ncx_href}" media-type="application/x-dtbncx+xml"/><item id="c" href="{content_href}" media-type="application/xhtml+xml"/></manifest>
<spine toc="ncx"><itemref idref="c"/></spine>{guide}</package>"#
            ),
        )
        .unwrap();
        fs::write(
            &ncx_path,
            format!(
                r#"<ncx><navMap><navPoint id="a"><navLabel><text>A</text></navLabel><content src="{ncx_src}"/></navPoint></navMap></ncx>"#
            ),
        )
        .unwrap();
        let page = dir.join(percent_decode(content_href));
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::write(&page, r#"<html><body><p id="top">x</p></body></html>"#).unwrap();

        let epub = ExtractedEpub::from_opf_path(&dir.join("content.opf")).unwrap();
        let mut report = ValidationReport::new();
        NavLinksChecks.run(&epub, &mut report);
        fs::remove_dir_all(&dir).ok();
        report
            .findings
            .iter()
            .filter_map(|f| f.rule_id.map(|id| format!("{id}: {}", f.message)))
            .collect()
    }

    #[test]
    fn an_ncx_in_a_subdirectory_resolves_its_links_from_there() {
        let clean = nav_findings("toc/toc.ncx", "../text/c.xhtml#top", None, "text/c.xhtml");
        assert!(clean.is_empty(), "{clean:?}");
        let broken = nav_findings("toc/toc.ncx", "../text/missing.xhtml", None, "text/c.xhtml");
        assert_eq!(broken.len(), 1, "{broken:?}");
        assert!(broken[0].starts_with("R5.2.3") && broken[0].contains("missing.xhtml"));
    }

    #[test]
    fn a_link_written_from_the_opf_is_broken_in_a_subdirectory_ncx() {
        // From toc/toc.ncx, text/c.xhtml names toc/text/c.xhtml, which does
        // not exist, and the TOC builder reads it that way too.
        let found = nav_findings("toc/toc.ncx", "text/c.xhtml", None, "text/c.xhtml");
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn a_link_that_climbs_out_of_the_opf_directory_is_broken() {
        let found = nav_findings("toc.ncx", "../c.xhtml", None, "c.xhtml");
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn links_are_compared_decoded_and_normalized() {
        // The manifest spells the name with a space; the NCX and the guide
        // percent-encode it, and the guide adds a ./ segment.
        let found = nav_findings(
            "toc/toc.ncx",
            "../text/My%20File.xhtml",
            Some("./text/My%20File.xhtml#top"),
            "text/My File.xhtml",
        );
        assert!(found.is_empty(), "{found:?}");
    }
}
