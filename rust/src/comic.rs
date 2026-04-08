/// Comic book to MOBI converter.
///
/// Converts image folders, CBZ files, or CBR files into Kindle-optimized
/// MOBI files using a fixed-layout EPUB intermediate representation.
///
/// Pipeline:
///   1. Extract/scan images from input (folder, CBZ)
///   2. Process images in parallel (resize, grayscale, JPEG encode)
///   3. Write processed images + OPF + XHTML to a temp directory
///   4. Call mobi::build_mobi() on the temp directory's OPF
///   5. Clean up temp dir

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use rayon::prelude::*;

use crate::mobi;

/// Device profile for Kindle screen dimensions.
#[derive(Debug, Clone, Copy)]
pub struct DeviceProfile {
    pub width: u32,
    pub height: u32,
    pub grayscale: bool,
    pub name: &'static str,
}

/// All supported device profiles.
const PROFILES: &[DeviceProfile] = &[
    DeviceProfile { width: 1072, height: 1448, grayscale: true, name: "paperwhite" },
    DeviceProfile { width: 1264, height: 1680, grayscale: true, name: "oasis" },
    DeviceProfile { width: 1860, height: 2480, grayscale: true, name: "scribe" },
    DeviceProfile { width: 1072, height: 1448, grayscale: true, name: "basic" },
    DeviceProfile { width: 1264, height: 1680, grayscale: false, name: "colorsoft" },
    DeviceProfile { width: 1200, height: 1920, grayscale: false, name: "fire-hd-10" },
];

/// Look up a device profile by name (case-insensitive).
pub fn get_profile(name: &str) -> Option<DeviceProfile> {
    let lower = name.to_lowercase();
    PROFILES.iter().find(|p| p.name == lower).copied()
}

/// Return a comma-separated list of valid device names.
pub fn valid_device_names() -> String {
    PROFILES.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
}

/// A single processed page image (JPEG bytes, ready for embedding).
struct ProcessedImage {
    /// 0-based page index
    index: usize,
    /// JPEG-encoded image bytes
    jpeg_data: Vec<u8>,
}

/// Run the full comic-to-MOBI pipeline.
pub fn build_comic(
    input: &Path,
    output: &Path,
    profile: &DeviceProfile,
) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Collect source images (and optionally a temp dir from CBZ extraction)
    let (source_images, cbz_temp_dir) = collect_images(input)?;
    if source_images.is_empty() {
        return Err("No images found in input".into());
    }
    eprintln!("Found {} images", source_images.len());

    // Step 2: Process images in parallel
    eprintln!("Processing images for {} ({}x{}, {})...",
        profile.name, profile.width, profile.height,
        if profile.grayscale { "grayscale" } else { "color" });

    let total = source_images.len();
    let processed: Vec<ProcessedImage> = source_images
        .par_iter()
        .enumerate()
        .map(|(idx, img_path)| {
            if idx % 10 == 0 || idx == total - 1 {
                eprintln!("Processing image {}/{}...", idx + 1, total);
            }
            let jpeg_data = process_image(img_path, profile)
                .unwrap_or_else(|e| {
                    eprintln!("Warning: failed to process {}: {}", img_path.display(), e);
                    // Return a tiny 1x1 white JPEG as fallback
                    create_fallback_jpeg()
                });
            ProcessedImage { index: idx, jpeg_data }
        })
        .collect();

    // Sort by index (rayon may return out of order)
    let mut processed = processed;
    processed.sort_by_key(|p| p.index);

    let total_image_bytes: usize = processed.iter().map(|p| p.jpeg_data.len()).sum();
    eprintln!("Processed {} images ({:.1} MB total JPEG data)",
        processed.len(),
        total_image_bytes as f64 / (1024.0 * 1024.0));

    // Step 3: Write OPF + XHTML + images to temp directory
    let temp_dir = create_temp_dir(output)?;
    let opf_path = write_fixed_layout_epub(&temp_dir, &processed, profile)?;

    // Step 4: Build MOBI
    eprintln!("Building MOBI...");
    let result = mobi::build_mobi(
        &opf_path,
        output,
        false,  // compress
        false,  // headwords_only (N/A for books)
        None,   // no SRCS embedding
        false,  // no CMET
        false,  // allow HD images
        true,   // creator_tag = kindling
    );

    // Step 5: Clean up temp dirs
    if temp_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&temp_dir) {
            eprintln!("Warning: failed to clean up temp dir {}: {}", temp_dir.display(), e);
        }
    }
    if let Some(cbz_dir) = cbz_temp_dir {
        if cbz_dir.exists() {
            if let Err(e) = fs::remove_dir_all(&cbz_dir) {
                eprintln!("Warning: failed to clean up CBZ extraction dir {}: {}", cbz_dir.display(), e);
            }
        }
    }

    result
}

