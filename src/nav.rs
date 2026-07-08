/// EPUB navigation document parser.
///
/// Extracts the ordered TOC entries (label + target href) from the EPUB 3
/// nav document (`properties="nav"`) or, failing that, the EPUB 2 NCX
/// (`toc.ncx`), so the on-device "Go To" TOC can show the book's real
/// chapter names instead of each spine file's `<title>` tag (issue #18).
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::opf::OPFData;

/// One TOC entry from the EPUB navigation document, in document order.
#[derive(Debug, Clone)]
pub struct NavPoint {
    /// Target file href, percent-decoded, with `.`/`..` segments resolved,
    /// expressed relative to the OPF base dir (manifest-style).
    pub file_href: String,
    /// Fragment identifier (the part after `#`), if any.
    pub fragment: Option<String>,
    /// Display label.
    pub label: String,
}

/// Parse the EPUB navigation document declared in the OPF manifest and
/// return its TOC entries in document order. Prefers the EPUB 3 nav
/// document (`properties="nav"`), falls back to the EPUB 2 NCX (the
/// spine `toc` idref or any `application/x-dtbncx+xml` manifest item).
/// Returns an empty Vec when neither exists or neither yields entries.
pub fn parse_nav_points(opf: &OPFData) -> Vec<NavPoint> {
    // EPUB 3 nav document.
    if let Some(item) = opf
        .manifest_items
        .iter()
        .find(|i| i.properties.split_whitespace().any(|p| p == "nav"))
    {
        let path = opf.base_dir.join(&item.href);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let points = parse_nav_xhtml(&content, parent_dir(&item.href));
            if !points.is_empty() {
                return points;
            }
        }
    }
    // EPUB 2 NCX.
    if let Some(href) = find_ncx_href(opf) {
        let path = opf.base_dir.join(&href);
        if let Ok(content) = std::fs::read_to_string(&path) {
            let points = parse_toc_ncx(&content, parent_dir(&href));
            if !points.is_empty() {
                return points;
            }
        }
    }
    Vec::new()
}

/// Locate the NCX href: any manifest item with the NCX media type, else a
/// manifest href ending in `.ncx`.
fn find_ncx_href(opf: &OPFData) -> Option<String> {
    if let Some(item) = opf
        .manifest_items
        .iter()
        .find(|i| i.media_type == "application/x-dtbncx+xml")
    {
        return Some(item.href.clone());
    }
    opf.manifest_items
        .iter()
        .find(|i| i.href.to_ascii_lowercase().ends_with(".ncx"))
        .map(|i| i.href.clone())
}

