/// MOBI file writer for dictionaries and books.
///
/// Builds a valid MOBI file from OPF source files, including:
/// - PalmDB header
/// - PalmDOC header + MOBI header + EXTH header (record 0)
/// - Compressed text content records
/// - INDX records with dictionary index (dictionaries only)
/// - FLIS, FCIS, EOF records

use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::exth;
use crate::indx::{self, encode_indx_label, LookupTerm};
use crate::opf::{self, DictionaryEntry, OPFData};
use crate::palmdoc;

const RECORD_SIZE: usize = 4096;
const MOBI_HEADER_LENGTH: usize = 264;

/// Build a MOBI file from an OPF source.
///
/// Automatically detects whether the input is a dictionary (contains idx:entry tags)
/// or a regular book, and adjusts the output accordingly.
pub fn build_mobi(
    opf_path: &Path,
    output_path: &Path,
    no_compress: bool,
    headwords_only: bool,
    srcs_data: Option<&[u8]>,
    include_cmet: bool,
    no_hd_images: bool,
    creator_tag: bool,
    kf8_only: bool,
    doc_type: Option<&str>,
    kindle_limits: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let opf = OPFData::parse(opf_path)?;

    // Detect dictionary vs book by checking HTML content for idx:entry tags
    let is_dictionary = detect_dictionary(&opf);

    if is_dictionary {
        if kf8_only {
            return Err("KF8-only output is not supported for dictionaries (dictionaries use MOBI7 format)".into());
        }
        eprintln!("Detected dictionary content");
        build_dictionary_mobi(&opf, output_path, no_compress, headwords_only, srcs_data, include_cmet, creator_tag, kindle_limits)
    } else {
        if kf8_only {
            eprintln!("Detected book content, building KF8-only (.azw3)");
        } else {
            eprintln!("Detected book content (no idx:entry tags found)");
        }
        build_book_mobi(&opf, output_path, no_compress, srcs_data, include_cmet, !no_hd_images, creator_tag, kf8_only, doc_type, kindle_limits)
    }
}

/// Check if any HTML content file contains dictionary markup (idx:entry tags).
fn detect_dictionary(opf: &OPFData) -> bool {
    for html_path in opf.get_content_html_paths() {
        if let Ok(content) = std::fs::read_to_string(&html_path) {
            if content.contains("<idx:entry") {
                return true;
            }
        }
    }
    false
}

/// Kindle publishing limit: maximum size per HTML chunk (30 MB).
const KINDLE_HTML_SIZE_LIMIT: usize = 30 * 1024 * 1024;

/// Kindle publishing limit: maximum number of HTML files.
const KINDLE_HTML_FILE_LIMIT: usize = 300;

/// Build a dictionary MOBI file (existing behavior).
fn build_dictionary_mobi(
    opf: &OPFData,
    output_path: &Path,
    no_compress: bool,
    headwords_only: bool,
    srcs_data: Option<&[u8]>,
    include_cmet: bool,
    creator_tag: bool,
    kindle_limits: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Parse all dictionary entries from HTML content
    let mut all_entries: Vec<DictionaryEntry> = Vec::new();
    for html_path in opf.get_content_html_paths() {
        let entries = opf::parse_dictionary_html(&html_path)?;
        all_entries.extend(entries);
    }

    if all_entries.is_empty() {
        return Err("No dictionary entries found in HTML content files".into());
    }

    eprintln!("Parsed {} dictionary entries", all_entries.len());

    // Build the text content (stripped HTML for all spine items)
    eprintln!("Building text content...");
    let text_content = if kindle_limits {
        build_text_content_by_letter(&opf, &all_entries)
    } else {
        build_text_content(&opf, true)
    };

    // Insert the guide reference tag
    let text_content = insert_guide_reference(&text_content);

    // Build text records
    let (text_records, text_length) = if no_compress {
        eprintln!("Splitting text into uncompressed records...");
        let result = split_text_uncompressed(&text_content);
        eprintln!(
            "Split text into {} uncompressed records ({} bytes)",
            result.0.len(),
            result.1
        );
        result
    } else {
        eprintln!("Compressing text...");
        let result = compress_text(&text_content);
        eprintln!(
            "Compressed text into {} records ({} bytes uncompressed)",
            result.0.len(),
            result.1
        );
        result
    };

    // Find entry positions in the stripped text
    eprintln!("Finding entry positions...");
    let entry_positions = find_entry_positions(&text_content, &all_entries);

    // Build lookup terms
    eprintln!("Building lookup terms...");
    let lookup_terms = build_lookup_terms(&all_entries, &entry_positions, &text_content, headwords_only);
    let label = if headwords_only {
        "headwords only"
    } else {
        "headwords + inflections"
    };
    eprintln!("Built {} lookup terms ({})", lookup_terms.len(), label);

    // Build INDX records
    eprintln!("Building INDX records...");
    let mut headword_chars_for_indx: HashSet<char> = HashSet::new();
    for entry in &all_entries {
        for c in entry.headword.chars() {
            if c as u32 > 0x7F {
                headword_chars_for_indx.insert(c);
            }
        }
    }
    let indx_records = indx::build_orth_indx(&lookup_terms, &headword_chars_for_indx);

    // Build FLIS, FCIS, EOF records
    let flis = build_flis();
    let fcis = build_fcis(text_length);
    let eof = build_eof();

    // Build optional SRCS and CMET records
    let srcs_record: Option<Vec<u8>> = srcs_data.map(|data| {
        // SRCS record format: "SRCS" + header_len(u32) + unknown(u32) + count(u32) + epub_data
        let mut rec = Vec::with_capacity(16 + data.len());
        rec.extend_from_slice(b"SRCS");
        rec.extend_from_slice(&0x10u32.to_be_bytes()); // header length = 16
        rec.extend_from_slice(&(data.len() as u32).to_be_bytes());
        rec.extend_from_slice(&1u32.to_be_bytes());
        rec.extend_from_slice(data);
        rec
    });
    let cmet_record: Option<Vec<u8>> = if include_cmet {
        Some(build_cmet())
    } else {
        None
    };
    let num_optional = srcs_record.as_ref().map_or(0, |_| 1) + cmet_record.as_ref().map_or(0, |_| 1);

    // Calculate record indices
    // Layout: record0 | text | INDX | FLIS | FCIS | [SRCS] | [CMET] | EOF
    let first_non_book = text_records.len() + 1;
    let orth_index_record = text_records.len() + 1;
    let flis_record = text_records.len() + 1 + indx_records.len();
    let fcis_record = flis_record + 1;
    let srcs_record_idx = if srcs_record.is_some() {
        Some(fcis_record + 1)
    } else {
        None
    };
    let total_records = 1 + text_records.len() + indx_records.len() + 3 + num_optional;

    // Collect unique headword characters for fontsignature
    let mut headword_chars: HashSet<u32> = HashSet::new();
    for entry in &all_entries {
        for c in entry.headword.chars() {
            headword_chars.insert(c as u32);
        }
    }

    // Build record 0
    let record0 = build_record0(
        &opf,
        text_length,
        text_records.len(),
        first_non_book,
        orth_index_record,
        total_records,
        flis_record,
        fcis_record,
        no_compress,
        &headword_chars,
        true, // is_dictionary
        0xFFFFFFFF, // no images for dictionaries
        None, // no cover offset
        None, // no fixed-layout for dictionaries
        None, // no version override (use default 7)
        None, // no KF8 boundary (dictionaries stay KF7-only)
        srcs_record_idx,
        None, // no HD images for dictionaries
        creator_tag,
        None, // no doc_type for dictionaries
    );

    // Assemble all records
    let mut all_records = vec![record0];
    all_records.extend(text_records);
    all_records.extend(indx_records);
    all_records.push(flis);
    all_records.push(fcis);
    if let Some(srcs) = srcs_record {
        all_records.push(srcs);
    }
    if let Some(cmet) = cmet_record {
        all_records.push(cmet);
    }
    all_records.push(eof);

    // Build PalmDB header and write file
    let title = if opf.title.is_empty() {
        "Dictionary"
    } else {
        &opf.title
    };
    let palmdb = build_palmdb(title, &all_records);

    std::fs::write(output_path, &palmdb)?;
    eprintln!("Wrote {} ({} bytes)", output_path.display(), palmdb.len());

    Ok(())
}

