//! Minimal example: build a MOBI from an EPUB using kindling as a library.
//!
//! Usage: cargo run --example build_one -- path/to/input.epub path/to/output.mobi

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: build_one <input.epub> <output.mobi>");
        std::process::exit(2);
    }
    let input = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);

    let (temp_dir, opf_path) = kindling::epub::extract_epub(&input)?;

    kindling::mobi::build_mobi(
        &opf_path,
        &output,
        false, // no_compress
        false, // headwords_only
        None,  // srcs_data
        false, // include_cmet
        false, // no_hd_images
        false, // creator_tag
        false, // kf8_only
        None,  // doc_type
        false, // kindle_limits
        true,  // self_check
    )?;

    kindling::epub::cleanup_temp_dir(&temp_dir);
    println!("Built {}", output.display());
    Ok(())
}
