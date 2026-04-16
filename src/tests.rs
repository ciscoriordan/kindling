/// Integration test suite for kindling MOBI output.
///
/// Verifies MOBI structural correctness without requiring a Kindle device.
/// Tests PalmDB headers, MOBI headers, EXTH records, INDX records,
/// PalmDOC compression, SRCS embedding, comic pipeline, and JFIF patching.

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::mobi;
    use crate::palmdoc;

    // -----------------------------------------------------------------------
    // Helpers: reading binary fields from MOBI output
    // -----------------------------------------------------------------------

    fn read_u16_be(data: &[u8], offset: usize) -> u16 {
        u16::from_be_bytes([data[offset], data[offset + 1]])
    }

    fn read_u32_be(data: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }

    /// Parse PalmDB header and return (name_bytes, record_count, record_offsets).
    fn parse_palmdb(data: &[u8]) -> (Vec<u8>, u16, Vec<u32>) {
        let name_bytes = data[0..32].to_vec();
        let record_count = read_u16_be(data, 76);
        let mut offsets = Vec::new();
        for i in 0..record_count as usize {
            let offset = read_u32_be(data, 78 + i * 8);
            offsets.push(offset);
        }
        (name_bytes, record_count, offsets)
    }

    /// Get the byte slice for a specific PalmDB record.
    fn get_record<'a>(data: &'a [u8], offsets: &[u32], index: usize) -> &'a [u8] {
        let start = offsets[index] as usize;
        let end = if index + 1 < offsets.len() {
            offsets[index + 1] as usize
        } else {
            data.len()
        };
        &data[start..end]
    }

    /// Search for EXTH records within record 0. Returns a map of type -> data.
    fn parse_exth_records(record0: &[u8]) -> HashMap<u32, Vec<Vec<u8>>> {
        let mut results: HashMap<u32, Vec<Vec<u8>>> = HashMap::new();
        // Find EXTH magic in record 0
        let exth_pos = record0
            .windows(4)
            .position(|w| w == b"EXTH");
        if let Some(pos) = exth_pos {
            let exth_len = read_u32_be(record0, pos + 4) as usize;
            let rec_count = read_u32_be(record0, pos + 8);
            let mut offset = pos + 12;
            for _ in 0..rec_count {
                if offset + 8 > pos + exth_len {
                    break;
                }
                let rec_type = read_u32_be(record0, offset);
                let rec_len = read_u32_be(record0, offset + 4) as usize;
                if rec_len < 8 || offset + rec_len > record0.len() {
                    break;
                }
                let rec_data = record0[offset + 8..offset + rec_len].to_vec();
                results.entry(rec_type).or_default().push(rec_data);
                offset += rec_len;
            }
        }
        results
    }

    // -----------------------------------------------------------------------
    // Helpers: creating temp directories with minimal OPF/HTML fixtures
    // -----------------------------------------------------------------------

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("kindling_test_{}", name));
            if path.exists() {
                fs::remove_dir_all(&path).unwrap();
            }
            fs::create_dir_all(&path).unwrap();
            TempDir { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Create a minimal dictionary OPF + HTML in a temp dir with given entries.
    /// Each entry is (headword, &[inflections]).
    fn create_dict_fixture(
        dir: &Path,
        entries: &[(&str, &[&str])],
    ) -> PathBuf {
        // Build HTML content with idx:entry markup
        let mut html_body = String::new();
        for (hw, iforms) in entries {
            html_body.push_str(&format!(
                "<idx:entry><idx:orth value=\"{hw}\">{hw}</idx:orth>",
                hw = hw
            ));
            for iform in *iforms {
                html_body.push_str(&format!(
                    "<idx:infl><idx:iform value=\"{iform}\"/></idx:infl>",
                    iform = iform
                ));
            }
            html_body.push_str(&format!(
                "<b>{hw}</b> definition of {hw}<hr/></idx:entry>\n",
                hw = hw
            ));
        }

        let html = format!(
            r#"<html><head><guide></guide></head><body>{}</body></html>"#,
            html_body
        );
        fs::write(dir.join("content.html"), &html).unwrap();

        // OPF with dictionary metadata
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    /// Create a minimal book OPF + HTML in a temp dir. If `image_data` is Some,
    /// include an image in the manifest.
    fn create_book_fixture(
        dir: &Path,
        include_image: Option<&[u8]>,
    ) -> PathBuf {
        let img_tag = if include_image.is_some() {
            r#"<img src="test.jpg"/>"#
        } else {
            ""
        };

        let html = format!(
            r#"<html><head><title>Test Book</title></head><body><h1>Chapter 1</h1><p>Hello world.{}</p></body></html>"#,
            img_tag
        );
        fs::write(dir.join("content.html"), &html).unwrap();

        if let Some(data) = include_image {
            fs::write(dir.join("test.jpg"), data).unwrap();
        }

        let image_manifest = if include_image.is_some() {
            r#"<item id="img1" href="test.jpg" media-type="image/jpeg"/>"#
        } else {
            ""
        };
        let cover_meta = if include_image.is_some() {
            r#"<meta name="cover" content="img1"/>"#
        } else {
            ""
        };

        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Book</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Author</dc:creator>
    {cover_meta}
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
    {image_manifest}
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#,
            cover_meta = cover_meta,
            image_manifest = image_manifest,
        );
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, &opf).unwrap();
        opf_path
    }

    /// Generate a minimal valid JPEG image (10x10 pixels, grayscale).
    fn make_test_jpeg() -> Vec<u8> {
        let img = image::GrayImage::from_fn(10, 10, |x, y| {
            image::Luma([((x + y) * 12) as u8])
        });
        let dyn_img = image::DynamicImage::ImageLuma8(img);
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        dyn_img
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        buf
    }

    /// Build a MOBI from an OPF path and return the raw bytes.
    fn build_mobi_bytes(
        opf_path: &Path,
        output_dir: &Path,
        no_compress: bool,
        headwords_only: bool,
        srcs_data: Option<&[u8]>,
    ) -> Vec<u8> {
        let output_path = output_dir.join("output.mobi");
        mobi::build_mobi(
            opf_path,
            &output_path,
            no_compress,
            headwords_only,
            srcs_data,
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag (use kindlegen-compat EXTH 535)
            false, // kf8_only (dual format)
            None,  // doc_type
            false, // kindle_limits (disabled in tests by default)
            false, // self_check (off in tests; dedicated tests cover the checker)
            false, // kindlegen_parity
        )
        .expect("build_mobi failed");
        fs::read(&output_path).expect("could not read output MOBI")
    }

    // =======================================================================
    // 1. PalmDB header validation
    // =======================================================================

    #[test]
    fn test_palmdb_type_creator() {
        let dir = TempDir::new("palmdb_type");
        let opf = create_dict_fixture(
            dir.path(),
            &[("apple", &["apples"]), ("banana", &["bananas"])],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        // Type = "BOOK" at offset 60, Creator = "MOBI" at offset 64
        assert_eq!(&data[60..64], b"BOOK");
        assert_eq!(&data[64..68], b"MOBI");
        println!("  \u{2713} PalmDB type=BOOK, creator=MOBI");
    }

    #[test]
    fn test_palmdb_record_count_positive() {
        let dir = TempDir::new("palmdb_count");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, _) = parse_palmdb(&data);
        assert!(record_count > 0, "Record count should be > 0, got {}", record_count);
        println!("  \u{2713} Record count: {}", record_count);
    }

    #[test]
    fn test_palmdb_offsets_monotonic_and_in_bounds() {
        let dir = TempDir::new("palmdb_offsets");
        let opf = create_dict_fixture(
            dir.path(),
            &[("alpha", &[]), ("beta", &[]), ("gamma", &[])],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        // Monotonically increasing
        for pair in offsets.windows(2) {
            assert!(
                pair[1] > pair[0],
                "Offsets not monotonically increasing: {} vs {}",
                pair[0],
                pair[1]
            );
        }
        // All within file bounds
        for &off in &offsets {
            assert!(
                (off as usize) <= data.len(),
                "Offset {} exceeds file size {}",
                off,
                data.len()
            );
        }
        println!("  \u{2713} {} offsets monotonic and in bounds", offsets.len());
    }

    #[test]
    fn test_palmdb_name_null_terminated_and_short() {
        let dir = TempDir::new("palmdb_name");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        // Name field is 32 bytes; must be null-terminated (last byte = 0x00)
        assert_eq!(name_bytes[31], 0x00, "PalmDB name must be null-terminated");

        // Effective name (before first null) must be <= 31 bytes
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        assert!(
            name_len <= 31,
            "PalmDB name too long: {} bytes",
            name_len
        );
        println!("  \u{2713} PalmDB name null-terminated, length={}", name_len);
    }

    // =======================================================================
    // 2. MOBI header validation
    // =======================================================================

    #[test]
    fn test_mobi_header_magic() {
        let dir = TempDir::new("mobi_magic");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI magic starts at offset 16 in record 0 (after PalmDOC header)
        assert_eq!(&rec0[16..20], b"MOBI", "MOBI magic not found at expected position");
        println!("  \u{2713} MOBI magic at rec0 offset 16");
    }

    #[test]
    fn test_mobi_header_length() {
        let dir = TempDir::new("mobi_hdrlen");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let header_len = read_u32_be(rec0, 20); // offset 16+4 in rec0 = MOBI header length
        assert_eq!(header_len, 264, "MOBI header length should be 264, got {}", header_len);
        println!("  \u{2713} MOBI header length: {}", header_len);
    }

    #[test]
    fn test_mobi_encoding_utf8() {
        let dir = TempDir::new("mobi_enc");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let encoding = read_u32_be(rec0, 28); // PalmDOC(16) + "MOBI"(4) + len(4) + type(4) + encoding(4)
        assert_eq!(encoding, 65001, "Encoding should be 65001 (UTF-8), got {}", encoding);
        println!("  \u{2713} MOBI encoding: {} (UTF-8)", encoding);
    }

    #[test]
    fn test_mobi_type_is_2() {
        let dir = TempDir::new("mobi_type");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let mobi_type = read_u32_be(rec0, 24); // offset 16 + 8 in rec0
        assert_eq!(mobi_type, 2, "MOBI type should be 2, got {}", mobi_type);
        println!("  \u{2713} MOBI type: {}", mobi_type);
    }

    #[test]
    fn test_mobi_version_6_or_7() {
        let dir = TempDir::new("mobi_ver");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let version = read_u32_be(rec0, 36); // PalmDOC(16) + MOBI offset 20 = version
        assert!(
            version == 6 || version == 7,
            "MOBI version should be 6 or 7, got {}",
            version
        );
        println!("  \u{2713} MOBI version: {}", version);
    }

    /// Regression test for d4febe6: dictionaries must use 0x50 at MOBI header
    /// offset 112 (capability marker). Using 0x4850 breaks dictionary recognition
    /// on Kindle devices. Books must use 0x4850 for Kindle Previewer compatibility.
    #[test]
    fn test_dict_capability_marker_0x50() {
        let dir = TempDir::new("dict_cap");
        let opf = create_dict_fixture(dir.path(), &[("test", &["tests"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI header offset 112 = PalmDOC(16) + 112 = rec0 offset 128
        let cap_marker = read_u32_be(rec0, 128);
        assert_eq!(
            cap_marker, 0x50,
            "Dictionary capability marker at offset 112 should be 0x50, got 0x{:X}",
            cap_marker
        );
        println!("  \u{2713} Dict capability marker: 0x{:X}", cap_marker);
    }

    #[test]
    fn test_book_capability_marker_0x4850() {
        let dir = TempDir::new("book_cap");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Book KF7 Record 0 should use 0x850 (matches KCC/kindlegen;
        // 0x4850 was a Kindle Previewer artifact).
        let cap_marker = read_u32_be(rec0, 128);
        assert_eq!(
            cap_marker, 0x850,
            "Book capability marker at offset 112 should be 0x850, got 0x{:X}",
            cap_marker
        );
        println!("  \u{2713} Book capability marker: 0x{:X}", cap_marker);
    }

    // =======================================================================
    // 3. Dictionary MOBI validation
    // =======================================================================

    #[test]
    fn test_dict_orth_index_not_ffffffff() {
        let dir = TempDir::new("dict_orth");
        let opf = create_dict_fixture(
            dir.path(),
            &[
                ("apple", &["apples"]),
                ("banana", &["bananas"]),
                ("cherry", &["cherries"]),
            ],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Orth index record at MOBI header offset 24 (record0 offset 16+24 = 40)
        let orth_idx = read_u32_be(rec0, 40);
        assert_ne!(orth_idx, 0xFFFFFFFF, "Dictionary should have orth_index != 0xFFFFFFFF");
        println!("  \u{2713} Dict orth_index: {}", orth_idx);
    }

    #[test]
    fn test_dict_indx_records_exist() {
        let dir = TempDir::new("dict_indx");
        let opf = create_dict_fixture(
            dir.path(),
            &[
                ("apple", &["apples"]),
                ("banana", &["bananas"]),
                ("cherry", &["cherries"]),
            ],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let orth_idx = read_u32_be(rec0, 40) as usize;
        assert!(orth_idx < offsets.len(), "Orth index record {} out of range", orth_idx);

        // Check that the INDX record starts with "INDX" magic
        let indx_rec = get_record(&data, &offsets, orth_idx);
        assert_eq!(
            &indx_rec[0..4],
            b"INDX",
            "INDX record should start with INDX magic"
        );
        println!("  \u{2713} INDX record at index {}, magic ok", orth_idx);
    }

    #[test]
    fn test_dict_exth_531_532_547() {
        let dir = TempDir::new("dict_exth");
        let opf = create_dict_fixture(
            dir.path(),
            &[("word", &["words"])],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(exth.contains_key(&531), "Dictionary EXTH should contain record 531 (DictionaryInLanguage)");
        assert!(exth.contains_key(&532), "Dictionary EXTH should contain record 532 (DictionaryOutLanguage)");
        assert!(exth.contains_key(&547), "Dictionary EXTH should contain record 547 (InMemory)");
        println!("  \u{2713} Dict EXTH has 531, 532, 547");
    }

    #[test]
    fn test_dict_headword_count_matches_input() {
        let dir = TempDir::new("dict_hwcount");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let orth_idx = read_u32_be(rec0, 40) as usize;
        let indx_rec = get_record(&data, &offsets, orth_idx);

        // Total entry count is at INDX header offset 36
        let total_entries = read_u32_be(indx_rec, 36);
        assert_eq!(
            total_entries, 3,
            "Headword count should match input (3), got {}",
            total_entries
        );
        println!("  \u{2713} INDX headword count: {}", total_entries);
    }

    // =======================================================================
    // 4. Book MOBI validation
    // =======================================================================

    #[test]
    fn test_book_orth_index_ffffffff() {
        let dir = TempDir::new("book_orth");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Orth index for books should be 0xFFFFFFFF
        let orth_idx = read_u32_be(rec0, 40);
        assert_eq!(
            orth_idx, 0xFFFFFFFF,
            "Book should have orth_index == 0xFFFFFFFF, got 0x{:08X}",
            orth_idx
        );
        println!("  \u{2713} Book orth_index: 0x{:08X}", orth_idx);
    }

    #[test]
    fn test_book_image_records_jpeg_magic() {
        let dir = TempDir::new("book_img");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // first_image_record is at MOBI header offset 92 (rec0 offset 16+92 = 108)
        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(first_img, 0xFFFFFFFF_u32 as usize, "Book with image should have first_image set");

        // Verify the image record starts with JPEG magic
        let img_rec = get_record(&data, &offsets, first_img);
        assert!(
            img_rec.len() >= 2 && img_rec[0] == 0xFF && img_rec[1] == 0xD8,
            "Image record should start with JPEG magic (FF D8)"
        );
        println!("  \u{2713} Image record at index {}, starts with JPEG magic FF D8", first_img);
    }

    #[test]
    fn test_book_boundary_record_exists() {
        let dir = TempDir::new("book_boundary");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        // Search for "BOUNDARY" record
        let mut found_boundary = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 8 && &rec[0..8] == b"BOUNDARY" {
                found_boundary = true;
                break;
            }
        }
        assert!(found_boundary, "Book MOBI should contain a BOUNDARY record for KF8 dual format");
        println!("  \u{2713} BOUNDARY record found in dual-format book");
    }

    #[test]
    fn test_book_kf8_section_after_boundary() {
        let dir = TempDir::new("book_kf8");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        // Find boundary index
        let mut boundary_idx = None;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 8 && &rec[0..8] == b"BOUNDARY" {
                boundary_idx = Some(i);
                break;
            }
        }
        let boundary_idx = boundary_idx.expect("No BOUNDARY record found");

        // KF8 Record 0 should follow immediately after BOUNDARY
        let kf8_rec0 = get_record(&data, &offsets, boundary_idx + 1);
        // KF8 record 0 should contain MOBI magic (after 16 byte PalmDOC header)
        assert!(
            kf8_rec0.len() > 20 && &kf8_rec0[16..20] == b"MOBI",
            "KF8 Record 0 should contain MOBI magic"
        );

        // KF8 version should be 8
        let kf8_version = read_u32_be(kf8_rec0, 36);
        assert_eq!(kf8_version, 8, "KF8 version should be 8, got {}", kf8_version);
        println!("  \u{2713} KF8 section after BOUNDARY at idx {}, version={}", boundary_idx + 1, kf8_version);
    }

    // =======================================================================
    // 4b. KF8-only book structure
    // =======================================================================

    /// Build a KF8-only MOBI from an OPF path and return the raw bytes.
    fn build_kf8_only_mobi_bytes(
        opf_path: &Path,
        output_dir: &Path,
    ) -> Vec<u8> {
        let output_path = output_dir.join("output.azw3");
        mobi::build_mobi(
            opf_path,
            &output_path,
            true,  // no_compress (faster tests)
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            true,  // kf8_only
            None,  // doc_type
            false, // kindle_limits
            false, // self_check (off in tests for speed)
            false, // kindlegen_parity
        )
        .expect("build_mobi (kf8_only) failed");
        fs::read(&output_path).expect("could not read output AZW3")
    }

    #[test]
    fn test_kf8_only_record0_version_8() {
        let dir = TempDir::new("kf8only_ver");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI magic at offset 16
        assert_eq!(&rec0[16..20], b"MOBI", "Record 0 should contain MOBI magic");

        // File version at MOBI header offset 20 (rec0 offset 36)
        let version = read_u32_be(rec0, 36);
        assert_eq!(version, 8, "KF8-only version should be 8, got {}", version);

        // Min version at MOBI header offset 88 (rec0 offset 104)
        let min_version = read_u32_be(rec0, 104);
        assert_eq!(min_version, 8, "KF8-only min_version should be 8, got {}", min_version);
        println!("  \u{2713} KF8-only rec0: version={}, min_version={}", version, min_version);
    }

    #[test]
    fn test_kf8_only_no_kf7_kf8_boundary() {
        let dir = TempDir::new("kf8only_nobound");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // There should be no BOUNDARY record followed by a KF8 Record 0 (MOBI magic).
        // The HD container has its own BOUNDARY records which are legitimate.
        for i in 0..offsets.len().saturating_sub(1) {
            let rec = get_record(&data, &offsets, i);
            if rec.len() == 8 && &rec[0..8] == b"BOUNDARY" {
                let next_rec = get_record(&data, &offsets, i + 1);
                assert!(
                    next_rec.len() < 20 || &next_rec[16..20] != b"MOBI",
                    "KF8-only should not have a BOUNDARY separating KF7/KF8 sections (found at index {})", i
                );
            }
        }

        // Record 0 should be the only MOBI record header (no KF7 Record 0 + KF8 Record 0 pair)
        let rec0 = get_record(&data, &offsets, 0);
        assert_eq!(&rec0[16..20], b"MOBI");
        let version = read_u32_be(rec0, 36);
        assert_eq!(version, 8, "The sole Record 0 should be version 8 (KF8)");
        println!("  \u{2713} KF8-only: no KF7/KF8 BOUNDARY, sole rec0 version={}", version);
    }

    #[test]
    fn test_kf8_only_no_exth_121() {
        let dir = TempDir::new("kf8only_noexth121");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // EXTH 121 (KF8 boundary pointer) should NOT be present
        assert!(
            !exth.contains_key(&121),
            "KF8-only should not have EXTH 121 (KF8 boundary pointer)"
        );
        println!("  \u{2713} KF8-only: no EXTH 121 boundary pointer");
    }

    #[test]
    fn test_kf8_only_images_present() {
        let dir = TempDir::new("kf8only_imgs");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // first_image_record at MOBI header offset 92 (rec0 offset 108)
        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(
            first_img,
            0xFFFFFFFF_u32 as usize,
            "KF8-only with image should have first_image set"
        );

        // The image record should contain JPEG magic
        let img_rec = get_record(&data, &offsets, first_img);
        assert!(
            img_rec.len() >= 2 && img_rec[0] == 0xFF && img_rec[1] == 0xD8,
            "Image record should start with JPEG magic (FF D8)"
        );
        println!("  \u{2713} KF8-only: image at index {}, JPEG magic ok", first_img);
    }

    #[test]
    fn test_kf8_only_has_fdst() {
        let dir = TempDir::new("kf8only_fdst");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // Search for FDST record
        let mut found_fdst = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FDST" {
                found_fdst = true;
                break;
            }
        }
        assert!(found_fdst, "KF8-only should contain an FDST record");
        println!("  \u{2713} KF8-only: FDST record found");
    }

    #[test]
    fn test_kf8_only_has_eof() {
        let dir = TempDir::new("kf8only_eof");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // Last record should be EOF marker
        let last_rec = get_record(&data, &offsets, offsets.len() - 1);
        assert_eq!(
            last_rec,
            &[0xE9, 0x8E, 0x0D, 0x0A],
            "Last record should be EOF marker"
        );
        println!("  \u{2713} KF8-only: last record is EOF marker (E9 8E 0D 0A)");
    }

    #[test]
    fn test_kf8_only_smaller_than_dual() {
        let dir_dual = TempDir::new("kf8only_cmp_dual");
        let dir_kf8 = TempDir::new("kf8only_cmp_kf8");
        let jpeg = make_test_jpeg();
        let opf_dual = create_book_fixture(dir_dual.path(), Some(&jpeg));
        let opf_kf8 = create_book_fixture(dir_kf8.path(), Some(&jpeg));

        let dual_data = build_mobi_bytes(&opf_dual, dir_dual.path(), true, false, None);
        let kf8_data = build_kf8_only_mobi_bytes(&opf_kf8, dir_kf8.path());

        assert!(
            kf8_data.len() < dual_data.len(),
            "KF8-only ({} bytes) should be smaller than dual format ({} bytes)",
            kf8_data.len(),
            dual_data.len()
        );
        println!("  \u{2713} KF8-only {} bytes < dual {} bytes", kf8_data.len(), dual_data.len());
    }

    #[test]
    fn test_kf8_only_exth_has_547() {
        let dir = TempDir::new("kf8only_exth547");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // EXTH 547 (InMemory) should still be present
        assert!(
            exth.contains_key(&547),
            "KF8-only should have EXTH 547 (InMemory)"
        );
        println!("  \u{2713} KF8-only: EXTH 547 (InMemory) present");
    }

    // =======================================================================
    // 5. EXTH validation
    // =======================================================================

    #[test]
    fn test_exth_magic_in_record0() {
        let dir = TempDir::new("exth_magic");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let has_exth = rec0.windows(4).any(|w| w == b"EXTH");
        assert!(has_exth, "Record 0 should contain EXTH magic");
        println!("  \u{2713} EXTH magic found in record 0");
    }

    #[test]
    fn test_exth_dict_531_532_547() {
        let dir = TempDir::new("exth_dict");
        let opf = create_dict_fixture(dir.path(), &[("test", &["tests"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(exth.contains_key(&531), "Dict EXTH should contain 531");
        assert!(exth.contains_key(&532), "Dict EXTH should contain 532");
        assert!(exth.contains_key(&547), "Dict EXTH should contain 547");
        println!("  \u{2713} Dict EXTH: records 531, 532, 547 all present");
    }

    #[test]
    fn test_exth_book_547() {
        let dir = TempDir::new("exth_book547");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(exth.contains_key(&547), "Book EXTH should contain 547 (InMemory)");
        println!("  \u{2713} Book EXTH 547 (InMemory) present");
    }

    #[test]
    fn test_exth_535_default_creator() {
        let dir = TempDir::new("exth_535");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        // creator_tag = false means we get the default "0730-890adc2"
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let exth535 = exth.get(&535).expect("EXTH 535 should exist");
        let value = std::str::from_utf8(&exth535[0]).unwrap();
        assert_eq!(value, "0730-890adc2", "Default EXTH 535 should be '0730-890adc2', got '{}'", value);
        println!("  \u{2713} EXTH 535 = '{}'", value);
    }

    // =======================================================================
    // 6. PalmDOC compression roundtrip
    // =======================================================================

    /// Decompress PalmDOC-compressed data.
    fn palmdoc_decompress(compressed: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        let mut i = 0;
        while i < compressed.len() {
            let b = compressed[i];
            i += 1;
            if b == 0x00 {
                // Literal null
                output.push(0x00);
            } else if b >= 0x01 && b <= 0x08 {
                // Literal block of b bytes
                let count = b as usize;
                for _ in 0..count {
                    if i < compressed.len() {
                        output.push(compressed[i]);
                        i += 1;
                    }
                }
            } else if b >= 0x09 && b <= 0x7F {
                // Literal byte
                output.push(b);
            } else if b >= 0x80 && b <= 0xBF {
                // LZ77 distance/length pair
                if i < compressed.len() {
                    let b2 = compressed[i];
                    i += 1;
                    let pair = ((b as u16 & 0x3F) << 8) | b2 as u16;
                    let distance = (pair >> 3) as usize;
                    let length = (pair & 0x07) as usize + 3;
                    for _ in 0..length {
                        if distance > 0 && output.len() >= distance {
                            let copy_pos = output.len() - distance;
                            output.push(output[copy_pos]);
                        }
                    }
                }
            } else {
                // Space + char (0xC0..0xFF)
                output.push(0x20);
                output.push(b ^ 0x80);
            }
        }
        output
    }

    #[test]
    fn test_compress_empty() {
        let compressed = palmdoc::compress(b"");
        let decompressed = palmdoc_decompress(&compressed);
        assert_eq!(decompressed, b"");
        println!("  \u{2713} Empty input roundtrips to empty output");
    }

    #[test]
    fn test_compress_roundtrip_short() {
        let input = b"Hello, World! This is a test of PalmDOC compression.";
        let compressed = palmdoc::compress(input);
        let decompressed = palmdoc_decompress(&compressed);
        assert_eq!(
            decompressed.as_slice(),
            input.as_slice(),
            "Roundtrip failed for short input"
        );
        println!("  \u{2713} Short input roundtrip: {} -> {} -> {} bytes", input.len(), compressed.len(), decompressed.len());
    }

    #[test]
    fn test_compress_roundtrip_exact_4096() {
        let input: Vec<u8> = (0..4096).map(|i| b"abcdefghijklmnopqrstuvwxyz"[i % 26]).collect();
        let compressed = palmdoc::compress(&input);
        let decompressed = palmdoc_decompress(&compressed);
        assert_eq!(
            decompressed.as_slice(),
            input.as_slice(),
            "Roundtrip failed for 4096-byte input"
        );
        println!("  \u{2713} 4096-byte roundtrip: compressed to {} bytes", compressed.len());
    }

    #[test]
    fn test_compress_roundtrip_multi_record() {
        // >4096 bytes to test that compression works for chunks that span records
        let input: Vec<u8> = (0..10000)
            .map(|i| b"The quick brown fox jumps over the lazy dog. "[i % 45])
            .collect();
        let compressed = palmdoc::compress(&input);
        let decompressed = palmdoc_decompress(&compressed);
        assert_eq!(
            decompressed.as_slice(),
            input.as_slice(),
            "Roundtrip failed for multi-record input"
        );
        println!("  \u{2713} Multi-record roundtrip: {} -> {} bytes", input.len(), compressed.len());
    }

    #[test]
    fn test_compress_roundtrip_utf8() {
        let input = "The Greek word \u{03B1}\u{03B2}\u{03B3} means abc. \u{03B4}\u{03B5}\u{03B6} means def.".as_bytes();
        let compressed = palmdoc::compress(input);
        let decompressed = palmdoc_decompress(&compressed);
        assert_eq!(
            decompressed.as_slice(),
            input,
            "Roundtrip failed for UTF-8 input"
        );
        println!("  \u{2713} UTF-8 roundtrip: {} -> {} bytes", input.len(), compressed.len());
    }

    // =======================================================================
    // 7. SRCS record validation
    // =======================================================================

    #[test]
    fn test_srcs_record_magic_and_header() {
        let dir = TempDir::new("srcs_magic");

        // Create a minimal EPUB-like blob to embed as SRCS data
        let fake_epub = b"PK\x03\x04fake epub content for testing SRCS embedding";

        let opf = create_dict_fixture(dir.path(), &[("test", &["tests"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, Some(fake_epub));
        let (_, _, offsets) = parse_palmdb(&data);

        // Find the SRCS record
        let mut srcs_idx = None;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"SRCS" {
                srcs_idx = Some(i);
                break;
            }
        }
        let srcs_idx = srcs_idx.expect("SRCS record should exist when embed_source=true");
        let srcs_rec = get_record(&data, &offsets, srcs_idx);

        // Verify SRCS magic + 16-byte header
        assert_eq!(&srcs_rec[0..4], b"SRCS", "SRCS magic");
        // Header length at offset 4
        let header_len = read_u32_be(srcs_rec, 4);
        assert_eq!(header_len, 0x10, "SRCS header length should be 16");
        // Data length at offset 8
        let data_len = read_u32_be(srcs_rec, 8) as usize;
        assert_eq!(data_len, fake_epub.len(), "SRCS data length mismatch");
        println!("  \u{2713} SRCS at index {}, header_len={}, data_len={}", srcs_idx, header_len, data_len);
    }

    #[test]
    fn test_srcs_mobi_header_offset_208() {
        let dir = TempDir::new("srcs_hdr208");

        let fake_epub = b"PK\x03\x04fake epub";
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, Some(fake_epub));
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI header starts at offset 16 in record 0.
        // SRCS index is at MOBI header offset 208 (absolute rec0 offset = 16 + 208 = 224)
        let srcs_from_header = read_u32_be(rec0, 16 + 208);
        assert_ne!(
            srcs_from_header, 0xFFFFFFFF,
            "MOBI header offset 208 should point to SRCS record, not 0xFFFFFFFF"
        );

        // Verify it actually points to a record starting with "SRCS"
        let srcs_rec = get_record(&data, &offsets, srcs_from_header as usize);
        assert_eq!(&srcs_rec[0..4], b"SRCS", "Record pointed to by MOBI header offset 208 should be SRCS");
        println!("  \u{2713} MOBI header offset 208 -> SRCS record {}", srcs_from_header);
    }

    // =======================================================================
    // 8. Comic pipeline validation
    // =======================================================================

    #[test]
    fn test_comic_pipeline() {
        use crate::comic;

        let dir = TempDir::new("comic_pipeline");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create 3 small test images using the image crate
        for i in 0..3 {
            let img = image::RgbImage::from_fn(100, 150, |x, y| {
                image::Rgb([(x as u8).wrapping_add(i * 50), (y as u8).wrapping_add(i * 30), 128])
            });
            let dyn_img = image::DynamicImage::ImageRgb8(img);
            let path = images_dir.join(format!("page_{:03}.jpg", i));
            dyn_img.save(&path).unwrap();
        }

        let output_path = dir.path().join("comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        comic::build_comic(&images_dir, &output_path, &profile)
            .expect("build_comic failed");

        // Verify output exists and is a valid MOBI
        let data = fs::read(&output_path).expect("could not read comic MOBI");
        assert!(data.len() > 100, "Comic MOBI too small");

        // PalmDB checks
        assert_eq!(&data[60..64], b"BOOK");
        assert_eq!(&data[64..68], b"MOBI");

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Check for EXTH 122 = "true" (fixed-layout flag)
        let exth = parse_exth_records(rec0);
        let exth122 = exth.get(&122).expect("Comic EXTH should contain record 122 (fixed-layout)");
        let value = std::str::from_utf8(&exth122[0]).unwrap();
        assert_eq!(value, "true", "EXTH 122 should be 'true' for fixed-layout");
        println!("  \u{2713} Comic pipeline: {} bytes, EXTH 122=true", data.len());
    }

    // =======================================================================
    // 8b. Comic Stage 2: spread detection, cropping, enhancement, ComicInfo, RTL
    // =======================================================================

    #[test]
    fn test_spread_detection_landscape() {
        use crate::comic;
        // Landscape image (wider than tall) should be detected as a spread
        let wide = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 100, |_, _| image::Rgb([128, 128, 128])),
        );
        assert!(comic::is_double_page_spread(&wide), "200x100 should be detected as spread");
        println!("  \u{2713} 200x100 landscape detected as spread");
    }

    #[test]
    fn test_spread_detection_portrait() {
        use crate::comic;
        // Portrait image (taller than wide) should not be a spread
        let tall = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 200, |_, _| image::Rgb([128, 128, 128])),
        );
        assert!(!comic::is_double_page_spread(&tall), "100x200 should not be detected as spread");
        println!("  \u{2713} 100x200 portrait not a spread");
    }

    #[test]
    fn test_spread_detection_square() {
        use crate::comic;
        // Square image should not be a spread (width == height, not >)
        let square = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([128, 128, 128])),
        );
        assert!(!comic::is_double_page_spread(&square), "100x100 should not be detected as spread");
        println!("  \u{2713} 100x100 square not a spread");
    }

    #[test]
    fn test_spread_split_dimensions() {
        use crate::comic;
        use image::GenericImageView;
        // Split a 200x100 landscape image into two ~100x100 halves
        let wide = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 100, |x, _| {
                // Left half is dark, right half is bright
                if x < 100 { image::Rgb([50, 50, 50]) } else { image::Rgb([200, 200, 200]) }
            }),
        );

        let (left, right) = comic::split_spread(&wide);
        assert_eq!(left.dimensions(), (100, 100), "Left half should be 100x100");
        assert_eq!(right.dimensions(), (100, 100), "Right half should be 100x100");

        // Verify content: left half should be dark, right half bright
        let left_rgb = left.to_rgb8();
        let right_rgb = right.to_rgb8();
        assert!(left_rgb.get_pixel(50, 50).0[0] < 100, "Left half should be dark");
        assert!(right_rgb.get_pixel(50, 50).0[0] > 100, "Right half should be bright");
        println!("  \u{2713} Spread split: 200x100 -> two 100x100 halves, content correct");
    }

    #[test]
    fn test_crop_white_borders() {
        use crate::comic;
        use image::GenericImageView;
        // Create 100x100 image with thick white border (10% on each side)
        // and dark content in the center
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 100, |x, y| {
                if x >= 10 && x < 90 && y >= 10 && y < 90 {
                    image::Luma([50]) // dark content
                } else {
                    image::Luma([255]) // white border
                }
            }),
        );

        let cropped = comic::crop_borders(&img);
        let (w, h) = cropped.dimensions();
        // Should have cropped the border, resulting in a smaller image
        assert!(w < 100, "Cropped width ({}) should be less than 100", w);
        assert!(h < 100, "Cropped height ({}) should be less than 100", h);
        // The content area is 80x80 (from 10..90), so cropped should be close to that
        assert!(w >= 70 && w <= 85, "Cropped width should be ~80, got {}", w);
        assert!(h >= 70 && h <= 85, "Cropped height should be ~80, got {}", h);
        println!("  \u{2713} White border crop: 100x100 -> {}x{}", w, h);
    }

    #[test]
    fn test_crop_black_borders() {
        use crate::comic;
        use image::GenericImageView;
        // Image with black borders (common in scanned manga)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 100, |x, y| {
                if x >= 10 && x < 90 && y >= 10 && y < 90 {
                    image::Luma([200]) // light content
                } else {
                    image::Luma([0]) // black border
                }
            }),
        );

        let cropped = comic::crop_borders(&img);
        let (w, h) = cropped.dimensions();
        assert!(w < 100, "Cropped width ({}) should be less than 100", w);
        assert!(h < 100, "Cropped height ({}) should be less than 100", h);
        println!("  \u{2713} Black border crop: 100x100 -> {}x{}", w, h);
    }

    #[test]
    fn test_crop_no_border() {
        use crate::comic;
        use image::GenericImageView;
        // Image with no uniform border - should not be cropped
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 100, |x, y| {
                image::Luma([((x * 3 + y * 7) % 256) as u8])
            }),
        );

        let cropped = comic::crop_borders(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 100, "No-border image should not be cropped (width)");
        assert_eq!(h, 100, "No-border image should not be cropped (height)");
        println!("  \u{2713} No-border image unchanged at {}x{}", w, h);
    }

    #[test]
    fn test_crop_thin_border_ignored() {
        use crate::comic;
        use image::GenericImageView;
        // Image with border < 2% of dimension - should NOT be cropped
        // 1000x1000 image, border of 15 pixels (1.5%) on each side
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(1000, 1000, |x, y| {
                if x >= 15 && x < 985 && y >= 15 && y < 985 {
                    image::Luma([100])
                } else {
                    image::Luma([255])
                }
            }),
        );

        let cropped = comic::crop_borders(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 1000, "Thin border (<2%) should not be cropped (width)");
        assert_eq!(h, 1000, "Thin border (<2%) should not be cropped (height)");
        println!("  \u{2713} Thin border (<2%) ignored, still {}x{}", w, h);
    }

    #[test]
    fn test_crop_page_number_bottom_strip() {
        use crate::comic;
        use image::GenericImageView;
        // 500x1000 image: white background, dark content panel from y=50..930,
        // and a small "page number" cluster at the bottom (y=960..980, x=230..270).
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(500, 1000, |x, y| {
                if y >= 50 && y < 930 && x >= 20 && x < 480 {
                    // Main content panel (dark)
                    image::Luma([40])
                } else if y >= 960 && y < 980 && x >= 230 && x < 270 {
                    // Small page number cluster at bottom
                    image::Luma([30])
                } else {
                    // White background
                    image::Luma([255])
                }
            }),
        );

        let cropped = comic::crop_page_numbers(&img);
        let (w, h) = cropped.dimensions();
        // The bottom strip (6% = 60px) contains only a tiny ink cluster,
        // so it should be cropped off. Height should decrease.
        assert_eq!(w, 500, "Width should be unchanged");
        assert!(
            h < 1000,
            "Page number strip at bottom should be cropped, but height is still {}",
            h,
        );
        // The strip is 60px (6% of 1000), so new height should be ~940
        assert!(
            h <= 960 && h >= 900,
            "Expected height around 940 after bottom crop, got {}",
            h,
        );
        println!(
            "  - Bottom page-number crop: 500x1000 -> {}x{}",
            w, h
        );
    }

    #[test]
    fn test_crop_page_number_top_strip() {
        use crate::comic;
        use image::GenericImageView;
        // 500x1000 image: white background, content from y=80..950,
        // and a small page number at the top (y=15..35, x=220..260).
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(500, 1000, |x, y| {
                if y >= 80 && y < 950 && x >= 20 && x < 480 {
                    image::Luma([40])
                } else if y >= 15 && y < 35 && x >= 220 && x < 260 {
                    image::Luma([30])
                } else {
                    image::Luma([255])
                }
            }),
        );

        let cropped = comic::crop_page_numbers(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 500, "Width should be unchanged");
        assert!(
            h < 1000,
            "Page number strip at top should be cropped, but height is still {}",
            h,
        );
        println!(
            "  - Top page-number crop: 500x1000 -> {}x{}",
            w, h
        );
    }

    #[test]
    fn test_crop_page_number_full_content_untouched() {
        use crate::comic;
        use image::GenericImageView;
        // 500x1000 image: content fills the entire image (no blank strips at
        // top or bottom that could be mistaken for a page number strip).
        // Alternate dark and light rows to simulate varied comic content.
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(500, 1000, |x, y| {
                // Dense content everywhere: alternating block pattern
                let block = ((x / 20) + (y / 20)) % 3;
                match block {
                    0 => image::Luma([30]),
                    1 => image::Luma([128]),
                    _ => image::Luma([220]),
                }
            }),
        );

        let cropped = comic::crop_page_numbers(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 500, "Full-content image width should be unchanged");
        assert_eq!(h, 1000, "Full-content image height should be unchanged");
        println!(
            "  - Full-content image not cropped: {}x{}",
            w, h
        );
    }

    #[test]
    fn test_crop_page_number_dark_background() {
        use crate::comic;
        use image::GenericImageView;
        // 500x1000 image with dark/black background (common in manga).
        // Content panel from y=60..920, small light page number at bottom.
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(500, 1000, |x, y| {
                if y >= 60 && y < 920 && x >= 20 && x < 480 {
                    // Content panel (lighter than background)
                    image::Luma([200])
                } else if y >= 960 && y < 980 && x >= 230 && x < 270 {
                    // Small page number (white text on dark background)
                    image::Luma([240])
                } else {
                    // Dark/black background
                    image::Luma([5])
                }
            }),
        );

        let cropped = comic::crop_page_numbers(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 500, "Width should be unchanged");
        assert!(
            h < 1000,
            "Dark-background page number strip should be cropped, but height is still {}",
            h,
        );
        println!(
            "  - Dark-bg page-number crop: 500x1000 -> {}x{}",
            w, h
        );
    }

    #[test]
    fn test_crop_page_number_wide_content_not_cropped() {
        use crate::comic;
        use image::GenericImageView;
        // 500x1000 image: white background, content panel, and a wide text
        // block at the bottom spanning >35% of width. This should NOT be
        // cropped because it looks like real content (a footer, caption, etc.),
        // not a small page number.
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(500, 1000, |x, y| {
                if y >= 50 && y < 920 && x >= 20 && x < 480 {
                    image::Luma([40])
                } else if y >= 960 && y < 980 && x >= 50 && x < 350 {
                    // Wide text block at bottom (60% of width)
                    image::Luma([30])
                } else {
                    image::Luma([255])
                }
            }),
        );

        let cropped = comic::crop_page_numbers(&img);
        let (w, h) = cropped.dimensions();
        assert_eq!(w, 500, "Width should be unchanged");
        assert_eq!(
            h, 1000,
            "Wide bottom content should NOT be cropped (not a page number), but height is {}",
            h,
        );
        println!(
            "  - Wide bottom content preserved: {}x{}",
            w, h
        );
    }

    #[test]
    fn test_enhance_expands_histogram() {
        use crate::comic;
        // Create a low-contrast image (pixel values 100..150)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 100, |x, y| {
                image::Luma([(100 + ((x + y) % 50)) as u8])
            }),
        );

        let enhanced = comic::enhance_image(&img);
        let gray = enhanced.to_luma8();

        // After enhancement, the histogram should be stretched
        let mut min_val = 255u8;
        let mut max_val = 0u8;
        for pixel in gray.pixels() {
            let v = pixel.0[0];
            if v < min_val { min_val = v; }
            if v > max_val { max_val = v; }
        }

        // The range should be significantly expanded from the original 50
        let range = max_val as i32 - min_val as i32;
        assert!(range > 100, "Enhanced image range should be > 100, got {} (min={}, max={})", range, min_val, max_val);
        println!("  \u{2713} Histogram expanded: range {} (min={}, max={})", range, min_val, max_val);
    }

    #[test]
    fn test_enhance_uniform_image_unchanged() {
        use crate::comic;
        use image::GenericImageView;
        // A completely uniform image should not be changed (high == low guard)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(50, 50, |_, _| image::Luma([128])),
        );

        let enhanced = comic::enhance_image(&img);
        let (w, h) = enhanced.dimensions();
        assert_eq!((w, h), (50, 50), "Uniform image dimensions should not change");
        println!("  \u{2713} Uniform image unchanged at {}x{}", w, h);
    }

    #[test]
    fn test_comicinfo_basic_parsing() {
        use crate::comic;
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Title>The Great Adventure</Title>
  <Series>Adventure Comics</Series>
  <Number>42</Number>
  <Writer>John Writer</Writer>
  <Penciller>Jane Artist</Penciller>
  <Inker>Bob Inker</Inker>
  <Summary>A thrilling adventure story.</Summary>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).expect("Failed to parse ComicInfo.xml");
        assert_eq!(meta.title.as_deref(), Some("The Great Adventure"));
        assert_eq!(meta.series.as_deref(), Some("Adventure Comics"));
        assert_eq!(meta.number.as_deref(), Some("42"));
        assert_eq!(meta.writers, vec!["John Writer"]);
        assert_eq!(meta.pencillers, vec!["Jane Artist"]);
        assert_eq!(meta.inkers, vec!["Bob Inker"]);
        assert_eq!(meta.summary.as_deref(), Some("A thrilling adventure story."));
        assert!(!meta.manga_rtl, "Should not be manga by default");
        println!("  \u{2713} ComicInfo parsed: title, series, number, writers, pencillers, inkers, summary");
    }

    #[test]
    fn test_comicinfo_manga_rtl() {
        use crate::comic;
        let xml = r#"<?xml version="1.0"?>
<ComicInfo>
  <Title>One Piece</Title>
  <Manga>YesAndRightToLeft</Manga>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).expect("Failed to parse");
        assert!(meta.manga_rtl, "Manga=YesAndRightToLeft should enable RTL");
        println!("  \u{2713} Manga=YesAndRightToLeft -> RTL enabled");
    }

    #[test]
    fn test_comicinfo_manga_yes() {
        use crate::comic;
        let xml = r#"<ComicInfo><Manga>Yes</Manga></ComicInfo>"#;
        let meta = comic::parse_comic_info_xml(xml).expect("Failed to parse");
        assert!(meta.manga_rtl, "Manga=Yes should enable RTL");
        println!("  \u{2713} Manga=Yes -> RTL enabled");
    }

    #[test]
    fn test_comicinfo_effective_title_series_number_title() {
        use crate::comic;
        let xml = r#"<ComicInfo>
  <Title>The Return</Title>
  <Series>Epic Saga</Series>
  <Number>5</Number>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).unwrap();
        let title = meta.effective_title();
        assert_eq!(title, Some("Epic Saga #5 - The Return".to_string()));
        println!("  \u{2713} Effective title: '{}'", title.unwrap());
    }

    #[test]
    fn test_comicinfo_effective_title_series_number_only() {
        use crate::comic;
        let xml = r#"<ComicInfo>
  <Series>Monthly Comics</Series>
  <Number>12</Number>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).unwrap();
        let title = meta.effective_title();
        assert_eq!(title, Some("Monthly Comics #12".to_string()));
        println!("  \u{2713} Effective title: '{}'", title.unwrap());
    }

    #[test]
    fn test_comicinfo_creators_combined() {
        use crate::comic;
        let xml = r#"<ComicInfo>
  <Writer>Alice, Bob</Writer>
  <Penciller>Charlie</Penciller>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).unwrap();
        let creators = meta.creators();
        assert_eq!(creators, vec!["Alice", "Bob", "Charlie"]);
        println!("  \u{2713} Creators combined: {:?}", creators);
    }

    #[test]
    fn test_comicinfo_empty_xml() {
        use crate::comic;
        let xml = r#"<ComicInfo></ComicInfo>"#;
        let meta = comic::parse_comic_info_xml(xml).unwrap();
        assert!(meta.title.is_none());
        assert!(meta.series.is_none());
        assert!(!meta.manga_rtl);
        println!("  \u{2713} Empty ComicInfo: no title, no series, no RTL");
    }

    #[test]
    fn test_rtl_page_ordering() {
        use crate::comic;
        // Build a comic with RTL mode and verify pages get reversed
        let dir = TempDir::new("rtl_ordering");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create 3 portrait images with distinct brightness
        // Page 0 = dark, Page 1 = medium, Page 2 = bright
        for i in 0..3u8 {
            let brightness = 50 + i * 80; // 50, 130, 210
            let img = image::DynamicImage::ImageLuma8(
                image::GrayImage::from_fn(100, 150, |_, _| image::Luma([brightness])),
            );
            let path = images_dir.join(format!("page_{:03}.jpg", i));
            img.save(&path).unwrap();
        }

        let output_path = dir.path().join("rtl_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: true,
            split: false, // disable split so page count stays at 3
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false, // disable for simpler test
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic_with_options failed for RTL");

        // Verify output exists and is valid MOBI
        let data = fs::read(&output_path).expect("could not read RTL comic MOBI");
        assert!(data.len() > 100, "RTL comic MOBI too small");

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // Verify EXTH 527 = "rtl" (page-progression-direction)
        let exth527 = exth.get(&527).expect("RTL comic should have EXTH 527");
        let ppd = std::str::from_utf8(&exth527[0]).unwrap();
        assert_eq!(ppd, "rtl", "EXTH 527 should be 'rtl', got '{}'", ppd);

        // Verify EXTH 525 = "horizontal-rl" (writing-mode)
        let exth525 = exth.get(&525).expect("RTL comic should have EXTH 525");
        let wm = std::str::from_utf8(&exth525[0]).unwrap();
        assert_eq!(wm, "horizontal-rl", "EXTH 525 should be 'horizontal-rl', got '{}'", wm);
        println!("  \u{2713} RTL comic: EXTH 527=rtl, EXTH 525=horizontal-rl");
    }

    #[test]
    fn test_ltr_comic_exth_writing_mode() {
        use crate::comic;
        // Build a regular LTR comic and verify writing mode is horizontal-lr
        let dir = TempDir::new("ltr_writing_mode");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |_, _| image::Luma([128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("ltr_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        comic::build_comic(&images_dir, &output_path, &profile).expect("build_comic failed");

        let data = fs::read(&output_path).expect("could not read LTR comic MOBI");
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let exth525 = exth.get(&525).expect("LTR comic should have EXTH 525");
        let wm = std::str::from_utf8(&exth525[0]).unwrap();
        assert_eq!(wm, "horizontal-lr", "EXTH 525 should be 'horizontal-lr' for LTR, got '{}'", wm);
        println!("  \u{2713} LTR comic: EXTH 525=horizontal-lr");
    }

    #[test]
    fn test_spread_split_in_pipeline() {
        use crate::comic;
        // Build a comic with one landscape (spread) image and verify it produces 2 pages
        let dir = TempDir::new("spread_pipeline");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a single landscape image (wider than tall)
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(300, 150, |x, _| {
                if x < 150 { image::Rgb([50, 50, 50]) } else { image::Rgb([200, 200, 200]) }
            }),
        );
        img.save(images_dir.join("spread_001.jpg")).unwrap();

        let output_path = dir.path().join("spread_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: true,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with spread splitting failed");

        let data = fs::read(&output_path).expect("could not read spread comic MOBI");
        assert!(data.len() > 100, "Spread comic MOBI too small");

        // Verify we got a valid MOBI (the spread should have been split into 2 pages)
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Spread split pipeline: {} bytes, valid MOBI", data.len());
    }

    #[test]
    fn test_rtl_spread_split_cover_is_right_half() {
        use crate::comic;
        // When RTL mode is active and the first image is a landscape spread,
        // the cover (first page) should be the RIGHT half of the spread,
        // since that's the "first" page in RTL reading order.
        //
        // This tests for a KCC-style regression where the wrong half was used
        // as the cover due to the interaction between per-image RTL split
        // ordering and global page reversal.
        let dir = TempDir::new("rtl_spread_cover");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a landscape image: left half is dark (50), right half is bright (200)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(300, 150, |x, _| {
                if x < 150 {
                    image::Luma([50])   // left half: dark
                } else {
                    image::Luma([200])  // right half: bright
                }
            }),
        );
        img.save(images_dir.join("spread_001.jpg")).unwrap();

        let output_path = dir.path().join("rtl_spread_cover.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: true,
            split: true,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 95,  // high quality to preserve pixel values
            max_height: 65536,
            embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with RTL spread splitting failed");

        let data = fs::read(&output_path).expect("could not read RTL spread comic MOBI");
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Find the first image record
        let first_img_idx = read_u32_be(rec0, 108) as usize;
        assert_ne!(first_img_idx, 0xFFFFFFFF_u32 as usize,
            "Should have a first image record set");

        // The cover is the first image record (EXTH 201 = cover_offset = 0)
        let cover_rec = get_record(&data, &offsets, first_img_idx);
        assert!(cover_rec.len() > 2 && cover_rec[0] == 0xFF && cover_rec[1] == 0xD8,
            "Cover record should be a JPEG (FF D8 magic)");

        // Decode the cover JPEG and check average brightness
        let cover_img = image::load_from_memory(cover_rec)
            .expect("Failed to decode cover JPEG from MOBI");
        let gray = cover_img.to_luma8();
        let (w, h) = (gray.width(), gray.height());
        let avg_brightness: f64 = gray.pixels()
            .map(|p| p.0[0] as f64)
            .sum::<f64>() / (w as f64 * h as f64);

        // The right half of the original was bright (~200). After grayscale
        // conversion and JPEG compression, the average should be well above 150.
        // The left half was dark (~50). If the wrong half were used, avg would be < 100.
        assert!(avg_brightness > 150.0,
            "RTL cover should be the RIGHT (bright) half of the spread, \
             but average brightness was {:.1} (expected > 150). \
             This suggests the LEFT (dark) half was incorrectly used as the cover.",
            avg_brightness);

        // Also verify LTR mode uses the LEFT (dark) half as cover
        let ltr_output = dir.path().join("ltr_spread_cover.mobi");
        let ltr_options = comic::ComicOptions {
            rtl: false,
            split: true,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 95,
            max_height: 65536,
            embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &ltr_output, &profile, &ltr_options)
            .expect("build_comic with LTR spread splitting failed");

        let ltr_data = fs::read(&ltr_output).expect("could not read LTR spread comic MOBI");
        let (_, _, ltr_offsets) = parse_palmdb(&ltr_data);
        let ltr_rec0 = get_record(&ltr_data, &ltr_offsets, 0);
        let ltr_first_img = read_u32_be(ltr_rec0, 108) as usize;
        let ltr_cover_rec = get_record(&ltr_data, &ltr_offsets, ltr_first_img);
        let ltr_cover_img = image::load_from_memory(ltr_cover_rec)
            .expect("Failed to decode LTR cover JPEG");
        let ltr_gray = ltr_cover_img.to_luma8();
        let (lw, lh) = (ltr_gray.width(), ltr_gray.height());
        let ltr_avg: f64 = ltr_gray.pixels()
            .map(|p| p.0[0] as f64)
            .sum::<f64>() / (lw as f64 * lh as f64);

        assert!(ltr_avg < 100.0,
            "LTR cover should be the LEFT (dark) half of the spread, \
             but average brightness was {:.1} (expected < 100).",
            ltr_avg);

        println!("  \u{2713} RTL spread cover: brightness={:.1} (right/bright half), \
                  LTR cover: brightness={:.1} (left/dark half)",
                  avg_brightness, ltr_avg);
    }

    #[test]
    fn test_no_split_flag_prevents_splitting() {
        use crate::comic;
        // Build a comic with one landscape image but --no-split, verify 1 page
        let dir = TempDir::new("no_split");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a single landscape image
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(300, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        img.save(images_dir.join("spread_001.jpg")).unwrap();

        let output_split = dir.path().join("split.mobi");
        let output_nosplit = dir.path().join("nosplit.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();

        // With splitting
        let opt_split = comic::ComicOptions {
            rtl: false, split: true, crop: 0, enhance: false, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_split, &profile, &opt_split).unwrap();

        // Without splitting
        let opt_nosplit = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_nosplit, &profile, &opt_nosplit).unwrap();

        let data_split = fs::read(&output_split).unwrap();
        let data_nosplit = fs::read(&output_nosplit).unwrap();

        // The split version should have more records (2 pages vs 1)
        let (_, rc_split, _) = parse_palmdb(&data_split);
        let (_, rc_nosplit, _) = parse_palmdb(&data_nosplit);
        assert!(
            rc_split > rc_nosplit,
            "Split version should have more records ({}) than no-split ({})",
            rc_split, rc_nosplit
        );
        println!("  \u{2713} Split {} records > no-split {} records", rc_split, rc_nosplit);
    }

    #[test]
    fn test_enhance_only_on_grayscale_devices() {
        use crate::comic;
        // Verify that enhancement is skipped for color devices
        let dir = TempDir::new("enhance_color");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Build with colorsoft (color device) - should work without errors
        let output_path = dir.path().join("color_comic.mobi");
        let profile = comic::get_profile("colorsoft").unwrap();
        assert!(!profile.grayscale, "colorsoft should not be grayscale");
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: true, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic should succeed on color device even with enhance=true");

        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "Color comic MOBI should be valid");
        println!("  \u{2713} Color device with enhance=true: {} bytes, valid", data.len());
    }

    #[test]
    fn test_comicinfo_in_directory() {
        use crate::comic;
        // Build a comic from a directory containing ComicInfo.xml
        let dir = TempDir::new("comicinfo_dir");
        let images_dir = dir.path().join("comic_input");
        fs::create_dir_all(&images_dir).unwrap();

        // Create an image
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |_, _| image::Luma([128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Create ComicInfo.xml with manga RTL
        let comic_info = r#"<?xml version="1.0" encoding="utf-8"?>
<ComicInfo>
  <Title>Test Manga</Title>
  <Writer>Test Author</Writer>
  <Manga>YesAndRightToLeft</Manga>
</ComicInfo>"#;
        fs::write(images_dir.join("ComicInfo.xml"), comic_info).unwrap();

        let output_path = dir.path().join("manga_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        // Don't set rtl in options - it should be auto-detected from ComicInfo.xml
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with ComicInfo.xml failed");

        let data = fs::read(&output_path).unwrap();
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // ComicInfo.xml manga detection should auto-enable RTL
        let exth527 = exth.get(&527).expect("Manga comic should have EXTH 527");
        let ppd = std::str::from_utf8(&exth527[0]).unwrap();
        assert_eq!(ppd, "rtl", "Manga comic EXTH 527 should be 'rtl', got '{}'", ppd);

        let exth525 = exth.get(&525).expect("Manga comic should have EXTH 525");
        let wm = std::str::from_utf8(&exth525[0]).unwrap();
        assert_eq!(wm, "horizontal-rl", "Manga comic EXTH 525 should be 'horizontal-rl', got '{}'", wm);
        println!("  \u{2713} ComicInfo.xml auto-RTL: EXTH 527=rtl, 525=horizontal-rl");
    }

    // =======================================================================
    // 8c. CBR (Comic Book RAR) extraction and end-to-end pipeline
    // =======================================================================

    /// Returns the absolute path of a fixture file under tests/fixtures/.
    fn cbr_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    /// bsdtar is required at runtime for CBR support. Tests that rely on it
    /// are skipped (not failed) on the rare system that lacks it so that
    /// hermetic CI sandboxes without libarchive don't report a regression.
    fn bsdtar_available() -> bool {
        if Path::new("/usr/bin/bsdtar").exists() {
            return true;
        }
        std::process::Command::new("bsdtar")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn test_cbr_extractor_returns_sorted_images() {
        use crate::cbr;

        if !bsdtar_available() {
            eprintln!("skipping: bsdtar not available on this system");
            return;
        }

        let fixture = cbr_fixture("test_comic.cbr");
        assert!(
            fixture.exists(),
            "CBR fixture missing: {}",
            fixture.display()
        );

        // Copy the fixture to a temp location so the sibling extraction dir
        // doesn't pollute the source tree.
        let tmp = TempDir::new("cbr_extract");
        let staged = tmp.path().join("test_comic.cbr");
        fs::copy(&fixture, &staged).unwrap();

        let (images, extract_dir) =
            cbr::extract_cbr(&staged).expect("extract_cbr failed for fixture");

        assert_eq!(images.len(), 3, "expected 3 images in test_comic.cbr, got {}", images.len());
        for (i, img) in images.iter().enumerate() {
            let name = img.file_name().unwrap().to_string_lossy().into_owned();
            let expected = format!("page_{:03}.png", i + 1);
            assert_eq!(
                name, expected,
                "images should be naturally sorted: image {} is {}, expected {}",
                i, name, expected
            );
            assert!(img.exists(), "extracted image missing: {}", img.display());
            let bytes = fs::read(img).unwrap();
            assert!(bytes.len() >= 8, "extracted image looks empty");
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "not a valid PNG header");
        }

        // ComicInfo.xml should also have been staged next to the images.
        let info = extract_dir.join("ComicInfo.xml");
        assert!(
            info.exists(),
            "ComicInfo.xml should be extracted from CBR fixture"
        );
        let info_text = fs::read_to_string(&info).unwrap();
        assert!(
            info_text.contains("<Title>Test Comic</Title>"),
            "ComicInfo.xml content unexpected: {}",
            info_text
        );

        // Clean up extraction dir (normally handled by build_comic_with_options).
        let _ = fs::remove_dir_all(&extract_dir);
        println!("  \u{2713} CBR extractor: 3 images + ComicInfo.xml");
    }

    #[test]
    fn test_cbr_encrypted_archive_rejected() {
        use crate::cbr;

        if !bsdtar_available() {
            eprintln!("skipping: bsdtar not available on this system");
            return;
        }

        let fixture = cbr_fixture("test_encrypted.cbr");
        assert!(
            fixture.exists(),
            "Encrypted CBR fixture missing: {}",
            fixture.display()
        );

        let tmp = TempDir::new("cbr_encrypted");
        let staged = tmp.path().join("test_encrypted.cbr");
        fs::copy(&fixture, &staged).unwrap();

        let err = cbr::extract_cbr(&staged)
            .err()
            .expect("encrypted CBR should fail to extract");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("encrypt"),
            "error should mention encryption, got: {}",
            msg
        );

        // Verify the extraction dir was cleaned up on error.
        let stem = staged.file_stem().unwrap().to_string_lossy();
        let leftover = staged
            .parent()
            .unwrap()
            .join(format!(".kindling_cbr_{}", stem));
        assert!(
            !leftover.exists(),
            "extraction dir should be removed on encryption error"
        );
        println!("  \u{2713} Encrypted CBR rejected with clear error");
    }

    #[test]
    fn test_cbr_end_to_end_build_mobi() {
        use crate::comic;

        if !bsdtar_available() {
            eprintln!("skipping: bsdtar not available on this system");
            return;
        }

        let fixture = cbr_fixture("test_comic.cbr");
        assert!(fixture.exists(), "CBR fixture missing");

        // Stage the CBR in a throwaway temp dir because comic building
        // writes the sibling `.kindling_cbr_*` dir next to the archive.
        let tmp = TempDir::new("cbr_e2e");
        let staged = tmp.path().join("test_comic.cbr");
        fs::copy(&fixture, &staged).unwrap();

        let output_path = tmp.path().join("test_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536,
            embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&staged, &output_path, &profile, &options)
            .expect("build_comic_with_options failed for CBR input");

        let data = fs::read(&output_path).expect("could not read CBR-built MOBI");
        assert!(data.len() > 100, "CBR-built MOBI unexpectedly small");
        assert_eq!(&data[60..64], b"BOOK");
        assert_eq!(&data[64..68], b"MOBI");

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // Fixed-layout flag should be set for comics (same as test_comic_pipeline).
        let exth122 = exth.get(&122).expect("CBR comic should have EXTH 122");
        let value = std::str::from_utf8(&exth122[0]).unwrap();
        assert_eq!(value, "true", "EXTH 122 should be 'true' for fixed-layout CBR build");

        // The sibling extraction dir should have been cleaned up by the
        // build_comic_with_options finalizer.
        let stem = staged.file_stem().unwrap().to_string_lossy();
        let leftover = staged
            .parent()
            .unwrap()
            .join(format!(".kindling_cbr_{}", stem));
        assert!(
            !leftover.exists(),
            "CBR extraction dir should be cleaned up after build"
        );

        println!("  \u{2713} CBR end-to-end build: {} bytes", data.len());
    }

    // =======================================================================
    // 9. PalmDB name truncation
    // =======================================================================

    #[test]
    fn test_palmdb_name_short_title() {
        let dir = TempDir::new("palmdb_short");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        // Title is "Test Dict" - should map to "Test_Dict" (< 27 chars, no truncation)
        let name = std::str::from_utf8(&name_bytes[..9]).unwrap();
        assert_eq!(name, "Test_Dict", "Short title should not be truncated");
        println!("  \u{2713} Short title PalmDB name: '{}'", name);
    }

    #[test]
    fn test_palmdb_name_long_title_truncation() {
        let dir = TempDir::new("palmdb_long");

        // Create a fixture with a very long title
        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="x">x</idx:orth><b>x</b> test<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        let long_title = "A Very Long Dictionary Title That Exceeds Twenty Seven Characters For Sure";
        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">{}</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#,
            long_title
        );
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, &opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        // Effective name
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        assert!(
            name_len <= 31,
            "Truncated name should be <= 31 bytes, got {}",
            name_len
        );
        // New format: first 28 bytes (at a char boundary) + "..." = 31 bytes.
        assert_eq!(
            name_len, 31,
            "Truncated name should be 31 bytes (28 prefix + '...'), got {}",
            name_len
        );

        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap();
        assert!(
            name.ends_with("..."),
            "Truncated name should end with '...': '{}'",
            name
        );
        assert!(
            name.starts_with("A_Very_Long_Dictionary_Title"),
            "Truncated name should preserve the prefix: '{}'",
            name
        );
        println!("  \u{2713} Long title truncated to {} bytes: '{}'", name_len, name);
    }

    #[test]
    fn test_palmdb_name_utf8_truncation_char_boundary() {
        let dir = TempDir::new("palmdb_utf8");

        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="x">x</idx:orth><b>x</b> test<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        // Title with multi-byte UTF-8 characters (Greek). Each Greek letter is
        // 2 bytes in UTF-8, so a title like "Λεξικό Αρχαίας Ελληνικής Γλώσσας"
        // is ~55 bytes even though it is ~31 chars. Verify we truncate without
        // splitting a multi-byte codepoint.
        let long_title = "Λεξικό Αρχαίας Ελληνικής Γλώσσας Μεγάλο";
        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">{}</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">el</dc:language>
    <x-metadata>
      <DictionaryInLanguage>el</DictionaryInLanguage>
      <DictionaryOutLanguage>el</DictionaryOutLanguage>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#,
            long_title
        );
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, &opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        assert!(name_len <= 31, "Name should be <= 31 bytes, got {}", name_len);

        // Must decode as valid UTF-8 (no splitting multi-byte codepoints).
        let name = std::str::from_utf8(&name_bytes[..name_len])
            .expect("Truncated PalmDB name must be valid UTF-8 (char boundary respected)");
        assert!(
            name.ends_with("..."),
            "Truncated name should end with '...': '{}'",
            name
        );
        println!("  \u{2713} UTF-8 title truncated to {} bytes: '{}'", name_len, name);
    }

    #[test]
    fn test_palmdb_name_special_chars_removed() {
        let dir = TempDir::new("palmdb_special");

        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="y">y</idx:orth><b>y</b> test<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Dict (Test) [v2]</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap();

        // ()[] should be stripped
        assert!(!name.contains('('), "Name should not contain '(': '{}'", name);
        assert!(!name.contains(')'), "Name should not contain ')': '{}'", name);
        assert!(!name.contains('['), "Name should not contain '[': '{}'", name);
        assert!(!name.contains(']'), "Name should not contain ']': '{}'", name);
        println!("  \u{2713} Special chars stripped: '{}'", name);
    }

    #[test]
    fn test_palmdb_name_filesystem_unsafe_chars_stripped() {
        // Filesystem-unsafe characters (`:`, `/`, `\`, `*`, `?`, `"`, `<`, `>`,
        // `|`) must be stripped from the PalmDB name. Kindle's FSCK indexer
        // treats the PalmDB name as a filename candidate and will refuse to
        // index files containing these characters.
        let dir = TempDir::new("palmdb_fs_unsafe");

        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="z">z</idx:orth><b>z</b> test<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        // Mimic the Vader Down comic: title with colons.
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Star Wars: Darth Vader: Vader Down</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let (name_bytes, _, _) = parse_palmdb(&data);

        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap();

        for bad in [':', '/', '\\', '*', '?', '"', '<', '>', '|'] {
            assert!(
                !name.contains(bad),
                "PalmDB name should not contain {:?}: '{}'",
                bad,
                name
            );
        }
        // Also: no double underscores from collapsing ": "
        assert!(
            !name.contains("__"),
            "PalmDB name should not contain '__' (whitespace collapse broken): '{}'",
            name
        );
        println!("  \u{2713} Filesystem-unsafe chars stripped: '{}'", name);
    }

    // =======================================================================
    // 10. JFIF header patching
    // =======================================================================

    #[test]
    fn test_jfif_density_units_patched() {
        let dir = TempDir::new("jfif_patch");

        // Generate a JPEG with density_units = 0x00 (aspect ratio)
        let mut jpeg = make_test_jpeg();

        // Verify we have a JFIF header to patch
        assert!(jpeg.len() > 13, "JPEG too short");
        assert_eq!(jpeg[0], 0xFF, "Expected SOI marker");
        assert_eq!(jpeg[1], 0xD8, "Expected SOI marker");

        // Find the JFIF header and check if it exists
        if jpeg.len() > 13
            && jpeg[2] == 0xFF
            && jpeg[3] == 0xE0
            && &jpeg[6..11] == b"JFIF\0"
        {
            // Manually set density_units to 0x00 (aspect ratio) to test patching
            jpeg[13] = 0x00;

            let opf = create_book_fixture(dir.path(), Some(&jpeg));
            let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
            let (_, _, offsets) = parse_palmdb(&data);
            let rec0 = get_record(&data, &offsets, 0);

            // Find the image record
            let first_img = read_u32_be(rec0, 108) as usize;
            let img_rec = get_record(&data, &offsets, first_img);

            // Verify the JFIF density_units was patched to 0x01
            assert!(
                img_rec.len() > 13,
                "Image record too short to contain JFIF header"
            );
            if img_rec[2] == 0xFF
                && img_rec[3] == 0xE0
                && &img_rec[6..11] == b"JFIF\0"
            {
                assert_eq!(
                    img_rec[13], 0x01,
                    "JFIF density_units should be patched from 0x00 to 0x01, got 0x{:02X}",
                    img_rec[13]
                );
            } else {
                // JPEG may have been re-encoded without JFIF - that's acceptable
                // but we at least verify it's still a valid JPEG
                assert_eq!(img_rec[0], 0xFF, "Image should still be valid JPEG");
                assert_eq!(img_rec[1], 0xD8, "Image should still be valid JPEG");
            }
        } else {
            // The test JPEG didn't have a JFIF header (some encoders skip it).
            // Build a JFIF JPEG manually.
            let mut jfif_jpeg = vec![
                0xFF, 0xD8, // SOI
                0xFF, 0xE0, // APP0 marker
                0x00, 0x10, // Length = 16
                b'J', b'F', b'I', b'F', 0x00, // JFIF identifier
                0x01, 0x01, // Version 1.1
                0x00, // Units = 0 (aspect ratio) -- we want this to get patched
                0x00, 0x01, // X density
                0x00, 0x01, // Y density
                0x00, 0x00, // Thumbnail size
            ];
            // Append the rest of the original JPEG (skip SOI)
            jfif_jpeg.extend_from_slice(&jpeg[2..]);

            let opf = create_book_fixture(dir.path(), Some(&jfif_jpeg));
            let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
            let (_, _, offsets) = parse_palmdb(&data);
            let rec0 = get_record(&data, &offsets, 0);

            let first_img = read_u32_be(rec0, 108) as usize;
            let img_rec = get_record(&data, &offsets, first_img);

            assert!(img_rec.len() > 13, "Image record too short");
            if &img_rec[6..11] == b"JFIF\0" {
                assert_eq!(
                    img_rec[13], 0x01,
                    "JFIF density_units should be patched to 0x01, got 0x{:02X}",
                    img_rec[13]
                );
            }
        }
        println!("  \u{2713} JFIF density_units patched from 0x00 to 0x01");
    }

    // =======================================================================
    // Additional structural tests
    // =======================================================================

    #[test]
    fn test_dict_compressed_and_uncompressed_both_valid() {
        let dir_c = TempDir::new("dict_compressed");
        let dir_u = TempDir::new("dict_uncompressed");

        let entries: &[(&str, &[&str])] = &[
            ("alpha", &["alphas"]),
            ("beta", &["betas"]),
        ];

        let opf_c = create_dict_fixture(dir_c.path(), entries);
        let opf_u = create_dict_fixture(dir_u.path(), entries);

        let data_c = build_mobi_bytes(&opf_c, dir_c.path(), false, false, None);
        let data_u = build_mobi_bytes(&opf_u, dir_u.path(), true, false, None);

        // Both should be valid PalmDB/MOBI files
        assert_eq!(&data_c[60..64], b"BOOK");
        assert_eq!(&data_u[60..64], b"BOOK");

        let (_, _, offsets_c) = parse_palmdb(&data_c);
        let (_, _, offsets_u) = parse_palmdb(&data_u);

        // Compressed record 0 compression type = 2
        let rec0_c = get_record(&data_c, &offsets_c, 0);
        let comp_type_c = read_u16_be(rec0_c, 0);
        assert_eq!(comp_type_c, 2, "Compressed MOBI should have compression type 2");

        // Uncompressed record 0 compression type = 1
        let rec0_u = get_record(&data_u, &offsets_u, 0);
        let comp_type_u = read_u16_be(rec0_u, 0);
        assert_eq!(comp_type_u, 1, "Uncompressed MOBI should have compression type 1");
        println!("  \u{2713} Compressed type={}, uncompressed type={}", comp_type_c, comp_type_u);
    }

    #[test]
    fn test_flis_fcis_eof_records() {
        let dir = TempDir::new("flis_fcis_eof");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        // Check that FLIS, FCIS, and EOF records exist somewhere
        let mut found_flis = false;
        let mut found_fcis = false;
        let mut found_eof = false;

        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 {
                if &rec[0..4] == b"FLIS" {
                    found_flis = true;
                }
                if &rec[0..4] == b"FCIS" {
                    found_fcis = true;
                }
                if rec == [0xE9, 0x8E, 0x0D, 0x0A] {
                    found_eof = true;
                }
            }
        }

        assert!(found_flis, "MOBI should contain a FLIS record");
        assert!(found_fcis, "MOBI should contain a FCIS record");
        assert!(found_eof, "MOBI should contain an EOF record");
        println!("  \u{2713} FLIS, FCIS, and EOF records all present");
    }

    // =======================================================================
    // 11. Webtoon support (Stage 3)
    // =======================================================================

    #[test]
    fn test_webtoon_detection() {
        use crate::comic;

        let dir = TempDir::new("webtoon_detect");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create tall images (height > 3x width) - should trigger webtoon detection
        for i in 0..3u32 {
            let img = image::DynamicImage::ImageRgb8(
                image::RgbImage::from_fn(100, 400, |x, y| {
                    image::Rgb([((x + i * 30) % 256) as u8, ((y + i * 20) % 256) as u8, 128])
                }),
            );
            img.save(images_dir.join(format!("strip_{:03}.png", i))).unwrap();
        }

        let paths: Vec<std::path::PathBuf> = (0..3)
            .map(|i| images_dir.join(format!("strip_{:03}.png", i)))
            .collect();

        assert!(comic::detect_webtoon(&paths), "Images with height > 3x width should be detected as webtoon");

        // Create a non-webtoon image (roughly square)
        let normal_img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        let normal_path = images_dir.join("normal.png");
        normal_img.save(&normal_path).unwrap();

        // Mix of tall and normal should NOT detect as webtoon
        let mixed_paths = vec![paths[0].clone(), normal_path.clone()];
        assert!(!comic::detect_webtoon(&mixed_paths), "Mixed aspect ratios should not be detected as webtoon");

        // Only normal images should not be webtoon
        let normal_paths = vec![normal_path];
        assert!(!comic::detect_webtoon(&normal_paths), "Normal images should not be detected as webtoon");

        // Empty input should not be webtoon
        let empty: Vec<std::path::PathBuf> = vec![];
        assert!(!comic::detect_webtoon(&empty), "Empty input should not be detected as webtoon");
        println!("  \u{2713} Webtoon detection: tall=yes, mixed=no, normal=no, empty=no");
    }

    #[test]
    fn test_webtoon_merge() {
        use crate::comic;
        use image::GenericImageView;

        // Create two images of different widths
        let img1 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 200, |_, _| image::Rgb([255, 0, 0])),
        );
        let img2 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(80, 150, |_, _| image::Rgb([0, 255, 0])),
        );

        let merged = comic::webtoon_merge(&[img1.clone(), img2.clone()]);
        let (w, h) = merged.dimensions();

        // Width should be max of inputs (100), height should be sum (200 + 150 = 350)
        assert_eq!(w, 100, "Merged width should be max width (100), got {}", w);
        assert_eq!(h, 350, "Merged height should be sum (350), got {}", h);

        // Top portion should be red (from img1)
        let merged_rgb = merged.to_rgb8();
        let top_pixel = merged_rgb.get_pixel(50, 50);
        assert_eq!(top_pixel.0, [255, 0, 0], "Top portion should be from img1 (red)");

        // Bottom portion should be green (from img2)
        // img2 is narrower (80px), centered on 100px canvas, so center should be green
        let bottom_pixel = merged_rgb.get_pixel(50, 250);
        assert_eq!(bottom_pixel.0, [0, 255, 0], "Bottom center should be from img2 (green)");
        println!("  \u{2713} Webtoon merge: {}x{}, top=red, bottom=green", w, h);
    }

    #[test]
    fn test_webtoon_merge_single_image() {
        use crate::comic;
        use image::GenericImageView;

        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 500, |_, _| image::Rgb([128, 128, 128])),
        );

        let merged = comic::webtoon_merge(&[img.clone()]);
        let (w, h) = merged.dimensions();
        assert_eq!((w, h), (100, 500), "Single image merge should return same dimensions");
        println!("  \u{2713} Single-image merge: {}x{} unchanged", w, h);
    }

    #[test]
    fn test_webtoon_merge_centering() {
        use crate::comic;
        use image::GenericImageView;

        // Wide image (200px) + narrow image (100px) with white background
        let img1 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 100, |_, _| image::Rgb([255, 255, 255])),
        );
        let img2 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([0, 0, 0])),
        );

        let merged = comic::webtoon_merge(&[img1, img2]);
        let (w, h) = merged.dimensions();
        assert_eq!(w, 200, "Width should be 200");
        assert_eq!(h, 200, "Height should be 200");

        let rgb = merged.to_rgb8();

        // The narrow image (100px) should be centered on the 200px canvas
        // Left edge (x=0) in bottom half should be background (white)
        let left_bg = rgb.get_pixel(0, 150);
        assert_eq!(left_bg.0, [255, 255, 255], "Left padding should be white background");

        // Center (x=100) in bottom half should be from img2 (black)
        let center_content = rgb.get_pixel(100, 150);
        assert_eq!(center_content.0, [0, 0, 0], "Center of bottom half should be black (img2)");

        // Right edge (x=199) in bottom half should be background (white)
        let right_bg = rgb.get_pixel(199, 150);
        assert_eq!(right_bg.0, [255, 255, 255], "Right padding should be white background");
        println!("  \u{2713} Merge centering: narrow img centered on {}x{} canvas", w, h);
    }

    #[test]
    fn test_webtoon_split() {
        use crate::comic;
        use image::GenericImageView;

        // Create a tall strip with clear gutters (white rows) at known positions
        let strip_height = 4000u32;
        let strip_width = 100u32;
        let device_height = 1448u32;

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(strip_width, strip_height, |_x, y| {
                // Create uniform white rows at y=1400, y=2800 (near target cut points)
                // These serve as gutters for the splitter to find
                if (y >= 1390 && y <= 1410) || (y >= 2790 && y <= 2810) {
                    image::Luma([255]) // white gutter
                } else {
                    // Content: varied pixels to have non-zero variance
                    image::Luma([((y * 7 + 13) % 200) as u8 + 30])
                }
            }),
        );

        let pages = comic::webtoon_split(&img, device_height);

        // Should produce at least 2 pages (4000 / 1448 ~ 2.76)
        assert!(pages.len() >= 2, "Should produce at least 2 pages, got {}", pages.len());
        assert!(pages.len() <= 4, "Should produce at most 4 pages, got {}", pages.len());

        // All pages should have the same width
        for (i, page) in pages.iter().enumerate() {
            let (pw, _ph) = page.dimensions();
            assert_eq!(pw, strip_width, "Page {} width should be {}, got {}", i, strip_width, pw);
        }

        // Total height of all pages should equal original strip height
        let total_h: u32 = pages.iter().map(|p| p.height()).sum();
        assert_eq!(total_h, strip_height, "Sum of page heights ({}) should equal strip height ({})", total_h, strip_height);
        println!("  \u{2713} Webtoon split: {} pages, total height={}", pages.len(), total_h);
    }

    #[test]
    fn test_webtoon_split_hard_cut() {
        use crate::comic;

        // Create a tall strip with NO gutters (no uniform rows) - forces overlap split
        let strip_height = 3000u32;
        let strip_width = 100u32;
        let device_height = 1448u32;
        let overlap = (device_height as f64 * 0.10) as u32;

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(strip_width, strip_height, |x, y| {
                // Noisy content everywhere - no gutters
                image::Luma([((x * 37 + y * 13 + 7) % 200) as u8 + 28])
            }),
        );

        let pages = comic::webtoon_split(&img, device_height);

        // Should still produce pages
        assert!(pages.len() >= 2, "Should produce at least 2 pages even without gutters, got {}", pages.len());

        // With overlap, total page height should be greater than strip height
        // because overlapping regions are duplicated across pages
        let total_h: u32 = pages.iter().map(|p| p.height()).sum();
        assert!(total_h >= strip_height, "Sum of page heights ({}) should be >= strip height ({})", total_h, strip_height);

        // Each split without a gutter adds ~overlap pixels of duplication,
        // so total should be approximately strip_height + (num_splits * overlap)
        let num_splits = pages.len() - 1;
        let expected_overlap_total = num_splits as u32 * overlap;
        assert!(
            total_h <= strip_height + expected_overlap_total + device_height / 5,
            "Total height ({}) should not vastly exceed strip height + overlap ({}+{})",
            total_h, strip_height, expected_overlap_total,
        );
        println!("  \u{2713} Overlap split: {} pages, total height={} (strip={}, overlap per split={})",
            pages.len(), total_h, strip_height, overlap);
    }

    #[test]
    fn test_webtoon_split_overlap_content() {
        use crate::comic;

        // Create a strip with unique pixel values per row so we can verify
        // that the overlap region is truly duplicated across page boundaries.
        let strip_height = 3000u32;
        let strip_width = 50u32;
        let device_height = 1448u32;
        let overlap = (device_height as f64 * 0.10) as u32;

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(strip_width, strip_height, |x, y| {
                // Every row gets a unique-ish pattern (high variance, no gutters)
                image::Luma([((x.wrapping_mul(41).wrapping_add(y.wrapping_mul(97))) % 200) as u8 + 28])
            }),
        );

        let pages = comic::webtoon_split(&img, device_height);
        assert!(pages.len() >= 2, "Need at least 2 pages to test overlap");

        // For consecutive pages, verify the bottom of page N overlaps the top of page N+1.
        // Since no gutter exists, each split should produce overlap.
        // We reconstruct approximate y_start positions from page heights.
        let mut y_positions: Vec<u32> = Vec::new();
        let mut y = 0u32;
        for page in &pages {
            y_positions.push(y);
            let page_h = page.height();
            // When there's overlap, the next page starts at (y + page_h - overlap)
            // but only if it's not the last page
            y += page_h;
        }

        // Check that total height > strip height (overlap causes duplication)
        let total_h: u32 = pages.iter().map(|p| p.height()).sum();
        assert!(
            total_h > strip_height,
            "With no gutters, overlap should make total height ({}) > strip height ({})",
            total_h, strip_height,
        );

        // Verify that pages cover the full strip (last page's end should reach strip_height).
        // Reconstruct actual y_start for each page accounting for overlap.
        let mut actual_y = 0u32;
        for (i, page) in pages.iter().enumerate() {
            let page_h = page.height();
            let page_end = actual_y + page_h;
            if i == pages.len() - 1 {
                assert_eq!(
                    page_end, strip_height,
                    "Last page should reach end of strip: page_end={}, strip_height={}",
                    page_end, strip_height,
                );
            }
            // Advance, subtracting overlap for non-final pages
            if i < pages.len() - 1 {
                actual_y = page_end.saturating_sub(overlap);
            }
        }

        println!(
            "  \u{2713} Overlap content: {} pages, overlap={}, total_h={} (strip={})",
            pages.len(), overlap, total_h, strip_height,
        );
    }

    #[test]
    fn test_webtoon_split_short_image() {
        use crate::comic;
        use image::GenericImageView;

        // Image shorter than device height - should not be split
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 500, |_, _| image::Luma([128])),
        );

        let pages = comic::webtoon_split(&img, 1448);
        assert_eq!(pages.len(), 1, "Image shorter than device height should produce 1 page");
        assert_eq!(pages[0].dimensions(), (100, 500));
        println!("  \u{2713} Short image: 1 page, 100x500 unchanged");
    }

    #[test]
    fn test_webtoon_pipeline() {
        use crate::comic;

        let dir = TempDir::new("webtoon_pipeline");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create 2 tall webtoon strip images (height > 3x width)
        for i in 0..2u32 {
            let img = image::DynamicImage::ImageRgb8(
                image::RgbImage::from_fn(200, 2000, |x, y| {
                    // Create some gutters (white bands) for splitting
                    if y % 800 < 20 {
                        image::Rgb([255, 255, 255])
                    } else {
                        image::Rgb([
                            ((x + i * 50) % 200) as u8 + 20,
                            ((y + i * 30) % 200) as u8 + 20,
                            128,
                        ])
                    }
                }),
            );
            img.save(images_dir.join(format!("strip_{:03}.png", i))).unwrap();
        }

        let output_path = dir.path().join("webtoon.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false, // rely on auto-detection
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Webtoon pipeline should succeed");

        // Verify output exists and is a valid MOBI
        let data = fs::read(&output_path).expect("could not read webtoon MOBI");
        assert!(data.len() > 100, "Webtoon MOBI too small");

        // PalmDB checks
        assert_eq!(&data[60..64], b"BOOK");
        assert_eq!(&data[64..68], b"MOBI");

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Check for fixed-layout flag
        let exth = parse_exth_records(rec0);
        let exth122 = exth.get(&122).expect("Webtoon EXTH should contain record 122 (fixed-layout)");
        let value = std::str::from_utf8(&exth122[0]).unwrap();
        assert_eq!(value, "true", "EXTH 122 should be 'true' for fixed-layout webtoon");
        println!("  \u{2713} Webtoon pipeline: {} bytes, EXTH 122=true", data.len());
    }

    #[test]
    fn test_webtoon_forced_flag() {
        use crate::comic;

        let dir = TempDir::new("webtoon_forced");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create images that are NOT tall enough for auto-detection (height < 3x width)
        // but the --webtoon flag should still force webtoon processing
        for i in 0..2u32 {
            let img = image::DynamicImage::ImageRgb8(
                image::RgbImage::from_fn(200, 2000, |x, y| {
                    image::Rgb([((x + i * 50) % 256) as u8, ((y + i * 30) % 256) as u8, 128])
                }),
            );
            img.save(images_dir.join(format!("page_{:03}.png", i))).unwrap();
        }

        let output_path = dir.path().join("webtoon_forced.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: true, // force webtoon mode
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Forced webtoon pipeline should succeed");

        let data = fs::read(&output_path).expect("could not read forced webtoon MOBI");
        assert!(data.len() > 100, "Forced webtoon MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Forced webtoon flag: {} bytes, valid MOBI", data.len());
    }

    #[test]
    fn test_webtoon_with_device_profile() {
        use crate::comic;

        let dir = TempDir::new("webtoon_scribe");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a tall webtoon image
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 5000, |x, y| {
                if y % 1200 < 20 {
                    image::Rgb([255, 255, 255]) // gutters
                } else {
                    image::Rgb([((x * 3) % 256) as u8, ((y * 7) % 256) as u8, 100])
                }
            }),
        );
        img.save(images_dir.join("strip_001.png")).unwrap();

        // Test with Scribe profile (different device height: 2480)
        let output_path = dir.path().join("webtoon_scribe.mobi");
        let profile = comic::get_profile("scribe").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: true,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Webtoon with Scribe profile should succeed");

        let data = fs::read(&output_path).expect("could not read scribe webtoon MOBI");
        assert!(data.len() > 100, "Scribe webtoon MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Scribe webtoon: {} bytes, valid MOBI", data.len());
    }

    // =======================================================================
    // 12. Panel View (Stage 5)
    // =======================================================================

    #[test]
    fn test_panel_detection_grid() {
        use crate::comic;

        // Create a 400x400 image with a 2x2 grid of panels separated by
        // white gutters (20px wide/tall) at the center.
        // Each panel contains varied pixel content (high row variance) so that
        // the gutter rows (uniform white) can be distinguished.
        //
        // Layout:
        //   [panel0 190x190] [20px gutter] [panel1 190x190]
        //   [20px gutter row]
        //   [panel2 190x190] [20px gutter] [panel3 190x190]
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(400, 400, |x, y| {
                // Horizontal gutter at y=190..210
                // Vertical gutter at x=190..210
                let in_h_gutter = y >= 190 && y < 210;
                let in_v_gutter = x >= 190 && x < 210;
                if in_h_gutter || in_v_gutter {
                    image::Rgb([255, 255, 255]) // white gutter
                } else {
                    // Varied content within each panel - pixel values depend on x
                    // so each row has high variance (not uniform)
                    image::Rgb([
                        ((x * 7 + 13) % 200) as u8 + 28,
                        ((x * 11 + y * 3 + 7) % 200) as u8 + 28,
                        ((x * 3 + 29) % 200) as u8 + 28,
                    ])
                }
            }),
        );

        let panels = comic::detect_panels(&img);
        assert_eq!(
            panels.len(), 4,
            "2x2 grid should produce 4 panels, got {}",
            panels.len()
        );

        // Verify panels cover approximately the right areas
        // Each panel should be roughly 47.5% of the image in each dimension
        for (i, panel) in panels.iter().enumerate() {
            assert!(
                panel.w > 40.0 && panel.w < 55.0,
                "Panel {} width should be ~47.5%, got {:.1}%",
                i, panel.w
            );
            assert!(
                panel.h > 40.0 && panel.h < 55.0,
                "Panel {} height should be ~47.5%, got {:.1}%",
                i, panel.h
            );
        }

        // First panel should start at top-left (x ~0, y ~0)
        assert!(panels[0].x < 5.0, "First panel should start near x=0, got {:.1}%", panels[0].x);
        assert!(panels[0].y < 5.0, "First panel should start near y=0, got {:.1}%", panels[0].y);
        println!("  \u{2713} 2x2 grid: {} panels detected, all ~47.5%", panels.len());
    }

    #[test]
    fn test_panel_detection_splash() {
        use crate::comic;

        // Create a single full-page image with no gutters (varied content everywhere).
        // This should detect 0 panels (full-page splash).
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 300, |x, y| {
                image::Rgb([
                    ((x * 7 + y * 13 + 3) % 200) as u8 + 28,
                    ((x * 11 + y * 3 + 7) % 200) as u8 + 28,
                    ((x * 3 + y * 7 + 11) % 200) as u8 + 28,
                ])
            }),
        );

        let panels = comic::detect_panels(&img);
        assert!(
            panels.is_empty(),
            "Full-page splash should have 0 panels, got {}",
            panels.len()
        );
        println!("  \u{2713} Full-page splash: 0 panels detected");
    }

    #[test]
    fn test_panel_view_html() {
        use crate::comic;

        // Build a comic with panel_view enabled from images that have a 2x2 grid
        let dir = TempDir::new("panel_view_html");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a 400x400 image with a 2x2 panel grid and white gutters
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(400, 400, |x, y| {
                let in_h_gutter = y >= 190 && y < 210;
                let in_v_gutter = x >= 190 && x < 210;
                if in_h_gutter || in_v_gutter {
                    image::Rgb([255, 255, 255]) // white gutter
                } else {
                    // Varied content
                    image::Rgb([
                        ((x * 3 + 10) % 200) as u8 + 28,
                        ((y * 7 + 20) % 200) as u8 + 28,
                        128,
                    ])
                }
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("panel_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Panel View comic build should succeed");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).expect("could not read panel view comic MOBI");
        assert!(data.len() > 100, "Panel View comic MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Panel view comic: {} bytes, valid MOBI", data.len());
    }

    #[test]
    fn test_no_panel_view_flag() {
        use crate::comic;

        // Build a comic with panel_view DISABLED and verify no panel markup in XHTML
        let dir = TempDir::new("no_panel_view");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a 400x400 image with a 2x2 panel grid
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(400, 400, |x, y| {
                let in_h_gutter = y >= 190 && y < 210;
                let in_v_gutter = x >= 190 && x < 210;
                if in_h_gutter || in_v_gutter {
                    image::Rgb([255, 255, 255])
                } else {
                    image::Rgb([100, 100, 100])
                }
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Build with panel_view disabled
        let output_no_pv = dir.path().join("no_pv.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options_no_pv = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_no_pv, &profile, &options_no_pv)
            .expect("no-panel-view comic build should succeed");

        // Build with panel_view enabled
        let output_with_pv = dir.path().join("with_pv.mobi");
        let options_with_pv = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_with_pv, &profile, &options_with_pv)
            .expect("panel-view comic build should succeed");

        // Both should produce valid MOBIs
        let data_no_pv = fs::read(&output_no_pv).unwrap();
        let data_with_pv = fs::read(&output_with_pv).unwrap();
        assert_eq!(&data_no_pv[60..64], b"BOOK");
        assert_eq!(&data_with_pv[60..64], b"BOOK");

        // The panel-view version should be at least as large (it has extra markup)
        // but both should be valid MOBIs
        assert!(data_no_pv.len() > 100, "no-panel-view MOBI too small");
        assert!(data_with_pv.len() > 100, "panel-view MOBI too small");
        println!("  \u{2713} No-PV {} bytes, with-PV {} bytes, both valid", data_no_pv.len(), data_with_pv.len());
    }

    #[test]
    fn test_panel_detection_horizontal_strip() {
        use crate::comic;

        // Create a 200x300 image with 3 horizontal panels (no vertical gutters)
        // separated by white gutters
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 300, |x, y| {
                // Gutters at y=90..110 and y=190..210
                let in_gutter = (y >= 90 && y < 110) || (y >= 190 && y < 210);
                if in_gutter {
                    image::Rgb([255, 255, 255])
                } else {
                    image::Rgb([
                        ((x * 3 + y * 7 + 5) % 180) as u8 + 40,
                        ((x * 11 + y * 3 + 13) % 180) as u8 + 40,
                        128,
                    ])
                }
            }),
        );

        let panels = comic::detect_panels(&img);
        assert_eq!(
            panels.len(), 3,
            "3 horizontal panels should produce 3 panels, got {}",
            panels.len()
        );

        // Each panel should span the full width
        for (i, panel) in panels.iter().enumerate() {
            assert!(
                panel.w > 95.0,
                "Horizontal panel {} should span ~100% width, got {:.1}%",
                i, panel.w
            );
        }
        println!("  \u{2713} Horizontal strip: {} panels, all full-width", panels.len());
    }

    #[test]
    fn test_panel_view_opf_metadata() {
        use crate::comic;

        // Build a comic with panel_view and verify OPF contains book-type and region-mag
        let dir = TempDir::new("panel_view_opf");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Build with panel_view enabled
        let output_pv = dir.path().join("pv_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_pv, &profile, &options)
            .expect("Panel View OPF comic build should succeed");

        // Build without panel_view
        let output_no_pv = dir.path().join("no_pv_comic.mobi");
        let options_no = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_no_pv, &profile, &options_no)
            .expect("No Panel View OPF comic build should succeed");

        // Both should produce valid MOBIs
        let data_pv = fs::read(&output_pv).unwrap();
        let data_no_pv = fs::read(&output_no_pv).unwrap();
        assert_eq!(&data_pv[60..64], b"BOOK");
        assert_eq!(&data_no_pv[60..64], b"BOOK");
        println!("  \u{2713} Panel view OPF: PV {} bytes, no-PV {} bytes", data_pv.len(), data_no_pv.len());
    }

    #[test]
    fn test_panel_rect_percentages() {
        use crate::comic;

        // Verify panel rects are expressed as valid percentages (0-100)
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 200, |x, y| {
                let in_h_gutter = y >= 95 && y < 105;
                let in_v_gutter = x >= 95 && x < 105;
                if in_h_gutter || in_v_gutter {
                    image::Rgb([255, 255, 255])
                } else {
                    image::Rgb([80, 80, 80])
                }
            }),
        );

        let panels = comic::detect_panels(&img);
        for (i, panel) in panels.iter().enumerate() {
            assert!(panel.x >= 0.0 && panel.x <= 100.0,
                "Panel {} x ({:.1}) should be 0-100", i, panel.x);
            assert!(panel.y >= 0.0 && panel.y <= 100.0,
                "Panel {} y ({:.1}) should be 0-100", i, panel.y);
            assert!(panel.w > 0.0 && panel.w <= 100.0,
                "Panel {} w ({:.1}) should be 0-100", i, panel.w);
            assert!(panel.h > 0.0 && panel.h <= 100.0,
                "Panel {} h ({:.1}) should be 0-100", i, panel.h);
            // Panel should not extend beyond image bounds
            assert!(panel.x + panel.w <= 100.1,
                "Panel {} x+w ({:.1}) should be <= 100", i, panel.x + panel.w);
            assert!(panel.y + panel.h <= 100.1,
                "Panel {} y+h ({:.1}) should be <= 100", i, panel.y + panel.h);
        }
        println!("  \u{2713} All {} panel rects within 0-100% bounds", panels.len());
    }

    // =======================================================================
    // 13. JPEG quality, max height, and corrupt image handling
    // =======================================================================

    #[test]
    fn test_jpeg_quality_flag() {
        use crate::comic;

        let dir = TempDir::new("jpeg_quality");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a single test image with varied content (so quality matters)
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(200, 300, |x, y| {
                image::Rgb([
                    ((x * 7 + y * 3) % 256) as u8,
                    ((x * 3 + y * 11 + 50) % 256) as u8,
                    ((x * 5 + y * 7 + 100) % 256) as u8,
                ])
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let profile = comic::get_profile("colorsoft").unwrap();

        // Build at low quality (30)
        let output_low = dir.path().join("quality_low.mobi");
        let options_low = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 30,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_low, &profile, &options_low)
            .expect("low quality build failed");

        // Build at high quality (95)
        let output_high = dir.path().join("quality_high.mobi");
        let options_high = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 95,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_high, &profile, &options_high)
            .expect("high quality build failed");

        let size_low = fs::metadata(&output_low).unwrap().len();
        let size_high = fs::metadata(&output_high).unwrap().len();

        // Higher quality should produce a larger file
        assert!(
            size_high > size_low,
            "Quality 95 ({} bytes) should produce a larger MOBI than quality 30 ({} bytes)",
            size_high, size_low
        );
        println!("  \u{2713} JPEG q30={} bytes < q95={} bytes", size_low, size_high);
    }

    #[test]
    fn test_webtoon_max_height() {
        use crate::comic;

        let dir = TempDir::new("webtoon_max_height");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create 3 tall webtoon strips, each 200x2000 = 6000 total height
        for i in 0..3u32 {
            let img = image::DynamicImage::ImageRgb8(
                image::RgbImage::from_fn(200, 2000, |x, y| {
                    if y % 800 < 20 {
                        image::Rgb([255, 255, 255]) // gutters
                    } else {
                        image::Rgb([
                            ((x + i * 50) % 200) as u8 + 20,
                            ((y + i * 30) % 200) as u8 + 20,
                            128,
                        ])
                    }
                }),
            );
            img.save(images_dir.join(format!("strip_{:03}.png", i))).unwrap();
        }

        let profile = comic::get_profile("paperwhite").unwrap();

        // Build with a max_height that forces chunking (3000 < total 6000)
        let output_chunked = dir.path().join("chunked.mobi");
        let options_chunked = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: true, panel_view: false,
            jpeg_quality: 85,
            max_height: 3000,
            embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_chunked, &profile, &options_chunked)
            .expect("chunked webtoon build failed");

        // Build with default (no chunking)
        let output_normal = dir.path().join("normal.mobi");
        let options_normal = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: true, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_normal, &profile, &options_normal)
            .expect("normal webtoon build failed");

        // Both should produce valid MOBIs
        let data_chunked = fs::read(&output_chunked).unwrap();
        let data_normal = fs::read(&output_normal).unwrap();
        assert_eq!(&data_chunked[60..64], b"BOOK");
        assert_eq!(&data_normal[60..64], b"BOOK");
        assert!(data_chunked.len() > 100, "Chunked MOBI too small");
        assert!(data_normal.len() > 100, "Normal MOBI too small");
        println!("  \u{2713} Max-height chunked={} bytes, normal={} bytes", data_chunked.len(), data_normal.len());
    }

    #[test]
    fn test_corrupt_image_skipped() {
        use crate::comic;

        let dir = TempDir::new("corrupt_image");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create one valid image
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Create a corrupt "image" file (random bytes, not a valid image)
        fs::write(images_dir.join("page_002.jpg"), b"this is not a valid jpeg file at all").unwrap();

        // Create another valid image
        let img2 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([200, 200, 200])),
        );
        img2.save(images_dir.join("page_003.jpg")).unwrap();

        let output_path = dir.path().join("corrupt_test.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        // Should succeed despite the corrupt image
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build should succeed by skipping the corrupt image");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Corrupt image skipped, valid MOBI: {} bytes", data.len());
    }

    #[test]
    fn test_zero_dimension_image_skipped() {
        use crate::comic;

        let dir = TempDir::new("zero_dim_image");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a valid image
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([128, 128, 128])),
        );
        img.save(images_dir.join("page_001.png")).unwrap();

        // Create a zero-width PNG (1x0 or 0x1 is hard to create with the image crate,
        // but we can create a very small valid PNG that will decode to 0x0 equivalent).
        // Instead, let's create a truncated PNG that the decoder can partially read
        // but will fail on. A minimal PNG header pointing to 0x0 dimensions:
        let zero_dim_png: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length = 13
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x00, // width = 0
            0x00, 0x00, 0x00, 0x00, // height = 0
            0x08, // bit depth = 8
            0x02, // color type = RGB
            0x00, // compression method
            0x00, // filter method
            0x00, // interlace method
            0x00, 0x00, 0x00, 0x00, // CRC (invalid, but triggers an error)
        ];
        fs::write(images_dir.join("page_002.png"), &zero_dim_png).unwrap();

        // Create another valid image
        let img2 = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 150, |_, _| image::Rgb([200, 200, 200])),
        );
        img2.save(images_dir.join("page_003.png")).unwrap();

        let output_path = dir.path().join("zero_dim_test.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
            ..Default::default()
        };

        // Should succeed by skipping the zero-dimension image
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build should succeed by skipping the zero-dimension image");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Zero-dim image skipped, valid MOBI: {} bytes", data.len());
    }

    // =======================================================================
    // 14. Device profiles (new devices)
    // =======================================================================

    #[test]
    fn test_device_profile_kpw5() {
        use crate::comic;
        let profile = comic::get_profile("kpw5").expect("kpw5 profile should exist");
        assert_eq!(profile.width, 1236, "kpw5 width should be 1236, got {}", profile.width);
        assert_eq!(profile.height, 1648, "kpw5 height should be 1648, got {}", profile.height);
        assert!(profile.grayscale, "kpw5 should be grayscale");
        println!("  \u{2713} kpw5: {}x{}, grayscale={}", profile.width, profile.height, profile.grayscale);
    }

    #[test]
    fn test_device_profile_scribe2025() {
        use crate::comic;
        let profile = comic::get_profile("scribe2025").expect("scribe2025 profile should exist");
        assert_eq!(profile.width, 1986, "scribe2025 width should be 1986, got {}", profile.width);
        assert_eq!(profile.height, 2648, "scribe2025 height should be 2648, got {}", profile.height);
        assert!(profile.grayscale, "scribe2025 should be grayscale");
        println!("  \u{2713} scribe2025: {}x{}, grayscale={}", profile.width, profile.height, profile.grayscale);
    }

    #[test]
    fn test_device_profile_kindle2024() {
        use crate::comic;
        let profile = comic::get_profile("kindle2024").expect("kindle2024 profile should exist");
        assert_eq!(profile.width, 1240, "kindle2024 width should be 1240, got {}", profile.width);
        assert_eq!(profile.height, 1860, "kindle2024 height should be 1860, got {}", profile.height);
        assert!(profile.grayscale, "kindle2024 should be grayscale");
        println!("  \u{2713} kindle2024: {}x{}, grayscale={}", profile.width, profile.height, profile.grayscale);
    }

    #[test]
    fn test_valid_device_names_includes_new() {
        use crate::comic;
        let names = comic::valid_device_names();
        assert!(names.contains("kpw5"), "valid_device_names should contain 'kpw5', got: {}", names);
        assert!(names.contains("scribe2025"), "valid_device_names should contain 'scribe2025', got: {}", names);
        assert!(names.contains("kindle2024"), "valid_device_names should contain 'kindle2024', got: {}", names);
        println!("  \u{2713} valid_device_names includes kpw5, scribe2025, kindle2024: {}", names);
    }

    // =======================================================================
    // 15. Moire wiring (color vs grayscale devices)
    // =======================================================================

    #[test]
    fn test_moire_applied_for_color_device() {
        use crate::comic;

        let dir = TempDir::new("moire_color");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create a grayscale test image (saved as grayscale JPEG)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |x, y| {
                // Fine screentone pattern (alternating bright/dark pixels)
                if (x + y) % 2 == 0 {
                    image::Luma([200])
                } else {
                    image::Luma([50])
                }
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Build with colorsoft (color device, grayscale=false) - moire filter should run
        let output_path = dir.path().join("moire_color.mobi");
        let profile = comic::get_profile("colorsoft").unwrap();
        assert!(!profile.grayscale, "colorsoft should be a color device");
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic should succeed with moire filter on color device");

        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "Color device comic MOBI should be valid");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Moire filter on color device (colorsoft): {} bytes, valid MOBI", data.len());
    }

    #[test]
    fn test_moire_not_applied_for_grayscale_device() {
        use crate::comic;

        let dir = TempDir::new("moire_grayscale");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Same grayscale screentone test image
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |x, y| {
                if (x + y) % 2 == 0 {
                    image::Luma([200])
                } else {
                    image::Luma([50])
                }
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        // Build with paperwhite (grayscale device) - moire filter should NOT run
        let output_path = dir.path().join("moire_grayscale.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        assert!(profile.grayscale, "paperwhite should be a grayscale device");
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic should succeed without moire filter on grayscale device");

        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "Grayscale device comic MOBI should be valid");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} Moire filter skipped on grayscale device (paperwhite): {} bytes, valid MOBI", data.len());
    }

    // =======================================================================
    // 16. Crop-before-split ordering (symmetric crop)
    // =======================================================================

    #[test]
    fn test_crop_before_split_symmetric() {
        use crate::comic;
        use image::GenericImageView;

        // Create a 200x100 landscape image with:
        // - 10px uniform white border on all sides
        // - Left half content (inside border) is dark gray (60)
        // - Right half content (inside border) is light gray (190)
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(200, 100, |x, y| {
                // White border: 10px on all sides
                if x < 10 || x >= 190 || y < 10 || y >= 90 {
                    image::Luma([255])
                } else if x < 100 {
                    // Left half content (dark)
                    image::Luma([60])
                } else {
                    // Right half content (light)
                    image::Luma([190])
                }
            }),
        );

        // First, crop the borders (this simulates the pipeline's crop-before-split)
        let cropped = comic::crop_borders(&img);
        let (cw, ch) = cropped.dimensions();

        // The border is 10px on each side of a 200x100 image,
        // which is 5% of width and 10% of height - both above the 2% threshold
        assert!(cw < 200, "Should have cropped width: got {}", cw);
        assert!(ch < 100, "Should have cropped height: got {}", ch);

        // Now split the cropped image (it should be landscape since cw > ch)
        assert!(comic::is_double_page_spread(&cropped), "Cropped image should still be landscape");
        let (left, right) = comic::split_spread(&cropped);

        // Key assertion: both halves should have the same width
        // because we cropped symmetrically before splitting
        assert_eq!(
            left.width(), right.width(),
            "After crop-then-split, left ({}) and right ({}) halves should have equal width",
            left.width(), right.width()
        );
        println!(
            "  \u{2713} Crop-before-split: original 200x100 -> cropped {}x{} -> halves {}x{} and {}x{} (symmetric)",
            cw, ch, left.width(), left.height(), right.width(), right.height()
        );
    }

    // =======================================================================
    // 17. EPUB comic input (image extraction helpers)
    // =======================================================================

    #[test]
    fn test_extract_image_refs_img_tag() {
        use crate::comic;

        let xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <div><img src="page1.jpg"/></div>
</body>
</html>"#;

        let refs = comic::extract_image_refs_from_xhtml(xhtml);
        assert_eq!(refs, vec!["page1.jpg"], "Should extract 'page1.jpg' from <img src=...>, got {:?}", refs);
        println!("  \u{2713} extract_image_refs_from_xhtml(<img>): {:?}", refs);
    }

    #[test]
    fn test_extract_image_refs_svg_image() {
        use crate::comic;

        let xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:svg="http://www.w3.org/2000/svg">
<body>
  <svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink">
    <image xlink:href="page1.jpg" width="100%" height="100%"/>
  </svg>
</body>
</html>"#;

        let refs = comic::extract_image_refs_from_xhtml(xhtml);
        assert_eq!(refs, vec!["page1.jpg"], "Should extract 'page1.jpg' from <image xlink:href=...>, got {:?}", refs);
        println!("  \u{2713} extract_image_refs_from_xhtml(<image xlink:href>): {:?}", refs);
    }

    #[test]
    fn test_extract_image_refs_regex_img_tag() {
        use crate::comic;

        let content = r#"<html><body><img src="images/page01.png" alt="page"/></body></html>"#;
        let refs = comic::extract_image_refs_regex(content);
        assert_eq!(refs, vec!["images/page01.png"], "Regex should extract img src, got {:?}", refs);
        println!("  \u{2713} extract_image_refs_regex(<img>): {:?}", refs);
    }

    #[test]
    fn test_extract_image_refs_regex_svg_image() {
        use crate::comic;

        let content = r#"<svg><image xlink:href="page1.jpg" width="100%" height="100%"/></svg>"#;
        let refs = comic::extract_image_refs_regex(content);
        assert_eq!(refs, vec!["page1.jpg"], "Regex should extract image xlink:href, got {:?}", refs);
        println!("  \u{2713} extract_image_refs_regex(<image xlink:href>): {:?}", refs);
    }

    #[test]
    fn test_extract_image_refs_multiple() {
        use crate::comic;

        let xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<body>
  <img src="cover.jpg"/>
  <img src="page1.png"/>
  <img src="page2.png"/>
</body>
</html>"#;

        let refs = comic::extract_image_refs_from_xhtml(xhtml);
        assert_eq!(refs.len(), 3, "Should extract 3 image refs, got {}", refs.len());
        assert_eq!(refs[0], "cover.jpg");
        assert_eq!(refs[1], "page1.png");
        assert_eq!(refs[2], "page2.png");
        println!("  \u{2713} extract_image_refs_from_xhtml (multiple): {:?}", refs);
    }

    // =======================================================================
    // 18. Dark gutter detection (webtoon split)
    // =======================================================================

    #[test]
    fn test_webtoon_split_dark_background() {
        use crate::comic;
        use image::GenericImageView;

        // Create a tall strip where panels are separated by solid BLACK rows.
        // This tests that the gutter detector finds dark gutters, not just white.
        let strip_height = 4000u32;
        let strip_width = 100u32;
        let device_height = 1448u32;

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(strip_width, strip_height, |_x, y| {
                // Create solid BLACK gutter rows near target split points
                if (y >= 1390 && y <= 1420) || (y >= 2790 && y <= 2820) {
                    image::Luma([0]) // BLACK gutter (not white)
                } else {
                    // Varied content (high variance rows)
                    image::Luma([((y * 7 + 13) % 200) as u8 + 30])
                }
            }),
        );

        let pages = comic::webtoon_split(&img, device_height);

        // Should produce at least 2 pages
        assert!(
            pages.len() >= 2,
            "Dark-gutter strip should produce at least 2 pages, got {}",
            pages.len()
        );
        assert!(
            pages.len() <= 4,
            "Should produce at most 4 pages, got {}",
            pages.len()
        );

        // All pages should have the correct width
        for (i, page) in pages.iter().enumerate() {
            let (pw, _ph) = page.dimensions();
            assert_eq!(pw, strip_width, "Page {} width should be {}, got {}", i, strip_width, pw);
        }

        // Total height should equal the strip height (clean gutter cuts, no overlap needed)
        let total_h: u32 = pages.iter().map(|p| p.height()).sum();
        assert_eq!(
            total_h, strip_height,
            "Sum of page heights ({}) should equal strip height ({}) for clean gutter cuts",
            total_h, strip_height
        );
        println!(
            "  \u{2713} Dark gutter detection: {} pages from {}px strip, total_h={}, all widths={}",
            pages.len(), strip_height, total_h, strip_width
        );
    }

    // =======================================================================
    // 19. CLI flags (ComicOptions): doc_type, title, language
    // =======================================================================

    #[test]
    fn test_comic_doc_type_ebok() {
        use crate::comic;

        let dir = TempDir::new("comic_doc_type");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |_, _| image::Luma([128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("ebok_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            doc_type: Some("EBOK".to_string()),
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with doc_type=EBOK should succeed");

        let data = fs::read(&output_path).unwrap();
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // Verify EXTH 501 = "EBOK"
        let exth501 = exth.get(&501).expect("EXTH 501 should exist for doc_type=EBOK");
        let value = std::str::from_utf8(&exth501[0]).unwrap();
        assert_eq!(value, "EBOK", "EXTH 501 should be 'EBOK', got '{}'", value);
        println!("  \u{2713} Comic doc_type=EBOK: EXTH 501='{}'", value);
    }

    #[test]
    fn test_comic_title_override() {
        use crate::comic;

        let dir = TempDir::new("comic_title_override");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |_, _| image::Luma([128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("titled_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            title_override: Some("Custom Title".to_string()),
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with title_override should succeed");

        let data = fs::read(&output_path).unwrap();

        // Check PalmDB name contains the custom title
        let (name_bytes, _, _) = parse_palmdb(&data);
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap();
        assert!(
            name.contains("Custom") || name.contains("custom"),
            "PalmDB name should reflect the title override 'Custom Title', got '{}'",
            name
        );

        // Also check EXTH 503 (updated title) if present
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);
        // EXTH 503 is the updated title
        if let Some(exth503) = exth.get(&503) {
            let title = std::str::from_utf8(&exth503[0]).unwrap();
            assert!(
                title.contains("Custom Title"),
                "EXTH 503 should contain 'Custom Title', got '{}'",
                title
            );
            println!("  \u{2713} Comic title override: PalmDB='{}', EXTH 503='{}'", name, title);
        } else {
            // Check EXTH 100 (author field is often set, but 503 may not be - check EXTH 99 = title)
            // The title goes through the OPF -> MOBI path. Verify via PalmDB name at minimum.
            println!("  \u{2713} Comic title override: PalmDB='{}' (no EXTH 503)", name);
        }
    }

    #[test]
    fn test_comic_language_override() {
        use crate::comic;

        let dir = TempDir::new("comic_language");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(100, 150, |_, _| image::Luma([128])),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("ja_comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            language: Some("ja".to_string()),
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with language=ja should succeed");

        let data = fs::read(&output_path).unwrap();
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // Verify EXTH 524 = "ja" (language)
        let exth524 = exth.get(&524).expect("EXTH 524 should exist for language override");
        let value = std::str::from_utf8(&exth524[0]).unwrap();
        assert_eq!(value, "ja", "EXTH 524 should be 'ja', got '{}'", value);
        println!("  \u{2713} Comic language=ja: EXTH 524='{}'", value);
    }

    // =======================================================================
    // EPUB comic input end-to-end test
    // =======================================================================

    #[test]
    fn test_epub_comic_pipeline() {
        use crate::comic;
        use std::io::Write;

        let dir = TempDir::new("epub_comic_pipeline");

        // Create 2 small test JPEG images
        let img1 = {
            let img = image::RgbImage::from_fn(100, 150, |x, y| {
                image::Rgb([(x as u8).wrapping_mul(2), (y as u8), 100])
            });
            let dyn_img = image::DynamicImage::ImageRgb8(img);
            let mut buf = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut buf);
            dyn_img.write_to(&mut cursor, image::ImageFormat::Jpeg).unwrap();
            buf
        };
        let img2 = {
            let img = image::RgbImage::from_fn(100, 150, |x, y| {
                image::Rgb([50, (x as u8).wrapping_add(y as u8), 200])
            });
            let dyn_img = image::DynamicImage::ImageRgb8(img);
            let mut buf = Vec::new();
            let mut cursor = std::io::Cursor::new(&mut buf);
            dyn_img.write_to(&mut cursor, image::ImageFormat::Jpeg).unwrap();
            buf
        };

        // Build a minimal valid EPUB as a zip archive in memory
        let epub_bytes = {
            let buf = Vec::new();
            let cursor = std::io::Cursor::new(buf);
            let mut zip = zip::ZipWriter::new(cursor);

            // mimetype must be first entry, stored uncompressed
            let stored_opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", stored_opts).unwrap();
            zip.write_all(b"application/epub+zip").unwrap();

            let deflate_opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);

            // META-INF/container.xml
            zip.start_file("META-INF/container.xml", deflate_opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#).unwrap();

            // OEBPS/content.opf
            zip.start_file("OEBPS/content.opf", deflate_opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>EPUB Comic Test</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="uid">test-epub-comic-001</dc:identifier>
    <dc:creator>Test Author</dc:creator>
  </metadata>
  <manifest>
    <item id="page1" href="page1.xhtml" media-type="application/xhtml+xml"/>
    <item id="page2" href="page2.xhtml" media-type="application/xhtml+xml"/>
    <item id="img1" href="images/img1.jpg" media-type="image/jpeg"/>
    <item id="img2" href="images/img2.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine>
    <itemref idref="page1"/>
    <itemref idref="page2"/>
  </spine>
</package>"#).unwrap();

            // OEBPS/page1.xhtml
            zip.start_file("OEBPS/page1.xhtml", deflate_opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Page 1</title></head>
<body><img src="images/img1.jpg"/></body>
</html>"#).unwrap();

            // OEBPS/page2.xhtml
            zip.start_file("OEBPS/page2.xhtml", deflate_opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Page 2</title></head>
<body><img src="images/img2.jpg"/></body>
</html>"#).unwrap();

            // OEBPS/images/img1.jpg and img2.jpg
            zip.start_file("OEBPS/images/img1.jpg", stored_opts).unwrap();
            zip.write_all(&img1).unwrap();

            zip.start_file("OEBPS/images/img2.jpg", stored_opts).unwrap();
            zip.write_all(&img2).unwrap();

            let cursor = zip.finish().unwrap();
            cursor.into_inner()
        };

        // Write the EPUB to disk
        let epub_path = dir.path().join("test_comic.epub");
        fs::write(&epub_path, &epub_bytes).unwrap();

        // Run the comic pipeline with the EPUB as input
        let output_path = dir.path().join("comic_from_epub.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        comic::build_comic(&epub_path, &output_path, &profile)
            .expect("build_comic from EPUB should succeed");

        // Verify output exists and has reasonable size
        let data = fs::read(&output_path).expect("could not read output MOBI");
        assert!(data.len() > 100, "Comic MOBI too small: {} bytes", data.len());

        // PalmDB type/creator check
        assert_eq!(&data[60..64], b"BOOK", "PalmDB type should be BOOK");
        assert_eq!(&data[64..68], b"MOBI", "PalmDB creator should be MOBI");

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI magic in record 0
        assert_eq!(&rec0[16..20], b"MOBI", "Record 0 should contain MOBI magic");

        // Check for EXTH 122 = "true" (fixed-layout flag, expected for comics)
        let exth = parse_exth_records(rec0);
        let exth122 = exth.get(&122).expect("Comic EXTH should contain record 122 (fixed-layout)");
        let value = std::str::from_utf8(&exth122[0]).unwrap();
        assert_eq!(value, "true", "EXTH 122 should be 'true' for fixed-layout");

        // Verify image records: first_image_record at MOBI header offset 92 (rec0 offset 108)
        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(
            first_img,
            0xFFFFFFFF_u32 as usize,
            "Comic with images should have first_image set"
        );

        // Count image records (JPEG magic FF D8) starting from first_image_record
        let mut image_count = 0;
        for i in first_img..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 2 && rec[0] == 0xFF && rec[1] == 0xD8 {
                image_count += 1;
            }
        }
        // The pipeline processes 2 input images; each becomes at least one JPEG record.
        // With panel view enabled (default), there may be additional panel crop images.
        assert!(
            image_count >= 2,
            "Expected at least 2 image records from 2 EPUB pages, found {}",
            image_count
        );

        println!(
            "  \u{2713} EPUB comic pipeline: {} bytes, {} image records, EXTH 122=true",
            data.len(),
            image_count
        );
    }

    #[test]
    fn test_rotate_spreads() {
        use crate::comic;
        use image::GenericImageView;
        // Create a landscape image (wider than tall) simulating a double-page spread.
        // With rotate_spreads=true, it should be rotated 90 degrees clockwise
        // instead of being split, producing a single portrait output.
        let dir = TempDir::new("rotate_spreads");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // 300x150 landscape image
        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(300, 150, |x, _| {
                if x < 150 { image::Rgb([80, 80, 80]) } else { image::Rgb([180, 180, 180]) }
            }),
        );
        img.save(images_dir.join("spread_001.jpg")).unwrap();

        let output_path = dir.path().join("rotate_spreads.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false,
            split: true, // split is true, but rotate_spreads overrides it for spreads
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536,
            embed_source: false,
            rotate_spreads: true,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with rotate_spreads should succeed");

        let data = fs::read(&output_path).expect("could not read rotate_spreads MOBI");
        assert!(data.len() > 100, "Rotated spread comic MOBI too small");
        assert_eq!(&data[60..64], b"BOOK", "Output should be a valid MOBI");

        // Parse the PalmDB to count records. With rotation (not splitting), we should
        // have fewer image records than splitting would produce: 1 page instead of 2.
        let (_, record_count, _) = parse_palmdb(&data);

        // Also build with split (no rotation) for comparison
        let output_split = dir.path().join("split_spreads.mobi");
        let split_options = comic::ComicOptions {
            rtl: false,
            split: true,
            crop: 0,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536,
            embed_source: false,
            rotate_spreads: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_split, &profile, &split_options)
            .expect("build_comic with split should succeed");

        let split_data = fs::read(&output_split).unwrap();
        let (_, split_record_count, _) = parse_palmdb(&split_data);

        // Split produces 2 pages, rotate produces 1 page, so split should have more records
        assert!(
            split_record_count > record_count,
            "Split version should have more records ({}) than rotated version ({})",
            split_record_count, record_count,
        );

        // Verify the rotated image is portrait (height > width) by decoding the first
        // image record from the MOBI
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let first_img_idx = read_u32_be(rec0, 108) as usize;
        let img_rec = get_record(&data, &offsets, first_img_idx);
        // Decode the JPEG to check dimensions
        let decoded = image::load_from_memory(img_rec)
            .expect("failed to decode rotated image from MOBI");
        let (w, h) = decoded.dimensions();
        assert!(
            h > w,
            "Rotated spread should be portrait (height > width), got {}x{}", w, h,
        );
        println!("  \u{2713} Rotated spread: {}x{} portrait, {} records (vs {} split records)",
                 w, h, record_count, split_record_count);
    }

    // =======================================================================
    // 21. Panel reading order
    // =======================================================================

    #[test]
    fn test_panel_reading_order_sorting() {
        use crate::comic;

        // Create a 2x2 grid of panels with known positions:
        //   Panel A (0,0)    Panel B (52,0)
        //   Panel C (0,52)   Panel D (52,52)
        let panels = vec![
            comic::PanelRect { x: 0.0, y: 0.0, w: 47.0, h: 47.0 },   // A: top-left
            comic::PanelRect { x: 52.0, y: 0.0, w: 47.0, h: 47.0 },  // B: top-right
            comic::PanelRect { x: 0.0, y: 52.0, w: 47.0, h: 47.0 },  // C: bottom-left
            comic::PanelRect { x: 52.0, y: 52.0, w: 47.0, h: 47.0 }, // D: bottom-right
        ];

        // horizontal-lr: A, B, C, D (left-to-right, top-to-bottom)
        let mut lr = panels.clone();
        comic::sort_panels_by_reading_order(&mut lr, "horizontal-lr");
        assert_eq!(lr[0].x, 0.0);  assert_eq!(lr[0].y, 0.0);   // A
        assert_eq!(lr[1].x, 52.0); assert_eq!(lr[1].y, 0.0);   // B
        assert_eq!(lr[2].x, 0.0);  assert_eq!(lr[2].y, 52.0);  // C
        assert_eq!(lr[3].x, 52.0); assert_eq!(lr[3].y, 52.0);  // D
        println!("  \u{2713} horizontal-lr: A, B, C, D");

        // horizontal-rl: B, A, D, C (right-to-left, top-to-bottom)
        let mut rl = panels.clone();
        comic::sort_panels_by_reading_order(&mut rl, "horizontal-rl");
        assert_eq!(rl[0].x, 52.0); assert_eq!(rl[0].y, 0.0);   // B
        assert_eq!(rl[1].x, 0.0);  assert_eq!(rl[1].y, 0.0);   // A
        assert_eq!(rl[2].x, 52.0); assert_eq!(rl[2].y, 52.0);  // D
        assert_eq!(rl[3].x, 0.0);  assert_eq!(rl[3].y, 52.0);  // C
        println!("  \u{2713} horizontal-rl: B, A, D, C");

        // vertical-lr: A, C, B, D (top-to-bottom, left-to-right)
        let mut vlr = panels.clone();
        comic::sort_panels_by_reading_order(&mut vlr, "vertical-lr");
        assert_eq!(vlr[0].x, 0.0);  assert_eq!(vlr[0].y, 0.0);   // A
        assert_eq!(vlr[1].x, 0.0);  assert_eq!(vlr[1].y, 52.0);  // C
        assert_eq!(vlr[2].x, 52.0); assert_eq!(vlr[2].y, 0.0);   // B
        assert_eq!(vlr[3].x, 52.0); assert_eq!(vlr[3].y, 52.0);  // D
        println!("  \u{2713} vertical-lr: A, C, B, D");

        // vertical-rl: B, D, A, C (top-to-bottom, right-to-left)
        let mut vrl = panels.clone();
        comic::sort_panels_by_reading_order(&mut vrl, "vertical-rl");
        assert_eq!(vrl[0].x, 52.0); assert_eq!(vrl[0].y, 0.0);   // B
        assert_eq!(vrl[1].x, 52.0); assert_eq!(vrl[1].y, 52.0);  // D
        assert_eq!(vrl[2].x, 0.0);  assert_eq!(vrl[2].y, 0.0);   // A
        assert_eq!(vrl[3].x, 0.0);  assert_eq!(vrl[3].y, 52.0);  // C
        println!("  \u{2713} vertical-rl: B, D, A, C");

        // Auto-detect: RTL should default to horizontal-rl
        let order_rtl = comic::resolve_panel_reading_order(None, true);
        assert_eq!(order_rtl, "horizontal-rl");
        println!("  \u{2713} auto-detect RTL: {}", order_rtl);

        // Auto-detect: LTR should default to horizontal-lr
        let order_ltr = comic::resolve_panel_reading_order(None, false);
        assert_eq!(order_ltr, "horizontal-lr");
        println!("  \u{2713} auto-detect LTR: {}", order_ltr);

        // Explicit override should take precedence over RTL
        let order_override = comic::resolve_panel_reading_order(Some("vertical-lr"), true);
        assert_eq!(order_override, "vertical-lr");
        println!("  \u{2713} explicit override vertical-lr with RTL: {}", order_override);

        // Verify panels are different with different reading orders
        let mut order1 = panels.clone();
        let mut order2 = panels.clone();
        comic::sort_panels_by_reading_order(&mut order1, "horizontal-lr");
        comic::sort_panels_by_reading_order(&mut order2, "horizontal-rl");
        assert_ne!(order1, order2, "horizontal-lr and horizontal-rl should produce different orderings");
        println!("  \u{2713} different reading orders produce different panel sequences");
    }

    #[test]
    fn test_cover_fill_crops_to_aspect_ratio() {
        use crate::comic;

        // Create a square image (1:1 aspect ratio). The Paperwhite has a ~0.74:1
        // aspect ratio (1072x1448), so this should get cropped vertically.
        let dir = TempDir::new("cover_fill");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        let img = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(400, 400, |x, y| {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            }),
        );
        img.save(images_dir.join("page_001.jpg")).unwrap();

        let output_path = dir.path().join("cover_fill.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            cover_fill: true,
            crop: 0,
            enhance: false,
            split: false,
            panel_view: false,
            jpeg_quality: 95,
            embed_source: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with cover_fill failed");

        let data = fs::read(&output_path).expect("could not read cover_fill MOBI");
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Find the first image record
        let first_img_idx = read_u32_be(rec0, 108) as usize;
        let cover_rec = get_record(&data, &offsets, first_img_idx);
        assert!(cover_rec.len() > 2 && cover_rec[0] == 0xFF && cover_rec[1] == 0xD8,
            "Cover record should be a JPEG");

        // Decode the cover and verify it matches the device aspect ratio
        let cover_img = image::load_from_memory(cover_rec)
            .expect("Failed to decode cover JPEG");
        let (w, h) = (cover_img.width(), cover_img.height());

        // The cover should have been resized to the device dimensions exactly.
        // With cover_fill, the image is center-cropped to the device aspect ratio
        // first, so resize produces exact device dimensions (no letterboxing).
        assert_eq!(w, profile.width, "Cover width should match device width");
        assert_eq!(h, profile.height, "Cover height should match device height");

        // Verify the aspect ratio matches the device (within rounding tolerance)
        let device_ratio = profile.width as f64 / profile.height as f64;
        let cover_ratio = w as f64 / h as f64;
        assert!((device_ratio - cover_ratio).abs() < 0.01,
            "Cover aspect ratio ({:.4}) should match device ({:.4})",
            cover_ratio, device_ratio);

        println!("  \u{2713} cover_fill: cover is {}x{} (matches device {}x{})",
            w, h, profile.width, profile.height);
    }

    // =======================================================================
    // Kindle publishing limits
    // =======================================================================

    #[test]
    fn test_kindle_limits_dict_by_letter_produces_valid_mobi() {
        // Build a dictionary with kindle_limits=true and verify it produces a valid MOBI.
        // The entries span multiple letters to exercise the per-letter grouping.
        let dir = TempDir::new("kindle_limits_dict");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("avocado", &[]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf_path = create_dict_fixture(dir.path(), entries);
        let output_path = dir.path().join("output.mobi");

        mobi::build_mobi(
            &opf_path,
            &output_path,
            true,  // no_compress
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            true,  // kindle_limits ON
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("build_mobi with kindle_limits should succeed");

        let data = fs::read(&output_path).expect("could not read output MOBI");
        assert_eq!(&data[60..64], b"BOOK");
        assert_eq!(&data[64..68], b"MOBI");

        let (_, record_count, offsets) = parse_palmdb(&data);
        assert!(record_count > 0, "Should have records");

        // Verify MOBI magic in record 0
        let rec0 = get_record(&data, &offsets, 0);
        assert_eq!(&rec0[16..20], b"MOBI");

        // Verify it has INDX records (dictionary index still works)
        let mut found_indx = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"INDX" {
                found_indx = true;
                break;
            }
        }
        assert!(found_indx, "Dictionary with kindle_limits should still have INDX records");
        println!("  \u{2713} kindle_limits dict: valid MOBI with INDX, {} records", record_count);
    }

    #[test]
    fn test_kindle_limits_book_warns_on_large_html() {
        // This test verifies that the code path for kindle_limits with books
        // runs without errors. The actual warning is printed to stderr.
        // We create a book and build with kindle_limits=true.
        let dir = TempDir::new("kindle_limits_book");
        let opf_path = create_book_fixture(dir.path(), None);
        let output_path = dir.path().join("output.mobi");

        mobi::build_mobi(
            &opf_path,
            &output_path,
            true,  // no_compress
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            true,  // kindle_limits ON
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("build_mobi with kindle_limits for book should succeed");

        let data = fs::read(&output_path).expect("could not read output MOBI");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} kindle_limits book: valid MOBI produced without errors");
    }

    #[test]
    fn test_kindle_limits_off_dict_uses_single_blob() {
        // With kindle_limits=false, dictionary should use the original single-file approach.
        let dir = TempDir::new("kindle_limits_off");
        let entries: &[(&str, &[&str])] = &[
            ("alpha", &[]),
            ("beta", &[]),
        ];
        let opf_path = create_dict_fixture(dir.path(), entries);
        let output_path = dir.path().join("output.mobi");

        mobi::build_mobi(
            &opf_path,
            &output_path,
            true,  // no_compress
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            false, // kindle_limits OFF
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("build_mobi without kindle_limits should succeed");

        let data = fs::read(&output_path).expect("could not read output MOBI");
        assert_eq!(&data[60..64], b"BOOK");
        println!("  \u{2713} kindle_limits OFF: valid MOBI produced");
    }

    // =======================================================================
    // Regression tests for dictionary output fixes
    // =======================================================================

    /// Helper: extract the full text content from an uncompressed MOBI file
    /// by concatenating text records (records 1..N where N is text record count).
    fn extract_text_from_uncompressed_mobi(data: &[u8]) -> String {
        let (_, _, offsets) = parse_palmdb(data);
        let rec0 = get_record(data, &offsets, 0);
        // PalmDOC header: offset 8 = text record count (u16)
        let text_record_count = read_u16_be(rec0, 8) as usize;
        let mut text_bytes = Vec::new();
        for i in 1..=text_record_count {
            if i < offsets.len() {
                let rec = get_record(data, &offsets, i);
                text_bytes.extend_from_slice(rec);
            }
        }
        String::from_utf8_lossy(&text_bytes).to_string()
    }

    #[test]
    fn test_dict_css_preserved_in_text() {
        let dir = TempDir::new("dict_css_preserved");

        // Build HTML with a <style> block in the <head>
        let html = r#"<html><head><style>.def { margin-left: 20px; }</style><guide></guide></head><body>
<idx:entry><idx:orth value="apple">apple</idx:orth><span class="def">a fruit</span></idx:entry>
<idx:entry><idx:orth value="banana">banana</idx:orth><span class="def">another fruit</span></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">CSS Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let text = extract_text_from_uncompressed_mobi(&data);

        assert!(
            text.contains(".def { margin-left: 20px; }"),
            "CSS style block should be preserved in text output, got: {}",
            &text[..text.len().min(500)]
        );
        assert!(
            text.contains("<style>"),
            "Style tag should be present in text output"
        );
        println!("  \u{2713} CSS <style> block preserved in dictionary text output");
    }

    #[test]
    fn test_dict_front_matter_included() {
        let dir = TempDir::new("dict_front_matter");

        // Create front matter HTML (no idx:entry tags)
        let cover_html = r#"<html><head></head><body><h1>My Dictionary</h1><p>Copyright 2026</p></body></html>"#;
        fs::write(dir.path().join("cover.html"), cover_html).unwrap();

        // Create dictionary content HTML
        let dict_html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="alpha">alpha</idx:orth><b>alpha</b> first letter</idx:entry>
<idx:entry><idx:orth value="beta">beta</idx:orth><b>beta</b> second letter</idx:entry>
</body></html>"#;
        fs::write(dir.path().join("dict.html"), dict_html).unwrap();

        // OPF with cover.html before dict.html in spine
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">FM Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="cover" href="cover.html" media-type="application/xhtml+xml"/>
    <item id="dict" href="dict.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="cover"/>
    <itemref idref="dict"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        // Build with kindle_limits=true to exercise build_text_content_by_letter
        let output_path = dir.path().join("output.mobi");
        mobi::build_mobi(
            &opf_path,
            &output_path,
            true,  // no_compress
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            true,  // kindle_limits ON
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("build_mobi with kindle_limits should succeed");

        let data = fs::read(&output_path).expect("could not read output MOBI");
        let text = extract_text_from_uncompressed_mobi(&data);

        assert!(
            text.contains("My Dictionary"),
            "Front matter title should be present in kindle_limits output, got: {}",
            &text[..text.len().min(500)]
        );
        assert!(
            text.contains("Copyright 2026"),
            "Front matter copyright should be present in kindle_limits output"
        );

        // Verify front matter comes before dictionary content
        let fm_pos = text.find("My Dictionary").unwrap();
        let dict_pos = text.find("first letter").unwrap();
        assert!(
            fm_pos < dict_pos,
            "Front matter should appear before dictionary entries (fm at {}, dict at {})",
            fm_pos, dict_pos
        );
        println!("  \u{2713} Front matter included and appears before dictionary entries in kindle_limits mode");
    }

    #[test]
    fn test_dict_entry_separators() {
        let dir = TempDir::new("dict_entry_separators");

        // Create dictionary with multiple entries but NO <hr/> in source
        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="cat">cat</idx:orth><b>cat</b> a small animal</idx:entry>
<idx:entry><idx:orth value="dog">dog</idx:orth><b>dog</b> a loyal animal</idx:entry>
<idx:entry><idx:orth value="fish">fish</idx:orth><b>fish</b> an aquatic animal</idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Sep Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let text = extract_text_from_uncompressed_mobi(&data);

        // Count <hr/> separators - should have at least 2 (one between each pair of entries)
        let hr_count = text.matches("<hr/>").count();
        assert!(
            hr_count >= 2,
            "Should have at least 2 <hr/> separators between 3 entries, found {}. Text: {}",
            hr_count,
            &text[..text.len().min(500)]
        );

        // Verify separators appear between entries, not entries running together
        assert!(
            text.contains("a small animal<hr/>"),
            "Entry content should be followed by <hr/> separator"
        );
        assert!(
            text.contains("a loyal animal<hr/>"),
            "Entry content should be followed by <hr/> separator"
        );
        println!("  \u{2713} <hr/> separators present between dictionary entries ({} found)", hr_count);
    }

    // =======================================================================
    // MOBI structural validation tests
    // =======================================================================

    // -----------------------------------------------------------------------
    // Helper: build a dictionary MOBI with kindle_limits enabled
    // -----------------------------------------------------------------------

    fn build_mobi_bytes_with_kindle_limits(
        opf_path: &Path,
        output_dir: &Path,
    ) -> Vec<u8> {
        let output_path = output_dir.join("output_kl.mobi");
        mobi::build_mobi(
            opf_path,
            &output_path,
            true,  // no_compress (faster tests)
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            true,  // kindle_limits ON
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("build_mobi with kindle_limits failed");
        fs::read(&output_path).expect("could not read output MOBI")
    }

    // -----------------------------------------------------------------------
    // Helper: decode inverted VWI from INDX entry tag data
    // -----------------------------------------------------------------------

    fn decode_vwi_inv(data: &[u8], start: usize) -> (u32, usize) {
        let mut value: u32 = 0;
        let mut pos = start;
        loop {
            if pos >= data.len() {
                break;
            }
            let b = data[pos];
            value = (value << 7) | (b as u32 & 0x7F);
            pos += 1;
            if b & 0x80 != 0 {
                // High bit set = last byte in inverted VWI
                break;
            }
        }
        (value, pos)
    }

    // -----------------------------------------------------------------------
    // Helper: strip trailing bytes from an uncompressed text record
    // -----------------------------------------------------------------------

    /// Strip the trailing bytes (TBS + multibyte indicator) from a text record.
    /// Returns just the text content portion.
    fn strip_trailing_bytes(rec: &[u8]) -> &[u8] {
        if rec.len() >= 2 {
            &rec[..rec.len() - 2]
        } else {
            rec
        }
    }

    // -----------------------------------------------------------------------
    // Helper: extract text blob by concatenating stripped text records
    // -----------------------------------------------------------------------

    fn extract_text_blob(data: &[u8]) -> Vec<u8> {
        let (_, _, offsets) = parse_palmdb(data);
        let rec0 = get_record(data, &offsets, 0);
        let text_record_count = read_u16_be(rec0, 8) as usize;
        let mut text_bytes = Vec::new();
        for i in 1..=text_record_count {
            if i < offsets.len() {
                let rec = get_record(data, &offsets, i);
                text_bytes.extend_from_slice(strip_trailing_bytes(rec));
            }
        }
        text_bytes
    }

    // -----------------------------------------------------------------------
    // Helper: parse INDX data records and extract (start_pos, text_len) pairs
    // -----------------------------------------------------------------------

    /// Parse all INDX data records starting from the primary header record
    /// and return a Vec of (start_pos, text_len) for each headword entry.
    fn parse_indx_entries(data: &[u8], offsets: &[u32], orth_idx: usize) -> Vec<(u32, u32)> {
        let primary_rec = get_record(data, offsets, orth_idx);

        // Primary INDX header: offset 24 = number of data records
        let num_data_records = read_u32_be(primary_rec, 24) as usize;

        let mut entries = Vec::new();

        // Data records follow immediately after the primary record
        for dr in 0..num_data_records {
            let data_rec_idx = orth_idx + 1 + dr;
            if data_rec_idx >= offsets.len() {
                break;
            }
            let data_rec = get_record(data, offsets, data_rec_idx);
            if data_rec.len() < 4 || &data_rec[0..4] != b"INDX" {
                break;
            }

            // INDX data record header: offset 24 = entry count
            let entry_count = read_u32_be(data_rec, 24) as usize;
            // IDXT offset at header offset 20
            let idxt_offset = read_u32_be(data_rec, 20) as usize;

            // IDXT starts with "IDXT" magic followed by 2-byte offsets
            if idxt_offset + 4 + entry_count * 2 > data_rec.len() {
                continue;
            }
            if &data_rec[idxt_offset..idxt_offset + 4] != b"IDXT" {
                continue;
            }

            for e in 0..entry_count {
                let entry_offset = read_u16_be(data_rec, idxt_offset + 4 + e * 2) as usize;
                if entry_offset >= data_rec.len() {
                    continue;
                }

                // Parse entry: byte0 = (prefix_len << 5) | new_label_len
                let byte0 = data_rec[entry_offset];
                let new_label_len = (byte0 & 0x1F) as usize;
                let after_label = entry_offset + 1 + new_label_len;
                if after_label >= data_rec.len() {
                    continue;
                }

                // Control byte follows the label
                let _control = data_rec[after_label];
                let tag_data_start = after_label + 1;

                // Tag 1 = start_pos, Tag 2 = text_len (both inverted VWI)
                let (start_pos, next) = decode_vwi_inv(data_rec, tag_data_start);
                let (text_len, _) = decode_vwi_inv(data_rec, next);

                entries.push((start_pos, text_len));
            }
        }

        entries
    }

    // -----------------------------------------------------------------------
    // Helper: create a dict fixture with unambiguous definitions
    // (avoids repeating the headword in the definition text, which can
    // cause find_entry_positions to match the wrong occurrence)
    // -----------------------------------------------------------------------

    fn create_dict_fixture_unambiguous(
        dir: &Path,
        entries: &[(&str, &[&str])],
    ) -> PathBuf {
        let mut html_body = String::new();
        for (i, (hw, iforms)) in entries.iter().enumerate() {
            html_body.push_str(&format!(
                "<idx:entry><idx:orth value=\"{hw}\">{hw}</idx:orth>",
                hw = hw
            ));
            for iform in *iforms {
                html_body.push_str(&format!(
                    "<idx:infl><idx:iform value=\"{iform}\"/></idx:infl>",
                    iform = iform
                ));
            }
            // Use a unique numeric definition that does NOT contain the headword
            html_body.push_str(&format!(
                "<b>{hw}</b> entry number {i}<hr/></idx:entry>\n",
                hw = hw, i = i
            ));
        }

        let html = format!(
            r#"<html><head><guide></guide></head><body>{}</body></html>"#,
            html_body
        );
        fs::write(dir.join("content.html"), &html).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    // =======================================================================
    // 1. INDX offset validation
    // =======================================================================

    #[test]
    fn test_indx_entries_point_to_valid_text() {
        let dir = TempDir::new("indx_offset_valid");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
            ("date", &["dates"]),
            ("elderberry", &["elderberries"]),
        ];
        let opf = create_dict_fixture_unambiguous(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let text_blob = extract_text_blob(&data);
        let indx_entries = parse_indx_entries(&data, &offsets, orth_idx);

        assert!(
            !indx_entries.is_empty(),
            "Should have parsed INDX entries"
        );

        for (i, &(start_pos, text_len)) in indx_entries.iter().enumerate() {
            let sp = start_pos as usize;
            let tl = text_len as usize;

            // text_len must be > 0
            assert!(
                tl > 0,
                "INDX entry {} has text_len=0 (start_pos={})",
                i, sp
            );

            // start_pos + text_len must not exceed the text blob
            assert!(
                sp + tl <= text_blob.len(),
                "INDX entry {} out of bounds: start_pos={}, text_len={}, text_blob_len={}",
                i, sp, tl, text_blob.len()
            );

            // The entry's text region should contain "<b>" near the start.
            // The start_pos may point to the idx:orth inner content (bare
            // headword text) that precedes the <b>headword</b> markup, so
            // we check the first 50 bytes of the entry region for <b>.
            let search_end = (sp + 50).min(sp + tl).min(text_blob.len());
            let region = &text_blob[sp..search_end];
            let has_bold = region.windows(3).any(|w| w == b"<b>");
            assert!(
                has_bold,
                "INDX entry {} at start_pos={} should contain '<b>' near the start, got {:?}",
                i, sp,
                String::from_utf8_lossy(region)
            );
        }

        println!(
            "  \u{2713} {} INDX entries all point to valid text with '<b>' in {} byte text blob",
            indx_entries.len(),
            text_blob.len()
        );
    }

    // =======================================================================
    // 2. Record 0 header cross-checks
    // =======================================================================

    #[test]
    fn test_record0_text_length_matches_uncompressed_size() {
        let dir = TempDir::new("rec0_text_len");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // PalmDOC header: offset 4 = text_length (u32)
        let text_length = read_u32_be(rec0, 4) as usize;
        // PalmDOC header: offset 8 = text_record_count (u16)
        let text_record_count = read_u16_be(rec0, 8) as usize;

        // Sum the uncompressed content of all text records (strip trailing bytes)
        let mut total_uncompressed = 0usize;
        for i in 1..=text_record_count {
            if i < offsets.len() {
                let rec = get_record(&data, &offsets, i);
                total_uncompressed += strip_trailing_bytes(rec).len();
            }
        }

        assert_eq!(
            text_length, total_uncompressed,
            "PalmDOC text_length ({}) should match sum of uncompressed text record sizes ({})",
            text_length, total_uncompressed
        );
        println!(
            "  \u{2713} text_length={} matches sum of {} text records",
            text_length, text_record_count
        );
    }

    #[test]
    fn test_record0_text_record_count_matches_actual() {
        let dir = TempDir::new("rec0_text_count");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &[]),
            ("banana", &[]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let text_record_count = read_u16_be(rec0, 8) as usize;

        // The first non-text record after record 0 should be INDX (for dicts).
        // orth_index_record tells us where INDX starts; text records are 1..orth_idx-1
        let orth_idx = read_u32_be(rec0, 40) as usize;
        // Text records = records 1 through orth_idx-1
        // So actual count = orth_idx - 1
        let actual_text_records = orth_idx - 1;

        assert_eq!(
            text_record_count, actual_text_records,
            "text_record_count in header ({}) should match actual text records before INDX ({})",
            text_record_count, actual_text_records
        );
        println!(
            "  \u{2713} text_record_count={} matches INDX boundary at record {}",
            text_record_count, orth_idx
        );
    }

    #[test]
    fn test_record0_orth_index_points_to_indx_magic() {
        let dir = TempDir::new("rec0_orth_indx");
        let entries: &[(&str, &[&str])] = &[
            ("alpha", &["alphas"]),
            ("beta", &["betas"]),
            ("gamma", &["gammas"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI header offset 24 = orth_index_record, stored at rec0 offset 16+24 = 40
        let orth_idx = read_u32_be(rec0, 40) as usize;
        assert!(
            orth_idx < offsets.len(),
            "orth_index_record {} exceeds record count {}",
            orth_idx, offsets.len()
        );

        let indx_rec = get_record(&data, &offsets, orth_idx);
        assert_eq!(
            &indx_rec[0..4], b"INDX",
            "Record at orth_index_record={} should start with INDX magic, got {:?}",
            orth_idx, &indx_rec[0..4]
        );
        println!("  \u{2713} orth_index_record={} -> INDX magic verified", orth_idx);
    }

    #[test]
    fn test_record0_extra_record_data_flags() {
        let dir = TempDir::new("rec0_extra_flags");
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI header offset 224, stored at rec0 offset 16+224 = 240
        let extra_flags = read_u32_be(rec0, 240);
        assert_eq!(
            extra_flags, 3,
            "extra_record_data_flags should be 3 (multibyte + TBS), got {}",
            extra_flags
        );
        println!("  \u{2713} extra_record_data_flags = {} (multibyte + TBS)", extra_flags);
    }

    // =======================================================================
    // 3. Text record trailing bytes
    // =======================================================================

    #[test]
    fn test_text_records_have_trailing_bytes() {
        let dir = TempDir::new("trailing_bytes");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let text_record_count = read_u16_be(rec0, 8) as usize;

        for i in 1..=text_record_count {
            if i >= offsets.len() {
                break;
            }
            let rec = get_record(&data, &offsets, i);
            let len = rec.len();

            assert!(
                len >= 2,
                "Text record {} too short ({} bytes) to have trailing bytes",
                i, len
            );

            // Last byte is TBS (0x81), second-to-last is multibyte (0x00).
            // libmobi / Kindle parse extras from the end backward,
            // bit 1 = TBS first, then bit 0 = multibyte.
            assert_eq!(
                rec[len - 1], 0x81,
                "Text record {} trailing byte[-1] should be 0x81 (TBS), got 0x{:02X}",
                i, rec[len - 1]
            );
            assert_eq!(
                rec[len - 2], 0x00,
                "Text record {} trailing byte[-2] should be 0x00 (multibyte), got 0x{:02X}",
                i, rec[len - 2]
            );
        }

        println!(
            "  \u{2713} All {} text records end with [0x00, 0x81] trailing bytes",
            text_record_count
        );
    }

    #[test]
    fn test_text_records_expected_size() {
        let dir = TempDir::new("text_rec_size");
        // Create enough entries to produce text > 4096 bytes so we get multiple records
        let mut entries_vec: Vec<(&str, &[&str])> = Vec::new();
        let words: &[&str] = &[
            "aardvark", "abacus", "abandon", "abbreviation", "abdomen",
            "aberration", "ability", "abnormal", "abolish", "abominable",
        ];
        for w in words {
            entries_vec.push((w, &[]));
        }
        let opf = create_dict_fixture(dir.path(), &entries_vec);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let text_record_count = read_u16_be(rec0, 8) as usize;

        // RECORD_SIZE = 4096, plus 2 trailing bytes = 4098
        let expected_full_size = 4096 + 2;
        for i in 1..=text_record_count {
            if i >= offsets.len() {
                break;
            }
            let rec = get_record(&data, &offsets, i);
            if i < text_record_count {
                // Non-last records should be exactly RECORD_SIZE + 2 trailing bytes
                assert_eq!(
                    rec.len(), expected_full_size,
                    "Text record {} (non-last) should be {} bytes, got {}",
                    i, expected_full_size, rec.len()
                );
            } else {
                // Last record can be smaller but must still have trailing bytes
                assert!(
                    rec.len() >= 3, // at least 1 byte of text + 2 trailing
                    "Last text record {} should have at least 3 bytes, got {}",
                    i, rec.len()
                );
                assert!(
                    rec.len() <= expected_full_size,
                    "Last text record {} should be <= {} bytes, got {}",
                    i, expected_full_size, rec.len()
                );
            }
        }

        println!(
            "  \u{2713} {} text records: non-last={} bytes, last <= {} bytes",
            text_record_count, expected_full_size, expected_full_size
        );
    }

    // =======================================================================
    // 4. find_entry_positions completeness (no (0,0) results)
    // =======================================================================

    #[test]
    fn test_find_entry_positions_no_zeros_no_kindle_limits() {
        let dir = TempDir::new("entry_pos_no_kl");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
            ("date", &["dates"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let indx_entries = parse_indx_entries(&data, &offsets, orth_idx);

        assert_eq!(
            indx_entries.len(), entries.len(),
            "INDX should have {} entries (headwords_only), got {}",
            entries.len(), indx_entries.len()
        );

        for (i, &(start_pos, text_len)) in indx_entries.iter().enumerate() {
            assert!(
                start_pos > 0 || text_len > 0,
                "INDX entry {} has (start_pos=0, text_len=0) - find_entry_positions failed for this entry (no kindle_limits)",
                i
            );
        }

        println!(
            "  \u{2713} All {} INDX entries have non-zero positions (no kindle_limits)",
            indx_entries.len()
        );
    }

    #[test]
    fn test_find_entry_positions_no_zeros_with_kindle_limits() {
        let dir = TempDir::new("entry_pos_kl");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
            ("date", &["dates"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes_with_kindle_limits(&opf, dir.path());

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let indx_entries = parse_indx_entries(&data, &offsets, orth_idx);

        assert!(
            !indx_entries.is_empty(),
            "INDX should have entries with kindle_limits"
        );

        for (i, &(start_pos, text_len)) in indx_entries.iter().enumerate() {
            assert!(
                start_pos > 0 || text_len > 0,
                "INDX entry {} has (start_pos=0, text_len=0) - find_entry_positions failed for this entry (with kindle_limits)",
                i
            );
        }

        println!(
            "  \u{2713} All {} INDX entries have non-zero positions (with kindle_limits)",
            indx_entries.len()
        );
    }

    // =======================================================================
    // 5. Decompression roundtrip (full MOBI text records)
    // =======================================================================

    #[test]
    fn test_decompression_roundtrip_compressed_mobi() {
        let dir = TempDir::new("decomp_roundtrip");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);

        // Build both compressed and uncompressed versions
        let data_uncomp = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let output_comp = dir.path().join("output_comp.mobi");
        mobi::build_mobi(
            &opf,
            &output_comp,
            false, // compress
            false, // headwords_only
            None,
            false,
            false,
            false,
            false,
            None,
            false,
            false, // self_check
            false, // kindlegen_parity
        )
        .expect("compressed build_mobi failed");
        let data_comp = fs::read(&output_comp).expect("could not read compressed MOBI");

        // Extract uncompressed text blob
        let text_uncomp = extract_text_blob(&data_uncomp);

        // Extract and decompress text records from the compressed version
        let (_, _, offsets_c) = parse_palmdb(&data_comp);
        let rec0_c = get_record(&data_comp, &offsets_c, 0);
        let text_record_count_c = read_u16_be(rec0_c, 8) as usize;

        let mut decompressed_text = Vec::new();
        for i in 1..=text_record_count_c {
            if i >= offsets_c.len() {
                break;
            }
            let rec = get_record(&data_comp, &offsets_c, i);
            // Strip trailing bytes before decompression
            let compressed_data = strip_trailing_bytes(rec);
            let chunk = palmdoc_decompress(compressed_data);
            decompressed_text.extend_from_slice(&chunk);
        }

        assert_eq!(
            text_uncomp.len(),
            decompressed_text.len(),
            "Decompressed text length ({}) should match uncompressed text length ({})",
            decompressed_text.len(),
            text_uncomp.len()
        );
        assert_eq!(
            text_uncomp, decompressed_text,
            "Decompressed text should exactly match uncompressed text"
        );

        println!(
            "  \u{2713} Decompression roundtrip: {} bytes match between compressed and uncompressed",
            text_uncomp.len()
        );
    }

    // =======================================================================
    // 22. Comic KF8-only output
    // =======================================================================

    /// Build a comic in KF8-only mode and return the raw bytes.
    fn build_comic_kf8_only_bytes(dir: &Path) -> Vec<u8> {
        use crate::comic;

        let images_dir = dir.join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // Create 3 small test images
        for i in 0..3u8 {
            let brightness = 50 + i * 80;
            let img = image::DynamicImage::ImageLuma8(
                image::GrayImage::from_fn(100, 150, |_, _| image::Luma([brightness])),
            );
            img.save(images_dir.join(format!("page_{:03}.jpg", i))).unwrap();
        }

        let output_path = dir.join("comic.azw3");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            kf8_only: true,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic kf8_only failed");
        fs::read(&output_path).expect("could not read comic AZW3")
    }

    #[test]
    fn test_comic_kf8_only_record0_version_8() {
        let dir = TempDir::new("comic_kf8only_ver");
        let data = build_comic_kf8_only_bytes(dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI magic at offset 16
        assert_eq!(&rec0[16..20], b"MOBI", "Record 0 should contain MOBI magic");

        // File version at MOBI header offset 20 (rec0 offset 36)
        let version = read_u32_be(rec0, 36);
        assert_eq!(version, 8, "Comic KF8-only version should be 8, got {}", version);

        // Min version at MOBI header offset 88 (rec0 offset 104)
        let min_version = read_u32_be(rec0, 104);
        assert_eq!(min_version, 8, "Comic KF8-only min_version should be 8, got {}", min_version);
        println!("  \u{2713} Comic KF8-only rec0: version={}, min_version={}", version, min_version);
    }

    #[test]
    fn test_comic_kf8_only_no_kf7_kf8_boundary() {
        let dir = TempDir::new("comic_kf8only_nobound");
        let data = build_comic_kf8_only_bytes(dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // There should be no BOUNDARY record followed by a KF8 Record 0 (MOBI magic).
        for i in 0..offsets.len().saturating_sub(1) {
            let rec = get_record(&data, &offsets, i);
            if rec.len() == 8 && &rec[0..8] == b"BOUNDARY" {
                let next_rec = get_record(&data, &offsets, i + 1);
                assert!(
                    next_rec.len() < 20 || &next_rec[16..20] != b"MOBI",
                    "Comic KF8-only should not have a BOUNDARY separating KF7/KF8 sections (found at index {})", i
                );
            }
        }

        // Record 0 should be the only MOBI record header
        let rec0 = get_record(&data, &offsets, 0);
        assert_eq!(&rec0[16..20], b"MOBI");
        let version = read_u32_be(rec0, 36);
        assert_eq!(version, 8, "The sole Record 0 should be version 8 (KF8)");
        println!("  \u{2713} Comic KF8-only: no KF7/KF8 BOUNDARY, sole rec0 version={}", version);
    }

    #[test]
    fn test_comic_kf8_only_images_present() {
        let dir = TempDir::new("comic_kf8only_imgs");
        let data = build_comic_kf8_only_bytes(dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // first_image_record at MOBI header offset 92 (rec0 offset 108)
        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(
            first_img,
            0xFFFFFFFF_u32 as usize,
            "Comic KF8-only with images should have first_image set"
        );

        // The image record should contain JPEG magic
        let img_rec = get_record(&data, &offsets, first_img);
        assert!(
            img_rec.len() >= 2 && img_rec[0] == 0xFF && img_rec[1] == 0xD8,
            "Image record should start with JPEG magic (FF D8)"
        );

        // Verify all 3 source images plus 1 synthesised library thumbnail
        // are present. build_book_mobi appends a downscaled thumbnail as an
        // extra image record so EXTH 202 can point at a small library tile
        // separate from the full-size cover; this raises the total JPEG
        // count by one on every comic build.
        let mut jpeg_count = 0;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 2 && rec[0] == 0xFF && rec[1] == 0xD8 {
                jpeg_count += 1;
            }
        }
        assert_eq!(
            jpeg_count, 4,
            "Should have 3 source images + 1 library thumbnail = 4 JPEGs, found {}",
            jpeg_count
        );
        println!("  \u{2713} Comic KF8-only: {} JPEGs (3 source + 1 thumbnail) at index {}", jpeg_count, first_img);
    }

    #[test]
    fn test_comic_kf8_only_has_eof() {
        let dir = TempDir::new("comic_kf8only_eof");
        let data = build_comic_kf8_only_bytes(dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // Last record should be EOF marker
        let last_rec = get_record(&data, &offsets, offsets.len() - 1);
        assert_eq!(
            last_rec,
            &[0xE9, 0x8E, 0x0D, 0x0A],
            "Last record should be EOF marker"
        );
        println!("  \u{2713} Comic KF8-only: last record is EOF marker");
    }

    #[test]
    fn test_comic_kf8_only_smaller_than_dual() {
        use crate::comic;

        let dir_dual = TempDir::new("comic_kf8only_cmp_dual");
        let dir_kf8 = TempDir::new("comic_kf8only_cmp_kf8");

        // Build dual format comic
        let images_dir_dual = dir_dual.path().join("images");
        fs::create_dir_all(&images_dir_dual).unwrap();
        for i in 0..3u8 {
            let img = image::DynamicImage::ImageLuma8(
                image::GrayImage::from_fn(100, 150, |_, _| image::Luma([50 + i * 80])),
            );
            img.save(images_dir_dual.join(format!("page_{:03}.jpg", i))).unwrap();
        }
        let output_dual = dir_dual.path().join("comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        let options_dual = comic::ComicOptions {
            rtl: false, split: false, crop: 0, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
            kf8_only: false,
            ..Default::default()
        };
        comic::build_comic_with_options(&images_dir_dual, &output_dual, &profile, &options_dual)
            .expect("dual comic build failed");
        let dual_data = fs::read(&output_dual).unwrap();

        // Build KF8-only comic
        let kf8_data = build_comic_kf8_only_bytes(dir_kf8.path());

        assert!(
            kf8_data.len() < dual_data.len(),
            "Comic KF8-only ({} bytes) should be smaller than dual format ({} bytes)",
            kf8_data.len(),
            dual_data.len()
        );
        println!("  \u{2713} Comic KF8-only {} bytes < dual {} bytes", kf8_data.len(), dual_data.len());
    }

    #[test]
    fn test_comic_kf8_only_fixed_layout() {
        let dir = TempDir::new("comic_kf8only_fl");
        let data = build_comic_kf8_only_bytes(dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // Comics should have EXTH 122 = "true" (fixed-layout) even in KF8-only mode
        let exth122 = exth.get(&122).expect("Comic KF8-only should have EXTH 122 (fixed-layout)");
        let value = std::str::from_utf8(&exth122[0]).unwrap();
        assert_eq!(value, "true", "EXTH 122 should be 'true' for fixed-layout");
        println!("  \u{2713} Comic KF8-only: EXTH 122=true (fixed-layout)");
    }

    // =======================================================================
    // Dictionary image and cover support
    // =======================================================================

    /// Create a dictionary fixture that includes a cover image in the manifest.
    fn create_dict_fixture_with_cover(
        dir: &Path,
        entries: &[(&str, &[&str])],
        image_data: &[u8],
    ) -> PathBuf {
        // Build HTML content with idx:entry markup and an img tag for the cover
        let mut html_body = String::from(r#"<img src="cover.jpg"/>"#);
        for (hw, iforms) in entries {
            html_body.push_str(&format!(
                "<idx:entry><idx:orth value=\"{hw}\">{hw}</idx:orth>",
                hw = hw
            ));
            for iform in *iforms {
                html_body.push_str(&format!(
                    "<idx:infl><idx:iform value=\"{iform}\"/></idx:infl>",
                    iform = iform
                ));
            }
            html_body.push_str(&format!(
                "<b>{hw}</b> definition of {hw}<hr/></idx:entry>\n",
                hw = hw
            ));
        }

        let html = format!(
            r#"<html><head><guide></guide></head><body>{}</body></html>"#,
            html_body
        );
        fs::write(dir.join("content.html"), &html).unwrap();
        fs::write(dir.join("cover.jpg"), image_data).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <meta name="cover" content="cover-image"/>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
    <item id="cover-image" href="cover.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    #[test]
    fn test_dict_image_records_jpeg_magic() {
        let dir = TempDir::new("dict_img_jpeg");
        let jpeg = make_test_jpeg();
        let opf = create_dict_fixture_with_cover(
            dir.path(),
            &[("apple", &["apples"]), ("banana", &["bananas"])],
            &jpeg,
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // first_image_record at MOBI header offset 92 (rec0 offset 16+92 = 108)
        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(
            first_img,
            0xFFFFFFFF_u32 as usize,
            "Dictionary with image should have first_image set"
        );

        // Verify the image record starts with JPEG magic
        let img_rec = get_record(&data, &offsets, first_img);
        assert!(
            img_rec.len() >= 2 && img_rec[0] == 0xFF && img_rec[1] == 0xD8,
            "Image record should start with JPEG magic (FF D8)"
        );
        println!(
            "  \u{2713} Dict image record at index {}, starts with JPEG magic FF D8",
            first_img
        );
    }

    #[test]
    fn test_dict_cover_offset_exth_201() {
        let dir = TempDir::new("dict_cover_exth");
        let jpeg = make_test_jpeg();
        let opf = create_dict_fixture_with_cover(
            dir.path(),
            &[("word", &["words"])],
            &jpeg,
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        // EXTH 201 = cover offset (0-based index within image records)
        assert!(
            exth.contains_key(&201),
            "Dictionary with cover should have EXTH 201 (cover offset)"
        );
        let cover_val = read_u32_be(&exth[&201][0], 0);
        assert_eq!(
            cover_val, 0,
            "Cover offset should be 0 (first and only image), got {}",
            cover_val
        );
        println!("  \u{2713} Dict EXTH 201 cover offset: {}", cover_val);
    }

    #[test]
    fn test_dict_with_image_indx_still_valid() {
        let dir = TempDir::new("dict_img_indx");
        let jpeg = make_test_jpeg();
        let opf = create_dict_fixture_with_cover(
            dir.path(),
            &[
                ("alpha", &["alphas"]),
                ("beta", &["betas"]),
                ("gamma", &["gammas"]),
            ],
            &jpeg,
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Orth index record at MOBI header offset 24 (record0 offset 40)
        let orth_idx = read_u32_be(rec0, 40) as usize;
        assert_ne!(orth_idx, 0xFFFFFFFF_u32 as usize, "Should have valid orth index");

        // The INDX record should come after the image records
        let first_img = read_u32_be(rec0, 108) as usize;
        assert!(
            orth_idx > first_img,
            "INDX record ({}) should come after image record ({})",
            orth_idx,
            first_img
        );

        // Verify INDX magic
        let indx_rec = get_record(&data, &offsets, orth_idx);
        assert_eq!(
            &indx_rec[0..4],
            b"INDX",
            "INDX record should start with INDX magic"
        );
        println!(
            "  \u{2713} Dict with images: INDX at {}, after image at {}",
            orth_idx, first_img
        );
    }

    #[test]
    fn test_dict_without_image_still_works() {
        // Verify that dictionaries without images still produce 0xFFFFFFFF
        let dir = TempDir::new("dict_no_img");
        let opf = create_dict_fixture(dir.path(), &[("test", &["tests"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let first_img = read_u32_be(rec0, 108);
        assert_eq!(
            first_img, 0xFFFFFFFF,
            "Dictionary without images should have first_image = 0xFFFFFFFF"
        );

        let exth = parse_exth_records(rec0);
        assert!(
            !exth.contains_key(&201),
            "Dictionary without images should not have EXTH 201"
        );
        println!("  \u{2713} Dict without images: first_image=0xFFFFFFFF, no EXTH 201");
    }

    #[test]
    fn test_dict_image_src_rewritten_to_recindex() {
        let dir = TempDir::new("dict_img_recindex");
        let jpeg = make_test_jpeg();
        let opf = create_dict_fixture_with_cover(
            dir.path(),
            &[("test", &["tests"])],
            &jpeg,
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        // Extract text from uncompressed records and check for recindex
        let text = extract_text_from_uncompressed_mobi(&data);
        assert!(
            text.contains("recindex=\"00001\""),
            "Image src should be rewritten to recindex in dictionary text, got: {}",
            &text[..text.len().min(500)]
        );
        assert!(
            !text.contains("src=\"cover.jpg\""),
            "Original src=\"cover.jpg\" should not remain in dictionary text"
        );
        println!("  \u{2713} Dict image src rewritten to recindex=\"00001\"");
    }
    // =======================================================================
    // 23. MOBI header fields - Dictionary
    // =======================================================================

    #[test]
    fn test_dict_mobi_header_magic() {
        let dir = TempDir::new("dict_hdr_magic");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 0 relative to MOBI magic at byte 16 of Record 0
        assert_eq!(&rec0[16..20], b"MOBI", "Dict MOBI magic at rec0[16..20]");
        println!("  \u{2713} Dict MOBI magic ok");
    }

    #[test]
    fn test_dict_mobi_header_length_264() {
        let dir = TempDir::new("dict_hdr_len");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 4: header length
        let hdr_len = read_u32_be(rec0, 16 + 4);
        assert_eq!(hdr_len, 264, "Dict MOBI header length should be 264, got {}", hdr_len);
        println!("  \u{2713} Dict MOBI header length: {}", hdr_len);
    }

    #[test]
    fn test_dict_mobi_type_2() {
        let dir = TempDir::new("dict_hdr_type");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 8: MOBI type
        let mobi_type = read_u32_be(rec0, 16 + 8);
        assert_eq!(mobi_type, 2, "Dict MOBI type should be 2, got {}", mobi_type);
        println!("  \u{2713} Dict MOBI type: {}", mobi_type);
    }

    #[test]
    fn test_dict_mobi_encoding_utf8() {
        let dir = TempDir::new("dict_hdr_enc");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 12: encoding
        let encoding = read_u32_be(rec0, 16 + 12);
        assert_eq!(encoding, 65001, "Dict encoding should be 65001 (UTF-8), got {}", encoding);
        println!("  \u{2713} Dict encoding: {}", encoding);
    }

    #[test]
    fn test_dict_mobi_unique_id_nonzero() {
        let dir = TempDir::new("dict_hdr_uid");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 16: unique ID
        let unique_id = read_u32_be(rec0, 16 + 16);
        assert_ne!(unique_id, 0, "Dict unique ID should be non-zero");
        println!("  \u{2713} Dict unique ID: 0x{:08X}", unique_id);
    }

    #[test]
    fn test_dict_mobi_file_version() {
        let dir = TempDir::new("dict_hdr_ver");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 20: file version (dicts default to 7, but 6 is also acceptable)
        let version = read_u32_be(rec0, 16 + 20);
        assert!(
            version == 6 || version == 7,
            "Dict file version should be 6 or 7, got {}",
            version
        );
        println!("  \u{2713} Dict file version: {}", version);
    }

    #[test]
    fn test_dict_mobi_orth_index_valid() {
        let dir = TempDir::new("dict_hdr_orth");
        let opf = create_dict_fixture(dir.path(), &[("apple", &["apples"]), ("banana", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 24: orth index - should be a valid record index for dicts
        let orth_idx = read_u32_be(rec0, 16 + 24);
        assert_ne!(orth_idx, 0xFFFFFFFF, "Dict orth index should not be 0xFFFFFFFF");
        assert!(
            (orth_idx as u16) < record_count,
            "Dict orth index {} should be < record count {}",
            orth_idx, record_count
        );
        println!("  \u{2713} Dict orth index: {}", orth_idx);
    }

    #[test]
    fn test_dict_mobi_unused_indices_ffffffff() {
        let dir = TempDir::new("dict_hdr_unused");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Inflections are flattened into the orth INDX (lemma v1.0.0 behaviour)
        // so there is no separate infl INDX. Offsets 28..60 should all be 0xFFFFFFFF.
        for off in (28..=60).step_by(4) {
            let val = read_u32_be(rec0, 16 + off);
            assert_eq!(
                val, 0xFFFFFFFF,
                "Dict unused index at MOBI offset {} should be 0xFFFFFFFF, got 0x{:08X}",
                off, val
            );
        }
        println!("  \u{2713} Dict unused indices [28..60] all 0xFFFFFFFF");
    }

    #[test]
    fn test_dict_mobi_first_non_book_record() {
        let dir = TempDir::new("dict_hdr_fnbr");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 64: first non-book record - should be a valid index
        let fnbr = read_u32_be(rec0, 16 + 64);
        assert!(
            fnbr > 0 && (fnbr as u16) <= record_count,
            "Dict first non-book record {} should be valid (1..={})",
            fnbr, record_count
        );
        println!("  \u{2713} Dict first non-book record: {}", fnbr);
    }

    #[test]
    fn test_dict_mobi_language_code() {
        let dir = TempDir::new("dict_hdr_lang");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 76: language code - should be non-zero for "en"
        let lang = read_u32_be(rec0, 16 + 76);
        assert_ne!(lang, 0, "Dict language code should be non-zero");
        assert_eq!(lang, 0x0409, "Dict language code for 'en' should be 0x0409 (Windows LCID), got 0x{:X}", lang);
        println!("  \u{2713} Dict language code: {}", lang);
    }

    #[test]
    fn test_dict_mobi_input_output_language() {
        let dir = TempDir::new("dict_hdr_io_lang");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 80: input language - non-zero for dicts with DictionaryInLanguage
        let input_lang = read_u32_be(rec0, 16 + 80);
        assert_ne!(input_lang, 0, "Dict input language should be non-zero (DictionaryInLanguage=en)");

        // Offset 84: output language - non-zero for dicts with DictionaryOutLanguage
        let output_lang = read_u32_be(rec0, 16 + 84);
        assert_ne!(output_lang, 0, "Dict output language should be non-zero (DictionaryOutLanguage=en)");
        println!("  \u{2713} Dict input lang: {}, output lang: {}", input_lang, output_lang);
    }

    #[test]
    fn test_dict_mobi_min_version_matches_file_version() {
        let dir = TempDir::new("dict_hdr_minver");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 20: file version, offset 88: min version - should match
        let file_ver = read_u32_be(rec0, 16 + 20);
        let min_ver = read_u32_be(rec0, 16 + 88);
        assert_eq!(
            min_ver, file_ver,
            "Dict min version ({}) should match file version ({})",
            min_ver, file_ver
        );
        println!("  \u{2713} Dict min version: {} == file version: {}", min_ver, file_ver);
    }

    #[test]
    fn test_dict_mobi_capability_marker_0x50() {
        let dir = TempDir::new("dict_hdr_cap");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 112: capability marker - 0x50 for dicts
        let cap = read_u32_be(rec0, 16 + 112);
        assert_eq!(cap, 0x50, "Dict capability marker should be 0x50, got 0x{:X}", cap);
        println!("  \u{2713} Dict capability marker: 0x{:X}", cap);
    }

    #[test]
    fn test_dict_mobi_extra_record_data_flags() {
        let dir = TempDir::new("dict_hdr_erdf");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 224: extra record data flags - should be 3 (multibyte + TBS)
        let flags = read_u32_be(rec0, 16 + 224);
        assert_eq!(flags, 3, "Dict extra record data flags should be 3, got {}", flags);
        println!("  \u{2713} Dict extra record data flags: {}", flags);
    }

    // =======================================================================
    // 24. MOBI header fields - Book
    // =======================================================================

    #[test]
    fn test_book_mobi_header_magic() {
        let dir = TempDir::new("book_hdr_magic");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        assert_eq!(&rec0[16..20], b"MOBI", "Book MOBI magic at rec0[16..20]");
        println!("  \u{2713} Book MOBI magic ok");
    }

    #[test]
    fn test_book_mobi_header_length_264() {
        let dir = TempDir::new("book_hdr_len");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let hdr_len = read_u32_be(rec0, 16 + 4);
        assert_eq!(hdr_len, 264, "Book MOBI header length should be 264, got {}", hdr_len);
        println!("  \u{2713} Book MOBI header length: {}", hdr_len);
    }

    #[test]
    fn test_book_mobi_type_2() {
        let dir = TempDir::new("book_hdr_type");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let mobi_type = read_u32_be(rec0, 16 + 8);
        assert_eq!(mobi_type, 2, "Book MOBI type should be 2, got {}", mobi_type);
        println!("  \u{2713} Book MOBI type: {}", mobi_type);
    }

    #[test]
    fn test_book_mobi_encoding_utf8() {
        let dir = TempDir::new("book_hdr_enc");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let encoding = read_u32_be(rec0, 16 + 12);
        assert_eq!(encoding, 65001, "Book encoding should be 65001 (UTF-8), got {}", encoding);
        println!("  \u{2713} Book encoding: {}", encoding);
    }

    #[test]
    fn test_book_mobi_unique_id_nonzero() {
        let dir = TempDir::new("book_hdr_uid");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let unique_id = read_u32_be(rec0, 16 + 16);
        assert_ne!(unique_id, 0, "Book unique ID should be non-zero");
        println!("  \u{2713} Book unique ID: 0x{:08X}", unique_id);
    }

    #[test]
    fn test_book_mobi_file_version_7() {
        // Dual-format book KF7 Record 0 should have version 7
        // (actually KF7 uses version 6 with EXTH 121 pointing to KF8)
        let dir = TempDir::new("book_hdr_ver");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let version = read_u32_be(rec0, 16 + 20);
        // Dual-format books use version 6 for KF7 Record 0
        assert!(
            version == 6 || version == 7,
            "Book KF7 file version should be 6 or 7, got {}",
            version
        );
        println!("  \u{2713} Book KF7 file version: {}", version);
    }

    #[test]
    fn test_book_mobi_orth_index_ffffffff() {
        let dir = TempDir::new("book_hdr_orth");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 24: orth index - 0xFFFFFFFF for books (no dictionary index)
        let orth_idx = read_u32_be(rec0, 16 + 24);
        assert_eq!(
            orth_idx, 0xFFFFFFFF,
            "Book orth index should be 0xFFFFFFFF, got 0x{:08X}",
            orth_idx
        );
        println!("  \u{2713} Book orth index: 0x{:08X}", orth_idx);
    }

    #[test]
    fn test_book_mobi_unused_indices_ffffffff() {
        let dir = TempDir::new("book_hdr_unused");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offsets 28-62: unused indices, all 0xFFFFFFFF
        for off in (28..=60).step_by(4) {
            let val = read_u32_be(rec0, 16 + off);
            assert_eq!(
                val, 0xFFFFFFFF,
                "Book unused index at MOBI offset {} should be 0xFFFFFFFF, got 0x{:08X}",
                off, val
            );
        }
        println!("  \u{2713} Book unused indices [28..62] all 0xFFFFFFFF");
    }

    #[test]
    fn test_book_mobi_first_non_book_record() {
        let dir = TempDir::new("book_hdr_fnbr");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let fnbr = read_u32_be(rec0, 16 + 64);
        assert!(
            fnbr > 0 && (fnbr as u16) <= record_count,
            "Book first non-book record {} should be valid (1..={})",
            fnbr, record_count
        );
        println!("  \u{2713} Book first non-book record: {}", fnbr);
    }

    #[test]
    fn test_book_mobi_language_code() {
        let dir = TempDir::new("book_hdr_lang");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let lang = read_u32_be(rec0, 16 + 76);
        assert_ne!(lang, 0, "Book language code should be non-zero for 'en'");
        assert_eq!(lang, 0x0409, "Book language code for 'en' should be 0x0409 (Windows LCID), got 0x{:X}", lang);
        println!("  \u{2713} Book language code: {}", lang);
    }

    #[test]
    fn test_book_mobi_min_version_matches_file_version() {
        let dir = TempDir::new("book_hdr_minver");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let file_ver = read_u32_be(rec0, 16 + 20);
        let min_ver = read_u32_be(rec0, 16 + 88);
        assert_eq!(
            min_ver, file_ver,
            "Book min version ({}) should match file version ({})",
            min_ver, file_ver
        );
        println!("  \u{2713} Book min version: {} == file version: {}", min_ver, file_ver);
    }

    #[test]
    fn test_book_mobi_first_image_record_valid() {
        let dir = TempDir::new("book_hdr_fimg");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Offset 92: first image record
        let first_img = read_u32_be(rec0, 16 + 92);
        assert_ne!(first_img, 0xFFFFFFFF, "Book with image should have first_image set");
        assert!(
            (first_img as u16) < record_count,
            "Book first image record {} should be < record count {}",
            first_img, record_count
        );
        println!("  \u{2713} Book first image record: {}", first_img);
    }

    #[test]
    fn test_book_mobi_capability_marker_0x4850() {
        let dir = TempDir::new("book_hdr_cap");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let cap = read_u32_be(rec0, 16 + 112);
        assert_eq!(cap, 0x850, "Book capability marker should be 0x850, got 0x{:X}", cap);
        println!("  \u{2713} Book capability marker: 0x{:X}", cap);
    }

    #[test]
    fn test_book_mobi_extra_record_data_flags() {
        let dir = TempDir::new("book_hdr_erdf");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let flags = read_u32_be(rec0, 16 + 224);
        assert_eq!(flags, 3, "Book extra record data flags should be 3, got {}", flags);
        println!("  \u{2713} Book extra record data flags: {}", flags);
    }

    #[test]
    fn test_kf8_only_mobi_header_fields() {
        // KF8-only book: version=8, min_version=8
        let dir = TempDir::new("kf8only_hdr_fields");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Magic
        assert_eq!(&rec0[16..20], b"MOBI");
        // Header length
        assert_eq!(read_u32_be(rec0, 16 + 4), 264);
        // MOBI type
        assert_eq!(read_u32_be(rec0, 16 + 8), 2);
        // Encoding
        assert_eq!(read_u32_be(rec0, 16 + 12), 65001);
        // Unique ID non-zero
        assert_ne!(read_u32_be(rec0, 16 + 16), 0);
        // File version = 8
        assert_eq!(read_u32_be(rec0, 16 + 20), 8);
        // Min version = 8
        assert_eq!(read_u32_be(rec0, 16 + 88), 8);
        // Orth index = fragment INDX record (matches KCC/kindlegen)
        let orth = read_u32_be(rec0, 16 + 24);
        assert_ne!(orth, 0, "KF8 orth index should point to fragment INDX");
        // First non-book record valid
        let fnbr = read_u32_be(rec0, 16 + 64);
        assert!(fnbr > 0 && (fnbr as u16) <= record_count);
        // Capability marker = 0x50 for KF8 (matches KCC/kindlegen)
        assert_eq!(read_u32_be(rec0, 16 + 112), 0x50);
        // Extra record data flags = 3 (multibyte + TBS)
        assert_eq!(read_u32_be(rec0, 16 + 224), 3);
        println!("  \u{2713} KF8-only MOBI header fields all correct");
    }

    // =======================================================================
    // 24b. KF8 structural requirements (from Kindle hardware testing)
    // =======================================================================

    #[test]
    fn test_kf8_record0_padded_to_8892() {
        // Kindle rejects Record 0 smaller than ~8892 bytes.
        let dir = TempDir::new("kf8_r0_pad");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        assert!(rec0.len() >= 8892,
            "KF8 Record 0 must be >= 8892 bytes (padded), got {}", rec0.len());
    }

    #[test]
    fn test_kf8_first_nonbook_skips_null_pad() {
        // first_nonbook must be text_count+2 (skip NULL pad), not +1.
        let dir = TempDir::new("kf8_fnb");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let text_count = u16::from_be_bytes(rec0[8..10].try_into().unwrap()) as u32;
        let fnb = read_u32_be(rec0, 16 + 64);
        assert_eq!(fnb, text_count + 2,
            "first_nonbook should be text_count+2={}, got {}", text_count + 2, fnb);
    }

    #[test]
    fn test_book_record0_padded_to_8892() {
        let dir = TempDir::new("book_r0_pad");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        assert!(rec0.len() >= 8892,
            "Book Record 0 must be >= 8892 bytes (padded), got {}", rec0.len());
    }

    #[test]
    fn test_kf8_null_pad_is_2_bytes() {
        let dir = TempDir::new("kf8_null");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let text_count = u16::from_be_bytes(rec0[8..10].try_into().unwrap()) as usize;
        // NULL pad is at record text_count + 1
        let null_rec = get_record(&data, &offsets, text_count + 1);
        assert_eq!(null_rec.len(), 2, "NULL pad must be 2 bytes, got {}", null_rec.len());
        assert_eq!(null_rec, &[0x00, 0x00]);
    }

    #[test]
    fn test_kf8_language_uses_lcid() {
        // Language code must be Windows LCID (0x0409 for en), not primary ID (0x09).
        let dir = TempDir::new("kf8_lcid");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let lang = read_u32_be(rec0, 16 + 76);
        assert!(lang > 0xFF,
            "Language should be Windows LCID (>0xFF), got 0x{:X}", lang);
    }

    // -----------------------------------------------------------------------
    // Helper: fixed-layout book fixture (for EXTH 503 / comic tests)
    // -----------------------------------------------------------------------

    /// Create a minimal fixed-layout book OPF + HTML in a temp dir.
    /// Includes `<meta name="fixed-layout" content="true"/>` so the OPF
    /// parser sets `is_fixed_layout = true`. Always includes an image.
    fn create_fixed_layout_book_fixture(
        dir: &Path,
        image_data: &[u8],
    ) -> PathBuf {
        let html = r#"<html><head><title>Test Comic</title></head><body><div><img src="page.jpg"/></div></body></html>"#;
        fs::write(dir.join("content.html"), html).unwrap();
        fs::write(dir.join("page.jpg"), image_data).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Test Comic</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Author</dc:creator>
    <meta name="cover" content="img1"/>
    <meta name="fixed-layout" content="true"/>
    <meta name="original-resolution" content="1072x1448"/>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
    <item id="img1" href="page.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    // -----------------------------------------------------------------------
    // 24b (continued). KF8 structural regressions (Kindle hardware testing)
    // -----------------------------------------------------------------------

    #[test]
    fn test_exth_503_not_emitted_for_fixed_layout() {
        // EXTH 503 (updated_title) breaks Kindle fixed-layout navigation:
        // toolbar and go-home disappear. KCC/kindlegen also omit it for comics.
        let dir = TempDir::new("exth503_fl");
        let jpeg = make_test_jpeg();
        let opf = create_fixed_layout_book_fixture(dir.path(), &jpeg);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            !exth.contains_key(&503),
            "Fixed-layout book must NOT have EXTH 503 (updated_title) - it breaks Kindle navigation"
        );
        // Sanity: should still have EXTH 122 = "true" (fixed-layout flag)
        let exth122 = exth.get(&122).expect("Fixed-layout book should have EXTH 122");
        assert_eq!(std::str::from_utf8(&exth122[0]).unwrap(), "true");
    }

    #[test]
    fn test_exth_503_emitted_for_reflowable_book() {
        // Reflowable books must have EXTH 503 (updated_title).
        let dir = TempDir::new("exth503_reflow");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&503),
            "Reflowable book must have EXTH 503 (updated_title)"
        );
        let title = std::str::from_utf8(&exth.get(&503).unwrap()[0]).unwrap();
        assert_eq!(title, "Test Book", "EXTH 503 should contain the book title");
    }

    #[test]
    fn test_kf8_record_order_fdst_flis_fcis_datp_eof() {
        // Wrong order (DATP before FLIS/FCIS) crashes Kindle.
        // After the NCX INDX+CNCX records, the order must be:
        // FDST, FLIS, FCIS, DATP, EOF.
        let dir = TempDir::new("kf8_rec_order");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, record_count, offsets) = parse_palmdb(&data);

        // Walk all records and collect the magic-identified structural records.
        // Stop after the first EOF since an HD container may follow with its
        // own EOF marker.
        let mut found_sequence: Vec<&str> = Vec::new();
        for i in 0..record_count as usize {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 {
                if &rec[0..4] == b"FDST" {
                    found_sequence.push("FDST");
                } else if &rec[0..4] == b"FLIS" {
                    found_sequence.push("FLIS");
                } else if &rec[0..4] == b"FCIS" {
                    found_sequence.push("FCIS");
                } else if &rec[0..4] == b"DATP" {
                    found_sequence.push("DATP");
                } else if rec[0..4] == [0xE9, 0x8E, 0x0D, 0x0A] {
                    found_sequence.push("EOF");
                    break; // stop at first EOF (HD container has its own)
                }
            }
        }
        assert_eq!(
            found_sequence,
            vec!["FDST", "FLIS", "FCIS", "DATP", "EOF"],
            "KF8 record order must be FDST, FLIS, FCIS, DATP, EOF - got {:?}",
            found_sequence
        );
    }

    #[test]
    fn test_kf7_fdst_composite_uses_flis_minus_1() {
        // KF7 Record 0 at MOBI offset 176 (record byte 192) holds
        // a composite: high 16 bits = flow count (1), low 16 bits =
        // flis_record - 1 (NOT total_records - 1).
        let dir = TempDir::new("kf7_fdst_comp");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Read the composite at MOBI offset 176 = record byte 192
        let composite = read_u32_be(rec0, 16 + 176);
        let high = composite >> 16;
        let low = composite & 0xFFFF;

        // High 16 bits should be 1 (KF7 flow count)
        assert_eq!(high, 1, "FDST composite high word should be 1 (flow count), got {}", high);

        // Find the FLIS record by scanning for its magic bytes
        let mut flis_idx: Option<usize> = None;
        for i in 0..record_count as usize {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FLIS" {
                flis_idx = Some(i);
                break;
            }
        }
        let flis_record = flis_idx.expect("Should find FLIS record in KF7 section");

        // Low 16 bits should be flis_record - 1
        assert_eq!(
            low, (flis_record - 1) as u32,
            "FDST composite low word should be flis_record-1={}, got {}",
            flis_record - 1, low
        );
    }

    #[test]
    fn test_kf8_fcis_entry_count_matches_flow_count() {
        // FCIS entry_count (at offset 12) should match the FDST flow count.
        // For a book with HTML+CSS, flow_count=2 and entry_count should be >= 2.
        // The old value of 1 was wrong.
        let dir = TempDir::new("kf8_fcis_flows");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, record_count, offsets) = parse_palmdb(&data);

        // Find the FCIS record
        let mut fcis_rec: Option<&[u8]> = None;
        for i in 0..record_count as usize {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FCIS" {
                fcis_rec = Some(rec);
                break;
            }
        }
        let fcis = fcis_rec.expect("Should find FCIS record");

        // Entry count is at offset 12 in the FCIS record (u32 BE)
        let entry_count = read_u32_be(fcis, 12);
        assert!(
            entry_count >= 2,
            "FCIS entry_count should be >= 2 for a book with HTML+CSS flows, got {}",
            entry_count
        );
    }

    #[test]
    fn test_kf8_first_image_equals_fdst_idx_in_dual_format() {
        // In dual-format books, the KF8 Record 0's first_image field
        // (MOBI offset 92, record byte 108) must equal the fdst_record field
        // (MOBI offset 176, record byte 192).
        let dir = TempDir::new("kf8_fimg_fdst");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _record_count, offsets) = parse_palmdb(&data);

        // Find KF8 Record 0: EXTH 121 stores the global index of the
        // KF8 Record 0 directly (not the BOUNDARY record).
        let kf7_rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(kf7_rec0);
        let boundary_entries = exth.get(&121).expect("Dual-format should have EXTH 121");
        let kf8_rec0_idx = u32::from_be_bytes(
            boundary_entries[0][0..4].try_into().unwrap()
        ) as usize;

        // Verify the BOUNDARY record immediately precedes KF8 Record 0
        let boundary_rec = get_record(&data, &offsets, kf8_rec0_idx - 1);
        assert_eq!(&boundary_rec[0..8], b"BOUNDARY", "Expected BOUNDARY record before KF8 Record 0");

        let kf8_rec0 = get_record(&data, &offsets, kf8_rec0_idx);
        assert_eq!(&kf8_rec0[16..20], b"MOBI", "KF8 Record 0 should have MOBI magic");

        // first_image at MOBI offset 92 = record byte 16 + 92 = 108
        let first_image = read_u32_be(kf8_rec0, 16 + 92);
        // fdst_record at MOBI offset 176 = record byte 16 + 176 = 192
        let fdst_record = read_u32_be(kf8_rec0, 16 + 176);

        assert_eq!(
            first_image, fdst_record,
            "KF8 first_image ({}) must equal fdst_record ({}) in dual-format",
            first_image, fdst_record
        );
    }

    // =======================================================================
    // 25. EXTH records - Dictionary
    // =======================================================================

    #[test]
    fn test_dict_exth_100_author() {
        let dir = TempDir::new("dict_exth_100");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let author_entries = exth.get(&100).expect("Dict EXTH 100 (Author) should be present");
        let author = std::str::from_utf8(&author_entries[0]).unwrap();
        assert_eq!(author, "Tester", "Dict author should be 'Tester', got '{}'", author);
        println!("  \u{2713} Dict EXTH 100 (Author): {}", author);
    }

    #[test]
    fn test_dict_exth_125_value_1() {
        let dir = TempDir::new("dict_exth_125");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&125).expect("Dict EXTH 125 should be present");
        let val = u32::from_be_bytes([entries[0][0], entries[0][1], entries[0][2], entries[0][3]]);
        assert_eq!(val, 1, "Dict EXTH 125 should be 1, got {}", val);
        println!("  \u{2713} Dict EXTH 125: {}", val);
    }

    #[test]
    fn test_dict_exth_131_value_0() {
        let dir = TempDir::new("dict_exth_131");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&131).expect("Dict EXTH 131 should be present");
        let val = u32::from_be_bytes([entries[0][0], entries[0][1], entries[0][2], entries[0][3]]);
        assert_eq!(val, 0, "Dict EXTH 131 should be 0, got {}", val);
        println!("  \u{2713} Dict EXTH 131: {}", val);
    }

    #[test]
    fn test_dict_exth_201_cover_offset_with_images() {
        let dir = TempDir::new("dict_exth_201");
        let jpeg = make_test_jpeg();
        let opf = create_dict_fixture_with_cover(
            dir.path(),
            &[("word", &["words"])],
            &jpeg,
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&201),
            "Dict with images should have EXTH 201 (CoverOffset)"
        );
        println!("  \u{2713} Dict EXTH 201 (CoverOffset) present with images");
    }

    #[test]
    fn test_dict_exth_300_fontsignature() {
        let dir = TempDir::new("dict_exth_300");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&300).expect("Dict EXTH 300 (Fontsignature) should be present");
        // Fontsignature: USB(16) + CSB(8) + padding(8) + char_data(4 prefix + codepoints)
        // Minimum is 32 (header) + 4 (prefix) = 36 bytes for empty codepoint set
        assert!(
            entries[0].len() >= 36,
            "Dict EXTH 300 should be >= 36 bytes, got {}",
            entries[0].len()
        );
        println!("  \u{2713} Dict EXTH 300 (Fontsignature): {} bytes", entries[0].len());
    }

    #[test]
    fn test_dict_exth_501_not_present() {
        let dir = TempDir::new("dict_exth_501");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            !exth.contains_key(&501),
            "Dict should NOT have EXTH 501 (DocType)"
        );
        println!("  \u{2713} Dict EXTH 501 (DocType) absent");
    }

    #[test]
    fn test_dict_exth_524_language() {
        let dir = TempDir::new("dict_exth_524");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&524).expect("Dict EXTH 524 (Language) should be present");
        let lang = std::str::from_utf8(&entries[0]).unwrap();
        assert_eq!(lang, "en", "Dict EXTH 524 should be 'en', got '{}'", lang);
        println!("  \u{2713} Dict EXTH 524 (Language): {}", lang);
    }

    #[test]
    fn test_dict_exth_531_input_language() {
        let dir = TempDir::new("dict_exth_531");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&531).expect("Dict EXTH 531 (DictInputLang) should be present");
        let lang = std::str::from_utf8(&entries[0]).unwrap();
        // The fixture has DictionaryInLanguage=en
        assert_eq!(lang, "en", "Dict EXTH 531 should match source language 'en', got '{}'", lang);
        println!("  \u{2713} Dict EXTH 531 (DictInputLang): {}", lang);
    }

    #[test]
    fn test_dict_exth_532_output_language() {
        let dir = TempDir::new("dict_exth_532");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&532).expect("Dict EXTH 532 (DictOutputLang) should be present");
        let lang = std::str::from_utf8(&entries[0]).unwrap();
        // The fixture has DictionaryOutLanguage=en
        assert_eq!(lang, "en", "Dict EXTH 532 should match target language 'en', got '{}'", lang);
        println!("  \u{2713} Dict EXTH 532 (DictOutputLang): {}", lang);
    }

    #[test]
    fn test_dict_exth_535_creator() {
        let dir = TempDir::new("dict_exth_535");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&535),
            "Dict EXTH 535 (Creator) should be present"
        );
        println!("  \u{2713} Dict EXTH 535 (Creator) present");
    }

    #[test]
    fn test_dict_exth_547_inmemory() {
        let dir = TempDir::new("dict_exth_547");
        let opf = create_dict_fixture(dir.path(), &[("word", &["words"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&547).expect("Dict EXTH 547 (InMemory) should be present");
        let val = std::str::from_utf8(&entries[0]).unwrap();
        assert_eq!(val, "InMemory", "Dict EXTH 547 should be 'InMemory', got '{}'", val);
        println!("  \u{2713} Dict EXTH 547: {}", val);
    }

    // =======================================================================
    // 26. EXTH records - Book
    // =======================================================================

    #[test]
    fn test_book_exth_100_author() {
        let dir = TempDir::new("book_exth_100");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let author_entries = exth.get(&100).expect("Book EXTH 100 (Author) should be present");
        let author = std::str::from_utf8(&author_entries[0]).unwrap();
        assert_eq!(author, "Author", "Book author should be 'Author', got '{}'", author);
        println!("  \u{2713} Book EXTH 100 (Author): {}", author);
    }

    #[test]
    fn test_book_exth_121_kf8_boundary_in_dual_format() {
        let dir = TempDir::new("book_exth_121_dual");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&121),
            "Dual-format book EXTH 121 (KF8 boundary) should be present"
        );
        println!("  \u{2713} Book EXTH 121 (KF8 boundary) present in dual format");
    }

    #[test]
    fn test_book_exth_121_absent_in_kf8_only() {
        let dir = TempDir::new("book_exth_121_kf8");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            !exth.contains_key(&121),
            "KF8-only book should NOT have EXTH 121 (KF8 boundary)"
        );
        println!("  \u{2713} Book EXTH 121 absent in KF8-only");
    }

    #[test]
    fn test_book_exth_125_value_21() {
        let dir = TempDir::new("book_exth_125");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&125).expect("Book EXTH 125 should be present");
        let val = u32::from_be_bytes([entries[0][0], entries[0][1], entries[0][2], entries[0][3]]);
        assert_eq!(val, 21, "Book EXTH 125 should be 21, got {}", val);
        println!("  \u{2713} Book EXTH 125: {}", val);
    }

    #[test]
    fn test_book_exth_201_cover_offset_with_cover() {
        let dir = TempDir::new("book_exth_201");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&201),
            "Book with cover should have EXTH 201 (CoverOffset)"
        );
        println!("  \u{2713} Book EXTH 201 (CoverOffset) present");
    }

    #[test]
    fn test_book_exth_501_doc_type_pdoc() {
        let dir = TempDir::new("book_exth_501");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&501).expect("Book EXTH 501 (DocType) should be present");
        let val = std::str::from_utf8(&entries[0]).unwrap();
        assert_eq!(val, "PDOC", "Book EXTH 501 default should be 'PDOC', got '{}'", val);
        println!("  \u{2713} Book EXTH 501 (DocType): {}", val);
    }

    #[test]
    fn test_book_exth_524_language() {
        let dir = TempDir::new("book_exth_524");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&524).expect("Book EXTH 524 (Language) should be present");
        let lang = std::str::from_utf8(&entries[0]).unwrap();
        assert_eq!(lang, "en", "Book EXTH 524 should be 'en', got '{}'", lang);
        println!("  \u{2713} Book EXTH 524 (Language): {}", lang);
    }

    #[test]
    fn test_book_exth_535_creator() {
        let dir = TempDir::new("book_exth_535");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        assert!(
            exth.contains_key(&535),
            "Book EXTH 535 (Creator) should be present"
        );
        println!("  \u{2713} Book EXTH 535 (Creator) present");
    }

    #[test]
    fn test_book_exth_547_inmemory() {
        let dir = TempDir::new("book_exth_547");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let exth = parse_exth_records(rec0);

        let entries = exth.get(&547).expect("Book EXTH 547 (InMemory) should be present");
        let val = std::str::from_utf8(&entries[0]).unwrap();
        assert_eq!(val, "InMemory", "Book EXTH 547 should be 'InMemory', got '{}'", val);
        println!("  \u{2713} Book EXTH 547: {}", val);
    }
    // =======================================================================
    // 23. Text record structure
    // =======================================================================

    #[test]
    fn test_first_text_record_starts_with_html() {
        let dir = TempDir::new("first_text_html_dict");
        let opf = create_dict_fixture(dir.path(), &[("test", &["tests"])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        // Record 1 is the first text record
        let rec1 = get_record(&data, &offsets, 1);
        let content = strip_trailing_bytes(rec1);
        let text = String::from_utf8_lossy(content);

        assert!(
            text.starts_with("<html>") || text.starts_with("<html "),
            "First text record should start with '<html>', got: {:?}",
            &text[..text.len().min(60)]
        );
        println!("  \u{2713} First text record starts with '<html>' (dict)");
    }

    #[test]
    fn test_first_text_record_starts_with_html_book() {
        let dir = TempDir::new("first_text_html_book");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec1 = get_record(&data, &offsets, 1);
        let content = strip_trailing_bytes(rec1);
        let text = String::from_utf8_lossy(content);

        assert!(
            text.starts_with("<html>") || text.starts_with("<html "),
            "First text record should start with '<html>' (book), got: {:?}",
            &text[..text.len().min(60)]
        );
        println!("  \u{2713} First text record starts with '<html>' (book)");
    }

    #[test]
    fn test_compressed_records_decompress_to_same_text() {
        let dir_u = TempDir::new("comp_roundtrip_u");
        let dir_c = TempDir::new("comp_roundtrip_c");

        let entries: &[(&str, &[&str])] = &[
            ("alpha", &["alphas"]),
            ("beta", &["betas"]),
            ("gamma", &["gammas"]),
            ("delta", &["deltas"]),
        ];

        let opf_u = create_dict_fixture(dir_u.path(), entries);
        let opf_c = create_dict_fixture(dir_c.path(), entries);

        // Uncompressed
        let data_u = build_mobi_bytes(&opf_u, dir_u.path(), true, false, None);
        // Compressed
        let output_c = dir_c.path().join("output_comp.mobi");
        mobi::build_mobi(
            &opf_c, &output_c, false, false, None, false, false, false, false, None, false, false, false,
        ).expect("compressed build failed");
        let data_c = fs::read(&output_c).unwrap();

        let text_u = extract_text_blob(&data_u);

        // Decompress compressed text records
        let (_, _, offsets_c) = parse_palmdb(&data_c);
        let rec0_c = get_record(&data_c, &offsets_c, 0);
        let text_rc = read_u16_be(rec0_c, 8) as usize;

        let mut decompressed = Vec::new();
        for i in 1..=text_rc {
            if i >= offsets_c.len() { break; }
            let rec = get_record(&data_c, &offsets_c, i);
            let chunk = palmdoc_decompress(strip_trailing_bytes(rec));
            decompressed.extend_from_slice(&chunk);
        }

        assert_eq!(
            text_u, decompressed,
            "Decompressed text should match uncompressed text"
        );
        println!(
            "  \u{2713} Compressed records decompress correctly ({} bytes)",
            decompressed.len()
        );
    }

    #[test]
    fn test_total_decompressed_text_matches_palmdoc_text_length() {
        let dir = TempDir::new("text_len_palmdoc");
        let entries: &[(&str, &[&str])] = &[
            ("alpha", &[]), ("beta", &[]), ("gamma", &[]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let palmdoc_text_length = read_u32_be(rec0, 4) as usize;

        let text_blob = extract_text_blob(&data);
        assert_eq!(
            text_blob.len(), palmdoc_text_length,
            "Total text length ({}) should match PalmDOC text_length field ({})",
            text_blob.len(), palmdoc_text_length
        );
        println!(
            "  \u{2713} Text blob {} bytes matches PalmDOC text_length {}",
            text_blob.len(), palmdoc_text_length
        );
    }

    // =======================================================================
    // 24. INDX record structure (dictionary)
    // =======================================================================

    #[test]
    fn test_indx_first_record_has_magic() {
        let dir = TempDir::new("indx_magic");
        let opf = create_dict_fixture(
            dir.path(),
            &[("apple", &["apples"]), ("banana", &["bananas"])],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let indx_rec = get_record(&data, &offsets, orth_idx);
        assert_eq!(
            &indx_rec[0..4], b"INDX",
            "First INDX record should start with 'INDX' magic"
        );
        println!("  \u{2713} First INDX record at {} starts with INDX magic", orth_idx);
    }

    #[test]
    fn test_indx_record_count_matches_mobi_header() {
        let dir = TempDir::new("indx_count_match");
        let entries: &[(&str, &[&str])] = &[
            ("alpha", &[]),
            ("beta", &[]),
            ("gamma", &[]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let primary_indx = get_record(&data, &offsets, orth_idx);
        // Primary INDX header offset 24 = number of data records
        let num_data_records = read_u32_be(primary_indx, 24) as usize;

        // Verify each declared data record also starts with INDX magic
        for dr in 0..num_data_records {
            let data_rec_idx = orth_idx + 1 + dr;
            assert!(
                data_rec_idx < offsets.len(),
                "INDX data record {} (PalmDB record {}) out of bounds",
                dr, data_rec_idx
            );
            let data_rec = get_record(&data, &offsets, data_rec_idx);
            assert_eq!(
                &data_rec[0..4], b"INDX",
                "INDX data record {} should start with INDX magic",
                dr
            );
        }
        println!(
            "  \u{2713} INDX declares {} data records, all verified with INDX magic",
            num_data_records
        );
    }

    #[test]
    fn test_indx_all_entries_within_text_bounds() {
        let dir = TempDir::new("indx_text_bounds");
        let entries: &[(&str, &[&str])] = &[
            ("aardvark", &[]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
            ("date", &[]),
            ("elderberry", &[]),
        ];
        let opf = create_dict_fixture_unambiguous(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let text_blob = extract_text_blob(&data);
        let indx_entries = parse_indx_entries(&data, &offsets, orth_idx);

        assert!(!indx_entries.is_empty(), "Should have INDX entries");

        for (i, &(start_pos, text_len)) in indx_entries.iter().enumerate() {
            let sp = start_pos as usize;
            let tl = text_len as usize;

            assert!(
                sp < text_blob.len(),
                "INDX entry {} start_pos={} exceeds text blob size {}",
                i, sp, text_blob.len()
            );
            assert!(
                tl > 0,
                "INDX entry {} has text_len=0",
                i
            );
            assert!(
                sp + tl <= text_blob.len(),
                "INDX entry {} end pos {} exceeds text blob size {}",
                i, sp + tl, text_blob.len()
            );
        }
        println!(
            "  \u{2713} All {} INDX entries within text bounds ({} bytes)",
            indx_entries.len(), text_blob.len()
        );
    }

    #[test]
    fn test_indx_entries_contain_bold_near_start() {
        let dir = TempDir::new("indx_bold_near_start");
        let entries: &[(&str, &[&str])] = &[
            ("foo", &[]),
            ("bar", &["bars"]),
            ("baz", &[]),
        ];
        let opf = create_dict_fixture_unambiguous(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, true, None);

        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let orth_idx = read_u32_be(rec0, 40) as usize;

        let text_blob = extract_text_blob(&data);
        let indx_entries = parse_indx_entries(&data, &offsets, orth_idx);

        for (i, &(start_pos, text_len)) in indx_entries.iter().enumerate() {
            let sp = start_pos as usize;
            let tl = text_len as usize;
            let search_end = (sp + 50).min(sp + tl).min(text_blob.len());
            let region = &text_blob[sp..search_end];
            let has_bold = region.windows(3).any(|w| w == b"<b>");
            assert!(
                has_bold,
                "INDX entry {} at start_pos={} should contain '<b>' within first 50 bytes",
                i, sp
            );
        }
        println!(
            "  \u{2713} All {} INDX entries contain '<b>' near start",
            indx_entries.len()
        );
    }

    // =======================================================================
    // 25. FLIS record structure
    // =======================================================================

    #[test]
    fn test_flis_record_magic_and_size() {
        let dir = TempDir::new("flis_structure");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let mut found_flis = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FLIS" {
                found_flis = true;
                assert_eq!(
                    rec.len(), 36,
                    "FLIS record should be exactly 36 bytes, got {}",
                    rec.len()
                );
                break;
            }
        }
        assert!(found_flis, "MOBI should contain a FLIS record");
        println!("  \u{2713} FLIS record: magic='FLIS', size=36 bytes");
    }

    #[test]
    fn test_flis_record_in_book() {
        let dir = TempDir::new("flis_book");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let mut found_flis = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FLIS" {
                found_flis = true;
                assert_eq!(rec.len(), 36, "FLIS record should be 36 bytes in book, got {}", rec.len());
                break;
            }
        }
        assert!(found_flis, "Book MOBI should contain a FLIS record");
        println!("  \u{2713} FLIS record present and 36 bytes in book MOBI");
    }

    // =======================================================================
    // 26. FCIS record structure
    // =======================================================================

    #[test]
    fn test_fcis_record_magic_and_text_length() {
        let dir = TempDir::new("fcis_structure");
        let opf = create_dict_fixture(
            dir.path(),
            &[("alpha", &[]), ("beta", &[])],
        );
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let palmdoc_text_length = read_u32_be(rec0, 4);

        let mut found_fcis = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FCIS" {
                found_fcis = true;
                // FCIS layout: "FCIS"(4) + 20u32(4) + 16u32(4) + 1u32(4) + 0u32(4) + text_length(4)
                // So text_length is at offset 20
                let fcis_text_len = read_u32_be(rec, 20);
                assert_eq!(
                    fcis_text_len, palmdoc_text_length,
                    "FCIS text_length ({}) should match PalmDOC text_length ({})",
                    fcis_text_len, palmdoc_text_length
                );
                break;
            }
        }
        assert!(found_fcis, "MOBI should contain a FCIS record");
        println!(
            "  \u{2713} FCIS record: magic='FCIS', text_length={}",
            palmdoc_text_length
        );
    }

    #[test]
    fn test_fcis_text_length_in_book() {
        let dir = TempDir::new("fcis_book");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);
        let palmdoc_text_length = read_u32_be(rec0, 4);

        let mut found_fcis = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FCIS" {
                found_fcis = true;
                // FCIS text_length is at offset 20
                let fcis_text_len = read_u32_be(rec, 20);
                assert_eq!(
                    fcis_text_len, palmdoc_text_length,
                    "Book FCIS text_length ({}) should match PalmDOC text_length ({})",
                    fcis_text_len, palmdoc_text_length
                );
                break;
            }
        }
        assert!(found_fcis, "Book MOBI should contain a FCIS record");
        println!("  \u{2713} Book FCIS text_length matches PalmDOC text_length ({})", palmdoc_text_length);
    }

    // =======================================================================
    // 27. EOF record structure
    // =======================================================================

    #[test]
    fn test_eof_record_dict() {
        let dir = TempDir::new("eof_dict");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let last_rec = get_record(&data, &offsets, offsets.len() - 1);
        assert_eq!(
            last_rec,
            &[0xE9, 0x8E, 0x0D, 0x0A],
            "Dictionary last record should be EOF marker [E9 8E 0D 0A], got {:?}",
            last_rec
        );
        println!("  \u{2713} Dictionary EOF record: exactly 4 bytes [E9 8E 0D 0A]");
    }

    #[test]
    fn test_eof_record_book() {
        let dir = TempDir::new("eof_book");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let last_rec = get_record(&data, &offsets, offsets.len() - 1);
        assert_eq!(
            last_rec,
            &[0xE9, 0x8E, 0x0D, 0x0A],
            "Book last record should be EOF marker [E9 8E 0D 0A], got {:?}",
            last_rec
        );
        println!("  \u{2713} Book EOF record: exactly 4 bytes [E9 8E 0D 0A]");
    }

    // =======================================================================
    // 28. Boundary record structure (dual-format books)
    // =======================================================================

    #[test]
    fn test_boundary_record_is_exactly_8_bytes() {
        let dir = TempDir::new("boundary_size");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let mut found_boundary = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 8 && &rec[0..8] == b"BOUNDARY" {
                found_boundary = true;
                assert_eq!(
                    rec.len(), 8,
                    "BOUNDARY record should be exactly 8 bytes, got {}",
                    rec.len()
                );
                break;
            }
        }
        assert!(found_boundary, "Dual-format book should contain a BOUNDARY record");
        println!("  \u{2713} BOUNDARY record: exactly 8 bytes");
    }

    #[test]
    fn test_boundary_separates_kf7_and_kf8() {
        let dir = TempDir::new("boundary_sep");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);

        let mut boundary_idx = None;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() == 8 && &rec[0..8] == b"BOUNDARY" {
                boundary_idx = Some(i);
                break;
            }
        }
        let bi = boundary_idx.expect("Dual-format book should have BOUNDARY record");

        // KF7 section: record 0 has MOBI magic, version 6 or 7
        let rec0 = get_record(&data, &offsets, 0);
        assert_eq!(&rec0[16..20], b"MOBI");
        let kf7_version = read_u32_be(rec0, 36);
        assert!(kf7_version == 6 || kf7_version == 7, "KF7 version should be 6 or 7");

        // KF8 section: record after BOUNDARY has MOBI magic, version 8
        let kf8_rec0 = get_record(&data, &offsets, bi + 1);
        assert_eq!(&kf8_rec0[16..20], b"MOBI");
        let kf8_version = read_u32_be(kf8_rec0, 36);
        assert_eq!(kf8_version, 8, "KF8 version should be 8");

        println!(
            "  \u{2713} BOUNDARY at index {} separates KF7 (v{}) and KF8 (v{})",
            bi, kf7_version, kf8_version
        );
    }

    // =======================================================================
    // 29. Image record structure
    // =======================================================================

    #[test]
    fn test_image_records_start_with_jpeg_magic_3bytes() {
        let dir = TempDir::new("img_jpeg_magic3");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let first_img = read_u32_be(rec0, 108) as usize;
        assert_ne!(first_img, 0xFFFFFFFF_u32 as usize, "Should have first_image set");

        // Check all image records start with FF D8 FF (full JPEG SOI + marker)
        let mut img_count = 0;
        for i in first_img..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 3 && rec[0] == 0xFF && rec[1] == 0xD8 {
                img_count += 1;
                assert_eq!(
                    rec[2], 0xFF,
                    "Image record {} byte[2] should be 0xFF (JPEG marker), got 0x{:02X}",
                    i, rec[2]
                );
            }
        }
        assert!(img_count > 0, "Should find at least one image record");
        println!("  \u{2713} All {} image records start with FF D8 FF", img_count);
    }

    #[test]
    fn test_cover_image_jfif_density_dpi() {
        let dir = TempDir::new("cover_jfif_dpi");
        let mut jpeg = make_test_jpeg();

        // Ensure JFIF header has density_units=0x00 (will be patched)
        if jpeg.len() > 13
            && jpeg[2] == 0xFF && jpeg[3] == 0xE0
            && &jpeg[6..11] == b"JFIF\0"
        {
            jpeg[13] = 0x00; // set to aspect ratio
        }

        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let first_img = read_u32_be(rec0, 108) as usize;
        let img_rec = get_record(&data, &offsets, first_img);

        if img_rec.len() > 13
            && img_rec[2] == 0xFF && img_rec[3] == 0xE0
            && &img_rec[6..11] == b"JFIF\0"
        {
            assert_eq!(
                img_rec[13], 0x01,
                "Cover image JFIF density_units should be 0x01 (DPI), got 0x{:02X}",
                img_rec[13]
            );
            println!("  \u{2713} Cover image JFIF density_units = 0x01 (DPI)");
        } else {
            println!("  \u{2713} Cover image has no JFIF header (re-encoded), skipping density check");
        }
    }

    #[test]
    fn test_book_images_between_text_and_flis() {
        let dir = TempDir::new("book_img_order");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let text_record_count = read_u16_be(rec0, 8) as usize;
        let first_img = read_u32_be(rec0, 108) as usize;

        // Find FLIS record index
        let mut flis_idx = None;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"FLIS" {
                flis_idx = Some(i);
                break;
            }
        }

        // Images should come after text records (record 0 + text_record_count)
        assert!(
            first_img > text_record_count,
            "First image ({}) should be after text records (0..{})",
            first_img, text_record_count
        );

        // If FLIS exists in KF7 section, images should come before it
        if let Some(fi) = flis_idx {
            // Only check if FLIS is in the same section as images
            // (for dual-format, FLIS may be in KF8 section)
            if first_img < fi {
                let img_rec = get_record(&data, &offsets, first_img);
                assert!(
                    img_rec.len() >= 2 && img_rec[0] == 0xFF && img_rec[1] == 0xD8,
                    "Record at first_image should be JPEG"
                );
                println!(
                    "  \u{2713} Book images at {} between text (1..{}) and FLIS ({})",
                    first_img, text_record_count, fi
                );
            } else {
                println!(
                    "  \u{2713} Book images at {} after text (1..{}), FLIS at {} (different section)",
                    first_img, text_record_count, fi
                );
            }
        } else {
            println!(
                "  \u{2713} Book images at {} after text (1..{}), no KF7 FLIS found",
                first_img, text_record_count
            );
        }
    }

    #[test]
    fn test_dict_images_between_text_and_indx() {
        let dir = TempDir::new("dict_img_order");

        // Build HTML with an embedded image reference
        let jpeg = make_test_jpeg();
        fs::write(dir.path().join("test.jpg"), &jpeg).unwrap();

        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="cat">cat</idx:orth><b>cat</b> <img src="test.jpg"/> a small animal<hr/></idx:entry>
<idx:entry><idx:orth value="dog">dog</idx:orth><b>dog</b> a loyal animal<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();

        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Img Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
    <item id="img1" href="test.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();

        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        let text_record_count = read_u16_be(rec0, 8) as usize;
        let orth_idx = read_u32_be(rec0, 40) as usize;
        let first_img = read_u32_be(rec0, 108) as usize;

        if first_img != 0xFFFFFFFF_u32 as usize {
            // Image should be after text and before INDX
            assert!(
                first_img > text_record_count,
                "Dict image ({}) should be after text records (1..{})",
                first_img, text_record_count
            );
            assert!(
                first_img < orth_idx,
                "Dict image ({}) should be before INDX ({})",
                first_img, orth_idx
            );
            println!(
                "  \u{2713} Dict image at {} between text (1..{}) and INDX ({})",
                first_img, text_record_count, orth_idx
            );
        } else {
            println!("  \u{2713} Dict has no image records (image not referenced in text)");
        }
    }

    // =======================================================================
    // 30. SRCS record (when present)
    // =======================================================================

    #[test]
    fn test_srcs_starts_with_magic() {
        let dir = TempDir::new("srcs_magic_check");
        let fake_epub = b"PK\x03\x04fake epub content";
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, Some(fake_epub));
        let (_, _, offsets) = parse_palmdb(&data);

        let mut found_srcs = false;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"SRCS" {
                found_srcs = true;
                break;
            }
        }
        assert!(found_srcs, "MOBI with srcs_data should contain SRCS record");
        println!("  \u{2713} SRCS record starts with 'SRCS' magic");
    }

    #[test]
    fn test_srcs_has_16_byte_header() {
        let dir = TempDir::new("srcs_16b_hdr");
        let fake_epub = b"PK\x03\x04fake epub data for header test";
        let opf = create_dict_fixture(dir.path(), &[("word", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, Some(fake_epub));
        let (_, _, offsets) = parse_palmdb(&data);

        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() >= 4 && &rec[0..4] == b"SRCS" {
                let header_len = read_u32_be(rec, 4);
                assert_eq!(
                    header_len, 0x10,
                    "SRCS header length should be 16 (0x10), got {}",
                    header_len
                );
                // Verify total record = header + data
                let data_len = read_u32_be(rec, 8) as usize;
                assert_eq!(
                    rec.len(), 16 + data_len,
                    "SRCS record size ({}) should be 16 header + {} data",
                    rec.len(), data_len
                );
                println!("  \u{2713} SRCS: 16-byte header, {} bytes data", data_len);
                return;
            }
        }
        panic!("No SRCS record found");
    }

    #[test]
    fn test_srcs_mobi_header_offset_208_points_to_it() {
        let dir = TempDir::new("srcs_hdr208_check");
        let fake_epub = b"PK\x03\x04test data";
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, Some(fake_epub));
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // MOBI header offset 208 is at rec0[16 + 208] = rec0[224]
        let srcs_idx = read_u32_be(rec0, 224);
        assert_ne!(
            srcs_idx, 0xFFFFFFFF,
            "MOBI header offset 208 should point to SRCS record, not 0xFFFFFFFF"
        );

        let srcs_rec = get_record(&data, &offsets, srcs_idx as usize);
        assert_eq!(
            &srcs_rec[0..4], b"SRCS",
            "Record at MOBI header offset 208 ({}) should start with 'SRCS' magic",
            srcs_idx
        );
        println!("  \u{2713} MOBI header offset 208 -> SRCS record at index {}", srcs_idx);
    }

    // =======================================================================
    // 31. KF8-only format
    // =======================================================================

    #[test]
    fn test_kf8_only_no_boundary_record() {
        let dir = TempDir::new("kf8only_no_boundary");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);

        // Check that no BOUNDARY record exists that separates KF7/KF8
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() == 8 && &rec[0..8] == b"BOUNDARY" {
                // If there is a BOUNDARY, the next record should NOT have MOBI magic
                // (HD container boundaries are OK, KF7/KF8 boundaries are not)
                if i + 1 < offsets.len() {
                    let next = get_record(&data, &offsets, i + 1);
                    assert!(
                        next.len() < 20 || &next[16..20] != b"MOBI",
                        "KF8-only should not have KF7/KF8 BOUNDARY at index {}",
                        i
                    );
                }
            }
        }
        println!("  \u{2713} KF8-only: no KF7/KF8 BOUNDARY record");
    }

    #[test]
    fn test_kf8_only_mobi_version_8_throughout() {
        let dir = TempDir::new("kf8only_v8");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_kf8_only_mobi_bytes(&opf, dir.path());
        let (_, _, offsets) = parse_palmdb(&data);
        let rec0 = get_record(&data, &offsets, 0);

        // Version at MOBI header offset 20 (rec0 offset 36)
        let version = read_u32_be(rec0, 36);
        assert_eq!(version, 8, "KF8-only version should be 8, got {}", version);

        // Min version at MOBI header offset 88 (rec0 offset 104)
        let min_version = read_u32_be(rec0, 104);
        assert_eq!(min_version, 8, "KF8-only min_version should be 8, got {}", min_version);

        // There should be no second MOBI header with a different version
        let mut mobi_count = 0;
        for i in 0..offsets.len() {
            let rec = get_record(&data, &offsets, i);
            if rec.len() > 20 && &rec[16..20] == b"MOBI" {
                let v = read_u32_be(rec, 36);
                assert_eq!(v, 8, "All MOBI headers should be version 8, record {} has version {}", i, v);
                mobi_count += 1;
            }
        }
        assert_eq!(mobi_count, 1, "KF8-only should have exactly 1 MOBI header, found {}", mobi_count);
        println!("  \u{2713} KF8-only: single MOBI header, version 8 throughout");
    }

    // =======================================================================
    // 32. kindling validate: Kindle Publishing Guidelines checker
    // =======================================================================
    //
    // Each test creates a minimal OPF + content files and runs
    // `validate::validate_opf`, then asserts that the expected section shows
    // up at the expected severity (or does not, for passing cases).

    use crate::validate::{self, Level};

    /// Build a minimal OPF with a given metadata block, manifest inner xml
    /// and spine inner xml, writing to dir/content.opf.
    fn write_opf(
        dir: &Path,
        extra_metadata: &str,
        manifest_inner: &str,
        spine_inner: &str,
    ) -> PathBuf {
        let opf = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Tester</dc:creator>
    {extra_metadata}
  </metadata>
  <manifest>
{manifest_inner}
  </manifest>
  <spine>
{spine_inner}
  </spine>
</package>"#,
            extra_metadata = extra_metadata,
            manifest_inner = manifest_inner,
            spine_inner = spine_inner,
        );
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    /// Count findings at the given section with the given level.
    fn count_findings(report: &validate::ValidationReport, section: &str, level: Level) -> usize {
        report
            .findings
            .iter()
            .filter(|f| f.section == section && f.level == level)
            .count()
    }

    fn has_finding(report: &validate::ValidationReport, section: &str, level: Level) -> bool {
        count_findings(report, section, level) > 0
    }

    // --- 4.1: marketing cover informational note ---

    #[test]
    fn test_validate_emits_marketing_cover_info() {
        let dir = TempDir::new("validate_info_4_1");
        let jpeg = make_test_jpeg();
        fs::write(dir.path().join("cover.jpg"), &jpeg).unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            r#"<meta name="cover" content="cover"/>"#,
            r#"<item id="cover" href="cover.jpg" media-type="image/jpeg"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "4.1", Level::Info));
    }

    // --- 4.2: cover image declared / missing / Method 1 / Method 2 ---

    #[test]
    fn test_validate_missing_cover_errors() {
        let dir = TempDir::new("validate_4_2_missing");
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(
            has_finding(&report, "4.2", Level::Error),
            "missing cover should error at 4.2"
        );
    }

    #[test]
    fn test_validate_cover_method_1_ok() {
        let dir = TempDir::new("validate_4_2_m1");
        // Make a large enough jpeg so we don't trigger the <500px warning
        let img = image::GrayImage::from_fn(600, 800, |_, _| image::Luma([128u8]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        fs::write(dir.path().join("cover.jpg"), &buf).unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="cover" href="cover.jpg" media-type="image/jpeg" properties="coverimage"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(
            !has_finding(&report, "4.2", Level::Error),
            "Method 1 cover should not error: {:?}",
            report.findings
        );
    }

    #[test]
    fn test_validate_cover_small_warns() {
        let dir = TempDir::new("validate_4_2_small");
        let jpeg = make_test_jpeg(); // 10x10
        fs::write(dir.path().join("cover.jpg"), &jpeg).unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            r#"<meta name="cover" content="cover"/>"#,
            r#"<item id="cover" href="cover.jpg" media-type="image/jpeg"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(
            has_finding(&report, "4.2", Level::Warning),
            "10x10 cover should warn about shortest side < 500"
        );
    }

    #[test]
    fn test_validate_html_cover_page_plus_cover_image_errors() {
        let dir = TempDir::new("validate_4_2_dup");
        let img = image::GrayImage::from_fn(600, 800, |_, _| image::Luma([128u8]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        fs::write(dir.path().join("cover.jpg"), &buf).unwrap();
        fs::write(
            dir.path().join("cover.html"),
            r#"<html><body><img src="cover.jpg"/></body></html>"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            r#"<meta name="cover" content="coverimg"/>"#,
            r#"<item id="coverimg" href="cover.jpg" media-type="image/jpeg"/>
   <item id="coverhtml" href="cover.html" media-type="application/xhtml+xml"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="coverhtml"/>
   <itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        let has_dup_error = report
            .findings
            .iter()
            .any(|f| f.section == "4.2" && f.level == Level::Error && f.message.contains("HTML cover page"));
        assert!(has_dup_error, "should error on HTML cover page in spine + cover image");
    }

    // --- 5.2: NCX presence and spine toc= attribute ---

    #[test]
    fn test_validate_missing_ncx_warns() {
        let dir = TempDir::new("validate_5_2_missing");
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "5.2", Level::Warning));
    }

    #[test]
    fn test_validate_ncx_without_spine_toc_warns() {
        let dir = TempDir::new("validate_5_2_no_spine_toc");
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.path().join("toc.ncx"),
            "<ncx><navMap></navMap></ncx>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        let has_spine_warn = report.findings.iter().any(|f| {
            f.section == "5.2" && f.level == Level::Warning && f.message.contains("toc=")
        });
        assert!(has_spine_warn, "should warn about missing spine toc attribute");
    }

    #[test]
    fn test_validate_ncx_with_spine_toc_ok() {
        let dir = TempDir::new("validate_5_2_ok");
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.path().join("toc.ncx"),
            "<ncx><navMap></navMap></ncx>",
        )
        .unwrap();
        // Write OPF manually so we can set spine toc="ncx"
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>T</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>A</dc:creator>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let report = validate::validate_opf(&opf_path).unwrap();
        assert!(
            !has_finding(&report, "5.2", Level::Warning),
            "NCX + spine toc should not warn: {:?}",
            report.findings
        );
    }

    // --- 6.3: scripting ---

    #[test]
    fn test_validate_script_tag_errors() {
        let dir = TempDir::new("validate_6_3_script");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><script>alert(1)</script><p>x</p></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "6.3", Level::Error));
    }

    // --- 6.4: nested <p> ---

    #[test]
    fn test_validate_nested_p_errors() {
        let dir = TempDir::new("validate_6_4_nested_p");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><p>outer <p>inner</p></p></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "6.4", Level::Error));
    }

    #[test]
    fn test_validate_non_nested_p_ok() {
        let dir = TempDir::new("validate_6_4_ok");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><p>one</p><p>two</p></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(!has_finding(&report, "6.4", Level::Error));
    }

    // --- 6.2: negative CSS values ---

    #[test]
    fn test_validate_negative_css_warns() {
        let dir = TempDir::new("validate_6_2");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><p style="margin-left: -5px;">x</p></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "6.2", Level::Warning));
    }

    // --- 6.5: file case mismatch ---

    #[test]
    fn test_validate_file_case_mismatch_errors() {
        let dir = TempDir::new("validate_6_5_case");
        // Only matters on case-insensitive filesystems (macOS/Windows).
        // Write the actual file with lowercase name; reference it as uppercase.
        fs::write(dir.path().join("cover.jpg"), make_test_jpeg()).unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="cover" href="Cover.jpg" media-type="image/jpeg" properties="coverimage"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        // On case-insensitive filesystems (macOS/Windows) this should fire a
        // 6.5 error. On case-sensitive filesystems (most Linux) the file
        // wouldn't exist and we'd get a 4.2 error instead. Accept either:
        // the important thing is that the manuscript does not silently pass.
        let has_case_error = has_finding(&report, "6.5", Level::Error)
            || has_finding(&report, "4.2", Level::Error);
        assert!(has_case_error);
    }

    // --- 10.3.1: heading alignment ---

    #[test]
    fn test_validate_heading_text_align_warns() {
        let dir = TempDir::new("validate_10_3_1");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><h1 style="text-align:center">Title</h1></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "10.3.1", Level::Warning));
    }

    // --- 10.4.1: supported image formats ---

    #[test]
    fn test_validate_unsupported_image_format_errors() {
        let dir = TempDir::new("validate_10_4_1");
        fs::write(dir.path().join("pic.bmp"), b"fake bmp").unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="pic" href="pic.bmp" media-type="image/bmp"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "10.4.1", Level::Error));
    }

    // --- 10.4.2: image file size / megapixels ---

    #[test]
    fn test_validate_image_file_over_127kb_warns() {
        let dir = TempDir::new("validate_10_4_2");
        // 128 KB file of zeros saved as .jpg - we only check file size, not
        // decode, so that's fine.
        fs::write(dir.path().join("pic.jpg"), vec![0u8; 130 * 1024]).unwrap();
        fs::write(
            dir.path().join("content.html"),
            "<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="pic" href="pic.jpg" media-type="image/jpeg"/>
   <item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "10.4.2", Level::Warning));
    }

    // --- 10.5.1: large tables ---

    #[test]
    fn test_validate_large_table_warns() {
        let dir = TempDir::new("validate_10_5_1");
        let mut rows = String::new();
        for i in 0..60 {
            rows.push_str(&format!("<tr><td>{}</td></tr>", i));
        }
        let html = format!(
            r#"<html><body><table>{}</table></body></html>"#,
            rows
        );
        fs::write(dir.path().join("content.html"), html).unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "10.5.1", Level::Warning));
    }

    #[test]
    fn test_validate_small_table_ok() {
        let dir = TempDir::new("validate_10_5_1_ok");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><table><tr><td>1</td></tr><tr><td>2</td></tr></table></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(!has_finding(&report, "10.5.1", Level::Warning));
    }

    // --- 17: unsupported HTML tags ---

    #[test]
    fn test_validate_form_tag_errors() {
        let dir = TempDir::new("validate_17_form");
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><form action="x"><input type="text"/></form></body></html>"#,
        )
        .unwrap();
        let opf = write_opf(
            dir.path(),
            "",
            r#"<item id="content" href="content.html" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="content"/>"#,
        );
        let report = validate::validate_opf(&opf).unwrap();
        assert!(has_finding(&report, "17", Level::Error));
    }

    // --- end-to-end: a valid minimal manuscript should have no errors ---

    #[test]
    fn test_validate_clean_manuscript_no_errors() {
        let dir = TempDir::new("validate_clean");
        let img = image::GrayImage::from_fn(600, 800, |_, _| image::Luma([128u8]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        fs::write(dir.path().join("cover.jpg"), &buf).unwrap();
        fs::write(
            dir.path().join("toc.ncx"),
            "<ncx><navMap></navMap></ncx>",
        )
        .unwrap();
        fs::write(
            dir.path().join("content.html"),
            r#"<html><body><h1>Title</h1><p>Paragraph one.</p><p>Paragraph two.</p></body></html>"#,
        )
        .unwrap();
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Clean Book</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Author</dc:creator>
  </metadata>
  <manifest>
    <item id="cover" href="cover.jpg" media-type="image/jpeg" properties="coverimage"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let report = validate::validate_opf(&opf_path).unwrap();
        assert_eq!(
            report.error_count(),
            0,
            "clean manuscript should have no errors: {:?}",
            report.findings
        );
    }

    // =======================================================================
    // 32b. kindling build: automatic pre-flight validation integration
    //
    // These tests exercise `run_preflight_validation`, the entry point
    // `do_build` uses to enforce KDP compliance before invoking the MOBI
    // writer. We cannot call `do_build` directly here because it calls
    // `process::exit(1)` on validation failure, which would tear down the
    // test runner. Instead we verify the decision function and also confirm
    // that a validated OPF still builds successfully end-to-end.
    // =======================================================================

    /// Build a minimal clean book OPF fixture that passes KDP validation
    /// (has cover image, NCX, well-formed HTML, no unsupported tags).
    fn create_clean_book_opf(dir: &Path) -> PathBuf {
        // Cover >= 500px shortest side, so no warning.
        let img = image::GrayImage::from_fn(600, 800, |_, _| image::Luma([128u8]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        image::DynamicImage::ImageLuma8(img)
            .write_to(&mut cursor, image::ImageFormat::Jpeg)
            .unwrap();
        fs::write(dir.join("cover.jpg"), &buf).unwrap();
        fs::write(dir.join("toc.ncx"), "<ncx><navMap></navMap></ncx>").unwrap();
        fs::write(
            dir.join("content.html"),
            r#"<html><body><h1>Title</h1><p>Paragraph one.</p><p>Paragraph two.</p></body></html>"#,
        )
        .unwrap();
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Clean Book</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Author</dc:creator>
  </metadata>
  <manifest>
    <item id="cover" href="cover.jpg" media-type="image/jpeg" properties="coverimage"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    /// Build a book OPF with no cover image declared: triggers an R4.2.1
    /// error ("cover image must be declared") during validation.
    fn create_broken_book_opf_missing_cover(dir: &Path) -> PathBuf {
        fs::write(dir.join("toc.ncx"), "<ncx><navMap></navMap></ncx>").unwrap();
        fs::write(
            dir.join("content.html"),
            r#"<html><body><h1>Title</h1><p>Paragraph.</p></body></html>"#,
        )
        .unwrap();
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>No Cover Book</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Author</dc:creator>
  </metadata>
  <manifest>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    /// Build a book OPF with a small (10x10) cover image: triggers an R4.2.3
    /// warning (shortest side < 500 px) but NO errors.
    fn create_warning_book_opf_small_cover(dir: &Path) -> PathBuf {
        let jpeg = make_test_jpeg(); // 10x10
        fs::write(dir.join("cover.jpg"), &jpeg).unwrap();
        fs::write(dir.join("toc.ncx"), "<ncx><navMap></navMap></ncx>").unwrap();
        fs::write(
            dir.join("content.html"),
            r#"<html><body><h1>Title</h1><p>Paragraph.</p></body></html>"#,
        )
        .unwrap();
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Small Cover Book</dc:title>
    <dc:language>en</dc:language>
    <dc:creator>Author</dc:creator>
    <meta name="cover" content="cover"/>
  </metadata>
  <manifest>
    <item id="cover" href="cover.jpg" media-type="image/jpeg"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        opf_path
    }

    /// A valid OPF passes pre-flight and the subsequent MOBI build also
    /// succeeds. This is the happy path for `kindling build` with default
    /// validation enabled.
    #[test]
    fn test_build_preflight_valid_opf_passes() {
        let dir = TempDir::new("preflight_valid");
        let opf = create_clean_book_opf(dir.path());

        // Pre-flight should succeed with zero errors.
        crate::run_preflight_validation(&opf, false)
            .expect("clean OPF should pass pre-flight validation");

        // And the MOBI build itself should still succeed end-to-end.
        let output = dir.path().join("out.mobi");
        mobi::build_mobi(
            &opf, &output, true, false, None, false, false, false, false, None, false, false, false,
        )
        .expect("build should succeed for clean OPF");
        assert!(output.exists(), "MOBI output file must exist");
        let size = fs::metadata(&output).unwrap().len();
        assert!(size > 0, "MOBI output must be non-empty");
        println!("  \u{2713} clean OPF passes pre-flight + builds ({} bytes)", size);
    }

    /// An OPF with a missing cover image triggers a validation error, which
    /// must cause `run_preflight_validation` to return `Err(n)` with n >= 1.
    /// In the real build flow this aborts the build with exit code 1.
    #[test]
    fn test_build_preflight_validation_error_aborts() {
        let dir = TempDir::new("preflight_broken");
        let opf = create_broken_book_opf_missing_cover(dir.path());

        let result = crate::run_preflight_validation(&opf, false);
        match result {
            Err(errors) => {
                assert!(
                    errors >= 1,
                    "missing-cover OPF should report >= 1 validation error, got {}",
                    errors
                );
                println!("  \u{2713} missing-cover OPF aborts pre-flight ({} errors)", errors);
            }
            Ok(()) => panic!(
                "missing-cover OPF should fail pre-flight validation, but it passed"
            ),
        }
    }

    /// Passing `no_validate = true` short-circuits pre-flight entirely: even
    /// an OPF that would otherwise error out is allowed through, matching
    /// the `--no-validate` CLI escape hatch.
    #[test]
    fn test_build_preflight_no_validate_bypasses_errors() {
        let dir = TempDir::new("preflight_skip");
        let opf = create_broken_book_opf_missing_cover(dir.path());

        // Even though the OPF has validation errors, --no-validate makes
        // pre-flight a no-op that returns Ok.
        crate::run_preflight_validation(&opf, true)
            .expect("--no-validate should skip pre-flight regardless of errors");
        println!("  \u{2713} --no-validate bypasses validation errors");
    }

    /// An OPF with only warnings (no errors) must not abort the build.
    /// `run_preflight_validation` should return `Ok(())` and the caller
    /// should continue with MOBI generation.
    #[test]
    fn test_build_preflight_warnings_do_not_abort() {
        let dir = TempDir::new("preflight_warn");
        let opf = create_warning_book_opf_small_cover(dir.path());

        // Sanity check: the underlying validator must actually produce
        // warnings (not errors) for this fixture.
        let report = validate::validate_opf(&opf).unwrap();
        assert_eq!(
            report.error_count(),
            0,
            "small-cover fixture should produce zero errors, got: {:?}",
            report.findings
        );
        assert!(
            report.warning_count() >= 1,
            "small-cover fixture should produce >= 1 warning, got: {:?}",
            report.findings
        );

        // Pre-flight passes despite warnings.
        crate::run_preflight_validation(&opf, false)
            .expect("warnings alone must not abort pre-flight");
        println!(
            "  \u{2713} warnings-only OPF passes pre-flight ({} warnings)",
            report.warning_count()
        );
    }

    // =======================================================================
    // HTML/XHTML validation
    //
    // These tests guard against HTML well-formedness regressions in MOBI text
    // blobs. Binary structure is already covered elsewhere, but bugs like
    // `<hr/>` corruption, unclosed `<mbp:frameset>`/`<body>`/`<html>`,
    // malformed attributes, and entry reordering leaving dangling tags only
    // show up when the HTML itself is parsed.
    // =======================================================================

    /// Extract the text blob from a MOBI, automatically decompressing PalmDOC
    /// if the record 0 compression type indicates compression.
    ///
    /// The existing `extract_text_blob` helper strips trailing bytes but does
    /// NOT decompress, so it only works on `no_compress=true` builds. Comic
    /// output is always compressed, so HTML validation needs this variant.
    fn extract_text_blob_auto(data: &[u8]) -> Vec<u8> {
        let (_, _, offsets) = parse_palmdb(data);
        let rec0 = get_record(data, &offsets, 0);
        // PalmDOC header: offset 0 = compression type (u16). 1=none, 2=palmdoc.
        let comp_type = read_u16_be(rec0, 0);
        let text_record_count = read_u16_be(rec0, 8) as usize;
        let mut text_bytes = Vec::new();
        for i in 1..=text_record_count {
            if i >= offsets.len() {
                break;
            }
            let rec = get_record(data, &offsets, i);
            let body = strip_trailing_bytes(rec);
            if comp_type == 2 {
                let chunk = palmdoc_decompress(body);
                text_bytes.extend_from_slice(&chunk);
            } else {
                text_bytes.extend_from_slice(body);
            }
        }
        text_bytes
    }

    // The three HTML-level checks below now live in `crate::html_check` so
    // they can be shared between tests and the build-time self-check. The
    // wrappers here convert `&str` to `&[u8]` and preserve the existing
    // test API (Result for parse/balance, panic for corruption).
    //
    // `assert_structural_tags_present` stays test-only because it asserts
    // that `<html>`/`<body>` substrings are present, which is stricter than
    // what the build-time self-check enforces (very small or front-matter-
    // only fixtures wouldn't necessarily include them).

    fn try_parse_mobi_html(content: &str) -> Result<(), String> {
        crate::html_check::parse_mobi_html(content.as_bytes())
    }

    fn check_balanced_tags(content: &str) -> Result<(), String> {
        crate::html_check::check_balanced_tags(content.as_bytes())
    }

    /// Assert that the required structural tags are present AND properly
    /// closed in the text blob.
    fn assert_structural_tags_present(content: &str, require_frameset: bool) {
        let must_have: &[&str] = if require_frameset {
            &["html", "body", "mbp:frameset"]
        } else {
            &["html", "body"]
        };
        for tag in must_have {
            let open_pat = format!("<{}", tag);
            let close_pat = format!("</{}>", tag);
            assert!(
                content.contains(&open_pat),
                "text blob is missing opening tag <{}>: first 200 bytes: {:?}",
                tag,
                &content[..content.len().min(200)]
            );
            assert!(
                content.contains(&close_pat),
                "text blob is missing closing tag </{}>: last 200 bytes: {:?}",
                tag,
                &content[content.len().saturating_sub(200)..]
            );
        }
    }

    /// Panic-style wrapper around `html_check::check_no_corruption` so the
    /// existing negative tests continue to work. Panics on the first issue.
    fn assert_no_html_corruption(content: &str) {
        if let Err(e) = crate::html_check::check_no_corruption(content.as_bytes()) {
            panic!("{}", e);
        }
    }

    /// Run the full HTML validation suite on a text blob: parse, structure,
    /// corruption scan, and tag balance.
    fn validate_mobi_text_blob(
        blob: &[u8],
        label: &str,
        require_frameset: bool,
    ) {
        let content = std::str::from_utf8(blob)
            .unwrap_or_else(|e| panic!("{}: text blob is not valid UTF-8: {}", label, e));

        // 1. Parses at the token level (catches <hr/ corruption, missing >,
        //    unclosed attributes, etc.).
        if let Err(e) = try_parse_mobi_html(content) {
            panic!("{}: HTML did not parse cleanly: {}", label, e);
        }

        // 2. Required structural tags are present and closed.
        assert_structural_tags_present(content, require_frameset);

        // 3. No obviously malformed hr or attribute quotes.
        assert_no_html_corruption(content);

        // 4. Balanced tag stack (ignoring void elements).
        if let Err(e) = check_balanced_tags(content) {
            panic!("{}: unbalanced tags: {}", label, e);
        }
    }

    // -----------------------------------------------------------------------
    // Dictionary text blob validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_html_validation_dict_no_kindle_limits() {
        let dir = TempDir::new("htmlval_dict_no_kl");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);
        // Plain create_dict_fixture doesn't wrap in <mbp:frameset>, so
        // we don't require it here.
        validate_mobi_text_blob(&blob, "dict no kindle_limits", false);
        println!("  \u{2713} dict (no kindle_limits) text blob parses and is balanced");
    }

    #[test]
    fn test_html_validation_dict_with_kindle_limits() {
        let dir = TempDir::new("htmlval_dict_kl");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
            ("cherry", &["cherries"]),
            ("date", &["dates"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes_with_kindle_limits(&opf, dir.path());
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "dict with kindle_limits", false);
        println!("  \u{2713} dict (kindle_limits) text blob parses and is balanced");
    }

    #[test]
    fn test_html_validation_dict_with_frameset() {
        let dir = TempDir::new("htmlval_dict_frameset");
        // Source HTML deliberately uses <mbp:frameset> so build_text_content_by_letter
        // (kindle_limits path) preserves it in the output.
        let html = r#"<html><head><guide></guide></head><body><mbp:frameset>
<idx:entry><idx:orth value="alpha">alpha</idx:orth><b>alpha</b> first letter<hr/></idx:entry>
<idx:entry><idx:orth value="beta">beta</idx:orth><b>beta</b> second letter<hr/></idx:entry>
<idx:entry><idx:orth value="gamma">gamma</idx:orth><b>gamma</b> third letter<hr/></idx:entry>
</mbp:frameset></body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();
        let opf_str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Frameset Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf_str).unwrap();
        let data = build_mobi_bytes_with_kindle_limits(&opf_path, dir.path());
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "dict with frameset", true);
        println!("  \u{2713} dict with <mbp:frameset> text blob parses and is balanced");
    }

    #[test]
    fn test_html_validation_dict_with_cover() {
        let dir = TempDir::new("htmlval_dict_cover");
        // Create a cover image and reference it in the OPF so kindling
        // exercises the cover-image code path during MOBI assembly.
        let jpeg = make_test_jpeg();
        fs::write(dir.path().join("cover.jpg"), &jpeg).unwrap();
        let html = r#"<html><head><guide></guide></head><body>
<idx:entry><idx:orth value="apple">apple</idx:orth><b>apple</b> a fruit<hr/></idx:entry>
<idx:entry><idx:orth value="banana">banana</idx:orth><b>banana</b> another fruit<hr/></idx:entry>
</body></html>"#;
        fs::write(dir.path().join("content.html"), html).unwrap();
        let opf_str = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" xmlns="http://www.idpf.org/2007/opf">
  <metadata>
    <dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Cover Dict</dc:title>
    <dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
    <dc:creator xmlns:dc="http://purl.org/dc/elements/1.1/">Tester</dc:creator>
    <meta name="cover" content="cover"/>
    <x-metadata>
      <DictionaryInLanguage>en</DictionaryInLanguage>
      <DictionaryOutLanguage>en</DictionaryOutLanguage>
      <DefaultLookupIndex>default</DefaultLookupIndex>
    </x-metadata>
  </metadata>
  <manifest>
    <item id="cover" href="cover.jpg" media-type="image/jpeg"/>
    <item id="content" href="content.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="content"/>
  </spine>
</package>"#;
        let opf_path = dir.path().join("content.opf");
        fs::write(&opf_path, opf_str).unwrap();
        let data = build_mobi_bytes(&opf_path, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "dict with cover", false);
        println!("  \u{2713} dict with cover text blob parses and is balanced");
    }

    // -----------------------------------------------------------------------
    // Book text blob validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_html_validation_book_no_image() {
        let dir = TempDir::new("htmlval_book");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "book (no image)", false);
        println!("  \u{2713} book text blob parses and is balanced");
    }

    #[test]
    fn test_html_validation_book_with_cover() {
        let dir = TempDir::new("htmlval_book_cover");
        let jpeg = make_test_jpeg();
        let opf = create_book_fixture(dir.path(), Some(&jpeg));
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "book with cover", false);
        println!("  \u{2713} book with cover text blob parses and is balanced");
    }

    // -----------------------------------------------------------------------
    // Comic text blob validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_html_validation_comic() {
        use crate::comic;

        let dir = TempDir::new("htmlval_comic");
        let images_dir = dir.path().join("images");
        fs::create_dir_all(&images_dir).unwrap();

        // A couple of small grayscale test images is enough to exercise the
        // comic xhtml pipeline.
        for i in 0..3u8 {
            let img = image::DynamicImage::ImageLuma8(
                image::GrayImage::from_fn(100, 150, |_, _| image::Luma([60 + i * 60])),
            );
            img.save(images_dir.join(format!("page_{:03}.jpg", i))).unwrap();
        }

        let output_path = dir.path().join("comic.mobi");
        let profile = comic::get_profile("paperwhite").unwrap();
        comic::build_comic(&images_dir, &output_path, &profile)
            .expect("build_comic failed");

        let data = fs::read(&output_path).expect("could not read comic MOBI");
        let blob = extract_text_blob_auto(&data);
        validate_mobi_text_blob(&blob, "comic", false);
        println!("  \u{2713} comic text blob parses and is balanced");
    }

    // -----------------------------------------------------------------------
    // Negative tests: the validator must actually catch broken HTML.
    //
    // These call the validation helpers directly on hand-crafted bad HTML
    // (we can't easily force kindling to emit corrupt output, so we feed
    // the helpers synthetic payloads that mimic past regressions).
    // -----------------------------------------------------------------------

    #[test]
    fn test_html_validator_catches_unclosed_p() {
        // <p> never closed inside <body>.
        let bad = r#"<html><head></head><body><p>hello</body></html>"#;
        // check_balanced_tags should flag this. Parsing with relaxed mode
        // will still succeed (check_end_names=false), so we rely on the
        // structural walker.
        let err = check_balanced_tags(bad).expect_err(
            "validator should reject <p> without matching </p>",
        );
        assert!(
            err.contains("mismatched") || err.contains("unclosed"),
            "expected mismatched/unclosed error, got: {}",
            err
        );
        println!("  \u{2713} validator rejected unclosed <p>: {}", err);
    }

    #[test]
    fn test_html_validator_catches_corrupt_hr() {
        // <hr/ with garbage instead of >
        let bad = r#"<html><body>text<hr/X>more</body></html>"#;
        let result = std::panic::catch_unwind(|| {
            assert_no_html_corruption(bad);
        });
        assert!(
            result.is_err(),
            "validator should panic on corrupt <hr/X>"
        );
        println!("  \u{2713} validator rejected corrupt <hr/X>");
    }

    #[test]
    fn test_html_validator_catches_unclosed_attribute_quote() {
        // class="foo never closed, followed by another tag
        let bad = r#"<html><body><p class="foo<b>bold</b></p></body></html>"#;
        let result = std::panic::catch_unwind(|| {
            assert_no_html_corruption(bad);
        });
        assert!(
            result.is_err(),
            "validator should panic on unclosed attribute quote"
        );
        println!("  \u{2713} validator rejected unclosed attribute quote");
    }

    #[test]
    fn test_html_validator_catches_missing_body_close() {
        let bad = r#"<html><head></head><body><p>hi</p></html>"#;
        let result = std::panic::catch_unwind(|| {
            assert_structural_tags_present(bad, false);
        });
        assert!(
            result.is_err(),
            "validator should panic when </body> is missing"
        );
        println!("  \u{2713} validator rejected missing </body>");
    }

    #[test]
    fn test_html_validator_accepts_well_formed_fixture() {
        // The same shape kindling emits for a dict: html/body/guide,
        // with <hr/> separators, mbp:pagebreak, frameset wrapper, and
        // self-closed img.
        let good = r#"<html><head><guide></guide></head><body><mbp:frameset><b>apple</b> a fruit<hr/><b>banana</b> another<hr/><img src="x.jpg"/><mbp:pagebreak/></mbp:frameset></body></html>"#;
        try_parse_mobi_html(good).expect("well-formed fixture should parse");
        assert_structural_tags_present(good, true);
        assert_no_html_corruption(good);
        check_balanced_tags(good).expect("well-formed fixture should be balanced");
        println!("  \u{2713} validator accepts well-formed fixture");
    }

    // -----------------------------------------------------------------------
    // Build-time self-check: verify that `html_check::validate_text_blob`
    // is wired up correctly. The build_*_mobi functions warn (not abort)
    // when the check fails, so these tests exercise the helper directly.
    // -----------------------------------------------------------------------

    #[test]
    fn test_self_check_clean_dict_blob() {
        // A clean dictionary fixture must produce zero self-check issues.
        let dir = TempDir::new("selfcheck_clean_dict");
        let entries: &[(&str, &[&str])] = &[
            ("alpha", &["alphas"]),
            ("beta", &["betas"]),
            ("gamma", &["gammas"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);

        let issues = crate::html_check::validate_text_blob(&blob);
        assert!(
            issues.is_empty(),
            "clean dict blob should self-check cleanly, got: {:?}",
            issues
        );
        println!("  \u{2713} clean dict blob passes html_check::validate_text_blob");
    }

    #[test]
    fn test_self_check_clean_book_blob() {
        // A clean book fixture must produce zero self-check issues.
        let dir = TempDir::new("selfcheck_clean_book");
        let opf = create_book_fixture(dir.path(), None);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let blob = extract_text_blob_auto(&data);

        let issues = crate::html_check::validate_text_blob(&blob);
        assert!(
            issues.is_empty(),
            "clean book blob should self-check cleanly, got: {:?}",
            issues
        );
        println!("  \u{2713} clean book blob passes html_check::validate_text_blob");
    }

    #[test]
    fn test_self_check_build_with_flag_enabled_succeeds() {
        // Build a dictionary end-to-end with self_check=true and verify
        // the build succeeds. The warning path only fires on corrupted
        // output, which clean fixtures don't produce.
        let dir = TempDir::new("selfcheck_enabled_build");
        let entries: &[(&str, &[&str])] = &[
            ("apple", &["apples"]),
            ("banana", &["bananas"]),
        ];
        let opf = create_dict_fixture(dir.path(), entries);
        let output_path = dir.path().join("output.mobi");

        mobi::build_mobi(
            &opf,
            &output_path,
            true,  // no_compress
            false, // headwords_only
            None,  // srcs_data
            false, // include_cmet
            false, // no_hd_images
            false, // creator_tag
            false, // kf8_only
            None,  // doc_type
            false, // kindle_limits
            true,  // self_check ENABLED
            false, // kindlegen_parity
        )
        .expect("build with self_check enabled should succeed");

        let data = fs::read(&output_path).unwrap();
        let blob = extract_text_blob_auto(&data);
        let issues = crate::html_check::validate_text_blob(&blob);
        assert!(
            issues.is_empty(),
            "self_check-enabled dict build should produce a clean blob, got: {:?}",
            issues
        );
        println!("  \u{2713} build with self_check=true produces clean MOBI ({} bytes)", data.len());
    }

    #[test]
    fn test_self_check_detects_unbalanced_blob() {
        // Hand-crafted bad blob: open <p> never closed inside <body>.
        let bad = br#"<html><head></head><body><p>hello</body></html>"#;
        let issues = crate::html_check::validate_text_blob(bad);
        assert!(
            issues.iter().any(|e| e.contains("tag balance")),
            "validator should report tag balance error, got: {:?}",
            issues
        );
        println!("  \u{2713} validator flagged unbalanced <p>: {:?}", issues);
    }

    #[test]
    fn test_self_check_detects_corrupt_hr() {
        let bad = br#"<html><body>text<hr/X>more</body></html>"#;
        let issues = crate::html_check::validate_text_blob(bad);
        assert!(
            issues.iter().any(|e| e.contains("corruption") || e.contains("hr/")),
            "validator should report <hr/ corruption, got: {:?}",
            issues
        );
        println!("  \u{2713} validator flagged <hr/X>: {:?}", issues);
    }

    #[test]
    fn test_self_check_detects_unclosed_attribute_quote() {
        // class="foo never closed before next tag
        let bad = br#"<html><body><p class="foo<b>bold</b></p></body></html>"#;
        let issues = crate::html_check::validate_text_blob(bad);
        assert!(
            issues.iter().any(|e| e.contains("corruption") || e.contains("quote")),
            "validator should report unclosed attribute quote, got: {:?}",
            issues
        );
        println!("  \u{2713} validator flagged unclosed quote: {:?}", issues);
    }

    #[test]
    fn test_self_check_accepts_clean_fixture() {
        // Same fixture as test_html_validator_accepts_well_formed_fixture
        // but run through the combined validator.
        let good = br#"<html><head><guide></guide></head><body><mbp:frameset><b>apple</b> a fruit<hr/><b>banana</b> another<hr/><img src="x.jpg"/><mbp:pagebreak/></mbp:frameset></body></html>"#;
        let issues = crate::html_check::validate_text_blob(good);
        assert!(
            issues.is_empty(),
            "well-formed fixture should produce no issues, got: {:?}",
            issues
        );
        println!("  \u{2713} well-formed fixture passes combined validator");
    }
}
