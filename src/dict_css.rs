//! Compile a dictionary's stylesheet into inline markup (issue #57).
//!
//! The Kindle lookup popup does not apply `<style>` blocks. A dictionary can
//! ship a perfectly good stylesheet and every entry still renders in the
//! reader's default face, which is what every kindling dictionary did until
//! this module existed.
//!
//! kindlegen works around it by resolving the rules at build time into legacy
//! presentational tags. Given `u { font-size: 260%; font-weight: bold; }` it
//! writes `<u><font size="+3"><b>UNDER</b></font></u>` into the text and ships
//! no stylesheet at all. Device-confirmed on a Paperwhite: text carrying a
//! literal `<font size="+3">` renders large in the popup, and the identical
//! text styled by a rule does not.
//!
//! So this takes the same approach. Only four properties are worth compiling,
//! because they are the ones with a legacy tag to compile *to*:
//!
//! | CSS | becomes |
//! |---|---|
//! | `font-size` | `<font size="+N">` |
//! | `font-weight: bold` | `<b>` |
//! | `font-style: italic` | `<i>` |
//! | `text-decoration: underline` | `<u>` |
//!
//! Only bare element selectors are compiled. That is not much of a limit
//! here: `class` attributes are stripped from entry HTML before this runs, so
//! a class selector could never have matched anything anyway, and dictionary
//! stylesheets are overwhelmingly written against element names and
//! `idx\:orth`.
//!
//! Declarations that are compiled are removed from the stylesheet that ships,
//! so a reader that *does* apply CSS cannot also apply the inline tag and
//! double the effect. Anything not compiled, `margin` and the like, stays in
//! the sheet untouched.

use std::borrow::Cow;
use std::collections::HashMap;

/// The presentational effect a rule asks for, reduced to what the popup can
/// actually render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Effect {
    /// Legacy relative font size, `-3` to `+3`. `None` leaves size alone.
    pub size: Option<i8>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

impl Effect {
    fn is_empty(&self) -> bool {
        self.size.is_none() && !self.bold && !self.italic && !self.underline
    }

    /// The markup to open and close around an element's contents. Closing
    /// order mirrors opening order so the result nests properly.
    fn markup(&self) -> (String, String) {
        let mut open = String::new();
        let mut close = String::new();
        if let Some(n) = self.size {
            open.push_str(&format!("<font size=\"{:+}\">", n));
            close.insert_str(0, "</font>");
        }
        if self.bold {
            open.push_str("<b>");
            close.insert_str(0, "</b>");
        }
        if self.italic {
            open.push_str("<i>");
            close.insert_str(0, "</i>");
        }
        if self.underline {
            open.push_str("<u>");
            close.insert_str(0, "</u>");
        }
        (open, close)
    }
}

/// What `compile` produced: the effects to inline, and the stylesheet left
/// over once the compiled declarations are taken out of it.
pub(crate) struct Compiled {
    pub effects: HashMap<String, Effect>,
    pub residual_css: String,
}

/// Map a CSS font-size to a legacy relative size.
///
/// Legacy sizes run 1 to 7 with 3 as the base, so the useful range either way
/// is three steps. Only ratios are handled: a percentage or an `em` is
/// relative to the surrounding text and can be converted, while `12pt` cannot,
/// because nothing here knows what the reader's base size is. The thresholds
/// put 260% at `+3`, which is what kindlegen emits for it.
fn size_delta(value: &str) -> Option<i8> {
    let v = value.trim().to_ascii_lowercase();
    let ratio = if let Some(pct) = v.strip_suffix('%') {
        pct.trim().parse::<f32>().ok()? / 100.0
    } else if let Some(em) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        em.trim().parse::<f32>().ok()?
    } else {
        match v.as_str() {
            "xx-large" => 2.0,
            "x-large" => 1.5,
            "large" | "larger" => 1.2,
            "small" | "smaller" => 0.83,
            "x-small" => 0.7,
            "xx-small" => 0.5,
            "medium" => 1.0,
            _ => return None,
        }
    };
    Some(match ratio {
        r if r >= 2.0 => 3,
        r if r >= 1.5 => 2,
        r if r >= 1.15 => 1,
        r if r <= 0.5 => -3,
        r if r <= 0.7 => -2,
        r if r <= 0.85 => -1,
        _ => return None,
    })
}

