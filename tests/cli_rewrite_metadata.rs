//! End-to-end CLI test for `kindling rewrite-metadata`.
//!
//! Builds a synthetic MOBI file with known EXTH records inside the test,
//! runs the compiled `kindling-cli` binary against it with various flag
//! combinations, and asserts exit codes, the JSON report shape, and that
//! the rewritten file's EXTH records match the requested changes.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output};

fn kindling_bin() -> &'static str {
    env!("CARGO_BIN_EXE_kindling-cli")
}

fn tmp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "kindling_rewrite_metadata_cli_{}_{}",
        std::process::id(),
        name
    ));
    p
}

fn dump(out: &Output) -> String {
    format!(
        "exit={:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

fn put_u32_be(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u16_be(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// Encode a single EXTH record: type(u32 BE) + length(u32 BE) + data.
fn exth_record(rtype: u32, data: &[u8]) -> Vec<u8> {
    let mut rec = Vec::with_capacity(8 + data.len());
    rec.extend_from_slice(&rtype.to_be_bytes());
    rec.extend_from_slice(&((8 + data.len()) as u32).to_be_bytes());
    rec.extend_from_slice(data);
    rec
}

/// Serialize an EXTH block (header + records + 4-byte-alignment padding).
fn serialize_exth_block(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let record_bytes: Vec<Vec<u8>> = records
        .iter()
        .map(|(t, d)| exth_record(*t, d))
        .collect();
    let record_total: usize = record_bytes.iter().map(|r| r.len()).sum();
    let unpadded_len = 12 + record_total;
    let padding = (4 - (unpadded_len % 4)) % 4;
    let padded_len = unpadded_len + padding;

    let mut out = Vec::with_capacity(padded_len);
    out.extend_from_slice(b"EXTH");
    out.extend_from_slice(&(padded_len as u32).to_be_bytes());
    out.extend_from_slice(&(records.len() as u32).to_be_bytes());
    for rec in &record_bytes {
        out.extend_from_slice(rec);
    }
    for _ in 0..padding {
        out.push(0);
    }
    out
}

/// Build a minimal synthetic MOBI with the given title and EXTH records.
/// The file has three PalmDB records: record 0 (PalmDOC + MOBI header +
/// EXTH + full_name), a dummy text record, and a dummy image record used
/// as the cover target.
fn build_synthetic_mobi(title: &str, exth_records: &[(u32, Vec<u8>)]) -> Vec<u8> {
    const MOBI_HEADER_LENGTH: usize = 264;
    let full_name = title.as_bytes();

    let mut mobi_header = vec![0u8; MOBI_HEADER_LENGTH];
    mobi_header[0..4].copy_from_slice(b"MOBI");
    mobi_header[4..8].copy_from_slice(&(MOBI_HEADER_LENGTH as u32).to_be_bytes());
    mobi_header[8..12].copy_from_slice(&2u32.to_be_bytes()); // type = 2 (book)
    mobi_header[12..16].copy_from_slice(&65001u32.to_be_bytes()); // UTF-8
    mobi_header[20..24].copy_from_slice(&6u32.to_be_bytes()); // file version
    mobi_header[112..116].copy_from_slice(&0x40u32.to_be_bytes()); // EXTH flag
    mobi_header[92..96].copy_from_slice(&2u32.to_be_bytes()); // first_image_record

    let exth_block = serialize_exth_block(exth_records);

    let mut record0 = Vec::new();
    // PalmDOC header: compression=1, reserved, text_length=1024, text_rec_count=1,
    // record_size=4096, encryption_type=0, unknown=0.
    record0.extend_from_slice(&1u16.to_be_bytes());
    record0.extend_from_slice(&0u16.to_be_bytes());
    record0.extend_from_slice(&1024u32.to_be_bytes());
    record0.extend_from_slice(&1u16.to_be_bytes());
    record0.extend_from_slice(&4096u16.to_be_bytes());
    record0.extend_from_slice(&0u16.to_be_bytes());
    record0.extend_from_slice(&0u16.to_be_bytes());
    record0.extend_from_slice(&mobi_header);
    record0.extend_from_slice(&exth_block);
    let full_name_offset = record0.len();
    record0.extend_from_slice(full_name);
    while record0.len() % 4 != 0 {
        record0.push(0);
    }
    // full_name_offset / full_name_length live at MOBI header +68/+72
    // which is record0 +84/+88.
    put_u32_be(&mut record0, 84, full_name_offset as u32);
    put_u32_be(&mut record0, 88, full_name.len() as u32);

    let dummy_text = vec![0u8; 128];
    // JPEG-magic-prefixed "cover" record.
    let mut cover = vec![0xFFu8, 0xD8, 0xFF, 0xE0];
    cover.extend(std::iter::repeat(0x11).take(256));
    cover.extend_from_slice(&[0xFF, 0xD9]);

    let records: Vec<Vec<u8>> = vec![record0, dummy_text, cover];

    let num_records = records.len();
    let record_info_len = num_records * 8;
    let gap_len = 2;
    let mut offsets: Vec<u32> = Vec::with_capacity(num_records);
    let mut cursor = 78 + record_info_len + gap_len;
    for rec in &records {
        offsets.push(cursor as u32);
        cursor += rec.len();
    }

    let mut out = Vec::with_capacity(cursor);
    // PalmDB header (78 bytes).
    let mut name = [0u8; 32];
    let tn = b"TestBook";
    name[..tn.len()].copy_from_slice(tn);
    out.extend_from_slice(&name);
    out.extend_from_slice(&[0u8; 12]); // attrs, ver, dates
    out.extend_from_slice(&[0u8; 12]); // backup, modnum, appinfo
    out.extend_from_slice(&[0u8; 4]); // sort info
    out.extend_from_slice(b"BOOK");
    out.extend_from_slice(b"MOBI");
    out.extend_from_slice(&[0u8; 4]); // uid seed
    out.extend_from_slice(&[0u8; 4]); // next record list
    out.extend_from_slice(&(num_records as u16).to_be_bytes());
    assert_eq!(out.len(), 78);

    for (i, off) in offsets.iter().enumerate() {
        out.extend_from_slice(&off.to_be_bytes());
        out.push(0); // attributes
        out.extend_from_slice(&[0u8, 0, i as u8]); // 3-byte unique id
    }
    out.extend_from_slice(&[0u8, 0]); // 2-byte gap

    for rec in &records {
        out.extend_from_slice(rec);
    }
    out
}

/// Find the EXTH block inside a MOBI file and return parsed (type, data)
/// records. Used to verify rewrite output.
fn parse_exth_records(data: &[u8]) -> Vec<(u32, Vec<u8>)> {
    // PalmDB record 0 offset.
    let record0_start = read_u32_be(data, 78) as usize;
    let record0_end = if read_u16_be(data, 76) > 1 {
        read_u32_be(data, 78 + 8) as usize
    } else {
        data.len()
    };
    let record0 = &data[record0_start..record0_end];
    // MOBI header length at record0 offset 20.
    let mobi_header_length = read_u32_be(record0, 20) as usize;
    // EXTH starts at 16 + mobi_header_length.
    let exth_start = 16 + mobi_header_length;
    assert_eq!(&record0[exth_start..exth_start + 4], b"EXTH", "expected EXTH magic");
    let padded_len = read_u32_be(record0, exth_start + 4) as usize;
    let count = read_u32_be(record0, exth_start + 8) as usize;

    let mut records = Vec::with_capacity(count);
    let mut pos = exth_start + 12;
    let end = exth_start + padded_len;
    for _ in 0..count {
        let rtype = read_u32_be(record0, pos);
        let rlen = read_u32_be(record0, pos + 4) as usize;
        assert!(rlen >= 8);
        assert!(pos + rlen <= end);
        let payload = record0[pos + 8..pos + rlen].to_vec();
        records.push((rtype, payload));
        pos += rlen;
    }
    records
}

fn default_exth() -> Vec<(u32, Vec<u8>)> {
    vec![
        (100, b"Jane Doe".to_vec()),                    // author
        (503, b"Original Title".to_vec()),              // updated title
        (524, b"en".to_vec()),                          // language
        (103, b"An original description.".to_vec()),    // description
        (201, 0u32.to_be_bytes().to_vec()),             // cover offset
    ]
}

fn write_synthetic(name: &str, title: &str, exth: &[(u32, Vec<u8>)]) -> PathBuf {
    let bytes = build_synthetic_mobi(title, exth);
    let p = tmp_path(name);
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(&bytes).unwrap();
    p
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn cli_rewrite_title_updates_exth_503() {
    let input = write_synthetic("title_in.mobi", "Original Title", &default_exth());
    let output = tmp_path("title_out.mobi");
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--title",
            "Brand New Title",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    let out_bytes = std::fs::read(&output).unwrap();
    let records = parse_exth_records(&out_bytes);
    assert_eq!(
        records
            .iter()
            .find(|(t, _)| *t == 503)
            .map(|(_, d)| d.as_slice()),
        Some(b"Brand New Title".as_slice())
    );
}

#[test]
fn cli_rewrite_multiple_authors() {
    let input = write_synthetic("multi_author_in.mobi", "T", &default_exth());
    let output = tmp_path("multi_author_out.mobi");
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--author",
            "Alice",
            "--author",
            "Bob",
            "--author",
            "Carol",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    let records = parse_exth_records(&std::fs::read(&output).unwrap());
    let authors: Vec<&Vec<u8>> = records
        .iter()
        .filter(|(t, _)| *t == 100)
        .map(|(_, d)| d)
        .collect();
    assert_eq!(authors.len(), 3);
    assert_eq!(authors[0], b"Alice");
    assert_eq!(authors[1], b"Bob");
    assert_eq!(authors[2], b"Carol");
}

#[test]
fn cli_report_json_emits_structured_output_on_stdout() {
    let input = write_synthetic("json_in.mobi", "Original Title", &default_exth());
    let output = tmp_path("json_out.mobi");
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--language",
            "fr",
            "--report-json",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Must be a JSON object with the expected top-level keys.
    assert!(stdout.trim().starts_with('{'));
    assert!(stdout.contains("\"input_path\""));
    assert!(stdout.contains("\"output_path\""));
    assert!(stdout.contains("\"no_op\":false"));
    assert!(stdout.contains("\"changes\":["));
    assert!(stdout.contains("\"exth_type\":524"));
}

#[test]
fn cli_dry_run_does_not_write_output() {
    let input = write_synthetic("dry_in.mobi", "Original Title", &default_exth());
    let output = tmp_path("dry_out.mobi");
    assert!(!output.exists());
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--title",
            "Something Totally Different",
            "--dry-run",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    assert!(!output.exists(), "dry-run must not write output file");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("(dry-run)"), "stderr should flag dry-run: {}", stderr);
}

#[test]
fn cli_no_changes_needed_is_noop() {
    let input = write_synthetic("noop_in.mobi", "Original Title", &default_exth());
    let output = tmp_path("noop_out.mobi");
    // Pass the same title and language that are already in the file.
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--title",
            "Original Title",
            "--language",
            "en",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("No metadata changes needed"), "stderr: {}", stderr);
    // Output file must be byte-identical to input.
    let in_bytes = std::fs::read(&input).unwrap();
    let out_bytes = std::fs::read(&output).unwrap();
    assert_eq!(in_bytes, out_bytes, "byte-stable no-op must copy input verbatim");
}

#[test]
fn cli_cover_replacement_via_file_path() {
    let input = write_synthetic("cover_in.mobi", "T", &default_exth());
    let output = tmp_path("cover_out.mobi");
    // Write a fake JPEG to a temp path.
    let mut cover_bytes = vec![0xFFu8, 0xD8, 0xFF, 0xE0];
    cover_bytes.extend(std::iter::repeat(0xAA).take(512));
    cover_bytes.extend_from_slice(&[0xFF, 0xD9]);
    let cover_path = tmp_path("cover.jpg");
    std::fs::write(&cover_path, &cover_bytes).unwrap();

    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--cover",
            cover_path.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(out.status.success(), "{}", dump(&out));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Replaced cover image record"), "stderr: {}", stderr);
    // Verify: cover record in output file contains our new bytes.
    let out_bytes = std::fs::read(&output).unwrap();
    // Record 2 is the cover in our synthetic layout. Read its offset and length.
    let off2 = read_u32_be(&out_bytes, 78 + 2 * 8) as usize;
    let end2 = out_bytes.len();
    assert_eq!(&out_bytes[off2..end2], &cover_bytes[..]);
}

#[test]
fn cli_drm_rejection_exits_nonzero() {
    // Build a synthetic MOBI with a DRM EXTH record (401) present.
    let mut exth = default_exth();
    exth.push((401, vec![0, 0, 0, 1]));
    let input = write_synthetic("drm_in.mobi", "T", &exth);
    let output = tmp_path("drm_out.mobi");
    let out = Command::new(kindling_bin())
        .args([
            "rewrite-metadata",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--title",
            "Pwned",
        ])
        .output()
        .expect("failed to run kindling-cli");
    assert!(!out.status.success(), "expected DRM rejection: {}", dump(&out));
    assert!(!output.exists(), "DRM-rejected files must not produce output");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("drm"), "stderr should mention DRM: {}", stderr);
}
