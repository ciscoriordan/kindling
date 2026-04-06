/// MOBI dictionary file writer.
///
/// Builds a valid MOBI dictionary file from OPF source files, including:
/// - PalmDB header
/// - PalmDOC header + MOBI header + EXTH header (record 0)
/// - Compressed text content records
/// - INDX records with dictionary index
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

/// Build a MOBI dictionary file from an OPF source.
pub fn build_mobi(
    opf_path: &Path,
    output_path: &Path,
    no_compress: bool,
    headwords_only: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let opf = OPFData::parse(opf_path)?;

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
    let text_content = build_text_content(&opf);

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

    // Calculate record indices
    let first_non_book = text_records.len() + 1;
    let orth_index_record = text_records.len() + 1;
    let flis_record = text_records.len() + 1 + indx_records.len();
    let fcis_record = flis_record + 1;
    let total_records = 1 + text_records.len() + indx_records.len() + 3;

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
    );

    // Assemble all records
    let mut all_records = vec![record0];
    all_records.extend(text_records);
    all_records.extend(indx_records);
    all_records.push(flis);
    all_records.push(fcis);
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

/// Read and concatenate all spine HTML files into a single text blob.
fn build_text_content(opf: &OPFData) -> Vec<u8> {
    let mut parts: Vec<String> = Vec::new();

    for html_path in opf.get_content_html_paths() {
        let content = std::fs::read_to_string(&html_path).unwrap_or_default();
        let stripped = strip_idx_markup(&content);
        parts.push(stripped);
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

/// Strip idx: namespace tags from HTML, keeping only display content.
fn strip_idx_markup(html: &str) -> String {
    let mut result = html.to_string();

    // Remove XML declarations
    let xml_decl = Regex::new(r"<\?xml[^?]*\?>\s*").unwrap();
    result = xml_decl.replace_all(&result, "").to_string();

    // Remove xmlns:* attributes
    let xmlns = Regex::new(r#"\s+xmlns:\w+="[^"]*""#).unwrap();
    result = xmlns.replace_all(&result, "").to_string();

    // Remove <head>...</head>, replace with kindlegen style
    let head_re = Regex::new(r"(?s)<head>.*?</head>").unwrap();
    result = head_re
        .replace_all(&result, "<head><guide></guide></head>")
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

    // Remove idx:entry tags but keep inner content
    let entry_open = Regex::new(r"<idx:entry[^>]*>\s*").unwrap();
    result = entry_open.replace_all(&result, "").to_string();

    let entry_close = Regex::new(r"\s*</idx:entry>").unwrap();
    result = entry_close.replace_all(&result, "").to_string();

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
fn find_entry_positions(text_bytes: &[u8], entries: &[DictionaryEntry]) -> Vec<(usize, usize)> {
    let mut positions = Vec::with_capacity(entries.len());
    let mut search_start: usize = 0;

    for entry in entries {
        let headword_bytes = entry.headword.as_bytes();

        let pos = match find_bytes_from(text_bytes, headword_bytes, search_start) {
            Some(p) => p,
            None => {
                positions.push((0, 0));
                continue;
            }
        };

        // Find the start of this entry's display block (look backward for <b>)
        let search_from = if pos >= 10 { pos - 10 } else { 0 };
        let block_start = match rfind_bytes(&text_bytes[search_from..pos], b"<b>") {
            Some(rel) => search_from + rel,
            None => pos,
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
) -> Vec<u8> {
    let full_name = if opf.title.is_empty() {
        "Dictionary"
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
    put32(&mut mobi, 8, 2); // mobi type = 2
    put32(&mut mobi, 12, 65001); // UTF-8

    // Unique ID from title hash
    let uid_hash = md5_simple(full_name.as_bytes());
    let unique_id = u32::from_be_bytes([uid_hash[0], uid_hash[1], uid_hash[2], uid_hash[3]]);
    put32(&mut mobi, 16, unique_id);

    put32(&mut mobi, 20, 7); // file version = 7
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
    put32(&mut mobi, 88, 7); // min version = 7
    put32(&mut mobi, 92, 0xFFFFFFFF); // first image record
    put32(&mut mobi, 96, 0); // huffman record
    put32(&mut mobi, 100, 0); // huffman count

    put32(&mut mobi, 112, 0x50); // EXTH flags

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
    put32(&mut mobi, 244, 0xFFFFFFFF);
    put32(&mut mobi, 248, 0xFFFFFFFF);
    put32(&mut mobi, 256, 0xFFFFFFFF);

    // Build EXTH header
    let exth_data = exth::build_exth(
        full_name,
        &opf.author,
        &opf.date,
        &opf.language,
        &opf.dict_in_language,
        &opf.dict_out_language,
        headword_chars,
    );

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
