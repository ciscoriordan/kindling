// EPUB content profile used to scope per-rule applicability.

use std::fs;
use std::path::Path;

use crate::opf::OPFData;

/// Which KDP rule set applies to a given EPUB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Default,
    Comic,
    Dict,
    #[allow(dead_code)]
    Textbook,
}

/// Bitmask value for `Rule::profile_mask` that selects every profile.
pub const ALL_PROFILES: u8 = 0b1111;

impl Profile {
    /// Bit value for this profile: `1 << variant_index`.
    pub const fn as_bit(&self) -> u8 {
        match self {
            Profile::Default => 1 << 0,
            Profile::Comic => 1 << 1,
            Profile::Dict => 1 << 2,
            Profile::Textbook => 1 << 3,
        }
    }

    /// Detect the profile from parsed OPF data plus the extracted content root.
    pub fn autodetect(opf: &OPFData, root: &Path) -> Profile {
        if is_dict(opf, root) {
            return Profile::Dict;
        }
        if is_comic(opf, root) {
            return Profile::Comic;
        }
        Profile::Default
    }
}

/// Dict profile: OPF has `<DictionaryInLanguage>` metadata or a spine file
/// contains an `idx:entry` marker.
fn is_dict(opf: &OPFData, root: &Path) -> bool {
    if !opf.dict_in_language.is_empty() {
        return true;
    }
    for (_, href) in &opf.spine_items {
        let path = root.join(href);
        if let Ok(content) = fs::read_to_string(&path) {
            if content.contains("idx:entry") {
                return true;
            }
        }
    }
    false
}

/// Comic profile: OPF declares fixed layout, or any spine item contains a
/// `<meta name="viewport">` with width/height.
fn is_comic(opf: &OPFData, root: &Path) -> bool {
    if opf.is_fixed_layout {
        return true;
    }
    for (_, href) in &opf.spine_items {
        let path = root.join(href);
        let Ok(content) = fs::read_to_string(&path) else { continue };
        if has_viewport_meta(&content) {
            return true;
        }
    }
    false
}

/// True if `content` has a `<meta name="viewport">` with both width and height.
fn has_viewport_meta(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(idx) = rest.find("<meta") {
        rest = &rest[idx + 5..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if tag.contains("name=\"viewport\"")
            && tag.contains("width=")
            && tag.contains("height=")
        {
            return true;
        }
        rest = &rest[end..];
    }
    false
}
