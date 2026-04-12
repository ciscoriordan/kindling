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
use crate::html_check;
use crate::indx::{self, LookupTerm};
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
    self_check: bool,
    kindlegen_parity: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let opf = OPFData::parse(opf_path)?;

    // Detect dictionary vs book by checking HTML content for idx:entry tags
    let is_dictionary = detect_dictionary(&opf);

    if is_dictionary {
        if kf8_only {
            return Err("KF8-only output is not supported for dictionaries (dictionaries use MOBI7 format)".into());
        }
        eprintln!("Detected dictionary content");
        // kindlegen_parity is a no-op for dictionaries (MOBI7 / KF7 path).
        let _ = kindlegen_parity;
        build_dictionary_mobi(&opf, output_path, no_compress, headwords_only, srcs_data, include_cmet, creator_tag, kindle_limits, self_check)
    } else {
        if kf8_only {
            eprintln!("Detected book content, building KF8-only (.azw3)");
        } else {
            eprintln!("Detected book content (no idx:entry tags found)");
        }
        build_book_mobi(&opf, output_path, no_compress, srcs_data, include_cmet, !no_hd_images, creator_tag, kf8_only, doc_type, kindle_limits, self_check, kindlegen_parity)
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
    self_check: bool,
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

    // Collect images from the OPF manifest
    let image_items = opf.get_image_items(); // Vec<(href, media_type)>
    let cover_href = opf.get_cover_image_href();

    let mut image_records: Vec<Vec<u8>> = Vec::new();
    let mut href_to_recindex: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut cover_offset: Option<u32> = None;
    let mut total_image_bytes: usize = 0;

    for (idx, (href, _media_type)) in image_items.iter().enumerate() {
        let recindex = idx + 1; // 1-based
        let image_path = opf.base_dir.join(href);

        let data = std::fs::read(&image_path).or_else(|_| {
            let decoded = percent_decode(href);
            std::fs::read(opf.base_dir.join(&decoded))
        });

        if let Ok(mut data) = data {
            // Patch JFIF density units: Kindle firmware needs DPI units (0x01)
            if data.len() > 13
                && data[0] == 0xFF && data[1] == 0xD8
                && data[2] == 0xFF && data[3] == 0xE0
                && data[6..11] == *b"JFIF\0"
                && data[13] == 0x00
            {
                data[13] = 0x01;
            }
            total_image_bytes += data.len();
            href_to_recindex.insert(href.clone(), recindex);
            image_records.push(data);

            if let Some(ref cover) = cover_href {
                if href == cover {
                    cover_offset = Some(idx as u32);
                }
            }
        } else {
            eprintln!("Warning: could not read image file: {}", image_path.display());
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

    let num_image_records = image_records.len();

    // Build the text content (stripped HTML for all spine items)
    eprintln!("Building text content...");
    let text_content = if kindle_limits {
        build_text_content_by_letter(&opf, &all_entries)
    } else {
        build_text_content(&opf, true)
    };

    // Insert the guide reference tag
    let text_content = insert_guide_reference(&text_content);

    // Rewrite image src attributes to recindex references
    let text_content = if !href_to_recindex.is_empty() {
        rewrite_image_src(&text_content, &href_to_recindex, &opf.spine_items)
    } else {
        text_content
    };

    // Pad the text so every PalmDOC record (except the last) is exactly
    // `RECORD_SIZE` bytes AND ends on a UTF-8 character boundary. Kindle
    // firmware uses byte_offset / text_record_size to route popup
    // lookups (so chunks must be exactly RECORD_SIZE), and the indexer
    // rejects dictionaries with too many records that decode as invalid
    // UTF-8 (so chunks must end at char boundaries). The only way to
    // satisfy both is to insert space-byte padding wherever a 4096-byte
    // boundary would otherwise straddle a multi-byte character. Padding
    // is invisible in HTML rendering and harmless to find_entry_positions
    // because we run that AFTER padding.
    let text_content = pad_text_for_chunking(&text_content, RECORD_SIZE);

    // Self-check: validate the final HTML blob before we split it into
    // records. This is the last chance to notice structural corruption
    // (unclosed tags, `<hr/` garbage, unclosed attribute quotes) before
    // the blob is locked into binary form. The check runs once on the
    // full assembled blob; it does not abort the build, only warns.
    if self_check {
        let issues = html_check::validate_text_blob(&text_content);
        if !issues.is_empty() {
            html_check::print_self_check_warnings(&issues);
        }

        // Per-record balance check: Kindle decodes each record in
        // isolation, so a record that opens <b> without a matching
        // </b> leaks bold state for the rest of the record, and a
        // record ending inside a tag leaves garbage at its start.
        // This check catches regressions in the record splitter.
        let chunk_size = compute_chunk_size(text_content.len());
        let ranges = split_on_utf8_boundaries(&text_content, chunk_size);
        let record_issues = html_check::validate_records(&text_content, &ranges, 20);
        if !record_issues.is_empty() {
            html_check::print_self_check_warnings(&record_issues);
        }
    }

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

    // Build lookup terms + separate infl INDX data.
    //
    // `KINDLING_FLATTEN_INFL` env var (set to "1" or "true") makes the
    // orth INDX ALSO contain every inflected form as a flat entry
    // pointing at its parent headword's (start_pos, text_len). Off by
    // default: only base headwords land in orth; inflected forms are
    // routed through the separate infl INDX. Test-mode toggle while we
    eprintln!("Building lookup terms...");
    let lookup_terms = build_lookup_terms(
        &all_entries,
        &entry_positions,
        &text_content,
        headwords_only,
    );

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
    eprintln!("  Orth INDX: {} records", indx_records.len());

    // Build FLIS, FCIS, EOF records
    let flis = build_flis();
    let fcis = build_fcis(text_length, 1); // dictionaries: 1 flow
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
    // Layout: record0 | text | image records | orth_INDX | infl_INDX | FLIS | FCIS | [SRCS] | [CMET] | EOF
    let first_non_book = text_records.len() + 1;
    let first_image_record = if num_image_records > 0 {
        text_records.len() + 1
    } else {
        0xFFFFFFFF
    };
    let orth_index_record = text_records.len() + 1 + num_image_records;
    let infl_index_record = 0xFFFFFFFFusize;
    let flis_record = orth_index_record + indx_records.len();
    let fcis_record = flis_record + 1;
    let srcs_record_idx = if srcs_record.is_some() {
        Some(fcis_record + 1)
    } else {
        None
    };
    let total_records = 1 + text_records.len() + num_image_records
        + indx_records.len() + 3 + num_optional;

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
        infl_index_record,
        total_records,
        flis_record,
        fcis_record,
        no_compress,
        &headword_chars,
        true, // is_dictionary
        first_image_record,
        cover_offset,
        None, // dictionaries have no library thumbnail slot
        None, // dictionaries have no KF8 cover URI
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
    all_records.extend(image_records);
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
    self_check: bool,
    kindlegen_parity: bool,
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

    // Generate a library-grid thumbnail from the cover image and append it
    // as the last record in the LD image pool. Both EXTH 202 (thumbnail
    // offset) and the MOBI spec require the thumbnail to live in the
    // contiguous image-record range that starts at first_image_record, so
    // we cannot store it elsewhere. The thumbnail record index within
    // `image_records` is tracked so build_hd_container can emit a
    // placeholder CRES slot for it instead of an HD copy.
    let (thumb_offset, thumb_hd_skip): (Option<u32>, std::collections::HashSet<usize>) = {
        let mut hd_skip: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let thumb_off = match cover_offset {
            Some(cov) => {
                let cov_idx = cov as usize;
                let cover_bytes = image_records.get(cov_idx).cloned().unwrap_or_default();
                if cover_bytes.is_empty() {
                    None
                } else if let Some(thumb) = build_thumbnail_record(&cover_bytes) {
                    let thumb_idx = image_records.len();
                    let thumb_len = thumb.len();
                    image_records.push(thumb);
                    let _ = total_image_bytes; // informational total already logged
                    hd_skip.insert(thumb_idx);
                    eprintln!(
                        "Generated {} byte library thumbnail (recindex {}, EXTH 202={})",
                        thumb_len,
                        thumb_idx + 1,
                        thumb_idx,
                    );
                    Some(thumb_idx as u32)
                } else {
                    eprintln!(
                        "Warning: could not decode cover image to generate thumbnail; library tile will fall back to the cover"
                    );
                    None
                }
            }
            None => None,
        };
        (thumb_off, hd_skip)
    };

    // Compute the KF8 cover URI once (same string for both KF7 and KF8
    // sections in dual-format, and for the sole KF8 section in KF8-only).
    // Modern Kindle firmware uses EXTH 129 = "kindle:embed:XXXX" where XXXX
    // is the base32-encoded 1-based recindex of the cover image relative to
    // first_image_record. This is the primary cover-lookup path on Paperwhite
    // 11+, Oasis 3, and Scribe; without it the library thumbnail pipeline
    // falls back to the placeholder and fixed-layout comics fail to open.
    let kf8_cover_uri: Option<String> = cover_offset.map(|off| {
        let recindex = (off as usize) + 1; // 1-based
        format!("kindle:embed:{}", encode_kindle_embed_base32(recindex))
    });

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
    let kf8_title = if opf.title.is_empty() { "Book" } else { &opf.title };
    let kf8_section = crate::kf8::build_kf8_section(
        &html_parts,
        &css_content,
        &href_to_recindex,
        &opf.spine_items,
        no_compress,
        kindlegen_parity,
        kf8_title,
    );
    eprintln!(
        "KF8: {} text records ({} bytes), {} flows",
        kf8_section.text_records.len(),
        kf8_section.text_length,
        kf8_section.flow_count,
    );

    // Self-check: validate the assembled KF8 HTML flow before any records
    // are written. Books, comics, and KF8-only builds all pass through
    // here, so this covers every non-dictionary MOBI path. The check runs
    // once on the full blob and does not abort the build.
    if self_check {
        let issues = html_check::validate_text_blob(&kf8_section.html_bytes);
        if !issues.is_empty() {
            html_check::print_self_check_warnings(&issues);
        }
    }

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
        // CNCX records sit between fragment INDX and skeleton INDX so Kindle
        // firmware can resolve the fragment selectors referenced by the
        // fragment index header (num_of_cncx + walk via PDB record table).
        let kf8_skeleton_start = kf8_fragment_start
            + kf8_section.fragment_indx.len()
            + kf8_section.cncx_records.len();
        let kf8_ncx_start = kf8_skeleton_start + kf8_section.skeleton_indx.len();
        let kf8_fdst_idx = kf8_ncx_start
            + kf8_section.ncx_indx.len()
            + kf8_section.ncx_cncx_records.len();
        // Record order must match kindlegen: FDST, FLIS, FCIS, DATP, EOF
        let kf8_flis_idx = kf8_fdst_idx + 1;
        let kf8_fcis_idx = kf8_flis_idx + 1;
        let kf8_datp_idx = kf8_fcis_idx + 1;
        let kf8_srcs_idx = if srcs_record.is_some() {
            Some(kf8_fcis_idx + 1)
        } else {
            None
        };
        // +2 skips the NULL pad record (text_count+1=NULL, +2=first INDX)
        let kf8_first_nonbook = kf8_text_count + 2;

        // HD container
        let hd_container: Option<HdContainer> = if hd_images && num_image_records > 0 {
            eprintln!("Building HD image container (CONT/CRES)...");
            Some(build_hd_container(opf, &image_records, &thumb_hd_skip))
        } else {
            None
        };
        let hd_record_count = hd_container.as_ref().map_or(0, |hd| hd.total_record_count());
        let hd_geometry_string: Option<String> = hd_container.as_ref().map(|hd| hd.geometry_string());

        let total_records = 1 + kf8_text_count + 1 + num_image_records
            + kf8_section.fragment_indx.len()
            + kf8_section.cncx_records.len()
            + kf8_section.skeleton_indx.len()
            + kf8_section.ncx_indx.len()
            + kf8_section.ncx_cncx_records.len()
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
            thumb_offset,
            kf8_cover_uri.as_deref(),
            fixed_layout.as_ref(),
            kf8_first_image,
            creator_tag,
            kf8_srcs_idx,
            hd_geometry_string.as_deref(),
            total_records,
            doc_type,
        );

        let kf8_flis_rec = build_flis();
        let kf8_fcis_rec = build_fcis(kf8_section.text_length, kf8_section.flow_count);
        let eof = build_eof();
        let null_pad_rec = vec![0x00u8, 0x00u8]; // 2-byte NULL pad (matches KCC)

        // Assemble KF8-only records
        let mut all_records: Vec<Vec<u8>> = Vec::new();
        all_records.push(kf8_record0);
        all_records.extend(kf8_section.text_records);
        all_records.push(null_pad_rec);
        all_records.extend(image_records);
        all_records.extend(kf8_section.fragment_indx);
        all_records.extend(kf8_section.cncx_records);
        all_records.extend(kf8_section.skeleton_indx);
        all_records.extend(kf8_section.ncx_indx);
        all_records.extend(kf8_section.ncx_cncx_records);
        all_records.push(kf8_section.fdst);
        // Order must match kindlegen: FLIS, FCIS, DATP (not DATP first)
        all_records.push(kf8_flis_rec);
        all_records.push(kf8_fcis_rec);
        all_records.push(kf8_section.datp);
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
        // CNCX records follow the fragment INDX and precede the skeleton INDX
        // so the Kindle firmware can walk the PDB record table to resolve the
        // CNCX references made by the fragment index header.
        let kf8_skeleton_start = kf8_fragment_start
            + kf8_section.fragment_indx.len()
            + kf8_section.cncx_records.len();
        let kf8_ncx_start = kf8_skeleton_start + kf8_section.skeleton_indx.len();
        let kf8_fdst_idx = kf8_ncx_start
            + kf8_section.ncx_indx.len()
            + kf8_section.ncx_cncx_records.len();
        // Record order must match kindlegen: FDST, FLIS, FCIS, DATP, EOF
        let kf8_flis_idx = kf8_fdst_idx + 1;
        let kf8_fcis_idx = kf8_flis_idx + 1;
        let kf8_datp_idx = kf8_fcis_idx + 1;
        // +2 skips the NULL pad record (text_count+1=NULL, +2=first INDX)
        let kf8_first_nonbook = kf8_text_count + 2;

        // HD container
        let hd_container: Option<HdContainer> = if hd_images && num_image_records > 0 {
            eprintln!("Building HD image container (CONT/CRES)...");
            Some(build_hd_container(opf, &image_records, &thumb_hd_skip))
        } else {
            None
        };
        let hd_record_count = hd_container.as_ref().map_or(0, |hd| hd.total_record_count());
        let hd_geometry_string: Option<String> = hd_container.as_ref().map(|hd| hd.geometry_string());

        let total_global_records = kf7_total + 1 + 1 + kf8_text_count + 1
            + kf8_section.fragment_indx.len()
            + kf8_section.cncx_records.len()
            + kf8_section.skeleton_indx.len()
            + kf8_section.ncx_indx.len()
            + kf8_section.ncx_cncx_records.len()
            + 1 + 1 + 3  // FDST + DATP + FLIS + FCIS + EOF
            + hd_record_count;

        // Build KF7 record 0 (version=6, with EXTH 121 pointing to KF8 record 0)
        let empty_chars: HashSet<u32> = HashSet::new();
        let kf7_record0 = build_record0(
            opf,
            text_length,
            text_records.len(),
            kf7_first_non_book,
            0xFFFFFFFF_usize, // orth_index (no dict in KF7 book mode)
            0xFFFFFFFF_usize, // infl_index (no dict)
            total_global_records,
            kf7_flis,
            kf7_fcis,
            no_compress,
            &empty_chars,
            false,
            kf7_first_image,
            cover_offset,
            thumb_offset,
            kf8_cover_uri.as_deref(),
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
            thumb_offset,
            kf8_cover_uri.as_deref(),
            fixed_layout.as_ref(),
            kf8_fdst_idx, // KF8 first_image = fdst_idx (matches KCC/kindlegen)
            creator_tag,
            None,  // no SRCS in KF8 section of dual format
            None,  // no HD geometry in KF8 section of dual format
            0,     // total_records not used for KF8 section in dual format
            doc_type,
        );

        // Build FLIS/FCIS/EOF for both sections
        let kf7_flis_rec = build_flis();
        let kf7_fcis_rec = build_fcis(text_length, 1); // KF7: 1 flow
        let kf8_flis_rec = build_flis();
        let kf8_fcis_rec = build_fcis(kf8_section.text_length, kf8_section.flow_count);
        let eof = build_eof();
        let boundary_rec = b"BOUNDARY".to_vec();
        let null_pad_rec = vec![0x00u8, 0x00u8]; // 2-byte NULL pad (matches KCC)

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
        all_records.extend(kf8_section.cncx_records);
        all_records.extend(kf8_section.skeleton_indx);
        all_records.extend(kf8_section.ncx_indx);
        all_records.extend(kf8_section.ncx_cncx_records);
        all_records.push(kf8_section.fdst);
        // Order must match kindlegen: FLIS, FCIS, DATP (not DATP first)
        all_records.push(kf8_flis_rec);
        all_records.push(kf8_fcis_rec);
        all_records.push(kf8_section.datp);
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
///
/// `hd_skip` holds LD image indices that should be represented by a
/// placeholder CRES slot even when the LD data would otherwise qualify. We
/// use this to skip the synthetic library thumbnail record appended at the
/// end of the LD image pool: the thumbnail is small, low quality, and exists
/// only for the library grid tile, so there is no value in also shipping an
/// HD version of it.
fn build_hd_container(
    opf: &OPFData,
    image_records: &[Vec<u8>],
    hd_skip: &std::collections::HashSet<usize>,
) -> HdContainer {
    let title = if opf.title.is_empty() { "Book" } else { &opf.title };
    let num_images = image_records.len();

    let mut cres_records: Vec<Vec<u8>> = Vec::new();
    let mut hd_image_count: u32 = 0;
    let mut max_width: u32 = 0;
    let mut max_height: u32 = 0;
    let mut kindle_embed_parts: Vec<String> = Vec::new();

    for (idx, img_data) in image_records.iter().enumerate() {
        if img_data.is_empty() || hd_skip.contains(&idx) {
            // Empty image slot or caller-requested skip - use placeholder
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

/// Downscale an image to a library-thumbnail sized JPEG.
///
/// Produces a JPEG at roughly 160x240 (Calibre's convention) so that modern
/// Kindle firmwares can render the library grid tile without decoding the
/// full-size cover. The aspect ratio of the source is preserved and the
/// image is fit inside the bounding box.
///
/// Returns None if the input cannot be decoded; the caller should fall back
/// to emitting no thumbnail rather than failing the build.
fn build_thumbnail_record(cover_bytes: &[u8]) -> Option<Vec<u8>> {
    // Target bounding box. Calibre's AZW3 output uses 160x240 for thumbnails
    // on a typical 2:3 cover, which gives about 10 KB per JPEG at q80.
    const THUMB_BOX_W: u32 = 330;
    const THUMB_BOX_H: u32 = 470;
    const THUMB_QUALITY: u8 = 80;

    let img = image::load_from_memory(cover_bytes).ok()?;
    let thumb = img.thumbnail(THUMB_BOX_W, THUMB_BOX_H);

    let mut buf: Vec<u8> = Vec::with_capacity(16 * 1024);
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(cursor, THUMB_QUALITY);
        thumb.write_with_encoder(encoder).ok()?;
    }
    Some(buf)
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

/// Build text content for a dictionary, splitting at entry boundaries to stay
/// under Amazon's per-HTML-file size limit.
///
/// Entries are kept in file order (matching what `find_entry_positions` expects)
/// and split into sections separated by `<mbp:pagebreak/>` tags whenever the
/// accumulated section size would exceed 30 MB.
///
/// Non-dictionary spine items (cover, usage guide, copyright, etc.) are included
/// before the dictionary entries so that front matter is preserved. The
/// `<mbp:frameset>` wrapper from the source dictionary HTML is also preserved,
/// as it is required for Kindle dictionary rendering.
fn build_text_content_by_letter(opf: &OPFData, entries: &[DictionaryEntry]) -> Vec<u8> {
    // Collect non-dictionary spine items (front matter) and extract styles
    let mut front_matter_sections: Vec<String> = Vec::new();
    let mut first_style: Option<String> = None;
    let mut has_frameset = false;
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
                if content.contains("<mbp:frameset") {
                    has_frameset = true;
                }
                continue; // Skip dictionary files, they'll be handled by entry chunking
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

    // Build dictionary sections, splitting at entry boundaries to stay under 30MB.
    // Entries stay in file order so find_entry_positions can locate them sequentially.
    let mut dict_sections: Vec<String> = Vec::new();
    let mut current_chunk = String::new();

    for entry in entries {
        let stripped = strip_idx_markup(&entry.html_content);
        if !current_chunk.is_empty() && current_chunk.len() + stripped.len() > KINDLE_HTML_SIZE_LIMIT {
            dict_sections.push(current_chunk);
            current_chunk = String::new();
        }
        current_chunk.push_str(&stripped);
    }
    if !current_chunk.is_empty() {
        dict_sections.push(current_chunk);
    }

    eprintln!("Kindle limits: split {} entries into {} sections", entries.len(), dict_sections.len());

    // Check total section count
    let total_sections = front_matter_sections.len() + dict_sections.len();
    if total_sections > KINDLE_HTML_FILE_LIMIT {
        eprintln!(
            "Warning: {} total HTML sections exceeds the Kindle limit of {} files",
            total_sections, KINDLE_HTML_FILE_LIMIT
        );
    }

    // Join front matter with pagebreaks
    let fm_body = front_matter_sections.join("<mbp:pagebreak/>");

    // Join dictionary sections with pagebreaks, wrapped in <mbp:frameset> if the
    // source dictionary HTML used one (required for Kindle dictionary rendering)
    let dict_body = dict_sections.join("<mbp:pagebreak/>");
    let dict_body = if has_frameset {
        format!("<mbp:frameset>{}</mbp:frameset>", dict_body)
    } else {
        dict_body
    };

    // Combine front matter and dictionary
    let merged_body = if fm_body.is_empty() {
        dict_body
    } else {
        format!("{}<mbp:pagebreak/>{}", fm_body, dict_body)
    };

    let style_block = first_style.unwrap_or_default();
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

    // Remove idx:orth tags but keep inner content (v0.5.0/lemma v1.0.0
    // behaviour; h5 wrapping was breaking on-device popup routing).
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
/// Minimal cleanup: drops any prefixed `xmlns:foo="..."` attributes
/// (epub:/opf:/dc: etc) that EPUB spine files pick up from their
/// authoring toolchain. The default `xmlns="http://www.w3.org/1999/xhtml"`
/// and the `<?xml version="1.0" ?>` declaration are BOTH preserved —
/// kindlegen's KF8 rawml keeps them and real Kindle hardware rejects
/// content without the XHTML namespace on `<html>` ("Unable to Open
/// Item", the v0.2.0..v0.10.0 Vader Down failure mode).
fn clean_book_html(html: &str) -> String {
    let mut result = html.to_string();

    // Remove prefixed xmlns:* attributes (epub: / opf: / dc: / etc).
    let xmlns = Regex::new(r#"\s+xmlns:\w+="[^"]*""#).unwrap();
    result = xmlns.replace_all(&result, "").to_string();

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

/// Compute the chunk size used when splitting text into records.
///
/// The PalmDOC `record_size` field in the header is an upper bound on
/// individual record sizes. For dictionaries with more than 65000 records
/// worth of 4096-byte chunks the limit is scaled up so the record count
/// fits in 16 bits.
fn compute_chunk_size(total_length: usize) -> usize {
    let mut chunk_size = RECORD_SIZE;
    if total_length / chunk_size > 65000 {
        chunk_size = (total_length / 65000) + 1;
        chunk_size = chunk_size.next_power_of_two();
    }
    chunk_size
}

/// Count how many trailing bytes of `chunk` belong to an incomplete UTF-8
/// character, i.e. a multi-byte sequence whose leading byte is present but
/// one or more continuation bytes are missing.
///
/// Returns a value in 0..=3. A return of 0 means the chunk ends on a
/// complete UTF-8 character boundary.
fn incomplete_utf8_tail_bytes(chunk: &[u8]) -> usize {
    // Walk backward over continuation bytes (0x80..=0xBF) to find the
    // most recent non-continuation byte.
    let mut trailing = 0usize;
    while trailing < 3 && trailing < chunk.len() {
        let b = chunk[chunk.len() - 1 - trailing];
        if (0x80..=0xBF).contains(&b) {
            trailing += 1;
            continue;
        }
        // b is the lead byte (or ASCII) for the final character.
        let expected = if b < 0x80 {
            1
        } else if (0xC0..=0xDF).contains(&b) {
            2
        } else if (0xE0..=0xEF).contains(&b) {
            3
        } else if (0xF0..=0xF7).contains(&b) {
            4
        } else {
            // Stray continuation byte deeper than 3 or invalid byte.
            // Not a valid UTF-8 character but we cannot fix it here.
            return 0;
        };
        let have = trailing + 1;
        return if have < expected { have } else { 0 };
    }
    0
}

/// Pad `text` so every `chunk_size`-byte slice (except the last) ends
/// exactly at a UTF-8 character boundary AND between two HTML elements
/// (after `>`, before `<`), inserting ASCII space padding to fill any
/// gap. The spaces sit in HTML inter-element whitespace, which parsers
/// collapse, so the padding has no visible rendering impact and never
/// lands inside a `<b>headword</b>` text run that `find_entry_positions`
/// later searches for.
///
/// Why this exists: Kindle firmware uses
///   record_idx = byte_offset / text_record_size
/// to route popup lookups, so PalmDOC records must be exactly
/// `text_record_size` bytes each (any drift accumulates and misroutes
/// later-in-alphabet entries by progressively larger amounts). At the
/// same time, the Kindle indexer rejects dictionaries where too many
/// records decode as invalid UTF-8 (mid-character splits). Backing off
/// to UTF-8 boundaries shrinks records below `text_record_size` and
/// breaks routing; padding to `text_record_size` between elements keeps
/// records both uniform-sized AND individually UTF-8/HTML-clean.
fn pad_text_for_chunking(text: &[u8], chunk_size: usize) -> Vec<u8> {
    let mut padded: Vec<u8> = Vec::with_capacity(text.len() + 8 * (text.len() / chunk_size + 1));
    let mut src = 0usize;
    while src < text.len() {
        let remaining_in_chunk = chunk_size - (padded.len() % chunk_size);
        let avail = text.len() - src;
        if avail <= remaining_in_chunk {
            // Last chunk: no need to pad, just copy the rest verbatim.
            padded.extend_from_slice(&text[src..]);
            break;
        }
        // Find a safe split point in text[src..src+remaining_in_chunk].
        // Preference: walk backward from the natural end looking for the
        // last `>` immediately followed by `<` (the gap between two HTML
        // elements). Padding between those bytes sits in HTML inter-element
        // whitespace, ignored by parsers.
        let chunk = &text[src..src + remaining_in_chunk];
        let mut split_at: Option<usize> = None;
        for i in (1..chunk.len()).rev() {
            if chunk[i - 1] == b'>' && chunk[i] == b'<' {
                split_at = Some(i);
                break;
            }
        }
        // Fallback: if no `><` boundary in the chunk (rare for HTML
        // dict text), back off to a UTF-8 character boundary. Records
        // produced this way may have whitespace inside an element, but
        // they remain valid UTF-8.
        let safe_n = match split_at {
            Some(n) => n,
            None => {
                let trailing = incomplete_utf8_tail_bytes(chunk);
                if trailing >= remaining_in_chunk {
                    1
                } else {
                    remaining_in_chunk - trailing
                }
            }
        };
        padded.extend_from_slice(&text[src..src + safe_n]);
        src += safe_n;
        // Pad with spaces to the chunk boundary.
        let pad_count = remaining_in_chunk - safe_n;
        for _ in 0..pad_count {
            padded.push(b' ');
        }
    }
    padded
}

/// Split `text_bytes` into chunk ranges of at most `chunk_size` bytes,
/// choosing end positions that keep each chunk well-formed enough to be
/// decoded independently by Kindle. Returns the start..end byte offsets
/// for each chunk.
///
/// Kindle readers concatenate decoded records back into a single byte
/// stream, but each record is also decoded independently for HTML
/// parsing and pagination. A chunk boundary landing inside:
///
///   - a multi-byte UTF-8 character - leaves an orphan lead byte that
///     renders as tofu;
///   - an HTML tag like `<b>` - leaves a truncated tag that corrupts
///     HTML state for the rest of the record;
///   - an HTML tag pair like `<b>...</b>` - leaves the opener in one
///     record with no closer, causing bold/italic/paragraph state to
///     leak for the rest of the record.
///
/// Split point preferences, in order:
///   1. Just after `<hr/>` - a lemma dictionary places one between
///      every entry, so aligning to it guarantees no tag pair straddles
///      a record boundary. Only used when it gives at least half the
///      chunk of forward progress, to avoid tiny chunks when the only
///      `<hr/>` in range is near the start.
///   2. Just before an unclosed `<` - so no chunk ends inside an HTML
///      tag. Tag pairs may still straddle (giving bold/italic leak),
///      but this is strictly better than leaving a truncated tag.
///   3. A UTF-8 character boundary - so no chunk ends mid-character.
fn split_on_utf8_boundaries(text_bytes: &[u8], chunk_size: usize) -> Vec<(usize, usize)> {
    // Kindle firmware uses `byte_offset / text_record_size` (4096) to
    // compute which PalmDOC record contains a given decompressed byte.
    // Any per-record backoff (UTF-8 safety, `<hr/>` alignment) makes
    // actual record sizes drift below the declared size, and the drift
    // accumulates across records, progressively misrouting popup
    // lookups the further into the alphabet the query lands. Split at
    // exactly `chunk_size`, full stop. Mid-character splits produce
    // boundary tofu in rendering but the correct entry in routing,
    // which is strictly better than the reverse.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let total = text_bytes.len();
    let mut start = 0usize;
    while start < total {
        let end = (start + chunk_size).min(total);
        ranges.push((start, end));
        start = end;
    }
    ranges
}

/// Compress text into PalmDOC records with trailing bytes.
///
/// Uses std::thread for parallel compression on large inputs (>1 MB)
/// since each chunk is independent.
fn compress_text(text_bytes: &[u8]) -> (Vec<Vec<u8>>, usize) {
    let total_length = text_bytes.len();
    let chunk_size = compute_chunk_size(total_length);

    // Split into owned chunks for thread safety. Chunks end on UTF-8
    // character boundaries, so individual record sizes may be up to 3
    // bytes smaller than chunk_size near a multi-byte character.
    let chunks: Vec<Vec<u8>> = split_on_utf8_boundaries(text_bytes, chunk_size)
        .into_iter()
        .map(|(s, e)| text_bytes[s..e].to_vec())
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
                    // Trailing bytes for extra_flags=3 (bit 0=multibyte,
                    // bit 1=TBS). Kindle / libmobi parse these FROM the
                    // end of the record backward, bit 1 FIRST then
                    // bit 0. So TBS must be the LAST byte, multibyte
                    // the byte before.
                    //   0x00 = multibyte byte (low 2 bits=0 → no
                    //          overhang; chunks end on UTF-8 boundaries)
                    //   0x81 = TBS varlen-dec encoding of size 1
                    //          (stop bit set, value 1 → just this byte)
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
                // Trailing bytes: multibyte(0x00) then TBS(0x81).
                // TBS is the LAST byte of the record (parsed backward
                // by libmobi / Kindle); multibyte is the byte before.
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
    let chunk_size = compute_chunk_size(total_length);

    let records: Vec<Vec<u8>> = split_on_utf8_boundaries(text_bytes, chunk_size)
        .into_iter()
        .map(|(s, e)| {
            let mut rec = text_bytes[s..e].to_vec();
            // Trailing bytes: multibyte(0x00) then TBS(0x81). TBS is
            // parsed from the last byte of the record backwards, so
            // TBS must be last and multibyte before it. Chunks end on
            // UTF-8 boundaries so the multibyte byte is always 0.
            rec.push(0x00);
            rec.push(0x81);
            rec
        })
        .collect();

    (records, total_length)
}

#[cfg(test)]
mod record_split_tests {
    use super::*;

    /// Strip the two trailing bytes (TBS 0x81 + multibyte 0x00) written by
    /// both record-producing functions.
    fn strip_trailers(rec: &[u8]) -> &[u8] {
        assert!(rec.len() >= 2, "record too small to contain trailers");
        // TBS is the LAST byte, multibyte is the byte before it
        // (libmobi parses from the end backwards, bit 1 = TBS first).
        let tbs_byte = rec[rec.len() - 1];
        assert_eq!(tbs_byte, 0x81, "TBS byte must be 0x81 at end of record");
        let mb_byte = rec[rec.len() - 2];
        assert_eq!(mb_byte & 0x3, 0, "multibyte byte overhang must be 0");
        &rec[..rec.len() - 2]
    }

    /// Minimal PalmDOC (LZ77 + RLE) decompressor for round-trip testing.
    fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
        let mut i = 0usize;
        while i < src.len() {
            let b = src[i];
            i += 1;
            if b == 0 {
                out.push(0);
            } else if (1..=8).contains(&b) {
                let n = b as usize;
                let end = (i + n).min(src.len());
                out.extend_from_slice(&src[i..end]);
                i = end;
            } else if (9..=0x7F).contains(&b) {
                out.push(b);
            } else if (0x80..=0xBF).contains(&b) {
                if i >= src.len() {
                    break;
                }
                let b2 = src[i];
                i += 1;
                let word = ((b as u16) << 8) | (b2 as u16);
                let distance = ((word >> 3) & 0x7FF) as usize;
                let length = ((word & 0x7) as usize) + 3;
                if distance == 0 || distance > out.len() {
                    break;
                }
                let start = out.len() - distance;
                for k in 0..length {
                    let byte = out[start + k];
                    out.push(byte);
                }
            } else {
                // 0xC0..=0xFF: space + (b ^ 0x80)
                out.push(0x20);
                out.push(b ^ 0x80);
            }
        }
        out
    }

    #[test]
    fn incomplete_utf8_tail_bytes_on_clean_ascii() {
        assert_eq!(incomplete_utf8_tail_bytes(b"hello"), 0);
    }

    #[test]
    fn incomplete_utf8_tail_bytes_on_complete_two_byte() {
        // "αβ" is two 2-byte characters, total 4 bytes.
        assert_eq!(incomplete_utf8_tail_bytes("αβ".as_bytes()), 0);
    }

    #[test]
    fn incomplete_utf8_tail_bytes_on_lead_only_two_byte() {
        // "α" = CE B1. Drop the B1 to leave a dangling lead byte.
        let bytes = [0xCEu8];
        assert_eq!(incomplete_utf8_tail_bytes(&bytes), 1);
    }

    #[test]
    fn incomplete_utf8_tail_bytes_on_lead_only_three_byte() {
        // U+2020 DAGGER = E2 80 A0. Various partial prefixes.
        assert_eq!(incomplete_utf8_tail_bytes(&[0xE2]), 1);
        assert_eq!(incomplete_utf8_tail_bytes(&[0xE2, 0x80]), 2);
        assert_eq!(incomplete_utf8_tail_bytes(&[0xE2, 0x80, 0xA0]), 0);
    }

    #[test]
    fn incomplete_utf8_tail_bytes_on_lead_only_four_byte() {
        // "͵Ζ" ... actually "͵Ζ" is two 2-byte chars (CD B4 CE 96).
        // For a 4-byte test use U+1F600 GRINNING FACE = F0 9F 98 80.
        assert_eq!(incomplete_utf8_tail_bytes(&[0xF0]), 1);
        assert_eq!(incomplete_utf8_tail_bytes(&[0xF0, 0x9F]), 2);
        assert_eq!(incomplete_utf8_tail_bytes(&[0xF0, 0x9F, 0x98]), 3);
        assert_eq!(incomplete_utf8_tail_bytes(&[0xF0, 0x9F, 0x98, 0x80]), 0);
    }

    #[test]
    fn split_on_utf8_boundaries_fixed_chunks() {
        // Records must be exactly chunk_size bytes (except the last),
        // regardless of UTF-8 character boundaries. Kindle firmware uses
        // byte_offset / text_record_size to route popup lookups and any
        // per-record drift accumulates across records.
        let mut text = vec![b'x'; 4095];
        text.extend_from_slice("α".as_bytes()); // straddles 4095..4097
        text.extend_from_slice(&[b'y'; 100]);
        let ranges = split_on_utf8_boundaries(&text, 4096);
        assert_eq!(ranges[0], (0, 4096));
        // First chunk ends mid-α (incomplete UTF-8 in this record is
        // an accepted tradeoff; the router stays byte-exact).
    }

    #[test]
    fn split_on_utf8_boundaries_reassembles_to_original() {
        let mut text: Vec<u8> = Vec::new();
        for i in 0..5000 {
            if i % 7 == 0 {
                text.extend_from_slice("αβγ".as_bytes());
            } else if i % 11 == 0 {
                text.extend_from_slice("\u{2020}".as_bytes());
            } else {
                text.push(b'a' + (i as u8 % 26));
            }
        }
        let ranges = split_on_utf8_boundaries(&text, 4096);
        let mut reassembled = Vec::with_capacity(text.len());
        for (s, e) in &ranges {
            reassembled.extend_from_slice(&text[*s..*e]);
            assert!(e - s <= 4096);
        }
        assert_eq!(reassembled, text);
    }

    /// Count matching `<tag>` and `</tag>` pairs in a byte slice. Used
    /// by HTML-safety tests to assert each record is balanced.
    fn count_tag_balance(bytes: &[u8], tag: &str) -> i32 {
        let haystack = std::str::from_utf8(bytes).unwrap_or("");
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        let opens = haystack.matches(&open).count() as i32;
        let closes = haystack.matches(&close).count() as i32;
        opens - closes
    }

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
        // HTML-escape the headword to match the text blob, which retains entities
        // like &#x27; for apostrophes. The headword was unescaped during parsing,
        // so we need to re-escape for searching.
        let escaped_hw = entry.headword
            .replace('&', "&amp;")
            .replace('\'', "&#x27;")
            .replace('"', "&quot;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let headword_bytes = escaped_hw.as_bytes();

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

    let unfound: Vec<_> = entries.iter().zip(positions.iter())
        .filter(|(_, (s, l))| *s == 0 && *l == 0)
        .map(|(e, _)| e.headword.clone())
        .collect();
    if !unfound.is_empty() {
        eprintln!("Warning: {} / {} entries not found in text blob", unfound.len(), entries.len());
        for hw in unfound.iter().take(20) {
            eprintln!("  Not found: {:?}", hw);
        }
        if unfound.len() > 20 {
            eprintln!("  ... and {} more", unfound.len() - 20);
        }
    }

    positions
}

/// Check if a `<b>` tag position is at an entry boundary (a headword heading).
///
/// Entry headings in the stripped text are preceded by `<hr/> `, `"/> `,
/// `<h5>` (the block-level headword wrapper), or appear near the start of
/// the text (first entry in the body). Bold text inside example sentences
/// or etymologies is preceded by `>` from a `<p>` or `<i>` tag.
fn is_entry_boundary(text_bytes: &[u8], bold_pos: usize) -> bool {
    // First entry: near the start of the body
    if bold_pos < 200 {
        return true;
    }

    // Look backward from the <b> position for the preceding context.
    // Entry headings can be preceded by any of:
    //   <h5><b>  (the block-level headword wrapper inserted by strip_idx_markup)
    //   <hr/> <b>  (with space between, legacy path when no h5 wrap)
    //   /> <b>  (after <br/> or other self-closing tags at end of prev entry)
    let check_start = if bold_pos >= 8 { bold_pos - 8 } else { 0 };
    let preceding = &text_bytes[check_start..bold_pos];

    // Check for "<h5>" immediately before (block-level headword wrapper)
    if preceding.ends_with(b"<h5>") {
        return true;
    }

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
///
/// Flattens headwords and inflected forms into a single sorted list
/// where every form is a direct orth INDX entry pointing at its
/// headword's text position. Matches lemma v1.0.0 / kindling v0.5.0
/// behaviour, the last demonstrably on-device-working state.
fn build_lookup_terms(
    entries: &[DictionaryEntry],
    positions: &[(usize, usize)],
    text_bytes: &[u8],
    headwords_only: bool,
) -> Vec<LookupTerm> {
    use std::collections::HashMap;

    let mut terms: HashMap<String, (usize, usize, usize, usize)> = HashMap::new();
    let mut headwords: HashSet<String> = HashSet::new();

    for (entry_ordinal, (entry, &(start_pos, text_len))) in
        entries.iter().zip(positions.iter()).enumerate()
    {
        let hw = &entry.headword;
        let hw_bytes = hw.as_bytes();
        let mut hw_display_len = 3 + hw_bytes.len() + 4 + 1;

        if start_pos > 0 && start_pos + hw_display_len <= text_bytes.len() {
            let mut expected = Vec::new();
            expected.extend_from_slice(b"<b>");
            expected.extend_from_slice(hw_bytes);
            expected.extend_from_slice(b"</b> ");
            let actual = &text_bytes[start_pos..start_pos + hw_display_len];
            if actual != expected.as_slice() {
                hw_display_len = 3 + hw_bytes.len() + 4;
            }
        }

        terms.insert(
            hw.clone(),
            (start_pos, text_len, hw_display_len, entry_ordinal),
        );
        headwords.insert(hw.clone());
    }

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

    eprintln!("Encoding {} unique lookup terms...", terms.len());
    let mut label_bytes_map: HashMap<String, Vec<u8>> = HashMap::new();
    for label in terms.keys() {
        label_bytes_map.insert(label.clone(), indx::encode_indx_label(label));
    }

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
#[allow(clippy::too_many_arguments)]
fn build_record0(
    opf: &OPFData,
    text_length: usize,
    text_record_count: usize,
    first_non_book_record: usize,
    orth_index_record: usize,
    infl_index_record: usize,
    total_records: usize,
    flis_record: usize,
    fcis_record: usize,
    no_compress: bool,
    headword_chars: &HashSet<u32>,
    is_dictionary: bool,
    first_image_record: usize,
    cover_offset: Option<u32>,
    thumb_offset: Option<u32>,
    kf8_cover_uri: Option<&str>,
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
    put32(&mut mobi, 28, infl_index_record as u32); // inflection index
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
    // KCC uses 0x850 for KF7 books/comics. 0x4850 was a Kindle Previewer
    // artifact that may cause rejection on real Kindle hardware.
    if is_dictionary {
        put32(&mut mobi, 112, 0x50);
    } else {
        put32(&mut mobi, 112, 0x850);
    }

    put32(&mut mobi, 148, 0xFFFFFFFF); // DRM flags
    put32(&mut mobi, 152, 0xFFFFFFFF);

    // FDST flow count composite (KF7 only): high word = 1 (flow count),
    // low word = index of last content record before FLIS/FCIS.
    // KCC/kindlegen uses (flis_record - 1), NOT total_records.
    put32(
        &mut mobi,
        176,
        (1u32 << 16) | ((flis_record - 1) as u32),
    );
    put32(&mut mobi, 180, 1);

    // FLIS/FCIS pointers (kindlegen puts FCIS at 184, FLIS at 192)
    put32(&mut mobi, 184, fcis_record as u32);
    put32(&mut mobi, 188, 1);
    put32(&mut mobi, 192, flis_record as u32);
    put32(&mut mobi, 196, 1);

    // Extra record data flags (multibyte + TBS)
    put32(&mut mobi, 224, 3);

    // NCX and other indices: 0xFFFFFFFF (matches lemma v1.0.0 output)
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
            cover_offset,
        )
    } else {
        exth::build_book_exth(
            full_name,
            &opf.author,
            &opf.date,
            &opf.language,
            cover_offset,
            thumb_offset,
            kf8_cover_uri,
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

    // Pad to 4-byte boundary, then pad to minimum size matching
    // KCC/kindlegen output (~8K). DualMetaFix-style tools need
    // padding space to insert EXTH records without changing record size.
    while record0.len() % 4 != 0 {
        record0.push(0x00);
    }
    const MIN_RECORD0_SIZE: usize = 8892;
    if record0.len() < MIN_RECORD0_SIZE {
        record0.resize(MIN_RECORD0_SIZE, 0x00);
    }

    record0
}

/// Build KF8 Record 0: PalmDOC header + MOBI header (version=8) + EXTH + full name.
///
/// All record indices are KF8-relative (relative to this record as index 0).
/// In KF8-only mode, `srcs_record` points to the SRCS record and `hd_geometry`
/// provides the HD image geometry string. `_total_records` is no longer used
/// (the KF8 MOBI header's fdst_idx at offset 176 is now written unconditionally
/// to the FDST record index, matching the Calibre writer8 layout).
///
/// NOTE: For dual-format .mobi files, the Kindle reads the library title from
/// the KF8 Record 0's full name, not the KF7 Record 0. Both must be set but
/// the KF8 value is what appears in the Kindle library.
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
    thumb_offset: Option<u32>,
    kf8_cover_uri: Option<&str>,
    fixed_layout: Option<&exth::FixedLayoutMeta>,
    first_image_record: usize,
    creator_tag: bool,
    srcs_record: Option<usize>,
    hd_geometry: Option<&str>,
    _total_records: usize,
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
    put32(&mut mobi, 24, fragment_indx_record as u32); // orth index = fragment INDX (matches KCC)
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

    put32(&mut mobi, 112, 0x50); // exth_flags (matches KCC KF8 section)

    put32(&mut mobi, 148, 0xFFFFFFFF); // DRM flags
    put32(&mut mobi, 152, 0xFFFFFFFF);

    // KF8 MOBI header layout per Calibre writer8/header.py and verified
    // against Calibre's MetadataHeader reader (palmDOC offset 0 = raw[16]):
    //
    //   160..176 unused / 0
    //   176      fdst_idx          (u32)
    //   180      num_flows         (u32)
    //   184      fcis_record_idx   (u32)
    //   188      fcis_count        (u32, =1)
    //   192      flis_record_idx   (u32)
    //   196      flis_count        (u32, =1)
    //   200..208 unknown6          (zero)
    //   208      srcs_record_idx   (u32)
    //   212      num_srcs_records  (u32)
    //   216..224 unknown7          (zero)
    //   224      extra_data_flags  (u32, bits 0+1 = multibyte + TBS = 3)
    //   228      primary_index_record  (NCX, u32)
    //   232      sect_idx          (fragment INDX, u32)
    //   236      skel_idx          (skeleton INDX, u32)
    //   240      datp_idx          (u32)
    //   244      oth_idx           (u32, 0xFFFFFFFF)
    //
    // Do NOT write fdst to offset 160 — that's a legacy KF7 field the
    // comic-format readers don't use, and calibre reads fdst exclusively
    // from offset 176.

    // FDST record / flow count
    put32(&mut mobi, 176, fdst_record as u32);
    put32(&mut mobi, 180, fdst_flow_count as u32);

    // FCIS/FLIS (KF8-relative)
    put32(&mut mobi, 184, fcis_record as u32);
    put32(&mut mobi, 188, 1);
    put32(&mut mobi, 192, flis_record as u32);
    put32(&mut mobi, 196, 1);

    // SRCS record index / count at 208/212
    if let Some(srcs_idx) = srcs_record {
        put32(&mut mobi, 208, srcs_idx as u32);
        put32(&mut mobi, 212, 1);
    } else {
        put32(&mut mobi, 208, 0xFFFFFFFF);
        put32(&mut mobi, 212, 0);
    }
    // Unknown fields at 216/220: 0xFFFFFFFF (matches KCC/kindlegen)
    put32(&mut mobi, 216, 0xFFFFFFFF);
    put32(&mut mobi, 220, 0xFFFFFFFF);

    // Extra record data flags: multibyte + TBS (matches KCC/kindlegen).
    // TBS data is appended to each KF8 text record by append_kf8_tbs().
    put32(&mut mobi, 224, 3);

    // NCX (primary index record)
    put32(&mut mobi, 228, ncx_record as u32);

    // Fragment / Skeleton / DATP / other INDX records (KF8-relative)
    put32(&mut mobi, 232, fragment_indx_record as u32);
    put32(&mut mobi, 236, skeleton_indx_record as u32);
    put32(&mut mobi, 240, datp_record as u32);
    put32(&mut mobi, 244, 0xFFFFFFFF);
    // Offsets 248-256: match KCC (248=0xFFFFFFFF, 252=0x00000000, 256=0xFFFFFFFF)
    put32(&mut mobi, 248, 0xFFFFFFFF);
    put32(&mut mobi, 252, 0x00000000);
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
        thumb_offset,
        kf8_cover_uri,
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

    // Pad to match KCC/kindlegen Record 0 size
    while record0.len() % 4 != 0 {
        record0.push(0x00);
    }
    const MIN_RECORD0_SIZE: usize = 8892;
    if record0.len() < MIN_RECORD0_SIZE {
        record0.resize(MIN_RECORD0_SIZE, 0x00);
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
fn build_fcis(text_length: usize, flow_count: usize) -> Vec<u8> {
    // FCIS entry count should match FDST flow count.
    // KCC/kindlegen uses entry_count=2 for dual-flow (HTML+CSS) files,
    // with an extra 8-byte block per additional flow.
    let entry_count = flow_count.max(1);
    let mut fcis = Vec::with_capacity(44 + (entry_count - 1) * 8);
    fcis.extend_from_slice(b"FCIS");
    fcis.extend_from_slice(&20u32.to_be_bytes());
    fcis.extend_from_slice(&16u32.to_be_bytes());
    fcis.extend_from_slice(&(entry_count as u32).to_be_bytes());
    fcis.extend_from_slice(&0u32.to_be_bytes());
    fcis.extend_from_slice(&(text_length as u32).to_be_bytes());
    fcis.extend_from_slice(&0u32.to_be_bytes());
    // Per-flow block: KCC uses 0x28 for 2-flow, 0x20 for 1-flow
    let block_size: u32 = if entry_count > 1 { 0x28 } else { 0x20 };
    fcis.extend_from_slice(&block_size.to_be_bytes());
    for _ in 1..entry_count {
        fcis.extend_from_slice(&0u32.to_be_bytes());
        fcis.extend_from_slice(&block_size.to_be_bytes());
    }
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

    // Derive PalmDB name from title. The PalmDB name field is 32 bytes, and
    // we reserve 1 byte for a null terminator, leaving 31 bytes for the name.
    //
    // Filesystem-unsafe characters (`:`, `/`, `\`, `*`, `?`, `"`, `<`, `>`,
    // `|`) are stripped because Kindle's FSCK indexer treats the PalmDB name
    // as a filename candidate and will either rename or refuse to index files
    // with these characters in their PalmDB name. Brackets and parentheses
    // are stripped because they were historically problematic. Control
    // characters and whitespace become underscores.
    //
    // If the cleaned name fits in 31 bytes we use it verbatim. Otherwise we
    // truncate the prefix to 28 bytes (at a UTF-8 character boundary) and
    // append "..." for a total of 31 bytes.
    let strip_chars: &[char] = &[
        '(', ')', '[', ']',
        ':', '/', '\\', '*', '?', '"', '<', '>', '|',
    ];
    let mut palmdb_name = title.to_string();
    for ch in strip_chars {
        palmdb_name = palmdb_name.replace(*ch, "");
    }
    // Collapse whitespace runs to a single underscore. This avoids things
    // like "Star_Wars__Vader" (double underscore from ": ").
    palmdb_name = palmdb_name
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    if palmdb_name.len() > 31 {
        // Truncate to the largest char boundary <= 28 bytes, then append "...".
        let mut cutoff = 28.min(palmdb_name.len());
        while cutoff > 0 && !palmdb_name.is_char_boundary(cutoff) {
            cutoff -= 1;
        }
        palmdb_name.truncate(cutoff);
        palmdb_name.push_str("...");
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
/// Map a language tag to a Windows LCID (locale identifier).
/// KCC/kindlegen uses full LCIDs (e.g., 0x0409 for en-US), not just the
/// primary language ID (0x09). Using primary IDs causes Kindle to reject files.
fn locale_code(lang: &str) -> u32 {
    match lang {
        "en" | "en-US" => 0x0409,
        "en-GB" => 0x0809,
        "el" | "el-GR" => 0x0408,
        "de" | "de-DE" => 0x0407,
        "fr" | "fr-FR" => 0x040C,
        "es" | "es-ES" => 0x0C0A,
        "it" | "it-IT" => 0x0410,
        "pt" | "pt-BR" => 0x0416,
        "pt-PT" => 0x0816,
        "nl" | "nl-NL" => 0x0413,
        "ru" | "ru-RU" => 0x0419,
        "ja" | "ja-JP" => 0x0411,
        "zh" | "zh-CN" => 0x0804,
        "zh-TW" => 0x0404,
        "ko" | "ko-KR" => 0x0412,
        "ar" | "ar-SA" => 0x0401,
        "he" | "he-IL" => 0x040D,
        "tr" | "tr-TR" => 0x041F,
        _ => 0x0409, // default to en-US
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

