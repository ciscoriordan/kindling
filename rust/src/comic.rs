/// Comic book to MOBI converter.
///
/// Converts image folders, CBZ files, or CBR files into Kindle-optimized
/// MOBI files using a fixed-layout EPUB intermediate representation.
///
/// Pipeline:
///   1. Extract/scan images from input (folder, CBZ)
///   2. Parse ComicInfo.xml if present (metadata, manga detection)
///   3. Process images in parallel:
///      a. Detect and split double-page spreads (landscape images)
///      b. Crop uniform-color borders
///      c. Apply auto-contrast and gamma correction (grayscale devices only)
///      d. Resize and encode as JPEG
///   4. Write processed images + OPF + XHTML to a temp directory
///   5. Call mobi::build_mobi() on the temp directory's OPF
///   6. Clean up temp dir

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::epub;

use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, GrayImage, Luma, Rgb, RgbImage};
use rayon::prelude::*;

use crate::mobi;
use crate::moire;

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
    DeviceProfile { width: 1236, height: 1648, grayscale: true, name: "kpw5" },
    DeviceProfile { width: 1986, height: 2648, grayscale: true, name: "scribe2025" },
    DeviceProfile { width: 1240, height: 1860, grayscale: true, name: "kindle2024" },
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

/// Specifies how the cover image should be chosen.
#[derive(Debug, Clone)]
pub enum CoverSource {
    /// Use a specific page number (1-based) as the cover.
    PageNumber(usize),
    /// Use an external image file as the cover.
    FilePath(PathBuf),
}

/// Options controlling comic image processing.
#[derive(Debug, Clone)]
pub struct ComicOptions {
    /// Enable RTL (right-to-left) reading direction for manga.
    pub rtl: bool,
    /// Enable double-page spread splitting (default: true).
    pub split: bool,
    /// Enable border/margin cropping (default: true).
    pub crop: bool,
    /// Enable auto-contrast and gamma correction (default: true).
    pub enhance: bool,
    /// Force webtoon mode (vertical strip merge + split).
    pub webtoon: bool,
    /// Enable Kindle Panel View (tap-to-zoom panels). Default: true for comics.
    pub panel_view: bool,
    /// JPEG encoding quality (1-100). Default: 85.
    pub jpeg_quality: u8,
    /// Maximum pixel height for webtoon strip merges before chunking. Default: 65536.
    pub max_height: u32,
    /// Embed the generated EPUB in the MOBI (for Kindle Previewer compat). Default: true.
    pub embed_source: bool,
    /// Document type for EXTH 501: "EBOK" (Books shelf) or "PDOC" (Documents shelf).
    /// Default: None (PDOC).
    pub doc_type: Option<String>,
    /// Override title from ComicInfo.xml.
    pub title_override: Option<String>,
    /// Override author from ComicInfo.xml.
    pub author_override: Option<String>,
    /// Language code for OPF metadata (e.g. "ja", "en", "ko").
    pub language: Option<String>,
    /// Override cover image selection.
    pub cover: Option<CoverSource>,
    /// Rotate double-page spreads 90 degrees clockwise instead of splitting them.
    /// Useful for tablet users who want a full-page spread view.
    pub rotate_spreads: bool,
    /// Panel reading order for Panel View. Controls the order panels are
    /// navigated when tapping on Kindle.
    /// - "horizontal-lr": left-to-right, top-to-bottom (Western comics)
    /// - "horizontal-rl": right-to-left, top-to-bottom (manga)
    /// - "vertical-lr": top-to-bottom, left-to-right (4-koma, vertical strips)
    /// - "vertical-rl": top-to-bottom, right-to-left (4-koma RTL)
    /// None means auto-detect: horizontal-rl if RTL, horizontal-lr otherwise.
    pub panel_reading_order: Option<String>,
    /// Center-crop the cover image to fill the device screen (no borders).
    pub cover_fill: bool,
}

impl Default for ComicOptions {
    fn default() -> Self {
        ComicOptions {
            rtl: false,
            split: true,
            crop: true,
            enhance: true,
            webtoon: false,
            panel_view: true,
            jpeg_quality: 85,
            max_height: 65536,
            embed_source: true,
            doc_type: None,
            title_override: None,
            author_override: None,
            language: None,
            cover: None,
            rotate_spreads: false,
            panel_reading_order: None,
            cover_fill: false,
        }
    }
}

/// Metadata parsed from a ComicInfo.xml file.
#[derive(Debug, Clone, Default)]
pub struct ComicMetadata {
    pub title: Option<String>,
    pub series: Option<String>,
    pub number: Option<String>,
    pub writers: Vec<String>,
    pub pencillers: Vec<String>,
    pub inkers: Vec<String>,
    pub summary: Option<String>,
    pub manga_rtl: bool,
}

impl ComicMetadata {
    /// Build an effective title from series/number/title fields.
    pub fn effective_title(&self) -> Option<String> {
        match (&self.series, &self.number, &self.title) {
            (Some(series), Some(num), Some(title)) => {
                Some(format!("{} #{} - {}", series, num, title))
            }
            (Some(series), Some(num), None) => Some(format!("{} #{}", series, num)),
            (Some(series), None, Some(title)) => Some(format!("{} - {}", series, title)),
            (None, _, Some(title)) => Some(title.clone()),
            _ => self.series.clone(),
        }
    }

    /// Collect all creators into a single string.
    pub fn creators(&self) -> Vec<String> {
        let mut all = Vec::new();
        all.extend(self.writers.iter().cloned());
        all.extend(self.pencillers.iter().cloned());
        all.extend(self.inkers.iter().cloned());
        all
    }
}

/// A single processed page image (JPEG bytes, ready for embedding).
struct ProcessedImage {
    /// 0-based page index
    index: usize,
    /// JPEG-encoded image bytes
    jpeg_data: Vec<u8>,
    /// Panel rectangles for Panel View (None = no panels / full-page splash)
    panels: Option<Vec<PanelRect>>,
}

/// Run the full comic-to-MOBI pipeline.
pub fn build_comic(
    input: &Path,
    output: &Path,
    profile: &DeviceProfile,
) -> Result<(), Box<dyn std::error::Error>> {
    build_comic_with_options(input, output, profile, &ComicOptions::default())
}

