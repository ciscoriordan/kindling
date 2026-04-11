/// HTML/XHTML self-check for MOBI text blobs.
///
/// Runs a set of relaxed well-formedness checks on the text content of a
/// MOBI before it is written to disk. The goal is to catch regressions like
/// `<hr/` corruption, unclosed attribute quotes, and dangling `<body>` /
/// `<mbp:frameset>` tags introduced by text-blob assembly bugs BEFORE they
/// reach a user's Kindle.
///
/// The checks here are intentionally relaxed: they use `quick_xml` in
/// non-strict mode (unchecked end names, unmatched ends allowed) so that
/// MOBI-specific markup like `<mbp:frameset>`, `<idx:entry>`, and
/// `<mbp:pagebreak/>` does not blow up the parser. The point is to detect
/// token-level corruption and gross tag imbalance, not to enforce schema
/// compliance.
///
/// The functions in this module are also used from `#[cfg(test)]` via thin
/// wrappers in `src/tests.rs`, so any behavior change should keep existing
/// HTML-validation tests passing.

use quick_xml::events::Event;
use quick_xml::Reader;

/// Void element names that are allowed to appear without a matching close
/// tag when walking MOBI HTML. Includes standard HTML voids used in book/
/// comic output (`<br/>`, `<img/>`, etc.) plus MOBI-specific empties.
pub const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link",
    "meta", "param", "source", "track", "wbr",
    // MOBI-specific empties
    "mbp:pagebreak",
    // XHTML processing tags occasionally seen empty in kindling output
    "guide",
];