/// Parse an EPUB 3 nav document: the `<nav epub:type="toc">` (or
/// `role="doc-toc"`) element's `<a href="...">label</a>` entries in
/// document order. Falls back to the first `<nav>` element when no nav is
/// marked as the TOC.
fn parse_nav_xhtml(content: &str, nav_dir: &str) -> Vec<NavPoint> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().check_end_names = false;

    let mut points = Vec::new();
    // Depth inside the matched <nav> element (0 = outside).
    let mut nav_depth: usize = 0;
    let mut in_toc_nav = false;
    // Set while collecting the text of an <a> inside the TOC nav.
    let mut current: Option<(String, Option<String>, String)> = None;
    let mut saw_marked_toc = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = local_name_lower(e.name().as_ref());
                if name == "nav" {
                    let is_toc = e.attributes().flatten().any(|a| {
                        let key = a.key.as_ref();
                        let val = String::from_utf8_lossy(&a.value).to_ascii_lowercase();
                        (key.ends_with(b"type") && val.split_whitespace().any(|v| v == "toc"))
                            || (key == b"role" && val.split_whitespace().any(|v| v == "doc-toc"))
                    });
                    if nav_depth > 0 {
                        nav_depth += 1;
                    } else if is_toc && !saw_marked_toc {
                        // A marked TOC nav supersedes entries collected
                        // from an earlier unmarked <nav>.
                        saw_marked_toc = true;
                        points.clear();
                        in_toc_nav = true;
                        nav_depth = 1;
                    } else if !saw_marked_toc {
                        in_toc_nav = true;
                        nav_depth = 1;
                    }
                } else if name == "a" && in_toc_nav && current.is_none() {
                    let href = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == b"href")
                        .map(|a| String::from_utf8_lossy(&a.value).to_string())
                        .unwrap_or_default();
                    if !href.is_empty() && !is_external_href(&href) {
                        let (file, frag) = resolve_href(nav_dir, &href);
                        current = Some((file, frag, String::new()));
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Some((_, _, label)) = current.as_mut() {
                    let text = e
                        .unescape()
                        .map(|t| t.to_string())
                        .unwrap_or_else(|_| String::from_utf8_lossy(e.as_ref()).to_string());
                    label.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let name = local_name_lower(e.name().as_ref());
                if name == "a" {
                    if let Some((file, frag, label)) = current.take() {
                        let label = collapse_whitespace(&label);
                        if !label.is_empty() && !file.is_empty() {
                            points.push(NavPoint {
                                file_href: file,
                                fragment: frag,
                                label,
                            });
                        }
                    }
                } else if name == "nav" && nav_depth > 0 {
                    nav_depth -= 1;
                    if nav_depth == 0 {
                        in_toc_nav = false;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    points
}

/// Parse an EPUB 2 NCX document: every `<navPoint>`'s
/// `<navLabel><text>label</text></navLabel>` + `<content src="..."/>`
/// pair, flattened in document order (nested navPoints included).
fn parse_toc_ncx(content: &str, ncx_dir: &str) -> Vec<NavPoint> {
    let mut reader = Reader::from_str(content);
    reader.config_mut().check_end_names = false;

    let mut points = Vec::new();
    let mut in_nav_map = false;
    // Pending label text awaiting its <content src>; navLabel precedes
    // content inside a navPoint per the NCX spec.
    let mut pending_label: Option<String> = None;
    let mut in_text = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = local_name_lower(e.name().as_ref());
                match name.as_str() {
                    "navmap" => in_nav_map = true,
                    "navpoint" if in_nav_map => {
                        pending_label = None;
                    }
                    "text" if in_nav_map => {
                        in_text = true;
                        if pending_label.is_none() {
                            pending_label = Some(String::new());
                        }
                    }
                    "content" if in_nav_map => {
                        let src = e
                            .attributes()
                            .flatten()
                            .find(|a| a.key.as_ref() == b"src")
                            .map(|a| String::from_utf8_lossy(&a.value).to_string())
                            .unwrap_or_default();
                        if let Some(label) = pending_label.take() {
                            let label = collapse_whitespace(&label);
                            if !label.is_empty() && !src.is_empty() && !is_external_href(&src) {
                                let (file, frag) = resolve_href(ncx_dir, &src);
                                if !file.is_empty() {
                                    points.push(NavPoint {
                                        file_href: file,
                                        fragment: frag,
                                        label,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text {
                    if let Some(label) = pending_label.as_mut() {
                        let text = e
                            .unescape()
                            .map(|t| t.to_string())
                            .unwrap_or_else(|_| String::from_utf8_lossy(e.as_ref()).to_string());
                        label.push_str(&text);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = local_name_lower(e.name().as_ref());
                match name.as_str() {
                    "navmap" => in_nav_map = false,
                    "text" => in_text = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    points
}

/// Group nav points by spine position: entry `i` of the result holds the
/// nav points targeting `spine_hrefs[i]`, in nav document order. Spine
/// hrefs and nav targets are both normalized (percent-decoded, dot
/// segments resolved) before comparison.
pub fn group_by_spine(nav_points: &[NavPoint], spine_hrefs: &[String]) -> Vec<Vec<NavPoint>> {
    let mut grouped: Vec<Vec<NavPoint>> = vec![Vec::new(); spine_hrefs.len()];
    let index: std::collections::HashMap<String, usize> = spine_hrefs
        .iter()
        .enumerate()
        .map(|(i, h)| (normalize_path(&percent_decode_str(h)), i))
        .collect();
    for p in nav_points {
        if let Some(&i) = index.get(&p.file_href) {
            grouped[i].push(p.clone());
        }
    }
    grouped
}

/// Directory part of an href ("" when it has none).
fn parent_dir(href: &str) -> &str {
    match href.rfind('/') {
        Some(i) => &href[..i],
        None => "",
    }
}

/// Split an href into (file, fragment), percent-decode the file part, and
/// resolve it against the referencing document's directory.
fn resolve_href(doc_dir: &str, href: &str) -> (String, Option<String>) {
    let (file, frag) = match href.find('#') {
        Some(i) => (&href[..i], Some(href[i + 1..].to_string())),
        None => (href, None),
    };
    let frag = frag.filter(|f| !f.is_empty());
    let decoded = percent_decode_str(file.trim());
    let joined = if doc_dir.is_empty() {
        decoded
    } else {
        format!("{}/{}", doc_dir, decoded)
    };
    (normalize_path(&joined), frag)
}

/// Resolve `.` and `..` segments in a slash-separated path.
fn normalize_path(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

fn is_external_href(href: &str) -> bool {
    let lower = href.trim().to_ascii_lowercase();
    [
        "http://",
        "https://",
        "mailto:",
        "kindle:",
        "tel:",
        "data:",
        "javascript:",
        "ftp://",
    ]
    .iter()
    .any(|s| lower.starts_with(s))
}

/// Collapse whitespace runs to single spaces and trim.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn local_name_lower(name: &[u8]) -> String {
    let local = match name.iter().position(|&b| b == b':') {
        Some(i) => &name[i + 1..],
        None => name,
    };
    String::from_utf8_lossy(local).to_ascii_lowercase()
}

/// Minimal percent-decoder (UTF-8 aware: decodes %-escapes to bytes, then
/// interprets the byte string as UTF-8, falling back to lossy conversion).
fn percent_decode_str(s: &str) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h1 = bytes.next();
            let h2 = bytes.next();
            if let (Some(h1), Some(h2)) = (h1, h2) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", h1 as char, h2 as char), 16)
                {
                    out.push(byte);
                    continue;
                }
                out.push(b'%');
                out.push(h1);
                out.push(h2);
                continue;
            }
            out.push(b'%');
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    #[test]
    fn parses_epub3_nav_toc() {
        let nav = r#"<?xml version="1.0"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<head><title>Contents</title></head>
<body>
<nav epub:type="landmarks"><ol><li><a href="cover.xhtml">Cover</a></li></ol></nav>
<nav epub:type="toc"><h1>Contents</h1>
<ol>
<li><a href="ch1.xhtml">第一章 起风了</a></li>
<li><a href="ch2.xhtml#part2"><span>Chapter</span> Two</a></li>
<li><a href="../text/ch3.xhtml">Chapter &amp; Three</a></li>
<li><a href="https://example.com">External</a></li>
</ol>
</nav>
</body></html>"#;
        let points = parse_nav_xhtml(nav, "OEBPS");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].file_href, "OEBPS/ch1.xhtml");
        assert_eq!(points[0].fragment, None);
        assert_eq!(points[0].label, "第一章 起风了");
        assert_eq!(points[1].file_href, "OEBPS/ch2.xhtml");
        assert_eq!(points[1].fragment.as_deref(), Some("part2"));
        assert_eq!(points[1].label, "Chapter Two");
        assert_eq!(points[2].file_href, "text/ch3.xhtml");
        assert_eq!(points[2].label, "Chapter & Three");
        println!("  \u{2713} EPUB3 nav TOC parsed: {} entries", points.len());
    }

    #[test]
    fn parses_epub2_ncx_navmap() {
        let ncx = r#"<?xml version="1.0"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
<head/>
<docTitle><text>Book</text></docTitle>
<navMap>
<navPoint id="n1"><navLabel><text>One</text></navLabel><content src="ch1.xhtml"/>
  <navPoint id="n1a"><navLabel><text>One A</text></navLabel><content src="ch1.xhtml#a"/></navPoint>
</navPoint>
<navPoint id="n2"><navLabel><text>Two &amp; more</text></navLabel><content src="sub/ch2.xhtml"/></navPoint>
</navMap>
</ncx>"#;
        let points = parse_toc_ncx(ncx, "");
        assert_eq!(points.len(), 3);
        assert_eq!(points[0].label, "One");
        assert_eq!(points[0].file_href, "ch1.xhtml");
        assert_eq!(points[1].label, "One A");
        assert_eq!(points[1].fragment.as_deref(), Some("a"));
        assert_eq!(points[2].label, "Two & more");
        assert_eq!(points[2].file_href, "sub/ch2.xhtml");
        println!("  \u{2713} EPUB2 NCX navMap parsed: {} entries", points.len());
    }

    #[test]
    fn groups_by_spine_with_percent_encoding() {
        let points = vec![
            NavPoint {
                file_href: "text/第一章.xhtml".to_string(),
                fragment: None,
                label: "第一章".to_string(),
            },
            NavPoint {
                file_href: "text/ch2.xhtml".to_string(),
                fragment: Some("x".to_string()),
                label: "Two".to_string(),
            },
        ];
        let spine = vec![
            "text/%E7%AC%AC%E4%B8%80%E7%AB%A0.xhtml".to_string(),
            "text/ch2.xhtml".to_string(),
        ];
        let grouped = group_by_spine(&points, &spine);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].len(), 1);
        assert_eq!(grouped[0][0].label, "第一章");
        assert_eq!(grouped[1].len(), 1);
        assert_eq!(grouped[1][0].label, "Two");
        println!("  \u{2713} nav points grouped onto spine positions");
    }
}