/// Build a regular book MOBI file with dual KF7+KF8 format.
///
/// Record layout:
///   KF7 Section: record0, text records, image records, FLIS, FCIS
///   BOUNDARY record (8 bytes: "BOUNDARY")
///   KF8 Section: kf8_record0, kf8_text records, NULL padding,
///                fragment INDX, skeleton INDX, NCX INDX, FDST, DATP,
///                FLIS, FCIS, EOF
fn build_book_mobi(
    opf: &OPFData,
    output_path: &Path,
    no_compress: bool,
    srcs_data: Option<&[u8]>,
    include_cmet: bool,
    hd_images: bool,
    creator_tag: bool,
    kf8_only: bool,
    doc_type: Option<&str>,
    kindle_limits: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Collect images from the OPF manifest
    let image_items = opf.get_image_items(); // Vec<(href, media_type)>
    let cover_href = opf.get_cover_image_href();

    // Build the href-to-recindex mapping and load image data
    // Image recindex is 1-based (first image = "00001")
    let mut image_records: Vec<Vec<u8>> = Vec::new();
    let mut href_to_recindex: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut cover_offset: Option<u32> = None;
    let mut total_image_bytes: usize = 0;

    for (idx, (href, _media_type)) in image_items.iter().enumerate() {
        let recindex = idx + 1; // 1-based
        let image_path = opf.base_dir.join(href);

        // Try reading the file directly, then with percent-decoded path
        let data = std::fs::read(&image_path).or_else(|_| {
            let decoded = percent_decode(href);
            std::fs::read(opf.base_dir.join(&decoded))
        });

        if let Ok(mut data) = data {
            // Patch JFIF density units: Kindle firmware needs DPI units (0x01)
            // for cover images to display on the lock screen. If the JFIF header
            // has units=0x00 (aspect ratio only), change it to 0x01 (DPI).
            // JFIF layout: FF D8 FF E0 [len:2] 'J' 'F' 'I' 'F' \0 [ver:2] [units:1]
            //              0  1  2  3   4  5   6   7   8   9  10  11  12    13
            if data.len() > 13
                && data[0] == 0xFF && data[1] == 0xD8  // SOI marker
                && data[2] == 0xFF && data[3] == 0xE0  // APP0 marker
                && data[6..11] == *b"JFIF\0"           // JFIF identifier
                && data[13] == 0x00                     // units = aspect ratio only
            {
                data[13] = 0x01; // patch to DPI
            }
            total_image_bytes += data.len();
            href_to_recindex.insert(href.clone(), recindex);
            image_records.push(data);

            // Check if this is the cover image
            if let Some(ref cover) = cover_href {
                if href == cover {
                    cover_offset = Some(idx as u32); // 0-based offset within image records
                }
            }
        } else {
            eprintln!("Warning: could not read image file: {}", image_path.display());
            // Still push an empty record to keep recindex alignment
            href_to_recindex.insert(href.clone(), recindex);
            image_records.push(Vec::new());
        }
    }

    if !image_records.is_empty() {
        eprintln!(
            "Collected {} images ({} bytes total)",
            image_records.len(),
            total_image_bytes
        );
    }

    // Build KF8 section (used by both dual and KF8-only modes)
    eprintln!("Building KF8 section...");
    let html_parts = build_html_parts(opf);

    // Kindle publishing limits checks for books
    if kindle_limits {
        let num_html_files = html_parts.len();
        if num_html_files > KINDLE_HTML_FILE_LIMIT {
            eprintln!(
                "Warning: {} HTML files exceeds the Kindle limit of {} files",
                num_html_files, KINDLE_HTML_FILE_LIMIT
            );
        }
        for (i, part) in html_parts.iter().enumerate() {
            let part_size = part.len();
            if part_size > KINDLE_HTML_SIZE_LIMIT {
                eprintln!(
                    "Warning: HTML part {} is {} bytes, exceeds 30 MB Kindle limit ({} bytes)",
                    i + 1, part_size, KINDLE_HTML_SIZE_LIMIT
                );
            }
        }
    }

    let css_content = extract_css_content(opf);
    let kf8_section = crate::kf8::build_kf8_section(
        &html_parts,
        &css_content,
        &href_to_recindex,
        &opf.spine_items,
        no_compress,
    );
    eprintln!(
        "KF8: {} text records ({} bytes), {} flows",
        kf8_section.text_records.len(),
        kf8_section.text_length,
        kf8_section.flow_count,
    );

    // Build optional SRCS and CMET records
    let srcs_record: Option<Vec<u8>> = srcs_data.map(|data| {
        let mut rec = Vec::with_capacity(16 + data.len());
        rec.extend_from_slice(b"SRCS");
        rec.extend_from_slice(&0x10u32.to_be_bytes());
        rec.extend_from_slice(&(data.len() as u32).to_be_bytes());
        rec.extend_from_slice(&1u32.to_be_bytes());
        rec.extend_from_slice(data);
        rec
    });
    let cmet_record: Option<Vec<u8>> = if include_cmet {
        Some(build_cmet())
    } else {
        None
    };
    let num_optional = srcs_record.as_ref().map_or(0, |_| 1) + cmet_record.as_ref().map_or(0, |_| 1);

    let num_image_records = image_records.len();

    // Build fixed-layout metadata if applicable
    let fixed_layout = if opf.is_fixed_layout {
        eprintln!("Detected fixed-layout content");
        Some(exth::FixedLayoutMeta {
            is_fixed_layout: true,
            original_resolution: opf.original_resolution.clone(),
            page_progression_direction: opf.page_progression_direction.clone(),
        })
    } else {
        None
    };

    let title = if opf.title.is_empty() {
        "Book"
    } else {
        &opf.title
    };

    if kf8_only {
        // --- KF8-only record layout ---
        // [0]          Record 0 (KF8, version=8)
        // [1..T]       KF8 text records
        // [T+1]        NULL padding
        // [T+2..T+I+1] Image records
        // [T+I+2..]   Fragment INDX
        // [...]        Skeleton INDX
        // [...]        NCX INDX
        // [...]        FDST
        // [...]        DATP
        // [...]        FLIS
        // [...]        FCIS
        // [...]        [SRCS]
        // [...]        [CMET]
        // [...]        EOF
        // [HD container if enabled]
        let kf8_text_count = kf8_section.text_records.len();
        let kf8_null_pad = kf8_text_count + 1;
        let kf8_first_image = if num_image_records > 0 {
            kf8_null_pad + 1
        } else {
            0xFFFFFFFF
        };
        let kf8_fragment_start = kf8_null_pad + 1 + num_image_records;
        let kf8_skeleton_start = kf8_fragment_start + kf8_section.fragment_indx.len();
        let kf8_ncx_start = kf8_skeleton_start + kf8_section.skeleton_indx.len();
        let kf8_fdst_idx = kf8_ncx_start + kf8_section.ncx_indx.len();
        let kf8_datp_idx = kf8_fdst_idx + 1;
        let kf8_flis_idx = kf8_datp_idx + 1;
        let kf8_fcis_idx = kf8_flis_idx + 1;
        let kf8_srcs_idx = if srcs_record.is_some() {
            Some(kf8_fcis_idx + 1)
        } else {
            None
        };
        let kf8_first_nonbook = kf8_text_count + 1;

        // HD container
        let hd_container: Option<HdContainer> = if hd_images && num_image_records > 0 {
            eprintln!("Building HD image container (CONT/CRES)...");
            Some(build_hd_container(opf, &image_records))
        } else {
            None
        };
        let hd_record_count = hd_container.as_ref().map_or(0, |hd| hd.total_record_count());
        let hd_geometry_string: Option<String> = hd_container.as_ref().map(|hd| hd.geometry_string());

        let total_records = 1 + kf8_text_count + 1 + num_image_records
            + kf8_section.fragment_indx.len()
            + kf8_section.skeleton_indx.len()
            + kf8_section.ncx_indx.len()
            + 1 + 1 + 1 + 1  // FDST + DATP + FLIS + FCIS
            + num_optional + 1  // [SRCS] + [CMET] + EOF
            + hd_record_count;

        // Build KF8 record 0
        let kf8_record0 = build_kf8_record0(
            opf,
            kf8_section.text_length,
            kf8_text_count,
            kf8_first_nonbook,
            kf8_fdst_idx,
            kf8_section.flow_count,
            kf8_skeleton_start,
            kf8_fragment_start,
            kf8_ncx_start,
            kf8_datp_idx,
            kf8_flis_idx,
            kf8_fcis_idx,
            no_compress,
            cover_offset,
            fixed_layout.as_ref(),
            kf8_first_image,
            creator_tag,
            kf8_srcs_idx,
            hd_geometry_string.as_deref(),
            total_records,
            doc_type,
        );

        let kf8_flis_rec = build_flis();
        let kf8_fcis_rec = build_fcis(kf8_section.text_length);
        let eof = build_eof();
        let null_pad_rec = vec![0x00u8];

        // Assemble KF8-only records
        let mut all_records: Vec<Vec<u8>> = Vec::new();
        all_records.push(kf8_record0);
        all_records.extend(kf8_section.text_records);
        all_records.push(null_pad_rec);
        all_records.extend(image_records);
        all_records.extend(kf8_section.fragment_indx);
        all_records.extend(kf8_section.skeleton_indx);
        all_records.extend(kf8_section.ncx_indx);
        all_records.push(kf8_section.fdst);
        all_records.push(kf8_section.datp);
        all_records.push(kf8_flis_rec);
        all_records.push(kf8_fcis_rec);
        if let Some(srcs) = srcs_record {
            all_records.push(srcs);
        }
        if let Some(cmet) = cmet_record {
            all_records.push(cmet);
        }
        all_records.push(eof);

        if let Some(hd) = hd_container {
            all_records.extend(hd.into_records());
        }

        let hd_str = if hd_record_count > 0 {
            format!(", HD: {}", hd_record_count)
        } else {
            String::new()
        };
        eprintln!(
            "KF8-only: {} total records{}",
            all_records.len(),
            hd_str,
        );

        let palmdb = build_palmdb(title, &all_records);
        std::fs::write(output_path, &palmdb)?;
        eprintln!("Wrote {} ({} bytes)", output_path.display(), palmdb.len());
    } else {
        // --- Dual KF7+KF8 format ---

        // Build the KF7 text content (stripped for KF7, with recindex for images)
        eprintln!("Building KF7 text content...");
        let text_content = build_text_content(opf, false);

        // Rewrite image src attributes to recindex references for KF7
        let text_content = if !href_to_recindex.is_empty() {
            rewrite_image_src(&text_content, &href_to_recindex, &opf.spine_items)
        } else {
            text_content
        };

        // Build KF7 text records
        let (text_records, text_length) = if no_compress {
            eprintln!("Splitting KF7 text into uncompressed records...");
            let result = split_text_uncompressed(&text_content);
            eprintln!(
                "Split KF7 text into {} uncompressed records ({} bytes)",
                result.0.len(),
                result.1
            );
            result
        } else {
            eprintln!("Compressing KF7 text...");
            let result = compress_text(&text_content);
            eprintln!(
                "Compressed KF7 text into {} records ({} bytes uncompressed)",
                result.0.len(),
                result.1
            );
            result
        };

        // --- KF7 Section record layout ---
        let kf7_first_non_book = text_records.len() + 1;
        let kf7_first_image = if num_image_records > 0 {
            text_records.len() + 1
        } else {
            0xFFFFFFFF
        };
        let kf7_flis = text_records.len() + 1 + num_image_records;
        let kf7_fcis = kf7_flis + 1;
        let kf7_srcs_idx = if srcs_record.is_some() {
            Some(kf7_fcis + 1)
        } else {
            None
        };

        let boundary_idx = 1 + text_records.len() + num_image_records + 2 + num_optional;
        let kf8_record0_global = boundary_idx + 1;
        let kf7_total = 1 + text_records.len() + num_image_records + 2 + num_optional;

        // --- KF8 Section record layout (KF8-relative indices) ---
        let kf8_text_count = kf8_section.text_records.len();
        let kf8_null_pad = kf8_text_count + 1;
        let kf8_fragment_start = kf8_null_pad + 1;
        let kf8_skeleton_start = kf8_fragment_start + kf8_section.fragment_indx.len();
        let kf8_ncx_start = kf8_skeleton_start + kf8_section.skeleton_indx.len();
        let kf8_fdst_idx = kf8_ncx_start + kf8_section.ncx_indx.len();
        let kf8_datp_idx = kf8_fdst_idx + 1;
        let kf8_flis_idx = kf8_datp_idx + 1;
        let kf8_fcis_idx = kf8_flis_idx + 1;
        let kf8_first_nonbook = kf8_text_count + 1;

        // HD container
        let hd_container: Option<HdContainer> = if hd_images && num_image_records > 0 {
            eprintln!("Building HD image container (CONT/CRES)...");
            Some(build_hd_container(opf, &image_records))
        } else {
            None
        };
        let hd_record_count = hd_container.as_ref().map_or(0, |hd| hd.total_record_count());
        let hd_geometry_string: Option<String> = hd_container.as_ref().map(|hd| hd.geometry_string());

        let total_global_records = kf7_total + 1 + 1 + kf8_text_count + 1
            + kf8_section.fragment_indx.len()
            + kf8_section.skeleton_indx.len()
            + kf8_section.ncx_indx.len()
            + 1 + 1 + 3  // FDST + DATP + FLIS + FCIS + EOF
            + hd_record_count;

        // Build KF7 record 0 (version=6, with EXTH 121 pointing to KF8 record 0)
        let empty_chars: HashSet<u32> = HashSet::new();
        let kf7_record0 = build_record0(
            opf,
            text_length,
            text_records.len(),
            kf7_first_non_book,
            0xFFFFFFFF_usize,
            total_global_records,
            kf7_flis,
            kf7_fcis,
            no_compress,
            &empty_chars,
            false,
            kf7_first_image,
            cover_offset,
            fixed_layout.as_ref(),
            Some(6),
            Some(kf8_record0_global as u32),
            kf7_srcs_idx,
            hd_geometry_string.as_deref(),
            creator_tag,
            doc_type,
        );

        // Build KF8 record 0 (version=8, KF8-relative indices)
        let kf8_record0 = build_kf8_record0(
            opf,
            kf8_section.text_length,
            kf8_text_count,
            kf8_first_nonbook,
            kf8_fdst_idx,
            kf8_section.flow_count,
            kf8_skeleton_start,
            kf8_fragment_start,
            kf8_ncx_start,
            kf8_datp_idx,
            kf8_flis_idx,
            kf8_fcis_idx,
            no_compress,
            cover_offset,
            fixed_layout.as_ref(),
            0xFFFFFFFF, // KF8 first_image (images are in KF7 section)
            creator_tag,
            None,  // no SRCS in KF8 section of dual format
            None,  // no HD geometry in KF8 section of dual format
            0,     // total_records not used for KF8 section in dual format
            doc_type,
        );

        // Build FLIS/FCIS/EOF for both sections
        let kf7_flis_rec = build_flis();
        let kf7_fcis_rec = build_fcis(text_length);
        let kf8_flis_rec = build_flis();
        let kf8_fcis_rec = build_fcis(kf8_section.text_length);
        let eof = build_eof();
        let boundary_rec = b"BOUNDARY".to_vec();
        let null_pad_rec = vec![0x00u8];

        // Assemble all records
        let mut all_records: Vec<Vec<u8>> = Vec::new();

        // KF7 section
        all_records.push(kf7_record0);
        all_records.extend(text_records);
        all_records.extend(image_records);
        all_records.push(kf7_flis_rec);
        all_records.push(kf7_fcis_rec);
        if let Some(srcs) = srcs_record {
            all_records.push(srcs);
        }
        if let Some(cmet) = cmet_record {
            all_records.push(cmet);
        }

        // Boundary
        all_records.push(boundary_rec);

        // KF8 section
        all_records.push(kf8_record0);
        all_records.extend(kf8_section.text_records);
        all_records.push(null_pad_rec);
        all_records.extend(kf8_section.fragment_indx);
        all_records.extend(kf8_section.skeleton_indx);
        all_records.extend(kf8_section.ncx_indx);
        all_records.push(kf8_section.fdst);
        all_records.push(kf8_section.datp);
        all_records.push(kf8_flis_rec);
        all_records.push(kf8_fcis_rec);
        all_records.push(eof);

        // HD image container
        if let Some(hd) = hd_container {
            all_records.extend(hd.into_records());
        }

        let hd_str = if hd_record_count > 0 {
            format!(", HD: {}", hd_record_count)
        } else {
            String::new()
        };
        eprintln!(
            "Dual format: {} total records (KF7: {}, boundary: 1, KF8: {}{})",
            all_records.len(),
            kf7_total,
            all_records.len() - kf7_total - 1 - hd_record_count,
            hd_str,
        );

        let palmdb = build_palmdb(title, &all_records);
        std::fs::write(output_path, &palmdb)?;
        eprintln!("Wrote {} ({} bytes)", output_path.display(), palmdb.len());
    }

    Ok(())
}