/// Collect image file paths from input (folder or CBZ).
///
/// Returns (image_paths, optional_cbz_temp_dir). The temp dir, if present,
/// should be cleaned up by the caller after processing.
fn collect_images(input: &Path) -> Result<(Vec<PathBuf>, Option<PathBuf>), Box<dyn std::error::Error>> {
    if input.is_dir() {
        Ok((collect_images_from_dir(input)?, None))
    } else if let Some(ext) = input.extension() {
        let ext_lower = ext.to_string_lossy().to_lowercase();
        match ext_lower.as_str() {
            "cbz" | "zip" => {
                let (images, temp_dir) = extract_cbz(input)?;
                Ok((images, Some(temp_dir)))
            }
            "cbr" | "rar" => Err("CBR (RAR) files are not supported directly. Please convert to CBZ first using:\n  unrar x input.cbr temp_dir/ && cd temp_dir && zip -r output.cbz .".into()),
            "pdf" => Err("PDF support coming soon".into()),
            _ => Err(format!("Unsupported input format: .{}", ext_lower).into()),
        }
    } else {
        Err("Cannot determine input type (not a directory and has no extension)".into())
    }
}

/// Scan a directory for image files, sorted naturally by filename.
fn collect_images_from_dir(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut images: Vec<PathBuf> = Vec::new();

    // Collect from this directory only (not recursive into subdirectories initially)
    // but if no images found at top level, try one level of subdirectories
    collect_images_recursive(dir, &mut images)?;

    if images.is_empty() {
        return Err(format!("No image files found in {}", dir.display()).into());
    }

    // Natural sort: sort by filename with numeric portions sorted numerically
    images.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));

    Ok(images)
}

/// Recursively collect image files from a directory.
fn collect_images_recursive(dir: &Path, images: &mut Vec<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries: Vec<_> = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_images_recursive(&path, images)?;
        } else if is_image_file(&path) {
            images.push(path);
        }
    }
    Ok(())
}

/// Check if a file path has an image extension.
fn is_image_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let lower = ext.to_lowercase();
            matches!(lower.as_str(), "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "tiff" | "tif")
        }
        None => false,
    }
}

/// Generate a natural sort key: split filename into text/numeric segments.
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum NaturalSortPart {
    // Number sorts before Text at the same position
    Number(u64),
    Text(String),
}

/// Extract images from a CBZ (ZIP) file to a temp directory, then collect paths.
///
/// Returns (image_paths, temp_extraction_dir).
fn extract_cbz(cbz_path: &Path) -> Result<(Vec<PathBuf>, PathBuf), Box<dyn std::error::Error>> {
    let file = fs::File::open(cbz_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let stem = cbz_path.file_stem().unwrap_or_default().to_string_lossy();
    let parent = cbz_path.parent().unwrap_or(Path::new("."));
    let extract_dir = parent.join(format!(".kindling_cbz_{}", stem));

    if extract_dir.exists() {
        fs::remove_dir_all(&extract_dir)?;
    }
    fs::create_dir_all(&extract_dir)?;

    let mut image_paths: Vec<PathBuf> = Vec::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();

        // Skip directories and hidden files (like __MACOSX)
        if name.ends_with('/') || name.starts_with("__MACOSX") || name.contains("/.") {
            continue;
        }

        let out_path = extract_dir.join(&name);

        // Check if this is an image file before extracting
        if !is_image_file(Path::new(&name)) {
            continue;
        }

        if let Some(parent_dir) = out_path.parent() {
            fs::create_dir_all(parent_dir)?;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        fs::write(&out_path, &buf)?;
        image_paths.push(out_path);
    }

    // Natural sort
    image_paths.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));

    if image_paths.is_empty() {
        // Clean up the empty extraction dir
        let _ = fs::remove_dir_all(&extract_dir);
        return Err("No image files found in CBZ archive".into());
    }

    Ok((image_paths, extract_dir))
}

/// Process a single image: load, resize, optionally convert to grayscale, encode as JPEG.
fn process_image(path: &Path, profile: &DeviceProfile) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let img = image::open(path)?;

    // Resize to fit device dimensions while maintaining aspect ratio
    let img = img.resize(profile.width, profile.height, FilterType::Lanczos3);

    // Convert to grayscale if the device profile requires it
    let img = if profile.grayscale {
        image::DynamicImage::ImageLuma8(img.to_luma8())
    } else {
        img
    };

    // Encode as JPEG
    let mut jpeg_buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut jpeg_buf);
    img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;

    Ok(jpeg_buf)
}

/// Create a minimal 1x1 white JPEG as a fallback for failed image processing.
fn create_fallback_jpeg() -> Vec<u8> {
    let img = image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(1, 1, image::Luma([255])));
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    img.write_to(&mut cursor, image::ImageFormat::Jpeg).unwrap();
    buf
}

/// Create a temporary directory for the fixed-layout EPUB content.
fn create_temp_dir(output: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
    let parent = output.parent().unwrap_or(Path::new("."));
    let temp_dir = parent.join(format!(".kindling_comic_{}", stem));

    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    Ok(temp_dir)
}