/// Run the full comic-to-MOBI pipeline with processing options.
pub fn build_comic_with_options(
    input: &Path,
    output: &Path,
    profile: &DeviceProfile,
    options: &ComicOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    // Step 1: Collect source images (and optionally a temp dir from CBZ extraction)
    let (source_images, cbz_temp_dir) = collect_images(input)?;
    if source_images.is_empty() {
        return Err("No images found in input".into());
    }
    eprintln!("Found {} images", source_images.len());

    // Step 1b: Webtoon detection and preprocessing
    let is_webtoon = options.webtoon || detect_webtoon(&source_images);
    let source_images = if is_webtoon {
        if options.webtoon {
            eprintln!("Webtoon mode enabled");
        } else {
            eprintln!("Detected webtoon format");
        }
        let pages = webtoon_preprocess(&source_images, profile, options)?;
        eprintln!("Split webtoon into {} pages", pages.len());
        pages
    } else {
        source_images
    };

    // Step 2: Parse ComicInfo.xml if present
    let metadata = find_and_parse_comic_info(input, cbz_temp_dir.as_deref());

    // Determine effective RTL setting: CLI flag OR ComicInfo.xml manga detection
    let rtl = options.rtl || metadata.as_ref().map_or(false, |m| m.manga_rtl);
    if rtl {
        eprintln!("RTL (manga) mode enabled");
    }

    // Step 3: Process images in parallel (with spread splitting, cropping, enhancement)
    eprintln!("Processing images for {} ({}x{}, {})...",
        profile.name, profile.width, profile.height,
        if profile.grayscale { "grayscale" } else { "color" });

    // Determine which source image index is the cover page (for cover_fill).
    // For external file covers, cover_fill is applied separately below.
    let cover_source_idx = match &options.cover {
        Some(CoverSource::PageNumber(n)) => Some(n.saturating_sub(1)),
        Some(CoverSource::FilePath(_)) => None, // handled separately
        None => Some(0),
    };

    let total = source_images.len();
    let processed_groups: Vec<Option<(usize, Vec<Vec<u8>>)>> = source_images
        .par_iter()
        .enumerate()
        .map(|(idx, img_path)| {
            if idx % 10 == 0 || idx == total - 1 {
                eprintln!("Processing image {}/{}...", idx + 1, total);
            }
            let is_cover = options.cover_fill && cover_source_idx == Some(idx);
            match process_image_pipeline(img_path, profile, options, is_cover) {
                Ok(jpeg_pages) => Some((idx, jpeg_pages)),
                Err(e) => {
                    eprintln!("Warning: skipping {} ({})", img_path.display(), e);
                    None
                }
            }
        })
        .collect();

    // Filter out skipped images and unwrap
    let processed_groups: Vec<(usize, Vec<Vec<u8>>)> = processed_groups.into_iter().flatten().collect();

    if processed_groups.is_empty() {
        return Err("All images failed to load - no valid images to process".into());
    }

    // Sort by original index, then flatten (each source image may produce 1 or 2 pages)
    let mut processed_groups = processed_groups;
    processed_groups.sort_by_key(|(idx, _)| *idx);

    let mut processed: Vec<ProcessedImage> = Vec::new();
    let mut page_idx = 0;
    for (_orig_idx, pages) in &processed_groups {
        for jpeg_data in pages {
            processed.push(ProcessedImage {
                index: page_idx,
                jpeg_data: jpeg_data.clone(),
                panels: None, // Filled in below if panel_view is enabled
            });
            page_idx += 1;
        }
    }

    // Reverse page order for RTL reading direction
    if rtl {
        processed.reverse();
        // Re-index after reversal
        for (i, page) in processed.iter_mut().enumerate() {
            page.index = i;
        }
    }

    let total_image_bytes: usize = processed.iter().map(|p| p.jpeg_data.len()).sum();
    eprintln!("Processed into {} pages ({:.1} MB total JPEG data)",
        processed.len(),
        total_image_bytes as f64 / (1024.0 * 1024.0));

    // Step 3b: Detect panels for Panel View if enabled
    if options.panel_view {
        eprintln!("Detecting panels for Panel View...");
        let panel_results = detect_panels_for_pages(&processed);
        let mut panel_count = 0;
        for (i, panels) in panel_results.into_iter().enumerate() {
            if let Some(ref p) = panels {
                panel_count += 1;
                eprintln!("  Page {}: {} panels", i + 1, p.len());
            }
            processed[i].panels = panels;
        }
        eprintln!("Panel View: detected panels on {}/{} pages", panel_count, processed.len());

        // Sort panels according to reading order
        let reading_order = resolve_panel_reading_order(
            options.panel_reading_order.as_deref(), options.rtl,
        );
        if reading_order != "horizontal-lr" {
            eprintln!("Panel View: sorting panels in {} order", reading_order);
        }
        for page in processed.iter_mut() {
            if let Some(ref mut panels) = page.panels {
                sort_panels_by_reading_order(panels, reading_order);
            }
        }
    }

    // Step 4: Write OPF + XHTML + images to temp directory
    let temp_dir = create_temp_dir(output)?;
    let opf_path = write_fixed_layout_epub_v2(
        &temp_dir, &processed, profile, rtl, metadata.as_ref(), options.panel_view, options,
    )?;

    // Step 5: Build MOBI
    eprintln!("Building MOBI...");
    // If embed_source, zip the temp dir into an EPUB for SRCS embedding
    let srcs_data = if options.embed_source {
        epub::create_epub_from_dir(&temp_dir).ok()
    } else {
        None
    };
    let result = mobi::build_mobi(
        &opf_path,
        output,
        false,  // compress
        false,  // headwords_only (N/A for books)
        srcs_data.as_deref(),
        false,  // no CMET
        false,  // allow HD images
        false,  // default creator identity
        false,  // dual format (not KF8-only)
        options.doc_type.as_deref(),
    );

    // Step 6: Clean up temp dirs
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

/// Collect image file paths from input (folder, CBZ, or EPUB).
///
/// Returns (image_paths, optional_temp_dir). The temp dir, if present,
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
            "epub" => {
                let (images, temp_dir) = extract_epub_images(input)?;
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

/// Extract images from an EPUB file in spine order.
///
/// Uses epub::extract_epub() to unpack the archive, then parses the OPF to get
/// spine ordering. For each spine item (XHTML file), finds referenced images
/// via `<img src="...">` or `<svg><image xlink:href="...">` tags. Returns images
/// in spine order, which is critical for correct page ordering in manga/comics.
///
/// Returns (image_paths, temp_extraction_dir).
fn extract_epub_images(epub_path: &Path) -> Result<(Vec<PathBuf>, PathBuf), Box<dyn std::error::Error>> {
    use crate::opf::OPFData;

    let (temp_dir, opf_path) = epub::extract_epub(epub_path)?;

    let opf = OPFData::parse(&opf_path)?;
    let opf_dir = opf_path.parent().unwrap_or(Path::new("."));

    // Collect images referenced in spine-ordered XHTML pages.
    // This preserves reading order rather than filesystem alphabetical order.
    let mut ordered_images: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (_id, href) in &opf.spine_items {
        let xhtml_path = opf_dir.join(href);
        if !xhtml_path.exists() {
            continue;
        }

        let xhtml_dir = xhtml_path.parent().unwrap_or(opf_dir);

        let content = match fs::read_to_string(&xhtml_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        // Extract image references from the XHTML content
        let image_refs = extract_image_refs_from_xhtml(&content);

        for img_ref in image_refs {
            // Resolve relative path against the XHTML file's directory
            let img_path = xhtml_dir.join(&img_ref);
            let img_path = img_path.canonicalize().unwrap_or(img_path);

            if img_path.exists() && is_image_file(&img_path) && seen.insert(img_path.clone()) {
                ordered_images.push(img_path);
            }
        }
    }

    // If spine parsing yielded no images, fall back to collecting all images
    // from the extracted directory in natural sort order
    if ordered_images.is_empty() {
        eprintln!("No images found via EPUB spine, falling back to directory scan");
        ordered_images = collect_images_from_dir(&temp_dir)?;
    }

    if ordered_images.is_empty() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err("No image files found in EPUB archive".into());
    }

    eprintln!("EPUB: found {} images in spine order", ordered_images.len());

    Ok((ordered_images, temp_dir))
}

/// Extract image file references from XHTML content.
///
/// Handles three common patterns found in fixed-layout EPUB comics:
///   1. `<img src="...">` - standard HTML image tags
///   2. `<image xlink:href="...">` or `<image href="...">` - SVG image elements
///   3. CSS background-image references (less common, but seen in some EPUBs)
pub fn extract_image_refs_from_xhtml(content: &str) -> Vec<String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut refs = Vec::new();
    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let tag_name = e.name();
                let tag = std::str::from_utf8(tag_name.as_ref()).unwrap_or("");
                // Strip namespace prefix if present (e.g., "xhtml:img" -> "img")
                let local = tag.rsplit(':').next().unwrap_or(tag);

                match local {
                    "img" => {
                        // <img src="..."/>
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"src" {
                                let src = String::from_utf8_lossy(&attr.value).to_string();
                                if !src.is_empty() {
                                    refs.push(src);
                                }
                            }
                        }
                    }
                    "image" => {
                        // <image xlink:href="..." /> or <image href="..."/>
                        for attr in e.attributes().flatten() {
                            let key = std::str::from_utf8(attr.key.as_ref()).unwrap_or("");
                            // Match "href", "xlink:href", or any ns-prefixed href
                            if key == "href" || key.ends_with(":href") {
                                let href = String::from_utf8_lossy(&attr.value).to_string();
                                if !href.is_empty() {
                                    refs.push(href);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => {
                // XHTML parsing failed; fall back to regex extraction
                return extract_image_refs_regex(content);
            }
            _ => {}
        }
        buf.clear();
    }

    // If XML parsing yielded nothing, try regex as fallback
    // (some EPUB XHTML is not well-formed XML)
    if refs.is_empty() {
        return extract_image_refs_regex(content);
    }

    refs
}

/// Regex fallback for extracting image references from XHTML.
///
/// Used when the XML parser fails (malformed XHTML) or finds no images.
pub fn extract_image_refs_regex(content: &str) -> Vec<String> {
    use regex::Regex;
    use std::sync::OnceLock;

    static IMG_SRC_RE: OnceLock<Regex> = OnceLock::new();
    static IMAGE_HREF_RE: OnceLock<Regex> = OnceLock::new();

    let img_re = IMG_SRC_RE.get_or_init(|| {
        Regex::new(r#"<img\s[^>]*src="([^"]+)""#).unwrap()
    });
    let image_re = IMAGE_HREF_RE.get_or_init(|| {
        Regex::new(r#"<image\s[^>]*(?:xlink:)?href="([^"]+)""#).unwrap()
    });

    let mut refs = Vec::new();
    for cap in img_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            refs.push(m.as_str().to_string());
        }
    }
    for cap in image_re.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            refs.push(m.as_str().to_string());
        }
    }
    refs
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

        // Also extract ComicInfo.xml if present
        let lower_name = name.to_lowercase();
        if lower_name == "comicinfo.xml" || lower_name.ends_with("/comicinfo.xml") {
            if let Some(parent_dir) = out_path.parent() {
                fs::create_dir_all(parent_dir)?;
            }
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            fs::write(&out_path, &buf)?;
            continue;
        }

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

/// Full image processing pipeline: split spreads, crop, enhance, resize, encode.
///
/// Returns one or two JPEG byte vectors (two if the image was a double-page spread
/// and splitting is enabled).
fn process_image_pipeline(
    path: &Path,
    profile: &DeviceProfile,
    options: &ComicOptions,
    cover_fill: bool,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let mut img = image::open(path)?;

    // Check for zero-dimension images
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err(format!("zero dimensions ({}x{})", w, h).into());
    }

    // Step 0: Moire correction for grayscale source images on color devices.
    // Color e-ink screens (Colorsoft, Fire) use a CFA overlay that produces
    // rainbow artifacts when displaying high-frequency grayscale content like
    // manga screentone. Apply a mild blur + unsharp mask to suppress this.
    if !profile.grayscale && is_grayscale_source(&img) {
        moire::remove_moire(&mut img);
    }

    // Step 1: Crop borders on the full image BEFORE splitting spreads.
    // Cropping the full spread first ensures both halves get symmetric treatment.
    // If we split first and crop each half independently, asymmetric borders on
    // the left vs. right half can produce mismatched page sizes.
    let img = if options.crop {
        crop_borders(&img)
    } else {
        img
    };

    // Step 2: Detect and split double-page spreads
    // Always split as [left, right] regardless of RTL. The global page reversal
    // in build_comic_with_options handles RTL ordering: reversing [left, right]
    // yields [right, left], which is the correct RTL reading order for a spread.
    // Swapping here AND reversing globally would cancel out, producing the wrong
    // cover image (left half instead of right half for RTL).
    //
    // When rotate_spreads is enabled, landscape spreads are rotated 90 degrees
    // clockwise instead of being split, producing a single portrait page that
    // preserves the full spread. This overrides the split behavior for spreads.
    let pages = if options.rotate_spreads && is_double_page_spread(&img) {
        vec![img.rotate90()]
    } else if options.split && is_double_page_spread(&img) {
        let (left, right) = split_spread(&img);
        vec![left, right]
    } else {
        vec![img]
    };

    // Step 3-4: Process each page (enhance, resize, encode)
    let mut results = Vec::new();
    for (page_idx, page) in pages.into_iter().enumerate() {
        let page = if options.enhance && profile.grayscale {
            enhance_image(&page)
        } else {
            page
        };

        // Cover fill: center-crop to device aspect ratio (first page only)
        let page = if cover_fill && page_idx == 0 {
            cover_fill_crop(&page, profile.width, profile.height)
        } else {
            page
        };

        // Resize to fit device dimensions while maintaining aspect ratio
        let page = page.resize(profile.width, profile.height, FilterType::Lanczos3);

        // Convert to grayscale if the device profile requires it
        let page = if profile.grayscale {
            DynamicImage::ImageLuma8(page.to_luma8())
        } else {
            page
        };

        // Encode as JPEG with the configured quality level
        let jpeg_buf = encode_jpeg(&page, options.jpeg_quality)?;
        results.push(jpeg_buf);
    }

    Ok(results)
}

/// Center-crop an image to match the target aspect ratio.
///
/// If the image is wider than the target ratio, trims left and right equally.
/// If the image is taller than the target ratio, trims top and bottom equally.
/// Returns the original image unchanged if it already matches the target ratio.
fn cover_fill_crop(img: &DynamicImage, target_width: u32, target_height: u32) -> DynamicImage {
    let (w, h) = img.dimensions();
    let target_ratio = target_width as f64 / target_height as f64;
    let img_ratio = w as f64 / h as f64;

    if (img_ratio - target_ratio).abs() < 0.001 {
        // Already matches the target aspect ratio
        return img.clone();
    }

    let (crop_w, crop_h) = if img_ratio > target_ratio {
        // Image is wider than target: crop horizontally (keep full height)
        let new_w = (h as f64 * target_ratio).round() as u32;
        (new_w.min(w), h)
    } else {
        // Image is taller than target: crop vertically (keep full width)
        let new_h = (w as f64 / target_ratio).round() as u32;
        (w, new_h.min(h))
    };

    let x = (w - crop_w) / 2;
    let y = (h - crop_h) / 2;

    image::imageops::crop_imm(img, x, y, crop_w, crop_h).to_image().into()
}

/// Encode a DynamicImage as JPEG with a specific quality level (1-100).
fn encode_jpeg(img: &DynamicImage, quality: u8) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut jpeg_buf = Vec::new();
    let cursor = std::io::Cursor::new(&mut jpeg_buf);
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(cursor, quality);
    img.write_with_encoder(encoder)?;
    Ok(jpeg_buf)
}

