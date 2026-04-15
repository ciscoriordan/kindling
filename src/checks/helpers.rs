// Shared helpers used by validator checks.

use std::path::Path;

/// Extract `attr="value"` from a tag body. Only handles double-quoted values.
pub fn extract_attr(tag: &str, attr: &str) -> Option<String> {
    let needle = format!("{}=\"", attr);
    let start = tag.find(&needle)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Try parsing `content` as XML. Returns Err with a descriptive message.
pub fn try_parse_xml(content: &str) -> Result<(), String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(content);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => return Ok(()),
            Err(e) => return Err(format!("{}", e)),
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// True if `line` contains an opening tag `<name`. Case-insensitive.
pub fn contains_tag(line: &str, name: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let needle_open = format!("<{}", name);
    let mut search = lower.as_str();
    while let Some(idx) = search.find(&needle_open) {
        let after = &search[idx + needle_open.len()..];
        if let Some(c) = after.chars().next() {
            if c == ' ' || c == '>' || c == '/' || c == '\t' || c == '\n' {
                return true;
            }
        } else {
            return true;
        }
        search = &search[idx + needle_open.len()..];
    }
    false
}

/// 1-based line number of `byte_offset` within `content`.
pub fn line_of(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset.min(content.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
        + 1
}

/// Find nested `<p>` tags. Returns the 1-based line of the offender.
pub fn find_nested_p(content: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut pos = 0usize;
    let lower = content.to_ascii_lowercase();

    while pos < lower.len() {
        let rest = &lower[pos..];
        let open = rest.find("<p");
        let close = rest.find("</p>");
        match (open, close) {
            (Some(o), Some(c)) if o < c => {
                let after_p = rest.as_bytes().get(o + 2).copied().unwrap_or(b' ');
                if after_p == b' ' || after_p == b'>' || after_p == b'\t' || after_p == b'/' {
                    if depth >= 1 {
                        let abs = pos + o;
                        return Some(line_of(content, abs));
                    }
                    depth += 1;
                }
                pos += o + 2;
            }
            (Some(o), None) => {
                let after_p = rest.as_bytes().get(o + 2).copied().unwrap_or(b' ');
                if after_p == b' ' || after_p == b'>' || after_p == b'\t' || after_p == b'/' {
                    if depth >= 1 {
                        let abs = pos + o;
                        return Some(line_of(content, abs));
                    }
                    depth += 1;
                }
                pos += o + 2;
            }
            (_, Some(c)) => {
                if depth > 0 {
                    depth -= 1;
                }
                pos += c + 4;
            }
            (None, None) => break,
        }
    }
    None
}

/// Detect common negative CSS values for margin/padding/line-height.
pub fn has_negative_css(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    for prop in &["margin", "padding", "line-height"] {
        let mut search = l.as_str();
        while let Some(idx) = search.find(prop) {
            let after = &search[idx + prop.len()..];
            let colon = after.find(':');
            if let Some(c) = colon {
                let value = after[c + 1..].trim_start();
                let bytes = value.as_bytes();
                if bytes.len() >= 2 && bytes[0] == b'-' && bytes[1].is_ascii_digit() {
                    return true;
                }
                let end = value
                    .find(|c: char| c == ';' || c == '"' || c == '}')
                    .unwrap_or(value.len());
                let vals = &value[..end];
                let chars: Vec<char> = vals.chars().collect();
                for i in 0..chars.len().saturating_sub(1) {
                    if chars[i] == '-' && chars[i + 1].is_ascii_digit() {
                        let prev = if i == 0 { ' ' } else { chars[i - 1] };
                        if prev == ' ' || prev == ':' || prev == '\t' {
                            return true;
                        }
                    }
                }
            }
            search = &search[idx + prop.len()..];
        }
    }
    false
}

/// Returns the heading tag name if `line` has a heading with explicit text-align.
pub fn heading_with_text_align(line: &str) -> Option<&'static str> {
    let l = line.to_ascii_lowercase();
    let tags: &[&'static str] = &["h1", "h2", "h3", "h4", "h5", "h6"];
    for tag in tags {
        let open = format!("<{}", tag);
        if let Some(idx) = l.find(&open) {
            if let Some(end) = l[idx..].find('>') {
                let tag_content = &l[idx..idx + end];
                if tag_content.contains("text-align") {
                    return Some(tag);
                }
            }
        }
    }
    None
}

/// Count `<tr>` rows per `<table>`. Returns one entry per table.
pub fn count_table_rows(content: &str) -> Vec<usize> {
    let lower = content.to_ascii_lowercase();
    let mut counts = Vec::new();
    let mut pos = 0usize;
    while let Some(start) = lower[pos..].find("<table") {
        let abs = pos + start;
        let end = match lower[abs..].find("</table>") {
            Some(e) => abs + e,
            None => break,
        };
        let table = &lower[abs..end];
        let row_count = table.matches("<tr").count();
        counts.push(row_count);
        pos = end + 8;
    }
    counts
}

/// Rough approximation of text length, stripping HTML tags.
pub fn strip_tags_len(html: &str) -> usize {
    let mut in_tag = false;
    let mut count = 0usize;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => count += 1,
            _ => {}
        }
    }
    count
}

/// True if this spine item looks like an HTML cover page.
pub fn looks_like_html_cover_page(
    base_dir: &Path,
    idref: &str,
    href: &str,
    cover_image_href: &str,
) -> bool {
    let href_lower = href.to_lowercase();
    let idref_lower = idref.to_lowercase();
    let file_stem = Path::new(&href_lower)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    let is_html = href_lower.ends_with(".html")
        || href_lower.ends_with(".xhtml")
        || href_lower.ends_with(".htm");
    if !is_html {
        return false;
    }

    let name_match = idref_lower.contains("cover") || file_stem.contains("cover");
    if !name_match {
        return false;
    }

    let full_path = base_dir.join(href);
    let content = match std::fs::read_to_string(&full_path) {
        Ok(s) => s,
        Err(_) => return true,
    };

    let image_stem = Path::new(cover_image_href)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    content.contains("<img") && (image_stem.is_empty() || content.contains(image_stem))
}
