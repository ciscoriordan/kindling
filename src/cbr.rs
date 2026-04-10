/// CBR (Comic Book RAR) extractor.
///
/// Extracts image files from a `.cbr` archive into a temporary directory
/// so the existing comic pipeline can treat it as a plain folder.
///
/// Strategy:
///
/// We deliberately avoid linking against the unrar C library (RARLAB source,
/// proprietary licensing on WinRAR, Gatekeeper issues on macOS, extra native
/// build steps). Instead we shell out to `bsdtar`, which ships on every
/// recent macOS (`/usr/bin/bsdtar`, Apple-signed) and is available on most
/// Linux distros via libarchive. libarchive's RAR reader handles both RAR4
/// and the common RAR5 cases we care about for comics (stored / deflated
/// image data, no compression, no per-file encryption). libarchive is
/// BSD-licensed, so there are no redistribution concerns.
///
/// Limitations:
///   * Header-encrypted archives are detected and rejected with a clear
///     error (bsdtar prints "Encryption is not supported").
///   * File-content-only encrypted archives (RAR `-p` without `-hp`) cannot
///     be detected from the file list; extraction will succeed but produce
///     garbage bytes, which the image decoder will reject downstream with a
///     per-file warning. This matches bsdtar's own behavior.
///   * Solid / multi-volume archives are passed through to bsdtar; whatever
///     libarchive can read, we can read.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Locate a usable `bsdtar` binary.
///
/// Prefer the Apple-signed `/usr/bin/bsdtar` that ships with macOS (so we
/// avoid any Gatekeeper prompts when running from a drag-and-drop app bundle),
/// then fall back to whatever `bsdtar` is on `PATH` for Linux users.
fn find_bsdtar() -> Option<PathBuf> {
    let system_path = Path::new("/usr/bin/bsdtar");
    if system_path.exists() {
        return Some(system_path.to_path_buf());
    }
    // Fall back to a PATH lookup. `Command::new("bsdtar")` will do that for
    // us at exec time, so we just need to confirm one exists.
    if let Ok(output) = Command::new("bsdtar").arg("--version").output() {
        if output.status.success() {
            return Some(PathBuf::from("bsdtar"));
        }
    }
    None
}

/// Report a friendly "bsdtar not found" error message.
fn bsdtar_missing_err() -> Box<dyn std::error::Error> {
    "CBR support requires bsdtar (libarchive). It ships with macOS at \
     /usr/bin/bsdtar; on Linux install libarchive via your package manager \
     (e.g. `apt install libarchive-tools` or `dnf install bsdtar`)."
        .into()
}

/// Check if bsdtar's stderr indicates the archive is header-encrypted.
///
/// libarchive's RAR reader does not support any form of encryption, so if we
/// see the "Encryption is not supported" message in stderr we can give a
/// precise error rather than a generic "extraction failed".
fn stderr_indicates_encryption(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("encryption is not supported")
        || lower.contains("encrypted file is unsupported")
}

/// Check if bsdtar's stderr suggests the archive is damaged or not a RAR.
fn stderr_indicates_bad_archive(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("unrecognized archive format")
        || lower.contains("damaged")
        || lower.contains("truncated")
        || lower.contains("bad rar")
}

/// Check if a file path has an image extension we care about in comics.
///
/// Intentionally duplicated from `comic::is_image_file` so this module stays
/// self-contained and can be unit-tested without pulling in the whole comic
/// pipeline.
fn is_image_entry(name: &str) -> bool {
    let lower = name.to_lowercase();
    let ext = Path::new(&lower).extension().and_then(|e| e.to_str());
    matches!(
        ext,
        Some("jpg") | Some("jpeg") | Some("png") | Some("gif") | Some("webp") | Some("bmp") | Some("tiff") | Some("tif")
    )
}

/// Return true if this archive entry name is a ComicInfo.xml file (any path).
fn is_comic_info_entry(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "comicinfo.xml" || lower.ends_with("/comicinfo.xml")
}

/// Entry names we want to keep should be neither directories nor macOS cruft.
fn entry_is_noise(name: &str) -> bool {
    name.is_empty()
        || name.ends_with('/')
        || name.starts_with("__MACOSX")
        || name.contains("/.")
}

