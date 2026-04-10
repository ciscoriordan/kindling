/// EPUB extraction support.
///
/// An EPUB is a ZIP archive containing an OPF file and HTML/CSS/image content.
/// This module extracts an EPUB to a temporary directory and locates the OPF
/// root file via META-INF/container.xml.

use quick_xml::events::Event;
use quick_xml::Reader;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Extract an EPUB file to a temporary directory and return the path to the OPF file.
///
/// The caller is responsible for cleaning up the temp directory after use.
pub fn extract_epub(epub_path: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let epub_path = epub_path
        .canonicalize()
        .unwrap_or_else(|_| epub_path.to_path_buf());

    // Create temp directory next to the EPUB file
    let stem = epub_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let parent = epub_path.parent().unwrap_or(Path::new("."));
    let temp_dir = parent.join(format!(".kindling_epub_{}", stem));

    // Clean up any previous extraction
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    // Open and extract the ZIP archive
    let file = fs::File::open(&epub_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        // Skip directories
        if name.ends_with('/') {
            let dir_path = temp_dir.join(&name);
            fs::create_dir_all(&dir_path)?;
            continue;
        }

        let out_path = temp_dir.join(&name);
        if let Some(parent_dir) = out_path.parent() {
            fs::create_dir_all(parent_dir)?;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        fs::write(&out_path, &buf)?;
    }

    // Find the OPF path from META-INF/container.xml
    let container_path = temp_dir.join("META-INF").join("container.xml");
    let opf_relative = if container_path.exists() {
        parse_container_xml(&container_path)?
    } else {
        // Fallback: look for any .opf file in the extracted directory
        find_opf_file(&temp_dir)?
    };

    let opf_path = temp_dir.join(&opf_relative);
    if !opf_path.exists() {
        return Err(format!(
            "OPF file not found at expected path: {}",
            opf_path.display()
        )
        .into());
    }

    eprintln!(
        "Extracted EPUB to {} (OPF: {})",
        temp_dir.display(),
        opf_relative
    );

    Ok((temp_dir, opf_path))
}

/// Parse META-INF/container.xml to find the rootfile (OPF) path.
fn parse_container_xml(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let mut reader = Reader::from_str(&content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                if name == "rootfile" || name.ends_with(":rootfile") {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"full-path" {
                            let path_str =
                                String::from_utf8_lossy(&attr.value).to_string();
                            return Ok(path_str);
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("Error parsing container.xml: {}", e).into()),
            _ => {}
        }
        buf.clear();
    }

    Err("No rootfile found in container.xml".into())
}

/// Fallback: recursively find a .opf file in the extracted directory.
fn find_opf_file(dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Ok(result) = find_opf_file(&path) {
                // Return path relative to the original dir
                let relative = path.join(result);
                return Ok(relative
                    .strip_prefix(dir)
                    .unwrap_or(&relative)
                    .to_string_lossy()
                    .to_string());
            }
        } else if let Some(ext) = path.extension() {
            if ext == "opf" {
                return Ok(path
                    .strip_prefix(dir)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string());
            }
        }
    }
    Err("No .opf file found in EPUB archive".into())
}

/// Create an EPUB (zip) from a directory of OPF/HTML/image files.
/// Returns the raw bytes of the EPUB zip.
pub fn create_epub_from_dir(dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Write;
    let buf = Vec::new();
    let cursor = std::io::Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Walk directory and add all files
    fn add_dir(zip: &mut zip::ZipWriter<std::io::Cursor<Vec<u8>>>, base: &Path, dir: &Path, options: zip::write::SimpleFileOptions) -> Result<(), Box<dyn std::error::Error>> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = path.strip_prefix(base)?.to_string_lossy().to_string();
            if path.is_dir() {
                add_dir(zip, base, &path, options)?;
            } else {
                zip.start_file(&name, options)?;
                let data = fs::read(&path)?;
                zip.write_all(&data)?;
            }
        }
        Ok(())
    }

    add_dir(&mut zip, dir, dir, options)?;
    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

/// Clean up the temporary extraction directory.
pub fn cleanup_temp_dir(temp_dir: &Path) {
    if temp_dir.exists() {
        if let Err(e) = fs::remove_dir_all(temp_dir) {
            eprintln!("Warning: failed to clean up temp dir {}: {}", temp_dir.display(), e);
        }
    }
}
