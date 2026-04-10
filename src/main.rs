/// kindling - Kindle MOBI builder for dictionaries and books
///
/// Usage:
///     kindling build input.opf -o output.mobi
///     kindling build input.epub -o output.mobi
///
/// Kindlegen-compatible usage:
///     kindling input.epub
///     kindling input.opf -o output.mobi -dont_append_source -verbose

mod comic;
mod epub;
mod exth;
mod html_check;
mod indx;
mod kdp_rules;
mod kf8;
mod mobi;
mod moire;
mod opf;
mod palmdoc;
#[cfg(test)]
mod tests;
mod validate;
mod vwi;

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kindling", about = "Kindle MOBI builder for dictionaries and books", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build MOBI file from OPF or EPUB
    #[command(version)]
    Build {
        /// Input OPF or EPUB file
        input: PathBuf,

        /// Output MOBI file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Skip PalmDOC compression (faster builds, larger files)
        #[arg(long)]
        no_compress: bool,

        /// Only index headwords (no inflected forms in orth index)
        #[arg(long)]
        headwords_only: bool,

        /// Skip embedding the EPUB source in the MOBI (saves space, breaks Kindle Previewer)
        #[arg(long)]
        no_embed_source: bool,

        /// Include a CMET (compilation metadata) record
        #[arg(long)]
        include_cmet: bool,

        /// Disable HD image container (CONT/CRES) for book MOBIs
        #[arg(long)]
        no_hd_images: bool,

        /// Identify as kindling in EXTH metadata instead of kindlegen
        #[arg(long)]
        creator_tag: bool,

        /// Output KF8-only format (.azw3) instead of dual MOBI7+KF8 (.mobi).
        /// KF8-only files are smaller and handled better by Calibre.
        /// Dual format remains available for maximum compatibility with older Kindles.
        #[arg(long)]
        kf8_only: bool,

        /// Enforce Kindle publishing limits: split HTML chunks >30MB at entry/paragraph
        /// boundaries, warn if >300 HTML files. ON by default for dictionaries, OFF for books.
        /// Use --no-kindle-limits to disable for dictionaries, --kindle-limits to enable for books.
        #[arg(long, overrides_with = "no_kindle_limits")]
        kindle_limits: bool,

        /// Disable Kindle publishing limits enforcement (see --kindle-limits)
        #[arg(long, overrides_with = "kindle_limits")]
        no_kindle_limits: bool,

        /// Skip the automatic Kindle Publishing Guidelines pre-flight check.
        /// Validation runs by default before every build; use this flag to
        /// bypass it (e.g. when a known-benign finding would otherwise abort).
        #[arg(long)]
        no_validate: bool,
    },

    /// Convert comic images/CBZ/CBR/EPUB to Kindle-optimized MOBI
    #[command(version)]
    Comic {
        /// Input image folder, CBZ file, CBR file, or EPUB file
        input: PathBuf,

        /// Output MOBI file
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Target Kindle device profile
        #[arg(short, long, default_value = "paperwhite")]
        device: String,

        /// Right-to-left reading mode (manga). Reverses page order and split order.
        #[arg(long)]
        rtl: bool,

        /// Disable double-page spread detection and splitting
        #[arg(long)]
        no_split: bool,

        /// Rotate double-page spreads 90 degrees clockwise instead of splitting.
        /// Gives a full-page spread view, useful for tablet users.
        #[arg(long)]
        rotate_spreads: bool,

        /// Disable automatic border/margin cropping
        #[arg(long)]
        no_crop: bool,

        /// Disable auto-contrast and gamma correction
        #[arg(long)]
        no_enhance: bool,

        /// Force webtoon mode (vertical strip merge + gutter-aware split)
        #[arg(long)]
        webtoon: bool,

        /// Disable Kindle Panel View (tap-to-zoom panels). Panel View is ON by default.
        #[arg(long)]
        no_panel_view: bool,

        /// Panel View reading order: controls which panel is shown first when
        /// tapping to zoom. Options: horizontal-lr (left-to-right, default for
        /// Western comics), horizontal-rl (right-to-left, default for manga
        /// when --rtl is set), vertical-lr (top-to-bottom then left-to-right,
        /// for 4-koma), vertical-rl (top-to-bottom then right-to-left).
        /// If omitted, auto-detected from --rtl flag.
        #[arg(long)]
        panel_reading_order: Option<String>,

        /// JPEG encoding quality (1-100). Lower values produce smaller files.
        /// Some Kindle devices may show blank pages with very high quality JPEGs,
        /// so 70-80 can be a workaround.
        #[arg(long, default_value = "85", value_parser = clap::value_parser!(u8).range(1..=100))]
        jpeg_quality: u8,

        /// Maximum pixel height for merged webtoon strips. If the merged strip
        /// exceeds this, it is split into chunks processed independently.
        /// Prevents OOM on large webtoon directories.
        #[arg(long, default_value = "65536")]
        max_height: u32,

        /// Skip embedding the EPUB source in the MOBI (saves space, breaks Kindle Previewer)
        #[arg(long)]
        no_embed_source: bool,

        /// Document type: "ebok" (Books shelf) or "pdoc" (Documents shelf, default).
        /// WARNING: Amazon may auto-delete sideloaded EBOK files when Kindle connects to WiFi.
        #[arg(long, default_value = "pdoc")]
        doc_type: String,

        /// Override the title from ComicInfo.xml
        #[arg(long)]
        title: Option<String>,

        /// Override the author from ComicInfo.xml
        #[arg(long)]
        author: Option<String>,

        /// Language code for OPF metadata (e.g. "ja", "en", "ko").
        /// Important for CJK content where language affects font selection on Kindle.
        #[arg(long)]
        language: Option<String>,

        /// Cover image: a page number (1-based) or a file path.
        /// When provided, use that image as the cover instead of the first page.
        #[arg(long)]
        cover: Option<String>,

        /// Center-crop the cover image to fill the device screen exactly.
        /// Removes letterbox borders by cropping to the device's aspect ratio.
        #[arg(long)]
        cover_fill: bool,

        /// Output KF8-only format (.azw3) instead of dual MOBI7+KF8 (.mobi).
        /// KF8-only files are smaller and handled better by Calibre.
        /// Dual format remains available for maximum compatibility with older Kindles.
        #[arg(long)]
        kf8_only: bool,

        /// Enforce Kindle publishing limits: warn if >300 HTML files.
        /// OFF by default for comics. Use --kindle-limits to enable.
        #[arg(long, overrides_with = "no_kindle_limits")]
        kindle_limits: bool,

        /// Disable Kindle publishing limits enforcement (see --kindle-limits)
        #[arg(long, overrides_with = "kindle_limits")]
        no_kindle_limits: bool,
    },

    /// Validate an OPF manuscript against the Amazon Kindle Publishing Guidelines (2026.1).
    ///
    /// Runs a set of pre-flight checks (cover image, NCX, HTML/CSS hygiene,
    /// image formats/sizes, table size, unsupported tags) and prints one line
    /// per finding with severity, KPG rule id, section, PDF page, and message.
    /// Exits 0 if there are no errors, 1 otherwise. With `--strict`, exits 1
    /// on any warning too.
    #[command(version)]
    Validate {
        /// Input OPF file
        input: PathBuf,

        /// Treat warnings as errors (exit non-zero on any warning).
        #[arg(long)]
        strict: bool,
    },
}

