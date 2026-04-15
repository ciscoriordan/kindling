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
];
