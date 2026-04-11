//! kindling: Kindle MOBI/AZW3 builder library.
//!
//! This crate provides EPUB-to-MOBI/AZW3 conversion, a comic pipeline,
//! KDP pre-flight validation, and post-build MOBI readback checks.
//! The same functionality is exposed through the `kindling-cli` binary,
//! which is a thin wrapper around these modules.

// Public API
pub mod comic;
pub mod epub;
pub mod kdp_rules;
pub mod mobi;
pub mod mobi_check;
pub mod opf;
pub mod repair;
pub mod validate;

// Internal implementation modules, visible inside the crate only.
pub(crate) mod cbr;
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

/// Run the KDP validator as a pre-flight step inside `do_build` and friends.
///
/// When `no_validate` is true, prints a skip notice and returns `Ok(())`.
/// Otherwise runs `validate_opf`, prints each finding, and prints the summary
/// line. Returns `Err(error_count)` if the report contains any errors (caller
/// should abort the build); `Ok(())` otherwise. Warnings never abort but a
/// "validation passed with N warnings" notice is printed.
///
/// Unlike `do_validate` in the CLI binary, this function does NOT call
/// `process::exit` on error so the caller can clean up temp directories before
/// aborting.
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

    let report = match validate::validate_opf(opf_path) {
        Ok(r) => r,
        Err(e) => {
            // Can't parse the OPF at all: print a warning and continue; the
            // build itself will produce a clearer error below.
            eprintln!(
                "Warning: could not parse OPF for pre-flight validation ({}): {}",
                opf_path.display(),
                e
            );
            return Ok(());
        }
    };

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
