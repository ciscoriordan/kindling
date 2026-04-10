//! Amazon Kindle Publishing Guidelines rule catalog.
//!
//! This is the single source of truth for the validation rules used by
//! `kindling validate`. Each rule references a numbered section of the KPG
//! PDF at the version defined below. When Amazon updates the guidelines,
//! only this file (and any affected check logic in `validate.rs`) needs to
//! change.
//!
//! Check functions in `validate.rs` reference rules by `id` and emit findings
//! that inherit the rule's severity, section, title, and PDF page reference.
//! The check may add dynamic context (sizes, filenames, line numbers) to the
//! finding's message.

use crate::validate::Level;

/// Version of the Amazon Kindle Publishing Guidelines PDF these rules target.
pub const KPG_VERSION: &str = "2026.1";

/// URL where the KPG PDF is published.
#[allow(dead_code)]
pub const KPG_PDF_URL: &str =
    "https://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf";

/// A single validation rule from the Kindle Publishing Guidelines.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    /// Stable rule identifier, e.g. "R4.2.1". Used by check functions to
    /// reference this rule and by users to disable specific rules.
    pub id: &'static str,
    /// KPG section number as printed in the PDF, e.g. "4.2" or "10.3.1".
    pub section: &'static str,
    /// Severity when this rule fires.
    pub level: Level,
    /// One-line rule title.
    #[allow(dead_code)]
    pub title: &'static str,
    /// Page number in the KPG PDF where the rule is documented.
    pub pdf_page: u32,
    /// Full rule description. The check function may append dynamic context.
    pub description: &'static str,
}

