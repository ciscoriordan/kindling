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
    }

    #[test]
    fn test_palmdb_record_count_positive() {
        let dir = TempDir::new("palmdb_count");
        let opf = create_dict_fixture(dir.path(), &[("test", &[])]);
        let data = build_mobi_bytes(&opf, dir.path(), true, false, None);
        let (_, record_count, _) = parse_palmdb(&data);
        assert!(record_count > 0, "Record count should be > 0, got {}", record_count);
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
    }

    #[test]
    fn test_spread_detection_portrait() {
        use crate::comic;
        // Portrait image (taller than wide) should not be a spread
        let tall = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 200, |_, _| image::Rgb([128, 128, 128])),
        );
        assert!(!comic::is_double_page_spread(&tall), "100x200 should not be detected as spread");
    }

    #[test]
    fn test_spread_detection_square() {
        use crate::comic;
        // Square image should not be a spread (width == height, not >)
        let square = image::DynamicImage::ImageRgb8(
            image::RgbImage::from_fn(100, 100, |_, _| image::Rgb([128, 128, 128])),
        );
        assert!(!comic::is_double_page_spread(&square), "100x100 should not be detected as spread");
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
    }

    #[test]
    fn test_comicinfo_manga_yes() {
        use crate::comic;
        let xml = r#"<ComicInfo><Manga>Yes</Manga></ComicInfo>"#;
        let meta = comic::parse_comic_info_xml(xml).expect("Failed to parse");
        assert!(meta.manga_rtl, "Manga=Yes should enable RTL");
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
        assert_eq!(meta.effective_title(), Some("Epic Saga #5 - The Return".to_string()));
    }

    #[test]
    fn test_comicinfo_effective_title_series_number_only() {
        use crate::comic;
        let xml = r#"<ComicInfo>
  <Series>Monthly Comics</Series>
  <Number>12</Number>
</ComicInfo>"#;

        let meta = comic::parse_comic_info_xml(xml).unwrap();
        assert_eq!(meta.effective_title(), Some("Monthly Comics #12".to_string()));
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
    }

    #[test]
    fn test_comicinfo_empty_xml() {
        use crate::comic;
        let xml = r#"<ComicInfo></ComicInfo>"#;
        let meta = comic::parse_comic_info_xml(xml).unwrap();
        assert!(meta.title.is_none());
        assert!(meta.series.is_none());
        assert!(!meta.manga_rtl);
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: false, // disable for simpler test
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic with spread splitting failed");

        let data = fs::read(&output_path).expect("could not read spread comic MOBI");
        assert!(data.len() > 100, "Spread comic MOBI too small");

        // Verify we got a valid MOBI (the spread should have been split into 2 pages)
        assert_eq!(&data[60..64], b"BOOK");
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
            rtl: false, split: true, crop: false, enhance: false, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_split, &profile, &opt_split).unwrap();

        // Without splitting
        let opt_nosplit = comic::ComicOptions {
            rtl: false, split: false, crop: false, enhance: false, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
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
            rtl: false, split: false, crop: false, enhance: true, webtoon: false, panel_view: false,
            jpeg_quality: 85, max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build_comic should succeed on color device even with enhance=true");

        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "Color comic MOBI should be valid");
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
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
        // Should follow the first_12 + "-" + last_14 format = 27 chars
        assert_eq!(name_len, 27, "Truncated name should be 27 bytes (12 + 1 + 14), got {}", name_len);

        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap();
        assert!(name.contains('-'), "Truncated name should contain '-' separator: '{}'", name);
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
    }

    #[test]
    fn test_webtoon_split_hard_cut() {
        use crate::comic;

        // Create a tall strip with NO gutters (no uniform rows) - forces hard split
        let strip_height = 3000u32;
        let strip_width = 100u32;
        let device_height = 1448u32;

        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_fn(strip_width, strip_height, |x, y| {
                // Noisy content everywhere - no gutters
                image::Luma([((x * 37 + y * 13 + 7) % 200) as u8 + 28])
            }),
        );

        let pages = comic::webtoon_split(&img, device_height);

        // Should still produce pages
        assert!(pages.len() >= 2, "Should produce at least 2 pages even without gutters, got {}", pages.len());

        // Total height should still equal original
        let total_h: u32 = pages.iter().map(|p| p.height()).sum();
        assert_eq!(total_h, strip_height, "Sum of page heights should equal strip height");
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
            crop: false,
            enhance: false,
            webtoon: false, // rely on auto-detection
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
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
            crop: false,
            enhance: false,
            webtoon: true, // force webtoon mode
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Forced webtoon pipeline should succeed");

        let data = fs::read(&output_path).expect("could not read forced webtoon MOBI");
        assert!(data.len() > 100, "Forced webtoon MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
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
            crop: false,
            enhance: false,
            webtoon: true,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Webtoon with Scribe profile should succeed");

        let data = fs::read(&output_path).expect("could not read scribe webtoon MOBI");
        assert!(data.len() > 100, "Scribe webtoon MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };

        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("Panel View comic build should succeed");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).expect("could not read panel view comic MOBI");
        assert!(data.len() > 100, "Panel View comic MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_no_pv, &profile, &options_no_pv)
            .expect("no-panel-view comic build should succeed");

        // Build with panel_view enabled
        let output_with_pv = dir.path().join("with_pv.mobi");
        let options_with_pv = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
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
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_pv, &profile, &options)
            .expect("Panel View OPF comic build should succeed");

        // Build without panel_view
        let output_no_pv = dir.path().join("no_pv_comic.mobi");
        let options_no = comic::ComicOptions {
            rtl: false,
            split: false,
            crop: false,
            enhance: false,
            webtoon: false,
            panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_no_pv, &profile, &options_no)
            .expect("No Panel View OPF comic build should succeed");

        // Both should produce valid MOBIs
        let data_pv = fs::read(&output_pv).unwrap();
        let data_no_pv = fs::read(&output_no_pv).unwrap();
        assert_eq!(&data_pv[60..64], b"BOOK");
        assert_eq!(&data_no_pv[60..64], b"BOOK");
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
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 30,
            max_height: 65536, embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_low, &profile, &options_low)
            .expect("low quality build failed");

        // Build at high quality (95)
        let output_high = dir.path().join("quality_high.mobi");
        let options_high = comic::ComicOptions {
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 95,
            max_height: 65536, embed_source: false,
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
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: true, panel_view: false,
            jpeg_quality: 85,
            max_height: 3000,
            embed_source: false,
        };
        comic::build_comic_with_options(&images_dir, &output_chunked, &profile, &options_chunked)
            .expect("chunked webtoon build failed");

        // Build with default (no chunking)
        let output_normal = dir.path().join("normal.mobi");
        let options_normal = comic::ComicOptions {
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: true, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
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
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };

        // Should succeed despite the corrupt image
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build should succeed by skipping the corrupt image");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
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
            rtl: false, split: false, crop: false, enhance: false,
            webtoon: false, panel_view: false,
            jpeg_quality: 85,
            max_height: 65536, embed_source: false,
        };

        // Should succeed by skipping the zero-dimension image
        comic::build_comic_with_options(&images_dir, &output_path, &profile, &options)
            .expect("build should succeed by skipping the zero-dimension image");

        // Verify output is a valid MOBI
        let data = fs::read(&output_path).unwrap();
        assert!(data.len() > 100, "MOBI too small");
        assert_eq!(&data[60..64], b"BOOK");
    }
}
