//! kindling: Kindle MOBI/AZW3 builder library.
//!
//! This crate provides EPUB-to-MOBI/AZW3 conversion, a comic pipeline,
//! KDP pre-flight validation, and post-build MOBI readback checks.
//! The same functionality is exposed through the `kindling-cli` binary,
//! which is a thin wrapper around these modules.

// Public API
pub mod comic;
pub mod epub;
pub mod extracted;
pub mod kdp_rules;
pub mod mobi;
pub mod mobi_check;
pub mod mobi_dump;
pub mod mobi_rewrite;
pub mod opf;
pub mod profile;
pub mod repair;
pub mod validate;

// Internal implementation modules, visible inside the crate only.
pub(crate) mod cbr;
pub(crate) mod checks;
pub(crate) mod cncx;
pub(crate) mod exth;
pub(crate) mod html_check;
pub(crate) mod indx;
pub(crate) mod kf8;
pub(crate) mod moire;
pub(crate) mod palmdoc;
pub(crate) mod vwi;

#[cfg(test)]
mod tests;

use std::path::Path;

use crate::extracted::ExtractedEpub;

/// Pre-flight KDP validation used by `do_build`. Returns `Err(error_count)`.
pub fn run_preflight_validation(opf_path: &Path, no_validate: bool) -> Result<(), usize> {
    if no_validate {
        println!("Skipping KDP validation (--no-validate)");
        return Ok(());
    }

    println!(
        "Validating {} against Kindle Publishing Guidelines v{}",
        opf_path.display(),
        kdp_rules::KPG_VERSION
    );

    let epub = match ExtractedEpub::from_opf_path(opf_path) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "Warning: could not parse OPF for pre-flight validation ({}): {}",
                opf_path.display(),
                e
            );
            return Ok(());
        }
    };
    let report = validate::validate(&epub);

    for finding in &report.findings {
        println!("{}", finding);
    }

    let errors = report.error_count();
    let warnings = report.warning_count();
    let infos = report.info_count();
    println!("{} errors, {} warnings, {} info", errors, warnings, infos);

    if errors > 0 {
        return Err(errors);
    }
    if warnings > 0 {
        println!("Validation passed with {} warnings", warnings);
    }
    Ok(())
}
