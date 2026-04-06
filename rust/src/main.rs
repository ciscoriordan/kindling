/// kindling - Kindle dictionary MOBI builder
///
/// Usage:
///     kindling build input.opf -o output.mobi

mod exth;
mod indx;
mod mobi;
mod opf;
mod palmdoc;
mod vwi;

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "kindling", about = "Kindle dictionary MOBI builder")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build MOBI file from OPF
    Build {
        /// Input OPF file
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
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Build {
            input,
            output,
            no_compress,
            headwords_only,
        } => {
            let output_path = output.unwrap_or_else(|| {
                input.with_extension("mobi")
            });

            if let Err(e) = mobi::build_mobi(&input, &output_path, no_compress, headwords_only) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
    }
}
