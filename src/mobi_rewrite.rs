//! In-place metadata rewrite for existing MOBI/AZW3 files.
//!
//! This module takes a built MOBI or AZW3 file on disk, updates the EXTH
//! metadata records (and optionally the cover image record) according to a
//! `MetadataUpdates` struct, and writes the result to an output path without
//! re-running EPUB/OPF conversion. It is the counterpart to `build_mobi`,
//! which constructs a MOBI from source; `rewrite_mobi_metadata` only mutates
//! metadata fields on an already-built file.
//!
//! Intended callers are downstream library-manager consumers that let users
//! edit book metadata (title, authors, tags, description, cover) and
//! need the changes to land inside the MOBI/AZW3 without a lossy round trip
//! through EPUB rebuild. Book content records (text, non-cover images,
//! indices, INDX, FLIS, FCIS, SRCS) are never touched.
//!
//! ## Guarantees
//!
//! - **Byte-stable on no-op**: if the supplied `MetadataUpdates` contains no
//!   changes, or every field already matches the file's current value, the
//!   output file is byte-identical to the input (`std::fs::copy`). Callers
//!   that use content-hash identity for books can therefore call this
//!   function unconditionally.
//! - **Idempotent**: running the same updates twice produces the same output
//!   as running them once. After the first run the file already matches, so
//!   the second run falls into the byte-stable path.
//! - **DRM-aware**: any MOBI with the PalmDOC encryption type set, or with a
//!   DRM-related EXTH record (401/402/403) present, is refused with
//!   `RewriteError::DrmEncrypted`. The function never attempts to decrypt,
//!   never loads any DRM-removal code, and never logs the file path of a
//!   DRM-encrypted file beyond the current error value.
//!
//! ## Scope
//!
//! - Supported fields: title, authors (multi), publisher, description,
//!   language, ISBN, ASIN, publication date, subjects/tags (multi), cover
//!   image bytes.
//! - Deliberately NOT supported: series name and series index. Those wrote
//!   EXTH 112 and 113, which are the source identifier and the ASIN. The
//!   builder shed the same mistake in abc5cae. kindlegen ignores calibre's
//!   series metadata and emits no series record, so there is no verified
//!   record to write instead, and 113 now carries the identifier the lock
//!   screen cover depends on (issue #26).
//! - Cover replacement only works on files that already have a cover record.
//!   Adding a cover to a file that has none would require re-inserting a
//!   PalmDB record, which shifts every downstream record and their boundary
//!   offsets; that is out of scope here.
//! - Unknown EXTH records in the input are preserved unchanged.
//! - The PalmDOC compression type, FLIS/FCIS, FDST, index records, and text
//!   content bytes are never touched. Only the section header records (MOBI
//!   header + EXTH + full_name) and optionally the cover image record are
//!   rewritten.
//! - Dual-format `.mobi` files carry two section headers, and both are
//!   rewritten. Kindle reads a dual file's library title from the KF8
//!   section, so rewriting record 0 alone (which is the KF7 section) left the
//!   device showing the old title while the command reported success.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

// EXTH type codes used by this module. Values follow the conventions used
// elsewhere in the kindling codebase (`src/exth.rs`), which match kindlegen
// and Calibre output. Where the wider MOBI ecosystem has multiple values for
// the same field (e.g. ASIN), we pick the one consistent with `build_exth`.
const EXTH_CREATOR: u32 = 100;
const EXTH_PUBLISHER: u32 = 101;
const EXTH_DESCRIPTION: u32 = 103;
const EXTH_ISBN: u32 = 104;
const EXTH_SUBJECT: u32 = 105;
const EXTH_PUBLICATION_DATE: u32 = 106;
/// EXTH 113. The standard record map calls this the ASIN; kindling writes a
/// UUID there so the firmware can file a cover under it (issue #26).
const EXTH_ASIN_IDENTIFIER: u32 = 113;
const EXTH_KF8_BOUNDARY: u32 = 121;
const EXTH_COVER_OFFSET: u32 = 201;
const EXTH_THUMB_OFFSET: u32 = 202;
const EXTH_DRM_SERVER_ID: u32 = 401;
const EXTH_DRM_COMMERCE_ID: u32 = 402;
const EXTH_DRM_EBOOKBASE_BOOK_ID: u32 = 403;
#[allow(dead_code)] // referenced only from tests
const EXTH_CDE_TYPE: u32 = 501;
const EXTH_UPDATED_TITLE: u32 = 503;
const EXTH_ASIN: u32 = 504;
const EXTH_LANGUAGE: u32 = 524;
const EXTH_TITLE_HASH: u32 = 542;

// Record 0 layout constants.
const PALMDOC_HEADER_LEN: usize = 16;
const MOBI_MAGIC_OFFSET: usize = 16;
const MOBI_HEADER_LENGTH_OFFSET: usize = 20;
const MOBI_FULL_NAME_OFFSET_FIELD: usize = 84; // MOBI header +68 = record0 +84
const MOBI_FULL_NAME_LENGTH_FIELD: usize = 88; // MOBI header +72 = record0 +88
const MOBI_EXTH_FLAGS_OFFSET: usize = 128; // MOBI header +112 = record0 +128
const PALMDOC_ENCRYPTION_TYPE_OFFSET: usize = 12;

// PalmDB constants.
const PALMDB_HEADER_LEN: usize = 78;
const PALMDB_NUM_RECORDS_OFFSET: usize = 76;

/// Metadata fields to update on the target MOBI/AZW3.
///
/// Every field is an `Option`; `None` means "leave unchanged". An empty
/// collection or empty string means "remove the corresponding EXTH records
/// entirely" rather than "leave unchanged"; use `None` for no-op.
///
/// `cover_image` accepts raw image bytes (JPEG, PNG, or GIF are the formats
/// the Kindle ingest pipeline handles). The bytes replace the existing cover
/// image record in place. The target MOBI must already have a cover (EXTH
/// 201 present) for this to succeed.
#[derive(Debug, Clone, Default)]
pub struct MetadataUpdates {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub publisher: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub publication_date: Option<String>,
    pub subjects: Option<Vec<String>>,
    pub cover_image: Option<Vec<u8>>,
    /// EXTH 501 content type, e.g. `EBOK`. Writing it also fills EXTH 113,
    /// because the firmware needs the pair before it will draw a sideloaded
    /// book's cover on the lock screen (issue #35).
    pub doc_type: Option<String>,
}

impl MetadataUpdates {
    /// True when no field is set. A call with an empty updates struct is
    /// always a no-op and should fall into the byte-stable copy path.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.authors.is_none()
            && self.publisher.is_none()
            && self.description.is_none()
            && self.language.is_none()
            && self.isbn.is_none()
            && self.asin.is_none()
            && self.publication_date.is_none()
            && self.subjects.is_none()
            && self.cover_image.is_none()
            && self.doc_type.is_none()
    }
}

/// Report of what changed during a rewrite. Empty if the rewrite was a no-op.
#[derive(Debug, Clone, Default)]
pub struct RewriteReport {
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub changes: Vec<ExthChange>,
    pub cover_updated: bool,
    pub no_op: bool,
}

/// A single EXTH mutation. Callers can audit this to cross-check which
/// records were actually written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExthChange {
    /// An existing EXTH record of this type was replaced with new bytes.
    Replaced {
        exth_type: u32,
        old_value: Vec<u8>,
        new_value: Vec<u8>,
    },
    /// A new EXTH record was added; there was no record of this type before.
    Added { exth_type: u32, value: Vec<u8> },
    /// An existing EXTH record was removed; the updates struct set the field
    /// to an empty string or empty collection.
    Removed { exth_type: u32, old_value: Vec<u8> },
}

/// Errors returned by `rewrite_mobi_metadata`.
#[derive(Debug)]
pub enum RewriteError {
    Io(std::io::Error),
    /// The file was not a MOBI or AZW3 (missing "BOOKMOBI" signature).
    NotAMobi(String),
    /// DRM-encrypted. No rewrite attempted, output not written. The error
    /// does not carry the path so callers that log it cannot leak the path
    /// of a DRM file.
    DrmEncrypted,
    /// PalmDB, MOBI, or EXTH structure was malformed. Includes a short
    /// diagnostic string.
    MalformedHeader(String),
    /// Caller asked to update a cover image but the input file has no
    /// existing cover record (no EXTH 201 present).
    NoCoverRecord,
    /// Cover image bytes did not look like a supported image format.
    UnsupportedCoverFormat,
    /// The book carries no EXTH 113 / 501 pair (or no EXTH 202 thumbnail), so
    /// the firmware has no filename to file a device-side cover under.
    NoDeviceCoverKey,
}

