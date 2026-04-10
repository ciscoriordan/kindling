/// OPF and HTML parser for Kindle dictionary source files.
///
/// Parses the OPF manifest/metadata and extracts dictionary entries
/// from the HTML content files with idx:entry markup.

use quick_xml::events::Event;
use quick_xml::Reader;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    /// Manifest item id with properties="coverimage" (EPUB 3 cover method).
    pub coverimage_id: Option<String>,
    /// True if the OPF declares fixed-layout (pre-paginated) rendering.
    pub is_fixed_layout: bool,
    /// Original resolution from OPF metadata (e.g. "1072x1448").
    pub original_resolution: Option<String>,
    /// Page progression direction from OPF spine element (e.g. "ltr", "rtl").
    pub page_progression_direction: Option<String>,
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
            coverimage_id: None,
            is_fixed_layout: false,
            original_resolution: None,
            page_progression_direction: None,
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

                    match local_name.as_str() {
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
                            current_tag = local_name.clone();
                        }
                        "DictionaryInLanguage" if in_metadata => {
                            current_tag = "DictionaryInLanguage".to_string();
                        }
                        "DictionaryOutLanguage" if in_metadata => {
                            current_tag = "DictionaryOutLanguage".to_string();
                        }
                        "DefaultLookupIndex" if in_metadata => {
                            current_tag = "DefaultLookupIndex".to_string();
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
                                    _ => {}
                                }
                            }
                            if !id.is_empty() {
                                // EPUB 3 cover method: properties="coverimage"
                                if properties.split_whitespace().any(|p| p == "coverimage") {
                                    self.coverimage_id = Some(id.clone());
                                }
                                self.manifest.insert(id, (href, media_type));
                            }
                        }
                        "itemref" if in_spine => {
                            let mut idref = String::new();
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"idref" {
                                    idref = String::from_utf8_lossy(&attr.value).to_string();
                                }
                            }
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
                            "DictionaryInLanguage" => self.dict_in_language = text,
                            "DictionaryOutLanguage" => self.dict_out_language = text,
                            "DefaultLookupIndex" => self.default_lookup_index = text,
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
                    match local_name.as_str() {
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

    /// Find the cover image href from OPF metadata.
    ///
    /// Supports both cover image methods from the Amazon Kindle Publishing
    /// Guidelines section 4.2:
    /// - Method 1 (preferred, EPUB 3): `<item ... properties="coverimage"/>`
    /// - Method 2: `<meta name="cover" content="..."/>` pointing to a manifest id
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
pub fn parse_dictionary_html(html_path: &Path) -> Result<Vec<DictionaryEntry>, std::io::Error> {
    let content = std::fs::read_to_string(html_path)?;
    let mut entries = Vec::new();

    // Static regex compilation (avoids recompilation if called multiple times)
    use std::sync::OnceLock;
    static ORTH_RE: OnceLock<Regex> = OnceLock::new();
    static IFORM_RE: OnceLock<Regex> = OnceLock::new();
    let orth_re = ORTH_RE.get_or_init(|| Regex::new(r#"<idx:orth\s+value="([^"]*)""#).unwrap());
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

        // Extract headword
        let headword = match orth_re.captures(entry_inner) {
            Some(cap) => unescape_html(cap.get(1).unwrap().as_str()),
            None => {
                search_pos = close_pos + entry_close.len();
                continue;
            }
        };

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