// ---------------------------------------------------------------------------
// Grayscale source detection (for moire correction)
// ---------------------------------------------------------------------------

/// Check if a source image is effectively grayscale.
///
/// Returns true if the image is a GrayImage, or if it's an RGB/RGBA image
/// where the color channels have very low variance (i.e. R ~= G ~= B for all
/// pixels). This catches grayscale manga/comics saved as RGB files.
fn is_grayscale_source(img: &DynamicImage) -> bool {
    match img {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageLuma16(_) => true,
        _ => {
            // Sample pixels to check if R ~= G ~= B across the image.
            // For efficiency, sample at most ~1000 pixels in a grid.
            let (w, h) = img.dimensions();
            if w == 0 || h == 0 {
                return true;
            }
            let rgb = img.to_rgb8();
            let step_x = (w / 32).max(1);
            let step_y = (h / 32).max(1);
            let mut max_channel_diff: u8 = 0;
            let mut x = 0;
            while x < w {
                let mut y = 0;
                while y < h {
                    let p = rgb.get_pixel(x, y).0;
                    let r = p[0];
                    let g = p[1];
                    let b = p[2];
                    let diff = (r.max(g).max(b)) - (r.min(g).min(b));
                    if diff > max_channel_diff {
                        max_channel_diff = diff;
                    }
                    // Early exit: if any sampled pixel has significant color,
                    // this is not a grayscale source.
                    if max_channel_diff > 10 {
                        return false;
                    }
                    y += step_y;
                }
                x += step_x;
            }
            // All sampled pixels had R ~= G ~= B within 10 levels
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Double-page spread detection and splitting
// ---------------------------------------------------------------------------

/// Detect whether an image is a double-page spread (landscape orientation).
pub fn is_double_page_spread(img: &DynamicImage) -> bool {
    let (w, h) = img.dimensions();
    w > h
}

/// Split a landscape image into left and right halves.
pub fn split_spread(img: &DynamicImage) -> (DynamicImage, DynamicImage) {
    let (w, h) = img.dimensions();
    let mid = w / 2;
    let left = img.crop_imm(0, 0, mid, h);
    let right = img.crop_imm(mid, 0, w - mid, h);
    (left, right)
}

// ---------------------------------------------------------------------------
// Border/margin cropping
// ---------------------------------------------------------------------------

/// Detect and crop uniform-color borders around an image.
///
/// Scans each edge inward looking for rows/columns whose average pixel value
/// is within a threshold of the edge pixel. Only crops if the border is at
/// least 2% of the image dimension to avoid false positives.
pub fn crop_borders(img: &DynamicImage) -> DynamicImage {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 10 || h < 10 {
        return img.clone();
    }

    let threshold: f64 = 20.0; // Pixel value tolerance for "same color as border"
    let min_border_frac: f64 = 0.02; // Minimum 2% of dimension to count as border

    // Detect top border
    let edge_top = row_average(&gray, 0);
    let mut top = 0u32;
    for y in 0..h {
        if (row_average(&gray, y) - edge_top).abs() > threshold {
            break;
        }
        top = y + 1;
    }
    if (top as f64) < (h as f64 * min_border_frac) {
        top = 0;
    }

    // Detect bottom border
    let edge_bottom = row_average(&gray, h - 1);
    let mut bottom = h;
    for y in (0..h).rev() {
        if (row_average(&gray, y) - edge_bottom).abs() > threshold {
            break;
        }
        bottom = y;
    }
    if ((h - bottom) as f64) < (h as f64 * min_border_frac) {
        bottom = h;
    }

    // Detect left border
    let edge_left = col_average(&gray, 0);
    let mut left = 0u32;
    for x in 0..w {
        if (col_average(&gray, x) - edge_left).abs() > threshold {
            break;
        }
        left = x + 1;
    }
    if (left as f64) < (w as f64 * min_border_frac) {
        left = 0;
    }

    // Detect right border
    let edge_right = col_average(&gray, w - 1);
    let mut right = w;
    for x in (0..w).rev() {
        if (col_average(&gray, x) - edge_right).abs() > threshold {
            break;
        }
        right = x;
    }
    if ((w - right) as f64) < (w as f64 * min_border_frac) {
        right = w;
    }

    // Ensure we have a valid crop region
    if left >= right || top >= bottom {
        return img.clone();
    }

    // Only crop if we actually detected borders
    if top == 0 && bottom == h && left == 0 && right == w {
        return img.clone();
    }

    img.crop_imm(left, top, right - left, bottom - top)
}

/// Average pixel value of a row in a grayscale image.
fn row_average(img: &GrayImage, y: u32) -> f64 {
    let w = img.width();
    let sum: f64 = (0..w).map(|x| img.get_pixel(x, y).0[0] as f64).sum();
    sum / w as f64
}

/// Average pixel value of a column in a grayscale image.
fn col_average(img: &GrayImage, x: u32) -> f64 {
    let h = img.height();
    let sum: f64 = (0..h).map(|y| img.get_pixel(x, y).0[0] as f64).sum();
    sum / h as f64
}

// ---------------------------------------------------------------------------
// Auto-contrast and gamma correction
// ---------------------------------------------------------------------------

/// Apply auto-contrast (histogram stretching) and gamma correction.
///
/// Auto-contrast clips 0.5% from each end of the histogram and stretches
/// the remaining range to 0-255. Gamma correction (0.8) darkens midtones
/// slightly for better e-ink readability.
pub fn enhance_image(img: &DynamicImage) -> DynamicImage {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    let total_pixels = (w * h) as f64;

    // Build histogram
    let mut histogram = [0u32; 256];
    for pixel in gray.pixels() {
        histogram[pixel.0[0] as usize] += 1;
    }

    // Find clip points at 0.5% from each end
    let clip_count = (total_pixels * 0.005) as u32;
    let mut low = 0u8;
    let mut cumulative = 0u32;
    for i in 0..256 {
        cumulative += histogram[i];
        if cumulative >= clip_count {
            low = i as u8;
            break;
        }
    }

    let mut high = 255u8;
    cumulative = 0;
    for i in (0..256).rev() {
        cumulative += histogram[i];
        if cumulative >= clip_count {
            high = i as u8;
            break;
        }
    }

    if high <= low {
        // Image is essentially uniform, nothing to enhance
        return img.clone();
    }

    // Build lookup table: auto-contrast + gamma
    let gamma: f64 = 0.8;
    let range = (high - low) as f64;
    let mut lut = [0u8; 256];
    for i in 0..256 {
        let clamped = (i as u8).max(low).min(high);
        let normalized = (clamped - low) as f64 / range; // 0.0 .. 1.0
        let gamma_corrected = normalized.powf(gamma);
        lut[i] = (gamma_corrected * 255.0).round().clamp(0.0, 255.0) as u8;
    }

    // Apply to all channels of the original image
    let (w, h) = img.dimensions();
    match img {
        DynamicImage::ImageLuma8(_) => {
            let mut out = GrayImage::new(w, h);
            for (x, y, pixel) in gray.enumerate_pixels() {
                out.put_pixel(x, y, Luma([lut[pixel.0[0] as usize]]));
            }
            DynamicImage::ImageLuma8(out)
        }
        _ => {
            // For color images, convert to grayscale for LUT, then apply to each channel
            let rgb = img.to_rgb8();
            let mut out = RgbImage::new(w, h);
            for (x, y, pixel) in rgb.enumerate_pixels() {
                out.put_pixel(x, y, Rgb([
                    lut[pixel.0[0] as usize],
                    lut[pixel.0[1] as usize],
                    lut[pixel.0[2] as usize],
                ]));
            }
            DynamicImage::ImageRgb8(out)
        }
    }
}

// ---------------------------------------------------------------------------
// Webtoon detection, merging, and splitting
// ---------------------------------------------------------------------------

/// Detect webtoon format: all input images have height > 3x width.
pub fn detect_webtoon(images: &[PathBuf]) -> bool {
    if images.is_empty() {
        return false;
    }
    images.iter().all(|path| {
        match image::image_dimensions(path) {
            Ok((w, h)) => h > 3 * w,
            Err(_) => false,
        }
    })
}

/// Full webtoon preprocessing: load images, merge into a tall strip, split at gutters.
///
/// If the merged strip would exceed `options.max_height`, the images are split
/// into chunks that each stay under the limit. Each chunk is merged and split
/// independently. This prevents OOM on massive webtoon directories.
///
/// Returns a list of temporary file paths for the split page images.
fn webtoon_preprocess(
    source_images: &[PathBuf],
    profile: &DeviceProfile,
    options: &ComicOptions,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    // Load all images, skipping corrupt/unreadable ones
    let mut images: Vec<DynamicImage> = Vec::new();
    for p in source_images {
        match image::open(p) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                if w == 0 || h == 0 {
                    eprintln!("Warning: skipping {} (zero dimensions {}x{})", p.display(), w, h);
                    continue;
                }
                images.push(img);
            }
            Err(e) => {
                eprintln!("Warning: skipping {} ({})", p.display(), e);
            }
        }
    }

    if images.is_empty() {
        return Err("All images failed to load - no valid images to process".into());
    }

    // Calculate total height to decide if we need to chunk
    let total_height: u32 = images.iter().map(|img| img.height()).sum();
    let max_height = options.max_height;

    let chunks: Vec<Vec<DynamicImage>> = if total_height > max_height {
        eprintln!(
            "Warning: merged strip height ({}) exceeds --max-height ({}), splitting into chunks",
            total_height, max_height
        );
        // Split images into chunks where each chunk's total height <= max_height
        let mut chunks = Vec::new();
        let mut current_chunk: Vec<DynamicImage> = Vec::new();
        let mut current_height: u32 = 0;

        for img in images {
            let h = img.height();
            if !current_chunk.is_empty() && current_height + h > max_height {
                chunks.push(std::mem::take(&mut current_chunk));
                current_height = 0;
            }
            current_height += h;
            current_chunk.push(img);
        }
        if !current_chunk.is_empty() {
            chunks.push(current_chunk);
        }
        eprintln!("Processing {} chunks", chunks.len());
        chunks
    } else {
        vec![images]
    };

    let temp_dir = std::env::temp_dir().join(format!(
        "kindling_webtoon_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    fs::create_dir_all(&temp_dir)?;

    let mut all_paths: Vec<PathBuf> = Vec::new();
    let mut page_offset = 0usize;

    for (chunk_idx, chunk_images) in chunks.iter().enumerate() {
        // Merge into a single tall strip
        let strip = webtoon_merge(chunk_images);
        let (strip_w, strip_h) = strip.dimensions();
        if chunks.len() > 1 {
            eprintln!("Chunk {}: merged strip {}x{}", chunk_idx + 1, strip_w, strip_h);
        } else {
            eprintln!("Merged webtoon strip: {}x{}", strip_w, strip_h);
        }

        // Split at gutters
        let pages = webtoon_split(&strip, profile.height);
        if chunks.len() > 1 {
            eprintln!("Chunk {}: split into {} pages", chunk_idx + 1, pages.len());
        } else {
            eprintln!("Split into {} page images", pages.len());
        }

        let offset = page_offset;
        let paths: Vec<PathBuf> = pages
            .into_par_iter()
            .enumerate()
            .map(|(i, page)| {
                let path = temp_dir.join(format!("page_{:04}.png", offset + i));
                page.save(&path).expect("Failed to save webtoon page");
                path
            })
            .collect();

        // Sort by filename to preserve order (par_iter may reorder)
        let mut paths = paths;
        paths.sort_by(|a, b| natural_sort_key(a).cmp(&natural_sort_key(b)));

        page_offset += paths.len();
        all_paths.extend(paths);
    }

    Ok(all_paths)
}

/// Merge multiple images vertically into one tall strip.
///
/// Images are stacked top-to-bottom. If widths differ, narrower images are
/// centered on a background color detected from the first image's edge pixels.
pub fn webtoon_merge(images: &[DynamicImage]) -> DynamicImage {
    if images.len() == 1 {
        return images[0].clone();
    }

    let max_width = images.iter().map(|img| img.width()).max().unwrap_or(0);
    let total_height: u32 = images.iter().map(|img| img.height()).sum();

    // Detect background color from the top-left corner of the first image
    let bg_color = detect_background_color(&images[0]);

    let mut canvas = RgbImage::from_pixel(max_width, total_height, bg_color);
    let mut y_offset = 0u32;

    for img in images {
        let rgb = img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        let x_offset = (max_width - w) / 2; // center narrower images

        for py in 0..h {
            for px in 0..w {
                canvas.put_pixel(x_offset + px, y_offset + py, *rgb.get_pixel(px, py));
            }
        }
        y_offset += h;
    }

    DynamicImage::ImageRgb8(canvas)
}

/// Detect the background color of an image from its edge pixels.
///
/// Samples the corners and edges to determine if the background is
/// predominantly white or black (or something else).
fn detect_background_color(img: &DynamicImage) -> Rgb<u8> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width(), rgb.height());
    if w == 0 || h == 0 {
        return Rgb([255, 255, 255]);
    }

    // Sample edge pixels: top row, bottom row, left column, right column
    let mut sum_r: u64 = 0;
    let mut sum_g: u64 = 0;
    let mut sum_b: u64 = 0;
    let mut count: u64 = 0;

    // Top row
    for x in 0..w {
        let p = rgb.get_pixel(x, 0);
        sum_r += p.0[0] as u64;
        sum_g += p.0[1] as u64;
        sum_b += p.0[2] as u64;
        count += 1;
    }
    // Bottom row
    for x in 0..w {
        let p = rgb.get_pixel(x, h - 1);
        sum_r += p.0[0] as u64;
        sum_g += p.0[1] as u64;
        sum_b += p.0[2] as u64;
        count += 1;
    }

    if count == 0 {
        return Rgb([255, 255, 255]);
    }

    let avg_r = (sum_r / count) as u8;
    let avg_g = (sum_g / count) as u8;
    let avg_b = (sum_b / count) as u8;

    // Classify: if average luminance is dark, use black; otherwise use white
    let luminance = (avg_r as u32 + avg_g as u32 + avg_b as u32) / 3;
    if luminance < 128 {
        Rgb([0, 0, 0])
    } else {
        Rgb([255, 255, 255])
    }
}

/// Split a tall strip image into device-height pages at natural gutters.
///
/// Searches for horizontal rows of low variance (gutters between panels)
/// within +/- 20% of the target height. When no good gutter is found,
/// splits at the target height with overlap: the bottom ~10% of each page
/// overlaps with the top of the next page, so no content is lost even if
/// the cut goes through a panel.
pub fn webtoon_split(strip: &DynamicImage, device_height: u32) -> Vec<DynamicImage> {
    let (w, h) = strip.dimensions();

    if h <= device_height {
        return vec![strip.clone()];
    }

    let gray = strip.to_luma8();
    let target = device_height;
    let margin = (target as f64 * 0.20) as u32;
    let overlap = (target as f64 * 0.10) as u32; // 10% overlap when no gutter found

    let mut pages = Vec::new();
    let mut y_start = 0u32;

    while y_start < h {
        let remaining = h - y_start;

        // If remaining content fits in one page (with some slack), take it all.
        // The slack accounts for overlap from the previous page: if the remaining
        // content is shorter than target + margin, just include it rather than
        // creating a tiny final page.
        if remaining <= target + margin {
            pages.push(strip.crop_imm(0, y_start, w, remaining));
            break;
        }

        // Search for best gutter in [target - margin, target + margin]
        let search_lo = target.saturating_sub(margin);
        let search_hi = (target + margin).min(remaining);

        let (best_y, gutter_found) = find_best_gutter(&gray, y_start, search_lo, search_hi, w);

        let cut_y = y_start + best_y;
        pages.push(strip.crop_imm(0, y_start, w, best_y));

        if gutter_found {
            // Clean gutter found - advance past it with no overlap
            y_start = cut_y;
        } else {
            // No good gutter - back up by the overlap amount so the next page
            // repeats the bottom portion of this page
            y_start = cut_y.saturating_sub(overlap);
        }
    }

    pages
}

/// Find the row with the lowest variance (best gutter) within a search range.
///
/// Scans rows from y_start + lo to y_start + hi and returns a tuple of
/// (offset_from_y_start, gutter_found). When `gutter_found` is false, the
/// returned offset is the target midpoint and the caller should apply
/// overlap to avoid cutting through content.
///
/// This works for both white gutters (all ~255) and dark/black gutters
/// (all ~0) since variance measures uniformity regardless of the mean
/// brightness.
///
/// To improve detection of gutters that span multiple consecutive rows (common
/// in webtoons with thick panel gaps), we average variance over a small window
/// of rows. This makes the detector more robust to single-row noise that might
/// appear inside a gutter band.
fn find_best_gutter(
    gray: &GrayImage,
    y_start: u32,
    lo: u32,
    hi: u32,
    width: u32,
) -> (u32, bool) {
    let target_mid = (lo + hi) / 2;
    let mut best_offset = target_mid;
    let mut best_score = f64::MAX;
    let img_height = gray.height();

    // Use a small window (5 rows) to average variance, making detection more
    // robust for thick gutters that may have slight noise in individual rows.
    let half_window: u32 = 2;

    for offset in lo..=hi {
        let y = y_start + offset;
        if y >= img_height {
            break;
        }

        // Average variance over a window of rows centered on y
        let win_lo = y.saturating_sub(half_window);
        let win_hi = (y + half_window + 1).min(img_height);
        let mut var_sum = 0.0;
        let mut count = 0u32;
        for wy in win_lo..win_hi {
            var_sum += row_variance(gray, wy, width);
            count += 1;
        }
        let avg_variance = if count > 0 { var_sum / count as f64 } else { f64::MAX };

        if avg_variance < best_score {
            best_score = avg_variance;
            best_offset = offset;
        }
    }

    // A "good" gutter has very low variance (near-uniform row).
    // Threshold: variance < 100.0 indicates a fairly uniform row.
    if best_score > 100.0 {
        // No good gutter found - return target midpoint with flag
        return (target_mid, false);
    }

    (best_offset, true)
}

/// Calculate the pixel variance of a single row.
fn row_variance(gray: &GrayImage, y: u32, width: u32) -> f64 {
    if width == 0 {
        return 0.0;
    }

    let mut sum: f64 = 0.0;
    let mut sum_sq: f64 = 0.0;

    for x in 0..width {
        let v = gray.get_pixel(x, y).0[0] as f64;
        sum += v;
        sum_sq += v * v;
    }

    let n = width as f64;
    let mean = sum / n;
    (sum_sq / n) - (mean * mean)
}

// ---------------------------------------------------------------------------
// Panel View: detection and markup
// ---------------------------------------------------------------------------

/// A rectangular panel region as percentages of the page dimensions.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelRect {
    /// Left edge as percentage (0.0 - 100.0)
    pub x: f64,
    /// Top edge as percentage (0.0 - 100.0)
    pub y: f64,
    /// Width as percentage (0.0 - 100.0)
    pub w: f64,
    /// Height as percentage (0.0 - 100.0)
    pub h: f64,
}

