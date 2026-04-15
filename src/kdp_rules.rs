// Amazon Kindle Publishing Guidelines rule catalog.

use crate::profile::{Profile, ALL_PROFILES};

/// Version of the Amazon Kindle Publishing Guidelines PDF these rules target.
pub const KPG_VERSION: &str = "2026.1";

/// URL where the KPG PDF is published.
#[allow(dead_code)]
pub const KPG_PDF_URL: &str =
    "https://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf";

/// Severity of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Finding is silenced; retained for round-trip compatibility with rule
    /// catalogs that assign every id a severity.
    #[allow(dead_code)]
    Suppressed,
    /// Usage-level note: stylistic, rarely surfaced.
    #[allow(dead_code)]
    Usage,
    /// Informational note. Does not affect pass/fail.
    Info,
    /// Something that should probably be fixed but is not a hard failure.
    Warning,
    /// Something that will almost certainly fail conversion or render badly.
    Error,
    /// Fatal: the manuscript cannot even be scanned further.
    #[allow(dead_code)]
    Fatal,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Suppressed => write!(f, "suppressed"),
            Severity::Usage => write!(f, "usage"),
            Severity::Info => write!(f, "info"),
            Severity::Warning => write!(f, "warning"),
            Severity::Error => write!(f, "error"),
            Severity::Fatal => write!(f, "fatal"),
        }
    }
}

/// A single validation rule from the Kindle Publishing Guidelines.
#[derive(Debug, Clone, Copy)]
pub struct Rule {
    pub id: &'static str,
    pub section: &'static str,
    pub level: Severity,
    #[allow(dead_code)]
    pub title: &'static str,
    pub pdf_page: u32,
    pub description: &'static str,
    /// Bitmask of profiles this rule fires on; defaults to every profile.
    pub profile_mask: u8,
}

impl Rule {
    /// True if this rule should run against `profile`.
    #[allow(dead_code)]
    pub fn applies_to(&self, profile: Profile) -> bool {
        (self.profile_mask & profile.as_bit()) != 0
    }
}

