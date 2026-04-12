//! One-shot generator for tests/fixtures/parity/simple_comic/simple_comic.cbz.
//! Packages the three JPEG pages into a zip archive.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use zip::write::SimpleFileOptions;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = root
        .join("tests")
        .join("fixtures")
        .join("parity")
        .join("simple_comic");
    let cbz_path = dir.join("simple_comic.cbz");

    let f = File::create(&cbz_path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for page in ["page1.jpg", "page2.jpg", "page3.jpg"] {
        let mut buf = Vec::new();
        File::open(dir.join(page))
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        zip.start_file(page, opts).unwrap();
        zip.write_all(&buf).unwrap();
    }
    zip.finish().unwrap();
    println!("wrote {}", cbz_path.display());
}