/// Write a fixed-layout EPUB structure to a temp directory.
///
/// Creates:
///   content.opf   - OPF manifest with fixed-layout metadata
///   page_NNN.xhtml - One XHTML file per page
///   images/page_NNN.jpg - Processed JPEG images
///
/// Returns the path to the OPF file.
fn write_fixed_layout_epub(
    temp_dir: &Path,
    pages: &[ProcessedImage],
    profile: &DeviceProfile,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let images_dir = temp_dir.join("images");
    fs::create_dir_all(&images_dir)?;

    // Write image files
    for page in pages {
        let filename = format!("page_{:04}.jpg", page.index);
        fs::write(images_dir.join(&filename), &page.jpeg_data)?;
    }

    // Write XHTML pages
    for page in pages {
        let xhtml = build_page_xhtml(page.index, profile);
        let filename = format!("page_{:04}.xhtml", page.index);
        fs::write(temp_dir.join(&filename), xhtml.as_bytes())?;
    }

    // Write CSS
    let css = build_comic_css();
    fs::write(temp_dir.join("comic.css"), css.as_bytes())?;

    // Write OPF
    let opf = build_comic_opf(pages.len(), profile);
    let opf_path = temp_dir.join("content.opf");
    fs::write(&opf_path, opf.as_bytes())?;

    // Write NCX
    let ncx = build_comic_ncx(pages.len());
    fs::write(temp_dir.join("toc.ncx"), ncx.as_bytes())?;

    Ok(opf_path)
}

/// Build the OPF manifest for the comic.
fn build_comic_opf(num_pages: usize, profile: &DeviceProfile) -> String {
    let mut manifest_items = String::new();
    let mut spine_items = String::new();

    // NCX
    manifest_items.push_str("    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");

    // CSS
    manifest_items.push_str("    <item id=\"css\" href=\"comic.css\" media-type=\"text/css\"/>\n");

    for i in 0..num_pages {
        // XHTML page
        manifest_items.push_str(&format!(
            "    <item id=\"page{:04}\" href=\"page_{:04}.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
            i, i
        ));
        // Image
        manifest_items.push_str(&format!(
            "    <item id=\"img{:04}\" href=\"images/page_{:04}.jpg\" media-type=\"image/jpeg\"/>\n",
            i, i
        ));
        // Spine ref
        spine_items.push_str(&format!(
            "    <itemref idref=\"page{:04}\"/>\n",
            i
        ));
    }

    // Mark first image as cover
    let cover_meta = if num_pages > 0 {
        "  <meta name=\"cover\" content=\"img0000\"/>\n"
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Comic</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="uid">kindling-comic-{timestamp}</dc:identifier>
    <meta name="fixed-layout" content="true"/>
    <meta name="original-resolution" content="{width}x{height}"/>
    <meta property="rendition:layout">pre-paginated</meta>
    <meta property="rendition:orientation">auto</meta>
{cover_meta}  </metadata>
  <manifest>
{manifest_items}  </manifest>
  <spine toc="ncx" page-progression-direction="ltr">
{spine_items}  </spine>
</package>
"#,
        timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        width = profile.width,
        height = profile.height,
        cover_meta = cover_meta,
        manifest_items = manifest_items,
        spine_items = spine_items,
    )
}

/// Build an XHTML page for a single comic page.
fn build_page_xhtml(page_index: usize, profile: &DeviceProfile) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
  <meta charset="UTF-8"/>
  <meta name="viewport" content="width={width}, height={height}"/>
  <link rel="stylesheet" type="text/css" href="comic.css"/>
  <title>Page {page_num}</title>
</head>
<body>
  <div class="page">
    <img src="images/page_{index:04}.jpg" alt="Page {page_num}"/>
  </div>
</body>
</html>
"#,
        width = profile.width,
        height = profile.height,
        page_num = page_index + 1,
        index = page_index,
    )
}

/// Build the CSS for full-bleed comic pages.
fn build_comic_css() -> String {
    r#"html, body {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
}
.page {
  width: 100%;
  height: 100%;
  text-align: center;
}
.page img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
"#.to_string()
}

/// Build a minimal NCX table of contents.
fn build_comic_ncx(num_pages: usize) -> String {
    let mut nav_points = String::new();
    for i in 0..num_pages {
        nav_points.push_str(&format!(
            r#"    <navPoint id="page{index:04}" playOrder="{order}">
      <navLabel><text>Page {page_num}</text></navLabel>
      <content src="page_{index:04}.xhtml"/>
    </navPoint>
"#,
            index = i,
            order = i + 1,
            page_num = i + 1,
        ));
    }

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head>
    <meta name="dtb:uid" content="kindling-comic"/>
    <meta name="dtb:depth" content="1"/>
    <meta name="dtb:totalPageCount" content="{num_pages}"/>
    <meta name="dtb:maxPageNumber" content="{num_pages}"/>
  </head>
  <docTitle><text>Comic</text></docTitle>
  <navMap>
{nav_points}  </navMap>
</ncx>
"#,
        num_pages = num_pages,
        nav_points = nav_points,
    )
}
