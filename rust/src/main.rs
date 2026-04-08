/// kindling - Kindle MOBI builder for dictionaries and books
///
/// Usage:
///     kindling build input.opf -o output.mobi
///     kindling build input.epub -o output.mobi
///
/// Kindlegen-compatible usage:
///     kindling input.epub
///     kindling input.opf -o output.mobi -dont_append_source -verbose

mod epub;
mod exth;
mod indx;
mod kf8;
mod mobi;
mod opf;
mod palmdoc;
mod vwi;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kindling", about = "Kindle MOBI builder for dictionaries and books")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build MOBI file from OPF or EPUB
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
/// -o is a full path. If no output is specified, replace the input extension with .mobi.
fn resolve_output_path(input: &PathBuf, output: Option<PathBuf>) -> PathBuf {
    match output {
        Some(p) => p,
        None => input.with_extension("mobi"),
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
            eprintln!("Note: --embed-source ignored for non-EPUB input");
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
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag,
        );
        epub::cleanup_temp_dir(&temp_dir);
        result
    } else {
        // Direct OPF input
        mobi::build_mobi(
            input, output_path, no_compress, headwords_only,
            srcs_data.as_deref(), include_cmet, no_hd_images, creator_tag,
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

        do_build(&input, &output_path, false, false, false, false, false, false);
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
            } => {
                let output_path = resolve_output_path(&input, output);
                do_build(&input, &output_path, no_compress, headwords_only, !no_embed_source, include_cmet, no_hd_images, creator_tag);
            }
        }
    }
}