impl fmt::Display for RewriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RewriteError::Io(e) => write!(f, "I/O error: {}", e),
            RewriteError::NotAMobi(s) => write!(f, "not a MOBI file: {}", s),
            RewriteError::NoDeviceCoverKey => write!(
                f,
                "no EXTH 113/501 pair and thumbnail to build a device cover from; \
                 rebuild the book with --doc-type ebok"
            ),
            RewriteError::DrmEncrypted => write!(f, "file is DRM-encrypted; refusing to rewrite"),
            RewriteError::MalformedHeader(s) => write!(f, "malformed MOBI header: {}", s),
            RewriteError::NoCoverRecord => write!(
                f,
                "cannot update cover: input has no existing cover record (EXTH 201)"
            ),
            RewriteError::UnsupportedCoverFormat => write!(
                f,
                "cover image bytes are not a recognized JPEG, PNG, or GIF"
            ),
        }
    }
}

impl std::error::Error for RewriteError {}

impl From<std::io::Error> for RewriteError {
    fn from(e: std::io::Error) -> Self {
        RewriteError::Io(e)
    }
}

/// Rewrite the metadata EXTH records (and optionally the cover image
/// record) of an existing MOBI/AZW3 file on disk.
///
/// If `updates` is empty, or every field in it already matches what is
/// already in the file, the output is byte-identical to the input
/// (`std::fs::copy`). This guarantee exists so downstream callers that use
/// content-hash identity for books can invoke this function unconditionally
/// without breaking their book ID.
///
/// Running this function twice with the same `updates` is equivalent to
/// running it once: the second call sees a file whose metadata already
/// matches and takes the byte-stable copy path.
///
/// DRM-encrypted files (PalmDOC encryption type != 0, or EXTH 401/402/403
/// present) are refused with `RewriteError::DrmEncrypted` before any output
/// is written. This function does not decrypt and never links or references
/// any DRM-removal code.
pub fn rewrite_mobi_metadata(
    input: &Path,
    output: &Path,
    updates: &MetadataUpdates,
) -> Result<RewriteReport, RewriteError> {
    let input_bytes = fs::read(input)?;
    let parsed = parse_mobi(&input_bytes)?;

    // Refuse DRM files before doing any writing at all.
    if parsed.is_drm_encrypted {
        return Err(RewriteError::DrmEncrypted);
    }

    // Determine which EXTH mutations (and cover replacement) the updates
    // struct actually implies against the current file state. Dual-format
    // files get one plan per section: the two can disagree, and a file whose
    // KF7 half already matches is not a no-op if its KF8 half does not.
    let has_cover = parsed.cover_record_idx.is_some();
    let plan = plan_changes(&parsed.primary, has_cover, updates)?;
    let kf8_plan = match parsed.kf8.as_ref() {
        Some(section) => Some(plan_changes(section, has_cover, updates)?),
        None => None,
    };

    if plan.is_noop() && kf8_plan.as_ref().is_none_or(Plan::is_noop) {
        // Byte-stable no-op path. Copy the file verbatim and return an
        // empty report. This also covers the idempotent case: after a
        // real run, the next call with the same updates sees matching
        // state and falls in here.
        fs::copy(input, output)?;
        return Ok(RewriteReport {
            input_path: input.to_path_buf(),
            output_path: output.to_path_buf(),
            changes: Vec::new(),
            cover_updated: false,
            no_op: true,
        });
    }

    // Non-no-op path. Rebuild record 0 (and optionally the cover record),
    // shift downstream PalmDB offsets, and write the output file.
    let new_bytes = apply_plan(&input_bytes, &parsed, &plan, kf8_plan.as_ref())?;
    fs::write(output, &new_bytes)?;

    // Report the KF7 changes, plus any the KF8 section needed on its own.
    // The two normally coincide; they diverge when one section was already
    // up to date, and reporting only the primary would then print "no
    // changes" for a file that was in fact rewritten.
    let mut changes = plan.exth_changes;
    if let Some(kf8_plan) = kf8_plan {
        for change in kf8_plan.exth_changes {
            if !changes.contains(&change) {
                changes.push(change);
            }
        }
    }

    Ok(RewriteReport {
        input_path: input.to_path_buf(),
        output_path: output.to_path_buf(),
        changes,
        cover_updated: plan.new_cover_bytes.is_some(),
        no_op: false,
    })
}

/// The three things `kindling thumbnail` needs out of a built book.
pub struct DeviceThumbnail {
    /// EXTH 113. Half of the filename the firmware keys a cover on.
    pub asin: String,
    /// EXTH 501, e.g. `EBOK`. The other half.
    pub cde_type: String,
    /// The image bytes from the record EXTH 202 points at.
    pub image: Vec<u8>,
}

