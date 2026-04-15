// Extracted EPUB content with lazy HTML/CSS caches.

use std::cell::{Ref, RefCell};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::epub;
use crate::opf::OPFData;
use crate::profile::Profile;

/// A ready-to-inspect EPUB: OPF parsed, content root set, parser caches ready.
pub struct ExtractedEpub {
    pub root: PathBuf,
    pub opf_path: PathBuf,
    pub opf: OPFData,
    pub profile: Profile,

    // Optional tempdir cleanup guard for from_epub_path callers.
    _temp_dir: Option<PathBuf>,

    raw_cache: RefCell<HashMap<String, Option<String>>>,
    html_cache: RefCell<HashMap<String, scraper::Html>>,
    css_text_cache: RefCell<HashMap<String, String>>,
    ids_cache: RefCell<HashMap<String, HashSet<String>>>,
}

impl ExtractedEpub {
    /// Build from an already-extracted OPF on disk.
    pub fn from_opf_path(opf_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let opf = OPFData::parse(opf_path)?;
        let root = opf.base_dir.clone();
        let profile = Profile::autodetect(&opf, &root);
        Ok(Self {
            root,
            opf_path: opf_path.to_path_buf(),
            opf,
            profile,
            _temp_dir: None,
            raw_cache: RefCell::new(HashMap::new()),
            html_cache: RefCell::new(HashMap::new()),
            css_text_cache: RefCell::new(HashMap::new()),
            ids_cache: RefCell::new(HashMap::new()),
        })
    }

    /// Unzip an EPUB into a temp directory and build from there.
    #[allow(dead_code)]
    pub fn from_epub_path(epub_path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let (temp_dir, opf_path) = epub::extract_epub(epub_path)?;
        let mut me = Self::from_opf_path(&opf_path)?;
        me._temp_dir = Some(temp_dir);
        Ok(me)
    }

    /// Raw file text relative to the content root. Caches a miss so we do not
    /// re-read failed paths.
    pub fn read(&self, href: &str) -> Option<String> {
        if let Some(cached) = self.raw_cache.borrow().get(href) {
            return cached.clone();
        }
        let path = self.root.join(href);
        let content = fs::read_to_string(&path).ok();
        self.raw_cache.borrow_mut().insert(href.to_string(), content.clone());
        content
    }

    /// Parsed HTML for `href`, if the file exists and is readable.
    #[allow(dead_code)]
    pub fn html(&self, href: &str) -> Option<Ref<'_, scraper::Html>> {
        if !self.html_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            let doc = scraper::Html::parse_document(&text);
            self.html_cache.borrow_mut().insert(href.to_string(), doc);
        }
        Some(Ref::map(self.html_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Raw CSS text for `href`. Phase 0 does not keep a parsed lightningcss
    /// stylesheet cached because the parser's borrow lifetimes are awkward to
    /// store next to a `RefCell`. Callers that need to parse should call
    /// `lightningcss::stylesheet::StyleSheet::parse(&text, ...)` themselves.
    #[allow(dead_code)]
    pub fn css(&self, href: &str) -> Option<Ref<'_, String>> {
        if !self.css_text_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            self.css_text_cache.borrow_mut().insert(href.to_string(), text);
        }
        Some(Ref::map(self.css_text_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Set of `id` attribute values found in `href`, for link-target checks.
    #[allow(dead_code)]
    pub fn ids(&self, href: &str) -> Option<Ref<'_, HashSet<String>>> {
        if !self.ids_cache.borrow().contains_key(href) {
            let text = self.read(href)?;
            let set = collect_ids(&text);
            self.ids_cache.borrow_mut().insert(href.to_string(), set);
        }
        Some(Ref::map(self.ids_cache.borrow(), |m| m.get(href).unwrap()))
    }

    /// Every manifest href, fragment stripped.
    #[allow(dead_code)]
    pub fn manifest_hrefs(&self) -> HashSet<String> {
        self.opf
            .manifest
            .values()
            .map(|(href, _)| match href.find('#') {
                Some(i) => href[..i].to_string(),
                None => href.clone(),
            })
            .collect()
    }
}

impl Drop for ExtractedEpub {
    fn drop(&mut self) {
        if let Some(ref dir) = self._temp_dir {
            if dir.exists() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }
}

/// Collect every `id="..."` attribute value from an HTML string.
fn collect_ids(html: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut rest = html;
    while let Some(idx) = rest.find(" id=\"") {
        rest = &rest[idx + 5..];
        let Some(end) = rest.find('"') else { break };
        out.insert(rest[..end].to_string());
        rest = &rest[end..];
    }
    out
}