// --- HD Image Container (CONT/CRES) support ---

/// Represents the HD image container that goes after the KF8 section.
///
/// Record layout:
///   BOUNDARY (8 bytes)
///   CONT (header with EXTH-like metadata)
///   CRES/placeholder records (one per image)
///   kindle:embed list record
///   CONTBOUNDARY marker (12 bytes)
///   EOF marker (4 bytes)
struct HdContainer {
    /// The CONT header record
    cont_record: Vec<u8>,
    /// CRES records (actual HD images) or placeholder records (0xA0A0A0A0)
    cres_records: Vec<Vec<u8>>,
    /// kindle:embed list record (pipe-delimited kindle:embed URLs for HD images)
    kindle_embed_list: Vec<u8>,
    /// Maximum image width across all HD images
    max_width: u32,
    /// Maximum image height across all HD images
    max_height: u32,
    /// Total number of CRES/placeholder slots
    num_cres_slots: usize,
}

impl HdContainer {
    /// Total number of PalmDB records this container adds.
    /// BOUNDARY + CONT + CRES slots + kindle:embed list + CONTBOUNDARY + EOF
    fn total_record_count(&self) -> usize {
        1 + 1 + self.cres_records.len() + 1 + 1 + 1
    }

    /// Build the EXTH 536 geometry string: "WxH:start-end|"
    /// start and end are 0-based indices covering the CRES/placeholder slots,
    /// the kindle:embed list, and the CONTBOUNDARY record.
    fn geometry_string(&self) -> String {
        // end index = num_cres_slots + 1 (kindle:embed list) + 1 (CONTBOUNDARY)
        let end = self.num_cres_slots + 2;
        format!("{}x{}:0-{}|", self.max_width, self.max_height, end)
    }

    /// Convert into a flat list of PalmDB records in order.
    fn into_records(self) -> Vec<Vec<u8>> {
        let mut records = Vec::with_capacity(self.total_record_count());
        records.push(b"BOUNDARY".to_vec());
        records.push(self.cont_record);
        records.extend(self.cres_records);
        records.push(self.kindle_embed_list);
        records.push(b"CONTBOUNDARY".to_vec());
        records.push(vec![0xE9, 0x8E, 0x0D, 0x0A]); // EOF
        records
    }
}

/// Build the HD image container for a book MOBI.
///
/// Each image from the KF7 section gets a corresponding slot in the HD container:
/// either a CRES record with the full image data (for all images, since the source
/// images from EPUB are typically already high-res) or a 4-byte placeholder
/// (0xA0A0A0A0) for empty/missing images.
fn build_hd_container(
    opf: &OPFData,
    image_records: &[Vec<u8>],
) -> HdContainer {
    let title = if opf.title.is_empty() { "Book" } else { &opf.title };
    let num_images = image_records.len();

    let mut cres_records: Vec<Vec<u8>> = Vec::new();
    let mut hd_image_count: u32 = 0;
    let mut max_width: u32 = 0;
    let mut max_height: u32 = 0;
    let mut kindle_embed_parts: Vec<String> = Vec::new();

    for (idx, img_data) in image_records.iter().enumerate() {
        if img_data.is_empty() {
            // Empty image slot - use placeholder
            cres_records.push(vec![0xA0, 0xA0, 0xA0, 0xA0]);
            continue;
        }

        // Check image dimensions
        let dims = get_image_dimensions(img_data);

        if let Some((w, h)) = dims {
            // Include as HD image: CRES header (12 bytes) + image data
            let mut cres = Vec::with_capacity(12 + img_data.len());
            cres.extend_from_slice(b"CRES");
            cres.extend_from_slice(&0u32.to_be_bytes()); // reserved
            cres.extend_from_slice(&12u32.to_be_bytes()); // offset to image data
            cres.extend_from_slice(img_data);
            cres_records.push(cres);

            hd_image_count += 1;
            if w > max_width { max_width = w; }
            if h > max_height { max_height = h; }

            // Build kindle:embed reference for this HD image
            // recindex is 1-based
            let recindex = idx + 1;
            let embed_ref = format!(
                "kindle:embed:{}?mime=image/jpg",
                encode_kindle_embed_base32(recindex)
            );
            kindle_embed_parts.push(embed_ref);
        } else {
            // Can't determine dimensions (not JPEG/PNG) - use placeholder
            cres_records.push(vec![0xA0, 0xA0, 0xA0, 0xA0]);
        }
    }

    // Build the kindle:embed list record (pipe-delimited, trailing pipe)
    let kindle_embed_list = if kindle_embed_parts.is_empty() {
        Vec::new()
    } else {
        let mut list_str = kindle_embed_parts.join("|");
        list_str.push('|');
        list_str.into_bytes()
    };

    // Build the CONT header record
    let cont_record = build_cont_record(
        title,
        &opf.author,
        num_images,
        hd_image_count,
        max_width,
        max_height,
    );

    eprintln!(
        "HD container: {} image slots, {} HD images, max {}x{}",
        num_images, hd_image_count, max_width, max_height,
    );

    HdContainer {
        cont_record,
        cres_records,
        kindle_embed_list,
        max_width,
        max_height,
        num_cres_slots: num_images,
    }
}