/// Check if the first argument looks like a file path (kindlegen compat mode)
/// rather than a subcommand like "build".
fn is_kindlegen_compat_mode() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        return false;
    }
    let first_arg = &args[1];
    // If first arg ends with .opf or .epub, treat as kindlegen compat mode
    let lower = first_arg.to_lowercase();
    lower.ends_with(".opf") || lower.ends_with(".epub")
}

/// Parse kindlegen-compatible arguments.
/// Accepts: kindling <input_file> [-o <filename>] [-dont_append_source] [-locale <value>]
///          [-c0] [-c1] [-c2] [-verbose] [-no_validate | --no-validate]
/// Returns (input, output_override, no_validate)
fn parse_kindlegen_args() -> (PathBuf, Option<String>, bool) {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(&args[1]);
    let mut output_name: Option<String> = None;
    let mut no_validate = false;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                if i + 1 < args.len() {
                    output_name = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "-locale" => {
                // Silently ignore -locale <value>
                i += 2;
            }
            "-dont_append_source" | "-c0" | "-c1" | "-c2" | "-verbose" => {
                // Silently ignore these flags
                i += 1;
            }
            "-no_validate" | "--no-validate" => {
                no_validate = true;
                i += 1;
            }
            _ => {
                // Unknown flag, skip
                i += 1;
            }
        }
    }
    (input, output_name, no_validate)
}