/// Complete rule catalog. Order is not significant; checks look up rules by id.
pub const RULES: &[Rule] = &[
    // ---- Section 4: Cover Image Guidelines ----
    Rule {
        id: "R4.1.1",
        section: "4.1",
        level: Severity::Info,
        title: "Marketing cover uploaded separately",
        pdf_page: 15,
        description: "Marketing cover image is uploaded separately to KDP and cannot be \
                      validated from the manuscript. Ensure you upload a 2560x1600 JPEG \
                      per Kindle Publishing Guidelines.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R4.2.1",
        section: "4.2",
        level: Severity::Error,
        title: "Cover image required",
        pdf_page: 16,
        description: "No internal content cover image declared. Add either \
                      <item properties=\"coverimage\" ...> (Method 1, preferred) or \
                      <meta name=\"cover\" content=\"<id>\"/> (Method 2) to the OPF.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R4.2.2",
        section: "4.2",
        level: Severity::Error,
        title: "Cover image file missing",
        pdf_page: 16,
        description: "Cover image declared in OPF but the file does not exist on disk.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R4.2.3",
        section: "4.2",
        level: Severity::Warning,
        title: "Cover image too small",
        pdf_page: 15,
        description: "Cover image shortest side is below 500 px. Kindle will not display \
                      covers under 500 px on the shortest side.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R4.2.4",
        section: "4.2",
        level: Severity::Error,
        title: "No HTML cover page in spine",
        pdf_page: 16,
        description: "Do not add an HTML cover page to the content in addition to the \
                      cover image. This may cause the cover to appear twice or fail \
                      conversion. Remove the HTML page from the spine.",
        profile_mask: ALL_PROFILES,
    },

    // ---- Section 5: Navigation Guidelines ----
    Rule {
        id: "R5.1",
        section: "5",
        level: Severity::Warning,
        title: "TOC recommended for books over 20 pages",
        pdf_page: 17,
        description: "KPG strongly recommends a logical TOC for books longer than 20 pages.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.2.1",
        section: "5.2",
        level: Severity::Warning,
        title: "NCX required",
        pdf_page: 19,
        description: "No NCX file found in manifest (media-type application/x-dtbncx+xml). \
                      Amazon requires a logical TOC via an NCX or a toc nav element for all \
                      Kindle books.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.2.2",
        section: "5.2",
        level: Severity::Warning,
        title: "NCX must be referenced from spine",
        pdf_page: 19,
        description: "NCX is declared in manifest but the <spine> element has no \
                      toc=\"<id>\" attribute. KPG 5.2 requires referencing the NCX from \
                      the spine.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.2.3",
        section: "5.2",
        level: Severity::Error,
        title: "NCX references a file not in the manifest",
        pdf_page: 19,
        description: "NCX <content src=\"...\"/> points at a file that is not declared in \
                      the OPF manifest. KDP will reject the upload with \"broken link in \
                      your Table of Contents\". Either add the file to the manifest or \
                      remove the navPoint.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.3.1",
        section: "5.3",
        level: Severity::Error,
        title: "Guide references a file not in the manifest",
        pdf_page: 22,
        description: "OPF <guide><reference href=\"...\"/> points at a file that is not \
                      declared in the manifest. Either add the file or remove the \
                      reference.",
        profile_mask: ALL_PROFILES,
    },

    // ---- Section 6: HTML and CSS Guidelines ----
    Rule {
        id: "R6.1",
        section: "6.1",
        level: Severity::Warning,
        title: "Well-formed XHTML required",
        pdf_page: 22,
        description: "Content is not well-formed XHTML. Kindle requires well-formed HTML \
                      documents for reliable conversion.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.2",
        section: "6.2",
        level: Severity::Warning,
        title: "Avoid negative CSS values",
        pdf_page: 23,
        description: "Negative CSS value for margin/padding/line-height. Positioning with \
                      negative values can cause content to be cut off at screen edges.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.3",
        section: "6.3",
        level: Severity::Error,
        title: "No scripting",
        pdf_page: 23,
        description: "<script> tag found. Scripting is not supported; scripts will be \
                      stripped during conversion and any functionality relying on them \
                      will break.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.4",
        section: "6.4",
        level: Severity::Error,
        title: "No nested <p> tags",
        pdf_page: 23,
        description: "Nested <p> tags found. Files with nested <p> tags do not convert \
                      properly.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.5",
        section: "6.5",
        level: Severity::Error,
        title: "File reference case must match",
        pdf_page: 23,
        description: "File reference case does not match the actual filename on disk. \
                      Case-sensitive filesystems will fail to resolve the reference.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.6",
        section: "6",
        level: Severity::Error,
        title: "XML 1.0 required",
        pdf_page: 22,
        description: "XHTML file declares XML 1.1 in its prolog. kindlegen only supports \
                      XML 1.0 and will reject XML 1.1 files at conversion time. Change \
                      the declaration to version=\"1.0\".",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.7",
        section: "6",
        level: Severity::Error,
        title: "No external entities",
        pdf_page: 22,
        description: "DOCTYPE declares an external ENTITY (SYSTEM or PUBLIC). kindlegen \
                      crashes on external entity resolution and this is also an XXE \
                      security risk. Remove the external entity declaration.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.8",
        section: "6",
        level: Severity::Warning,
        title: "Irregular DOCTYPE",
        pdf_page: 22,
        description: "DOCTYPE is neither HTML5 (<!DOCTYPE html>) nor a canonical XHTML \
                      1.0/1.1 form. Unusual DOCTYPEs trigger quirks mode in the \
                      converter and break some fragments.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.9",
        section: "6",
        level: Severity::Error,
        title: "EPUB namespace wrong",
        pdf_page: 22,
        description: "xmlns:epub points at a URI other than \
                      http://www.idpf.org/2007/ops. This is the Vader Down bug class: \
                      kindlegen silently drops the epub:type attribute so structural \
                      nav entries point at blank pages.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.10",
        section: "6",
        level: Severity::Warning,
        title: "Undeclared entity",
        pdf_page: 22,
        description: "XHTML references a named entity that is not in the XML 1.0 \
                      predefined set or the common HTML5 whitelist. Undeclared \
                      entities render as literal text on Kindle.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.11",
        section: "6",
        level: Severity::Error,
        title: "HTML must be UTF-8",
        pdf_page: 23,
        description: "XHTML file begins with a UTF-16 BOM or declares a non-UTF-8 \
                      encoding. kindlegen only handles UTF-8; other encodings produce \
                      garbled text or an outright rejection.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R6.12",
        section: "6",
        level: Severity::Error,
        title: "CSS must be UTF-8",
        pdf_page: 24,
        description: "CSS file begins with a UTF-16 BOM or declares a non-UTF-8 \
                      @charset. Non-UTF-8 stylesheets are silently dropped wholesale \
                      by kindlegen.",
        profile_mask: ALL_PROFILES,
    },

    // ---- Section 10: Text-Heavy Reflowable Books ----
    Rule {
        id: "R10.3.1",
        section: "10.3.1",
        level: Severity::Warning,
        title: "Heading alignment should use default",
        pdf_page: 29,
        description: "Heading has an explicit text-align. KPG 10.3.1 recommends letting \
                      headings use the default alignment.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R10.4.1",
        section: "10.4.1",
        level: Severity::Error,
        title: "Use supported image format",
        pdf_page: 38,
        description: "Image is not in a supported format (JPEG, PNG, GIF, SVG).",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R10.4.2a",
        section: "10.4.2",
        level: Severity::Warning,
        title: "Image file too large",
        pdf_page: 38,
        description: "Image file exceeds 127 KB. Large image files increase download size \
                      and may fail conversion.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R10.4.2b",
        section: "10.4.2",
        level: Severity::Warning,
        title: "Image dimensions too large",
        pdf_page: 38,
        description: "Image exceeds 5 megapixels. Large images waste storage and may fail \
                      conversion.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R10.5.1",
        section: "10.5.1",
        level: Severity::Warning,
        title: "Avoid large tables",
        pdf_page: 43,
        description: "Table has more than 50 rows. KPG 10.5.1 recommends keeping tables \
                      below 100 rows and 10 columns; large tables may render poorly.",
        profile_mask: ALL_PROFILES,
    },

    // ---- Section 17/18.1: Supported Tags ----
    Rule {
        id: "R17.1",
        section: "17",
        level: Severity::Error,
        title: "Unsupported HTML tag",
        pdf_page: 22,
        description: "Unsupported HTML tag found. KPG 6.1 lists forms, frames, and \
                      JavaScript as unsupported; section 18.1 lists supported tags.",
        profile_mask: ALL_PROFILES,
    },

    // ---- Section 15: Dictionaries (Amazon-legacy KDP format) ----
    Rule {
        id: "R15.1",
        section: "15",
        level: Severity::Error,
        title: "DictionaryInLanguage required",
        pdf_page: 60,
        description: "Dictionary OPF must declare <x-metadata><DictionaryInLanguage> with a \
                      BCP47 language code. Without it, KDP's dict compiler will not enable \
                      lookup mode and the book will appear as a regular book.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.2",
        section: "15",
        level: Severity::Error,
        title: "DictionaryOutLanguage required",
        pdf_page: 60,
        description: "Dictionary OPF must declare <x-metadata><DictionaryOutLanguage> with a \
                      BCP47 language code. Without it, KDP's dict compiler will not enable \
                      lookup mode and the book will appear as a regular book.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.3",
        section: "15",
        level: Severity::Error,
        title: "DefaultLookupIndex must match an idx:entry name",
        pdf_page: 60,
        description: "The <x-metadata><DefaultLookupIndex> value must match at least one \
                      <idx:entry name=\"...\"> in the spine content. A mismatch causes Kindle \
                      to show 'no entries found' on every lookup.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.4",
        section: "15",
        level: Severity::Error,
        title: "At least one idx:entry required",
        pdf_page: 60,
        description: "Dictionary builds must contain at least one <idx:entry> element in spine \
                      content. If zero idx:entry elements are found, the file is not actually \
                      a dictionary.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.5",
        section: "15",
        level: Severity::Warning,
        title: "Spine content should be wrapped in <mbp:frameset>",
        pdf_page: 60,
        description: "Amazon's dictionary HTML parser expects entry content to be wrapped in \
                      <mbp:frameset>. Omitting it works sometimes and fails silently other \
                      times.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.6",
        section: "15",
        level: Severity::Error,
        title: "idx:orth must have a non-empty value attribute",
        pdf_page: 60,
        description: "Every <idx:orth> element must have a non-empty value=\"...\" attribute. \
                      An empty orth leaves a blank lookup entry and crashes lookup on \
                      Paperwhite firmware.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.7",
        section: "15",
        level: Severity::Warning,
        title: "OPF <guide> should contain reference type=\"index\"",
        pdf_page: 60,
        description: "OPF <guide> should include a <reference type=\"index\" ...> entry. Older \
                      Kindle firmware versions use this to locate the dictionary's entry \
                      section.",
        profile_mask: Profile::Dict.as_bit(),
    },

    // ---- Section 15: Dictionaries (epubcheck EPUB 3 DICT rules, gated on EPUB 3) ----
    Rule {
        id: "R15.e1",
        section: "15",
        level: Severity::Error,
        title: "EPUB 3 dict requires content with epub:type=\"dictionary\" (OPF_078)",
        pdf_page: 60,
        description: "OPF_078: An EPUB 3 dictionary must contain at least one content document \
                      with epub:type=\"dictionary\". Fires only when package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e2",
        section: "15",
        level: Severity::Error,
        title: "Dictionary content found but OPF lacks dc:type=dictionary (OPF_079)",
        pdf_page: 60,
        description: "OPF_079: idx:entry or dictionary content is present but the OPF does \
                      not declare <dc:type>dictionary</dc:type> in metadata. Fires only when \
                      package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e3",
        section: "15",
        level: Severity::Warning,
        title: "Search Key Map document must use .xml extension (OPF_080)",
        pdf_page: 60,
        description: "OPF_080: A Search Key Map Document referenced from a dictionary \
                      collection must have a .xml file extension. Fires only when \
                      package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e4",
        section: "15",
        level: Severity::Error,
        title: "Collection link target missing from manifest (OPF_081)",
        pdf_page: 60,
        description: "OPF_081: A resource referenced by a <collection> element must exist in \
                      the OPF manifest. Fires only when package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e5",
        section: "15",
        level: Severity::Error,
        title: "At most one Search Key Map per dictionary collection (OPF_082)",
        pdf_page: 60,
        description: "OPF_082: A dictionary collection may contain at most one Search Key Map \
                      Document. Fires only when package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e6",
        section: "15",
        level: Severity::Error,
        title: "At least one Search Key Map per dictionary collection (OPF_083)",
        pdf_page: 60,
        description: "OPF_083: A dictionary collection must contain at least one Search Key \
                      Map Document. Fires only when package_version is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    Rule {
        id: "R15.e7",
        section: "15",
        level: Severity::Error,
        title: "Dictionary collection may only contain XHTML or SKM docs (OPF_084)",
        pdf_page: 60,
        description: "OPF_084: A dictionary collection may only contain XHTML Content \
                      Documents or Search Key Map Documents. Fires only when package_version \
                      is 3.0.",
        profile_mask: Profile::Dict.as_bit(),
    },
    // ---- Section 11: Fixed-layout EPUB rules (epubcheck HTM_046-060, OPF_011) ----
    Rule {
        id: "R11.1",
        section: "11",
        level: Severity::Error,
        title: "Fixed-layout content without rendition:layout declaration (OPF_011)",
        pdf_page: 45,
        description: "OPF_011: Content looks fixed-layout (viewport meta present, pixel \
                      dimensions, image-heavy pages) but the OPF does not declare \
                      <meta property=\"rendition:layout\">pre-paginated</meta>. KDP will \
                      treat the book as reflowable and reflow the art, destroying the \
                      layout.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.2",
        section: "11",
        level: Severity::Error,
        title: "Fixed-layout XHTML missing viewport meta (HTM_046)",
        pdf_page: 45,
        description: "HTM_046: A fixed-layout content document must carry a <meta \
                      name=\"viewport\" content=\"width=..., height=...\"> so Kindle \
                      knows the page dimensions. Without it the page renders at the \
                      wrong size on every device.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.3",
        section: "11",
        level: Severity::Error,
        title: "Viewport meta missing width or height (HTM_047)",
        pdf_page: 45,
        description: "HTM_047: The viewport meta element must specify both width and \
                      height. Fixed-layout pages without one of these render with the \
                      wrong aspect ratio on Kindle.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.4",
        section: "11",
        level: Severity::Error,
        title: "Invalid rendition:spread value (HTM_048)",
        pdf_page: 45,
        description: "HTM_048: <meta property=\"rendition:spread\"> must be one of none, \
                      landscape, portrait, both, or auto. Unknown values are silently \
                      dropped, which usually means no two-page spread at all.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.5",
        section: "11",
        level: Severity::Error,
        title: "Invalid rendition:orientation value (HTM_049)",
        pdf_page: 45,
        description: "HTM_049: <meta property=\"rendition:orientation\"> must be one of \
                      auto, landscape, or portrait. Unknown values break orientation \
                      locking on Kindle Fire.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.6",
        section: "11",
        level: Severity::Error,
        title: "Invalid rendition:layout value (HTM_050)",
        pdf_page: 45,
        description: "HTM_050: <meta property=\"rendition:layout\"> must be one of \
                      pre-paginated or reflowable. Typos like \"fixed\" or \
                      \"prepaginated\" are silently ignored and the book falls back to \
                      reflowable.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.7",
        section: "11",
        level: Severity::Error,
        title: "Fixed-layout OPF but XHTML has no viewport (HTM_051)",
        pdf_page: 45,
        description: "HTM_051: OPF declares rendition:layout=pre-paginated but this \
                      spine document has no <meta name=\"viewport\"> element. The OPF \
                      declaration and the content disagree; Kindle picks the wrong \
                      layout.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.8",
        section: "11",
        level: Severity::Warning,
        title: "Fixed-layout page with no image content (HTM_052)",
        pdf_page: 45,
        description: "HTM_052: Fixed-layout pages without an <img>, <image>, or <svg> \
                      element render as a blank rectangle on Kindle comic readers. If \
                      this is intentional (title card), add a transparent 1x1 png.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    Rule {
        id: "R11.9",
        section: "11",
        level: Severity::Warning,
        title: "Fixed-layout missing original-resolution metadata (HTM_053)",
        pdf_page: 45,
        description: "HTM_053: KF8 fixed-layout builds should declare \
                      <meta name=\"original-resolution\" content=\"WxH\"/> so Kindle \
                      picks the right pixel scale. Missing the hint causes blurry \
                      rendering on high-DPI Paperwhites.",
        profile_mask: Profile::Comic.as_bit() | Profile::Textbook.as_bit(),
    },
    // ---- Section 7: Manifest and spine integrity (epubcheck OPF_*) ----
    Rule {
        id: "R7.1",
        section: "7",
        level: Severity::Warning,
        title: "File not declared in manifest (OPF_003)",
        pdf_page: 10,
        description: "OPF_003: A file exists in the EPUB content tree but is not declared \
                      in the manifest. Undeclared files are ignored by converters and \
                      waste space in the final book.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.2",
        section: "7",
        level: Severity::Error,
        title: "Declared media-type does not match file bytes (OPF_013)",
        pdf_page: 10,
        description: "OPF_013: A manifest item's declared media-type does not match the \
                      actual file, based on magic bytes. Kindle refuses to decode a file \
                      whose declared type disagrees with its content.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.3",
        section: "7",
        level: Severity::Error,
        title: "File contents do not match declared media-type (OPF_029)",
        pdf_page: 10,
        description: "OPF_029: The file bytes do not match any media-type we recognize \
                      from the declaration. Either the file is corrupt or the declared \
                      media-type is wrong.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.4",
        section: "7",
        level: Severity::Error,
        title: "Spine has no linear content (OPF_033)",
        pdf_page: 10,
        description: "OPF_033: Every <itemref> in the spine has linear=\"no\". At least \
                      one linear itemref is required or the book has no reading order.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.5",
        section: "7",
        level: Severity::Error,
        title: "Duplicate itemref idref (OPF_034)",
        pdf_page: 10,
        description: "OPF_034: Two <itemref> elements reference the same manifest id. The \
                      second reference is redundant and can confuse pagination.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.6",
        section: "7",
        level: Severity::Warning,
        title: "text/html used where xhtml was expected (OPF_035)",
        pdf_page: 10,
        description: "OPF_035: A manifest item has media-type text/html on a .xhtml/.html \
                      resource. EPUB uses application/xhtml+xml for content documents.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.7",
        section: "7",
        level: Severity::Warning,
        title: "Deprecated media-type (OPF_037)",
        pdf_page: 10,
        description: "OPF_037: The item uses a deprecated media-type (image/jpg, \
                      text/xml, application/x-dtbook+xml, text/x-oeb1-document). Replace \
                      it with the canonical equivalent.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.8",
        section: "7",
        level: Severity::Error,
        title: "Dangling fallback id (OPF_040)",
        pdf_page: 10,
        description: "OPF_040: A manifest item declares fallback=\"X\" but X is not a \
                      manifest id. The fallback chain is broken.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.9",
        section: "7",
        level: Severity::Error,
        title: "Dangling fallback-style id (OPF_041)",
        pdf_page: 10,
        description: "OPF_041: A manifest item declares fallback-style=\"X\" but X is not \
                      a manifest id. The fallback-style chain is broken.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.10",
        section: "7",
        level: Severity::Error,
        title: "Non-permissible spine media-type without fallback (OPF_042)",
        pdf_page: 10,
        description: "OPF_042: A spine item has a non-permissible media-type (not xhtml, \
                      svg, or dtbook) and no fallback attribute. Kindle will not render \
                      it as a reading-order page.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.11",
        section: "7",
        level: Severity::Error,
        title: "Spine fallback chain never reaches xhtml/svg (OPF_043)",
        pdf_page: 10,
        description: "OPF_043: A spine item with a non-standard media-type has a fallback \
                      chain that never terminates at an xhtml or svg resource. Kindle \
                      cannot reach a displayable form.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.12",
        section: "7",
        level: Severity::Error,
        title: "Duplicate manifest href (OPF_074)",
        pdf_page: 10,
        description: "OPF_074: Two manifest items share the same href. Each resource must \
                      appear at most once in the manifest.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R7.13",
        section: "7",
        level: Severity::Error,
        title: "Manifest item points at the OPF file itself (OPF_099)",
        pdf_page: 10,
        description: "OPF_099: A manifest item href resolves back to the OPF package file. \
                      The package file must not be listed in its own manifest.",
        profile_mask: ALL_PROFILES,
    },
    // ---- Section 8: OPF prefix attribute and manifest property grammar ----
    Rule {
        id: "R8.1",
        section: "8",
        level: Severity::Error,
        title: "Malformed package prefix attribute (OPF_004)",
        pdf_page: 14,
        description: "OPF_004: The <package prefix=\"...\"> attribute must follow the \
                      syntax `prefix: url [whitespace prefix: url]*`. Fires only when \
                      package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.2",
        section: "8",
        level: Severity::Error,
        title: "Duplicate prefix in package prefix attribute (OPF_005)",
        pdf_page: 14,
        description: "OPF_005: Each prefix name may only appear once in the package \
                      prefix attribute. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.3",
        section: "8",
        level: Severity::Error,
        title: "Reserved prefix rebound to non-standard URI (OPF_006)",
        pdf_page: 14,
        description: "OPF_006: A reserved prefix (dcterms, epub, marc, media, onix, opf, \
                      rendition, schema, xsd) may not be rebound to a non-standard URI in \
                      the package prefix attribute. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.4",
        section: "8",
        level: Severity::Error,
        title: "Prefix maps to a malformed URI (OPF_007)",
        pdf_page: 14,
        description: "OPF_007: A prefix declared in the package prefix attribute must map \
                      to a syntactically valid URI. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.5",
        section: "8",
        level: Severity::Error,
        title: "Manifest item property invalid for media-type (OPF_012)",
        pdf_page: 14,
        description: "OPF_012: A manifest item's properties=\"...\" value is not permitted \
                      for its media-type (e.g., nav on non-xhtml, cover-image on non-image, \
                      mathml on non-xhtml). Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.6",
        section: "8",
        level: Severity::Warning,
        title: "Spine XHTML uses a feature without declaring the property (OPF_014)",
        pdf_page: 14,
        description: "OPF_014: A spine XHTML contains MathML, SVG, scripts, or remote \
                      resources but the manifest item does not declare the matching \
                      property (mathml, svg, scripted, remote-resources). Fires only when \
                      package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.7",
        section: "8",
        level: Severity::Warning,
        title: "Manifest declares a property the content does not use (OPF_015)",
        pdf_page: 14,
        description: "OPF_015: A manifest item declares one of the feature properties \
                      (mathml, svg, scripted, remote-resources) but the content does not \
                      actually use that feature. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.8",
        section: "8",
        level: Severity::Error,
        title: "Property value is syntactically malformed (OPF_026)",
        pdf_page: 14,
        description: "OPF_026: A property value in a manifest item's properties attribute \
                      is syntactically malformed. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.9",
        section: "8",
        level: Severity::Warning,
        title: "Unknown property without a declared prefix (OPF_027)",
        pdf_page: 14,
        description: "OPF_027: A property name is not in the known EPUB property set and \
                      has no declared prefix. Fires only when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R8.10",
        section: "8",
        level: Severity::Error,
        title: "Property uses an undeclared prefix (OPF_028)",
        pdf_page: 14,
        description: "OPF_028: A property uses a prefix that is not in the default prefixes \
                      and was never declared in the package prefix attribute. Fires only \
                      when package_version is 3.0.",
        profile_mask: ALL_PROFILES,
    },
    // ---- Section 9: Cross-references and dead links (epubcheck RSC_*/OPF_091/OPF_098) ----
    Rule {
        id: "R9.1",
        section: "9",
        level: Severity::Warning,
        title: "Non-SVG image referenced with a fragment",
        pdf_page: 27,
        description: "RSC_009: An <img src=\"foo.png#fragment\"> uses a fragment identifier on a \
                      non-SVG raster image. Only SVG content documents support fragment \
                      targeting; the fragment is silently ignored elsewhere.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.2",
        section: "9",
        level: Severity::Warning,
        title: "Link targets a manifest item not in the spine",
        pdf_page: 27,
        description: "RSC_011: An <a href=\"...\"> points at a manifest item that is not \
                      listed in the spine. The target file will not be reachable through \
                      normal reading order and Kindle will not compile the jump target.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.3",
        section: "9",
        level: Severity::Error,
        title: "Fragment id not defined in the target file",
        pdf_page: 27,
        description: "RSC_012: The file on the left of '#' exists in the manifest but the \
                      '#anchor' id is not declared anywhere inside that file. The link will \
                      scroll to the top of the target instead of the intended element.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.4",
        section: "9",
        level: Severity::Error,
        title: "Fragment points into a resource without ids",
        pdf_page: 27,
        description: "RSC_014: The fragment identifier targets a CSS file, image, or font, \
                      none of which support element ids. The anchor is meaningless.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.5",
        section: "9",
        level: Severity::Error,
        title: "SVG <use> without a fragment identifier",
        pdf_page: 27,
        description: "RSC_015: An SVG <use> element must reference another symbol by \
                      fragment identifier (for example xlink:href=\"#icon\"). A bare file \
                      reference is a structural error.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.6",
        section: "9",
        level: Severity::Error,
        title: "href/src is not a valid URL",
        pdf_page: 27,
        description: "RSC_020: An href or src attribute value contains whitespace, control \
                      characters, or bare angle brackets. RFC 3986 cannot parse such a \
                      reference and Kindle will silently strip the link.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.7",
        section: "9",
        level: Severity::Error,
        title: "Relative URL escapes the EPUB container",
        pdf_page: 27,
        description: "RSC_026: A relative URL uses '..' path segments that would resolve \
                      outside the EPUB root. This is a packaging error and a security risk \
                      (path traversal).",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.8",
        section: "9",
        level: Severity::Warning,
        title: "data: URL in href or src",
        pdf_page: 27,
        description: "RSC_029: A data: URL is used in an href or src attribute. Kindle does \
                      not support data: URLs; images and stylesheets must be packaged as \
                      manifest items.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.9",
        section: "9",
        level: Severity::Error,
        title: "file: URL in href or src",
        pdf_page: 27,
        description: "RSC_030: A file: URL is used in an href or src attribute. file: \
                      references point at the author's local disk and will never resolve \
                      on a reader device.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.10",
        section: "9",
        level: Severity::Warning,
        title: "Relative URL carries a ?query component",
        pdf_page: 27,
        description: "RSC_033: A relative URL contains a '?query' component. kindlegen's \
                      URL hashing drops the query before resolving the reference, which \
                      breaks any link that depends on the query part.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.11",
        section: "9",
        level: Severity::Error,
        title: "Manifest item href contains a fragment",
        pdf_page: 27,
        description: "OPF_091: An OPF <manifest> item href must identify a whole resource. \
                      A '#' fragment is not allowed in manifest hrefs because manifest \
                      items are resources, not elements.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R9.12",
        section: "9",
        level: Severity::Error,
        title: "Manifest item href references an element",
        pdf_page: 27,
        description: "OPF_098: An OPF <manifest> item href points at an element (bare \
                      '#id') rather than a resource. Manifest hrefs must name a file.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.4",
        section: "5",
        level: Severity::Warning,
        title: "Pagebreak content but no page-list in NAV",
        pdf_page: 19,
        description: "Content documents contain epub:type=\"pagebreak\" elements but the NAV \
                      document has no <nav epub:type=\"page-list\"> list (NAV_003). Kindle \
                      will not expose page numbers for navigation.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.5",
        section: "5",
        level: Severity::Error,
        title: "Nav or NCX contains remote resource link",
        pdf_page: 19,
        description: "Nav document or NCX contains a link to a remote resource (http:// or \
                      https://) (NAV_010). Kindle navigation must point at packaged content, \
                      not the network.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.6",
        section: "5",
        level: Severity::Warning,
        title: "Nav TOC entries not in spine order",
        pdf_page: 19,
        description: "Nav TOC entries are not in spine reading order (NAV_011). An entry \
                      points at a spine item that comes after the next entry's spine item, \
                      so the Kindle chapter list reads backwards.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.7",
        section: "5",
        level: Severity::Error,
        title: "NCX dtb:uid does not match OPF identifier",
        pdf_page: 19,
        description: "NCX <meta name=\"dtb:uid\"> value does not match the OPF <dc:identifier> \
                      pointed at by <package unique-identifier> (NCX_001). Kindle's TOC will \
                      not bind to the book.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.8",
        section: "5",
        level: Severity::Warning,
        title: "NCX dtb:uid has surrounding whitespace",
        pdf_page: 19,
        description: "NCX dtb:uid value has leading or trailing whitespace (NCX_004). Some \
                      parsers treat this as an identifier mismatch against the OPF.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.9",
        section: "5",
        level: Severity::Warning,
        title: "NCX navPoint has empty text label",
        pdf_page: 19,
        description: "NCX navPoint has an empty <text> label inside <navLabel> (NCX_006). \
                      Empty labels render as blank lines in the Kindle TOC.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.10",
        section: "5",
        level: Severity::Error,
        title: "Guide reference target is not an OPS content document",
        pdf_page: 19,
        description: "OPF <guide><reference href> points at a file that is not a valid OPS \
                      Content Document (OPF_032). The target must be in the manifest with \
                      media-type application/xhtml+xml.",
        profile_mask: ALL_PROFILES,
    },
    Rule {
        id: "R5.11",
        section: "5",
        level: Severity::Error,
        title: "Spine toc attribute target is not an NCX",
        pdf_page: 19,
        description: "OPF <spine toc=\"X\"> points at a manifest item whose media-type is not \
                      application/x-dtbncx+xml (OPF_050). The toc attribute must name the NCX \
                      manifest item.",
        profile_mask: ALL_PROFILES,
    },
    // PHASE2-RULE: G
    // PHASE2-RULE: H
    // PHASE2-RULE: I
    // PHASE2-RULE: K
];

