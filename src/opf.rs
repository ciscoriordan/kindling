/// OPF and HTML parser for Kindle dictionary source files.
///
/// Parses the OPF manifest/metadata and extracts dictionary entries
/// from the HTML content files with idx:entry markup.

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One `<item>` entry in the OPF manifest, including EPUB 3 fallback attributes.
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub properties: String,
    pub fallback: Option<String>,
    pub fallback_style: Option<String>,
}

/// One `<itemref>` entry in the OPF spine, including `linear` and `properties`.
#[derive(Debug, Clone)]
pub struct SpineItemRef {
    pub idref: String,
    pub linear: String,
    #[allow(dead_code)]
    pub properties: String,
}

/// Parsed OPF file data.
pub struct OPFData {
    pub base_dir: PathBuf,
    pub title: String,
    pub author: String,
    pub language: String,
    pub identifier: String,
    pub date: String,
    pub dict_in_language: String,
    pub dict_out_language: String,
    pub default_lookup_index: String,
    pub spine_items: Vec<(String, String)>, // (id, href) tuples in spine order
    pub manifest: HashMap<String, (String, String)>, // id -> (href, media_type)
    /// Full manifest item list preserving attributes needed by cluster C checks.
    pub manifest_items: Vec<ManifestItem>,
    /// Raw itemref list in spine order, including dangling or duplicated idrefs.
    pub raw_itemrefs: Vec<SpineItemRef>,
    /// Manifest item id with properties="coverimage" (EPUB 3 cover method).
    pub coverimage_id: Option<String>,
    /// True if the OPF declares fixed-layout (pre-paginated) rendering.
    pub is_fixed_layout: bool,
    /// Original resolution from OPF metadata (e.g. "1072x1448").
    pub original_resolution: Option<String>,
    /// Page progression direction from OPF spine element (e.g. "ltr", "rtl").
    pub page_progression_direction: Option<String>,
    /// `<package version="...">` attribute, e.g. "2.0" or "3.0".
    pub package_version: String,
    /// Collected `<dc:type>` values from metadata (EPUB 3 publication types).
    pub dc_types: Vec<String>,
    /// OEB 1.x `<x-metadata><EmbeddedCover>cover.png</EmbeddedCover>` cover
    /// href, used by kindlegen as a legacy alternative to Method 1 / Method 2.
    pub embedded_cover_href: Option<String>,
}

impl OPFData {
    pub fn parse(opf_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let opf_path = opf_path.canonicalize().unwrap_or_else(|_| opf_path.to_path_buf());
        let base_dir = opf_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        let content = std::fs::read_to_string(&opf_path)?;
        let mut data = OPFData {
            base_dir,
            title: String::new(),
            author: String::new(),
            language: String::from("en"),
            identifier: String::new(),
            date: String::new(),
            dict_in_language: String::new(),
            dict_out_language: String::new(),
            default_lookup_index: String::from("default"),
            spine_items: Vec::new(),
            manifest: HashMap::new(),
            manifest_items: Vec::new(),
            raw_itemrefs: Vec::new(),
            coverimage_id: None,
            is_fixed_layout: false,
            original_resolution: None,
            page_progression_direction: None,
            package_version: String::new(),
            dc_types: Vec::new(),
            embedded_cover_href: None,
        };

        // Clean the XML for parsing - strip namespace prefixes that may be unbound
        let cleaned = clean_opf_xml(&content);
        data.parse_xml(&cleaned)?;

        Ok(data)
    }