/// Detect panel boundaries in a comic page image.
///
/// Algorithm:
/// 1. Convert to grayscale
/// 2. Find horizontal gutters (rows of low variance spanning the image width)
/// 3. For each horizontal strip between gutters, find vertical gutters
/// 4. Each resulting rectangle is a panel
///
/// Returns an empty Vec if no panels are detected (e.g., full-page splash).
pub fn detect_panels(img: &DynamicImage) -> Vec<PanelRect> {
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 20 || h < 20 {
        return Vec::new();
    }

    let variance_threshold: f64 = 50.0;
    let min_gutter_height = ((h as f64) * 0.005).max(2.0) as u32; // 0.5% of height, min 2px
    let min_gutter_width = ((w as f64) * 0.005).max(2.0) as u32;  // 0.5% of width, min 2px

    // Step 1: Find horizontal gutters
    let h_gutters = find_horizontal_gutters(&gray, w, h, variance_threshold, min_gutter_height);

    // Build horizontal strip boundaries from gutters
    let h_strips = strips_from_gutters(&h_gutters, h);

    // If we only have one horizontal strip (no horizontal gutters found),
    // try vertical gutters across the entire image. If none found either, return empty.
    if h_strips.len() <= 1 {
        let v_gutters = find_vertical_gutters(&gray, 0, h, w, variance_threshold, min_gutter_width);
        let v_strips = strips_from_gutters(&v_gutters, w);
        if v_strips.len() <= 1 {
            // No panels detected - full page splash
            return Vec::new();
        }
        // Single row, multiple columns
        return v_strips
            .iter()
            .map(|&(x_start, x_end)| PanelRect {
                x: (x_start as f64 / w as f64) * 100.0,
                y: 0.0,
                w: ((x_end - x_start) as f64 / w as f64) * 100.0,
                h: 100.0,
            })
            .collect();
    }

    // Step 2: For each horizontal strip, find vertical gutters
    let mut panels = Vec::new();
    for &(y_start, y_end) in &h_strips {
        let v_gutters = find_vertical_gutters(
            &gray, y_start, y_end, w, variance_threshold, min_gutter_width,
        );
        let v_strips = strips_from_gutters(&v_gutters, w);

        for &(x_start, x_end) in &v_strips {
            panels.push(PanelRect {
                x: (x_start as f64 / w as f64) * 100.0,
                y: (y_start as f64 / h as f64) * 100.0,
                w: ((x_end - x_start) as f64 / w as f64) * 100.0,
                h: ((y_end - y_start) as f64 / h as f64) * 100.0,
            });
        }
    }

    // Only return panels if we found more than one
    if panels.len() <= 1 {
        return Vec::new();
    }

    panels
}