/// Resolve the output path for a build.
///
/// If an explicit output is given, use it. For kindlegen compat mode, the -o flag
/// specifies just a filename (output goes next to input). For the build subcommand,
/// -o is a full path. If no output is specified, replace the input extension with
/// .azw3 (KF8-only) or .mobi (dual format).
fn resolve_output_path(input: &PathBuf, output: Option<PathBuf>, kf8_only: bool) -> PathBuf {
    match output {
        Some(p) => p,
        None => {
            let ext = if kf8_only { "azw3" } else { "mobi" };
            input.with_extension(ext)
        }
    }
}

fn do_build(
    input: &PathBuf,
    output_path: &PathBuf,
    no_compress: bool,
    headwords_only: bool,
    embed_source: bool,
    include_cmet: bool,
    no_hd_images: bool,
    creator_tag: bool,
    kf8_only: bool,
    kindle_limits: bool,
    no_validate: bool,
) {
    let is_epub = input
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("epub"))
        .unwrap_or(false);

    // Read the EPUB bytes for SRCS embedding if requested and input is EPUB
    let srcs_data = if embed_source && is_epub {
        match std::fs::read(input) {
            Ok(data) => {
                eprintln!("SRCS: embedding {} bytes of EPUB source", data.len());
                Some(data)
            }
            Err(e) => {
                eprintln!("Warning: could not read EPUB for SRCS embedding: {}", e);
                None
            }
        }
    } else {
        if embed_source && !is_epub {
            eprintln!("Note: EPUB source embedding skipped for non-EPUB input");
        }
        None
    };

    let result = if is_epub {
        // Extract EPUB to temp dir, find OPF, build, clean up
        let (temp_dir, opf_path) = match epub::extract_epub(input) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("Error extracting EPUB: {}", e);
                println!("Error(prcgen):E24000: Could not process input file");
                process::exit(1);
            }
        };

        // Pre-flight KDP validation on the extracted OPF.
        if let Err(errors) = run_preflight_validation(&opf_path, no_validate) {
            epub::cleanup_temp_dir(&temp_dir);
            eprintln!(
                "Build aborted: {} validation errors. Run with --no-validate to skip.",
                errors
            );
            println!("Error(prcgen):E24000: Could not build Mobi file");
            process::exit(1);
        }

        let result = mobi::build_mobi(
            &opf_path, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None, kindle_limits,
        );
        epub::cleanup_temp_dir(&temp_dir);
        result
    } else {
        // Direct OPF input: run pre-flight validation first.
        if let Err(errors) = run_preflight_validation(input, no_validate) {
            eprintln!(
                "Build aborted: {} validation errors. Run with --no-validate to skip.",
                errors
            );
            println!("Error(prcgen):E24000: Could not build Mobi file");
            process::exit(1);
        }

        mobi::build_mobi(
            input, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None, kindle_limits,
        )
    };

    match result {
        Ok(()) => {
            println!("Info(prcgen):I1036: Mobi file built successfully");
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            // Check if this looks like a file-too-big error
            let err_str = format!("{}", e);
            if err_str.contains("too big") || err_str.contains("too large") {
                println!("Error(prcgen):E23026: File too big");
            } else {
                println!("Error(prcgen):E24000: Could not build Mobi file");
            }
            process::exit(1);
        }
    }
}