    fn parse_xml(&mut self, xml: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut in_metadata = false;
        let mut in_manifest = false;
        let mut in_spine = false;
        let mut current_tag = String::new();
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let local_name = local_tag_name(e.name().as_ref());
                    // OEB 1.x uses capitalized DC elements (<dc:Title>,
                    // <dc:Identifier>, etc.). Match case-insensitively on a
                    // lowercased copy and preserve the original for tags that
                    // are already case-sensitive (DictionaryInLanguage, etc.).
                    let lower = local_name.to_ascii_lowercase();

                    match lower.as_str() {
                        "package" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"version" {
                                    self.package_version =
                                        String::from_utf8_lossy(&attr.value).to_string();
                                }
                            }
                        }
                        "metadata" => in_metadata = true,
                        "manifest" => in_manifest = true,
                        "spine" => {
                            in_spine = true;
                            // Check for page-progression-direction attribute
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"page-progression-direction" {
                                    let ppd = String::from_utf8_lossy(&attr.value).to_string();
                                    if !ppd.is_empty() {
                                        self.page_progression_direction = Some(ppd);
                                    }
                                }
                            }
                        }
                        "title" | "creator" | "language" | "identifier" | "date"
                            if in_metadata =>
                        {
                            current_tag = lower.clone();
                        }
                        "type" if in_metadata => {
                            current_tag = "type".to_string();
                        }
                        "dictionaryinlanguage" if in_metadata => {
                            current_tag = "DictionaryInLanguage".to_string();
                        }
                        "dictionaryoutlanguage" if in_metadata => {
                            current_tag = "DictionaryOutLanguage".to_string();
                        }
                        "defaultlookupindex" if in_metadata => {
                            current_tag = "DefaultLookupIndex".to_string();
                        }
                        "embeddedcover" if in_metadata => {
                            current_tag = "EmbeddedCover".to_string();
                        }
                        "meta" if in_metadata => {
                            // Check for fixed-layout and original-resolution metadata
                            let mut name_val = String::new();
                            let mut content_val = String::new();
                            let mut property_val = String::new();
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"name" => {
                                        name_val = String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"content" => {
                                        content_val = String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"property" => {
                                        property_val = String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    _ => {}
                                }
                            }
                            // <meta name="fixed-layout" content="true"/>
                            if name_val == "fixed-layout" && content_val == "true" {
                                self.is_fixed_layout = true;
                            }
                            // <meta name="original-resolution" content="WxH"/>
                            if name_val == "original-resolution" && !content_val.is_empty() {
                                self.original_resolution = Some(content_val.clone());
                            }
                            // <meta property="rendition:layout">pre-paginated</meta>
                            // For empty/self-closing tags, the value is in content attr;
                            // for start tags, we need to capture the text content
                            if property_val == "rendition:layout" {
                                current_tag = "rendition:layout".to_string();
                            }
                        }
                        "item" if in_manifest => {
                            let mut id = String::new();
                            let mut href = String::new();
                            let mut media_type = String::new();
                            let mut properties = String::new();
                            let mut fallback: Option<String> = None;
                            let mut fallback_style: Option<String> = None;
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"id" => {
                                        id = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"href" => {
                                        href = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"media-type" => {
                                        media_type =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"properties" => {
                                        properties =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"fallback" => {
                                        fallback = Some(
                                            String::from_utf8_lossy(&attr.value).to_string(),
                                        );
                                    }
                                    b"fallback-style" => {
                                        fallback_style = Some(
                                            String::from_utf8_lossy(&attr.value).to_string(),
                                        );
                                    }
                                    _ => {}
                                }
                            }
                            if !id.is_empty() {
                                // EPUB 3 cover method: properties="coverimage"
                                if properties.split_whitespace().any(|p| p == "coverimage") {
                                    self.coverimage_id = Some(id.clone());
                                }
                                self.manifest.insert(id.clone(), (href.clone(), media_type.clone()));
                                self.manifest_items.push(ManifestItem {
                                    id,
                                    href,
                                    media_type,
                                    properties,
                                    fallback,
                                    fallback_style,
                                });
                            }
                        }
                        "itemref" if in_spine => {
                            let mut idref = String::new();
                            let mut linear = String::new();
                            let mut properties = String::new();
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"idref" => {
                                        idref =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"linear" => {
                                        linear =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    b"properties" => {
                                        properties =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                    _ => {}
                                }
                            }
                            self.raw_itemrefs.push(SpineItemRef {
                                idref: idref.clone(),
                                linear,
                                properties,
                            });
                            if let Some((href, _)) = self.manifest.get(&idref) {
                                self.spine_items.push((idref, href.clone()));
                            }
                        }
                        _ => {}
                    }
                }
                Ok(Event::Text(ref e)) => {
                    let text = e.unescape().unwrap_or_default().trim().to_string();
                    if !text.is_empty() && in_metadata {
                        match current_tag.as_str() {
                            "title" => self.title = text,
                            "creator" => self.author = text,
                            "language" => self.language = text,
                            "identifier" => self.identifier = text,
                            "date" => self.date = text,
                            "type" => self.dc_types.push(text),
                            "DictionaryInLanguage" => self.dict_in_language = text,
                            "DictionaryOutLanguage" => self.dict_out_language = text,
                            "DefaultLookupIndex" => self.default_lookup_index = text,
                            "EmbeddedCover" => self.embedded_cover_href = Some(text),
                            "rendition:layout" => {
                                if text == "pre-paginated" {
                                    self.is_fixed_layout = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(Event::End(ref e)) => {
                    let local_name = local_tag_name(e.name().as_ref());
                    match local_name.to_ascii_lowercase().as_str() {
                        "metadata" => in_metadata = false,
                        "manifest" => in_manifest = false,
                        "spine" => in_spine = false,
                        _ => {}
                    }
                    current_tag.clear();
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        Ok(())
    }

    /// Return full paths to HTML content files in spine order.
    pub fn get_content_html_paths(&self) -> Vec<PathBuf> {
        self.spine_items
            .iter()
            .filter_map(|(_, href)| {
                let full_path = self.base_dir.join(href);
                if full_path.exists() {
                    Some(full_path)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Return image manifest items as (href, media_type) pairs, ordered by manifest id.
    ///
    /// Only includes items with media-type starting with "image/".
    pub fn get_image_items(&self) -> Vec<(String, String)> {
        let mut items: Vec<(String, String, String)> = self
            .manifest
            .iter()
            .filter(|(_, (_, media_type))| media_type.starts_with("image/"))
            .map(|(id, (href, media_type))| (id.clone(), href.clone(), media_type.clone()))
            .collect();
        // Sort by id for deterministic ordering
        items.sort_by(|a, b| a.0.cmp(&b.0));
        items.into_iter().map(|(_, href, mt)| (href, mt)).collect()
    }

    /// Scan spine HTML files for `<img src="...">` references that point at
    /// image files present on disk but NOT declared in the OPF manifest, and
    /// return them as `(href_relative_to_opf_base, media_type)` pairs.
    ///
    /// kindlegen quietly embeds such undeclared images. PyGlossary and other
    /// OEB 1.x-era tools sometimes emit manifests that omit inline glyph GIFs
    /// referenced from inside `<idx:entry>` HTML, so dictionaries produced
    /// that way need the same "fall back to disk" behavior to render.
    ///
    /// Image srcs are resolved relative to the HTML file's directory (OPF
    /// spec), collapsed into a manifest-style href relative to `base_dir`.
    /// Results are deduped and returned in deterministic href order.
    pub fn find_unreferenced_images(&self) -> Vec<(String, String)> {
        use std::collections::BTreeSet;
        use std::sync::OnceLock;
        static IMG_SRC_RE: OnceLock<Regex> = OnceLock::new();
        let img_src_re = IMG_SRC_RE.get_or_init(|| {
            Regex::new(r#"(?i)<img\b[^>]*?\bsrc\s*=\s*"([^"]*)""#).unwrap()
        });

        let manifest_hrefs: HashMap<String, ()> = self
            .manifest_items
            .iter()
            .map(|item| (item.href.clone(), ()))
            .collect();

        let mut found: BTreeSet<String> = BTreeSet::new();
        for html_path in self.get_content_html_paths() {
            let content = match std::fs::read_to_string(&html_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let html_dir = html_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf();

            for cap in img_src_re.captures_iter(&content) {
                let src = cap.get(1).unwrap().as_str();
                if src.is_empty() {
                    continue;
                }
                // Remote, data: and scheme-qualified URLs have no local file.
                if src.contains("://") || src.starts_with("data:") || src.starts_with("#") {
                    continue;
                }
                let src_no_frag = src.split('#').next().unwrap_or(src);
                let src_decoded = super_percent_decode(src_no_frag);

                let candidate = html_dir.join(&src_decoded);
                let canonical = candidate.canonicalize().unwrap_or(candidate);
                if !canonical.is_file() {
                    continue;
                }
                let base_canonical = self
                    .base_dir
                    .canonicalize()
                    .unwrap_or_else(|_| self.base_dir.clone());
                let rel = match canonical.strip_prefix(&base_canonical) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                let href = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if href.is_empty() || manifest_hrefs.contains_key(&href) {
                    continue;
                }
                found.insert(href);
            }
        }

        found
            .into_iter()
            .filter_map(|href| {
                guess_image_media_type(&href).map(|mt| (href, mt.to_string()))
            })
            .collect()
    }

    /// Find the cover image manifest item id from OPF metadata.
    ///
    /// Returns the OPF manifest item `id` attribute for the cover image, which
    /// is the string used for EXTH 129 (KF8 cover URI) on modern Kindle
    /// firmware. Mirrors `get_cover_image_href`: prefers
    /// `properties="coverimage"` (EPUB 3) and falls back to
    /// `<meta name="cover" content="...">`.
    pub fn get_cover_image_id(&self) -> Option<String> {
        if let Some(ref cover_id) = self.coverimage_id {
            if let Some((_, media_type)) = self.manifest.get(cover_id) {
                if media_type.starts_with("image/") {
                    return Some(cover_id.clone());
                }
            }
        }

        let opf_path = self.base_dir.join(
            std::fs::read_dir(&self.base_dir)
                .ok()?
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "opf")
                        .unwrap_or(false)
                })?
                .file_name(),
        );

        let content = std::fs::read_to_string(&opf_path).ok()?;
        let cleaned = clean_opf_xml(&content);

        let mut reader = Reader::from_str(&cleaned);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let local_name = local_tag_name(e.name().as_ref());
                    if local_name == "meta" {
                        let mut name_val = String::new();
                        let mut content_val = String::new();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"name" => {
                                    name_val =
                                        String::from_utf8_lossy(&attr.value).to_string()
                                }
                                b"content" => {
                                    content_val =
                                        String::from_utf8_lossy(&attr.value).to_string()
                                }
                                _ => {}
                            }
                        }
                        if name_val == "cover" && !content_val.is_empty() {
                            if let Some((_, media_type)) = self.manifest.get(&content_val) {
                                if media_type.starts_with("image/") {
                                    return Some(content_val);
                                }
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        // Method 3: OEB 1.x EmbeddedCover; reverse-look up the manifest id
        // from the href so EXTH 129 can still carry the KF8 cover URI.
        if let Some(href) = &self.embedded_cover_href {
            for item in &self.manifest_items {
                if item.href == *href && item.media_type.starts_with("image/") {
                    return Some(item.id.clone());
                }
            }
        }

        None
    }

    /// Find the cover image href from OPF metadata.
    ///
    /// Supports three cover image methods:
    /// - Method 1 (preferred, EPUB 3): `<item ... properties="coverimage"/>`
    /// - Method 2 (OPF 2.0): `<meta name="cover" content="..."/>` pointing to a manifest id
    /// - Method 3 (OEB 1.x legacy, kindlegen-compatible):
    ///   `<x-metadata><EmbeddedCover>cover.png</EmbeddedCover>` naming a
    ///   manifest item by href. PyGlossary and other OEB 1.x tools emit this.
    pub fn get_cover_image_href(&self) -> Option<String> {
        // Method 1: check for properties="coverimage" captured during manifest parsing
        if let Some(ref cover_id) = self.coverimage_id {
            if let Some((href, media_type)) = self.manifest.get(cover_id) {
                if media_type.starts_with("image/") {
                    return Some(href.clone());
                }
            }
        }

        // Method 2: re-parse the OPF to find <meta name="cover" content="..."/>
        let opf_path = self.base_dir.join(
            // We need to find the OPF file - check common locations
            std::fs::read_dir(&self.base_dir)
                .ok()?
                .filter_map(|e| e.ok())
                .find(|e| {
                    e.path()
                        .extension()
                        .map(|ext| ext == "opf")
                        .unwrap_or(false)
                })?
                .file_name(),
        );

        let content = std::fs::read_to_string(&opf_path).ok()?;
        let cleaned = clean_opf_xml(&content);

        // Find <meta name="cover" content="..."/>
        let mut reader = Reader::from_str(&cleaned);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                    let local_name = local_tag_name(e.name().as_ref());
                    if local_name == "meta" {
                        let mut name_val = String::new();
                        let mut content_val = String::new();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"name" => {
                                    name_val =
                                        String::from_utf8_lossy(&attr.value).to_string()
                                }
                                b"content" => {
                                    content_val =
                                        String::from_utf8_lossy(&attr.value).to_string()
                                }
                                _ => {}
                            }
                        }
                        if name_val == "cover" && !content_val.is_empty() {
                            // content_val is the manifest item ID
                            if let Some((href, media_type)) = self.manifest.get(&content_val) {
                                if media_type.starts_with("image/") {
                                    return Some(href.clone());
                                }
                            }
                        }
                    }
                }
                Ok(Event::Eof) => break,
                Err(_) => break,
                _ => {}
            }
            buf.clear();
        }

        // Method 3: OEB 1.x <EmbeddedCover>cover.png</EmbeddedCover>. Validate
        // that the named file is declared as an image in the manifest so we
        // never embed something that does not render.
        if let Some(href) = &self.embedded_cover_href {
            for item in &self.manifest_items {
                if item.href == *href && item.media_type.starts_with("image/") {
                    return Some(href.clone());
                }
            }
        }

        None
    }

    #[allow(dead_code)]
    pub fn is_dictionary(&self) -> bool {
        !self.dict_in_language.is_empty() || !self.dict_out_language.is_empty()
    }
}

/// A single dictionary entry parsed from HTML.
#[derive(Debug)]
#[allow(dead_code)]
pub struct DictionaryEntry {
    pub headword: String,
    pub inflections: Vec<String>,
    pub html_content: String,
}

/// Parse a dictionary HTML file and extract entries.
///
/// Looks for idx:entry elements with idx:orth headwords and idx:iform inflections.
/// Uses direct string searching instead of regex for the outer entry matching,
/// since the (?s).*? pattern is extremely slow on large files (100+ MB).
///
/// Amazon's KPG §15.6 permits two headword markup styles:
///   Attribute form: `<idx:orth value="headword"/>`
///   Body form:      `<idx:orth><b>headword</b></idx:orth>`
/// kindlegen accepted both; PyGlossary emits the body form. We fall back to the
/// body text when no `value=` attribute is present.
pub fn parse_dictionary_html(html_path: &Path) -> Result<Vec<DictionaryEntry>, std::io::Error> {
    let content = std::fs::read_to_string(html_path)?;
    let mut entries = Vec::new();

    // Static regex compilation (avoids recompilation if called multiple times)
    use std::sync::OnceLock;
    static ORTH_VAL_RE: OnceLock<Regex> = OnceLock::new();
    static IFORM_RE: OnceLock<Regex> = OnceLock::new();
    let orth_val_re =
        ORTH_VAL_RE.get_or_init(|| Regex::new(r#"<idx:orth\b[^>]*\svalue="([^"]*)""#).unwrap());
    let iform_re = IFORM_RE.get_or_init(|| Regex::new(r#"<idx:iform\s+value="([^"]*)""#).unwrap());

    // Find entry blocks by direct string search (much faster than regex on 100+ MB)
    let entry_open = "<idx:entry";
    let entry_close = "</idx:entry>";
    let mut search_pos = 0;

    while let Some(start) = content[search_pos..].find(entry_open) {
        let abs_start = search_pos + start;

        // Find the end of the opening tag
        let after_open = match content[abs_start..].find('>') {
            Some(p) => abs_start + p + 1,
            None => break,
        };

        // Find the closing tag
        let close_pos = match content[after_open..].find(entry_close) {
            Some(p) => after_open + p,
            None => break,
        };

        let entry_inner = &content[after_open..close_pos];
        let full_entry = &content[abs_start..close_pos + entry_close.len()];

        // Extract headword: prefer `value=` attribute; otherwise fall back to
        // the body text of the first `<idx:orth>...</idx:orth>` element.
        let headword = if let Some(cap) = orth_val_re.captures(entry_inner) {
            unescape_html(cap.get(1).unwrap().as_str())
        } else if let Some(hw) = extract_orth_body_text(entry_inner) {
            hw
        } else {
            search_pos = close_pos + entry_close.len();
            continue;
        };

        if headword.is_empty() {
            search_pos = close_pos + entry_close.len();
            continue;
        }

        // Extract inflections
        let inflections: Vec<String> = iform_re
            .captures_iter(entry_inner)
            .map(|cap| unescape_html(cap.get(1).unwrap().as_str()))
            .collect();

        entries.push(DictionaryEntry {
            headword,
            inflections,
            html_content: full_entry.to_string(),
        });

        search_pos = close_pos + entry_close.len();
    }

    Ok(entries)
}

/// Extract headword from the body of the first `<idx:orth>...</idx:orth>` in
/// an entry, stripping inline HTML markup (`<b>`, `<br>`, `<br/>`, `<idx:infl>`,
/// etc.) and unescaping entities. Returns None when there is no `<idx:orth>`
/// element with a non-self-closing body. Used as the body-form headword
/// fallback for PyGlossary-style dictionaries.
fn extract_orth_body_text(entry_inner: &str) -> Option<String> {
    let open = entry_inner.find("<idx:orth")?;
    let after_tag_name = open + "<idx:orth".len();
    let gt = entry_inner[after_tag_name..].find('>')?;
    let tag_close = after_tag_name + gt;
    // Self-closing <idx:orth ... /> has no body; caller will fall through.
    if entry_inner.as_bytes().get(tag_close.saturating_sub(1)) == Some(&b'/') {
        return None;
    }
    let body_start = tag_close + 1;
    let close_rel = entry_inner[body_start..].find("</idx:orth>")?;
    let body = &entry_inner[body_start..body_start + close_rel];

    // Strip nested `<idx:infl>...</idx:infl>` blocks entirely (they carry
    // inflections, not headword text).
    let mut cleaned = String::with_capacity(body.len());
    let mut rest = body;
    loop {
        match rest.find("<idx:infl") {
            Some(i) => {
                cleaned.push_str(&rest[..i]);
                match rest[i..].find("</idx:infl>") {
                    Some(j) => {
                        rest = &rest[i + j + "</idx:infl>".len()..];
                    }
                    None => break,
                }
            }
            None => {
                cleaned.push_str(rest);
                break;
            }
        }
    }

    // Strip any remaining tags and collapse whitespace.
    let mut out = String::with_capacity(cleaned.len());
    let mut in_tag = false;
    for ch in cleaned.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    let unescaped = unescape_html(&out);
    let trimmed = unescaped.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Unescape HTML entities including numeric (decimal and hex) forms.
///
/// Uses static regex compilation to avoid recompiling per-call.
fn unescape_html(text: &str) -> String {
    use std::sync::OnceLock;
    static HEX_RE: OnceLock<Regex> = OnceLock::new();
    static DEC_RE: OnceLock<Regex> = OnceLock::new();

    // Fast path: if no '&' present, nothing to unescape
    if !text.contains('&') {
        return text.to_string();
    }

    let mut result = text.to_string();

    // Hex entities: &#x27; -> '
    let hex_re = HEX_RE.get_or_init(|| Regex::new(r"&#x([0-9a-fA-F]+);").unwrap());
    result = hex_re
        .replace_all(&result, |caps: &regex::Captures| {
            let hex = caps.get(1).unwrap().as_str();
            match u32::from_str_radix(hex, 16) {
                Ok(cp) => char::from_u32(cp).unwrap_or('\u{FFFD}').to_string(),
                Err(_) => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .to_string();

    // Decimal entities: &#39; -> '
    let dec_re = DEC_RE.get_or_init(|| Regex::new(r"&#(\d+);").unwrap());
    result = dec_re
        .replace_all(&result, |caps: &regex::Captures| {
            let num = caps.get(1).unwrap().as_str();
            match num.parse::<u32>() {
                Ok(cp) => char::from_u32(cp).unwrap_or('\u{FFFD}').to_string(),
                Err(_) => caps.get(0).unwrap().as_str().to_string(),
            }
        })
        .to_string();

    // Named entities
    result = result.replace("&amp;", "&");
    result = result.replace("&lt;", "<");
    result = result.replace("&gt;", ">");
    result = result.replace("&quot;", "\"");
    result = result.replace("&apos;", "'");
    result = result.replace("&nbsp;", "\u{00A0}");

    result
}

/// Percent-decode a URL path. Preserves the original bytes when the input
/// contains no `%` escapes; otherwise decodes `%XX` triples as raw bytes.
fn super_percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(h), Some(l)) = (hi, lo) {
                out.push(((h << 4) | l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Map a filename extension to a Kindle-supported image MIME type.
/// Returns `None` for extensions we don't recognize as image formats, so
/// unknown assets stay out of the image pool.
fn guess_image_media_type(href: &str) -> Option<&'static str> {
    let lower = href.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    match ext {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Strip namespace prefixes and clean XML for parsing.
fn clean_opf_xml(content: &str) -> String {
    let mut cleaned = content.to_string();

    // Remove XML declaration
    let xml_decl = Regex::new(r"<\?xml[^?]*\?>").unwrap();
    cleaned = xml_decl.replace_all(&cleaned, "").to_string();

    // Remove namespace prefixes
    let opf_prefix = Regex::new(r"\bopf:").unwrap();
    cleaned = opf_prefix.replace_all(&cleaned, "").to_string();

    let dc_prefix = Regex::new(r"\bdc:").unwrap();
    cleaned = dc_prefix.replace_all(&cleaned, "").to_string();

    // Remove xmlns:* attributes
    let xmlns_attr = Regex::new(r#"\s+xmlns:\w+="[^"]*""#).unwrap();
    cleaned = xmlns_attr.replace_all(&cleaned, "").to_string();

    cleaned
}

/// Extract the local tag name (after any namespace prefix like {uri}name).
fn local_tag_name(name: &[u8]) -> String {
    let s = std::str::from_utf8(name).unwrap_or("");
    if let Some(pos) = s.rfind('}') {
        s[pos + 1..].to_string()
    } else if let Some(pos) = s.rfind(':') {
        s[pos + 1..].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    //! Regression tests for OEB 1.x-style OPFs (PyGlossary, legacy kindlegen
    //! sources) and body-form `<idx:orth>` dictionary markup. Upstream issue:
    //! https://github.com/ciscoriordan/kindling/issues/3.
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "kindling_opf_test_{}_{}",
            tag,
            std::process::id()
        ));
        if d.exists() {
            fs::remove_dir_all(&d).unwrap();
        }
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn extract_orth_body_text_plain() {
        let entry = r#"<idx:orth><b>hello</b></idx:orth>body"#;
        assert_eq!(extract_orth_body_text(entry), Some("hello".to_string()));
    }

    #[test]
    fn extract_orth_body_text_with_br_and_whitespace() {
        // PyGlossary-style: body + <br/> inside <idx:orth>.
        let entry = "<idx:orth>\n<b>-eresse</b><br/>\n</idx:orth>";
        assert_eq!(extract_orth_body_text(entry), Some("-eresse".to_string()));
    }

    #[test]
    fn extract_orth_body_text_skips_idx_infl() {
        // Inflections sit inside <idx:orth> in some templates; must not bleed
        // into the headword text.
        let entry = r#"<idx:orth><b>grogner</b><idx:infl><idx:iform value="grognasses"/></idx:infl></idx:orth>"#;
        assert_eq!(extract_orth_body_text(entry), Some("grogner".to_string()));
    }

    #[test]
    fn extract_orth_body_text_self_closing_returns_none() {
        let entry = r#"<idx:orth value="x"/>"#;
        assert_eq!(extract_orth_body_text(entry), None);
    }

    #[test]
    fn extract_orth_body_text_empty_body_returns_none() {
        let entry = r#"<idx:orth></idx:orth>"#;
        assert_eq!(extract_orth_body_text(entry), None);
    }

    #[test]
    fn extract_orth_body_text_unescapes_entities() {
        let entry = r#"<idx:orth><b>caf&#233;</b></idx:orth>"#;
        assert_eq!(extract_orth_body_text(entry), Some("café".to_string()));
    }

    #[test]
    fn parse_dictionary_html_body_form_idx_orth() {
        // PyGlossary-style: no `value=` attribute, headword lives in body.
        let dir = temp_dir("body_form");
        let html = r#"<html><head></head><body><mbp:frameset>
<idx:entry scriptable="yes" spell="yes">
<idx:orth>
<b>alpha</b><br/>
</idx:orth>
first letter
</idx:entry>
<hr/>
<idx:entry>
<idx:orth><b>beta</b></idx:orth>
second letter
</idx:entry>
</mbp:frameset></body></html>"#;
        let path = dir.join("c.html");
        fs::write(&path, html).unwrap();
        let entries = parse_dictionary_html(&path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].headword, "alpha");
        assert_eq!(entries[1].headword, "beta");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_dictionary_html_body_form_with_inflections() {
        let dir = temp_dir("body_form_infl");
        let html = r#"<html><body>
<idx:entry>
<idx:orth><b>grogner</b>
<idx:infl>
<idx:iform value="grognes"/>
<idx:iform value="grognasses"/>
</idx:infl>
</idx:orth>
verb
</idx:entry>
</body></html>"#;
        let path = dir.join("c.html");
        fs::write(&path, html).unwrap();
        let entries = parse_dictionary_html(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].headword, "grogner");
        assert_eq!(entries[0].inflections, vec!["grognes", "grognasses"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_dictionary_html_attribute_form_still_works() {
        // Make sure the pre-existing attribute form still parses identically.
        let dir = temp_dir("attr_form");
        let html = r#"<html><body>
<idx:entry><idx:orth value="cat">cat</idx:orth><b>cat</b> animal<hr/></idx:entry>
</body></html>"#;
        let path = dir.join("c.html");
        fs::write(&path, html).unwrap();
        let entries = parse_dictionary_html(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].headword, "cat");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opf_parse_oeb1x_capitalized_dc_elements() {
        // PyGlossary emits OEB 1.x DC elements with capital initial letters,
        // wrapped in <dc-metadata>/<x-metadata>. Make sure we still pull out
        // title, language, identifier, dictionary metadata, and EmbeddedCover.
        let dir = temp_dir("oeb1x");
        let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package unique-identifier="uid">
<metadata>
<dc-metadata xmlns:dc="http://purl.org/metadata/dublin_core">
<dc:Title>PyGlossary Dict</dc:Title>
<dc:Language>fr</dc:Language>
<dc:Identifier id="uid">abc123</dc:Identifier>
<dc:Creator>An Author</dc:Creator>
</dc-metadata>
<x-metadata>
<DictionaryInLanguage>fr</DictionaryInLanguage>
<DictionaryOutLanguage>fr</DictionaryOutLanguage>
<EmbeddedCover>cover.png</EmbeddedCover>
</x-metadata>
</metadata>
<manifest>
<item id="cover.png" href="cover.png" media-type="image/png"/>
<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>
</manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        // HTML target does not need to exist for OPF metadata parsing.
        let data = OPFData::parse(&opf_path).unwrap();
        assert_eq!(data.title, "PyGlossary Dict");
        assert_eq!(data.language, "fr");
        assert_eq!(data.identifier, "abc123");
        assert_eq!(data.author, "An Author");
        assert_eq!(data.dict_in_language, "fr");
        assert_eq!(data.dict_out_language, "fr");
        assert_eq!(data.embedded_cover_href.as_deref(), Some("cover.png"));
        // Method 3 (EmbeddedCover) falls through to get_cover_image_href.
        assert_eq!(data.get_cover_image_href().as_deref(), Some("cover.png"));
        assert_eq!(data.get_cover_image_id().as_deref(), Some("cover.png"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn opf_parse_modern_lowercase_dc_elements_unchanged() {
        // Regression: case-insensitive matching must not break lowercase input.
        let dir = temp_dir("modern_dc");
        let opf = r#"<?xml version="1.0"?>
<package unique-identifier="u" xmlns="http://www.idpf.org/2007/opf">
<metadata>
<dc:title xmlns:dc="http://purl.org/dc/elements/1.1/">Lower Dict</dc:title>
<dc:language xmlns:dc="http://purl.org/dc/elements/1.1/">en</dc:language>
<dc:identifier xmlns:dc="http://purl.org/dc/elements/1.1/" id="u">x</dc:identifier>
</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let data = OPFData::parse(&opf_path).unwrap();
        assert_eq!(data.title, "Lower Dict");
        assert_eq!(data.language, "en");
        assert_eq!(data.identifier, "x");
        assert_eq!(data.embedded_cover_href, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn embedded_cover_ignored_if_not_in_manifest() {
        // If <EmbeddedCover> points at something that is not an image manifest
        // item, get_cover_image_href must not invent a path.
        let dir = temp_dir("embedded_cover_nomanifest");
        let opf = r#"<?xml version="1.0"?>
<package unique-identifier="u">
<metadata>
<x-metadata><EmbeddedCover>missing.png</EmbeddedCover></x-metadata>
</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let data = OPFData::parse(&opf_path).unwrap();
        assert_eq!(data.embedded_cover_href.as_deref(), Some("missing.png"));
        assert_eq!(data.get_cover_image_href(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_unreferenced_images_picks_up_undeclared_gif() {
        // PyGlossary-style: an entry HTML references a GIF that exists on disk
        // next to the OPF, but the OPF manifest only declares the HTML file.
        // kindlegen embeds the glyph anyway; we need to match that. Issue #4.
        let dir = temp_dir("undeclared_img");
        let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package unique-identifier="u">
<metadata>
<dc-metadata xmlns:dc="http://purl.org/metadata/dublin_core">
<dc:Title>T</dc:Title><dc:Language>fr</dc:Language><dc:Identifier id="u">x</dc:Identifier>
</dc-metadata>
<x-metadata><DictionaryInLanguage>fr</DictionaryInLanguage></x-metadata>
</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        // Entry HTML with an <img> pointing at a glyph in the same dir.
        let html = r#"<html><body><mbp:frameset>
<idx:entry><idx:orth value="djed"/><b>djed</b>
<img src="./glyph.gif" alt="g"/>
</idx:entry></mbp:frameset></body></html>"#;
        fs::write(dir.join("c.xhtml"), html).unwrap();
        fs::write(dir.join("glyph.gif"), b"GIF89aSTUB").unwrap();

        let data = OPFData::parse(&opf_path).unwrap();
        let extras = data.find_unreferenced_images();
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, "glyph.gif");
        assert_eq!(extras[0].1, "image/gif");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_unreferenced_images_skips_declared_and_missing_files() {
        // Images already in the manifest are NOT duplicated. src paths that
        // don't resolve to a real file on disk are silently skipped.
        let dir = temp_dir("declared_and_missing");
        let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package unique-identifier="u">
<metadata>
<dc-metadata xmlns:dc="http://purl.org/metadata/dublin_core">
<dc:Title>T</dc:Title><dc:Language>en</dc:Language><dc:Identifier id="u">x</dc:Identifier>
</dc-metadata>
</metadata>
<manifest>
<item id="c" href="c.xhtml" media-type="application/xhtml+xml"/>
<item id="cov" href="cover.png" media-type="image/png"/>
</manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let html = r#"<html><body>
<img src="cover.png"/>
<img src="does-not-exist.png"/>
<img src="https://example.com/remote.png"/>
</body></html>"#;
        fs::write(dir.join("c.xhtml"), html).unwrap();
        fs::write(dir.join("cover.png"), b"\x89PNG\r\n\x1a\n").unwrap();

        let data = OPFData::parse(&opf_path).unwrap();
        let extras = data.find_unreferenced_images();
        assert!(extras.is_empty(), "expected no extras, got {:?}", extras);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_unreferenced_images_handles_percent_encoded_src() {
        // Spaces and other reserved characters in <img src> are percent-
        // encoded per HTML / OPF spec. The path must be decoded before we
        // try to stat the file on disk.
        let dir = temp_dir("percent_encoded_img");
        let opf = r#"<?xml version="1.0" encoding="utf-8"?>
<package unique-identifier="u">
<metadata>
<dc-metadata xmlns:dc="http://purl.org/metadata/dublin_core">
<dc:Title>T</dc:Title><dc:Language>en</dc:Language><dc:Identifier id="u">x</dc:Identifier>
</dc-metadata>
</metadata>
<manifest><item id="c" href="c.xhtml" media-type="application/xhtml+xml"/></manifest>
<spine><itemref idref="c"/></spine>
</package>"#;
        let opf_path = dir.join("content.opf");
        fs::write(&opf_path, opf).unwrap();
        let html = r#"<html><body><img src="my%20glyph.png"/></body></html>"#;
        fs::write(dir.join("c.xhtml"), html).unwrap();
        fs::write(dir.join("my glyph.png"), b"\x89PNG\r\n\x1a\n").unwrap();

        let data = OPFData::parse(&opf_path).unwrap();
        let extras = data.find_unreferenced_images();
        assert_eq!(extras.len(), 1);
        assert_eq!(extras[0].0, "my glyph.png");
        fs::remove_dir_all(&dir).ok();
    }
}