/// Build the CONT record (HD container header).
///
/// Structure (48-byte header + EXTH block + padded title):
///   0: "CONT" magic
///   4: total record length (u32 BE)
///   8: (version << 16) | total_records_in_container (u32 BE)
///   12: encoding (65001 = UTF-8)
///   16: 0
///   20: 1
///   24: num_cres_slots (number of CRES/placeholder records)
///   28: num_hd_images (number of actual HD images)
///   32: kindle_embed_list_index (CONT-relative index of kindle:embed list record)
///   36: 1
///   40: EXTH_offset (offset where EXTH starts in this record, always 216 after padding)
///   44: title_length
///   48: EXTH block
///   48+exth_len: padded title
fn build_cont_record(
    title: &str,
    _author: &str,
    num_cres_slots: usize,
    num_hd_images: u32,
    max_width: u32,
    max_height: u32,
) -> Vec<u8> {
    // CONT-relative index of the kindle:embed list record:
    // CONT is record 0, CRES slots are records 1..num_cres_slots, kindle:embed = num_cres_slots + 1
    let kindle_embed_index = num_cres_slots + 1;

    // Total records in the container (CRES slots + kindle:embed list + CONTBOUNDARY)
    // The version/count field at offset 8 encodes (1 << 16) | total_count
    // where total_count includes all records from CONT itself through CONTBOUNDARY + EOF
    // Observed: kindlegen uses count = num_cres_slots + 3 (kindle:embed + CONTBOUNDARY + EOF?)
    let container_total = num_cres_slots + 3;

    // Build CONT EXTH block
    let mut exth_records: Vec<Vec<u8>> = vec![
        exth::exth_record(125, &4u32.to_be_bytes()),
        exth::exth_record(204, &202u32.to_be_bytes()), // creator platform
        exth::exth_record(205, &0u32.to_be_bytes()),   // major
        exth::exth_record(206, &1u32.to_be_bytes()),   // minor
        exth::exth_record(535, format!("kindling-{}", env!("CARGO_PKG_VERSION")).as_bytes()),
        exth::exth_record(207, &0u32.to_be_bytes()),   // build
        exth::exth_record(539, b"application/image"),   // container MIME
    ];
    let dims_str = format!("{}x{}", max_width, max_height);
    exth_records.push(exth::exth_record(538, dims_str.as_bytes()));   // HD dimensions
    // EXTH 542 - content hash (4 bytes from MD5 of title)
    let title_bytes = if title.is_empty() { b"Book".to_vec() } else { title.as_bytes().to_vec() };
    let title_hash = md5_simple(&title_bytes);
    exth_records.push(exth::exth_record(542, &title_hash[..4]));
    exth_records.push(exth::exth_record(543, b"HD_CONTAINER"));      // container type

    let exth_record_data: Vec<u8> = exth_records.iter().flat_map(|r| r.iter().copied()).collect();
    let exth_length = 12 + exth_record_data.len();
    let exth_padding = (4 - (exth_length % 4)) % 4;
    let exth_padded_length = exth_length + exth_padding;

    let mut exth_block = Vec::with_capacity(exth_padded_length);
    exth_block.extend_from_slice(b"EXTH");
    exth_block.extend_from_slice(&(exth_padded_length as u32).to_be_bytes());
    exth_block.extend_from_slice(&(exth_records.len() as u32).to_be_bytes());
    exth_block.extend_from_slice(&exth_record_data);
    exth_block.extend_from_slice(&vec![0u8; exth_padding]);

    // Compute title padding
    let title_raw = title.as_bytes();
    let title_len = title_raw.len();

    // The EXTH offset field at offset 40 is the total size of header(48) + exth + padded_title area.
    // In kindlegen this was 216, which equals 48 (header) + 168 (EXTH).
    // The title follows the EXTH and is padded with zeros to fill out the record.
    let header_size = 48;
    let exth_offset = header_size + exth_block.len();

    // Total record size: header + EXTH + title + padding to fill nicely
    // We want the title area to be padded to at least a reasonable size
    let title_area_size = std::cmp::max(256, title_len.div_ceil(4) * 4);
    let total_size = header_size + exth_block.len() + title_area_size;

    // Build the 48-byte header
    let mut record = Vec::with_capacity(total_size);
    record.extend_from_slice(b"CONT");
    record.extend_from_slice(&(total_size as u32).to_be_bytes());
    record.extend_from_slice(&((1u32 << 16) | container_total as u32).to_be_bytes());
    record.extend_from_slice(&65001u32.to_be_bytes()); // UTF-8
    record.extend_from_slice(&0u32.to_be_bytes());     // offset 16: 0
    record.extend_from_slice(&1u32.to_be_bytes());     // offset 20: 1
    record.extend_from_slice(&(num_cres_slots as u32).to_be_bytes()); // offset 24
    record.extend_from_slice(&num_hd_images.to_be_bytes());           // offset 28
    record.extend_from_slice(&(kindle_embed_index as u32).to_be_bytes()); // offset 32
    record.extend_from_slice(&1u32.to_be_bytes());     // offset 36: 1
    record.extend_from_slice(&(exth_offset as u32).to_be_bytes()); // offset 40
    record.extend_from_slice(&(title_len as u32).to_be_bytes());   // offset 44

    // EXTH block
    record.extend_from_slice(&exth_block);

    // Padded title
    record.extend_from_slice(title_raw);
    while record.len() < total_size {
        record.push(0x00);
    }

    record
}

/// Encode a 1-based record index as base-32 for kindle:embed URLs.
/// Uses digits 0-9 and uppercase A-V (32 characters), 4 chars zero-padded.
fn encode_kindle_embed_base32(recindex: usize) -> String {
    const CHARS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut result = [b'0'; 4];
    let mut v = recindex;
    for i in (0..4).rev() {
        result[i] = CHARS[v % 32];
        v /= 32;
    }
    String::from_utf8(result.to_vec()).unwrap()
}

/// Get image dimensions (width, height) from JPEG or PNG image data.
///
/// For JPEG: parses SOF markers to find dimensions.
/// For PNG: reads IHDR chunk.
/// Returns None if the format is unrecognized or dimensions can't be determined.
fn get_image_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    if data.len() < 24 {
        return None;
    }

    // JPEG: starts with FF D8
    if data[0] == 0xFF && data[1] == 0xD8 {
        return get_jpeg_dimensions(data);
    }

    // PNG: starts with 89 50 4E 47 0D 0A 1A 0A
    if data.len() >= 24 && data[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        // IHDR chunk starts at offset 8: length(4) + "IHDR"(4) + width(4) + height(4)
        if &data[12..16] == b"IHDR" {
            let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
            let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
            return Some((w, h));
        }
    }

    // GIF: starts with "GIF"
    if data.len() >= 10 && &data[0..3] == b"GIF" {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some((w, h));
    }

    None
}

/// Parse JPEG SOF markers to get image dimensions.
fn get_jpeg_dimensions(data: &[u8]) -> Option<(u32, u32)> {
    let mut i = 0;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        match marker {
            0xD8 => { // SOI
                i += 2;
            }
            0xD9 | 0xDA => { // EOI or SOS - stop searching
                break;
            }
            // SOF markers: C0, C1, C2, C3
            0xC0..=0xC3 => {
                if i + 9 <= data.len() {
                    let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                    let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                    return Some((w, h));
                }
                break;
            }
            0x00 => {
                // Stuffed byte after FF - not a marker
                i += 2;
            }
            _ => {
                // Other marker with length field
                if i + 4 <= data.len() {
                    let length = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
                    i += 2 + length;
                } else {
                    break;
                }
            }
        }
    }
    None
}

/// Get the individual cleaned HTML parts for each spine item (for KF8).
///
/// Returns the cleaned HTML content of each file as a separate string,
/// not merged into one document.
fn build_html_parts(opf: &OPFData) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for html_path in opf.get_content_html_paths() {
        let content = std::fs::read_to_string(&html_path).unwrap_or_default();
        let cleaned = clean_book_html(&content);
        parts.push(cleaned);
    }
    parts
}

/// Extract CSS content from the OPF manifest.
///
/// Reads all CSS files referenced in the manifest and concatenates them.
fn extract_css_content(opf: &OPFData) -> String {
    let mut css_parts: Vec<String> = Vec::new();

    for (_, (href, media_type)) in &opf.manifest {
        if media_type == "text/css" || href.ends_with(".css") {
            let css_path = opf.base_dir.join(href);
            if let Ok(content) = std::fs::read_to_string(&css_path) {
                css_parts.push(content);
            }
        }
    }

    css_parts.join("\n")
}

/// Read and concatenate all spine HTML files into a single text blob.
///
/// When `strip_idx` is true (dictionary mode), idx: namespace markup is stripped.
/// When false (book mode), HTML is cleaned minimally.
fn build_text_content(opf: &OPFData, strip_idx: bool) -> Vec<u8> {
    let mut parts: Vec<String> = Vec::new();

    for html_path in opf.get_content_html_paths() {
        let content = std::fs::read_to_string(&html_path).unwrap_or_default();
        let cleaned = if strip_idx {
            strip_idx_markup(&content)
        } else {
            clean_book_html(&content)
        };
        parts.push(cleaned);
    }

    // Merge all HTML files into a single document
    let body_re = Regex::new(r"(?s)<body[^>]*>(.*?)</body>").unwrap();
    let head_re = Regex::new(r"(?s)<head[^>]*>.*?</head>").unwrap();

    let mut body_contents: Vec<String> = Vec::new();
    let mut first_head: Option<String> = None;

    for part in &parts {
        if let Some(cap) = body_re.captures(part) {
            body_contents.push(cap.get(1).unwrap().as_str().trim().to_string());
        } else {
            body_contents.push(part.clone());
        }
        if first_head.is_none() {
            if let Some(cap) = head_re.captures(part) {
                first_head = Some(cap.get(0).unwrap().as_str().to_string());
            }
        }
    }

    let head = first_head.unwrap_or_else(|| "<head><guide></guide></head>".to_string());
    let merged_body = body_contents.join("<mbp:pagebreak/>");
    let combined = format!(
        "<html>{}<body>{}  <mbp:pagebreak/></body></html>",
        head, merged_body
    );
    combined.into_bytes()
}

/// Build text content for a dictionary grouped by the first letter of each headword.
///
/// Groups all entries by the first Unicode character of their headword (case-folded),
/// producing one logical section per letter separated by `<mbp:pagebreak/>` tags.
/// Each entry's HTML content has idx: markup stripped for the text blob.
///
/// Non-dictionary spine items (cover, usage guide, copyright, etc.) are included
/// before the dictionary entries so that front matter is preserved.
///
/// This structure follows Amazon's recommendation for dictionary organization and
/// keeps individual logical "file" sizes manageable.
fn build_text_content_by_letter(opf: &OPFData, entries: &[DictionaryEntry]) -> Vec<u8> {
    use std::collections::BTreeMap;

    // Collect non-dictionary spine items (front matter) and extract styles
    let mut front_matter_sections: Vec<String> = Vec::new();
    let mut first_style: Option<String> = None;
    let body_re = Regex::new(r"(?s)<body[^>]*>(.*?)</body>").unwrap();
    let style_re = Regex::new(r"(?s)<style[^>]*>.*?</style>").unwrap();
    for html_path in opf.get_content_html_paths() {
        if let Ok(content) = std::fs::read_to_string(&html_path) {
            if content.contains("<idx:entry") {
                // Dictionary file - extract style from first dictionary file if available
                if first_style.is_none() {
                    let head_re = Regex::new(r"(?s)<head>.*?</head>").unwrap();
                    if let Some(head_match) = head_re.find(&content) {
                        let styles: Vec<String> = style_re
                            .find_iter(head_match.as_str())
                            .map(|m| m.as_str().to_string())
                            .collect();
                        if !styles.is_empty() {
                            first_style = Some(styles.join(""));
                        }
                    }
                }
                continue; // Skip dictionary files, they'll be handled by letter grouping
            }
            // Non-dictionary file: clean it and extract body content
            let cleaned = strip_idx_markup(&content);
            if let Some(cap) = body_re.captures(&cleaned) {
                front_matter_sections.push(cap.get(1).unwrap().as_str().trim().to_string());
            } else {
                front_matter_sections.push(cleaned);
            }
        }
    }

    // Group entries by first character (lowercased)
    let mut letter_groups: BTreeMap<String, Vec<&DictionaryEntry>> = BTreeMap::new();
    for entry in entries {
        let key = entry.headword.chars().next()
            .map(|c| c.to_lowercase().to_string())
            .unwrap_or_else(|| "_".to_string());
        letter_groups.entry(key).or_default().push(entry);
    }

    let num_groups = letter_groups.len();
    eprintln!("Kindle limits: grouped {} entries into {} letter groups", entries.len(), num_groups);

    if num_groups > KINDLE_HTML_FILE_LIMIT {
        eprintln!(
            "Warning: {} letter groups exceeds the Kindle limit of {} HTML files",
            num_groups, KINDLE_HTML_FILE_LIMIT
        );
    }

    // Build body sections per letter group
    let mut body_sections: Vec<String> = Vec::new();

    // Prepend front matter sections
    for fm in &front_matter_sections {
        body_sections.push(fm.clone());
    }

    for (letter, group_entries) in &letter_groups {
        let mut section_parts: Vec<String> = Vec::new();
        for entry in group_entries {
            let stripped = strip_idx_markup(&entry.html_content);
            section_parts.push(stripped);
        }
        let section = section_parts.join("");
        let section_size = section.len();
        if section_size > KINDLE_HTML_SIZE_LIMIT {
            eprintln!(
                "Warning: letter group '{}' HTML is {} bytes, exceeds 30 MB Kindle limit ({} bytes). Splitting further at entry boundaries.",
                letter,
                section_size,
                KINDLE_HTML_SIZE_LIMIT
            );
            // Split this letter's entries into sub-chunks under 30MB
            let mut chunk = String::new();
            for entry in group_entries {
                let stripped = strip_idx_markup(&entry.html_content);
                if !chunk.is_empty() && chunk.len() + stripped.len() > KINDLE_HTML_SIZE_LIMIT {
                    body_sections.push(chunk);
                    chunk = String::new();
                }
                chunk.push_str(&stripped);
            }
            if !chunk.is_empty() {
                body_sections.push(chunk);
            }
        } else {
            body_sections.push(section);
        }
    }

    // Check total section count after potential sub-splitting
    if body_sections.len() > KINDLE_HTML_FILE_LIMIT {
        eprintln!(
            "Warning: {} total HTML sections after splitting exceeds the Kindle limit of {} files",
            body_sections.len(), KINDLE_HTML_FILE_LIMIT
        );
    }

    let style_block = first_style.unwrap_or_default();
    let merged_body = body_sections.join("<mbp:pagebreak/>");
    let combined = format!(
        "<html><head>{}<guide></guide></head><body>{}  <mbp:pagebreak/></body></html>",
        style_block, merged_body
    );
    combined.into_bytes()
}

