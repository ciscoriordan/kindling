/// kindling - Kindle MOBI builder for dictionaries and books
///
/// Usage:
///     kindling build input.opf -o output.mobi
///     kindling build input.epub -o output.mobi
///
/// Kindlegen-compatible usage:
///     kindling input.epub
///     kindling input.opf -o output.mobi -dont_append_source -verbose

use kindling::{
    comic, epub, kdp_rules, mobi, mobi_check, mobi_dump, mobi_rewrite, opf, repair, validate,
};

use std::path::PathBuf;
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

        /// Build legacy dual-format MOBI7+KF8 (.mobi) instead of the modern
        /// KF8-only (.azw3) default. Only useful for pre-2012 Kindles; modern
        /// devices prefer KF8-only output. Dictionaries always use legacy
        /// MOBI7 because Kindle's lookup popup requires the MOBI7 INDX format,
        /// so this flag is a no-op on dictionary builds.
        #[arg(long)]
        legacy_mobi: bool,

        /// Deprecated no-op. KF8-only (.azw3) is now the default for
        /// non-dictionary builds; pass `--legacy-mobi` to opt back into the
        /// old dual MOBI7+KF8 behavior. Kept so existing scripts that pass
        /// `--kf8-only` keep working.
        #[arg(long, hide = true)]
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

        /// Skip the build-time HTML self-check on the MOBI text blob.
        /// The self-check runs by default and prints a warning (without
        /// aborting the build) if it detects malformed HTML, unbalanced
        /// tags, or `<hr/` corruption. Overhead is typically 50-200 ms.
        #[arg(long)]
        no_self_check: bool,
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

        /// Crop mode: 0=disabled, 1=margins only, 2=margins+page numbers (default)
        #[arg(long, default_value = "2", value_parser = clap::value_parser!(u8).range(0..=2))]
        crop: u8,

        /// Deprecated: equivalent to --crop 0
        #[arg(long, hide = true)]
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

        /// Embed the intermediate EPUB source as a SRCS record inside the
        /// MOBI. Off by default for comics: the SRCS record contains a zipped
        /// copy of every image in the comic, which for a large book produces
        /// a single PalmDB record well over 100 MB. That oversize record is
        /// what triggered the "Unable to Open Item" failure on Vader Down,
        /// even though the Kindle indexer accepted the file. Only enable
        /// this when you need Kindle Previewer to round-trip to EPUB.
        #[arg(long)]
        embed_source: bool,

        /// Deprecated no-op. Comics no longer embed the EPUB source by
        /// default; pass `--embed-source` to opt back in. Kept so existing
        /// scripts that pass `--no-embed-source` keep working.
        #[arg(long, hide = true)]
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

        /// Build legacy dual-format MOBI7+KF8 (.mobi) instead of the modern
        /// KF8-only (.azw3) default. Only useful for pre-2012 Kindles; modern
        /// devices prefer KF8-only output.
        #[arg(long)]
        legacy_mobi: bool,

        /// Deprecated no-op. KF8-only (.azw3) is now the default for comics;
        /// pass `--legacy-mobi` to opt back into the old dual MOBI7+KF8
        /// behavior. Kept so existing scripts that pass `--kf8-only` keep
        /// working.
        #[arg(long, hide = true)]
        kf8_only: bool,

        /// Enforce Kindle publishing limits: warn if >300 HTML files.
        /// OFF by default for comics. Use --kindle-limits to enable.
        #[arg(long, overrides_with = "no_kindle_limits")]
        kindle_limits: bool,

        /// Disable Kindle publishing limits enforcement (see --kindle-limits)
        #[arg(long, overrides_with = "kindle_limits")]
        no_kindle_limits: bool,

        /// Skip the build-time HTML self-check on the comic's MOBI text
        /// blob. See `kindling build --help` for details.
        #[arg(long)]
        no_self_check: bool,

        /// Emit KF8 comic output that matches kindlegen's byte-level
        /// shape. Off by default (kindling emits its normal pretty-
        /// printed "better than kindlegen" form). Turn this on for
        /// parity tests or when producing reference builds — byte
        /// differences between kindling and kindlegen shrink to the
        /// unavoidable ones (compression seeds, timestamps, UIDs).
        #[arg(long)]
        kindlegen_parity: bool,
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

    /// Repair an EPUB file for cleaner Kindle ingest.
    ///
    /// Applies the fix list from innocenat/kindle-epub-fix (public domain):
    /// prepend missing XML declarations, rewrite body-id hyperlinks that
    /// Kindle drops, inject a fallback dc:language, and delete stray <img>
    /// tags with no src. Byte-stable on clean input, idempotent on broken
    /// input, rejects DRM-protected files.
    #[command(version)]
    Repair {
        /// Input EPUB file
        input: PathBuf,

        /// Output EPUB file. Defaults to `<stem>-fixed.epub` next to the input.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Emit the full RepairReport as JSON on stdout.
        #[arg(long)]
        report_json: bool,

        /// Detect fixes without writing an output file.
        #[arg(long)]
        dry_run: bool,
    },

    /// Rewrite MOBI/AZW3 metadata in place without rebuilding from source.
    ///
    /// Takes an existing MOBI or AZW3 file and replaces the EXTH metadata
    /// records (title, authors, publisher, description, language, ISBN,
    /// ASIN, publication date, tags, series, cover image) according to
    /// the provided flags. Book content records (text, non-cover images,
    /// indices) are never touched. Byte-stable on no-op, idempotent, and
    /// refuses DRM-encrypted files.
    #[command(version)]
    RewriteMetadata {
        /// Input MOBI or AZW3 file.
        input: PathBuf,

        /// Output MOBI/AZW3 file. Defaults to `<stem>-meta.<ext>` next to
        /// the input.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Emit the full RewriteReport as JSON on stdout.
        #[arg(long)]
        report_json: bool,

        /// Scan and plan changes without writing an output file.
        #[arg(long)]
        dry_run: bool,

        /// New title (EXTH 503 plus full_name).
        #[arg(long)]
        title: Option<String>,

        /// New author. Pass multiple times for multiple creators.
        #[arg(long = "author", action = clap::ArgAction::Append)]
        authors: Vec<String>,

        /// New publisher (EXTH 101).
        #[arg(long)]
        publisher: Option<String>,

        /// New description (EXTH 103).
        #[arg(long)]
        description: Option<String>,

        /// New primary writing language, BCP47 code (EXTH 524).
        #[arg(long)]
        language: Option<String>,

        /// New ISBN (EXTH 104).
        #[arg(long)]
        isbn: Option<String>,

        /// New ASIN (EXTH 504).
        #[arg(long)]
        asin: Option<String>,

        /// New publication date (EXTH 106).
        #[arg(long = "publication-date")]
        publication_date: Option<String>,

        /// New subject/tag. Pass multiple times for multiple tags.
        #[arg(long = "subject", action = clap::ArgAction::Append)]
        subjects: Vec<String>,

        /// New series name (EXTH 112).
        #[arg(long)]
        series: Option<String>,

        /// New series index (EXTH 113).
        #[arg(long = "series-index")]
        series_index: Option<String>,

        /// Path to a new cover image (JPEG, PNG, or GIF). Replaces the
        /// existing cover image record in place. The input file must
        /// already have a cover.
        #[arg(long)]
        cover: Option<PathBuf>,
    },

    /// Dump the structural contents of a MOBI/AZW3 file to stdout.
    ///
    /// Emits one line per parsed field in `section.field = value` form so
    /// `diff -u` between two dumps surfaces semantic differences (EXTH
    /// records, MOBI header fields, INDX / ORDT2 tables, entry labels)
    /// without drowning in absolute-offset cascades. Text and image
    /// record contents are summarized (length + magic only) to keep the
    /// diff focused on structure.
    #[command(version)]
    Dump {
        /// Input MOBI or AZW3 file.
        input: PathBuf,
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
///          [-no_self_check | --no-self-check]
/// Returns (input, output_override, no_validate, no_self_check)
fn parse_kindlegen_args() -> (PathBuf, Option<String>, bool, bool) {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(&args[1]);
    let mut output_name: Option<String> = None;
    let mut no_validate = false;
    let mut no_self_check = false;
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
            "--no-self-check" | "-no_self_check" => {
                no_self_check = true;
                i += 1;
            }
            _ => {
                // Unknown flag, skip
                i += 1;
            }
        }
    }
    (input, output_name, no_validate, no_self_check)
}

