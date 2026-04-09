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
mod indx;
mod kf8;
mod mobi;
mod moire;
mod opf;
mod palmdoc;
#[cfg(test)]
mod tests;
mod vwi;

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

        /// Output KF8-only format (.azw3) instead of dual MOBI7+KF8 (.mobi).
        /// KF8-only files are smaller and handled better by Calibre.
        /// Dual format remains available for maximum compatibility with older Kindles.
        #[arg(long)]
        kf8_only: bool,
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
///          [-c0] [-c1] [-c2] [-verbose]
/// Returns (input, output_override)
fn parse_kindlegen_args() -> (PathBuf, Option<String>) {
    let args: Vec<String> = std::env::args().collect();
    let input = PathBuf::from(&args[1]);
    let mut output_name: Option<String> = None;
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
            _ => {
                // Unknown flag, skip
                i += 1;
            }
        }
    }
    (input, output_name)
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

        let result = mobi::build_mobi(
            &opf_path, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None,
        );
        epub::cleanup_temp_dir(&temp_dir);
        result
    } else {
        // Direct OPF input
        mobi::build_mobi(
            input, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag, kf8_only, None,
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
        let (input, output_name) = parse_kindlegen_args();

        // In kindlegen compat mode, -o specifies just a filename next to the input
        let output_path = if let Some(name) = output_name {
            let parent = input.parent().unwrap_or(std::path::Path::new("."));
            parent.join(name)
        } else {
            input.with_extension("mobi")
        };

        do_build(&input, &output_path, false, false, true, false, false, false, false);
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
            } => {
                let output_path = resolve_output_path(&input, output, kf8_only);
                do_build(&input, &output_path, no_compress, headwords_only, !no_embed_source, include_cmet, no_hd_images, creator_tag, kf8_only);
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
                        // Default: input path with .mobi extension
                        if input.is_dir() {
                            input.with_extension("mobi")
                        } else {
                            input.with_extension("mobi")
                        }
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
                };

                match comic::build_comic_with_options(&input, &output_path, &profile, &options) {
                    Ok(()) => {
                        eprintln!("Comic MOBI built successfully: {}", output_path.display());
                    }
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        process::exit(1);
                    }
                }
            }
        }
    }
}