/// Resolve the effective panel reading order string.
///
/// If an explicit reading order is provided, returns it. Otherwise returns
/// "horizontal-rl" when RTL mode is active, or "horizontal-lr" as the default.
pub fn resolve_panel_reading_order(explicit: Option<&str>, rtl: bool) -> &'static str {
    match explicit {
        Some("horizontal-lr") => "horizontal-lr",
        Some("horizontal-rl") => "horizontal-rl",
        Some("vertical-lr") => "vertical-lr",
        Some("vertical-rl") => "vertical-rl",
        Some(other) => {
            eprintln!("Warning: unknown panel-reading-order '{}', using default", other);
            if rtl { "horizontal-rl" } else { "horizontal-lr" }
        }
        None => {
            if rtl { "horizontal-rl" } else { "horizontal-lr" }
        }
    }
}

/// Sort panels according to the specified reading order.
///
/// The panels produced by `detect_panels` are in row-major left-to-right order
/// (horizontal-lr). This function re-sorts them for other reading orders:
/// - "horizontal-lr": left-to-right, then top-to-bottom (no change needed)
/// - "horizontal-rl": right-to-left, then top-to-bottom
/// - "vertical-lr": top-to-bottom, then left-to-right
/// - "vertical-rl": top-to-bottom, then right-to-left
///
/// Panels are grouped into rows/columns using a tolerance of 5% to account
/// for slight misalignment in detected panel edges.
pub fn sort_panels_by_reading_order(panels: &mut Vec<PanelRect>, reading_order: &str) {
    if panels.len() <= 1 {
        return;
    }

    let tolerance = 5.0; // percentage tolerance for grouping panels into rows/columns

    match reading_order {
        "horizontal-lr" => {
            // Primary: top-to-bottom (by y), Secondary: left-to-right (by x)
            // This is the default order from detect_panels, but sort explicitly
            panels.sort_by(|a, b| {
                let row_a = (a.y / tolerance).floor() as i64;
                let row_b = (b.y / tolerance).floor() as i64;
                row_a.cmp(&row_b).then_with(|| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal))
            });
        }
        "horizontal-rl" => {
            // Primary: top-to-bottom (by y), Secondary: right-to-left (by x, descending)
            panels.sort_by(|a, b| {
                let row_a = (a.y / tolerance).floor() as i64;
                let row_b = (b.y / tolerance).floor() as i64;
                row_a.cmp(&row_b).then_with(|| b.x.partial_cmp(&a.x).unwrap_or(std::cmp::Ordering::Equal))
            });
        }
        "vertical-lr" => {
            // Primary: left-to-right (by x), Secondary: top-to-bottom (by y)
            panels.sort_by(|a, b| {
                let col_a = (a.x / tolerance).floor() as i64;
                let col_b = (b.x / tolerance).floor() as i64;
                col_a.cmp(&col_b).then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
            });
        }
        "vertical-rl" => {
            // Primary: right-to-left (by x, descending), Secondary: top-to-bottom (by y)
            panels.sort_by(|a, b| {
                let col_a = (a.x / tolerance).floor() as i64;
                let col_b = (b.x / tolerance).floor() as i64;
                col_b.cmp(&col_a).then_with(|| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal))
            });
        }
        _ => {
            // Unknown reading order - leave as-is (horizontal-lr default)
        }
    }
}

