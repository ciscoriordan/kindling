//! One-shot generator for a kindlegen-compatible fixed-layout EPUB wrapper
//! around the three JPEG pages in tests/fixtures/parity/simple_comic.
//! Run with `cargo run --example gen_parity_comic_epub`. Output:
//! tests/fixtures/parity/simple_comic/simple_comic.epub
//!
//! Kindlegen does not read CBZ, so the kindlegen reference build pipeline
//! feeds it this EPUB wrapper. Kindling uses the raw CBZ via the `comic`
//! subcommand. The two builders start from the same three images.

use std::fs::{self, File};
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
    let epub_path = dir.join("simple_comic.epub");

    let f = File::create(&epub_path).unwrap();
    let mut zip = zip::ZipWriter::new(f);
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
    )
    .unwrap();

    zip.start_file("OEBPS/content.opf", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="BookId">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
    <dc:title>Parity Test Comic</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Kindling Parity Suite</dc:creator>
    <dc:identifier id="BookId">kindling-parity-comic</dc:identifier>
    <meta name="cover" content="page1"/>
    <meta name="fixed-layout" content="true"/>
    <meta name="orientation-lock" content="portrait"/>
    <meta name="original-resolution" content="400x600"/>
  </metadata>
  <manifest>
    <item id="page1" href="page1.jpg" media-type="image/jpeg"/>
    <item id="page2" href="page2.jpg" media-type="image/jpeg"/>
    <item id="page3" href="page3.jpg" media-type="image/jpeg"/>
    <item id="p1" href="p1.xhtml" media-type="application/xhtml+xml"/>
    <item id="p2" href="p2.xhtml" media-type="application/xhtml+xml"/>
    <item id="p3" href="p3.xhtml" media-type="application/xhtml+xml"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="p1"/>
    <itemref idref="p2"/>
    <itemref idref="p3"/>
  </spine>
</package>"#,
    )
    .unwrap();

    for (i, name) in ["page1.jpg", "page2.jpg", "page3.jpg"].iter().enumerate() {
        let page = i + 1;
        let xhtml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Page {page}</title></head>
<body><div><img src="{name}" alt="page {page}"/></div></body>
</html>"#
        );
        zip.start_file(format!("OEBPS/p{page}.xhtml"), deflated).unwrap();
        zip.write_all(xhtml.as_bytes()).unwrap();

        let mut buf = Vec::new();
        File::open(dir.join(name))
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        zip.start_file(format!("OEBPS/{name}"), stored).unwrap();
        zip.write_all(&buf).unwrap();
    }

    zip.start_file("OEBPS/toc.ncx", deflated).unwrap();
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="kindling-parity-comic"/></head>
  <docTitle><text>Parity Test Comic</text></docTitle>
  <navMap>
    <navPoint id="n1" playOrder="1"><navLabel><text>Page 1</text></navLabel><content src="p1.xhtml"/></navPoint>
    <navPoint id="n2" playOrder="2"><navLabel><text>Page 2</text></navLabel><content src="p2.xhtml"/></navPoint>
    <navPoint id="n3" playOrder="3"><navLabel><text>Page 3</text></navLabel><content src="p3.xhtml"/></navPoint>
  </navMap>
</ncx>"#,
    )
    .unwrap();

    zip.finish().unwrap();
    // Touch fs to flush.
    let _ = fs::metadata(&epub_path).unwrap();
    println!("wrote {}", epub_path.display());
}