/// True when a selector is a single element name this can compile against.
///
/// `idx\:orth` counts: the backslash is CSS escaping for the colon in the
/// element's name, and the tag scanner reports that element as `idx:orth`.
/// Anything with a class, id, attribute, combinator or pseudo-selector is
/// left alone, because matching those properly means a real cascade.
fn selector_element(selector: &str) -> Option<String> {
    let s = selector.trim();
    if s.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(esc) => out.push(esc),
                None => return None,
            },
            c if c.is_ascii_alphanumeric() || c == '-' || c == '_' => out.push(c),
            _ => return None,
        }
    }
    if out.is_empty() || !out.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(out.to_ascii_lowercase())
}

/// Split a declaration block into `(property, value)` pairs, keeping the
/// original text of each so uncompiled ones can be put back verbatim.
fn declarations(block: &str) -> Vec<(String, String, String)> {
    block
        .split(';')
        .filter(|d| !d.trim().is_empty())
        .filter_map(|d| {
            let (name, value) = d.split_once(':')?;
            Some((
                name.trim().to_ascii_lowercase(),
                value.trim().to_string(),
                d.to_string(),
            ))
        })
        .collect()
}

/// Compile a stylesheet into inline effects plus whatever is left of it.
///
/// Later rules win over earlier ones for the same element, which is the
/// cascade's behavior for equal specificity, and every selector this compiles
/// has the same specificity.
pub(crate) fn compile(css: &str) -> Compiled {
    let mut effects: HashMap<String, Effect> = HashMap::new();
    let mut residual = String::new();

    for chunk in css.split('}') {
        let Some((selectors, block)) = chunk.split_once('{') else {
            // Trailing whitespace or a stray fragment; keep anything real.
            if !chunk.trim().is_empty() {
                residual.push_str(chunk);
            }
            continue;
        };
        // An at-rule brings its own nesting and cascade; leave it whole.
        if selectors.trim_start().starts_with('@') {
            residual.push_str(chunk);
            residual.push('}');
            continue;
        }

        let names: Vec<String> = selectors.split(',').filter_map(selector_element).collect();
        let compilable = names.len() == selectors.split(',').count() && !names.is_empty();

        if !compilable {
            residual.push_str(chunk);
            residual.push('}');
            continue;
        }

        let mut effect = Effect::default();
        let mut kept: Vec<String> = Vec::new();
        for (prop, value, original) in declarations(block) {
            let mut consumed = true;
            match prop.as_str() {
                "font-size" => match size_delta(&value) {
                    Some(d) => effect.size = Some(d),
                    None => consumed = false,
                },
                "font-weight" => {
                    let bold = value == "bold"
                        || value == "bolder"
                        || value.parse::<u32>().map(|n| n >= 600).unwrap_or(false);
                    if bold {
                        effect.bold = true;
                    } else {
                        consumed = false;
                    }
                }
                "font-style" => {
                    if value == "italic" || value == "oblique" {
                        effect.italic = true;
                    } else {
                        consumed = false;
                    }
                }
                "text-decoration" | "text-decoration-line" => {
                    if value.split_whitespace().any(|w| w == "underline") {
                        effect.underline = true;
                    } else {
                        consumed = false;
                    }
                }
                _ => consumed = false,
            }
            if !consumed {
                kept.push(original);
            }
        }

        if !effect.is_empty() {
            for name in &names {
                effects
                    .entry(name.clone())
                    .and_modify(|e| {
                        if effect.size.is_some() {
                            e.size = effect.size;
                        }
                        e.bold |= effect.bold;
                        e.italic |= effect.italic;
                        e.underline |= effect.underline;
                    })
                    .or_insert_with(|| effect.clone());
            }
        }
        // Put the rule back only if something in it was not compiled.
        if !kept.is_empty() {
            residual.push_str(selectors);
            residual.push('{');
            residual.push_str(&kept.join(";"));
            residual.push_str(";}");
        }
    }

    Compiled {
        effects,
        residual_css: residual,
    }
}