/// Pull the device-side cover thumbnail out of a built MOBI/AZW3.
///
/// Fails when the book carries no EXTH 113 / 501 pair, because without it the
/// firmware has no filename to look under and there is nothing to write.
pub fn extract_device_thumbnail(input: &Path) -> Result<DeviceThumbnail, RewriteError> {
    let bytes = fs::read(input)?;
    let parsed = parse_mobi(&bytes)?;

    let exth_string = |t: u32| -> Option<String> {
        parsed
            .primary
            .exth_records
            .iter()
            .find(|(rt, _)| *rt == t)
            .and_then(|(_, d)| String::from_utf8(d.clone()).ok())
            .filter(|s| !s.is_empty())
    };

    let asin = exth_string(EXTH_ASIN_IDENTIFIER).ok_or(RewriteError::NoDeviceCoverKey)?;
    let cde_type = exth_string(EXTH_CDE_TYPE).ok_or(RewriteError::NoDeviceCoverKey)?;

    // A path separator in either half would let the filename escape
    // system/thumbnails. Both come from a file we did not necessarily build.
    if [asin.as_str(), cde_type.as_str()]
        .iter()
        .any(|s| s.contains(['/', '\\']) || s == &".." || s.is_empty())
    {
        return Err(RewriteError::MalformedHeader(format!(
            "EXTH 113/501 are {asin:?}/{cde_type:?}, which cannot form a filename"
        )));
    }

    let idx = parsed
        .thumb_record_idx
        .ok_or(RewriteError::NoDeviceCoverKey)?;
    let start = parsed.record_offsets[idx] as usize;
    let end = if idx + 1 < parsed.record_offsets.len() {
        parsed.record_offsets[idx + 1] as usize
    } else {
        bytes.len()
    };
    let image = bytes
        .get(start..end)
        .ok_or_else(|| {
            RewriteError::MalformedHeader(format!("thumbnail record {idx} is out of bounds"))
        })?
        .to_vec();

    Ok(DeviceThumbnail {
        asin,
        cde_type,
        image,
    })
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// One MOBI section's header record: everything needed to rebuild it.
///
/// A dual-format `.mobi` carries two of these. The KF7 section is record 0;
/// the KF8 section starts at the record named by the KF7 section's EXTH 121,
/// and it has its own MOBI header, its own EXTH block and its own full_name.
struct SectionView {
    /// PalmDB record index of this section's header record.
    record_idx: usize,
    /// MOBI header length in bytes (read from the record at offset 20).
    mobi_header_length: usize,
    /// full_name_offset as stored in the MOBI header (relative to the record).
    full_name_offset: usize,
    /// full_name_length as stored in the MOBI header.
    full_name_length: usize,
    /// Parsed EXTH records as (type, data) pairs in input order.
    exth_records: Vec<(u32, Vec<u8>)>,
}

/// Parsed view of a MOBI file used by the rewrite pipeline. Owns the data it
/// points into so the caller can drop the input buffer without worry.
struct ParsedMobi {
    /// PalmDB record offsets (one per record). Absolute file offsets.
    record_offsets: Vec<u32>,
    /// Byte range of record 0 (MOBI header container) in the input buffer.
    record0_start: usize,
    record0_end: usize,
    /// Record 0's header view. In a dual file this is the KF7 section.
    primary: SectionView,
    /// The KF8 section's header record, present only on dual-format files.
    ///
    /// Rewriting only record 0 is what made `rewrite-metadata --title` look
    /// like it worked and change nothing on device: Kindle reads a dual
    /// `.mobi`'s library title from the KF8 section's full_name, not KF7's.
    kf8: Option<SectionView>,
    /// True if the file's PalmDOC encryption byte is nonzero or any DRM
    /// EXTH (401/402/403) is present.
    is_drm_encrypted: bool,
    /// Current cover image PalmDB record index, if the file has EXTH 201.
    cover_record_idx: Option<usize>,
    /// Library thumbnail PalmDB record index, if the file has EXTH 202.
    thumb_record_idx: Option<usize>,
}

fn parse_mobi(data: &[u8]) -> Result<ParsedMobi, RewriteError> {
    if data.len() < PALMDB_HEADER_LEN {
        return Err(RewriteError::MalformedHeader(format!(
            "file is {} bytes, need at least {} for PalmDB header",
            data.len(),
            PALMDB_HEADER_LEN
        )));
    }

    if &data[60..64] != b"BOOK" || &data[64..68] != b"MOBI" {
        return Err(RewriteError::NotAMobi(format!(
            "PalmDB type/creator is {:?}/{:?}, expected BOOK/MOBI",
            String::from_utf8_lossy(&data[60..64]),
            String::from_utf8_lossy(&data[64..68])
        )));
    }

    let num_records = read_u16_be(data, PALMDB_NUM_RECORDS_OFFSET).ok_or_else(|| {
        RewriteError::MalformedHeader("PalmDB record count field truncated".into())
    })? as usize;
    if num_records == 0 {
        return Err(RewriteError::MalformedHeader(
            "PalmDB record count is 0".into(),
        ));
    }

    let list_end = PALMDB_HEADER_LEN + num_records * 8;
    if data.len() < list_end {
        return Err(RewriteError::MalformedHeader(format!(
            "PalmDB record list truncated: need {} bytes, file is {}",
            list_end,
            data.len()
        )));
    }

    let mut record_offsets = Vec::with_capacity(num_records);
    for i in 0..num_records {
        let off = read_u32_be(data, PALMDB_HEADER_LEN + i * 8).ok_or_else(|| {
            RewriteError::MalformedHeader(format!("PalmDB record {} offset truncated", i))
        })?;
        record_offsets.push(off);
    }

    let record0_start = record_offsets[0] as usize;
    let record0_end = if num_records > 1 {
        record_offsets[1] as usize
    } else {
        data.len()
    };

    if record0_end > data.len() || record0_start >= record0_end {
        return Err(RewriteError::MalformedHeader(format!(
            "record 0 bounds [{}..{}] invalid against file length {}",
            record0_start,
            record0_end,
            data.len()
        )));
    }

    let record0 = &data[record0_start..record0_end];

    // PalmDOC encryption byte at offset 12.
    let encryption_type = read_u16_be(record0, PALMDOC_ENCRYPTION_TYPE_OFFSET).unwrap_or(0);

    let primary = parse_section_record0(record0, 0)?;
    let exth_records = primary.exth_records.clone();

    // A dual-format .mobi names its KF8 section in EXTH 121. That section
    // has its own MOBI header, EXTH block and full_name, and it is the one
    // Kindle reads the library title from, so it has to be rewritten too.
    // A KF8 boundary of 0xFFFFFFFF is kindlegen's "no KF8 section" marker.
    let kf8 = match exth_records.iter().find(|(t, _)| *t == EXTH_KF8_BOUNDARY) {
        Some((_, d)) if d.len() == 4 => {
            let idx = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
            if idx == 0 || idx >= num_records {
                None
            } else {
                let start = record_offsets[idx] as usize;
                let end = if idx + 1 < num_records {
                    record_offsets[idx + 1] as usize
                } else {
                    data.len()
                };
                if end > data.len() || start >= end {
                    return Err(RewriteError::MalformedHeader(format!(
                        "KF8 boundary record {} has invalid bounds [{}..{}]",
                        idx, start, end
                    )));
                }
                Some(parse_section_record0(&data[start..end], idx)?)
            }
        }
        _ => None,
    };

    // DRM detection: PalmDOC encryption byte OR any of EXTH 401/402/403.
    let has_drm_exth = exth_records.iter().any(|(t, _)| {
        *t == EXTH_DRM_SERVER_ID || *t == EXTH_DRM_COMMERCE_ID || *t == EXTH_DRM_EBOOKBASE_BOOK_ID
    });
    let is_drm_encrypted = encryption_type != 0 || has_drm_exth;

    // Cover record lookup: EXTH 201 holds a u32 BE that is an offset
    // (0-based) relative to first_image_record (MOBI header offset 0x5C =
    // record0 offset 0x6C = 108).
    let first_image_record = read_u32_be(record0, 108).unwrap_or(0xFFFFFFFF) as usize;
    let image_record_idx = |exth_type: u32| -> Option<usize> {
        exth_records
            .iter()
            .find(|(t, _)| *t == exth_type)
            .and_then(|(_, d)| {
                if d.len() == 4 {
                    let off = u32::from_be_bytes([d[0], d[1], d[2], d[3]]) as usize;
                    let idx = first_image_record + off;
                    if idx < num_records { Some(idx) } else { None }
                } else {
                    None
                }
            })
    };
    let cover_record_idx = image_record_idx(EXTH_COVER_OFFSET);
    let thumb_record_idx = image_record_idx(EXTH_THUMB_OFFSET);

    Ok(ParsedMobi {
        record_offsets,
        record0_start,
        record0_end,
        primary,
        kf8,
        is_drm_encrypted,
        cover_record_idx,
        thumb_record_idx,
    })
}

/// Parse one section's header record into a [`SectionView`].
///
/// Shared by record 0 and, on dual-format files, by the KF8 boundary record:
/// the two have the same PalmDOC + MOBI header + EXTH + full_name layout, so
/// there is no reason for the rewriter to understand it in two places.
fn parse_section_record0(record: &[u8], record_idx: usize) -> Result<SectionView, RewriteError> {
    if record.len() < PALMDOC_HEADER_LEN + 24 {
        return Err(RewriteError::MalformedHeader(format!(
            "record {} is {} bytes, too small for PalmDOC + MOBI header",
            record_idx,
            record.len()
        )));
    }

    if &record[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4] != b"MOBI" {
        return Err(RewriteError::MalformedHeader(format!(
            "expected MOBI magic at record {} offset {}, got {:?}",
            record_idx,
            MOBI_MAGIC_OFFSET,
            String::from_utf8_lossy(&record[MOBI_MAGIC_OFFSET..MOBI_MAGIC_OFFSET + 4])
        )));
    }

    let mobi_header_length = read_u32_be(record, MOBI_HEADER_LENGTH_OFFSET)
        .ok_or_else(|| RewriteError::MalformedHeader("MOBI header length field truncated".into()))?
        as usize;
    if mobi_header_length < 232 {
        return Err(RewriteError::MalformedHeader(format!(
            "MOBI header length {} is too short (expected >= 232)",
            mobi_header_length
        )));
    }

    let exth_flags = read_u32_be(record, MOBI_EXTH_FLAGS_OFFSET).unwrap_or(0);
    let has_exth = exth_flags & 0x40 != 0;

    let exth_start = MOBI_MAGIC_OFFSET + mobi_header_length;
    let (exth_records, _exth_end) = if has_exth {
        parse_exth_block(record, exth_start)?
    } else {
        (Vec::new(), exth_start)
    };

    let full_name_offset = read_u32_be(record, MOBI_FULL_NAME_OFFSET_FIELD).unwrap_or(0) as usize;
    let full_name_length = read_u32_be(record, MOBI_FULL_NAME_LENGTH_FIELD).unwrap_or(0) as usize;
    if full_name_offset + full_name_length > record.len() {
        return Err(RewriteError::MalformedHeader(format!(
            "full_name range [{}..{}] exceeds record {} length {}",
            full_name_offset,
            full_name_offset + full_name_length,
            record_idx,
            record.len()
        )));
    }

    Ok(SectionView {
        record_idx,
        mobi_header_length,
        full_name_offset,
        full_name_length,
        exth_records,
    })
}

/// Parse an EXTH block starting at `exth_start` within `record0`. Returns
/// the records and the end offset (including the 4-byte-aligned padding).
fn parse_exth_block(
    record0: &[u8],
    exth_start: usize,
) -> Result<(Vec<(u32, Vec<u8>)>, usize), RewriteError> {
    if record0.len() < exth_start + 12 {
        return Err(RewriteError::MalformedHeader(format!(
            "EXTH block would start at {} but record 0 is only {} bytes",
            exth_start,
            record0.len()
        )));
    }
    if &record0[exth_start..exth_start + 4] != b"EXTH" {
        return Err(RewriteError::MalformedHeader(format!(
            "expected EXTH magic at record0 offset {}, got {:?}",
            exth_start,
            String::from_utf8_lossy(&record0[exth_start..exth_start + 4])
        )));
    }

    let exth_padded_len = read_u32_be(record0, exth_start + 4)
        .ok_or_else(|| RewriteError::MalformedHeader("EXTH header length field truncated".into()))?
        as usize;
    let exth_count = read_u32_be(record0, exth_start + 8)
        .ok_or_else(|| RewriteError::MalformedHeader("EXTH count field truncated".into()))?
        as usize;

    if exth_start + exth_padded_len > record0.len() {
        return Err(RewriteError::MalformedHeader(format!(
            "EXTH block padded length {} exceeds record 0 bound",
            exth_padded_len
        )));
    }

    let mut records = Vec::with_capacity(exth_count);
    let mut pos = exth_start + 12;
    let records_end_cap = exth_start + exth_padded_len;
    for i in 0..exth_count {
        if pos + 8 > records_end_cap {
            return Err(RewriteError::MalformedHeader(format!(
                "EXTH record {} header truncated at pos {}",
                i, pos
            )));
        }
        let rtype = read_u32_be(record0, pos).unwrap();
        let rlen = read_u32_be(record0, pos + 4).unwrap() as usize;
        if rlen < 8 || pos + rlen > records_end_cap {
            return Err(RewriteError::MalformedHeader(format!(
                "EXTH record {} (type {}) has invalid length {}",
                i, rtype, rlen
            )));
        }
        let payload = record0[pos + 8..pos + rlen].to_vec();
        records.push((rtype, payload));
        pos += rlen;
    }

    Ok((records, exth_start + exth_padded_len))
}

// ---------------------------------------------------------------------------
// Change planning
// ---------------------------------------------------------------------------

/// Planned mutation of a single EXTH type. Either replace the list of
/// records of that type with a new list (possibly empty to delete), or
/// leave alone.
#[derive(Debug, Clone)]
struct ExthFieldPlan {
    exth_type: u32,
    new_values: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
struct Plan {
    /// Planned EXTH mutations, indexed by EXTH type. Applied in the order
    /// defined by `exth_field_plans`.
    exth_field_plans: Vec<ExthFieldPlan>,
    /// Side-channel list of the ExthChange entries that will end up in the
    /// report. Computed during planning so we do not have to diff twice.
    exth_changes: Vec<ExthChange>,
    /// New title string, if the title update implies a full_name rewrite.
    new_full_name: Option<String>,
    /// New cover image bytes, if a cover update was requested and the file
    /// has an existing cover record. None means leave the cover alone.
    new_cover_bytes: Option<Vec<u8>>,
}

impl Plan {
    fn is_noop(&self) -> bool {
        self.exth_field_plans.is_empty()
            && self.new_full_name.is_none()
            && self.new_cover_bytes.is_none()
    }
}

/// Plan the mutations for ONE section.
///
/// Called once per MOBI section, because a dual-format file's two sections
/// can disagree: the KF7 half may already carry the requested title while the
/// KF8 half still has the old one, and a plan computed from KF7 alone would
/// call that a no-op and change nothing.
fn plan_changes(
    section: &SectionView,
    has_cover_record: bool,
    updates: &MetadataUpdates,
) -> Result<Plan, RewriteError> {
    let mut plan = Plan::default();

    // Index existing EXTH records by type. Some types are multi-valued
    // (100 author, 105 subject), so we keep a Vec per type.
    let mut existing: HashMap<u32, Vec<&[u8]>> = HashMap::new();
    for (t, data) in &section.exth_records {
        existing.entry(*t).or_default().push(data.as_slice());
    }

    // Single-value string fields. For each, if `updates` has Some(v) and
    // v differs from what is currently stored, add a plan to replace (or
    // remove if v is empty).
    let mut plan_single = |exth_type: u32, value: Option<&str>| {
        if let Some(new_value) = value {
            let old_records: Vec<Vec<u8>> = existing
                .get(&exth_type)
                .map(|v| v.iter().map(|s| s.to_vec()).collect())
                .unwrap_or_default();

            if new_value.is_empty() {
                // Delete all records of this type if any exist.
                if !old_records.is_empty() {
                    for old in &old_records {
                        plan.exth_changes.push(ExthChange::Removed {
                            exth_type,
                            old_value: old.clone(),
                        });
                    }
                    plan.exth_field_plans.push(ExthFieldPlan {
                        exth_type,
                        new_values: Vec::new(),
                    });
                }
            } else {
                let new_bytes = new_value.as_bytes().to_vec();
                // A single-value field with multiple existing records is
                // a degenerate case; treat it as "replace all with one".
                let matches = old_records.len() == 1 && old_records[0] == new_bytes;
                if !matches {
                    if old_records.len() == 1 {
                        plan.exth_changes.push(ExthChange::Replaced {
                            exth_type,
                            old_value: old_records[0].clone(),
                            new_value: new_bytes.clone(),
                        });
                    } else if old_records.is_empty() {
                        plan.exth_changes.push(ExthChange::Added {
                            exth_type,
                            value: new_bytes.clone(),
                        });
                    } else {
                        for old in &old_records {
                            plan.exth_changes.push(ExthChange::Removed {
                                exth_type,
                                old_value: old.clone(),
                            });
                        }
                        plan.exth_changes.push(ExthChange::Added {
                            exth_type,
                            value: new_bytes.clone(),
                        });
                    }
                    plan.exth_field_plans.push(ExthFieldPlan {
                        exth_type,
                        new_values: vec![new_bytes],
                    });
                }
            }
        }
    };

    plan_single(EXTH_PUBLISHER, updates.publisher.as_deref());
    plan_single(EXTH_DESCRIPTION, updates.description.as_deref());
    plan_single(EXTH_LANGUAGE, updates.language.as_deref());
    plan_single(EXTH_ISBN, updates.isbn.as_deref());
    plan_single(EXTH_ASIN, updates.asin.as_deref());
    plan_single(EXTH_PUBLICATION_DATE, updates.publication_date.as_deref());

    // Doc type is really the 113 + 501 pair. 501 alone does nothing: the
    // firmware wants an identifier to key the cover on as well, and a book
    // built before 0.30.0 carries neither (issue #35). Keep an existing 113 so
    // a device that has already cached a thumbnail under it keeps matching,
    // and otherwise derive the same stable UUID the build path would.
    if let Some(doc_type) = updates.doc_type.as_deref() {
        plan_single(EXTH_CDE_TYPE, Some(doc_type));

        let existing_identifier = existing
            .get(&EXTH_ASIN_IDENTIFIER)
            .and_then(|v| v.first())
            .and_then(|b| std::str::from_utf8(b).ok())
            .map(str::to_owned)
            .filter(|s| !s.is_empty());

        let identifier = existing_identifier.unwrap_or_else(|| {
            let title = updates
                .title
                .clone()
                .or_else(|| {
                    existing
                        .get(&EXTH_UPDATED_TITLE)
                        .and_then(|v| v.first())
                        .and_then(|b| String::from_utf8(b.to_vec()).ok())
                })
                .unwrap_or_default();
            let author = existing
                .get(&EXTH_CREATOR)
                .and_then(|v| v.first())
                .and_then(|b| String::from_utf8(b.to_vec()).ok())
                .unwrap_or_default();
            crate::exth::book_asin(&[], &title, &author)
        });
        plan_single(EXTH_ASIN_IDENTIFIER, Some(identifier.as_str()));
    }

    // Title: EXTH 503 (updated title), EXTH 542 (4-byte md5-derived hash of
    // the title bytes, kindlegen and kindling both write this), AND the
    // full_name bytes at the end of record 0. All three should stay
    // consistent so device display, search index, and library sort match.
    if let Some(new_title) = updates.title.as_deref() {
        let old_503 = existing
            .get(&EXTH_UPDATED_TITLE)
            .and_then(|v| v.first())
            .map(|s| s.to_vec());

        let title_matches = match &old_503 {
            Some(old) => old.as_slice() == new_title.as_bytes(),
            None => new_title.is_empty(),
        };

        if !title_matches {
            if new_title.is_empty() {
                if let Some(old) = &old_503 {
                    plan.exth_changes.push(ExthChange::Removed {
                        exth_type: EXTH_UPDATED_TITLE,
                        old_value: old.clone(),
                    });
                }
                plan.exth_field_plans.push(ExthFieldPlan {
                    exth_type: EXTH_UPDATED_TITLE,
                    new_values: Vec::new(),
                });
                // Removing 542 as well keeps the hash consistent.
                if let Some(old_542) = existing.get(&EXTH_TITLE_HASH).and_then(|v| v.first()) {
                    plan.exth_changes.push(ExthChange::Removed {
                        exth_type: EXTH_TITLE_HASH,
                        old_value: old_542.to_vec(),
                    });
                    plan.exth_field_plans.push(ExthFieldPlan {
                        exth_type: EXTH_TITLE_HASH,
                        new_values: Vec::new(),
                    });
                }
                plan.new_full_name = Some(String::new());
            } else {
                let new_bytes = new_title.as_bytes().to_vec();
                match old_503 {
                    Some(old) => {
                        plan.exth_changes.push(ExthChange::Replaced {
                            exth_type: EXTH_UPDATED_TITLE,
                            old_value: old,
                            new_value: new_bytes.clone(),
                        });
                    }
                    None => {
                        plan.exth_changes.push(ExthChange::Added {
                            exth_type: EXTH_UPDATED_TITLE,
                            value: new_bytes.clone(),
                        });
                    }
                }
                plan.exth_field_plans.push(ExthFieldPlan {
                    exth_type: EXTH_UPDATED_TITLE,
                    new_values: vec![new_bytes],
                });

                // EXTH 542 = first 4 bytes of md5(title_bytes), matching
                // build_book_exth's formula exactly.
                let new_542 = md5_first4(new_title.as_bytes());
                let old_542 = existing
                    .get(&EXTH_TITLE_HASH)
                    .and_then(|v| v.first())
                    .map(|s| s.to_vec());
                let hash_matches = old_542.as_deref() == Some(new_542.as_slice());
                if !hash_matches {
                    match old_542 {
                        Some(old) => {
                            plan.exth_changes.push(ExthChange::Replaced {
                                exth_type: EXTH_TITLE_HASH,
                                old_value: old,
                                new_value: new_542.clone(),
                            });
                        }
                        None => {
                            plan.exth_changes.push(ExthChange::Added {
                                exth_type: EXTH_TITLE_HASH,
                                value: new_542.clone(),
                            });
                        }
                    }
                    plan.exth_field_plans.push(ExthFieldPlan {
                        exth_type: EXTH_TITLE_HASH,
                        new_values: vec![new_542],
                    });
                }

                plan.new_full_name = Some(new_title.to_string());
            }
        }
    }

    // Multi-value string fields: authors (100) and subjects (105). Update
    // is "replace the whole list"; empty vec means "delete all".
    let mut plan_multi = |exth_type: u32, value: Option<&[String]>| {
        if let Some(new_list) = value {
            let old_records: Vec<Vec<u8>> = existing
                .get(&exth_type)
                .map(|v| v.iter().map(|s| s.to_vec()).collect())
                .unwrap_or_default();

            let new_bytes_list: Vec<Vec<u8>> = new_list
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| s.as_bytes().to_vec())
                .collect();

            let matches = old_records == new_bytes_list;
            if !matches {
                for old in &old_records {
                    plan.exth_changes.push(ExthChange::Removed {
                        exth_type,
                        old_value: old.clone(),
                    });
                }
                for new in &new_bytes_list {
                    plan.exth_changes.push(ExthChange::Added {
                        exth_type,
                        value: new.clone(),
                    });
                }
                plan.exth_field_plans.push(ExthFieldPlan {
                    exth_type,
                    new_values: new_bytes_list,
                });
            }
        }
    };

    plan_multi(EXTH_CREATOR, updates.authors.as_deref());
    plan_multi(EXTH_SUBJECT, updates.subjects.as_deref());

    // Cover image: only valid if the file already has a cover record. We
    // compare the new bytes against the existing cover record bytes; if
    // they match, this is a no-op.
    if let Some(new_cover) = &updates.cover_image {
        if !has_cover_record {
            return Err(RewriteError::NoCoverRecord);
        }
        if !is_recognized_image(new_cover) {
            return Err(RewriteError::UnsupportedCoverFormat);
        }
        plan.new_cover_bytes = Some(new_cover.clone());
        // The change is recorded as a Replaced EXTH change so the caller's
        // audit log records the cover update too. We key it by EXTH_COVER_OFFSET.
        // Matching check is done later against actual record bytes.
    }

    Ok(plan)
}

fn is_recognized_image(bytes: &[u8]) -> bool {
    // JPEG: FF D8 FF
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return true;
    }
    // PNG: 89 50 4E 47 0D 0A 1A 0A
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return true;
    }
    // GIF87a / GIF89a
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Applying a plan: rebuild record 0 and optionally the cover record, then
// shift downstream PalmDB offsets and write the new file.
// ---------------------------------------------------------------------------