/// List entry names in a RAR archive via `bsdtar -tf`.
///
/// Returns the raw entry paths in their archive order. Filtering (images vs.
/// ComicInfo.xml vs. directories) is done by the caller.
fn list_entries(bsdtar: &Path, cbr_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let output = Command::new(bsdtar)
        .arg("-tf")
        .arg(cbr_path)
        .output()
        .map_err(|e| format!("failed to invoke bsdtar: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        if stderr_indicates_encryption(&stderr) {
            return Err(
                "encrypted CBRs are not supported (header-encrypted RAR \
                 archive). Remove the password with an external tool first."
                    .into(),
            );
        }
        if stderr_indicates_bad_archive(&stderr) {
            return Err(format!("corrupted or unreadable CBR archive: {}", stderr.trim()).into());
        }
        return Err(format!("bsdtar failed to list CBR contents: {}", stderr.trim()).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Extract a single entry by name to a file on disk, streaming through
/// `bsdtar -xOf <archive> <entry>`.
fn extract_entry_to_file(
    bsdtar: &Path,
    cbr_path: &Path,
    entry_name: &str,
    out_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(bsdtar)
        .arg("-xOf")
        .arg(cbr_path)
        .arg(entry_name)
        .output()
        .map_err(|e| format!("failed to invoke bsdtar: {}", e))?;

    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        if stderr_indicates_encryption(&stderr) {
            return Err(
                "encrypted CBRs are not supported (header-encrypted RAR \
                 archive). Remove the password with an external tool first."
                    .into(),
            );
        }
        return Err(format!(
            "bsdtar failed to extract '{}': {}",
            entry_name,
            stderr.trim()
        )
        .into());
    }

    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out_path, &output.stdout)?;
    Ok(())
}

/// Extract images (and any ComicInfo.xml) from a CBR file into a sibling
/// temp directory next to the archive.
///
/// Returns `(sorted_image_paths, extraction_dir)`. The caller is responsible
/// for cleaning up the extraction dir, mirroring `comic::extract_cbz`.
pub fn extract_cbr(cbr_path: &Path) -> Result<(Vec<PathBuf>, PathBuf), Box<dyn std::error::Error>> {
    let bsdtar = find_bsdtar().ok_or_else(bsdtar_missing_err)?;

    // List entries up-front so we can detect encryption and empty archives
    // without littering the filesystem first.
    let raw_entries = list_entries(&bsdtar, cbr_path)?;

    let stem = cbr_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let parent = cbr_path.parent().unwrap_or_else(|| Path::new("."));
    let extract_dir = parent.join(format!(".kindling_cbr_{}", stem));

    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)?;
    }
    fs::create_dir_all(&extract_dir)?;

    // Scratch closure so we can clean up on any error after creating the dir.
    let extraction = (|| -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
        let mut image_paths: Vec<PathBuf> = Vec::new();

        for entry_name in &raw_entries {
            if entry_is_noise(entry_name) {
                continue;
            }

            // ComicInfo.xml at any depth: extract to the root of the temp dir
            // where find_and_parse_comic_info expects it.
            if is_comic_info_entry(entry_name) {
                let out_path = extract_dir.join("ComicInfo.xml");
                extract_entry_to_file(&bsdtar, cbr_path, entry_name, &out_path)?;
                continue;
            }

            if !is_image_entry(entry_name) {
                continue;
            }

            let out_path = extract_dir.join(entry_name);
            extract_entry_to_file(&bsdtar, cbr_path, entry_name, &out_path)?;
            image_paths.push(out_path);
        }

        // Natural sort by filename so `page_2.jpg` < `page_10.jpg`.
        image_paths.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));

        if image_paths.is_empty() {
            return Err("No image files found in CBR archive".into());
        }
        Ok(image_paths)
    })();

    match extraction {
        Ok(images) => Ok((images, extract_dir)),
        Err(e) => {
            let _ = fs::remove_dir_all(&extract_dir);
            Err(e)
        }
    }
}

// --------------------------------------------------------------------------
// Natural sort helpers (local copy to keep this module standalone).
// --------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NaturalSortPart {
    Number(u64),
    Text(String),
}

fn natural_sort_key(path: &Path) -> Vec<NaturalSortPart> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut parts = Vec::new();
    let mut current_num = String::new();
    let mut current_text = String::new();

    for ch in name.chars() {
        if ch.is_ascii_digit() {
            if !current_text.is_empty() {
                parts.push(NaturalSortPart::Text(current_text.to_lowercase()));
                current_text.clear();
            }
            current_num.push(ch);
        } else {
            if !current_num.is_empty() {
                parts.push(NaturalSortPart::Number(current_num.parse::<u64>().unwrap_or(0)));
                current_num.clear();
            }
            current_text.push(ch);
        }
    }
    if !current_num.is_empty() {
        parts.push(NaturalSortPart::Number(current_num.parse::<u64>().unwrap_or(0)));
    }
    if !current_text.is_empty() {
        parts.push(NaturalSortPart::Text(current_text.to_lowercase()));
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_image_entries() {
        assert!(is_image_entry("page_001.jpg"));
        assert!(is_image_entry("page_001.JPG"));
        assert!(is_image_entry("nested/path/001.png"));
        assert!(is_image_entry("001.webp"));
        assert!(!is_image_entry("ComicInfo.xml"));
        assert!(!is_image_entry("readme.txt"));
        assert!(!is_image_entry("folder/"));
    }

    #[test]
    fn classifies_comic_info_entries() {
        assert!(is_comic_info_entry("ComicInfo.xml"));
        assert!(is_comic_info_entry("comicinfo.xml"));
        assert!(is_comic_info_entry("COMICINFO.XML"));
        assert!(is_comic_info_entry("nested/ComicInfo.xml"));
        assert!(!is_comic_info_entry("notComicInfo.xml"));
    }

    #[test]
    fn ignores_noise_entries() {
        assert!(entry_is_noise(""));
        assert!(entry_is_noise("pages/"));
        assert!(entry_is_noise("__MACOSX/._page_001.jpg"));
        assert!(entry_is_noise("pages/.DS_Store"));
        assert!(!entry_is_noise("pages/page_001.jpg"));
    }

    #[test]
    fn natural_sort_orders_numerically() {
        let a = Path::new("page_2.jpg");
        let b = Path::new("page_10.jpg");
        assert!(natural_sort_key(a) < natural_sort_key(b));
    }

    #[test]
    fn stderr_encryption_detection() {
        assert!(stderr_indicates_encryption("bsdtar: Encryption is not supported"));
        assert!(stderr_indicates_encryption("ENCRYPTION IS NOT SUPPORTED"));
        assert!(!stderr_indicates_encryption("bsdtar: Unrecognized archive format"));
    }
}