/// Find horizontal gutters - consecutive runs of low-variance rows.
///
/// Returns a list of (start_y, end_y) pairs for each gutter.
fn find_horizontal_gutters(
    gray: &GrayImage,
    width: u32,
    height: u32,
    variance_threshold: f64,
    min_gutter_height: u32,
) -> Vec<(u32, u32)> {
    let mut gutters = Vec::new();
    let mut gutter_start: Option<u32> = None;

    for y in 0..height {
        let var = row_variance(gray, y, width);
        if var < variance_threshold {
            if gutter_start.is_none() {
                gutter_start = Some(y);
            }
        } else {
            if let Some(start) = gutter_start {
                let run_len = y - start;
                if run_len >= min_gutter_height {
                    gutters.push((start, y));
                }
                gutter_start = None;
            }
        }
    }
    // Handle gutter at bottom edge
    if let Some(start) = gutter_start {
        let run_len = height - start;
        if run_len >= min_gutter_height {
            gutters.push((start, height));
        }
    }

    gutters
}

/// Find vertical gutters within a horizontal strip of the image.
///
/// Scans columns from x=0 to x=width within rows [y_start, y_end).
/// Returns a list of (start_x, end_x) pairs for each gutter.
fn find_vertical_gutters(
    gray: &GrayImage,
    y_start: u32,
    y_end: u32,
    width: u32,
    variance_threshold: f64,
    min_gutter_width: u32,
) -> Vec<(u32, u32)> {
    let mut gutters = Vec::new();
    let mut gutter_start: Option<u32> = None;

    let strip_height = y_end - y_start;
    if strip_height == 0 {
        return gutters;
    }

    for x in 0..width {
        let var = col_variance_range(gray, x, y_start, y_end);
        if var < variance_threshold {
            if gutter_start.is_none() {
                gutter_start = Some(x);
            }
        } else {
            if let Some(start) = gutter_start {
                let run_len = x - start;
                if run_len >= min_gutter_width {
                    gutters.push((start, x));
                }
                gutter_start = None;
            }
        }
    }
    // Handle gutter at right edge
    if let Some(start) = gutter_start {
        let run_len = width - start;
        if run_len >= min_gutter_width {
            gutters.push((start, width));
        }
    }

    gutters
}