/// Compile the `<style>` blocks kindling collected from a dictionary's source.
///
/// Takes the blocks concatenated as they were gathered and gives back the
/// effects to inline plus the blocks that still have something in them. A
/// block whose every declaration was compiled is dropped, because an empty
/// stylesheet is worth no bytes.
pub(crate) fn compile_style_blocks(blocks: &str) -> Compiled {
    let mut effects: HashMap<String, Effect> = HashMap::new();
    let mut residual = String::new();
    let lower = blocks.to_ascii_lowercase();
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find("<style") {
        let tag_start = cursor + rel;
        // Anything between two blocks is not CSS; carry it through untouched.
        residual.push_str(&blocks[cursor..tag_start]);
        let Some(rel_open_end) = lower[tag_start..].find('>') else {
            break;
        };
        let open_end = tag_start + rel_open_end + 1;
        let Some(rel_close) = lower[open_end..].find("</style>") else {
            break;
        };
        let close = open_end + rel_close;

        let compiled = compile(&blocks[open_end..close]);
        merge_effects(&mut effects, compiled.effects);
        if !compiled.residual_css.trim().is_empty() {
            residual.push_str(&blocks[tag_start..open_end]);
            residual.push_str(&compiled.residual_css);
            residual.push_str("</style>");
        }
        cursor = close + "</style>".len();
    }
    residual.push_str(&blocks[cursor..]);

    Compiled {
        effects,
        residual_css: residual,
    }
}

/// Fold one stylesheet's effects into the running set, later winning.
fn merge_effects(into: &mut HashMap<String, Effect>, from: HashMap<String, Effect>) {
    for (name, effect) in from {
        into.entry(name)
            .and_modify(|e| {
                if effect.size.is_some() {
                    e.size = effect.size;
                }
                e.bold |= effect.bold;
                e.italic |= effect.italic;
                e.underline |= effect.underline;
            })
            .or_insert(effect);
    }
}

