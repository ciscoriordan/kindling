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
}