/// Parse the given HTML/XHTML bytes with `quick_xml::Reader` in relaxed
/// mode. Returns `Ok(())` iff the parser reaches EOF without a hard syntax
/// error.
///
/// Kindling output mixes HTML-ish void tags (`<br/>`, `<img/>`) with
/// MOBI-specific markup (`<mbp:pagebreak/>`, `<mbp:frameset>`). Strict XML
/// parsing would reject namespace prefixes without xmlns declarations, so
/// we disable end-name matching and allow unmatched ends. The goal is to
/// catch token-level corruption (unclosed attributes, missing `>`, stray
/// null bytes), not to validate schema compliance.
pub fn parse_mobi_html(content: &[u8]) -> Result<(), String> {
    let content_str = std::str::from_utf8(content)
        .map_err(|e| format!("text blob is not valid UTF-8: {}", e))?;

    let mut reader = Reader::from_str(content_str);
    {
        let cfg = reader.config_mut();
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.check_comments = false;
        cfg.trim_text_start = false;
        cfg.trim_text_end = false;
    }
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => return Ok(()),
            Err(e) => {
                return Err(format!(
                    "XML parse error at byte {}: {}",
                    reader.buffer_position(),
                    e
                ));
            }
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// Walk the event stream of the given HTML bytes and verify that every
/// Start tag has a matching End, ignoring void elements listed in
/// `VOID_ELEMENTS`. Uses `check_end_names = false` so mismatched
/// namespaces don't blow up the walker, but we enforce our own stack
/// discipline. Returns `Err` with a descriptive message on mismatch.
pub fn check_balanced_tags(content: &[u8]) -> Result<(), String> {
    let content_str = std::str::from_utf8(content)
        .map_err(|e| format!("text blob is not valid UTF-8: {}", e))?;

    let mut reader = Reader::from_str(content_str);
    {
        let cfg = reader.config_mut();
        cfg.check_end_names = false;
        cfg.allow_unmatched_ends = true;
        cfg.check_comments = false;
        cfg.trim_text_start = false;
        cfg.trim_text_end = false;
    }

    let mut stack: Vec<String> = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                // Start-tag for a void element (e.g. `<br>` without `/`) is
                // treated as empty. kindling shouldn't emit these, but be
                // lenient.
                if VOID_ELEMENTS.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
                    // ignore
                } else {
                    stack.push(name);
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if VOID_ELEMENTS.iter().any(|v| v.eq_ignore_ascii_case(&name)) {
                    continue;
                }
                match stack.pop() {
                    Some(open) => {
                        if !open.eq_ignore_ascii_case(&name) {
                            return Err(format!(
                                "mismatched close </{}>, expected </{}>",
                                name, open
                            ));
                        }
                    }
                    None => {
                        return Err(format!(
                            "close </{}> with no matching open",
                            name
                        ));
                    }
                }
            }
            Ok(Event::Empty(_)) => {
                // self-closing tag - always balanced
            }
            Err(e) => {
                return Err(format!(
                    "walker parse error at byte {}: {}",
                    reader.buffer_position(),
                    e
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    if !stack.is_empty() {
        return Err(format!(
            "unclosed tags at EOF: {:?}",
            stack
        ));
    }
    Ok(())
}

/// Scan for malformed `<hr/` patterns where the `>` is missing or
/// clobbered, and for stray unclosed attribute quotes. Returns `Err` with
/// a descriptive message on the first issue found.
///
/// This is a hand-rolled byte-level scan rather than a regex walk so that
/// we get precise byte offsets for diagnostics and avoid the cost of regex
/// allocation on multi-MB blobs. The corruption patterns we care about are
/// specific past regressions, so an explicit scan is easier to reason
/// about.
pub fn check_no_corruption(content: &[u8]) -> Result<(), String> {
    // A correct self-closing hr is "<hr/>". Anything else after "<hr/"
    // (besides `>`) indicates corruption. Walk the byte buffer directly.
    let needle = b"<hr/";
    let mut i = 0;
    while i + needle.len() < content.len() {
        if &content[i..i + needle.len()] == needle {
            let next = content[i + needle.len()];
            if next != b'>' {
                let end = (i + 20).min(content.len());
                return Err(format!(
                    "malformed `<hr/` at byte {}: {:?}",
                    i,
                    String::from_utf8_lossy(&content[i..end])
                ));
            }
            i += needle.len() + 1;
        } else {
            i += 1;
        }
    }

    // Count `="` openers and find their matching closing `"` within a
    // reasonable window. kindling attribute values should never span
    // more than ~4 KiB (long style or class lists notwithstanding).
    //
    // For each `="`, find the next `"` and assert it exists within the
    // next 4096 bytes AND before the next `<`.
    let bytes = content;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'=' && bytes[i + 1] == b'"' {
            let start = i + 2;
            let window_end = (start + 4096).min(bytes.len());
            let mut found = false;
            for j in start..window_end {
                if bytes[j] == b'"' {
                    found = true;
                    i = j + 1;
                    break;
                }
                if bytes[j] == b'<' {
                    // Next tag starts before we found a closing quote.
                    let end = (i + 60).min(bytes.len());
                    return Err(format!(
                        "unclosed attribute quote at byte {}: {:?}",
                        i,
                        String::from_utf8_lossy(&bytes[i..end])
                    ));
                }
            }
            if !found {
                let end = (i + 60).min(bytes.len());
                return Err(format!(
                    "unclosed attribute quote (no `\"` within 4096 bytes) at byte {}: {:?}",
                    i,
                    String::from_utf8_lossy(&bytes[i..end])
                ));
            }
        } else {
            i += 1;
        }
    }

    Ok(())
}

/// Run the full HTML self-check on a MOBI text blob.
///
/// Returns a `Vec<String>` of all issues found. An empty vec means the
/// blob passed every check. This function never panics: UTF-8 errors and
/// parse errors are converted into strings and returned.
///
/// The three checks are:
///   1. `parse_mobi_html` - relaxed quick_xml parse (catches token-level
///      corruption like missing `>`, unclosed attrs, stray nulls).
///   2. `check_no_corruption` - byte scan for known-bad patterns
///      (`<hr/X`, unclosed attribute quotes near tag boundaries).
///   3. `check_balanced_tags` - stack walk verifying that every open tag
///      has a matching close, ignoring void elements.
///
/// Note: structural-tags-present is NOT included here. It asserts that
/// `<html>`/`<body>` substrings exist, which is a stronger requirement
/// than what we want to enforce at build time (e.g. very small test
/// fixtures may not include them).
pub fn validate_text_blob(blob: &[u8]) -> Vec<String> {
    let mut errors = Vec::new();

    if let Err(e) = parse_mobi_html(blob) {
        errors.push(format!("HTML parse: {}", e));
    }

    if let Err(e) = check_no_corruption(blob) {
        errors.push(format!("corruption scan: {}", e));
    }

    if let Err(e) = check_balanced_tags(blob) {
        errors.push(format!("tag balance: {}", e));
    }

    errors
}

/// Validate that each record, when decoded independently, has balanced
/// HTML tag state. A Kindle reader decodes each text record in isolation
/// for HTML parsing and pagination, so a record that opens `<b>` without
/// a matching `</b>` causes bold state to leak for the rest of the
/// record, and a record ending inside a tag leaves garbage at its start.
///
/// `records` is an iterator of `(start, end)` byte offsets into `blob`,
/// which is the pre-compression text. Returns human-readable issue
/// strings for every record that fails balance. Reports up to
/// `max_issues` entries to keep the error list bounded.
pub fn validate_records(
    blob: &[u8],
    records: &[(usize, usize)],
    max_issues: usize,
) -> Vec<String> {
    let mut issues = Vec::new();
    let mut unbalanced_b = 0usize;
    let mut unbalanced_i = 0usize;
    let mut unbalanced_p = 0usize;
    let mut unbalanced_h5 = 0usize;
    let mut mid_tag = 0usize;

    for (idx, &(s, e)) in records.iter().enumerate() {
        if e > blob.len() || s > e {
            continue;
        }
        let rec = &blob[s..e];

        // Per-tag balance scan (cheap substring count).
        let rec_str = match std::str::from_utf8(rec) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for (tag, counter) in [
            ("b", &mut unbalanced_b),
            ("i", &mut unbalanced_i),
            ("p", &mut unbalanced_p),
            ("h5", &mut unbalanced_h5),
        ] {
            let opens = rec_str.matches(&format!("<{}>", tag)).count() as i32;
            let closes = rec_str.matches(&format!("</{}>", tag)).count() as i32;
            if opens != closes {
                *counter += 1;
                if issues.len() < max_issues {
                    issues.push(format!(
                        "record {} ({}..{}): <{}> unbalanced (opens={}, closes={})",
                        idx, s, e, tag, opens, closes
                    ));
                }
            }
        }

        // Mid-tag scan: does the record end with an unclosed `<`?
        if record_ends_in_tag(rec) {
            mid_tag += 1;
            if issues.len() < max_issues {
                let tail_start = rec.len().saturating_sub(40);
                let tail = String::from_utf8_lossy(&rec[tail_start..]);
                issues.push(format!(
                    "record {} ({}..{}): ends inside an HTML tag, tail={:?}",
                    idx, s, e, tail
                ));
            }
        }
    }

    if unbalanced_b + unbalanced_i + unbalanced_p + unbalanced_h5 + mid_tag > 0 {
        issues.push(format!(
            "summary: {}/{} records unbalanced-<b>, {} unbalanced-<i>, {} unbalanced-<p>, {} unbalanced-<h5>, {} mid-tag",
            unbalanced_b, records.len(), unbalanced_i, unbalanced_p, unbalanced_h5, mid_tag
        ));
    }

    issues
}

/// Return true if the record byte slice ends inside an HTML tag
/// (an unclosed `<` with no matching `>` before end).
fn record_ends_in_tag(rec: &[u8]) -> bool {
    let mut in_tag = false;
    for &b in rec {
        if b == b'<' {
            in_tag = true;
        } else if b == b'>' {
            in_tag = false;
        }
    }
    in_tag
}

/// Print a user-facing warning block for self-check issues and advise how
/// to suppress. Used by `mobi::build_*` when `self_check` is enabled and
/// `validate_text_blob` returns a non-empty list.
///
/// This does NOT abort the build: the caller still writes the MOBI.
pub fn print_self_check_warnings(issues: &[String]) {
    eprintln!("Warning: MOBI output self-check found issues:");
    for issue in issues {
        eprintln!("  - {}", issue);
    }
    eprintln!(
        "These may indicate a kindling bug. Please report at \
         https://github.com/ciscoriordan/kindling/issues"
    );
    eprintln!(
        "The MOBI will still be written; use --no-self-check to suppress these warnings."
    );
}