/// Wrap the contents of every matching element in the markup its rules ask
/// for.
///
/// Insertions go inside the element rather than around it, so an element that
/// carries an id stays where a link expects to find it. Nothing else in the
/// markup moves, which matters because dictionary entry offsets are measured
/// on these bytes afterwards.
pub(crate) fn apply<'a>(html: &'a str, effects: &HashMap<String, Effect>) -> Cow<'a, str> {
    if effects.is_empty() || html.is_empty() {
        return Cow::Borrowed(html);
    }
    // (offset, text) pairs, collected in document order.
    let mut inserts: Vec<(usize, String)> = Vec::new();
    let mut open_stack: Vec<(String, String)> = Vec::new();

    crate::links::for_each_tag(html, |tag, _attrs| {
        if tag.closing {
            if let Some(at) = open_stack.iter().rposition(|(n, _)| *n == tag.name) {
                let (_, close) = open_stack.remove(at);
                inserts.push((tag.start, close));
            }
            return;
        }
        if tag.self_closing {
            return;
        }
        let Some(effect) = effects.get(&tag.name) else {
            return;
        };
        let (open, close) = effect.markup();
        inserts.push((tag.end, open));
        open_stack.push((tag.name.clone(), close));
    });

    // Source markup that never closes an element would otherwise leave the
    // tags this opened hanging open into the rest of the book. Close them at
    // the end, innermost first.
    while let Some((_, close)) = open_stack.pop() {
        inserts.push((html.len(), close));
    }

    if inserts.is_empty() {
        return Cow::Borrowed(html);
    }
    // A stable sort keeps two insertions at the same offset in the order they
    // were found, which is what makes the nesting come out right.
    inserts.sort_by_key(|(at, _)| *at);

    let mut out = String::with_capacity(html.len() + inserts.len() * 12);
    let mut cursor = 0usize;
    for (at, text) in inserts {
        if at < cursor || at > html.len() {
            continue;
        }
        out.push_str(&html[cursor..at]);
        out.push_str(&text);
        cursor = at;
    }
    out.push_str(&html[cursor..]);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects_of(css: &str) -> HashMap<String, Effect> {
        compile(css).effects
    }

    #[test]
    fn compiles_the_four_properties_the_popup_honors() {
        let e = effects_of(
            "b { font-weight: bold; } i { font-style: italic; } \
             u { text-decoration: underline; } s { font-size: 200%; }",
        );
        assert!(e["b"].bold);
        assert!(e["i"].italic);
        assert!(e["u"].underline);
        assert_eq!(e["s"].size, Some(3));
    }

    #[test]
    fn maps_260_percent_to_plus_three_like_kindlegen() {
        // The one mapping with an oracle behind it: kindlegen turns
        // `font-size: 260%` into `<font size="+3">`.
        assert_eq!(size_delta("260%"), Some(3));
        assert_eq!(size_delta("150%"), Some(2));
        assert_eq!(size_delta("120%"), Some(1));
        assert_eq!(size_delta("50%"), Some(-3));
        assert_eq!(size_delta("0.7em"), Some(-2));
        assert_eq!(size_delta("2em"), Some(3));
        assert_eq!(size_delta("x-large"), Some(2));
        // Too close to the base to be worth a tag.
        assert_eq!(size_delta("105%"), None);
        // No way to know the base, so no way to convert.
        assert_eq!(size_delta("12pt"), None);
        assert_eq!(size_delta("16px"), None);
    }

    #[test]
    fn only_bare_element_selectors_compile() {
        assert_eq!(selector_element("b"), Some("b".to_string()));
        assert_eq!(
            selector_element(" idx\\:orth "),
            Some("idx:orth".to_string())
        );
        assert_eq!(selector_element("H2"), Some("h2".to_string()));
        for s in [
            ".big", "#id", "b.x", "p > b", "p b", "a:hover", "[data-x]", "*", "",
        ] {
            assert_eq!(selector_element(s), None, "{s:?} should not compile");
        }
    }

    #[test]
    fn a_selector_list_compiles_only_when_every_part_does() {
        let e = effects_of("b, strong { font-weight: bold; }");
        assert!(e["b"].bold && e["strong"].bold);
        // One unusable part disqualifies the whole rule.
        assert!(effects_of("b, .cls { font-weight: bold; }").is_empty());
    }

    #[test]
    fn wraps_element_contents_leaving_the_tag_alone() {
        let e = effects_of("u { font-size: 260%; font-weight: bold; }");
        // The `<u>` tag itself must not move: an id on it is a link target.
        assert_eq!(
            apply(r#"<u id="k">UNDER</u>"#, &e),
            r#"<u id="k"><font size="+3"><b>UNDER</b></font></u>"#
        );
    }

    #[test]
    fn closes_in_the_reverse_order_it_opened() {
        let e =
            effects_of("p { font-weight: bold; font-style: italic; text-decoration: underline; }");
        assert_eq!(apply("<p>x</p>", &e), "<p><b><i><u>x</u></i></b></p>");
    }

    #[test]
    fn handles_nesting_of_the_same_element() {
        let e = effects_of("b { font-style: italic; }");
        assert_eq!(
            apply("<b>a<b>c</b>d</b>", &e),
            "<b><i>a<b><i>c</i></b>d</i></b>"
        );
    }

    #[test]
    fn leaves_unmatched_and_self_closing_elements_alone() {
        let e = effects_of("b { font-weight: bold; }");
        assert_eq!(apply("<p>x</p><br/><b/>", &e), "<p>x</p><br/><b/>");
    }

    #[test]
    fn closes_what_the_source_left_open() {
        let e = effects_of("b { font-weight: bold; }");
        assert_eq!(apply("<b>x", &e), "<b><b>x</b>");
    }

    #[test]
    fn does_nothing_without_rules() {
        assert_eq!(apply("<b>x</b>", &HashMap::new()), "<b>x</b>");
    }

    #[test]
    fn compiled_declarations_leave_the_stylesheet() {
        // Otherwise a reader that does apply CSS would apply the rule and the
        // inline tag both, and double the effect.
        let c = compile("b { font-weight: bold; margin: 1em; }");
        assert!(c.effects["b"].bold);
        assert!(c.residual_css.contains("margin"));
        assert!(!c.residual_css.contains("font-weight"));
    }

    #[test]
    fn a_fully_compiled_rule_leaves_nothing_behind() {
        let c = compile("b { font-weight: bold; }");
        assert_eq!(c.residual_css.trim(), "");
    }

    #[test]
    fn rules_it_cannot_compile_survive_untouched() {
        for css in [
            ".headword { font-size: 200%; }",
            "p { margin: 0.3em 0; }",
            "@media screen { b { font-weight: bold; } }",
        ] {
            let c = compile(css);
            assert!(
                c.residual_css
                    .contains(css.split('{').next().unwrap().trim()),
                "{css:?} was dropped"
            );
        }
    }

    #[test]
    fn a_later_rule_wins_for_the_same_element() {
        let e = effects_of("b { font-size: 120%; } b { font-size: 200%; }");
        assert_eq!(e["b"].size, Some(3));
    }

    #[test]
    fn an_escaped_colon_selector_reaches_its_element() {
        // The rule kindling already moves to the end of the block for #39.
        let e = effects_of("idx\\:orth { font-weight: bold; }");
        assert_eq!(
            apply("<idx:orth>word</idx:orth>", &e),
            "<idx:orth><b>word</b></idx:orth>"
        );
    }

    #[test]
    fn survives_markup_that_would_break_a_regex() {
        let e = effects_of("b { font-weight: bold; }");
        let html = r#"<b title="a > b">x</b>"#;
        assert_eq!(apply(html, &e), r#"<b title="a > b"><b>x</b></b>"#);
    }
}