/// Calculate pixel variance of a column within a row range [y_start, y_end).
fn col_variance_range(gray: &GrayImage, x: u32, y_start: u32, y_end: u32) -> f64 {
    let n = (y_end - y_start) as f64;
    if n <= 0.0 {
        return 0.0;
    }
    let mut sum: f64 = 0.0;
    let mut sum_sq: f64 = 0.0;
    for y in y_start..y_end {
        let v = gray.get_pixel(x, y).0[0] as f64;
        sum += v;
        sum_sq += v * v;
    }
    let mean = sum / n;
    (sum_sq / n) - (mean * mean)
}

/// Convert a list of gutter intervals into content strip intervals.
///
/// Given gutters within [0, total_size), returns the content regions
/// between them. Gutters at the very edges are treated as borders
/// (content starts after them, ends before them).
fn strips_from_gutters(gutters: &[(u32, u32)], total_size: u32) -> Vec<(u32, u32)> {
    if gutters.is_empty() {
        return vec![(0, total_size)];
    }

    let mut strips = Vec::new();
    let mut pos = 0u32;

    for &(g_start, g_end) in gutters {
        if g_start > pos {
            strips.push((pos, g_start));
        }
        pos = g_end;
    }

    // Content after the last gutter
    if pos < total_size {
        strips.push((pos, total_size));
    }

    strips
}

/// Detect panels for each processed page image.
///
/// Returns a Vec parallel to `pages`, where each element is either
/// Some(panels) if panels were detected, or None for full-page splash pages.
fn detect_panels_for_pages(pages: &[ProcessedImage]) -> Vec<Option<Vec<PanelRect>>> {
    pages
        .par_iter()
        .map(|page| {
            let img = image::load_from_memory(&page.jpeg_data).ok()?;
            let panels = detect_panels(&img);
            if panels.is_empty() {
                None
            } else {
                Some(panels)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// ComicInfo.xml parsing
// ---------------------------------------------------------------------------

/// Find and parse ComicInfo.xml from the input source.
fn find_and_parse_comic_info(
    input: &Path,
    cbz_temp_dir: Option<&Path>,
) -> Option<ComicMetadata> {
    // Check in CBZ extraction directory first
    if let Some(temp_dir) = cbz_temp_dir {
        let path = temp_dir.join("ComicInfo.xml");
        if path.exists() {
            return parse_comic_info(&path).ok();
        }
        // Also check case-insensitive
        if let Ok(entries) = fs::read_dir(temp_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name == "comicinfo.xml" {
                    return parse_comic_info(&entry.path()).ok();
                }
            }
        }
    }

    // Check in input directory
    if input.is_dir() {
        let path = input.join("ComicInfo.xml");
        if path.exists() {
            return parse_comic_info(&path).ok();
        }
        // Case-insensitive search
        if let Ok(entries) = fs::read_dir(input) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if name == "comicinfo.xml" {
                    return parse_comic_info(&entry.path()).ok();
                }
            }
        }
    }

    None
}

/// Parse a ComicInfo.xml file into ComicMetadata.
pub fn parse_comic_info(path: &Path) -> Result<ComicMetadata, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    parse_comic_info_xml(&content)
}