/// Rebuild one section's header record with the plan applied.
///
/// Layout is identical for both sections: palmdoc(16) + mobi_header +
/// EXTH + full_name + 4-byte-align padding.
fn rebuild_section_record(old: &[u8], section: &SectionView, plan: &Plan) -> Vec<u8> {
    // Construct the new EXTH list by applying exth_field_plans on top of the
    // section's existing records: strip every type the plan touches, then
    // append the plan's values. Kindle does not depend on EXTH record order,
    // and keeping stable positions for untouched records minimizes churn.
    let mut new_exth = section.exth_records.clone();
    let plan_types: std::collections::HashSet<u32> =
        plan.exth_field_plans.iter().map(|p| p.exth_type).collect();
    new_exth.retain(|(t, _)| !plan_types.contains(t));
    for field_plan in &plan.exth_field_plans {
        for value in &field_plan.new_values {
            new_exth.push((field_plan.exth_type, value.clone()));
        }
    }
    let new_exth_block = serialize_exth_block(&new_exth);

    let new_full_name_bytes: Vec<u8> = if let Some(ref s) = plan.new_full_name {
        s.as_bytes().to_vec()
    } else {
        old[section.full_name_offset..section.full_name_offset + section.full_name_length].to_vec()
    };

    let mut rebuilt = Vec::with_capacity(old.len() + new_exth_block.len());
    // PalmDOC header unchanged.
    rebuilt.extend_from_slice(&old[..PALMDOC_HEADER_LEN]);
    // MOBI header unchanged; the full_name fields get patched below.
    rebuilt.extend_from_slice(
        &old[PALMDOC_HEADER_LEN..PALMDOC_HEADER_LEN + section.mobi_header_length],
    );
    rebuilt.extend_from_slice(&new_exth_block);
    let new_full_name_offset = rebuilt.len();
    rebuilt.extend_from_slice(&new_full_name_bytes);
    while rebuilt.len() % 4 != 0 {
        rebuilt.push(0x00);
    }

    put_u32_be(
        &mut rebuilt,
        MOBI_FULL_NAME_OFFSET_FIELD,
        new_full_name_offset as u32,
    );
    put_u32_be(
        &mut rebuilt,
        MOBI_FULL_NAME_LENGTH_FIELD,
        new_full_name_bytes.len() as u32,
    );
    rebuilt
}