/// Resolve the output path for a build.
///
/// If an explicit output is given, use it verbatim (the user's extension
/// choice always wins, even if it does not match the actual container
/// format). If no output is specified, derive a default filename by
/// replacing the input extension with `.azw3` for KF8-only builds or
/// `.mobi` for dual-format (legacy MOBI7+KF8 and dictionary) builds.
fn resolve_output_path(input: &PathBuf, output: Option<PathBuf>, kf8_only: bool) -> PathBuf {
    match output {
        Some(p) => p,
        None => {
            let ext = if kf8_only { "azw3" } else { "mobi" };
            input.with_extension(ext)
        }
    }
}

/// Best-effort detection of whether the build input describes a dictionary.
///
/// For `.opf` inputs this parses the OPF directly and consults
/// `OPFData::is_dictionary()`. For `.epub` inputs this extracts the archive
/// into a temporary directory, parses the inner OPF, and cleans up. Any
/// error (parse failure, missing metadata) is treated as "not a dictionary",
/// which is the safer default since the worst case is a non-dictionary book
/// accidentally built as dual-format MOBI7+KF8 instead of KF8-only.
fn detect_is_dictionary(input: &std::path::Path) -> bool {
    let is_epub = input
        .extension()
        .map(|ext| ext.eq_ignore_ascii_case("epub"))
        .unwrap_or(false);

    if is_epub {
        match epub::extract_epub(input) {
            Ok((temp_dir, opf_path)) => {
                let result = opf::OPFData::parse(&opf_path)
                    .map(|data| data.is_dictionary())
                    .unwrap_or(false);
                epub::cleanup_temp_dir(&temp_dir);
                result
            }
            Err(_) => false,
        }
    } else {
        opf::OPFData::parse(input)
            .map(|data| data.is_dictionary())
            .unwrap_or(false)
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
    self_check: bool,
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

    // Capture the OPF so we can round-trip its title/author through the
    // post-build MOBI readback check.
    let mut opf_snapshot: Option<(String, String, bool)> = None;

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
        if let Err(errors) = kindling::run_preflight_validation(&opf_path, no_validate) {
            epub::cleanup_temp_dir(&temp_dir);
            eprintln!(
                "Build aborted: {} validation errors. Run with --no-validate to skip.",
                errors
            );
            println!("Error(prcgen):E24000: Could not build Mobi file");
            process::exit(1);
        }

        if let Ok(parsed) = opf::OPFData::parse(&opf_path) {
            opf_snapshot = Some((parsed.title.clone(), parsed.author.clone(), parsed.is_dictionary()));
        }

        let result = mobi::build_mobi(
            &opf_path, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None, kindle_limits, self_check,
            false, // kindlegen_parity: only meaningful for the comic path
        );
        epub::cleanup_temp_dir(&temp_dir);
        result
    } else {
        // Direct OPF input: run pre-flight validation first.
        if let Err(errors) = kindling::run_preflight_validation(input, no_validate) {
            eprintln!(
                "Build aborted: {} validation errors. Run with --no-validate to skip.",
                errors
            );
            println!("Error(prcgen):E24000: Could not build Mobi file");
            process::exit(1);
        }

        if let Ok(parsed) = opf::OPFData::parse(input) {
            opf_snapshot = Some((parsed.title.clone(), parsed.author.clone(), parsed.is_dictionary()));
        }

        mobi::build_mobi(
            input, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None, kindle_limits, self_check,
            false, // kindlegen_parity: only meaningful for the comic path
        )
    };

    match result {
        Ok(()) => {
            // Post-build MOBI readback check. This is the only thing between
            // a broken library entry and a happy user, so don't skip it by
            // default.
            let (title, author, is_dictionary) = opf_snapshot
                .as_ref()
                .map(|(t, a, d)| (t.as_str(), a.as_str(), *d))
                .unwrap_or(("", "", false));
            let expected = mobi_check::ExpectedMetadata {
                title: if title.is_empty() { None } else { Some(title) },
                author: if author.is_empty() { None } else { Some(author) },
                is_comic: false,
                is_dictionary,
            };
            match mobi_check::check_mobi_file(output_path, &expected) {
                Ok(report) => {
                    if let Err(e) = mobi_check::report_result(output_path, &report) {
                        eprintln!("Error: {}", e);
                        println!("Error(prcgen):E24000: Could not build Mobi file");
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("Warning: MOBI post-build check could not run: {}", e);
                }
            }
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
        let (input, output_name, no_validate, no_self_check) = parse_kindlegen_args();

        // In kindlegen compat mode, -o specifies just a filename next to the input
        let output_path = if let Some(name) = output_name {
            let parent = input.parent().unwrap_or(std::path::Path::new("."));
            parent.join(name)
        } else {
            input.with_extension("mobi")
        };

        do_build(&input, &output_path, false, false, true, false, false, false, false, true, no_validate, !no_self_check);
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
                legacy_mobi,
                kf8_only,
                kindle_limits,
                no_kindle_limits,
                no_validate,
                no_self_check,
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

                // Deprecated: `--kf8-only` is now the default for non-dict
                // builds and a no-op for dicts. Note this loudly so users
                // update their scripts, but do not fail.
                if kf8_only {
                    eprintln!(
                        "Note: --kf8-only is now the default for non-dictionary \
                         builds and has no effect. Dictionaries still build as \
                         dual-format MOBI7+KF8 because Kindle's lookup popup \
                         requires the MOBI7 INDX format."
                    );
                }

                // Detect dictionary vs non-dictionary up front so we can flip
                // the kf8_only default correctly and pick the right output
                // extension when the user has not passed -o.
                let is_dictionary = detect_is_dictionary(&input);

                // Dictionaries always build as dual-format MOBI7+KF8.
                // Non-dictionaries default to KF8-only (.azw3); the user can
                // opt back into legacy dual-format via --legacy-mobi.
                let effective_kf8_only = if is_dictionary {
                    if legacy_mobi {
                        eprintln!(
                            "Note: --legacy-mobi is implicit for dictionary \
                             builds (dictionaries always use MOBI7 INDX)."
                        );
                    }
                    false
                } else if legacy_mobi {
                    eprintln!(
                        "Building dual-format MOBI7+KF8 (.mobi). Modern Kindles \
                         prefer KF8-only .azw3. Drop --legacy-mobi to use the \
                         modern default."
                    );
                    false
                } else {
                    true
                };

                let output_path = resolve_output_path(&input, output, effective_kf8_only);
                do_build(&input, &output_path, no_compress, headwords_only, !no_embed_source, include_cmet, no_hd_images, creator_tag, effective_kf8_only, effective_kindle_limits, no_validate, !no_self_check);
            }
            Commands::Comic {
                input,
                output,
                device,
                rtl,
                no_split,
                rotate_spreads,
                crop,
                no_crop,
                no_enhance,
                webtoon,
                no_panel_view,
                jpeg_quality,
                max_height,
                embed_source,
                no_embed_source,
                doc_type,
                title,
                author,
                language,
                cover,
                cover_fill,
                panel_reading_order,
                legacy_mobi,
                kf8_only,
                kindle_limits,
                no_kindle_limits,
                no_self_check,
                kindlegen_parity,
            } => {
                let profile = match comic::get_profile(&device) {
                    Some(p) => p,
                    None => {
                        eprintln!("Error: unknown device '{}'. Valid devices: {}", device, comic::valid_device_names());
                        process::exit(1);
                    }
                };

                // Deprecated: `--kf8-only` is now the default for comic
                // builds. Keep the flag as a hidden no-op so existing scripts
                // keep working, but nudge users toward dropping it.
                if kf8_only {
                    eprintln!(
                        "Note: --kf8-only is now the default for comic builds \
                         and has no effect. Pass --legacy-mobi for the old \
                         dual MOBI7+KF8 behavior."
                    );
                }

                // Comics default to KF8-only (.azw3). --legacy-mobi opts back
                // into dual-format MOBI7+KF8 for pre-2012 Kindles.
                let effective_kf8_only = if legacy_mobi {
                    eprintln!(
                        "Building dual-format MOBI7+KF8 (.mobi). Modern Kindles \
                         prefer KF8-only .azw3. Drop --legacy-mobi to use the \
                         modern default."
                    );
                    false
                } else {
                    true
                };

                let output_path = match output {
                    Some(p) => p,
                    None => {
                        let ext = if effective_kf8_only { "azw3" } else { "mobi" };
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

                // `--no-embed-source` is accepted for backward compatibility
                // but has been a no-op since comics stopped defaulting to
                // embed-source in v0.7.7. Only `--embed-source` turns it on.
                if no_embed_source {
                    eprintln!("Note: --no-embed-source is now the default for comics and has no effect");
                }
                let effective_embed_source = embed_source;

                // --no-crop is a deprecated alias for --crop 0
                let effective_crop = if no_crop { 0 } else { crop };

                let options = comic::ComicOptions {
                    rtl,
                    split: !no_split,
                    crop: effective_crop,
                    enhance: !no_enhance,
                    webtoon,
                    panel_view: !no_panel_view,
                    jpeg_quality,
                    max_height,
                    embed_source: effective_embed_source,
                    doc_type: doc_type_value,
                    title_override: title,
                    author_override: author,
                    language,
                    cover: cover_source,
                    rotate_spreads,
                    panel_reading_order,
                    cover_fill,
                    kindle_limits: effective_kindle_limits,
                    kf8_only: effective_kf8_only,
                    self_check: !no_self_check,
                    kindlegen_parity,
                };

                match comic::build_comic_with_options(&input, &output_path, &profile, &options) {
                    Ok(()) => {
                        let format_name = if effective_kf8_only { "AZW3" } else { "MOBI" };
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
            Commands::Repair {
                input,
                output,
                report_json,
                dry_run,
            } => {
                do_repair(&input, output.as_ref(), report_json, dry_run);
            }
            Commands::RewriteMetadata {
                input,
                output,
                report_json,
                dry_run,
                title,
                authors,
                publisher,
                description,
                language,
                isbn,
                asin,
                publication_date,
                subjects,
                series,
                series_index,
                cover,
            } => {
                do_rewrite_metadata(
                    &input,
                    output.as_ref(),
                    report_json,
                    dry_run,
                    title,
                    authors,
                    publisher,
                    description,
                    language,
                    isbn,
                    asin,
                    publication_date,
                    subjects,
                    series,
                    series_index,
                    cover.as_ref(),
                );
            }
            Commands::Dump { input } => {
                do_dump(&input);
            }
        }
    }
}

/// Parse a MOBI/AZW3 file and print a structural dump to stdout.
fn do_dump(path: &PathBuf) {
    match mobi_dump::dump_mobi(path) {
        Ok(s) => {
            print!("{}", s);
        }
        Err(e) => {
            eprintln!("Error: could not dump {}: {}", path.display(), e);
            process::exit(1);
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

/// Run the EPUB repair pass and print the result.
///
/// `input` is an EPUB file to repair. When `output` is `None`, the repaired
/// copy is written to `<stem>-fixed.epub` in the same directory.
///
/// With `report_json`, the full [`repair::RepairReport`] is emitted as JSON
/// on stdout. Otherwise a human-readable summary is written to stderr with
/// one line per fix and a final count.
///
/// With `dry_run`, the input is only scanned: no output file is written,
/// and the summary is prefixed with "(dry-run)".
///
/// Exit codes:
///   * 0 on success, even if fixes were applied. Callers wanting to know
///     whether the file was already clean should check the report or JSON.
///   * 1 on any `RepairError` (including DRM rejection and non-EPUB input).
fn do_repair(input: &PathBuf, output: Option<&PathBuf>, report_json: bool, dry_run: bool) {
    let default_output;
    let output_path: PathBuf = if dry_run {
        input.clone()
    } else if let Some(p) = output {
        p.clone()
    } else {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repaired".to_string());
        let parent = input.parent().unwrap_or(std::path::Path::new("."));
        default_output = parent.join(format!("{}-fixed.epub", stem));
        default_output
    };

    let result = if dry_run {
        repair::scan_epub(input)
    } else {
        repair::repair_epub(input, &output_path)
    };

    let report = match result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if report_json {
        println!("{}", report.to_json());
    } else {
        let prefix = if dry_run { "(dry-run) " } else { "" };
        for fix in &report.fixes_applied {
            eprintln!("{}{}", prefix, fix.describe());
        }
        for warn in &report.warnings {
            eprintln!("{}warning: {}: {}", prefix, warn.file, warn.message);
        }
        if report.any_fixes() {
            eprintln!(
                "{}Repaired {} issue{} in {}",
                prefix,
                report.fix_count(),
                if report.fix_count() == 1 { "" } else { "s" },
                input.display()
            );
        } else {
            eprintln!("{}No repairs needed for {}", prefix, input.display());
        }
        if !dry_run {
            eprintln!("Output written to {}", output_path.display());
        }
    }
}

/// Run the in-place MOBI/AZW3 metadata rewrite and print the result.
///
/// `input` is a MOBI or AZW3 file whose EXTH metadata records (and
/// optionally the cover image record) are updated according to the
/// provided flags. When `output` is `None`, the rewritten copy is written
/// to `<stem>-meta.<ext>` in the same directory.
///
/// With `report_json`, the full [`mobi_rewrite::RewriteReport`] is emitted
/// as JSON on stdout. Otherwise a human-readable summary is written to
/// stderr listing each EXTH change and a final count.
///
/// With `dry_run`, the input is only scanned: no output file is written
/// and the summary is prefixed with "(dry-run)". Changes still reported.
///
/// Exit codes:
///   * 0 on success, even if no fields were actually changed. Callers
///     wanting to know whether the file was already current should check
///     `no_op` in the report or JSON.
///   * 1 on any `RewriteError` (including DRM rejection, non-MOBI input,
///     or a cover update on a file with no existing cover record).
#[allow(clippy::too_many_arguments)]
fn do_rewrite_metadata(
    input: &PathBuf,
    output: Option<&PathBuf>,
    report_json: bool,
    dry_run: bool,
    title: Option<String>,
    authors: Vec<String>,
    publisher: Option<String>,
    description: Option<String>,
    language: Option<String>,
    isbn: Option<String>,
    asin: Option<String>,
    publication_date: Option<String>,
    subjects: Vec<String>,
    series: Option<String>,
    series_index: Option<String>,
    cover: Option<&PathBuf>,
) {
    let default_output;
    let output_path: PathBuf = if dry_run {
        input.clone()
    } else if let Some(p) = output {
        p.clone()
    } else {
        let stem = input
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "rewritten".to_string());
        let ext = input
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "mobi".to_string());
        let parent = input.parent().unwrap_or(std::path::Path::new("."));
        default_output = parent.join(format!("{}-meta.{}", stem, ext));
        default_output
    };

    // Load cover image bytes up front so any I/O error is reported before
    // we touch the MOBI file.
    let cover_bytes = match cover {
        Some(p) => match std::fs::read(p) {
            Ok(b) => Some(b),
            Err(e) => {
                eprintln!("Error: could not read cover image {}: {}", p.display(), e);
                process::exit(1);
            }
        },
        None => None,
    };

    let updates = mobi_rewrite::MetadataUpdates {
        title,
        authors: if authors.is_empty() { None } else { Some(authors) },
        publisher,
        description,
        language,
        isbn,
        asin,
        publication_date,
        subjects: if subjects.is_empty() {
            None
        } else {
            Some(subjects)
        },
        series,
        series_index,
        cover_image: cover_bytes,
    };

    // Dry run: plan changes against a scratch output then discard. We do
    // this by writing into a temp file and removing it; the rewrite
    // function's byte-stable no-op path handles the case where nothing
    // actually changed.
    let report_result = if dry_run {
        let scratch = std::env::temp_dir().join(format!(
            "kindling_rewrite_metadata_dryrun_{}.bin",
            std::process::id()
        ));
        let r = mobi_rewrite::rewrite_mobi_metadata(input, &scratch, &updates);
        let _ = std::fs::remove_file(&scratch);
        r
    } else {
        mobi_rewrite::rewrite_mobi_metadata(input, &output_path, &updates)
    };

    let report = match report_result {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    };

    if report_json {
        println!("{}", rewrite_report_to_json(&report));
    } else {
        let prefix = if dry_run { "(dry-run) " } else { "" };
        if report.no_op {
            eprintln!("{}No metadata changes needed for {}", prefix, input.display());
        } else {
            for change in &report.changes {
                eprintln!("{}{}", prefix, describe_exth_change(change));
            }
            if report.cover_updated {
                eprintln!("{}Replaced cover image record", prefix);
            }
            let n = report.changes.len() + if report.cover_updated { 1 } else { 0 };
            eprintln!(
                "{}Rewrote {} metadata field{} in {}",
                prefix,
                n,
                if n == 1 { "" } else { "s" },
                input.display()
            );
        }
        if !dry_run {
            eprintln!("Output written to {}", output_path.display());
        }
    }
}

fn describe_exth_change(change: &mobi_rewrite::ExthChange) -> String {
    match change {
        mobi_rewrite::ExthChange::Added { exth_type, value } => {
            format!("added EXTH {} ({})", exth_type, preview_bytes(value))
        }
        mobi_rewrite::ExthChange::Replaced {
            exth_type,
            old_value,
            new_value,
        } => format!(
            "replaced EXTH {} ({} -> {})",
            exth_type,
            preview_bytes(old_value),
            preview_bytes(new_value)
        ),
        mobi_rewrite::ExthChange::Removed { exth_type, old_value } => {
            format!("removed EXTH {} ({})", exth_type, preview_bytes(old_value))
        }
    }
}

fn preview_bytes(b: &[u8]) -> String {
    const MAX: usize = 80;
    match std::str::from_utf8(b) {
        Ok(s) if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t') => {
            if s.len() <= MAX {
                format!("{:?}", s)
            } else {
                format!("{:?}...", &s[..MAX])
            }
        }
        _ => format!("{} bytes", b.len()),
    }
}

/// Serialize a [`mobi_rewrite::RewriteReport`] to a JSON string. Implemented
/// by hand so the module does not need a serde dependency.
fn rewrite_report_to_json(report: &mobi_rewrite::RewriteReport) -> String {
    let mut out = String::new();
    out.push('{');
    out.push_str(&format!(
        "\"input_path\":{},",
        json_string(&report.input_path.display().to_string())
    ));
    out.push_str(&format!(
        "\"output_path\":{},",
        json_string(&report.output_path.display().to_string())
    ));
    out.push_str(&format!("\"no_op\":{},", report.no_op));
    out.push_str(&format!("\"cover_updated\":{},", report.cover_updated));
    out.push_str("\"changes\":[");
    for (i, change) in report.changes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&exth_change_to_json(change));
    }
    out.push(']');
    out.push('}');
    out
}

fn exth_change_to_json(change: &mobi_rewrite::ExthChange) -> String {
    match change {
        mobi_rewrite::ExthChange::Added { exth_type, value } => format!(
            "{{\"kind\":\"added\",\"exth_type\":{},\"value\":{}}}",
            exth_type,
            json_bytes(value)
        ),
        mobi_rewrite::ExthChange::Replaced {
            exth_type,
            old_value,
            new_value,
        } => format!(
            "{{\"kind\":\"replaced\",\"exth_type\":{},\"old_value\":{},\"new_value\":{}}}",
            exth_type,
            json_bytes(old_value),
            json_bytes(new_value)
        ),
        mobi_rewrite::ExthChange::Removed {
            exth_type,
            old_value,
        } => format!(
            "{{\"kind\":\"removed\",\"exth_type\":{},\"old_value\":{}}}",
            exth_type,
            json_bytes(old_value)
        ),
    }
}

fn json_bytes(b: &[u8]) -> String {
    match std::str::from_utf8(b) {
        Ok(s) => json_string(s),
        Err(_) => {
            // Fall back to a base64-free hex-ish notation so the JSON stays
            // self-contained without adding a base64 dependency.
            let mut out = String::from("{\"hex\":\"");
            for byte in b {
                out.push_str(&format!("{:02x}", byte));
            }
            out.push_str("\"}");
            out
        }
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