/// Complete rule catalog. Order is not significant; checks look up rules by id.
pub const RULES: &[Rule] = &[
    // ---- Section 4: Cover Image Guidelines ----
    Rule {
        id: "R4.1.1",
        section: "4.1",
        level: Level::Info,
        title: "Marketing cover uploaded separately",
        pdf_page: 15,
        description: "Marketing cover image is uploaded separately to KDP and cannot be \
                      validated from the manuscript. Ensure you upload a 2560x1600 JPEG \
                      per Kindle Publishing Guidelines.",
    },
    Rule {
        id: "R4.2.1",
        section: "4.2",
        level: Level::Error,
        title: "Cover image required",
        pdf_page: 16,
        description: "No internal content cover image declared. Add either \
                      <item properties=\"coverimage\" ...> (Method 1, preferred) or \
                      <meta name=\"cover\" content=\"<id>\"/> (Method 2) to the OPF.",
    },
    Rule {
        id: "R4.2.2",
        section: "4.2",
        level: Level::Error,
        title: "Cover image file missing",
        pdf_page: 16,
        description: "Cover image declared in OPF but the file does not exist on disk.",
    },
    Rule {
        id: "R4.2.3",
        section: "4.2",
        level: Level::Warning,
        title: "Cover image too small",
        pdf_page: 15,
        description: "Cover image shortest side is below 500 px. Kindle will not display \
                      covers under 500 px on the shortest side.",
    },
    Rule {
        id: "R4.2.4",
        section: "4.2",
        level: Level::Error,
        title: "No HTML cover page in spine",
        pdf_page: 16,
        description: "Do not add an HTML cover page to the content in addition to the \
                      cover image. This may cause the cover to appear twice or fail \
                      conversion. Remove the HTML page from the spine.",
    },

    // ---- Section 5: Navigation Guidelines ----
    Rule {
        id: "R5.1",
        section: "5",
        level: Level::Warning,
        title: "TOC recommended for books over 20 pages",
        pdf_page: 17,
        description: "KPG strongly recommends a logical TOC for books longer than 20 pages.",
    },
    Rule {
        id: "R5.2.1",
        section: "5.2",
        level: Level::Warning,
        title: "NCX required",
        pdf_page: 19,
        description: "No NCX file found in manifest (media-type application/x-dtbncx+xml). \
                      Amazon requires a logical TOC via an NCX or a toc nav element for all \
                      Kindle books.",
    },
    Rule {
        id: "R5.2.2",
        section: "5.2",
        level: Level::Warning,
        title: "NCX must be referenced from spine",
        pdf_page: 19,
        description: "NCX is declared in manifest but the <spine> element has no \
                      toc=\"<id>\" attribute. KPG 5.2 requires referencing the NCX from \
                      the spine.",
    },

    // ---- Section 6: HTML and CSS Guidelines ----
    Rule {
        id: "R6.1",
        section: "6.1",
        level: Level::Warning,
        title: "Well-formed XHTML required",
        pdf_page: 22,
        description: "Content is not well-formed XHTML. Kindle requires well-formed HTML \
                      documents for reliable conversion.",
    },
    Rule {
        id: "R6.2",
        section: "6.2",
        level: Level::Warning,
        title: "Avoid negative CSS values",
        pdf_page: 23,
        description: "Negative CSS value for margin/padding/line-height. Positioning with \
                      negative values can cause content to be cut off at screen edges.",
    },
    Rule {
        id: "R6.3",
        section: "6.3",
        level: Level::Error,
        title: "No scripting",
        pdf_page: 23,
        description: "<script> tag found. Scripting is not supported; scripts will be \
                      stripped during conversion and any functionality relying on them \
                      will break.",
    },
    Rule {
        id: "R6.4",
        section: "6.4",
        level: Level::Error,
        title: "No nested <p> tags",
        pdf_page: 23,
        description: "Nested <p> tags found. Files with nested <p> tags do not convert \
                      properly.",
    },
    Rule {
        id: "R6.5",
        section: "6.5",
        level: Level::Error,
        title: "File reference case must match",
        pdf_page: 23,
        description: "File reference case does not match the actual filename on disk. \
                      Case-sensitive filesystems will fail to resolve the reference.",
    },

    // ---- Section 10: Text-Heavy Reflowable Books ----
    Rule {
        id: "R10.3.1",
        section: "10.3.1",
        level: Level::Warning,
        title: "Heading alignment should use default",
        pdf_page: 29,
        description: "Heading has an explicit text-align. KPG 10.3.1 recommends letting \
                      headings use the default alignment.",
    },
    Rule {
        id: "R10.4.1",
        section: "10.4.1",
        level: Level::Error,
        title: "Use supported image format",
        pdf_page: 38,
        description: "Image is not in a supported format (JPEG, PNG, GIF, SVG).",
    },
    Rule {
        id: "R10.4.2a",
        section: "10.4.2",
        level: Level::Warning,
        title: "Image file too large",
        pdf_page: 38,
        description: "Image file exceeds 127 KB. Large image files increase download size \
                      and may fail conversion.",
    },
    Rule {
        id: "R10.4.2b",
        section: "10.4.2",
        level: Level::Warning,
        title: "Image dimensions too large",
        pdf_page: 38,
        description: "Image exceeds 5 megapixels. Large images waste storage and may fail \
                      conversion.",
    },
    Rule {
        id: "R10.5.1",
        section: "10.5.1",
        level: Level::Warning,
        title: "Avoid large tables",
        pdf_page: 43,
        description: "Table has more than 50 rows. KPG 10.5.1 recommends keeping tables \
                      below 100 rows and 10 columns; large tables may render poorly.",
    },

    // ---- Section 17/18.1: Supported Tags ----
    Rule {
        id: "R17.1",
        section: "17",
        level: Level::Error,
        title: "Unsupported HTML tag",
        pdf_page: 22,
        description: "Unsupported HTML tag found. KPG 6.1 lists forms, frames, and \
                      JavaScript as unsupported; section 18.1 lists supported tags.",
    },
];

/// Look up a rule by its id. Panics in debug if the id is unknown (this is a
/// programming error: check functions should only reference rules defined in
/// this file).
pub fn get(id: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("unknown KDP rule id: {}", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_rule_ids_unique() {
        let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        let len_before = ids.len();
        ids.dedup();
        assert_eq!(
            len_before,
            ids.len(),
            "duplicate rule id(s) in RULES catalog"
        );
    }

    #[test]
    fn test_get_rule_by_id() {
        let rule = get("R4.2.1");
        assert_eq!(rule.section, "4.2");
        assert_eq!(rule.level, Level::Error);
    }

    #[test]
    #[should_panic(expected = "unknown KDP rule id")]
    fn test_get_unknown_rule_panics() {
        get("R999");
    }

    #[test]
    fn test_kpg_version_set() {
        assert!(!KPG_VERSION.is_empty());
    }
}