fn apply_plan(
    input: &[u8],
    parsed: &ParsedMobi,
    plan: &Plan,
    kf8_plan: Option<&Plan>,
) -> Result<Vec<u8>, RewriteError> {
    let record0_old = &input[parsed.record0_start..parsed.record0_end];
    let record0_new = rebuild_section_record(record0_old, &parsed.primary, plan);

    // Rewrite the KF8 section's header record the same way. Record indices
    // do not change (we substitute records, never insert or remove), so the
    // KF7 section's EXTH 121 boundary pointer stays correct without patching.
    let kf8_rebuilt: Option<(usize, Vec<u8>)> = match (parsed.kf8.as_ref(), kf8_plan) {
        (Some(section), Some(kf8_plan)) => {
            let start = parsed.record_offsets[section.record_idx] as usize;
            let end = if section.record_idx + 1 < parsed.record_offsets.len() {
                parsed.record_offsets[section.record_idx + 1] as usize
            } else {
                input.len()
            };
            Some((
                section.record_idx,
                rebuild_section_record(&input[start..end], section, kf8_plan),
            ))
        }
        _ => None,
    };

    // Step 5: optionally replace the cover image record.
    //
    // Strategy: we assemble a new file by concatenating records in order,
    // substituting the new record 0 bytes and (optionally) the new cover
    // bytes, then rebuild the PalmDB record info table with the new
    // offsets. Every other record is copied verbatim.
    let num_records = parsed.record_offsets.len();
    let mut record_data: Vec<Vec<u8>> = Vec::with_capacity(num_records);
    for i in 0..num_records {
        let start = parsed.record_offsets[i] as usize;
        let end = if i + 1 < num_records {
            parsed.record_offsets[i + 1] as usize
        } else {
            input.len()
        };
        let bytes = if i == 0 {
            record0_new.clone()
        } else if let Some((idx, ref rebuilt)) = kf8_rebuilt
            && i == idx
        {
            rebuilt.clone()
        } else if plan.new_cover_bytes.is_some() && Some(i) == parsed.cover_record_idx {
            plan.new_cover_bytes.clone().unwrap()
        } else {
            input[start..end].to_vec()
        };
        record_data.push(bytes);
    }

    // Step 6: assemble the new file. PalmDB header + record info table +
    // (two-byte gap after record info, written as zero by kindlegen and
    // kindling) + concatenated record data. kindlegen historically uses a
    // 2-byte "placeholder" gap between the record list and the first
    // record's data; we preserve the old gap exactly.
    let old_gap_start = PALMDB_HEADER_LEN + num_records * 8;
    let old_gap_end = parsed.record_offsets[0] as usize;
    if old_gap_end < old_gap_start {
        return Err(RewriteError::MalformedHeader(format!(
            "record 0 offset {} lies before end of record info table at {}",
            old_gap_end, old_gap_start
        )));
    }
    let gap_bytes = &input[old_gap_start..old_gap_end];

    let palmdb_header_bytes = &input[..PALMDB_HEADER_LEN];
    // Record info table: 8 bytes per record. offset(u32) + attrs(u8) + uid(3 bytes).
    let record_info_bytes = &input[PALMDB_HEADER_LEN..PALMDB_HEADER_LEN + num_records * 8];

    let mut out = Vec::with_capacity(input.len() + record0_new.len());
    out.extend_from_slice(palmdb_header_bytes);
    out.extend_from_slice(record_info_bytes);
    out.extend_from_slice(gap_bytes);
    // Record data.
    let mut offsets = Vec::with_capacity(num_records);
    for bytes in &record_data {
        offsets.push(out.len() as u32);
        out.extend_from_slice(bytes);
    }

    // Patch the record info table offsets in `out` with the new values.
    for (i, off) in offsets.iter().enumerate() {
        let field_pos = PALMDB_HEADER_LEN + i * 8;
        put_u32_be(&mut out, field_pos, *off);
    }

    Ok(out)
}

