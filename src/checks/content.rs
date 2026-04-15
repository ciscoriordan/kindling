// Section 6.x / 10.3.1 / 10.5.1 / 17: content-file HTML/CSS checks.

use std::fs;
use std::path::PathBuf;

use super::helpers::{
    contains_tag, count_table_rows, find_nested_p, has_negative_css, heading_with_text_align,
    try_parse_xml,
};
use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

/// Tags explicitly called out as unsupported in KPG 6.1 / 18.1.
const UNSUPPORTED_TAGS: &[&str] = &[
    "script",
    "form",
    "input",
    "button",
    "select",
    "textarea",
    "fieldset",
    "legend",
    "frame",
    "frameset",
    "iframe",
    "noframes",
    "applet",
    "embed",
    "object",
    "canvas",
];

pub struct ContentChecks;

impl Check for ContentChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R6.1", "R6.2", "R6.3", "R6.4", "R10.3.1", "R10.5.1", "R17.1"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;
        for (_, href) in &opf.spine_items {
            let full = opf.base_dir.join(href);
            let content = match fs::read_to_string(&full) {
                Ok(c) => c,
                Err(_) => continue,
            };
            check_content_html(href, &content, report);
        }
    }
}

fn check_content_html(href: &str, content: &str, report: &mut ValidationReport) {
    let file = Some(PathBuf::from(href));

    // 6.1 Well-formed XHTML (skipped for HTML5 doctypes).
    if !content.contains("<!DOCTYPE html>") && !content.contains("<!doctype html>") {
        if let Err(e) = try_parse_xml(content) {
            report.emit_at("R6.1", format!("Parse error: {}", e), file.clone(), None);
        }
    }

    // 6.3 Avoid scripting.
    for (line_no, line) in content.lines().enumerate() {
        if contains_tag(line, "script") {
            report.emit_at("R6.3", "", file.clone(), Some(line_no + 1));
        }
    }

    // 6.4 Avoid nested <p> tags.
    if let Some(line_no) = find_nested_p(content) {
        report.emit_at("R6.4", "", file.clone(), Some(line_no));
    }

    // 6.2 Avoid negative CSS values.
    for (line_no, line) in content.lines().enumerate() {
        if has_negative_css(line) {
            report.emit_at("R6.2", "", file.clone(), Some(line_no + 1));
        }
    }

    // 10.3.1 Heading alignment.
    for (line_no, line) in content.lines().enumerate() {
        if let Some(tag) = heading_with_text_align(line) {
            report.emit_at(
                "R10.3.1",
                format!("Tag: <{}>.", tag),
                file.clone(),
                Some(line_no + 1),
            );
        }
    }

    // 17 / 18.1 Unsupported tags.
    for (line_no, line) in content.lines().enumerate() {
        for &tag in UNSUPPORTED_TAGS {
            if contains_tag(line, tag) {
                report.emit_at(
                    "R17.1",
                    format!("Tag: <{}>.", tag),
                    file.clone(),
                    Some(line_no + 1),
                );
            }
        }
    }

    // 10.5.1 Avoid large tables (> 50 rows).
    let table_rows = count_table_rows(content);
    for (table_idx, row_count) in table_rows.iter().enumerate() {
        if *row_count > 50 {
            report.emit_at(
                "R10.5.1",
                format!("Table #{} has {} rows.", table_idx + 1, row_count),
                file.clone(),
                None,
            );
        }
    }
}
