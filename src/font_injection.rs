//! Caller-supplied fonts for reflowable KF8 books. Source files stay unchanged.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::fonts;

/// Validated TTF/OTF faces to embed and select for a book's body and paragraphs.
///
/// Families follow the order of the supplied paths. Faces in the same family
/// keep their weight and style, so regular, bold, and italic files can be
/// supplied together. A family override groups all supplied faces under that
/// name. Duplicate family/weight/style combinations are rejected.
pub struct FontInjection {
    faces: Vec<Face>,
    families: Vec<String>,
}

struct Face {
    data: Vec<u8>,
    family: String,
    weight: u16,
    style: &'static str,
}

impl FontInjection {
    /// Read standalone, static TTF/OTF files and derive their family names
    /// from the OpenType name table (typographic family, then legacy family).
    /// `family` overrides the names for all supplied faces.
    pub fn from_paths(
        paths: &[PathBuf],
        family: Option<&str>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if paths.is_empty() {
            return Err("font injection requires at least one --embed-font path".into());
        }
        let family = family.map(str::trim);
        if family.is_some_and(str::is_empty) {
            return Err("--font-family must not be empty".into());
        }
        let mut faces = Vec::new();
        let mut families = Vec::new();
        let mut combinations = HashSet::new();
        for path in paths {
            let data = std::fs::read(path)
                .map_err(|e| format!("cannot read font {}: {e}", path.display()))?;
            if !matches!(data.get(..4), Some(b"OTTO" | b"true" | [0, 1, 0, 0])) {
                return Err(format!("{}: --embed-font requires a standalone TTF or OTF file (WOFF and collections are unsupported)", path.display()).into());
            }
            let parsed = ttf_parser::Face::parse(&data, 0)
                .map_err(|e| format!("{}: invalid TTF/OTF font: {e}", path.display()))?;
            if parsed
                .raw_face()
                .table(ttf_parser::Tag::from_bytes(b"fvar"))
                .is_some()
            {
                return Err(format!(
                    "{}: use a static TTF/OTF instance instead of a variable font",
                    path.display()
                )
                .into());
            }
            let name = family
                .map(str::to_owned)
                .or_else(|| family_name(&parsed))
                .ok_or_else(|| {
                    format!(
                        "{}: font has no Unicode family name; supply --font-family",
                        path.display()
                    )
                })?;
            if name.chars().any(char::is_control) {
                return Err(format!(
                    "{}: font family must not contain control characters",
                    path.display()
                )
                .into());
            }
            let weight = parsed.weight().to_number();
            let style = if parsed.is_italic() {
                "italic"
            } else if parsed.is_oblique() {
                "oblique"
            } else {
                "normal"
            };
            if !combinations.insert((name.to_lowercase(), weight, style)) {
                return Err(format!("{}: duplicate injected face for family {name:?}, weight {weight}, style {style}", path.display()).into());
            }
            if !families
                .iter()
                .any(|f: &String| f.eq_ignore_ascii_case(&name))
            {
                families.push(name.clone());
            }
            faces.push(Face {
                data,
                family: name,
                weight,
                style,
            });
        }
        Ok(Self { faces, families })
    }

    /// Font-family fallback order, with the first supplied family preferred.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    pub(crate) fn records(&self) -> Vec<Vec<u8>> {
        self.faces
            .iter()
            .map(|f| fonts::build_font_record(&f.data))
            .collect()
    }

    pub(crate) fn css(&self, first_index: usize) -> String {
        let mut css = String::new();
        for (i, face) in self.faces.iter().enumerate() {
            let index = fonts::encode_kindle_embed_base32(first_index + i);
            let mime = fonts::embed_media_type(&face.data);
            css.push_str(&format!("\n@font-face {{ font-family: {}; font-weight: {}; font-style: {}; src: url(kindle:embed:{index}?mime={mime}); }}\n", css_string(&face.family), face.weight, face.style));
        }
        let stack = self
            .families
            .iter()
            .map(|f| css_string(f))
            .collect::<Vec<_>>()
            .join(", ");
        css.push_str(&format!(
            "body, body p {{ font-family: {stack} !important; }}\n"
        ));
        css
    }
}

fn family_name(face: &ttf_parser::Face<'_>) -> Option<String> {
    for id in [
        ttf_parser::name_id::TYPOGRAPHIC_FAMILY,
        ttf_parser::name_id::FAMILY,
    ] {
        let mut names: Vec<_> = face
            .names()
            .into_iter()
            .filter(|n| n.name_id == id)
            .filter_map(|n| {
                n.to_string()
                    .map(|s| (n.language_id != 0x0409, s.trim().to_string()))
            })
            .filter(|(_, s)| !s.is_empty())
            .collect();
        names.sort();
        if let Some((_, name)) = names.into_iter().next() {
            return Some(name);
        }
    }
    None
}

fn css_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Each spine document needs a reference, including books without stylesheets.
pub(crate) fn link_css_flow(html: &str) -> String {
    const LINK: &str =
        "<link rel=\"stylesheet\" type=\"text/css\" href=\"kindle:flow:0001?mime=text/css\"/>";
    let close = regex::Regex::new(r"(?i)</head\s*>").unwrap();
    if let Some(end) = close.find(html) {
        let mut result = html.to_string();
        result.insert_str(end.start(), LINK);
        return result;
    }
    let open = regex::Regex::new(r"(?i)<html\b[^>]*>").unwrap();
    let head = format!("<head>{LINK}</head>");
    let mut result = html.to_string();
    result.insert_str(open.find(html).map_or(0, |m| m.end()), &head);
    result
}