/// Look up a rule by its id. Panics if the id is unknown.
pub fn get(id: &str) -> &'static Rule {
    RULES
        .iter()
        .find(|r| r.id == id)
        .unwrap_or_else(|| panic!("unknown KDP rule id: {}", id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_rule_ids_unique() {
        let mut ids: Vec<&str> = RULES.iter().map(|r| r.id).collect();
        ids.sort();
        let len_before = ids.len();
        ids.dedup();
        assert_eq!(
            len_before,
            ids.len(),
            "duplicate rule id(s) in RULES catalog"
        );
    }

    #[test]
    fn test_get_rule_by_id() {
        let rule = get("R4.2.1");
        assert_eq!(rule.section, "4.2");
        assert_eq!(rule.level, Severity::Error);
    }

    #[test]
    #[should_panic(expected = "unknown KDP rule id")]
    fn test_get_unknown_rule_panics() {
        get("R999");
    }

    #[test]
    fn test_kpg_version_set() {
        assert!(!KPG_VERSION.is_empty());
    }

    #[test]
    fn test_applies_to_default_profile() {
        let rule = get("R4.2.1");
        assert!(rule.applies_to(Profile::Default));
        assert!(rule.applies_to(Profile::Comic));
        assert!(rule.applies_to(Profile::Dict));
    }
}