/// Strip idx: namespace tags from HTML, keeping only display content.
fn strip_idx_markup(html: &str) -> String {
    let mut result = html.to_string();

    // Remove XML declarations
    let xml_decl = Regex::new(r"<\?xml[^?]*\?>\s*").unwrap();
    result = xml_decl.replace_all(&result, "").to_string();

    // Remove xmlns:* attributes
    let xmlns = Regex::new(r#"\s+xmlns:\w+="[^"]*""#).unwrap();
    result = xmlns.replace_all(&result, "").to_string();

    // Extract any <style>...</style> blocks from the <head> before replacing it
    let style_re = Regex::new(r"(?s)<style[^>]*>.*?</style>").unwrap();
    let head_re = Regex::new(r"(?s)<head>.*?</head>").unwrap();
    let style_block: String = head_re
        .find(&result)
        .map(|head_match| {
            style_re
                .find_iter(head_match.as_str())
                .map(|m| m.as_str().to_string())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let new_head = if style_block.is_empty() {
        "<head><guide></guide></head>".to_string()
    } else {
        format!("<head>{}<guide></guide></head>", style_block)
    };
    result = head_re
        .replace_all(&result, new_head.as_str())
        .to_string();

    // Remove idx:iform tags entirely
    let iform = Regex::new(r"<idx:iform[^/]*/>\s*").unwrap();
    result = iform.replace_all(&result, "").to_string();

    // Remove idx:infl tags and content
    let infl_empty = Regex::new(r"<idx:infl>\s*</idx:infl>\s*").unwrap();
    result = infl_empty.replace_all(&result, "").to_string();

    let infl_full = Regex::new(r"(?s)\s*<idx:infl>.*?</idx:infl>\s*").unwrap();
    result = infl_full.replace_all(&result, "").to_string();

    // Remove idx:orth tags but keep inner content
    let orth_self = Regex::new(r"<idx:orth[^>]*/>").unwrap();
    result = orth_self.replace_all(&result, "").to_string();

    let orth_open = Regex::new(r"<idx:orth[^>]*>").unwrap();
    result = orth_open.replace_all(&result, "").to_string();

    let orth_close = Regex::new(r"</idx:orth>").unwrap();
    result = orth_close.replace_all(&result, "").to_string();

    // Remove idx:short tags but keep inner content
    let short_open = Regex::new(r"<idx:short>\s*").unwrap();
    result = short_open.replace_all(&result, "").to_string();

    let short_close = Regex::new(r"\s*</idx:short>").unwrap();
    result = short_close.replace_all(&result, "").to_string();

    // Remove idx:entry open tags but keep inner content;
    // replace close tags with <hr/> to visually separate entries
    let entry_open = Regex::new(r"<idx:entry[^>]*>\s*").unwrap();
    result = entry_open.replace_all(&result, "").to_string();

    let entry_close = Regex::new(r"\s*</idx:entry>").unwrap();
    result = entry_close.replace_all(&result, "<hr/>").to_string();

    // Collapse whitespace
    let ws = Regex::new(r"\s+").unwrap();
    result = ws.replace_all(&result, " ").to_string();

    // Clean up spaces around HTML tags
    let tag_space = Regex::new(r">\s+<").unwrap();
    result = tag_space.replace_all(&result, "><").to_string();

    // Restore important spaces
    result = result.replace("</b><", "</b> <");
    result = result.replace("</p><hr", "</p> <hr");
    result = result.replace("/><b>", "/> <b>");

    result.trim().to_string()
}

/// Clean book HTML for non-dictionary content.
///
/// Minimal cleanup: removes XML declarations and xmlns attributes,
/// but preserves all HTML content as-is (no idx markup to strip).
fn clean_book_html(html: &str) -> String {
    let mut result = html.to_string();

    // Remove XML declarations
    let xml_decl = Regex::new(r"<\?xml[^?]*\?>\s*").unwrap();
    result = xml_decl.replace_all(&result, "").to_string();

    // Remove xmlns:* attributes
    let xmlns = Regex::new(r#"\s+xmlns:\w+="[^"]*""#).unwrap();
    result = xmlns.replace_all(&result, "").to_string();

    // Remove the default xmlns attribute too (common in EPUB XHTML)
    let xmlns_default = Regex::new(r#"\s+xmlns="[^"]*""#).unwrap();
    result = xmlns_default.replace_all(&result, "").to_string();

    result.trim().to_string()
}

/// Rewrite image `src="..."` attributes to `recindex="NNNNN"` in the text content.
///
/// The src paths in HTML files may be relative to the HTML file's own location
/// (e.g., `../Images/foo.jpg` from `Text/chapter1.xhtml`), so we need to try
/// multiple path resolution strategies to match against the manifest hrefs.
fn rewrite_image_src(
    text_bytes: &[u8],
    href_to_recindex: &std::collections::HashMap<String, usize>,
    spine_items: &[(String, String)],
) -> Vec<u8> {
    let text = String::from_utf8_lossy(text_bytes);

    // Build a lookup that maps various path forms to recindex.
    // For each manifest href like "Images/cover.jpg", we want to match:
    // - "Images/cover.jpg" (exact)
    // - "../Images/cover.jpg" (relative from a subdirectory)
    // - "cover.jpg" (filename only)
    let mut path_to_recindex: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (href, &recindex) in href_to_recindex {
        // Exact manifest href
        path_to_recindex.insert(href.clone(), recindex);

        // Also try with URL-decoded form (spaces encoded as %20, etc.)
        let decoded = percent_decode(href);
        if decoded != *href {
            path_to_recindex.insert(decoded, recindex);
        }

        // Filename only (last path component)
        if let Some(fname) = href.rsplit('/').next() {
            path_to_recindex.entry(fname.to_string()).or_insert(recindex);
        }
    }

    // For relative paths from spine HTML locations, resolve "../" prefixes
    // by computing what each spine item's relative reference would be.
    for (_, spine_href) in spine_items {
        if let Some(spine_dir) = spine_href.rsplit_once('/') {
            let spine_dir = spine_dir.0; // e.g., "Text" from "Text/chapter1.xhtml"
            for (href, &recindex) in href_to_recindex {
                // Compute relative path from spine_dir to href
                // Common case: spine is "Text/ch1.xhtml", image is "Images/foo.jpg"
                // Relative path would be "../Images/foo.jpg"
                let relative = format!("../{}", href);
                path_to_recindex.entry(relative).or_insert(recindex);

                // Also try if spine and image share a common root
                if let Some(img_dir) = href.rsplit_once('/') {
                    if spine_dir != img_dir.0 {
                        let relative2 = format!("../{}", href);
                        path_to_recindex.entry(relative2).or_insert(recindex);
                    } else {
                        // Same directory - just the filename
                        let fname = img_dir.1;
                        path_to_recindex.entry(fname.to_string()).or_insert(recindex);
                    }
                }
            }
        }
    }

    // Replace src="..." with recindex="NNNNN" using regex
    let src_re = Regex::new(r#"(?i)\bsrc\s*=\s*"([^"]*)""#).unwrap();
    let result = src_re.replace_all(&text, |caps: &regex::Captures| {
        let src_path = caps.get(1).unwrap().as_str();
        // Try to match the src path
        if let Some(&recindex) = path_to_recindex.get(src_path) {
            format!("recindex=\"{:05}\"", recindex)
        } else {
            // Try URL-decoded version
            let decoded = percent_decode(src_path);
            if let Some(&recindex) = path_to_recindex.get(&decoded) {
                format!("recindex=\"{:05}\"", recindex)
            } else {
                // Try filename-only match
                if let Some(fname) = src_path.rsplit('/').next() {
                    if let Some(&recindex) = path_to_recindex.get(fname) {
                        format!("recindex=\"{:05}\"", recindex)
                    } else {
                        // Keep original - not an image we know about
                        caps.get(0).unwrap().as_str().to_string()
                    }
                } else {
                    caps.get(0).unwrap().as_str().to_string()
                }
            }
        }
    });

    result.into_owned().into_bytes()
}

/// Simple percent-decoding for URL-encoded paths (handles %20, %2F, etc.)
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(byte) = u8::from_str_radix(
                    &format!("{}{}", h1 as char, h2 as char),
                    16,
                ) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
        } else {
            result.push(b as char);
        }
    }
    result
}

/// Insert the guide reference tag with the correct filepos.
fn insert_guide_reference(text_bytes: &[u8]) -> Vec<u8> {
    let empty_guide = b"<guide></guide>";

    let guide_pos = match find_bytes(text_bytes, empty_guide) {
        Some(pos) => pos,
        None => return text_bytes.to_vec(),
    };

    let first_b = match find_bytes(text_bytes, b"<b>") {
        Some(pos) => pos,
        None => return text_bytes.to_vec(),
    };

    // The reference tag template has a fixed-width filepos (10 digits)
    let ref_template_zero = b"<guide><reference title=\"IndexName\" type=\"index\"  filepos=0000000000 /></guide>";
    let insert_delta = ref_template_zero.len() - empty_guide.len();

    let filepos = first_b + insert_delta;
    let full_guide = format!(
        "<guide><reference title=\"IndexName\" type=\"index\"  filepos={:010} /></guide>",
        filepos
    );

    let mut result = Vec::with_capacity(text_bytes.len() + insert_delta);
    result.extend_from_slice(&text_bytes[..guide_pos]);
    result.extend_from_slice(full_guide.as_bytes());
    result.extend_from_slice(&text_bytes[guide_pos + empty_guide.len()..]);
    result
}

/// Threshold for parallel compression (1 MB).
const PARALLEL_THRESHOLD: usize = 1024 * 1024;

/// Compress text into PalmDOC records with trailing bytes.
///
/// Uses std::thread for parallel compression on large inputs (>1 MB)
/// since each chunk is independent.
fn compress_text(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();

    // Scale chunk size if needed for >65000 records
    let mut chunk_size = RECORD_SIZE;
    if total_length / chunk_size > 65000 {
        chunk_size = (total_length / 65000) + 1;
        chunk_size = chunk_size.next_power_of_two();
    }

    // Split into owned chunks for thread safety
    let chunks: Vec<Vec<u8>> = text_bytes
        .chunks(chunk_size)
        .map(|c| c.to_vec())
        .collect();

    let records = if total_length > PARALLEL_THRESHOLD && chunks.len() > 1 {
        // Parallel compression using std::thread
        let num_workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(chunks.len());
        eprintln!(
            "  Using {} workers for parallel compression ({} chunks)...",
            num_workers,
            chunks.len()
        );

        // Split work into batches for each thread
        let chunk_count = chunks.len();
        let chunks = std::sync::Arc::new(chunks);
        let mut handles = Vec::with_capacity(num_workers);

        // Each thread processes a strided slice of chunks
        for worker_id in 0..num_workers {
            let chunks = std::sync::Arc::clone(&chunks);
            handles.push(std::thread::spawn(move || {
                let mut results: Vec<(usize, Vec<u8>)> = Vec::new();
                let mut idx = worker_id;
                while idx < chunk_count {
                    let mut compressed = palmdoc::compress(&chunks[idx]);
                    compressed.push(0x00);
                    compressed.push(0x81);
                    results.push((idx, compressed));
                    idx += num_workers;
                }
                results
            }));
        }

        // Collect results and sort by original index
        let mut indexed_results: Vec<(usize, Vec<u8>)> = Vec::with_capacity(chunk_count);
        for handle in handles {
            indexed_results.extend(handle.join().unwrap());
        }
        indexed_results.sort_by_key(|(idx, _)| *idx);
        indexed_results.into_iter().map(|(_, data)| data).collect()
    } else {
        // Sequential compression for small data
        chunks
            .iter()
            .map(|chunk| {
                let mut compressed = palmdoc::compress(chunk);
                compressed.push(0x00);
                compressed.push(0x81);
                compressed
            })
            .collect()
    };

    (records, total_length)
}

/// Split text into uncompressed records with trailing bytes.
fn split_text_uncompressed(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();

    let mut chunk_size = RECORD_SIZE;
    if total_length / chunk_size > 65000 {
        chunk_size = (total_length / 65000) + 1;
        chunk_size = chunk_size.next_power_of_two();
    }

    let records: Vec<Vec<u8>> = text_bytes
        .chunks(chunk_size)
        .map(|chunk| {
            let mut rec = chunk.to_vec();
            rec.push(0x00);
            rec.push(0x81);
            rec
        })
        .collect();

    (records, total_length)
}

/// Find the byte position of each dictionary entry in the stripped text.
///
/// Searches for `<b>headword</b>` at entry boundaries to avoid matching
/// headword text inside etymologies, definitions, example sentences, or
/// cross-reference links. Entry headings always follow `<hr/>` (or are at
/// the start of the text body). If a `<b>headword</b>` match is inside
/// an example sentence or other content, it is skipped.
///
/// Falls back to bare headword search if no bold match is found.
fn find_entry_positions(text_bytes: &[u8], entries: &[DictionaryEntry]) -> Vec<(usize, usize)> {
    let mut positions = Vec::with_capacity(entries.len());
    let mut search_start: usize = 0;

    for entry in entries {
        let headword_bytes = entry.headword.as_bytes();

        // Build <b>headword</b> needle
        let mut bold_needle = Vec::with_capacity(3 + headword_bytes.len() + 4);
        bold_needle.extend_from_slice(b"<b>");
        bold_needle.extend_from_slice(headword_bytes);
        bold_needle.extend_from_slice(b"</b>");

        // Search for <b>headword</b> at an entry boundary.
        // Entry headings are preceded by "<hr/> " or "/> " (after <br/>) or
        // appear near the start of the body. Skip matches inside example
        // sentences or other content.
        let mut found = None;
        let mut scan_from = search_start;

        loop {
            match find_bytes_from(text_bytes, &bold_needle, scan_from) {
                Some(bold_pos) => {
                    if is_entry_boundary(text_bytes, bold_pos) {
                        found = Some((bold_pos, bold_pos + 3));
                        break;
                    }
                    // Not at entry boundary: skip this match and keep searching
                    scan_from = bold_pos + bold_needle.len();
                }
                None => break,
            }
        }

        let (block_start, pos) = match found {
            Some(result) => result,
            None => {
                // Fallback: search for bare headword (for entries without <b> tags)
                match find_bytes_from(text_bytes, headword_bytes, search_start) {
                    Some(p) => {
                        let search_from = if p >= 10 { p - 10 } else { 0 };
                        let bs = match rfind_bytes(&text_bytes[search_from..p], b"<b>") {
                            Some(rel) => search_from + rel,
                            None => p,
                        };
                        (bs, p)
                    }
                    None => {
                        positions.push((0, 0));
                        continue;
                    }
                }
            }
        };

        // Find the end of the definition
        let hr_pos = find_bytes_from(text_bytes, b"<hr/>", pos);
        let text_len = match hr_pos {
            Some(hr) => hr - block_start,
            None => {
                let block_end =
                    find_bytes_from(text_bytes, b"<mbp:pagebreak/>", pos).unwrap_or(text_bytes.len());
                block_end - block_start
            }
        };

        positions.push((block_start, text_len));
        search_start = pos + headword_bytes.len();
    }

    positions
}

/// Check if a `<b>` tag position is at an entry boundary (a headword heading).
///
/// Entry headings in the stripped text are preceded by `<hr/> ` or `"/> `, or
/// appear near the start of the text (first entry in the body). Bold text
/// inside example sentences or etymologies is preceded by `>` from a `<p>` or
/// `<i>` tag, not by `<hr/>`.
fn is_entry_boundary(text_bytes: &[u8], bold_pos: usize) -> bool {
    // First entry: near the start of the body
    if bold_pos < 200 {
        return true;
    }

    // Look backward from the <b> position for the preceding context.
    // Entry headings are preceded by: <hr/> <b>  (with space between)
    // or by: /> <b> (after <br/> or self-closing tags at end of prev entry)
    let check_start = if bold_pos >= 8 { bold_pos - 8 } else { 0 };
    let preceding = &text_bytes[check_start..bold_pos];

    // Check for "<hr/> " immediately before
    if preceding.ends_with(b"<hr/> ") || preceding.ends_with(b"<hr/>") {
        return true;
    }

    // Check for "/> " (after <br/> or self-closing tags)
    if preceding.ends_with(b"/> ") {
        return true;
    }

    false
}

/// Build the complete list of lookup terms for the orth index.
fn build_lookup_terms(
    entries: &[DictionaryEntry],
    positions: &[(usize, usize)],
    text_bytes: &[u8],
    headwords_only: bool,
) -> Vec<LookupTerm> {
    use std::collections::HashMap;

    let mut terms: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
    let mut headwords: HashSet<String> = HashSet::new();

    // First pass: register all headwords
    for (entry_ordinal, (entry, &(start_pos, text_len))) in
        entries.iter().zip(positions.iter()).enumerate()
    {
        let hw = &entry.headword;
        let hw_bytes = hw.as_bytes();
        let mut hw_display_len = 3 + hw_bytes.len() + 4 + 1; // <b> + hw + </b> + space

        // Verify against actual text
        if start_pos > 0 && start_pos + hw_display_len <= text_bytes.len() {
            let mut expected = Vec::new();
            expected.extend_from_slice(b"<b>");
            expected.extend_from_slice(hw_bytes);
            expected.extend_from_slice(b"</b> ");
            let actual = &text_bytes[start_pos..start_pos + hw_display_len];
            if actual != expected.as_slice() {
                hw_display_len = 3 + hw_bytes.len() + 4; // without trailing space
            }
        }

        terms.insert(
            hw.clone(),
            (start_pos, text_len, hw_display_len, entry_ordinal),
        );
        headwords.insert(hw.clone());
    }

    // Second pass: add inflected forms
    if !headwords_only {
        let bad_chars: HashSet<char> = "()[]{}".chars().collect();
        for (entry_ordinal, (entry, &(start_pos, text_len))) in
            entries.iter().zip(positions.iter()).enumerate()
        {
            for iform in &entry.inflections {
                if !terms.contains_key(iform)
                    && !iform.chars().any(|c| bad_chars.contains(&c))
                {
                    let hw = &entry.headword;
                    let hw_display_len = if let Some((_, _, hdl, _)) = terms.get(hw) {
                        *hdl
                    } else {
                        3 + iform.as_bytes().len() + 4 + 1
                    };
                    terms.insert(
                        iform.clone(),
                        (start_pos, text_len, hw_display_len, entry_ordinal),
                    );
                }
            }
        }
    }

    // Precompute binary encoding for each label
    eprintln!("Encoding {} unique lookup terms...", terms.len());
    let mut label_bytes_map: HashMap<String, Vec<u8>> = HashMap::new();
    for label in terms.keys() {
        label_bytes_map.insert(label.clone(), encode_indx_label(label));
    }

    // Sort by encoded form
    let mut sorted_labels: Vec<String> = terms.keys().cloned().collect();
    sorted_labels.sort_by(|a, b| label_bytes_map[a].cmp(&label_bytes_map[b]));

    sorted_labels
        .into_iter()
        .map(|label| {
            let (start_pos, text_len, hw_display_len, source_ordinal) = terms[&label];
            LookupTerm {
                label: label.clone(),
                label_bytes: label_bytes_map[&label].clone(),
                start_pos,
                text_len,
                headword_display_len: hw_display_len,
                source_ordinal,
            }
        })
        .collect()
}

/// Build record 0: PalmDOC header + MOBI header + EXTH header + full name.
///
/// `override_version`: if Some, overrides the MOBI version/min_version
/// (e.g., Some(6) for KF7 in dual-format mode).
/// `kf8_boundary_record`: if Some, adds EXTH 121 pointing to KF8 Record 0.
/// `hd_geometry`: if Some, adds EXTH 536 with HD image geometry (format: "WxH:start-end|").
fn build_record0(
    opf: &OPFData,
    text_length: usize,
    text_record_count: usize,
    first_non_book_record: usize,
    orth_index_record: usize,
    total_records: usize,
    flis_record: usize,
    fcis_record: usize,
    no_compress: bool,
    headword_chars: &HashSet<u32>,
    is_dictionary: bool,
    first_image_record: usize,
    cover_offset: Option<u32>,
    fixed_layout: Option<&exth::FixedLayoutMeta>,
    override_version: Option<u32>,
    kf8_boundary_record: Option<u32>,
    srcs_record: Option<usize>,
    hd_geometry: Option<&str>,
    creator_tag: bool,
    doc_type: Option<&str>,
) -> Vec<u8> {
    let default_name = if is_dictionary { "Dictionary" } else { "Book" };
    let full_name = if opf.title.is_empty() {
        default_name
    } else {
        &opf.title
    };
    let full_name_bytes = full_name.as_bytes();

    // PalmDOC header (16 bytes)
    let compression_type: u16 = if no_compress { 1 } else { 2 };
    let mut record_size = RECORD_SIZE;
    let mut text_rec_count = text_record_count;
    if text_rec_count > 65000 {
        record_size = std::cmp::max(RECORD_SIZE, (text_length / 65000) + 1);
        text_rec_count = std::cmp::min(text_rec_count, 65535);
    }

    let mut palmdoc = Vec::with_capacity(16);
    palmdoc.extend_from_slice(&compression_type.to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    palmdoc.extend_from_slice(&(text_length as u32).to_be_bytes());
    palmdoc.extend_from_slice(&(text_rec_count as u16).to_be_bytes());
    palmdoc.extend_from_slice(&(record_size as u16).to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    assert_eq!(palmdoc.len(), 16);

    // MOBI header
    let mut mobi = vec![0u8; MOBI_HEADER_LENGTH];
    put_bytes(&mut mobi, 0, b"MOBI");
    put32(&mut mobi, 4, MOBI_HEADER_LENGTH as u32);
    // MOBI type: 2 = MOBI book, but kindlegen uses 2 for both books and dicts
    put32(&mut mobi, 8, 2);
    put32(&mut mobi, 12, 65001); // UTF-8

    // Unique ID from title hash
    let uid_hash = md5_simple(full_name.as_bytes());
    let unique_id = u32::from_be_bytes([uid_hash[0], uid_hash[1], uid_hash[2], uid_hash[3]]);
    put32(&mut mobi, 16, unique_id);

    let version = override_version.unwrap_or(7);
    put32(&mut mobi, 20, version); // file version
    put32(&mut mobi, 24, orth_index_record as u32);
    put32(&mut mobi, 28, 0xFFFFFFFF); // inflection index (none)
    put32(&mut mobi, 32, 0xFFFFFFFF); // index names
    put32(&mut mobi, 36, 0xFFFFFFFF); // index keys
    for off in (40..64).step_by(4) {
        put32(&mut mobi, off, 0xFFFFFFFF); // extra indices
    }
    put32(&mut mobi, 64, first_non_book_record as u32);

    put32(&mut mobi, 76, locale_code(&opf.language));
    put32(&mut mobi, 80, locale_code(&opf.dict_in_language));
    put32(&mut mobi, 84, locale_code(&opf.dict_out_language));
    put32(&mut mobi, 88, version); // min version = same as file version
    put32(&mut mobi, 92, first_image_record as u32); // first image record
    put32(&mut mobi, 96, 0); // huffman record
    put32(&mut mobi, 100, 0); // huffman count

    // EXTH flags / locale marker at offset 112.
    // Dictionaries: 0x50 (bit 6 = EXTH present, bit 4 set) - matches Kindle Previewer output.
    // Books: 0x4850 required for Kindle Previewer compatibility.
    // Using 0x4850 for dictionaries breaks dictionary recognition on Kindle devices.
    if is_dictionary {
        put32(&mut mobi, 112, 0x50);
    } else {
        put32(&mut mobi, 112, 0x4850);
    }

    put32(&mut mobi, 148, 0xFFFFFFFF); // DRM flags
    put32(&mut mobi, 152, 0xFFFFFFFF);

    // FDST flow count
    put32(
        &mut mobi,
        176,
        (1u32 << 16) | ((total_records - 1) as u32),
    );
    put32(&mut mobi, 180, 1);

    // FLIS/FCIS pointers (kindlegen puts FCIS at 184, FLIS at 192)
    put32(&mut mobi, 184, fcis_record as u32);
    put32(&mut mobi, 188, 1);
    put32(&mut mobi, 192, flis_record as u32);
    put32(&mut mobi, 196, 1);

    // Extra record data flags (multibyte + TBS)
    put32(&mut mobi, 224, 3);

    // NCX and other indices: 0xFFFFFFFF
    put32(&mut mobi, 216, 0xFFFFFFFF);
    put32(&mut mobi, 220, 0xFFFFFFFF);
    put32(&mut mobi, 228, 0xFFFFFFFF);
    put32(&mut mobi, 232, 0xFFFFFFFF);
    put32(&mut mobi, 236, 0xFFFFFFFF);
    put32(&mut mobi, 240, 0xFFFFFFFF);

    // SRCS record index and count
    // Offset 208/212 is where Kindle Previewer looks for SRCS
    // Offset 244/248 is documented on MobileRead wiki
    // Set both for compatibility
    if let Some(srcs_idx) = srcs_record {
        put32(&mut mobi, 208, srcs_idx as u32);
        put32(&mut mobi, 212, 1);
        put32(&mut mobi, 244, srcs_idx as u32);
        put32(&mut mobi, 248, 1);
    } else {
        put32(&mut mobi, 244, 0xFFFFFFFF);
        put32(&mut mobi, 248, 0xFFFFFFFF);
    }

    put32(&mut mobi, 256, 0xFFFFFFFF);

    // Build EXTH header
    let exth_data = if is_dictionary {
        exth::build_exth(
            full_name,
            &opf.author,
            &opf.date,
            &opf.language,
            &opf.dict_in_language,
            &opf.dict_out_language,
            headword_chars,
            creator_tag,
        )
    } else {
        exth::build_book_exth(
            full_name,
            &opf.author,
            &opf.date,
            &opf.language,
            cover_offset,
            fixed_layout,
            kf8_boundary_record,
            hd_geometry,
            creator_tag,
            doc_type,
            None, // description
            None, // subject
            None, // series
            None, // series_index
        )
    };

    // Full name offset
    let full_name_offset = 16 + MOBI_HEADER_LENGTH + exth_data.len();
    put32(&mut mobi, 68, full_name_offset as u32);
    put32(&mut mobi, 72, full_name_bytes.len() as u32);

    // Assemble record 0
    let mut record0 = Vec::new();
    record0.extend_from_slice(&palmdoc);
    record0.extend_from_slice(&mobi);
    record0.extend_from_slice(&exth_data);
    record0.extend_from_slice(full_name_bytes);

    // Pad to 4-byte boundary
    while record0.len() % 4 != 0 {
        record0.push(0x00);
    }

    record0
}

/// Build KF8 Record 0: PalmDOC header + MOBI header (version=8) + EXTH + full name.
///
/// All record indices are KF8-relative (relative to this record as index 0).
/// In KF8-only mode, `srcs_record` points to the SRCS record and `hd_geometry`
/// provides the HD image geometry string. `total_records` is used to set the
/// MOBI header field at offset 176 (only meaningful in KF8-only mode).
fn build_kf8_record0(
    opf: &OPFData,
    text_length: usize,
    text_record_count: usize,
    first_non_book_record: usize,
    fdst_record: usize,
    fdst_flow_count: usize,
    skeleton_indx_record: usize,
    fragment_indx_record: usize,
    ncx_record: usize,
    datp_record: usize,
    flis_record: usize,
    fcis_record: usize,
    no_compress: bool,
    cover_offset: Option<u32>,
    fixed_layout: Option<&exth::FixedLayoutMeta>,
    first_image_record: usize,
    creator_tag: bool,
    srcs_record: Option<usize>,
    hd_geometry: Option<&str>,
    total_records: usize,
    doc_type: Option<&str>,
) -> Vec<u8> {
    let full_name = if opf.title.is_empty() {
        "Book"
    } else {
        &opf.title
    };
    let full_name_bytes = full_name.as_bytes();

    // PalmDOC header (16 bytes)
    let compression_type: u16 = if no_compress { 1 } else { 2 };
    let mut record_size = RECORD_SIZE;
    let mut text_rec_count = text_record_count;
    if text_rec_count > 65000 {
        record_size = std::cmp::max(RECORD_SIZE, (text_length / 65000) + 1);
        text_rec_count = std::cmp::min(text_rec_count, 65535);
    }

    let mut palmdoc = Vec::with_capacity(16);
    palmdoc.extend_from_slice(&compression_type.to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    palmdoc.extend_from_slice(&(text_length as u32).to_be_bytes());
    palmdoc.extend_from_slice(&(text_rec_count as u16).to_be_bytes());
    palmdoc.extend_from_slice(&(record_size as u16).to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    palmdoc.extend_from_slice(&0u16.to_be_bytes());
    assert_eq!(palmdoc.len(), 16);

    // MOBI header
    let mut mobi = vec![0u8; MOBI_HEADER_LENGTH];
    put_bytes(&mut mobi, 0, b"MOBI");
    put32(&mut mobi, 4, MOBI_HEADER_LENGTH as u32);
    put32(&mut mobi, 8, 2); // MOBI type = book
    put32(&mut mobi, 12, 65001); // UTF-8

    // Unique ID from title hash
    let uid_hash = md5_simple(full_name.as_bytes());
    let unique_id = u32::from_be_bytes([uid_hash[0], uid_hash[1], uid_hash[2], uid_hash[3]]);
    put32(&mut mobi, 16, unique_id);

    put32(&mut mobi, 20, 8); // file version = 8 (KF8)
    put32(&mut mobi, 24, 0xFFFFFFFF); // orth index (none for KF8 books)
    put32(&mut mobi, 28, 0xFFFFFFFF); // inflection index
    put32(&mut mobi, 32, 0xFFFFFFFF); // index names
    put32(&mut mobi, 36, 0xFFFFFFFF); // index keys
    for off in (40..64).step_by(4) {
        put32(&mut mobi, off, 0xFFFFFFFF); // extra indices
    }
    put32(&mut mobi, 64, first_non_book_record as u32);

    put32(&mut mobi, 76, locale_code(&opf.language));
    put32(&mut mobi, 80, 0); // no dict_in for KF8
    put32(&mut mobi, 84, 0); // no dict_out for KF8
    put32(&mut mobi, 88, 8); // min version = 8
    put32(&mut mobi, 92, first_image_record as u32);
    put32(&mut mobi, 96, 0); // huffman record
    put32(&mut mobi, 100, 0); // huffman count

    put32(&mut mobi, 112, 0x4850); // locale/capability marker (required by Kindle Previewer)

    put32(&mut mobi, 148, 0xFFFFFFFF); // DRM flags
    put32(&mut mobi, 152, 0xFFFFFFFF);

    // FDST record and flow count (KF8-relative)
    put32(&mut mobi, 160, fdst_record as u32);
    put32(&mut mobi, 164, fdst_flow_count as u32);

    // Offset 176: in KF8-only mode, this field mirrors KF7 behavior
    if total_records > 0 {
        // KF8-only: use the same encoding as KF7 record 0
        put32(&mut mobi, 176, (1u32 << 16) | ((total_records - 1) as u32));
        put32(&mut mobi, 180, 1);
    } else {
        // Dual format KF8 section
        put32(&mut mobi, 176, 0);
        put32(&mut mobi, 180, fdst_flow_count as u32);
    }

    // FCIS/FLIS (KF8-relative)
    put32(&mut mobi, 184, fcis_record as u32);
    put32(&mut mobi, 188, 1);
    put32(&mut mobi, 192, flis_record as u32);
    put32(&mut mobi, 196, 1);

    // Skeleton INDX (KF8-relative)
    put32(&mut mobi, 212, skeleton_indx_record as u32);

    // DATP (KF8-relative)
    put32(&mut mobi, 216, datp_record as u32);

    // Fragment INDX (KF8-relative)
    put32(&mut mobi, 220, fragment_indx_record as u32);

    // Extra record data flags (multibyte + TBS)
    put32(&mut mobi, 224, 3);

    // NCX (KF8-relative)
    put32(&mut mobi, 228, ncx_record as u32);

    // Other indices: 0xFFFFFFFF
    put32(&mut mobi, 232, 0xFFFFFFFF);
    put32(&mut mobi, 236, 0xFFFFFFFF);
    put32(&mut mobi, 240, 0xFFFFFFFF);

    // SRCS record index and count (KF8-only mode)
    // Note: offset 208/212 overlap with KF8 INDX fields, so use 244/248 only
    if let Some(srcs_idx) = srcs_record {
        put32(&mut mobi, 244, srcs_idx as u32);
        put32(&mut mobi, 248, 1);
    } else {
        put32(&mut mobi, 244, 0xFFFFFFFF);
        put32(&mut mobi, 248, 0xFFFFFFFF);
    }
    put32(&mut mobi, 256, 0xFFFFFFFF);

    // Build EXTH header
    // In KF8-only mode, include HD geometry (EXTH 536) if present.
    // Never include EXTH 121 (KF8 boundary) since there's no KF7 section.
    let exth_data = exth::build_book_exth(
        full_name,
        &opf.author,
        &opf.date,
        &opf.language,
        cover_offset,
        fixed_layout,
        None, // no KF8 boundary in KF8 header itself
        hd_geometry,
        creator_tag,
        doc_type,
        None, // description
        None, // subject
        None, // series
        None, // series_index
    );

    // Full name offset
    let full_name_offset = 16 + MOBI_HEADER_LENGTH + exth_data.len();
    put32(&mut mobi, 68, full_name_offset as u32);
    put32(&mut mobi, 72, full_name_bytes.len() as u32);

    // Assemble KF8 record 0
    let mut record0 = Vec::new();
    record0.extend_from_slice(&palmdoc);
    record0.extend_from_slice(&mobi);
    record0.extend_from_slice(&exth_data);
    record0.extend_from_slice(full_name_bytes);

    // Pad to 4-byte boundary
    while record0.len() % 4 != 0 {
        record0.push(0x00);
    }

    record0
}

/// Build the FLIS record.
fn build_flis() -> Vec<u8> {
    let mut flis = Vec::with_capacity(36);
    flis.extend_from_slice(b"FLIS");
    flis.extend_from_slice(&8u32.to_be_bytes());
    flis.extend_from_slice(&65u16.to_be_bytes());
    flis.extend_from_slice(&0u16.to_be_bytes());
    flis.extend_from_slice(&0u32.to_be_bytes());
    flis.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
    flis.extend_from_slice(&1u16.to_be_bytes());
    flis.extend_from_slice(&3u16.to_be_bytes());
    flis.extend_from_slice(&3u32.to_be_bytes());
    flis.extend_from_slice(&1u32.to_be_bytes());
    flis.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
    flis
}

/// Build the FCIS record.
fn build_fcis(text_length: usize) -> Vec<u8> {
    let mut fcis = Vec::with_capacity(44);
    fcis.extend_from_slice(b"FCIS");
    fcis.extend_from_slice(&20u32.to_be_bytes());
    fcis.extend_from_slice(&16u32.to_be_bytes());
    fcis.extend_from_slice(&1u32.to_be_bytes());
    fcis.extend_from_slice(&0u32.to_be_bytes());
    fcis.extend_from_slice(&(text_length as u32).to_be_bytes());
    fcis.extend_from_slice(&0u32.to_be_bytes());
    fcis.extend_from_slice(&32u32.to_be_bytes());
    fcis.extend_from_slice(&8u32.to_be_bytes());
    fcis.extend_from_slice(&1u16.to_be_bytes());
    fcis.extend_from_slice(&1u16.to_be_bytes());
    fcis.extend_from_slice(&0u32.to_be_bytes());
    fcis
}

/// Build a CMET (compilation metadata) record.
///
/// This is a simple ASCII string identifying the tool that built the MOBI.
/// Kindle ignores it, but some analysis tools look for it.
fn build_cmet() -> Vec<u8> {
    let version = env!("CARGO_PKG_VERSION");
    format!("kindling {}", version).into_bytes()
}

/// Build the EOF marker record.
fn build_eof() -> Vec<u8> {
    vec![0xE9, 0x8E, 0x0D, 0x0A]
}

/// Build the complete PalmDB file from a list of records.
fn build_palmdb(title: &str, records: &[Vec<u8>]) -> Vec<u8> {
    let num_records = records.len();
    let header_size = 78 + num_records * 8 + 2;

    // Calculate record offsets
    let mut offsets = Vec::with_capacity(num_records);
    let mut current_offset = header_size;
    for rec in records {
        offsets.push(current_offset);
        current_offset += rec.len();
    }

    // Derive PalmDB name from title
    let mut palmdb_name = title.to_string();
    for ch in &['(', ')', '[', ']'] {
        palmdb_name = palmdb_name.replace(*ch, "");
    }
    palmdb_name = palmdb_name.replace(' ', "_");
    if palmdb_name.len() > 27 {
        let first12: String = palmdb_name.chars().take(12).collect();
        let last14: String = palmdb_name
            .chars()
            .rev()
            .take(14)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        palmdb_name = format!("{}-{}", first12, last14);
    }

    let mut name_bytes = [0u8; 32];
    let name_raw = palmdb_name.as_bytes();
    let copy_len = name_raw.len().min(31);
    name_bytes[..copy_len].copy_from_slice(&name_raw[..copy_len]);

    let now = palm_timestamp();

    // Build PalmDB header (78 bytes)
    let mut header = vec![0u8; 78];
    header[0..32].copy_from_slice(&name_bytes);
    put16(&mut header, 32, 0); // attributes
    put16(&mut header, 34, 0); // version
    put32(&mut header, 36, now);
    put32(&mut header, 40, now);
    put32(&mut header, 44, 0); // backup date
    put32(&mut header, 48, 0); // modification number
    put32(&mut header, 52, 0); // app info offset
    put32(&mut header, 56, 0); // sort info offset
    header[60..64].copy_from_slice(b"BOOK");
    header[64..68].copy_from_slice(b"MOBI");
    put32(&mut header, 68, ((num_records - 1) * 2 + 1) as u32); // unique ID seed
    put32(&mut header, 72, 0); // next record list ID
    put16(&mut header, 76, num_records as u16);

    // Record list
    let mut record_list = Vec::with_capacity(num_records * 8);
    for i in 0..num_records {
        record_list.extend_from_slice(&(offsets[i] as u32).to_be_bytes());
        let uid = (i * 2) as u32;
        let attrs_uid = uid & 0x00FFFFFF;
        record_list.extend_from_slice(&attrs_uid.to_be_bytes());
    }

    // 2 bytes gap padding
    let gap = [0u8; 2];

    // Assemble
    let total_size: usize = header.len() + record_list.len() + gap.len()
        + records.iter().map(|r| r.len()).sum::<usize>();
    let mut output = Vec::with_capacity(total_size);
    output.extend_from_slice(&header);
    output.extend_from_slice(&record_list);
    output.extend_from_slice(&gap);
    for rec in records {
        output.extend_from_slice(rec);
    }

    output
}

/// Convert a language code to a MOBI locale code.
fn locale_code(lang: &str) -> u32 {
    match lang {
        "en" => 9,
        "el" => 8,
        "de" => 7,
        "fr" => 12,
        "es" => 10,
        "it" => 16,
        "pt" => 22,
        "nl" => 19,
        "ru" => 25,
        "ja" => 17,
        "zh" => 4,
        "ko" => 18,
        "ar" => 1,
        "he" => 13,
        "tr" => 31,
        _ => 0,
    }
}

/// Get current time as a Palm OS timestamp (seconds since 1904-01-01).
fn palm_timestamp() -> u32 {
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (unix_secs + 2082844800) as u32
}

/// Simple MD5 hash (reusing the same algorithm from exth).
fn md5_simple(data: &[u8]) -> [u8; 16] {
    // Implement inline to avoid circular dependency
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64) * 8;
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());

    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xEFCDAB89;
    let mut c0: u32 = 0x98BADCFE;
    let mut d0: u32 = 0x10325476;

    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
        5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
        6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, word) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([word[0], word[1], word[2], word[3]]);
        }

        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                (a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g])).rotate_left(S[i]),
            );
            a = temp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut result = [0u8; 16];
    result[0..4].copy_from_slice(&a0.to_le_bytes());
    result[4..8].copy_from_slice(&b0.to_le_bytes());
    result[8..12].copy_from_slice(&c0.to_le_bytes());
    result[12..16].copy_from_slice(&d0.to_le_bytes());
    result
}

// --- Byte buffer helpers ---

fn put_bytes(buf: &mut [u8], offset: usize, data: &[u8]) {
    buf[offset..offset + data.len()].copy_from_slice(data);
}

fn put16(buf: &mut [u8], offset: usize, value: u16) {
    buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn put32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

/// Find a byte sequence in a slice, returning the start position.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Find a byte sequence starting from a given position.
fn find_bytes_from(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start >= haystack.len() || needle.is_empty() {
        return None;
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + start)
}

/// Find a byte sequence searching backwards in a slice.
fn rfind_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .rposition(|w| w == needle)
}