fn main() {
    if is_kindlegen_compat_mode() {
        // Kindlegen-compatible invocation: kindling <file> [-o name] [flags...]
        let (input, output_name, no_validate) = parse_kindlegen_args();

        // In kindlegen compat mode, -o specifies just a filename next to the input
        let output_path = if let Some(name) = output_name {
            let parent = input.parent().unwrap_or(std::path::Path::new("."));
            parent.join(name)
        } else {
            input.with_extension("mobi")
        };

        do_build(&input, &output_path, false, false, true, false, false, false, false, true, no_validate);
    } else {
        let cli = Cli::parse();

        match cli.command {
            Commands::Build {
                input,
                output,
                no_compress,
                headwords_only,
                no_embed_source,
                include_cmet,
                no_hd_images,
                creator_tag,
                kf8_only,
                kindle_limits,
                no_kindle_limits,
                no_validate,
            } => {
                // Default: ON for dictionaries, OFF for books.
                // Since we don't know the content type yet at parse time, we pass
                // a tri-state: if user explicitly set a flag, use that; otherwise
                // let build_mobi decide based on content type detection.
                // We use a simple heuristic: if neither flag is set, pass true
                // (the dictionary default, which is more conservative). The book
                // path will only warn, not restructure, so it's safe.
                let effective_kindle_limits = if no_kindle_limits {
                    false
                } else if kindle_limits {
                    true
                } else {
                    // Default: true (dictionary default, harmless for books since
                    // books only warn, not split)
                    true
                };
                let output_path = resolve_output_path(&input, output, kf8_only);
                do_build(&input, &output_path, no_compress, headwords_only, !no_embed_source, include_cmet, no_hd_images, creator_tag, kf8_only, effective_kindle_limits, no_validate);
            }
            Commands::Comic {
                input,
                output,
                device,
                rtl,
                no_split,
                rotate_spreads,
                no_crop,
                no_enhance,
                webtoon,
                no_panel_view,
                jpeg_quality,
                max_height,
                no_embed_source,
                doc_type,
                title,
                author,
                language,
                cover,
                cover_fill,
                panel_reading_order,
                kf8_only,
                kindle_limits,
                no_kindle_limits,
            } => {
                let profile = match comic::get_profile(&device) {
                    Some(p) => p,
                    None => {
                        eprintln!("Error: unknown device '{}'. Valid devices: {}", device, comic::valid_device_names());
                        process::exit(1);
                    }
                };

                let output_path = match output {
                    Some(p) => p,
                    None => {
                        let ext = if kf8_only { "azw3" } else { "mobi" };
                        input.with_extension(ext)
                    }
                };

                // Parse doc_type flag
                let doc_type_value = match doc_type.to_lowercase().as_str() {
                    "ebok" => Some("EBOK".to_string()),
                    "pdoc" => None, // None means default PDOC
                    other => {
                        eprintln!("Warning: unknown --doc-type '{}', using default 'pdoc'", other);
                        None
                    }
                };

                // Parse cover flag: either a page number or a file path
                let cover_source = cover.map(|c| {
                    if let Ok(page_num) = c.parse::<usize>() {
                        if page_num >= 1 {
                            comic::CoverSource::PageNumber(page_num)
                        } else {
                            eprintln!("Warning: cover page number must be >= 1, treating as file path");
                            comic::CoverSource::FilePath(PathBuf::from(c))
                        }
                    } else {
                        comic::CoverSource::FilePath(PathBuf::from(c))
                    }
                });

                // Comic defaults to OFF for kindle_limits
                let effective_kindle_limits = kindle_limits && !no_kindle_limits;

                let options = comic::ComicOptions {
                    rtl,
                    split: !no_split,
                    crop: !no_crop,
                    enhance: !no_enhance,
                    webtoon,
                    panel_view: !no_panel_view,
                    jpeg_quality,
                    max_height,
                    embed_source: !no_embed_source,
                    doc_type: doc_type_value,
                    title_override: title,
                    author_override: author,
                    language,
                    cover: cover_source,
                    rotate_spreads,
                    panel_reading_order,
                    cover_fill,
                    kindle_limits: effective_kindle_limits,
                    kf8_only,
                };

                match comic::build_comic_with_options(&input, &output_path, &profile, &options) {
                    Ok(()) => {
                        let format_name = if kf8_only { "AZW3" } else { "MOBI" };
                        eprintln!("Comic {} built successfully: {}", format_name, output_path.display());
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                }
            }
            Commands::Validate { input, strict } => {
                do_validate(&input, strict);
            }
        }
    }
}

/// Run the validator on an OPF and print the report.
///
/// Exits 0 if there are no errors (and no warnings when `strict` is set),
/// 1 otherwise.
fn do_validate(opf_path: &PathBuf, strict: bool) {
    println!(
        "Validating {} against Kindle Publishing Guidelines v{}",
        opf_path.display(),
        kdp_rules::KPG_VERSION
    );

    let report = match validate::validate_opf(opf_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: could not parse OPF {}: {}", opf_path.display(), e);
            process::exit(2);
        }
    };

    for finding in &report.findings {
        println!("{}", finding);
    }

    let errors = report.error_count();
    let warnings = report.warning_count();
    let infos = report.info_count();
    println!("{} errors, {} warnings, {} info", errors, warnings, infos);

    let fail = errors > 0 || (strict && warnings > 0);
    if fail {
        process::exit(1);
    }
}

/// Run the KDP validator as a pre-flight step inside `do_build` and friends.
///
/// When `no_validate` is true, prints a skip notice and returns `Ok(())`.
/// Otherwise runs `validate_opf`, prints each finding, and prints the summary
/// line. Returns `Err(error_count)` if the report contains any errors (caller
/// should abort the build); `Ok(())` otherwise. Warnings never abort but a
/// "validation passed with N warnings" notice is printed.
///
/// Unlike `do_validate`, this function does NOT call `process::exit` on error
/// so the caller can clean up temp directories before aborting.
fn run_preflight_validation(opf_path: &Path, no_validate: bool) -> Result<(), usize> {
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