/// Parse ComicInfo.xml content string into ComicMetadata.
pub fn parse_comic_info_xml(xml: &str) -> Result<ComicMetadata, Box<dyn std::error::Error>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    let mut metadata = ComicMetadata::default();
    let mut current_tag = String::new();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                current_tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                let text = text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                match current_tag.as_str() {
                    "Title" => metadata.title = Some(text),
                    "Series" => metadata.series = Some(text),
                    "Number" => metadata.number = Some(text),
                    "Writer" => {
                        // May contain comma-separated names
                        for name in text.split(',') {
                            let name = name.trim().to_string();
                            if !name.is_empty() {
                                metadata.writers.push(name);
                            }
                        }
                    }
                    "Penciller" => {
                        for name in text.split(',') {
                            let name = name.trim().to_string();
                            if !name.is_empty() {
                                metadata.pencillers.push(name);
                            }
                        }
                    }
                    "Inker" => {
                        for name in text.split(',') {
                            let name = name.trim().to_string();
                            if !name.is_empty() {
                                metadata.inkers.push(name);
                            }
                        }
                    }
                    "Summary" => metadata.summary = Some(text),
                    "Manga" => {
                        if text == "YesAndRightToLeft" || text == "Yes" {
                            metadata.manga_rtl = true;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => {
                current_tag.clear();
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                eprintln!("Warning: error parsing ComicInfo.xml: {}", e);
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    if metadata.title.is_some() || metadata.series.is_some() {
        eprintln!("Parsed ComicInfo.xml: {}", metadata.effective_title().unwrap_or_default());
    }
    if metadata.manga_rtl {
        eprintln!("ComicInfo.xml specifies manga (RTL) reading direction");
    }

    Ok(metadata)
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
/// Supports RTL page progression, ComicInfo.xml metadata, and Panel View markup.
fn write_fixed_layout_epub_v2(
    temp_dir: &Path,
    pages: &[ProcessedImage],
    profile: &DeviceProfile,
    rtl: bool,
    metadata: Option<&ComicMetadata>,
    panel_view: bool,
    options: &ComicOptions,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let images_dir = temp_dir.join("images");
    fs::create_dir_all(&images_dir)?;

    // Write image files
    for page in pages {
        let filename = format!("page_{:04}.jpg", page.index);
        fs::write(images_dir.join(&filename), &page.jpeg_data)?;
    }

    // Check if any page actually has panel data
    let any_panels = panel_view && pages.iter().any(|p| p.panels.is_some());

    // Write XHTML pages
    for page in pages {
        let xhtml = build_page_xhtml(page.index, profile, page.panels.as_deref());
        let filename = format!("page_{:04}.xhtml", page.index);
        fs::write(temp_dir.join(&filename), xhtml.as_bytes())?;
    }

    // Write CSS
    let css = build_comic_css(any_panels);
    fs::write(temp_dir.join("comic.css"), css.as_bytes())?;

    // Handle external cover image if provided
    let external_cover_id = if let Some(CoverSource::FilePath(ref path)) = options.cover {
        let cover_filename = "cover_override.jpg";
        let cover_data = fs::read(path).map_err(|e| {
            format!("Could not read cover image {}: {}", path.display(), e)
        })?;
        // Re-encode cover image as JPEG to ensure Kindle compatibility
        let cover_img = image::load_from_memory(&cover_data).map_err(|e| {
            format!("Could not decode cover image {}: {}", path.display(), e)
        })?;
        let cover_img = if options.cover_fill {
            cover_fill_crop(&cover_img, profile.width, profile.height)
        } else {
            cover_img
        };
        let cover_resized = cover_img.resize(profile.width, profile.height, FilterType::Lanczos3);
        let cover_jpeg = encode_jpeg(&cover_resized, options.jpeg_quality)?;
        fs::write(images_dir.join(cover_filename), &cover_jpeg)?;
        Some(cover_filename.to_string())
    } else {
        None
    };

    // Write OPF
    let opf = build_comic_opf_v2(pages.len(), profile, rtl, metadata, any_panels, options, external_cover_id.as_deref());
    let opf_path = temp_dir.join("content.opf");
    fs::write(&opf_path, opf.as_bytes())?;

    // Write NCX
    let ncx = build_comic_ncx(pages.len());
    fs::write(temp_dir.join("toc.ncx"), ncx.as_bytes())?;

    Ok(opf_path)
}

/// Build the OPF manifest for the comic.
fn build_comic_opf_v2(
    num_pages: usize,
    profile: &DeviceProfile,
    rtl: bool,
    metadata: Option<&ComicMetadata>,
    panel_view: bool,
    options: &ComicOptions,
    external_cover_filename: Option<&str>,
) -> String {
    let mut manifest_items = String::new();
    let mut spine_items = String::new();

    // NCX
    manifest_items.push_str("    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n");

    // CSS
    manifest_items.push_str("    <item id=\"css\" href=\"comic.css\" media-type=\"text/css\"/>\n");

    for i in 0..num_pages {
        manifest_items.push_str(&format!(
            "    <item id=\"page{:04}\" href=\"page_{:04}.xhtml\" media-type=\"application/xhtml+xml\"/>\n",
            i, i
        ));
        manifest_items.push_str(&format!(
            "    <item id=\"img{:04}\" href=\"images/page_{:04}.jpg\" media-type=\"image/jpeg\"/>\n",
            i, i
        ));
        spine_items.push_str(&format!(
            "    <itemref idref=\"page{:04}\"/>\n",
            i
        ));
    }

    // Handle cover: external file, page number override, or default (first page)
    let (cover_meta, cover_manifest_entry) = if let Some(ref cover_filename) = external_cover_filename {
        // External cover image file
        let entry = format!(
            "    <item id=\"cover_img\" href=\"images/{}\" media-type=\"image/jpeg\"/>\n",
            cover_filename
        );
        ("  <meta name=\"cover\" content=\"cover_img\"/>\n".to_string(), entry)
    } else if let Some(CoverSource::PageNumber(page_num)) = options.cover {
        // 1-based page number
        let idx = page_num.saturating_sub(1);
        if idx < num_pages {
            (format!("  <meta name=\"cover\" content=\"img{:04}\"/>\n", idx), String::new())
        } else {
            eprintln!("Warning: cover page {} exceeds page count {}, using first page", page_num, num_pages);
            if num_pages > 0 {
                ("  <meta name=\"cover\" content=\"img0000\"/>\n".to_string(), String::new())
            } else {
                (String::new(), String::new())
            }
        }
    } else if num_pages > 0 {
        ("  <meta name=\"cover\" content=\"img0000\"/>\n".to_string(), String::new())
    } else {
        (String::new(), String::new())
    };

    // Determine title: CLI override > ComicInfo.xml > fallback
    let title = if let Some(ref t) = options.title_override {
        t.clone()
    } else {
        metadata
            .and_then(|m| m.effective_title())
            .unwrap_or_else(|| "Comic".to_string())
    };

    // Determine creators: CLI override > ComicInfo.xml
    let mut creator_entries = String::new();
    if let Some(ref author) = options.author_override {
        creator_entries.push_str(&format!(
            "    <dc:creator>{}</dc:creator>\n",
            escape_xml(author)
        ));
    } else if let Some(meta) = metadata {
        for creator in meta.creators() {
            creator_entries.push_str(&format!(
                "    <dc:creator>{}</dc:creator>\n",
                escape_xml(&creator)
            ));
        }
    }

    // Determine language: CLI override > default "en"
    let language = options.language.as_deref().unwrap_or("en");

    // Determine description
    let mut description_entry = String::new();
    if let Some(meta) = metadata {
        if let Some(ref summary) = meta.summary {
            description_entry = format!(
                "    <dc:description>{}</dc:description>\n",
                escape_xml(summary)
            );
        }
    }

    // Page progression direction
    let ppd = if rtl { "rtl" } else { "ltr" };

    // Writing mode meta for RTL
    let writing_mode_meta = if rtl {
        "    <meta name=\"writing-mode\" content=\"horizontal-rl\"/>\n"
    } else {
        ""
    };

    // Panel View metadata
    let panel_view_meta = if panel_view {
        "    <meta name=\"book-type\" content=\"comic\"/>\n    <meta name=\"region-mag\" content=\"true\"/>\n"
    } else {
        ""
    };

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" xmlns="http://www.idpf.org/2007/opf" unique-identifier="uid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>{title}</dc:title>
    <dc:language>{language}</dc:language>
    <dc:identifier id="uid">kindling-comic-{timestamp}</dc:identifier>
{creator_entries}{description_entry}    <meta name="fixed-layout" content="true"/>
    <meta name="original-resolution" content="{width}x{height}"/>
    <meta property="rendition:layout">pre-paginated</meta>
    <meta property="rendition:orientation">auto</meta>
{writing_mode_meta}{panel_view_meta}{cover_meta}  </metadata>
  <manifest>
{cover_manifest_entry}{manifest_items}  </manifest>
  <spine toc="ncx" page-progression-direction="{ppd}">
{spine_items}  </spine>
</package>
"#,
        title = escape_xml(&title),
        language = escape_xml(language),
        timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        width = profile.width,
        height = profile.height,
        cover_meta = cover_meta,
        cover_manifest_entry = cover_manifest_entry,
        creator_entries = creator_entries,
        description_entry = description_entry,
        manifest_items = manifest_items,
        spine_items = spine_items,
        ppd = ppd,
        writing_mode_meta = writing_mode_meta,
        panel_view_meta = panel_view_meta,
    )
}

/// Escape special XML characters.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&apos;")
}

/// Build an XHTML page for a single comic page, with optional Panel View markup.
fn build_page_xhtml(page_index: usize, profile: &DeviceProfile, panels: Option<&[PanelRect]>) -> String {
    let panel_divs = match panels {
        Some(rects) if !rects.is_empty() => {
            let mut divs = String::new();
            divs.push_str("  <div id=\"panels\">\n");
            for rect in rects {
                divs.push_str(&format!(
                    "    <div class=\"panel\" style=\"position:absolute;left:{x:.1}%;top:{y:.1}%;width:{w:.1}%;height:{h:.1}%\"></div>\n",
                    x = rect.x,
                    y = rect.y,
                    w = rect.w,
                    h = rect.h,
                ));
            }
            divs.push_str("  </div>\n");
            divs
        }
        _ => String::new(),
    };

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
  <div id="content">
    <img src="images/page_{index:04}.jpg" alt="Page {page_num}" style="width:100%;height:100%"/>
  </div>
{panel_divs}</body>
</html>
"#,
        width = profile.width,
        height = profile.height,
        page_num = page_index + 1,
        index = page_index,
        panel_divs = panel_divs,
    )
}

/// Build the CSS for full-bleed comic pages, with optional Panel View styles.
fn build_comic_css(panel_view: bool) -> String {
    let mut css = r#"html, body {
  margin: 0;
  padding: 0;
  width: 100%;
  height: 100%;
}
#content {
  width: 100%;
  height: 100%;
  text-align: center;
}
#content img {
  width: 100%;
  height: 100%;
  object-fit: contain;
}
"#.to_string();

    if panel_view {
        css.push_str(r#"#panels {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
}
.panel {
  position: absolute;
}
"#);
    }

    css
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
