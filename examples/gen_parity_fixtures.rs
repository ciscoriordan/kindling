//! One-shot generator for the tests/fixtures/parity/ JPEG assets.
//!
//! Run once with `cargo run --example gen_parity_fixtures` and commit the
//! resulting files. Generates:
//! - tests/fixtures/parity/simple_dict/cover.jpg (200x300 solid gray)
//! - tests/fixtures/parity/simple_book/cover.jpg (200x300 solid gray)
//! - tests/fixtures/parity/simple_comic/page1.jpg (400x600 solid red)
//! - tests/fixtures/parity/simple_comic/page2.jpg (400x600 solid green)
//! - tests/fixtures/parity/simple_comic/page3.jpg (400x600 solid blue)

use image::{ImageBuffer, Rgb};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn save_solid(path: PathBuf, w: u32, h: u32, color: [u8; 3], stripe_row: u32) {
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(w, h);
    for (_x, y, px) in img.enumerate_pixels_mut() {
        // Paint a horizontal stripe at a different row per page so the
        // three comic pages don't hash to the same bytes and trigger
        // kindling's image dedup.
        if y == stripe_row || y == stripe_row + 1 {
            *px = Rgb([0, 0, 0]);
        } else {
            *px = Rgb(color);
        }
    }
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    img.save(&path).unwrap();
    println!("wrote {}", path.display());
}

fn main() {
    let root = repo_root();
    let parity = root.join("tests").join("fixtures").join("parity");

    save_solid(
        parity.join("simple_dict").join("cover.jpg"),
        200,
        300,
        [200, 200, 200],
        150,
    );
    save_solid(
        parity.join("simple_book").join("cover.jpg"),
        200,
        300,
        [180, 180, 180],
        100,
    );
    save_solid(
        parity.join("simple_comic").join("page1.jpg"),
        400,
        600,
        [220, 60, 60],
        100,
    );
    save_solid(
        parity.join("simple_comic").join("page2.jpg"),
        400,
        600,
        [60, 200, 60],
        250,
    );
    save_solid(
        parity.join("simple_comic").join("page3.jpg"),
        400,
        600,
        [60, 60, 220],
        400,
    );
}
