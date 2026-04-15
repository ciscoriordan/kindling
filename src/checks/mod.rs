// Validator check trait and static registry.

use crate::extracted::ExtractedEpub;
use crate::validate::ValidationReport;

pub mod content;
pub mod cover;
pub mod dict;
pub mod file_case;
pub mod helpers;
pub mod images;
pub mod nav_links;
pub mod navigation;
pub mod parse_encoding;
pub mod fixed_layout;
pub mod manifest_spine;
pub mod opf_grammar;
// PHASE2-MOD: E
pub mod cross_refs;
// PHASE2-MOD: G
// PHASE2-MOD: H
// PHASE2-MOD: I
// PHASE2-MOD: K

/// A single-purpose validator module. Each impl owns one or more rule ids.
pub trait Check: Sync {
    /// Rule ids this check is responsible for.
    #[allow(dead_code)]
    fn ids(&self) -> &'static [&'static str];
    /// Run this check and push findings into `report`.
    fn run(&self, epub: &ExtractedEpub, report: &mut ValidationReport);
}

/// Registry of every check invoked by `validate::validate`.
pub static CHECKS: &[&dyn Check] = &[
    &cover::CoverChecks,
    &navigation::NavigationChecks,
    &nav_links::NavLinksChecks,
    &content::ContentChecks,
    &images::ImageChecks,
    &file_case::FileCaseChecks,
    &parse_encoding::ParseEncodingChecks,
    &dict::DictChecks,
    &fixed_layout::FixedLayoutChecks,
    &manifest_spine::ManifestSpineChecks,
    &opf_grammar::OpfGrammarChecks,
    // PHASE2-CHECK: E
    &cross_refs::CrossRefsChecks,
    // PHASE2-CHECK: G
    // PHASE2-CHECK: H
    // PHASE2-CHECK: I
    // PHASE2-CHECK: K
];