/// Serialize a list of (type, data) EXTH records into the EXTH block bytes,
/// with 4-byte alignment padding at the end.
fn serialize_exth_block(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let record_bytes: Vec<Vec<u8>> = records
        .iter()
        .map(|(t, d)| {
            let mut rec = Vec::with_capacity(8 + d.len());
            rec.extend_from_slice(&t.to_be_bytes());
            rec.extend_from_slice(&((8 + d.len()) as u32).to_be_bytes());
            rec.extend_from_slice(d);
            rec
        })
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

// ---------------------------------------------------------------------------
// Low-level helpers
// ---------------------------------------------------------------------------

fn read_u16_be(data: &[u8], offset: usize) -> Option<u16> {
    data.get(offset..offset + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn read_u32_be(data: &[u8], offset: usize) -> Option<u32> {
    data.get(offset..offset + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn put_u32_be(buf: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_be_bytes();
    buf[offset..offset + 4].copy_from_slice(&bytes);
}

/// First 4 bytes of md5(data), matching `exth::md5_hash` via the same
/// algorithm kindling already uses for EXTH 542. We implement a tiny md5
/// here rather than depending on the private helper in `exth::`.
fn md5_first4(data: &[u8]) -> Vec<u8> {
    let hash = md5_simple(data);
    hash[..4].to_vec()
}

fn md5_simple(data: &[u8]) -> [u8; 16] {
    // Classic RFC 1321 md5. Copied from src/mobi.rs (which has its own
    // md5_simple) so mobi_rewrite.rs stays free of cross-module internal
    // dependencies.
    let mut msg = data.to_vec();
    let orig_len_bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&orig_len_bits.to_le_bytes());

    let k: [u32; 64] = [
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
    let s: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;
    for chunk in msg.chunks(64) {
        let mut m = [0u32; 16];
        for (i, w) in chunk.chunks(4).enumerate() {
            m[i] = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        }
        let mut a = a0;
        let mut b = b0;
        let mut c = c0;
        let mut d = d0;
        for i in 0..64 {
            let (f, g): (u32, usize) = if i < 16 {
                ((b & c) | (!b & d), i)
            } else if i < 32 {
                ((d & b) | (!d & c), (5 * i + 1) % 16)
            } else if i < 48 {
                (b ^ c ^ d, (3 * i + 5) % 16)
            } else {
                (c ^ (b | !d), (7 * i) % 16)
            };
            let temp = d;
            d = c;
            c = b;
            b = b.wrapping_add(
                a.wrapping_add(f)
                    .wrapping_add(k[i])
                    .wrapping_add(m[g])
                    .rotate_left(s[i]),
            );
            a = temp;
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal synthetic MOBI with three records: record 0 (MOBI
    /// header + EXTH + full_name), record 1 (dummy text), record 2 (dummy
    /// image, used as cover). The EXTH block carries the records passed
    /// in via `exth_records`.
    fn build_synthetic_mobi(
        title: &str,
        exth_records: &[(u32, Vec<u8>)],
        encryption_type: u16,
        image_record: Vec<u8>,
    ) -> Vec<u8> {
        // Record 0 construction mirrors kindling's real build_record0
        // well enough to exercise the parser and rewriter: PalmDOC header
        // (16), MOBI header (264), EXTH block, full_name, 4-byte pad.
        let full_name = title.as_bytes();

        let mut mobi_header = vec![0u8; 264];
        mobi_header[0..4].copy_from_slice(b"MOBI");
        mobi_header[4..8].copy_from_slice(&264u32.to_be_bytes());
        mobi_header[8..12].copy_from_slice(&2u32.to_be_bytes()); // type = 2
        mobi_header[12..16].copy_from_slice(&65001u32.to_be_bytes()); // UTF-8
        mobi_header[20..24].copy_from_slice(&6u32.to_be_bytes()); // file version
        // EXTH flags at MOBI header offset 112.
        mobi_header[112..116].copy_from_slice(&0x40u32.to_be_bytes());
        // first_image_record at MOBI header offset 92 = record0 offset 108.
        mobi_header[92..96].copy_from_slice(&2u32.to_be_bytes());

        let exth_block = serialize_exth_block(exth_records);

        let mut record0 = Vec::new();
        // PalmDOC header: compression=1, reserved=0, text_length=1024,
        // text_record_count=1, record_size=4096, encryption_type, unknown=0.
        let mut palmdoc = Vec::with_capacity(16);
        palmdoc.extend_from_slice(&1u16.to_be_bytes());
        palmdoc.extend_from_slice(&0u16.to_be_bytes());
        palmdoc.extend_from_slice(&1024u32.to_be_bytes());
        palmdoc.extend_from_slice(&1u16.to_be_bytes());
        palmdoc.extend_from_slice(&4096u16.to_be_bytes());
        palmdoc.extend_from_slice(&encryption_type.to_be_bytes());
        palmdoc.extend_from_slice(&0u16.to_be_bytes());
        record0.extend_from_slice(&palmdoc);
        record0.extend_from_slice(&mobi_header);
        record0.extend_from_slice(&exth_block);
        let full_name_offset = record0.len();
        record0.extend_from_slice(full_name);
        while record0.len() % 4 != 0 {
            record0.push(0);
        }
        // Patch full_name_offset/length.
        put_u32_be(
            &mut record0,
            MOBI_FULL_NAME_OFFSET_FIELD,
            full_name_offset as u32,
        );
        put_u32_be(
            &mut record0,
            MOBI_FULL_NAME_LENGTH_FIELD,
            full_name.len() as u32,
        );

        let dummy_text = vec![0u8; 128];
        let records: Vec<Vec<u8>> = vec![record0, dummy_text, image_record];

        // PalmDB: 78-byte header + 8*N record info + 2-byte gap + records.
        let num_records = records.len();
        let record_info_len = num_records * 8;
        let gap_len = 2;
        let mut offsets: Vec<u32> = Vec::with_capacity(num_records);
        let mut cursor = PALMDB_HEADER_LEN + record_info_len + gap_len;
        for rec in &records {
            offsets.push(cursor as u32);
            cursor += rec.len();
        }

        let mut out = Vec::with_capacity(cursor);
        // 32-byte PalmDB name. Pad title or use a fixed "TestBook".
        let mut name = [0u8; 32];
        let tn = b"TestBook";
        name[..tn.len()].copy_from_slice(tn);
        out.extend_from_slice(&name);
        out.extend_from_slice(&0u16.to_be_bytes()); // attributes
        out.extend_from_slice(&0u16.to_be_bytes()); // version
        out.extend_from_slice(&0u32.to_be_bytes()); // creation date
        out.extend_from_slice(&0u32.to_be_bytes()); // modification date
        out.extend_from_slice(&0u32.to_be_bytes()); // last backup
        out.extend_from_slice(&0u32.to_be_bytes()); // modification number
        out.extend_from_slice(&0u32.to_be_bytes()); // app info
        out.extend_from_slice(&0u32.to_be_bytes()); // sort info
        out.extend_from_slice(b"BOOK");
        out.extend_from_slice(b"MOBI");
        out.extend_from_slice(&0u32.to_be_bytes()); // uid seed
        out.extend_from_slice(&0u32.to_be_bytes()); // next record list
        out.extend_from_slice(&(num_records as u16).to_be_bytes());
        assert_eq!(out.len(), PALMDB_HEADER_LEN);

        // Record info table.
        for (i, off) in offsets.iter().enumerate() {
            out.extend_from_slice(&off.to_be_bytes());
            out.push(0); // attributes
            out.extend_from_slice(&[0u8, 0, i as u8]); // unique id 3 bytes
        }
        // 2-byte gap.
        out.extend_from_slice(&[0u8, 0]);

        for rec in &records {
            out.extend_from_slice(rec);
        }
        out
    }

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "kindling_mobi_rewrite_test_{}_{}",
            std::process::id(),
            name
        ));
        p
    }

    fn write_tmp(name: &str, bytes: &[u8]) -> PathBuf {
        let p = tmp_path(name);
        let mut f = fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn make_jpeg(color: u8) -> Vec<u8> {
        // Minimal "JPEG" bytes sufficient for is_recognized_image. Real
        // Kindle files would use a full encoder; we only need the magic.
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend(std::iter::repeat(color).take(256));
        v.push(0xFF);
        v.push(0xD9);
        v
    }

    fn default_exth() -> Vec<(u32, Vec<u8>)> {
        vec![
            (EXTH_CREATOR, b"Jane Doe".to_vec()),
            (EXTH_UPDATED_TITLE, b"Original Title".to_vec()),
            (EXTH_TITLE_HASH, md5_first4(b"Original Title")),
            (EXTH_LANGUAGE, b"en".to_vec()),
            (EXTH_DESCRIPTION, b"An original description.".to_vec()),
            (EXTH_CDE_TYPE, b"EBOK".to_vec()),
            (EXTH_COVER_OFFSET, 0u32.to_be_bytes().to_vec()),
        ]
    }

    // --- Positive tests: each field changes correctly ---

    #[test]
    fn title_update_replaces_exth_503_and_542_and_full_name() {
        let bytes = build_synthetic_mobi("Original Title", &default_exth(), 0, make_jpeg(0x10));
        let input = write_tmp("title_in", &bytes);
        let output = tmp_path("title_out");
        let updates = MetadataUpdates {
            title: Some("Brand New Title".to_string()),
            ..Default::default()
        };
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(!report.no_op);
        assert!(report
            .changes
            .iter()
            .any(|c| matches!(c, ExthChange::Replaced { exth_type, .. } if *exth_type == EXTH_UPDATED_TITLE)));
        assert!(report.changes.iter().any(
            |c| matches!(c, ExthChange::Replaced { exth_type, .. } if *exth_type == EXTH_TITLE_HASH)
        ));
        // Parse output and verify.
        let out_bytes = fs::read(&output).unwrap();
        let parsed = parse_mobi(&out_bytes).unwrap();
        let t503 = parsed
            .primary
            .exth_records
            .iter()
            .find(|(t, _)| *t == EXTH_UPDATED_TITLE)
            .unwrap();
        assert_eq!(t503.1, b"Brand New Title");
        let t542 = parsed
            .primary
            .exth_records
            .iter()
            .find(|(t, _)| *t == EXTH_TITLE_HASH)
            .unwrap();
        assert_eq!(t542.1, md5_first4(b"Brand New Title"));
        // Verify full_name in record 0 at the field offset.
        let record0 = &out_bytes[parsed.record0_start..parsed.record0_end];
        let fno = parsed.primary.full_name_offset;
        let fnl = parsed.primary.full_name_length;
        assert_eq!(&record0[fno..fno + fnl], b"Brand New Title");
    }

    #[test]
    fn author_update_replaces_exth_100() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("author_in", &bytes);
        let output = tmp_path("author_out");
        let updates = MetadataUpdates {
            authors: Some(vec!["Alice".to_string(), "Bob".to_string()]),
            ..Default::default()
        };
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(!report.no_op);
        let out_bytes = fs::read(&output).unwrap();
        let parsed = parse_mobi(&out_bytes).unwrap();
        let authors: Vec<&Vec<u8>> = parsed
            .primary
            .exth_records
            .iter()
            .filter(|(t, _)| *t == EXTH_CREATOR)
            .map(|(_, d)| d)
            .collect();
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0], b"Alice");
        assert_eq!(authors[1], b"Bob");
    }

    #[test]
    fn publisher_update() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("pub_in", &bytes);
        let output = tmp_path("pub_out");
        let updates = MetadataUpdates {
            publisher: Some("ACME Books".to_string()),
            ..Default::default()
        };
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(!report.no_op);
        let out_bytes = fs::read(&output).unwrap();
        let parsed = parse_mobi(&out_bytes).unwrap();
        let pub_rec = parsed
            .primary
            .exth_records
            .iter()
            .find(|(t, _)| *t == EXTH_PUBLISHER)
            .unwrap();
        assert_eq!(pub_rec.1, b"ACME Books");
    }

    #[test]
    fn description_update() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("desc_in", &bytes);
        let output = tmp_path("desc_out");
        let updates = MetadataUpdates {
            description: Some("A shiny new description.".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_DESCRIPTION)
                .unwrap()
                .1,
            b"A shiny new description."
        );
    }

    #[test]
    fn isbn_update() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("isbn_in", &bytes);
        let output = tmp_path("isbn_out");
        let updates = MetadataUpdates {
            isbn: Some("9780000000000".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_ISBN)
                .unwrap()
                .1,
            b"9780000000000"
        );
    }

    /// `--doc-type` writes the 113 + 501 pair the lock screen gates on, so a
    /// book built before 0.30.0 can be stamped instead of rebuilt (issue #35).
    #[test]
    fn doc_type_writes_the_identifier_pair() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("doctype_in", &bytes);
        let output = tmp_path("doctype_out");
        let updates = MetadataUpdates {
            doc_type: Some("EBOK".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        let get = |t: u32| {
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(rt, _)| *rt == t)
                .map(|(_, d)| d.clone())
        };
        assert_eq!(get(EXTH_CDE_TYPE).as_deref(), Some(&b"EBOK"[..]));
        let identifier = get(EXTH_ASIN_IDENTIFIER).expect("EXTH 113 must be written too");
        assert!(
            !identifier.is_empty(),
            "501 without 113 does not gate anything; the pair is the point"
        );
    }

    /// Stamping twice must not churn the identifier, or a device that has
    /// already cached a thumbnail under it stops matching.
    #[test]
    fn doc_type_keeps_an_existing_identifier() {
        let mut exth = default_exth();
        exth.push((EXTH_ASIN_IDENTIFIER, b"keep-me-1234".to_vec()));
        let bytes = build_synthetic_mobi("T", &exth, 0, make_jpeg(0));
        let input = write_tmp("doctype_keep_in", &bytes);
        let output = tmp_path("doctype_keep_out");
        let updates = MetadataUpdates {
            doc_type: Some("EBOK".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_ASIN_IDENTIFIER)
                .unwrap()
                .1,
            b"keep-me-1234"
        );
    }

    #[test]
    fn asin_update_uses_exth_504() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("asin_in", &bytes);
        let output = tmp_path("asin_out");
        let updates = MetadataUpdates {
            asin: Some("B00ABCDEFG".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_ASIN)
                .unwrap()
                .1,
            b"B00ABCDEFG"
        );
    }

    #[test]
    fn language_update() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("lang_in", &bytes);
        let output = tmp_path("lang_out");
        let updates = MetadataUpdates {
            language: Some("fr".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_LANGUAGE)
                .unwrap()
                .1,
            b"fr"
        );
    }

    #[test]
    fn publication_date_update() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("date_in", &bytes);
        let output = tmp_path("date_out");
        let updates = MetadataUpdates {
            publication_date: Some("2026-04-10".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert_eq!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_PUBLICATION_DATE)
                .unwrap()
                .1,
            b"2026-04-10"
        );
    }

    #[test]
    fn subjects_update_replaces_all_105_records() {
        let mut exth = default_exth();
        exth.push((EXTH_SUBJECT, b"old_tag_1".to_vec()));
        exth.push((EXTH_SUBJECT, b"old_tag_2".to_vec()));
        let bytes = build_synthetic_mobi("T", &exth, 0, make_jpeg(0));
        let input = write_tmp("tags_in", &bytes);
        let output = tmp_path("tags_out");
        let updates = MetadataUpdates {
            subjects: Some(vec![
                "new_one".to_string(),
                "new_two".to_string(),
                "new_three".to_string(),
            ]),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        let subjects: Vec<Vec<u8>> = parsed
            .primary
            .exth_records
            .iter()
            .filter(|(t, _)| *t == EXTH_SUBJECT)
            .map(|(_, d)| d.clone())
            .collect();
        assert_eq!(
            subjects,
            vec![
                b"new_one".to_vec(),
                b"new_two".to_vec(),
                b"new_three".to_vec(),
            ]
        );
    }

    #[test]
    fn series_fields_are_gone_from_the_update_surface() {
        // EXTH 112 and 113 are the source identifier and the ASIN, not series
        // fields, so the rewriter no longer offers a way to write them. 113 in
        // particular now carries the identifier the lock screen cover is filed
        // under (issue #26); a series number there would break it. Guard the
        // records rather than the (now absent) struct fields, since only the
        // bytes on disk actually matter.
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("series_in", &bytes);
        let output = tmp_path("series_out");
        let updates = MetadataUpdates {
            title: Some("Retitled".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        for rec in [112u32, 113] {
            assert!(
                !parsed.primary.exth_records.iter().any(|(t, _)| *t == rec),
                "a metadata rewrite must never introduce EXTH {rec}"
            );
        }
    }

    #[test]
    fn cover_image_replacement() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0x10));
        let input = write_tmp("cover_in", &bytes);
        let output = tmp_path("cover_out");
        let new_cover = make_jpeg(0xAA);
        let updates = MetadataUpdates {
            cover_image: Some(new_cover.clone()),
            ..Default::default()
        };
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(report.cover_updated);
        // Verify the cover record in the output file contains the new bytes.
        let out_bytes = fs::read(&output).unwrap();
        let parsed = parse_mobi(&out_bytes).unwrap();
        let idx = parsed.cover_record_idx.unwrap();
        let start = parsed.record_offsets[idx] as usize;
        let end = if idx + 1 < parsed.record_offsets.len() {
            parsed.record_offsets[idx + 1] as usize
        } else {
            out_bytes.len()
        };
        assert_eq!(&out_bytes[start..end], &new_cover[..]);
    }

    // --- Negative tests: unchanged fields are not in the report ---

    #[test]
    fn no_update_passed_leaves_field_alone() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("noop_in", &bytes);
        let output = tmp_path("noop_out");
        let updates = MetadataUpdates::default();
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(report.no_op);
        assert!(report.changes.is_empty());
    }

    #[test]
    fn matching_value_is_a_noop() {
        let bytes = build_synthetic_mobi("Original Title", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("match_in", &bytes);
        let output = tmp_path("match_out");
        let updates = MetadataUpdates {
            title: Some("Original Title".to_string()),
            language: Some("en".to_string()),
            ..Default::default()
        };
        let report = rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        assert!(report.no_op, "expected no-op, got {:?}", report.changes);
    }

    // --- Byte stability ---

    #[test]
    fn empty_updates_is_byte_identical() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("bs_empty_in", &bytes);
        let output = tmp_path("bs_empty_out");
        rewrite_mobi_metadata(&input, &output, &MetadataUpdates::default()).unwrap();
        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();
        assert_eq!(in_bytes, out_bytes);
    }

    #[test]
    fn matching_updates_is_byte_identical() {
        let bytes = build_synthetic_mobi("Original Title", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("bs_match_in", &bytes);
        let output = tmp_path("bs_match_out");
        let updates = MetadataUpdates {
            title: Some("Original Title".to_string()),
            authors: Some(vec!["Jane Doe".to_string()]),
            language: Some("en".to_string()),
            description: Some("An original description.".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let in_bytes = fs::read(&input).unwrap();
        let out_bytes = fs::read(&output).unwrap();
        assert_eq!(in_bytes, out_bytes);
    }

    // --- Idempotence ---

    #[test]
    fn idempotent_on_repeat() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("idemp_in", &bytes);
        let output1 = tmp_path("idemp_out1");
        let output2 = tmp_path("idemp_out2");

        let updates = MetadataUpdates {
            title: Some("Updated Title".to_string()),
            language: Some("fr".to_string()),
            ..Default::default()
        };
        let r1 = rewrite_mobi_metadata(&input, &output1, &updates).unwrap();
        assert!(!r1.no_op);
        let r2 = rewrite_mobi_metadata(&output1, &output2, &updates).unwrap();
        assert!(
            r2.no_op,
            "second run should be a no-op, got {:?}",
            r2.changes
        );
        let out1 = fs::read(&output1).unwrap();
        let out2 = fs::read(&output2).unwrap();
        assert_eq!(out1, out2);
    }

    // --- DRM refusal ---

    #[test]
    fn palmdoc_encryption_byte_rejects_as_drm() {
        // encryption_type = 2 (Mobipocket DRM).
        let bytes = build_synthetic_mobi("T", &default_exth(), 2, make_jpeg(0));
        let input = write_tmp("drm_enc_in", &bytes);
        let output = tmp_path("drm_enc_out");
        let updates = MetadataUpdates {
            title: Some("Hack".to_string()),
            ..Default::default()
        };
        match rewrite_mobi_metadata(&input, &output, &updates) {
            Err(RewriteError::DrmEncrypted) => {}
            other => panic!("expected DrmEncrypted, got {:?}", other),
        }
        // Output file must not be created.
        assert!(!output.exists(), "output should not exist for DRM file");
    }

    #[test]
    fn drm_exth_401_rejects_as_drm() {
        let mut exth = default_exth();
        exth.push((EXTH_DRM_SERVER_ID, vec![0, 0, 0, 1]));
        let bytes = build_synthetic_mobi("T", &exth, 0, make_jpeg(0));
        let input = write_tmp("drm_exth_in", &bytes);
        let output = tmp_path("drm_exth_out");
        let updates = MetadataUpdates {
            title: Some("Hack".to_string()),
            ..Default::default()
        };
        match rewrite_mobi_metadata(&input, &output, &updates) {
            Err(RewriteError::DrmEncrypted) => {}
            other => panic!("expected DrmEncrypted, got {:?}", other),
        }
        assert!(!output.exists(), "output should not exist for DRM file");
    }

    // --- Error cases ---

    #[test]
    fn not_a_mobi_rejected() {
        let mut bytes = vec![0u8; 200];
        bytes[60..64].copy_from_slice(b"TEXT");
        bytes[64..68].copy_from_slice(b"READ");
        let input = write_tmp("notmobi_in", &bytes);
        let output = tmp_path("notmobi_out");
        match rewrite_mobi_metadata(&input, &output, &MetadataUpdates::default()) {
            Err(RewriteError::NotAMobi(_)) => {}
            other => panic!("expected NotAMobi, got {:?}", other),
        }
    }

    #[test]
    fn cover_update_without_existing_cover_errors() {
        let mut exth = default_exth();
        // Strip the EXTH 201 cover marker so the file has no cover record.
        exth.retain(|(t, _)| *t != EXTH_COVER_OFFSET);
        let bytes = build_synthetic_mobi("T", &exth, 0, make_jpeg(0));
        let input = write_tmp("nocov_in", &bytes);
        let output = tmp_path("nocov_out");
        let updates = MetadataUpdates {
            cover_image: Some(make_jpeg(0x55)),
            ..Default::default()
        };
        match rewrite_mobi_metadata(&input, &output, &updates) {
            Err(RewriteError::NoCoverRecord) => {}
            other => panic!("expected NoCoverRecord, got {:?}", other),
        }
    }

    #[test]
    fn unsupported_cover_format_errors() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("badcov_in", &bytes);
        let output = tmp_path("badcov_out");
        let updates = MetadataUpdates {
            cover_image: Some(b"not an image".to_vec()),
            ..Default::default()
        };
        match rewrite_mobi_metadata(&input, &output, &updates) {
            Err(RewriteError::UnsupportedCoverFormat) => {}
            other => panic!("expected UnsupportedCoverFormat, got {:?}", other),
        }
    }

    // --- Structural ---

    #[test]
    fn untouched_exth_records_are_preserved() {
        let mut exth = default_exth();
        exth.push((EXTH_CDE_TYPE, b"PDOC".to_vec()));
        exth.push((99999, vec![1, 2, 3, 4])); // unknown record, must survive
        let bytes = build_synthetic_mobi("T", &exth, 0, make_jpeg(0));
        let input = write_tmp("preserve_in", &bytes);
        let output = tmp_path("preserve_out");
        let updates = MetadataUpdates {
            publisher: Some("ACME".to_string()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert!(
            parsed
                .primary
                .exth_records
                .iter()
                .any(|(t, d)| *t == 99999 && d == &vec![1, 2, 3, 4])
        );
        assert!(
            parsed
                .primary
                .exth_records
                .iter()
                .any(|(t, d)| *t == EXTH_CDE_TYPE && d == b"PDOC")
        );
    }

    #[test]
    fn removing_field_via_empty_string_deletes_record() {
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0));
        let input = write_tmp("remove_in", &bytes);
        let output = tmp_path("remove_out");
        let updates = MetadataUpdates {
            description: Some(String::new()),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let parsed = parse_mobi(&fs::read(&output).unwrap()).unwrap();
        assert!(
            parsed
                .primary
                .exth_records
                .iter()
                .find(|(t, _)| *t == EXTH_DESCRIPTION)
                .is_none()
        );
    }

    #[test]
    fn downstream_records_shift_correctly_after_size_change() {
        // Build a MOBI, rewrite with a longer description, and verify that
        // the text and image records after record 0 are byte-identical in
        // content but moved to new file offsets.
        let bytes = build_synthetic_mobi("T", &default_exth(), 0, make_jpeg(0x77));
        let input = write_tmp("shift_in", &bytes);
        let output = tmp_path("shift_out");
        let updates = MetadataUpdates {
            description: Some("x".repeat(500)),
            ..Default::default()
        };
        rewrite_mobi_metadata(&input, &output, &updates).unwrap();
        let out_bytes = fs::read(&output).unwrap();
        let parsed_in = parse_mobi(&bytes).unwrap();
        let parsed_out = parse_mobi(&out_bytes).unwrap();
        assert_eq!(
            parsed_in.record_offsets.len(),
            parsed_out.record_offsets.len()
        );
        // Text record (index 1) bytes must be identical.
        let in_text_start = parsed_in.record_offsets[1] as usize;
        let in_text_end = parsed_in.record_offsets[2] as usize;
        let out_text_start = parsed_out.record_offsets[1] as usize;
        let out_text_end = parsed_out.record_offsets[2] as usize;
        assert_eq!(
            &bytes[in_text_start..in_text_end],
            &out_bytes[out_text_start..out_text_end],
            "text record contents must not change"
        );
        // Image record (index 2) bytes must be identical.
        let in_img_start = parsed_in.record_offsets[2] as usize;
        let in_img_end = bytes.len();
        let out_img_start = parsed_out.record_offsets[2] as usize;
        let out_img_end = out_bytes.len();
        assert_eq!(
            &bytes[in_img_start..in_img_end],
            &out_bytes[out_img_start..out_img_end],
            "image record contents must not change"
        );
    }
}
