// Section 13: OCF filename rules for manifest item hrefs (R13.1-R13.5).
//
// These mirror the epubcheck PKG_009 through PKG_012 and OPF_060 messages. They
// catch filename hazards that break on Windows, fail to resolve on
// case-insensitive filesystems, or trip OCF path validation:
//   R13.1 PKG_009 OCF-illegal characters in href (< > : " | ? * control chars)
//   R13.2 PKG_010 space in href
//   R13.3 PKG_011 trailing dot in href
//   R13.4 PKG_012 non-ASCII character in href
//   R13.5 OPF_060 case-insensitive duplicate hrefs

use std::collections::HashMap;
use std::path::PathBuf;

use super::Check;
use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub struct FilenameChecks;

impl Check for FilenameChecks {
    fn ids(&self) -> &'static [&'static str] {
        &["R13.1", "R13.2", "R13.3", "R13.4", "R13.5"]
    }

    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport) {
        let opf = &epub.opf;

        // Per-item character checks for R13.1 through R13.4.
        for (_id, (href, _media_type)) in &opf.manifest {
            let file = Some(PathBuf::from(href));

            if let Some(bad) = ocf_illegal_char(href) {
                report.emit_at(
                    "R13.1",
                    format!("Illegal character {} in href '{}'.", describe_char(bad), href),
                    file.clone(),
                    None,
                );
            }

            if has_space(href) {
                report.emit_at(
                    "R13.2",
                    format!("Space in href '{}'.", href),
                    file.clone(),
                    None,
                );
            }

            if ends_with_dot(href) {
                report.emit_at(
                    "R13.3",
                    format!("Href '{}' ends with '.'.", href),
                    file.clone(),
                    None,
                );
            }

            if has_non_ascii(href) {
                report.emit_at(
                    "R13.4",
                    format!("Non-ASCII character in href '{}'.", href),
                    file.clone(),
                    None,
                );
            }
        }

        // Pairwise R13.5: case-fold manifest hrefs and report any collision.
        let mut seen: HashMap<String, String> = HashMap::new();
        let mut reported_pairs: Vec<(String, String)> = Vec::new();
        for (_id, (href, _media_type)) in &opf.manifest {
            let folded = href.to_lowercase();
            if let Some(prev) = seen.get(&folded) {
                if prev == href {
                    continue;
                }
                let pair = if prev <= href {
                    (prev.clone(), href.clone())
                } else {
                    (href.clone(), prev.clone())
                };
                if !reported_pairs.contains(&pair) {
                    report.emit_at(
                        "R13.5",
                        format!(
                            "Manifest hrefs '{}' and '{}' are equal after case-folding.",
                            pair.0, pair.1
                        ),
                        Some(PathBuf::from(&pair.1)),
                        None,
                    );
                    reported_pairs.push(pair);
                }
            } else {
                seen.insert(folded, href.clone());
            }
        }
    }
}

/// Return the first character in `href` that is not allowed in OCF filenames.
fn ocf_illegal_char(href: &str) -> Option<char> {
    href.chars().find(|c| is_ocf_illegal(*c))
}

/// True if `c` is one of the OCF-illegal characters or a sub-U+0020 control.
fn is_ocf_illegal(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (c as u32) < 0x20
}

/// True if `href` contains a U+0020 space.
fn has_space(href: &str) -> bool {
    href.contains(' ')
}

/// True if the last character of `href` is a literal `.`.
fn ends_with_dot(href: &str) -> bool {
    href.ends_with('.')
}

/// True if any character in `href` is above U+007E.
fn has_non_ascii(href: &str) -> bool {
    href.chars().any(|c| (c as u32) > 0x7E)
}

/// Human-readable label for a character that appears in a finding message.
fn describe_char(c: char) -> String {
    if (c as u32) < 0x20 {
        format!("U+{:04X}", c as u32)
    } else {
        format!("'{}'", c)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- R13.1 OCF-illegal characters ----

    #[test]
    fn r13_1_less_than_fires() {
        assert_eq!(ocf_illegal_char("foo<bar.html"), Some('<'));
    }

    #[test]
    fn r13_1_asterisk_fires() {
        assert_eq!(ocf_illegal_char("foo*bar.html"), Some('*'));
    }

    #[test]
    fn r13_1_colon_fires() {
        assert_eq!(ocf_illegal_char("OEBPS/chapter:1.html"), Some(':'));
    }

    #[test]
    fn r13_1_control_char_fires() {
        let bad = format!("foo{}bar.html", '\u{0007}');
        assert_eq!(ocf_illegal_char(&bad), Some('\u{0007}'));
    }

    #[test]
    fn r13_1_plain_ascii_clean() {
        assert!(ocf_illegal_char("OEBPS/content.html").is_none());
    }

    #[test]
    fn r13_1_slash_clean() {
        // Forward slash is a path separator, not an OCF-illegal character.
        assert!(ocf_illegal_char("OEBPS/sub/content.html").is_none());
    }

    // ---- R13.2 Space ----

    #[test]
    fn r13_2_space_fires() {
        assert!(has_space("chapter 1.html"));
    }

    #[test]
    fn r13_2_no_space_clean() {
        assert!(!has_space("chapter_1.html"));
    }

    // ---- R13.3 Trailing dot ----

    #[test]
    fn r13_3_trailing_dot_fires() {
        assert!(ends_with_dot("chapter."));
    }

    #[test]
    fn r13_3_no_trailing_dot_clean() {
        assert!(!ends_with_dot("chapter.html"));
    }

    #[test]
    fn r13_3_dot_in_middle_clean() {
        assert!(!ends_with_dot("chapter.1.html"));
    }

    // ---- R13.4 Non-ASCII ----

    #[test]
    fn r13_4_greek_letter_fires() {
        assert!(has_non_ascii("κεφάλαιο.html"));
    }

    #[test]
    fn r13_4_em_dash_fires() {
        let href = format!("chapter{}one.html", '\u{2014}');
        assert!(has_non_ascii(&href));
    }

    #[test]
    fn r13_4_pure_ascii_clean() {
        assert!(!has_non_ascii("chapter_one.html"));
    }

    #[test]
    fn r13_4_tilde_boundary_clean() {
        // U+007E tilde is the top of the ASCII printable range and must not fire.
        assert!(!has_non_ascii("chapter~one.html"));
    }

    // ---- R13.5 Case-fold collision ----

    #[test]
    fn r13_5_case_fold_collides() {
        let a = "Foo.html".to_lowercase();
        let b = "foo.html".to_lowercase();
        assert_eq!(a, b);
    }

    #[test]
    fn r13_5_distinct_names_clean() {
        let a = "foo.html".to_lowercase();
        let b = "bar.html".to_lowercase();
        assert_ne!(a, b);
    }

    // ---- describe_char helper ----

    #[test]
    fn describe_char_control_uses_codepoint() {
        assert_eq!(describe_char('\u{0007}'), "U+0007");
    }

    #[test]
    fn describe_char_printable_quoted() {
        assert_eq!(describe_char('<'), "'<'");
    }
}
