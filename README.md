# Kindling

<img width="100%" alt="Kindling - The missing MOBI generator. Dictionaries, books, comics." src="https://raw.githubusercontent.com/ciscoriordan/kindling/main/images/kindling_social.jpg">

The missing Kindle toolkit. Dictionaries, books, and comics. Single static Rust binary, no dependencies, cross-platform.

[![Crates.io](https://img.shields.io/crates/v/kindling-mobi.svg)](https://crates.io/crates/kindling-mobi) [![docs.rs](https://img.shields.io/docsrs/kindling-mobi)](https://docs.rs/kindling-mobi) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Amazon deprecated *kindlegen* in 2020, leaving no supported way to build Kindle MOBIs. The only remaining copy is buried inside Kindle Previewer 3's GUI, can't run headless, and can take 12+ hours (or run out of memory entirely) for large dictionaries due to x86-only Rosetta 2 emulation on Apple Silicon, superlinear inflection index computation, and a 32-bit Windows build that crashes on large files. Kindling builds the same dictionary in 6 seconds.

For comics, [KCC](https://github.com/ciromattia/kcc) exists but requires Python, PySide6/Qt, Pillow, 7z, mozjpeg, psutil, pymupdf, and more. Installation is painful across platforms, there's no headless mode for CI, and Python image processing is slow. Kindling replaces all of that with a single statically-linked native binary, compiled from Rust.

Kindling also publishes dictionaries to StarDict (`kindling stardict`), producing the four-file `.ifo` / `.idx` / `.dict` / `.syn` bundle that [GoldenDict](http://goldendict.org/), [GoldenDict-ng](https://github.com/xiaoyifang/goldendict-ng), [KOReader](https://github.com/koreader/koreader), [sdcv](https://github.com/Dushistov/sdcv), and other non-Kindle dictionary readers consume. The same OPF or EPUB you pass to `kindling build` is the input, so one dictionary project can target Kindle, Linux/macOS/Windows desktops, Android, and Kobo/PocketBook e-readers from a single source. Headword lookup, inflection lookup, and case-insensitive matching all work without configuration; see [StarDict export](#stardict-export) for format details and current cross-reference caveats.

Kindling was built by reverse-engineering Amazon's undocumented MOBI format, with help from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI).

Pre-built binaries for Mac (Apple Silicon, Intel), Linux (x86_64), and Windows (x86_64): [Releases](https://github.com/ciscoriordan/kindling/releases)

<p align="center">
  <img width="400" alt="Greek dictionary lookup on Kindle" src="https://raw.githubusercontent.com/ciscoriordan/kindling/main/images/kindle_test.jpg">
  <img width="400" alt="Pepper & Carrot comic on Kindle" src="https://raw.githubusercontent.com/ciscoriordan/kindling/main/images/kindle_comic_test.jpg">
</p>

## Features

- **Dictionaries**: Full orth index with headword + inflection lookup, ORDT/SPL sort tables, generated CJK and Arabic collation tables, fontsignature, and the EXTH subject record that makes the device list the file as a dictionary in the lookup popup
- **Dictionary styling**: a dictionary's CSS is picked up from every dictionary file, and the rules that can be expressed as legacy inline markup are compiled into the entries at build time, which is what kindlegen does: `font-size` becomes `<font size="+N">`, bold/italic/underline become `<b>`/`<i>`/`<u>` (issue #57)
- **Books**: EPUB or OPF input, embedded images, embedded fonts (with IDPF/Adobe deobfuscation), hierarchical on-device TOC from the EPUB nav document (toc.ncx / nav.xhtml, including `file#anchor` entries and nested volume/chapter levels), user font switching kept working by stripping font-family from stylesheets, `<style>` blocks, and inline `style="..."` attributes when no fonts are embedded (`--force-user-fonts` to strip always), KF8-only (.azw3) by default with legacy dual-format (MOBI7+KF8) available via `--legacy-mobi`, which is also what restores the sideloaded library cover (issue #20), HD image container, fixed-layout support
- **Comics**: Image folder, CBZ, CBR, or EPUB input, device-specific downscaling (a page already smaller than the profile is left alone rather than enlarged, matching kindlegen), spread splitting, margin cropping, auto-contrast, moire correction for color e-ink, manga RTL, webtoon with overlap fallback, Panel View, KF8-only (.azw3) by default (`--legacy-mobi` for a dual `.mobi` so sideloaded library covers show), metadata overrides
- **StarDict export**: `kindling stardict` builds a four-file StarDict bundle (`.ifo` / `.idx` / `.dict` / `.syn`) from the same OPF or EPUB dictionary input as `kindling build`, for use with GoldenDict, GoldenDict-ng, KOReader, sdcv, and other non-Kindle dictionary readers (see [StarDict export](#stardict-export))
- **EPUB export**: `kindling epub2` and `kindling epub3` build a reflowable EPUB from the same OPF or EPUB input, conformant to EPUB 2.0.1 and EPUB 3.3 respectively (epubcheck-clean), carrying the book's images into the archive and rewriting cross-document links to the names the export gives its own output files (issue #55). EPUB2 is always a plain book; EPUB3 is a plain book by default and emits an EPUB Dictionaries and Glossaries layer (Search Key Map, `dc:type=dictionary`, `epub:type` semantics) when the input is a dictionary (see [EPUB export](#epub-export))
- **EPUB repair**: `kindling repair` applies a small, byte-stable, idempotent set of structural fixes to an EPUB for cleaner Send-to-Kindle ingest (see [Repair](#repair))
- **Metadata rewrite**: `kindling rewrite-metadata` updates title, authors, publisher, description, language, ISBN, ASIN, publication date, tags, cover image, and the device content type on an existing MOBI/AZW3 in place without rebuilding from source. Byte-stable on no-op, idempotent, refuses DRM files (see [Rewrite metadata](#rewrite-metadata))
- **Structural dump**: `kindling dump` prints the parsed structure of a MOBI/AZW3 (PalmDB, MOBI header, EXTH, INDX/ORDT tables, entry labels) as line-oriented `section.field = value` output, so two dumps can be compared with `diff` (see [Dump](#dump))
- **Lookup simulator**: `kindling lookup <dict.mobi> <word>` predicts which headword a Paperwhite 5 opens when the word is tapped: it formats the word as the Kindle does, searches the index in its stored order, picks one of the labels the search returns, and names any headword the index order hides (see [Lookup simulator](#lookup-simulator))
- **Reads huffdic (`-c2`) files**: text compressed with HUFF/CDIC (PalmDOC compression type 17480), which is what `kindlegen -c2` and every Amazon store dictionary use, is decompressed by [`src/huffcdic.rs`](src/huffcdic.rs), so `dump` reports the compression model and the bytes it decodes to instead of treating those records as opaque. kindling can also write it, behind `KINDLING_HUFFDIC=1` (issue #49)
- **Build-time HTML self-check**: every `build` runs a two-pass HTML check on the assembled MOBI text blob, following tag nesting across the whole text rather than inside each record and checking that no record ends part-way through a tag, catching regressions like dangling tags, `<hr/` corruption, and unclosed attribute quotes (see [Build-time self-check](#build-time-self-check))
- **UTF-8 and tag-safe record splitter**: every text record is exactly the declared record size, which the firmware relies on to route popup lookups, and the bytes that would otherwise straddle a record end are pushed into the next record by padding the last gap between two tags with spaces, so no record ends inside a multi-byte character or a tag
- Drop-in *kindlegen* replacement (same CLI flags, same status codes)
- Kindle Previewer compatible (EPUB source embedded by default)
- Usable as both a CLI (`kindling-cli`) and a Rust library crate (`kindling`) with a public API for external consumers (see `src/lib.rs`)
- Test suite with CI on every push (see [Testing](#testing))

## Installation

Download the latest release for your platform from [Releases](https://github.com/ciscoriordan/kindling/releases):

- **Mac Apple Silicon** - `kindling-cli-mac-apple-silicon`
- **Mac Intel** - `kindling-cli-mac-intel`
- **Linux** - `kindling-cli-linux`
- **Windows** - `kindling-cli-windows.exe`

The Mac binaries are signed with a Developer ID and notarized, so they run as
downloaded. Releases up to and including v0.28.0 were unsigned; if you are on one
of those and macOS refuses to run it with "Apple could not verify ... is free of
malware", clear the quarantine flag with `xattr -d com.apple.quarantine kindling-cli-mac-apple-silicon`
or upgrade to a later release. Binaries obtained via `cargo install`, AppMan, AM,
or `curl` are never quarantined and are unaffected either way.

Then mark it executable and put it somewhere on your `PATH`:

```bash
chmod +x kindling-cli-mac-apple-silicon
mv kindling-cli-mac-apple-silicon /usr/local/bin/kindling-cli
```

On Linux and BSD, install via [AppMan](https://github.com/ivan-hc/AppMan) (rootless, per-user):

```bash
appman -i kindling-cli
```

Or via [AM](https://github.com/ivan-hc/AM) (system-wide):

```bash
am -i kindling-cli
```

Or install via Cargo (builds from source, installs `kindling-cli` to `~/.cargo/bin`):

```bash
cargo install kindling-mobi
```

Or build from source. Kindling uses Rust edition 2024 and requires Rust 1.85 or newer. Run from the repo root:
```bash
cargo build --release
```

The binary is written to `target/release/kindling-cli`.

### As a library

Add the crate to your `Cargo.toml` (published as `kindling-mobi`; the library name is `kindling`):

```toml
[dependencies]
kindling-mobi = "0.43"
```

Then `use kindling::...`. Public API is defined in `src/lib.rs`.

## Usage

The examples below assume `kindling-cli` is on your PATH (see [Installation](#installation)). If it is not, call it by its full path instead.

### Dictionaries

```bash
kindling-cli build input.opf -o output.mobi
kindling-cli build input.opf -o output.mobi --no-compress    # skip compression for fast dev builds
kindling-cli build input.opf -o output.mobi --headwords-only  # index headwords only (no inflections)
kindling-cli build input.opf -o output.mobi --no-kindle-limits  # skip the 30 MB section split
kindling-cli build input.opf -o output.mobi --no-validate     # skip KDP pre-flight validation
kindling-cli build input.opf -o output.mobi --fold-accents    # plain UTF-16 labels with the kindlegen-derived ORDT/SPL blob (see below)
kindling-cli build input.opf -o output.mobi --strict-accents  # exact per-character table for any script (see below)
KINDLING_FOLD_ACCENTS=1 kindling-cli build input.opf -o output.mobi  # --fold-accents for wrappers that can't pass the flag (pyglossary/reader.dict)
kindling-cli lookup output.mobi rivière   # simulate the on-device lookup of a word
```

The input OPF must reference HTML files with `<idx:entry>`, `<idx:orth>`, and `<idx:iform>` markup following the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf). Both headwords and inflected forms are indexed so that looking up any form on the Kindle finds the correct dictionary entry.

A dictionary's CSS is picked up from every dictionary file, whether it sits in an inline `<style>` block or an external `<link rel="stylesheet">`. The Kindle lookup popup applies no stylesheet at all, so the rules that can be expressed as legacy inline markup are compiled into it at build time, which is what kindlegen does: `font-size` becomes `<font size="+N">`, `font-weight: bold` becomes `<b>`, `font-style: italic` becomes `<i>`, and `text-decoration: underline` becomes `<u>` (issue #57). Only bare element selectors are compiled (`u`, `p`, `idx\:orth`), and only a `font-size` given as a ratio, since a `12pt` cannot be resolved without knowing the reader's base size. A declaration that compiles is removed from the stylesheet that ships, so a reader that does apply CSS cannot apply the rule and the inline tag both; everything else stays in the sheet, with escaped-colon selectors (`idx\:orth`) emitted last because the popup's CSS parser discards everything after one (issue #39). Note that `class=` attributes are stripped from entry HTML, deliberately, so class selectors never match a dictionary entry however the sheet arrives.

A dictionary's cross-references are resolved too, not just a book's (issue #54). The validator knows the same convention: `R9.3` counts `hw_<headword>` as a declared anchor for any headword the file has, because otherwise it calls a link the builder resolves correctly a dangling fragment and aborts the build. An `<a href>` inside an entry becomes a `filepos` byte offset, the only thing a MOBI6 reader follows, and resolution is per source file: a bare `#word` means that fragment in the file the link was written in, so two files that each define `#dup` reach their own copy rather than whichever came first. That matches kindlegen, and resolving book-wide would produce a live link to the wrong definition, which is worse than a dead one. The fragments dictionaries actually use mostly do not survive as anchors, because PyGlossary and reader.dict write the entry's anchor as `id="hw_<headword>"` on the `<idx:entry>` element and stripping that element for the MOBI text takes the id with it; each entry's own start offset stands in for its `hw_<headword>`, so the link lands on the entry the fragment names. kindlegen has the same hole and simply drops those links. An anchor that did survive inside an entry body wins over the fallback, and a fragment naming a headword the dictionary does not have keeps the same-width inert marker.

An entry's headword may be an attribute (`<idx:orth value="word"/>`) or the element's own text (`<idx:orth>word</idx:orth>`), and the entry body may be shaped however you like. kindling locates each entry by the bytes it contributed to the text blob, so an entry whose body never repeats its own headword, or wraps it in `<p>`/`<h1>`/`<span>` rather than `<b>`, still gets a correct lookup span. Before 0.32.0 only `<b>`- or `<big>`-wrapped headwords at the very start of an entry were found; anything else was stored as a zero-length span and popped up blank on device, and each miss cost a scan of the whole blob, which made large builds quadratic (issue #27).

Text that sits outside an `<idx:entry>` is kept rather than discarded (issue #42). Letter headings, a usage note between two entries, an image before the first one, a closing credits paragraph, and the body of any `<idx:entry>` the parser rejects all reach the book now; before, they were dropped with no warning and exit 0. Each run is replayed where it was, with any separator trimmed off its front so it does not draw a second horizontal rule beside the one every entry already ends with, and the entry indexer steps over it so a heading cannot take the anchor belonging to the entry after it. A file that carries `idx:` markup but yields no usable entry has no entry to attach its text to and is read as front matter instead of skipped.

Every entry closes with a horizontal rule, which the guidelines ask for, and a page break, which is what Amazon's own dictionaries put between entries. Without the page break the lookup popup was reported to scroll past the end of the matched entry into the next one (issue #53, pull request #52); the same change means that when the dictionary is opened as a book, each entry starts on its own page. kindling adds the page break itself, so a source whose entries are separated only by `<hr/>` (which is what PyGlossary writes) gets it too; kindlegen only keeps a page break the source already has. A page break the source wrote between two entries is now part of the run between them, and is trimmed away there so the two do not stack.

Headwords may be wrapped in either `<b>` or `<big>`. PyGlossary picks the wrapper by writing system and uses `<big>` for Hangul, CJK, Devanagari, Armenian, Bengali, Burmese and Greek, so dictionaries built through it (including reader.dict's) rely on the `<big>` path (issue #22).

If an entry cannot be located in the text blob, its lookup resolves to a zero-length body and renders as a blank popup on device. kindling warns about this and still writes the dictionary. Set `KINDLING_STRICT_ENTRIES=1` to abort the build instead, which is the right setting for CI or any caller that checks exit codes. It is off by default because the wrappers most likely to hit this do not check exit codes, so aborting leaves them with no output file at all, which is a worse outcome than a dictionary with some blank entries. 0.29.0 briefly had this the other way round and regressed reader.dict from blank definitions to no working dictionary; 0.29.1 restored the warning.

If the OPF references a cover image (Method 1 `<item properties="cover-image"/>` or Method 2 `<meta name="cover">`), Kindling embeds it in the dictionary MOBI via EXTH 201 so it shows up on the Kindle home screen next to regular books and comics.

Images referenced from entry HTML via `<img src="..."/>` but not declared in the OPF manifest are also embedded automatically. PyGlossary and other OEB 1.x-era tools commonly emit manifests that omit inline glyph GIFs referenced from within `<idx:entry>` blocks; kindlegen silently picks these up, and kindling matches that behavior so the glyphs render on device.

A PalmDB header counts its records in 16 bits, so a book cannot hold more than 65535 of them in total: text records, image records and index records all count toward the same ceiling. Above it the count wrapped and the file said `total mod 65536`, which left every record offset correct and the reader able to see only a fraction of the book, reading the last visible record as everything to the end of the file. A build that crosses the limit is now refused before anything is written, so there is no plausible-looking file left at the output path, and `kindling-cli check` recognizes a file whose count already wrapped by comparing it against what the record list actually holds (issue #47). Note that `--legacy-mobi` stores the text twice and so roughly halves what fits.

Setting `KINDLING_HUFFDIC=1` compresses a dictionary's text with HUFF/CDIC rather than PalmDOC (issue #49). PalmDOC can only reach back 2047 bytes for a match, while a huffdic phrase dictionary is shared by the whole book, which is why every Amazon-published dictionary uses it; on an 8000-entry test dictionary the file goes from 1.03 MB to 0.55 MB, and on Webster's Unabridged 1913, 98842 entries and 28 MB of text, from 15.7 MB to 8.8 MB. The phrase dictionary is not chosen in one pass: the encoder parses the book, prices every phrase at what it measurably saves against the code it was given, offers itself the merges that parse asked for, re-selects, and repeats until the output stops shrinking, which on Webster is about twenty rounds and seven seconds. Which one wins depends on the book, so both are built and the smaller kept: entries that are near-duplicates of their neighbors suit PalmDOC's window very well and huffdic loses on them. The encoder also declines whenever it cannot decode its own output, so a dictionary that would not round trip never reaches a file. It is off by default because no Kindle has yet opened a huffdic file kindling wrote. Mobipocket Reader for Windows does open one, Webster's Unabridged at full size included, which took a `DATP` record the first version did not write (see the huffdic format notes below). Verified here against three decoders, two of them nothing to do with kindling: kindling's own, an independent implementation written from the published algorithm, and calibre, whose full conversion pipeline reads a huffdic build to the same bytes as the PalmDOC build of the same source.

By default, dictionaries enforce Kindle publishing limits (`--kindle-limits`): entries are split into HTML sections under 30 MB each, and a warning is printed if the total exceeds 300 sections. `--no-kindle-limits` turns off the splitting and the warning, and nothing else. Until 0.36.0 it also swapped in a different text assembler, which silently cost the build its per-entry index offsets and its stylesheets (issue #41).

Every `build` also runs the Kindle Publishing Guidelines validator as an automatic pre-flight step before writing the MOBI. If validation reports any errors, the build is aborted with exit code 1; warnings are printed but do not block the build. Pass `--no-validate` to skip the check. See [Validation](#validation) for the full list of rules.

#### How a Kindle finds a headword

A Kindle looks up a tapped word by binary-searching the dictionary index in the order the labels are stored. It gives each character of the word, and of every stored label, a weight from one fixed table, whatever collation tables the file carries. The Kindle 4 (firmware 4.1.4), the Paperwhite 4 (firmware 5.16.5), the Paperwhite 5 (firmware 5.19.2) and firmware 5.19.6 all use the same weights. kindling sorts the labels of every dictionary by these weights, except in Japanese, Chinese, Korean and Arabic-script dictionaries, which kindling still sorts by the generated tables it writes for them, as 0.45.1 did (see below). When labels are stored out of this order, the search can step past a label, and not only past the misplaced one: labels next to it that are in their right place can be hidden too. kindling 0.45.1 sorted a hyphen or an apostrophe as a low letter, and sorted the labels of Greek and Cyrillic dictionaries by code unit, which kept their letters in order but misplaced punctuation, superscript digits and Latin letters (`из-под` hid `изба`, `Zebra` hid `apple`). reader.dict's Italian dictionary, rebuilt from its source by 0.45.1, opened nothing on the Paperwhite 4 for 886 of its 307,581 headwords and inflected forms, among them `posta`, `-a` and `'ndrangheta`. Built by this version, it opens nothing for 19 of them, all words a Kindle does not look up whole: `007` is formatted to nothing, and `MP3` and `MP4` are both looked up as `MP` (see the aliases below).

The weights are these:

- A Latin letter weighs as its lowercase base letter, so its case and accents do not count. This covers every letter from `À` to `ſ` except `ß`, `æ`, `Æ`, `œ`, `Œ`, `ĳ` and `Ĳ` (see below), the micro sign `µ`, about half of Latin Extended-B and most of the IPA letters: `é` weighs as e, `ł` as l, `ð` as d, `ø` as o, `þ` as t, `ı` as i, `ſ` as s, `µ` as m, `ƒ` as f, and Vietnamese `ơ` and `ư` as o and u.
- Some characters weigh nothing, in a label and in a tapped word alike: the control characters; the ASCII punctuation and symbols (hyphen, apostrophe, period, slash, `$`, `+` and the rest); the Latin-1 characters from `¡` to `¿` except `µ` and the superscript digits (among them `«`, `·`, `£`, `©`, the soft hyphen, `ª`, `º` and the fractions), and `×` and `÷`; everything from U+02B0 to U+02FF (the Ukrainian apostrophe U+02BC, the Hawaiian ʻokina U+02BB, the IPA stress and length marks, and spacing accents such as `˘` and `˜`); the General Punctuation block (dashes, curly quotes, the ellipsis, the typographic spaces, the zero-width and direction marks); everything from U+2190 to U+23FF, U+2500 to U+27FF and U+2900 to U+2BFF (arrows, mathematical operators such as `−`, `≤` and `∞`, technical symbols, box drawing, geometric shapes, symbols such as `♥`, `♯` and `✓`, and dingbats); CJK and fullwidth punctuation; the Hangul syllables; and some Latin letters: the DŽ, LJ and NJ digraph letters, Romanian `ș` and `ț` with the rest of U+0218 to U+024F, and letters such as `ǽ`, `ɑ`, `ɛ`, `ɣ`, `ʃ` and `ʒ`. So `T-shirt` sorts as `tshirt`, Russian `из-под` as `изпод` and `Hawaiʻi` as `hawaii`. The stored label keeps every character.
- The no-break and ideographic spaces weigh as a space, the superscript digits `¹²³` and the fullwidth digits as digits, the fullwidth Latin letters as ASCII letters, and katakana as hiragana.
- Every other character weighs as itself, so its case and accents count: Greek, Cyrillic, Armenian and Hebrew letters, the combining marks (a decomposed `é` is not a precomposed one), Latin Extended Additional (Vietnamese `ấ`, the capital `ẞ`), `€`, `⁴`, `①`, the CJK ideographs, and the characters outside the BMP, which weigh as their two UTF-16 surrogates and so sort after U+D7FF and before U+E000.

The Hangul jamo (U+1100 to U+11F9 and U+3131 to U+318E) are the one exception kindling does not follow. The devices give the initial, final and compatibility forms of one consonant (`ᄀ`, `ᆨ` and `ㄱ`) the same weight, give every vowel one shared weight, and sort all of them after the CJK ideographs. kindling sorts them by code unit, so a label with a jamo in it, or a label stored near one, can open nothing. `build` names such labels in a warning, and `lookup` prints a note when a dictionary holds them.

`ß`, `æ` and `œ` (with `Æ` and `Œ`) are stored the way kindlegen stores them: a marker character from U+0001 to U+0005 followed by the second letter of the pair, so `Fuß` is stored as `Fu`, U+0005, `s`. The marker weighs as the first letter, so the label weighs as `ss`, `ae` or `oe`, and `Fuß` sorts between `Fusion` and `Futter`. The devices search a tapped word with these letters spelled out, so `Fuß`, `FUSS` and `Fuss` all open `Fuß`, and a dictionary with both `Straße` and `Strasse` opens each for its own spelling. Stored as one character, as kindling 0.45.1 did, the letter weighed nothing: of the 4,662 headwords with `ß` in a German word list, none opened for its own spelling, its `ss` spelling or its capitals, and 2,113 opened only for the word without the `ß` (`barfu` for `barfuß`). Now all 4,662 open for their own spelling, and none for the word without the `ß`. Tools that read the index without decoding this form, KindleUnpack among them, show such a headword as a control character followed by the second letter (`Fu`, U+0005, `s`), as they do for a dictionary kindlegen built; the entry text keeps the real letter, and `kindling dump` shows the label as `Fuß`. kindling reads a source headword spelled that way, a marker from U+0001 to U+0005 followed by the second letter, as the letter, so a dictionary unpacked from a kindlegen or kindling file rebuilds with `Fuß` intact; any other U+0001 to U+0005 in a headword is left out.

`ĳ` and `Ĳ` are stored as `ij` and `IJ`, as kindlegen stores them. The headword is found by its usual spelling (`ijs`, `IJsland`, `IJSLAND`), but not by a tapped word spelled with the single character `ĳ`, which weighs nothing. A capital `ẞ` weighs as itself and finds only a headword spelled with it. A prolonged sound mark `ー` after kana is stored as a marker of the vowel it lengthens, as the generated tables described below store it, because the devices search for a tapped `ー` that way. Stored as the mark itself, `スーパー` was opened by `スパ` but not by `スーパー` or `すーぱー`. The devices carry that vowel past `ん`, `ン`, `・` and characters that are not kana, and also take it from halfwidth katakana and apply it to the halfwidth mark `ｰ`. kindling stores the mark as itself after those characters and after halfwidth katakana, and always stores `ｰ` as itself. So a label such as `ラ・ー`, `ｹーｷ`, `ケｰキ` or `ﾗｰﾒﾝ` is not found by its own spelling, only by the spelling without the mark (`ラ`, `ｹｷ`, `ケキ`, `ﾗﾒﾝ`).

A tapped word finds every label with the same weights, and one of them opens: the label spelled exactly like the word, else the first one that equals the word when case is ignored (accents always count, and a German book compares case too), and otherwise, apart from some prefix rules, the first of them. The full rule is under [Lookup simulator](#lookup-simulator). So `riviere` and `RIVIÈRE` open `rivière` when no other label has its weights, and in a dictionary with both `meme` and `même`, each opens for its own spelling (issue #8). When no label is spelled like the word, the stored order decides which of the labels equal to it with case ignored opens, or, when no label is equal to it with case ignored, which label opens. Among labels with the same weights, kindling stores first a label the Kindle rewrites when it is tapped (step 1 under [Lookup simulator](#lookup-simulator)), because such a label never equals the word it is looked up by, so it opens for its own spelling only when it comes first. Then comes the one with more apostrophes (`'`, `’` or `ʼ`); then, except in Danish dictionaries, the one with fewer accented Latin letters from `À` to `ſ`, not counting `Ø`, `Đ`, `Ħ`, `Ĳ`, `Ł`, `Ŀ`, `Ŋ`, `Ŧ` and their lowercase forms, whose case a Kindle compares, or `İ` and `ı`; then the one with fewer capital letters, where capitals do not count in a label the Kindle rewrites, and a German or Danish dictionary moves back only a label with no lowercase letter; then the one with fewer characters that weigh nothing; then the spelling without `ß`, `æ`, `œ` or `ĳ`; then the one whose characters that weigh nothing have the lower code units; then the one with the lower stored bytes. So in a Norwegian dictionary `-aren` opens `-aren` rather than `åren` and `FØRER` opens `fører` rather than `fôrer`, a sentence-initial `If` opens `if` rather than `IF`, `donʼt` spelled with U+02BC opens `don't` rather than `dont`, Ukrainian `Дем'янюка` opens `Дем’янюка` rather than `Демянюка`, `andr-` opens `andr-` rather than `Andr.`, and in a German book `ANODE` opens `Anode` rather than `anöde`, a sentence-initial `Schlagen` opens `schlagen` rather than `Schlägen`, `HAUS` opens `Haus` rather than `haus`, `CAN'T` opens `can't` rather than `cant`, `E-MAIL` opens `email` rather than `E-mail`, and `DASS` opens `dass` rather than `daß`. No order is right for every word. Of two rewritten twins only the first opens, so `a.i.` opens `A.I.`. Outside German and Danish, a word spelled like neither twin opens the one with fewer capitals, so in Italian `ABACO` opens `abaco` rather than `Abaco`. In a German book it opens the one with fewer accents, so `ABSCHLÄGE` opens `abschlage` rather than `Abschläge`. We looked up every word of the definition text of reader.dict's German dictionary (4,307,860 words) on the Paperwhite 4 with and without the accent count in German: with it, 461 words that opened a label with other letters open one with their own letters (`Schlagen`, `Nahen`, `Rote`), and 12 no longer do (`hüfte` opens `hufte`). Of that dictionary's 1,082,330 headwords, inflected forms and their spellings, 21 open their entry on the Paperwhite 4 and the Kindle 4 only with the accent count and 47 only without it.

Latin-script dictionaries store each character of a label as a symbol of an exact per-character ORDT table, and so does any dictionary built with `--strict-accents`, Greek and Cyrillic included. `--fold-accents` stores a Latin dictionary's labels as plain UTF-16 instead, with a folding ORDT/SPL blob taken from a kindlegen Greek dictionary, which Greek dictionaries get by default. kindlegen itself stores Latin labels through a per-character table, as kindling's default does. Cyrillic, Hebrew, Armenian and other dictionaries store plain UTF-16 labels with no blob. None of this changes the weights the labels are sorted by, because the devices do not weigh by the file's tables. So the default, `--fold-accents` and `--strict-accents` builds of a Latin dictionary store their labels in the same order and open the same entries, apart from the Kindle 4 aliases that `--strict-accents` leaves out (see below): in a dictionary of `a-b` and `a_b`, `ab` opens `a-b` in all three. The other difference is length: a label is cut at 254 bytes. The exact table stores a character in one byte only while it has 256 symbols or fewer (`%`, `_` and a NUL among them) and no character outside the BMP. Otherwise it stores two bytes per character, as plain UTF-16 always does, and the cut falls after 127 characters. So `--fold-accents` differs from the default here only in a dictionary whose exact table is one byte wide: there the default build keeps up to 254 characters of a headword or inflected form, while `--fold-accents` cuts it after 127, and the cut form can miss for its full spelling. In a dictionary with a two-byte table, every build cuts it after 127 characters, as kindlegen does. The weights the exact table writes agree with the stored order, although neither the Kindle 4 nor the Paperwhite 4 reads them. Both flags have environment-variable forms, `KINDLING_FOLD_ACCENTS=1` and `KINDLING_STRICT_ACCENTS=1`, which are the only way to reach them when kindling runs under a wrapper that controls the command line (for example pyglossary's Mobi writer, used by reader.dict). Neither affects book builds or the Japanese, Chinese, Korean and Arabic-script dictionaries.

Cyrillic dictionaries get generated lookup aliases (issue #17). Every indexed form with a combining stress mark (acute U+0301 or grave U+0300, as in `пробормота́в`) also gets its bare spelling as an extra index entry, and every form with uppercase letters gets a lowercased alias, so an all-caps abbreviation like `ФСБ` is found without the manual lowercase variants dictionaries needed under kindlegen. Aliases point at the same entry and dedupe against forms the source already ships. `--strict-accents` skips the stress-stripped aliases. A Russian dictionary keeps the lowercase ones even then, because the Paperwhite 4 and 5 lowercase every word they look up in a Russian dictionary, so there a capitalized headword without a lowercase label (`Москва`, `СССР`) is opened by no spelling; the Kindle 4 looks the word up as tapped and opens the capitalized label. Ukrainian, Bulgarian and other Cyrillic dictionaries get no aliases under `--strict-accents`, since no device changes the case of a word looked up in them.

Romanian s and t come in two spellings, with a comma below (`ș`, `ț`) and with a cedilla (`ş`, `ţ`), and books use both. The devices pass over the comma-below letters but weigh the cedilla letters as `s` and `t`. So kindling sorts `școală` as `coală`. A 97-word Romanian dictionary that sorted `ș` and `ț` as `s` and `t`, and four other letters from U+0218 to U+024F (`ȷ`, `Ȣ`, `ȧ` and `ȳ`) after `z`, had 45 headwords that no spelling opened on the Kindle 4 and the Paperwhite 4: typed as written, 37 of them (`stat`, `tară` and `șarpe` among them) opened nothing and the other 8 opened another entry (`șapte` opened `apte`). It also means that a word typed with one spelling does not find a headword written with the other. So every headword or inflected form with a comma-below letter also gets its cedilla spelling as an alias, and `Timişoara` finds `Timișoara`. In a Romanian dictionary a form with a cedilla letter also gets its comma-below spelling. Other dictionaries skip that direction, because the comma-below spelling weighs as the word without the letter: in Turkish, `şehir` would also open for `ehir`. The cedilla alias has a cost of the same kind. A Kindle weighs it as the word written with a plain `s` or `t` and skips the comma-below letters of a tapped word, so the alias also opens for some words the dictionary does not have: in reader.dict's Romanian dictionary, `servește` opens `șervete`, `știe` opens `ție` and `să-și` opens `șai`. A word that the dictionary has as a headword or an inflected form still opens its own entry. We looked up every word of that dictionary's definition text (1,906,612 words, written with comma-below letters) on the Paperwhite 4 and 5. Of the words that 0.45.1 opened nothing for, 1,369 now open, through the alias, an entry whose letters differ from the word's even without diacritics, and 445 open one that differs only in diacritics and punctuation (`să-i` opens `șai`). The same text written with cedilla letters has 25,137 different words with `ş` or `ţ`: 20,713 of them now open the right entry and 226 open another one, where 0.45.1 opened nothing. Leaving out an alias when another entry has a label with the same weights, as for the rewritten spellings below, would not remove these hits, because no other label weighs the same as the aliases `şervete` and `ţie`. So kindling keeps the alias.

A Kindle rewrites a tapped word before it looks the word up (see [Lookup simulator](#lookup-simulator)), so a headword that the rewrite changes is never matched as written. When the rewrite drops only characters that weigh nothing (`Bsp.` looked up as `Bsp`), the headword is still found. When it drops digits or letters, it is not: `MP3` is looked up as `MP`, `COVID-19` as `COVID`, a Thai word ending in `์` (U+0E4C) without it, `people's` in an English book as `people`, and `l'altro` in an Italian book as `altro`. So every such headword or inflected form also gets the rewritten spelling as an alias, both for a book in the dictionary's language and for a book in a language with no rules of its own. The Kindle 4 (firmware 4.1.4) also drops the marks at the end of a word that the Paperwhite 4 and 5 keep: the Thai vowel signs and tone marks (`ให้` is looked up as `ให`), the vowel signs of the Indic scripts, Hebrew points, and a final combining accent. So a headword or inflected form also gets the spelling the Kindle 4 looks it up by in a book in the dictionary's language, unless the Kindle 4 shows no popup for that spelling or the build uses `--strict-accents`, since the Kindle 4 spelling can drop an accent. The alias is left out when a label of another entry, or another form's rewritten spelling, has the same weights, because it would then take that entry's lookups or open for a different word: `MP3` and `MP4` together get no `MP`. A spelling the Paperwhites look a form up by is kept over a Kindle 4 spelling of another entry, so the Kindle 4 aliases change nothing a Paperwhite opens for a headword. In reader.dict's dictionaries this adds 3,998 aliases in Thai, 1,296 in French and 626 in English. In the Thai dictionary the Kindle 4 opens the entry for 29,840 of its 32,384 headwords and inflected forms, where it opened 27,475 with the Paperwhite spellings alone. Of the 2,544 it still does not open, 1,797 are formatted to nothing and the others are looked up by a spelling that has the weights of another entry's label or Kindle 4 spelling. The cost is that a word spelled like a Kindle 4 alias opens its entry on every Kindle. On 20,002 words of Thai text taken from the dictionary's definitions, 586 taps on the Kindle 4 that opened nothing with the Paperwhite spellings alone now open the word's own entry, while 35 such taps on the Paperwhite 4 and 5, and 58 on the Kindle 4, now open a headword that differs from the tapped word only in its marks (`ไหม` opens `ไหม้`). Japanese, Chinese, Korean and Arabic-script dictionaries get neither these aliases nor the Romanian ones.

In a German dictionary with `nach` and the prefix `Nach-`, a tapped `Nach` finds both labels, because the hyphen weighs nothing. In a German book a Kindle tells case apart when it chooses among the labels it found, so neither label equals `Nach`, and the Kindle opens the first label that is longer than the word and begins with it (step 5 under [Lookup simulator](#lookup-simulator) gives the full rule). A sentence that starts with `Nach` therefore opens `Nach-` on the Kindle 4 (firmware 4.1.4), the Paperwhite 4 (firmware 5.16.5) and the Paperwhite 5 (firmware 5.19.2), and so does a tapped `Nach-`, which is looked up as `Nach`. So in a German dictionary, a headword or inflected form that starts with a capital letter and ends in a hyphen also gives its spelling without the hyphen, as an alias, to the entry whose label is that spelling with a lowercase first letter, unless a label is already spelled that way. `Nach`, `Zu` and `Über` then open `nach`, `zu` and `über`, and so do `Nach-`, `Zu-` and `Über-`, which is also what a kindlegen dictionary that lists `Nach-` as an inflected form opens. In reader.dict's German dictionary this adds 41 aliases. In that dictionary's definition text, looked up on the Paperwhite 4, 1,526 words such as a sentence-initial `Im`, `Ein` or `Nach` now open the word rather than the prefix. The cost is that 194 tapped prefixes such as `Ein-` and `Sonder-`, and the 41 prefix labels themselves, now open the word rather than the prefix entry.

Japanese, Chinese, Korean and the Arabic-script languages (`<DictionaryInLanguage>` of `ja`, `zh`, `ko`, `ar`, `fa`, `ur`, `ps`, `ug`, `sd` or `ckb`) instead get generated ORDT collation tables for each dictionary, with one label element per character. Persian, Urdu, Pashto, Uyghur, Sindhi and Central Kurdish reuse the all-literal table of Arabic, each with its own neutral-primary MOBI locale. Kana are collation symbols in an embedded ORDT table, so hiragana and katakana fold together and ヴ collates as ウ; every other character (kanji, Hangul, Arabic letters) is stored as a literal Unicode code point. The prolonged sound mark ー folds onto the vowel before it (so ローゼマイン collates with a long `o`), as kindlegen stores it and as a Kindle searches for a tapped ー; a label that keeps the raw mark after kana opens nothing. The kana tables are embedded only for dictionaries that contain kana. The all-literal scripts get the minimal table kindlegen writes for them, plus a space symbol when a headword contains a space, and characters outside the BMP are stored as UTF-16 surrogate pairs, as kindlegen stores them. The MOBI-header locale is the neutral primary LCID. This layout works on hardware at production scale for Japanese and Chinese (a 52k-entry Chinese build). The 0.16.0 and 0.17.0 releases used a one-symbol-per-byte encoding copied from kindlegen's output on toy dictionaries; that form only appears for tiny inputs and never matched on device, which is why issue #11 got worse before it got better. `--strict-accents` and `--fold-accents` have no effect on these dictionaries.

Korean is the exception, and the limitation is the device, not the index: stock Kindle firmware has no working lookup of Korean headwords. Amazon ships no Korean dictionary, Korean is not a KDP-supported book language, kindlegen's own output fails the same way (checked in a 262k-entry A/B test on hardware), and community reports of the failure go back to 2019. The problem is the Hangul script itself, not the declared language: the same Hangul index declared Chinese, Russian or English (with matching book tags) fails the same way, while Latin headwords declared Korean work, so no metadata or layout choice can get around it. Latin-headword dictionaries with Korean definitions work normally. A Korean-headword dictionary still builds, is structurally sound and works in non-Kindle readers, but on a Kindle every lookup opens the same arbitrary entry. Use the StarDict or EPUB3 export with KOReader, GoldenDict or another non-Kindle reader instead. See issue #22 for the full investigation.

#### Supported dictionary languages

Languages exercised by the test suite, with their index layout and how far each has been verified:

| Language | Code | Flag | Index collation | Verified |
|---|---|---|---|---|
| Greek | `el` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/el.svg" width="20" alt="Greek flag"/> | UTF-16BE + kindlegen-derived ORDT/SPL blob | On device (production dictionaries) and structural tests |
| English | `en` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/en.svg" width="20" alt="English flag"/> | Exact per-character ORDT | On device (community use) and structural tests |
| French | `fr` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/fr.svg" width="20" alt="French flag"/> | Exact per-character ORDT | On device and structural tests |
| Russian | `ru` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ru.svg" width="20" alt="Russian flag"/> | UTF-16BE (folding blob suppressed) + stress/case aliases | On device and structural tests |
| Turkish | `tr` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/tr.svg" width="20" alt="Turkish flag"/> | Exact per-character ORDT | On device and structural tests |
| Japanese | `ja` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ja.svg" width="20" alt="Japanese flag"/> | Generated ORDT (per-character) | On device; byte parity with kindlegen lookup keys |
| Chinese | `zh` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/zh.svg" width="20" alt="Chinese flag"/> | Generated ORDT (per-character) | On device at production scale (52k entries); byte parity with kindlegen lookup keys |
| Korean | `ko` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ko.svg" width="20" alt="Korean flag"/> | Generated ORDT (per-character) | Not supported on Kindle: stock firmware cannot look up Korean headwords from any tool, kindlegen included (issue #22). Use the StarDict/EPUB3 exports instead |
| Arabic | `ar` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ar.svg" width="20" alt="Arabic flag"/> | Generated ORDT (per-character) | On device at fixture scale; byte parity with kindlegen lookup keys |

Persian (`fa`), Urdu (`ur`), Pashto (`ps`), Uyghur (`ug`), Sindhi (`sd`), and Central Kurdish (`ckb`) route through the same generated all-literal ORDT as Arabic, each with its own neutral-primary MOBI locale. They share the device-verified `ar` collation path, so they are not in the table above as separate fixtures; on-device verification of the per-language locales is in progress.

Each language in the table has a committed fixture under `tests/fixtures/langs/<code>/` with a dictionary source, a kindling build, a kindlegen build, and a sideloadable test book (regenerate with `tests/fixtures/langs/generate.py`). `tests/dict_languages.rs` builds each dictionary with kindling and checks the language ids and locale, that every entry has a real text pointer, that the headwords round-trip through the on-disk labels, and the per-language collation; for the generated-ORDT scripts it also asserts byte parity of the ORDT table and headword labels against the committed kindlegen build (identical for the all-literal scripts, value-equivalent for Japanese). Languages not listed still build with correct ids: a dictionary with Latin-script headwords gets the exact per-character table, and any other script gets UTF-16BE labels. The MOBI locale field maps Ancient Greek (`grc`), Thai (`th`) and a broad set of Latin- and Cyrillic-script languages to their own neutral Windows LCID rather than defaulting to English; codes with no mapping, such as Esperanto (`eo`), still fall back to English. The language does not change the weights above. On the Paperwhite 4, the Paperwhite 5 and firmware 5.19.6 it only decides whether a looked-up word is lowercased, which they do in a Russian dictionary. The Kindle 4 (firmware 4.1.4) looks a tapped word up in the dictionary whose language id matches the book's, so a Thai dictionary that declared English opened nothing for a Thai book there. Unlike kindlegen, which aborts on a language it does not know, kindling builds a dictionary for any `dc:language`. If you ship a dictionary in one and lookups misbehave on device, please open an issue.

### Books

```bash
kindling-cli build input.epub                          # output next to input as input.azw3 (KF8-only)
kindling-cli build input.epub -o output.azw3           # explicit output path
kindling-cli build input.epub --legacy-mobi            # opt into legacy dual MOBI7+KF8 (.mobi)
kindling-cli build input.epub --legacy-mobi            # dual MOBI7+KF8 .mobi so sideloaded covers show (issue #20)
kindling-cli build input.epub --no-hd-images           # skip HD image container
kindling-cli build input.epub --no-embed-source        # smaller file, but breaks Kindle Previewer
kindling-cli build input.epub --kindle-limits          # warn about HTML files exceeding 30 MB
kindling-cli build input.epub --no-validate            # skip KDP pre-flight validation
kindling-cli build input.epub --force-user-fonts       # skip font embedding; Aa menu font always applies
```

Auto-detects dictionary vs book from the OPF's `DictionaryInLanguage` metadata. Book MOBIs include embedded images and, when any image exceeds the 128 KB per-record limit the reader can decode, an HD image container (for high-DPI Kindle screens) carrying the full-resolution originals. The original EPUB is embedded by default for Kindle Previewer compatibility (`--no-embed-source` to skip).

The on-device "Go To" table of contents is built from the EPUB navigation document, preferring the EPUB3 nav (`properties="nav"`) and falling back to the EPUB2 `toc.ncx`, so chapter names survive even when every chapter file carries the same generic `<title>` (issue #18). Entries that point at `file#anchor` targets inside a shared spine file each get their own TOC node at the anchor position, matching kindlegen's NCX. Files without a nav document fall back to per-file `<title>` labels.

Dictionaries get a KF7 logical TOC from the same navigation document (issue #64). Its targets are remapped after dictionary assembly, which moves front matter ahead of the entries and strips the `idx:` wrappers: bare files, anchors in front matter, letter headings between entries, entry ids and anchors inside entries all land at their final byte positions. The four-tag KF7 NCX omits KF8's fragment target and keeps the ordinary empty text-record TBS trailers; a Paperwhite on firmware 5.19.2 showed the complete Go To menu, landed correctly at targets near the beginning, middle and end of a multi-record dictionary, and still returned all three entries through lookup.

Links inside the book navigate too (issue #50). Neither Kindle format follows an ordinary href, because both concatenate every spine document into one byte stream and throw the file boundaries away, so kindling resolves each `<a href>` to a byte offset: `filepos` in the legacy MOBI6 half, `kindle:pos:fid:FFFF:off:OOOOOOOOOO` in KF8. That covers footnote markers and their back-links, cross-references between chapters, and in-content tables of contents, whether the source writes them as `../Text/notes.xhtml#n1` or as a bare `#n1`. Fragments are resolved inside the document that declares them, which matters because a book whose chapters each number their footnotes from one has an `ftn-1` in every chapter.

A link that names a document rather than a place inside one still goes to that document: a bare `#`, a fragment that is an id on the target's `<body>`, and a link to the in-spine cover page that KF8 drops all land at the start of the document they mean. External links and an empty `href=""` are left exactly as they are. What is left over is a link naming a document that is not in the book at all, or a fragment no element declares; those are written as an inert placeholder of the same width and counted in a build warning, which is what kindlegen reports as an unresolved hyperlink.

Nested TOC levels are preserved (issue #19): nested `<ol>` lists in the EPUB3 nav (or nested `<navPoint>` elements in the NCX) become a hierarchical KF8 NCX with kindlegen's exact tag layout - breadth-first entry numbering, parent/first-child/last-child links, and subtree lengths, so a 卷/章 (volume/chapter) structure collapses and expands on device just like a Send-to-Kindle or Calibre conversion. The per-record TBS (trailing byte sequences) switch to the hierarchical strand encoding for these books, verified byte-for-byte against kindlegen on 2- and 3-level test books.

Every embedded JPEG is given the JFIF header the firmware expects. A file whose first marker is APP1 (Exif) rather than APP0, which is what Photoshop and most camera pipelines write, used to ship with no JFIF header at all: no density, no units, and its Exif still attached, because the old fixup only patched the units byte of an APP0 that was already present and nothing under the 128 KB cap is re-encoded (issue #43). The header is rebuilt rather than the image re-encoded, so the entropy-coded scan is copied through and the decoded pixels are identical; an existing JFIF APP0 keeps its own density and only has its units corrected, and segments other than APP0 and Exif are preserved, since an ICC profile or an Adobe transform decides how a passed-through scan's colors decode. Anything that does not parse plainly as a JPEG is left exactly as it was. `rewrite-metadata --cover` runs the same normalization.

The KF7 half of a dual-format file says its layout in markup rather than CSS, because a MOBI6 reader applies no stylesheet, the same way the dictionary lookup popup does not (issue #58). `text-align:center` on a wrapper becomes the legacy `align="center"` attribute, which those readers do honor and which is what kindlegen writes for the same intent; without it no `--legacy-mobi` comic page was centered on the hardware that half exists for. `position:absolute` is dropped, since it is KF8 markup with nothing to position against, so the Panel View block comes through as bare elements the way kindlegen reduces it. The viewport `<meta>` goes too, being a fixed-layout instruction KF7 has no fixed layout for. Every other `style` attribute is left alone: a KF7 reader ignores them, so stripping them would only churn bytes. The KF8 half is untouched, so Panel View still works where it is read.

Kindle's renderer keeps any `font-family` the book CSS names over the reader's Aa menu choice, so a book whose stylesheets name fonts that are not even embedded (common in Chinese EPUBs) shows the font menu but never changes face. When a book embeds no usable fonts, kindling strips `font-family` declarations and dead `@font-face` rules from the stylesheets, inline `<style>` blocks, and per-element `style="..."` attributes so the Aa menu stays in control; declarations whose family stack includes the generic `monospace` are kept as plain `font-family: monospace` so code blocks stay fixed-pitch. Books that do embed fonts keep their CSS untouched by default (the publisher design wins); pass `--force-user-fonts` to skip font embedding and strip `font-family` anyway, mirroring KOReader's reader-first behavior.

Fonts declared in the manifest (TTF/OTF) are embedded as KF8 FONT resource records, with `@font-face` `src: url(...)` in the stylesheets rewritten to the matching `kindle:embed` resource, so publisher fonts survive conversion. EPUB font obfuscation (both the IDPF and Adobe schemes declared in `META-INF/encryption.xml`) is undone at build time; the embedded output is zlib-deflated and unobfuscated, matching kindlegen's own FONT records. WOFF/WOFF2 fonts are skipped with a warning since Kindle cannot render them. Note that the Kindle language also matters for on-device font choice: a book without a `<dc:language>` (or with an unrecognized tag) is treated as English, which hides the CJK font menu on Chinese/Japanese books, so kindling warns when that happens.

Non-dictionary builds default to KF8-only `.azw3`, because Amazon deprecated MOBI for Send-to-Kindle in August 2022 and modern Kindles prefer KF8-only. Dictionaries continue to build as dual-format MOBI7+KF8 `.mobi`, because Kindle's lookup popup requires the MOBI7 INDX structure and KF8 has no equivalent. Pass `--legacy-mobi` on a book build to opt back into the old dual-format `.mobi` output for pre-2012 Kindles; the flag is a no-op on dictionary builds. If you pass `-o foo.mobi` or `-o foo.azw3` explicitly, kindling respects whatever extension you chose. One caveat if you sideload over USB: a `.azw3` shows no home-screen library cover on current firmware, because the tile is cached only via the legacy `.mobi` ingest path (issue #20). Pass `--legacy-mobi` for a dual MOBI7+KF8 `.mobi`, which shows the cover and opens; naming KF8-only bytes `.mobi` shows the cover but leaves a book no Kindle will open (issue #24). See [Known Kindle firmware issues](#known-kindle-firmware-issues).

Every `build` runs the Kindle Publishing Guidelines validator automatically before writing the MOBI. Findings are printed with severity, rule id, and file:line; the build is aborted on any error (warnings are advisory). Pass `--no-validate` to skip pre-flight entirely.

### Comics

```bash
kindling-cli comic input.cbz --device paperwhite                # output next to input as input.azw3
kindling-cli comic input.cbr -o output.azw3                     # CBR (RAR) input, explicit output
kindling-cli comic manga.epub --rtl                             # EPUB comic/manga
kindling-cli comic manga.cbz --rtl                              # manga (right-to-left)
kindling-cli comic webtoon/ --webtoon                           # webtoon (vertical strip)
kindling-cli comic input/ --no-split --crop 0                   # disable smart processing
kindling-cli comic input/ --no-optimize                         # ship pages exactly as they are (issue #29)
kindling-cli comic input.cbz --title "My Comic" --language ja   # metadata overrides
kindling-cli comic input.cbz --doc-type ebok                    # appear under Books on Kindle (default: no shelf)
kindling-cli build book.epub --doc-type ebok                    # Books shelf + a lock screen cover (issue #26)
kindling-cli thumbnail book.azw3 --kindle /Volumes/Kindle       # fix the library tile that --doc-type ebok costs you
kindling-cli comic input.cbz --cover 3                          # use page 3 as cover
kindling-cli comic input.cbz --legacy-mobi                      # opt into legacy dual MOBI7+KF8 (.mobi)
kindling-cli comic input.cbz --legacy-mobi                      # dual MOBI7+KF8 .mobi so sideloaded covers show (issue #20)
kindling-cli comic input.cbz --embed-source                     # embed EPUB source (off by default, see note below)
```

Comics default to KF8-only `.azw3` for the same reason books do: Amazon deprecated MOBI for Send-to-Kindle in August 2022, and the legacy MOBI7 section in dual-format files is at best wasted bytes on modern Kindles. `--legacy-mobi` is the escape hatch for pre-2012 devices. If you pass `-o foo.mobi` explicitly, kindling respects your extension choice. Like books, a sideloaded `.azw3` shows no home-screen library cover on current firmware; pass `--legacy-mobi` for a dual MOBI7+KF8 `.mobi` so the cover shows and the file still opens (issue #20, issue #24).

Comic builds do not embed the intermediate EPUB as a SRCS record by default (this changed in v0.7.7). Embedding duplicates every page image as a zipped EPUB inside the MOBI, which for a large comic produces a single PalmDB record over 100 MB. Kindle devices index the resulting file but then fail to open it with "Unable to Open Item". Pass `--embed-source` only when you need to round-trip through Kindle Previewer.

Converts image folders, CBZ files, CBR files, and EPUB files to Kindle-optimized MOBI with:
- **Device profiles**: *paperwhite*, *kpw5*, *oasis*, *scribe*, *scribe2025*, *kindle2024*, *basic*, *colorsoft*, *kpw6*, *scribe-colorsoft*, *fire-hd-10*, and the 600x800 era: *k34* (Kindle 3/Keyboard and Touch), *k57*, *k810*, *kpw1* (758x1024) and *kdx* (824x1000). `--device 800x600` resolves to the 600x800 box, since that is how those screens are advertised (issue #28). The four pre-KF8 profiles warn when selected without `--legacy-mobi`, because a KF8-only `.azw3` will not open on them. The DX height is 1000 rather than its panel's 1200 on purpose: it reserves the bottom strip for the progress bar, and a full-height image gives blank pages between the real ones
- **Spread splitting**: Landscape images auto-split into two pages (disable: `--no-split`)
- **Margin cropping**: `--crop 2` (default) crops margins + page numbers, `--crop 1` crops margins only, `--crop 0` disables cropping
- **Auto-contrast**: Histogram stretching and gamma correction for e-ink (disable: `--no-enhance`)
- **Moire correction**: Rainbow artifact removal for color e-ink screens (Colorsoft), applied automatically to grayscale source images
- **Manga mode**: `--rtl` reverses page order and split direction
- **Webtoon mode**: `--webtoon` merges vertical strips and splits at panel gutters with overlap fallback to prevent content loss
- **Panel View**: Tap-to-zoom panel detection for Kindle (disable: `--no-panel-view`). Reading order configurable via `--panel-reading-order` (`horizontal-lr`, `horizontal-rl`, `vertical-lr`, `vertical-rl`); defaults to `horizontal-rl` with `--rtl`
- **EPUB support**: Fixed-layout EPUB comics extracted in spine order (correct page sequence)
- **CBR support**: RAR-based comic archives extracted via `bsdtar` (libarchive). `/usr/bin/bsdtar` ships with macOS; on Linux install `libarchive-tools` (`apt`) or `bsdtar` (`dnf`). Both RAR4 and RAR5 are supported. Header-encrypted CBRs are rejected with a clear error.
- **ComicInfo.xml**: Auto-reads metadata and manga direction from CBZ and CBR files
- **Metadata overrides**: `--title`, `--author`, `--language`, `--cover` (page number or file path). Without `--title`, the title is read from ComicInfo.xml or defaults to "Comic".

Kindle library field mapping (what the Kindle actually displays for sideloaded content):

| Library field | MOBI source | Notes |
|---|---|---|
| Title | EXTH 503 (books/dicts) or KF8 Record 0 full_name (comics) | EXTH 503 is emitted for reflowable books and dictionaries. For fixed-layout comics, EXTH 503 is omitted - it breaks Kindle navigation (toolbar/go-home disappear). KCC/kindlegen also omit it for comics. For dual-format `.mobi`, Kindle reads full_name from KF8 Record 0, not KF7. |
| Author | EXTH 100 | Set via `--author` flag or ComicInfo.xml `<Writer>`/`<Penciller>`. Defaults to "kindling". |
| Cover | EXTH 201 (cover image offset in image pool) | Cover offset is 0-based index within image records starting at `first_image`. |
| Library thumbnail | EXTH 202 (thumbnail offset) + EXTH 129 (`kindle:embed:XXXX` thumbnail URI) | A downscaled JPEG generated from the cover and appended to the image pool. EXTH 129 is the base32 of the same 0-based offset EXTH 202 carries, matching kindlegen and calibre; it is the thumbnail, not the cover (issue #26). |
| Lock screen cover | EXTH 113 (ASIN) + EXTH 501 (doc type) | Both records must be present for the firmware to treat a sideloaded book as lock-screen eligible; the image itself comes from the book's own cover and thumbnail records. Opt in with `--doc-type ebok`; 113 is filled in automatically (issue #26). |
| Document type | EXTH 501 | Omitted entirely by default, for comics as well as reflowable books. Its mere presence (any value) makes the Kindle reader treat the content as a non-navigable document and hide the back-to-library toolbar, trapping the reader inside (device-verified for books in issue #15, reported for comics on Paperwhite 5 firmware 5.18.1/5.19.2 in issue #21). kindlegen writes none for any content type. Set one explicitly with `--doc-type`: `PDOC` = Documents shelf, `EBOK` = Books shelf, `none` = omit (default). |
- **Document type** (books and comics): `--doc-type pdoc` for the Documents shelf or `--doc-type ebok` for Books (default: `none`, no shelf assignment). Comics used to default to `pdoc`; they now omit the record like reflowable books do, because its presence hides the back-to-library button on some firmware and leaves no way out of the book (issue #21). Asking for a shelf is opt-in, and an unrecognized value is now an error rather than a silent fallback to `pdoc`.
- **Lock screen covers**: with "Display book cover on lock screen" turned on, a sideloaded book shows its cover only if it carries both EXTH 113 and EXTH 501. Neither is written by default, so the wallpaper shows instead. `--doc-type ebok` writes both: it fills EXTH 113 with a UUID the EPUB already publishes, or derives a stable one from the metadata so rebuilds do not churn the identifier. This is the same combination calibre writes. Verified on a Paperwhite 5 (5.19.2) against two builds of one EPUB, with and without the pair.

  It is a tradeoff, not a free win. The firmware treats EXTH 113 as a real ASIN: it asks the store, finds nothing for a sideloaded UUID, and caches an Amazon "No image available" image at `system/thumbnails/thumbnail_<113>_<501>_portrait.jpg`. On the same Paperwhite 5 that gained the lock screen cover, the library tile became that placeholder. So `--doc-type ebok` trades a blank shelf tile for a placeholder one and gains the lock screen cover; the lock screen keeps working because it draws on the book's own cover, not on that cached file. This is the long-standing disappearing-cover complaint that calibre works around by keeping backups in an `amazon-cover-bug/` directory. `kindling thumbnail book.azw3 --kindle /Volumes/Kindle` does the same thing: it writes the book's own thumbnail over that cached file, which restores the tile and leaves the lock screen cover working, and drops a restore copy in `amazon-cover-bug/` for the next time a sync clobbers it. Also note issue #15: EXTH 501 is what hides the back-to-library toolbar on some firmware, though a reflowable book with 501 = EBOK navigated fine on 5.19.2.
- **KF8-only by default**: comics output `.azw3` with only the KF8 section (no MOBI7); pass `--legacy-mobi` for the old dual-format behavior on pre-2012 Kindles

### Library tiles for `--doc-type ebok` books

```bash
kindling-cli thumbnail book.azw3 --kindle /Volumes/Kindle
```

Only relevant to books built with `--doc-type ebok`. That flag gets a lock screen cover, and the cost is the library tile: the firmware reads EXTH 113 as a real ASIN, asks the Amazon store, and caches a "No image available" image as the shelf art. This writes the book's own thumbnail over that cached file, which fixes the tile and leaves the lock screen cover alone, and keeps a restore copy in `amazon-cover-bug/` because a later sync can put the placeholder back. It refuses a path with no `system/` or `documents/` directory, and refuses a book with no EXTH 113/501 pair, since there would be no filename for the firmware to look under.

### Validation

```bash
kindling-cli validate input.opf             # print findings, exit 1 on errors
kindling-cli validate input.opf --strict    # exit 1 on any warning too
```

Validation also runs automatically as a pre-flight step inside every `kindling build` invocation (including kindlegen-compat mode `kindling input.opf`). Any validation errors abort the build with exit code 1; warnings are printed but do not block the build. Pass `--no-validate` to `build` to skip the pre-flight entirely. Comic builds (`kindling comic`) do not run the validator because comics have different structural requirements that the book-oriented rules do not cover.

Runs 116 pre-flight checks against the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf) (version 2026.2). Most rules are ports of the corresponding w3c/epubcheck checks; the rest are KDP-specific rules kindling adds on top. A finding cites the KPG section and page where the Guidelines cover what the rule checks; a rule they do not cover, which is most of the epubcheck ports, cites no page and its message names the epubcheck check instead. Every citation was checked against the 2026.2 PDF. The groups below follow the section numbers of an older KPG edition:

- **4 Cover image** (5 rules): internal cover must exist via Method 1 (`<item properties="cover-image"/>`, the unhyphenated `coverimage` is also accepted) or Method 2 (`<meta name="cover">`), file must exist on disk, shortest side >= 500 px, no duplicate HTML cover page in the spine (`R4.1.1`-`R4.2.4`)
- **5 Navigation** (13 rules): NCX declared in manifest and referenced from `<spine toc>`, NCX and guide targets must resolve to manifest items, TOC recommended for books > 20 pages, NCX `dtb:uid` must match the OPF unique-identifier, page-list required when `epub:type="pagebreak"` is used, nav entries in spine order, no remote links in nav or NCX (`R5.1`-`R5.11`, epubcheck `NAV_003/010/011`, `NCX_001/004/006`, `OPF_032/050`)
- **6 HTML, CSS, and encoding** (19 rules): well-formed XHTML, no `<script>`, no nested `<p>`, filename case must match, XML 1.0 only, no external entities, `epub:` namespace URI must be correct (Vader Down bug), UTF-8 required for HTML and CSS, no forbidden `position` values, `@import`/`url()`/`@font-face` targets must resolve through the manifest, `@namespace` and unsupported `@media` features flagged (`R6.1`-`R6.17`, `R6.e1`, `R6.e2`, epubcheck `CSS_005`-`CSS_027`)
- **7 Manifest and spine integrity** (13 rules): declared media-types must match file bytes, every spine `itemref` must have a linear target, no duplicate `idref` or `href`, fallback chains must terminate at a renderable resource, deprecated media-types flagged, manifest cannot point at the OPF itself (`R7.1`-`R7.13`, epubcheck `OPF_003/013/029/033/034/035/037/040/041/042/043/074/099`)
- **8 OPF prefix and property grammar** (10 rules): `<package prefix>` syntax, reserved-prefix rebinding, manifest `properties` attributes must match the content's feature use, unknown or undeclared prefixes flagged (`R8.1`-`R8.10`, epubcheck `OPF_004/005/006/007/012/014/015/026/027/028`, EPUB 3 only)
- **9 Cross-references and dead links** (12 rules): fragment ids must exist in their target, fragments are rejected on non-SVG raster images and on manifest hrefs, `data:` and `file:` URLs refused, `..` path traversal blocked, manifest hrefs must name a resource (`R9.1`-`R9.12`, epubcheck `RSC_009/011/012/014/015/020/026/029/030/033`, `OPF_091/098`)
- **10 Text-heavy reflowable** (7 rules): supported image formats (JPEG, PNG, GIF, and SVG outside dictionaries), per-image size <= 127 KB, dimensions <= 5 megapixels, headers require valid JPEG/PNG/GIF end markers, image extension must match magic bytes, tables of 100 rows or more flagged (`R10.4.1`-`R10.4.5`, `R10.5.1`, epubcheck `MED_004`, `PKG_021/022`)
- **11 Fixed-layout** (9 rules, comic and textbook profiles only): OPF must declare `rendition:layout=pre-paginated`, XHTML must carry `<meta name="viewport">` with width and height, `rendition:spread`/`orientation`/`layout` values constrained, pages should contain image content, HD builds should carry `original-resolution` (`R11.1`-`R11.9`, epubcheck `OPF_011`, `HTM_046`-`HTM_053`)
- **13 OCF filenames** (5 rules): no OCF-forbidden characters (`< > : " | ? *` and controls), spaces and non-ASCII flagged, trailing dot rejected (Windows drops it), case-insensitive duplicate hrefs rejected (`R13.1`-`R13.5`)
- **15 Dictionaries** (14 rules, dict profile only): Amazon-legacy KDP format requires `DictionaryInLanguage`, `DictionaryOutLanguage`, `DefaultLookupIndex` matching an `idx:entry name`, at least one `idx:entry`, and an `idx:orth` that names its headword, as `value="..."` or as the element's own text (`R15.1`-`R15.7`). EPUB 3 dict rules gated on `package_version="3.0"` cover `epub:type="dictionary"`, `dc:type=dictionary`, Search Key Map Documents, and dictionary collections (`R15.e1`-`R15.e7`, epubcheck `OPF_078`-`OPF_084`)
- **16 OPF metadata and package identity** (8 rules): `<package unique-identifier>` must point at a real `<dc:identifier>`, `<dc:date>` must be W3CDTF syntax and a real calendar date, no empty Dublin Core elements, `opf:scheme="UUID"` must be a UUID, `<dc:language>` must be BCP47 (`R16.1`-`R16.8`, epubcheck `OPF_030/048/053/054/055/072/085/092`)
- **17/18.1 Unsupported tags** (1 rule): `<form>`, `<input>`, `<frame>`, `<iframe>`, `<canvas>`, `<object>`, etc. (`R17.1`)

A dictionary is held to nearly all the same rules, since KPG says it has all the same components as a book, but on the dict profile some errors report as warnings instead, because an error withholds the whole file while lookup can still work: a missing cover, which KPG says a dictionary only should have; NCX, guide, nav and `@font-face` problems (`R5.2.3`, `R5.3.1`, `R5.5`, `R5.7`, `R5.10`, `R5.11`, `R6.17`); a link to an anchor that is not there or with a space in it (`R9.3`, `R9.6`), which the lookup popup disables anyway; and `<script>` and other tags Kindle does not support (`R6.3`, `R17.1`). The dictionary builder uses valid navigation targets for its Go To menu and omits any it cannot resolve rather than withholding the dictionary. kindlegen builds a dictionary with any of these problems, and kindling drops a `<script>` element from a dictionary's text as kindlegen does. PyGlossary, which runs kindling in place of kindlegen, ignores the exit status and never checks that the file exists, so a dictionary without a cover, or with a cross-reference such as `bword://ice cream`, used to come out as nothing at all (issue #63). An image labeled as a different raster format is a warning in a dictionary for the same reason, and an SVG image draws a warning there, because a dictionary is Mobi 7 and cannot show one. The date check (`R16.3`) is a warning everywhere, since the build writes the date through as it is, and it accepts every W3CDTF form, including a numeric zone such as `+00:00`; the UUID check (`R16.7`) is a warning, as it is in epubcheck.

Output: one line per finding with severity (`info`/`warning`/`error`), rule id (e.g. `R4.2.1`), the KPG section and page where the Guidelines cover the rule, message, and file:line where applicable, followed by a summary (`X errors, Y warnings, Z info`). Exit code is 0 on success, 1 if any errors are present (or any warnings in `--strict` mode).

The rule catalog is a single Rust const array in [`src/kdp_rules.rs`](src/kdp_rules.rs) with a `KPG_VERSION` constant and a `Rule` struct holding id, group, level, title, the KPG section and page it cites (or none), description, and a profile mask (default, comic, dict, textbook). Each rule cluster lives in its own module under [`src/checks/<name>.rs`](src/checks/) and implements the `Check` trait; all active checks are registered in the `CHECKS` array in [`src/checks/mod.rs`](src/checks/mod.rs). Phase 2 added `fixed_layout`, `manifest_spine`, `opf_grammar`, `toc_extras`, `cross_refs`, `filenames`, `image_integrity`, `css_forbidden`, and `metadata` alongside the pilot clusters `parse_encoding` and `dict`. The pre-Phase-2 checks (`cover`, `navigation`, `nav_links`, `content`, `images`, `file_case`) are still `Check` impls. Updating the guidelines version touches `kdp_rules.rs` plus whatever `src/checks/*.rs` modules the affected rules live in.

### StarDict export

```bash
kindling-cli stardict input.opf                   # writes input-stardict/ next to the input
kindling-cli stardict input.epub -o my_dict       # explicit output directory
kindling-cli stardict input.opf -o my_dict --bookname "My Greek Dictionary" --author "Jane Doe" --date 2026-05-07
kindling-cli stardict input.opf --website "https://example.com/dict" --email "you@example.com" --description "License: CC-BY-SA 4.0."
```

`kindling stardict` reads the same OPF or EPUB dictionary input as `kindling build` and emits a four-file StarDict 2.4.2 bundle ready to drop into GoldenDict, GoldenDict-ng, KOReader, sdcv, or any other reader that consumes the format. The output directory contains:

- `<name>.ifo`: UTF-8 manifest with `bookname`, `wordcount`, `idxfilesize`, optional `synwordcount`, `author`, `email`, `website`, `description`, `date`, and `sametypesequence=h`. `bookname` / `author` / `date` default to the OPF's `dc:title` / `dc:creator` / `dc:date`; CLI flags override. `email`, `website`, and `description` have no OPF counterpart and are emitted only when supplied. StarDict 2.4.2 has no `license` field, so license info is conventionally folded into `description` (use `<br>` for line breaks). When the OPF declares `<DictionaryInLanguage>` and `<DictionaryOutLanguage>` and the bookname does not already contain a 2-3 letter hyphenated pair, kindling appends ` (in-out)` to the bookname so GoldenDict-ng / KOReader can parse the language pair and populate the "Translates from / to" fields (the StarDict spec has no formal source/target language slot, so embedding the codes in the bookname is the de-facto convention).
- `<name>.idx`: concatenation of `(headword\0, offset:u32be, size:u32be)`, sorted by `stardict_strcmp`, which is `g_ascii_strcasecmp` with an exact byte comparison breaking ties, so readers can binary-search. The tie-break is not cosmetic: sorting only case-insensitively and leaving a pair like `apple` and `Apple` in source order puts the index out of the order the search assumes, and the search then walks past one of them while it sits in the file (issue #60).
- `<name>.dict`: concatenation of per-entry HTML payloads. Each entry's `<idx:entry>` / `<idx:orth>` wrapper is stripped, `<idx:infl>` / `<idx:iform>` blocks are dropped (those forms are surfaced through `.syn` instead), and self-closing `<idx:orth value="X"/>` is rewritten to `<b>X</b>` so the headword stays visible in apps that render entries verbatim. Cross-entry references that target MOBI per-letter HTML (`content_NN.html#hw_X` or same-page `#hw_X`) are rewritten to StarDict's `bword://X` scheme so GoldenDict, GoldenDict-ng, KOReader, and sdcv resolve them as in-dictionary lookups.
- `<name>.syn`: `(form\0, original_word_index:u32be)` pairs mapping each inflected form to its lemma's row in `.idx`, sorted by the same key as `.idx`. Omitted when the source dictionary has no inflections.

### EPUB export

```bash
kindling-cli epub2 input.opf                          # plain reflowable EPUB2, writes input.epub2.epub
kindling-cli epub3 input.opf                          # EPUB3; auto-detects a dictionary and adds the dictionary layer
kindling-cli epub3 input.opf -o out.epub --book       # force a plain EPUB3 book even on dictionary input
kindling-cli epub3 input.opf --dictionary el en       # force the dictionary layer with explicit source/target languages
kindling-cli epub2 input.epub --title "My Book" --author "Jane Doe"
```

`--no-optimize` ships the source pages as they are: no resize to the device profile, no grayscale conversion, no border crop, no contrast or gamma pass, no moire filter, and no JPEG re-encode. It is for pages that were prepared deliberately, at twice a device's resolution so they can be zoomed, for instance, where every one of those steps is damage (issue #29). One step still runs, because it has to: a page over the 128 KB per-record limit closes the reading app on the page that uses it, so an oversized page is still re-encoded to fit and says so. That is also the one case where `--no-optimize` builds an HD container, so the original survives in a CRES record rather than being thrown away; pages under the cap add no HD bytes at all. `--device` keeps its other jobs, including webtoon splitting and the OPF canvas; it just stops rewriting pixels.

`kindling epub2` and `kindling epub3` read the same OPF or EPUB input as `kindling build` and emit a reflowable EPUB. Both write a spec-conformant archive: the `mimetype` entry first and stored uncompressed, then `META-INF/container.xml`, then the `OEBPS/*` payload deflated. The output validates clean under epubcheck (EPUB 2.0.1 rules for `epub2`, EPUB 3.3 rules for `epub3`).

- `epub2` is always a plain, reflowable [EPUB 2.0.1](http://idpf.org/epub/20/spec/OPF_2.0.1_draft.htm) book (`<package version="2.0">` plus an NCX; the `version` attribute is `2.0`, `2.0.1` is the spec revision). It is never dictionary-aware: if the input carries Kindle dictionary markup (`<idx:*>` tags, `<DictionaryInLanguage>` metadata), the dictionary semantics are ignored and a plain readable book is produced. Dictionary output is an EPUB3-only feature by design.
- `epub3` is a generic [EPUB 3.3](https://www.w3.org/TR/epub-33/) book (`<package version="3.0">` plus an EPUB3 nav document; the `version` attribute is `3.0` for all EPUB 3.x, `3.3` is the spec revision) by default. When the input looks like a dictionary, it additionally emits an [EPUB Dictionaries and Glossaries](https://www.w3.org/TR/epub-dictionaries/) layer (a profile on top of EPUB 3.3, not part of EPUB 3.3 core): a single Search Key Map (`skm.xml`), `<dc:type>dictionary</dc:type>`, `source-language` / `target-language` metadata (also declared as `<dc:language>`), and `epub:type="dictionary"` / `epub:type="dictentry"` semantics in the content. Each entry's body is re-parsed and re-serialized as well-formed XHTML so it passes epubcheck's strict DICT profile.

The dictionary layer is selected automatically: if the OPF declares `<DictionaryInLanguage>` / `<DictionaryOutLanguage>` or `<dc:type>dictionary`, `epub3` emits a dictionary with those languages as source/target. Pass `--book` to force a plain book regardless, or `--dictionary SOURCE TARGET` to force the dictionary layer with explicit language codes (overriding both auto-detection and the OPF's own language fields). There is intentionally no EPUB2 dictionary mode.

Both exporters flatten the spine into `content_NN.xhtml` in `OEBPS/`, so cross-document links are resolved against the document that wrote them and rewritten to the name the export gave their target (issue #55). A fragment survives only when the target document still has an element by that name: a link naming a `<body id>` becomes a link to the top of that document, since the export keeps the body's contents and drops the element. A link to a document that is not in the spine keeps its text and loses its href, so nothing in the output references a resource the archive does not contain. Bare same-document fragments, external links, and an `href=""` that names nothing are left exactly as written.

Images travel with the book. Every image the manifest declares, plus the ones only an `<img src>` mentions (which is how OEB 1.x-era tools ship inline glyphs), is read from disk and written into the archive as `images/img_NN.ext`, flattened the same way documents are so that duplicate basenames, `..` above the OPF, percent-encoding, and case-insensitive filesystems cannot collide. Each gets a manifest item with its media type, and the cover is marked `properties="cover-image"` in EPUB3 or named by a `<meta name="cover">` in EPUB2, which is the only mechanism EPUB 2.0.1 has. An `<img>` whose file is not in the archive is removed rather than left pointing at nothing, since `src` is required and epubcheck rejects one that resolves to no resource.

The Search Key Map holds exactly one `<search-key-group>` per headword (the spec mandates a single Search Key Map document per dictionary), with one `<match>` per searchable form: the headword plus every inflected form. At full dictionary scale this is a single large file, which is expected.

### Repair

```bash
kindling-cli repair input.epub                    # writes input-fixed.epub next to input
kindling-cli repair input.epub -o output.epub     # explicit output path
kindling-cli repair input.epub --dry-run          # scan without writing
kindling-cli repair input.epub --report-json      # full report as JSON on stdout
```

`kindling repair` runs a structural repair pass on an EPUB that fixes a small set of issues Amazon's Send-to-Kindle pipeline is unusually strict about. It is a Rust port of [`innocenat/kindle-epub-fix`](https://github.com/innocenat/kindle-epub-fix) (public domain), and applies the same four fixes the reference does:

1. **Missing XML declaration**: prepend `<?xml version="1.0" encoding="utf-8"?>` to any XHTML/HTML file that lacks one. Send-to-Kindle otherwise assumes ISO-8859-1 and corrupts non-ASCII characters.
2. **Body-id hyperlinks**: rewrite `filename#body-id` references to just `filename`, because Kindle silently drops fragments that point at a `<body>` tag, breaking TOC entries.
3. **Missing `dc:language`**: inject a fallback `en` into OPFs that have no `<dc:language>`, and warn when an existing language is outside Amazon's allowed list.
4. **Stray `<img>`**: delete `<img>` tags with no `src` attribute, which otherwise show up as broken image placeholders on Kindle.

The pass is byte-stable on clean input: if no fixes are needed, the output is a `fs::copy` of the input with identical bytes, so content-hash-based book identity stays the same. It is idempotent: running it twice produces the same result as running it once. DRM-protected EPUBs (`META-INF/encryption.xml` or `META-INF/rights.xml`) are rejected with exit code 1 and not touched; no DRM removal code is linked or referenced.

`kindling build` and `kindling validate` do not automatically invoke repair; it is a separate explicit pass. This lets downstream consumers that need reliable EPUB preprocessing run `repair` in their ingest pipeline without affecting the build or validation paths.

### Rewrite metadata

```bash
kindling-cli rewrite-metadata input.azw3 -o output.azw3 --title "New Title" --author Alice --author Bob
kindling-cli rewrite-metadata input.mobi --publisher "ACME" --language en --isbn 9780000000000
kindling-cli rewrite-metadata input.azw3 --cover new_cover.jpg
kindling-cli rewrite-metadata input.azw3 --title "New Title" --dry-run

# Stamp the lock screen cover pair (EXTH 501, plus EXTH 113 when the file has
# none) onto a book built before 0.30.0, instead of rebuilding it from source.
# Follow with `kindling thumbnail` so the library tile keeps the book's own art.
kindling-cli rewrite-metadata old_book.azw3 -o fixed.azw3 --doc-type ebok
kindling-cli thumbnail fixed.azw3 --kindle /Volumes/Kindle
kindling-cli rewrite-metadata input.azw3 --title "New Title" --report-json
```

`kindling rewrite-metadata` updates the EXTH metadata records (and optionally the cover image record) of an existing MOBI/AZW3 file without re-running the EPUB/OPF build pipeline. Supported fields: title (EXTH 503 plus full_name), multi-value author (100), publisher (101), description (103), language (524), ISBN (104), ASIN (504), publication date (106), multi-value subject/tag (105), and the cover image bytes. Book content records (text, indices, INDX/FLIS/FCIS) are never touched, and neither is any image except the two that belong to the cover. Replacing the cover regenerates the library thumbnail (EXTH 202) from the new image, because a tile made from the old cover is what the library goes on showing, and holds the new cover to the same 128 KB per-record cap a build enforces, so the flag cannot install a record that closes the reader (issue #45). A book with an HD image container keeps a full-resolution copy of each picture in a CRES record and those are not rewritten, so the command says so and a rebuild from source is the way to replace both. Record 0 also keeps the size it arrived at: it ships with a long run of trailing zeros so that a later tool can add EXTH records without moving every record after it, and rebuilding it to a bare 4-byte boundary used to throw that away and take a 15 KB book down to 7 KB (issue #44). On a dual-format `.mobi` both section headers are rewritten, not just record 0: Kindle reads the library title from the KF8 section, so rewriting KF7 alone reported success and changed nothing on device. `--identifier` sets EXTH 113, the record the firmware keys a sideloaded book's cover on and the one a rewrite could not otherwise get right: a book built from a bare OPF stores its source `dc:identifier` nowhere, so without it the rewriter derives a stand-in from title and author and a rebuild of the same source writes something else (issue #46). A book built from an EPUB carries its source in a SRCS record, and the identifier is recovered from the OPF inside it automatically; the flag overrides that. An identifier already in the file still wins over both, so a device that has cached a thumbnail under it keeps matching. `--series` and `--series-index` are rejected; they wrote EXTH 112 and 113, which are the source identifier and the ASIN, and kindlegen emits no series record to copy instead. Multi-value flags like `--author` and `--subject` accept repeats to accumulate values.

The pass is byte-stable on no-op: if the requested updates match what is already in the file, or if no field flags are passed at all, the output is a `fs::copy` of the input with identical bytes. Downstream library managers that use content-hash identity for books can therefore call `rewrite-metadata` unconditionally when a user opens the metadata editor and closes it without changes. It is idempotent: running with the same updates a second time reports zero changes and produces a byte-identical output. DRM-protected files (PalmDOC encryption byte set, or EXTH 401/402/403 present) are rejected with exit code 1 and not touched; no DRM removal code is linked or referenced.

Unknown EXTH records in the input are preserved unchanged, so tool-specific metadata written by Calibre or kindlegen survives the rewrite pass.

### Build-time self-check

Every `build` and `comic` run now performs an HTML self-check on the assembled MOBI text blob before writing the output file. The check catches regressions like dangling `<body>` / `<mbp:frameset>` tags, `<hr/` corruption, and unclosed attribute quotes that would otherwise reach a user's Kindle as a white screen.

The self-check runs in two passes: once on the full assembled blob for structural corruption, and once over its tag nesting, which is followed across the whole text rather than inside each record. It reports a closing tag with nothing open and an element still open when the text ends, because those are corruption. It used to compare opens against closes inside each record, which is not a test of anything: a record boundary falls wherever 4096 bytes land, so an element that spans one was counted as unbalanced in both halves, and a 771k-headword dictionary drew 7188 warnings on markup that was provably correct (issue #51). A separate per-record check does still run, for a record that ends part-way through a tag, which would be a bug in kindling's own chunker rather than in the markup. Together the passes add ~50-200 ms to a large dictionary build. The check never aborts the build: when something is wrong, kindling prints a warning block pointing at the issue and writes the MOBI anyway, so you can still inspect the output.

```bash
kindling-cli build input.opf --no-self-check       # skip the self-check (not recommended)
kindling-cli comic input.cbz --no-self-check       # skip the self-check for comics
kindling-cli input.epub --no-self-check            # also works in kindlegen compat mode
```

A self-check warning indicates a likely kindling bug; please [open an issue](https://github.com/ciscoriordan/kindling/issues) with the failing OPF/EPUB if you hit one.

### Kindlegen compatibility

```bash
kindling-cli input.epub                          # same as kindlegen
kindling-cli input.epub -dont_append_source      # flag accepted
kindling-cli input.epub -o output.mobi           # explicit output path
kindling-cli input.opf  -no_validate             # skip KDP pre-flight validation
kindling-cli input.opf  -c2                      # accepted; output is PalmDOC, see below
```

Drop-in replacement. Same CLI syntax, same status codes (`:I1036:` on success, `:E23026:` on failure). The KDP pre-flight validator runs by default in kindlegen-compat mode too; pass `-no_validate` (or `--no-validate`) to skip it. `-c0`, `-c1` and `-c2` are all accepted; `-c2` asks for huffdic compression, which kindling writes only when `KINDLING_HUFFDIC=1` is set, so `-c2` on its own prints a note and produces PalmDOC output instead of failing.

### Dump

```bash
kindling-cli dump input.mobi      # structural dump to stdout
kindling-cli dump input.azw3
```

`kindling dump` prints the parsed structure of a MOBI/AZW3 file one `section.field = value` line at a time: PalmDB and MOBI header fields, every EXTH record, the INDX and ORDT2 tables, and entry labels. Text and image records are summarized by length and magic only. The line-oriented output is designed so `diff -u` between two dumps surfaces semantic differences without drowning in absolute-offset noise, which is how the kindlegen parity work is done. It is a read-only inspection tool and never writes to the input. On a huffdic file the dump also names the HUFF and CDIC records, the size of the phrase dictionary, and the total number of bytes the text records decompress to, which is the line that shows the compression model was understood rather than merely detected. It flags an orth index pointer that does not name the record the index is in, since that is invisible from the record list alone.

### Lookup simulator

```bash
kindling-cli lookup dict.mobi rivière   # prints how the word would resolve on-device
```

`kindling lookup` predicts what a Paperwhite 5 (firmware 5.19.2) opens when a word is tapped in a book whose language is the dictionary's input language, and prints the headword and its text position, or that nothing opens. The exit status is non-zero when nothing opens, so it works as a scriptable assertion. It follows the steps the Kindle 4 (firmware 4.1.4), the Paperwhite 4 (firmware 5.16.5) and the Paperwhite 5 were measured to take:

1. The tapped word is formatted for the book's language. A curly apostrophe becomes a straight one; characters before the first letter or apostrophe and after the last letter are dropped (`MP3` is looked up as `MP`, `ex-` as `ex`, `(casa)` as `casa`), and with a trailing apostrophe gone a leading one goes too (`'tis'` as `tis`); Italian and French books drop an elided word from a fixed list (`l'altro` as `altro`, `quest'anno` as `anno`, while `tutt'al più` and `aujourd'hui` stay whole); English books drop a final `'s`; French and Portuguese books drop a hyphenated clitic (`rendez-vous` as `rendez`, `dar-te-ei` as `darei`). A word that formats to nothing, such as `123` or `...`, is not looked up.
2. In a Russian dictionary the Paperwhite 4 and 5 lowercase the word, so `ДОМ` opens `дом` there, but not in a Ukrainian or Bulgarian dictionary, and not on the Kindle 4.
3. The word and every stored label are weighed character by character, with the same weights kindling sorts labels by: Latin letters fold case and accents, `ß`, `æ` and `œ` in the word count as `ss`, `ae` and `oe`, punctuation weighs nothing (`tshirt` finds `T-shirt`, `изпод` finds `из-под`), and Greek, Cyrillic and most other scripts weigh as they are written (`Κάτι` does not find `κάτι`). A prolonged sound mark, `ー` or halfwidth `ｰ`, counts as the vowel of the last kana before it that has one, so `ｹｰｷ` finds `ケーキ`, and `カンー` finds `カンア`, since `ん` and `ン` have none.
4. The index is binary-searched in the order its labels are stored, over the label that ends each index record and then within the record, and the search returns every adjacent label that weighs the same as the word. Nothing is matched by prefix. When labels are stored out of that order, the search can step past a label: the misplaced one, or a neighbor that is in its right place. When the index holds a label that weighs the same as the word but that the search cannot reach, the simulator names it, with the number of adjacent label pairs that are out of order.
5. One label of that run opens: the only one; else the one spelled exactly like the formatted word; else the first one equal to it ignoring case, but not accents, punctuation or spacing, under the book language's rules; else the longest label that equals the beginning of the word; else the first label that begins with the word; else the first label of the run. So in an Italian dictionary with both `T-shirt` and `tshirt`, `T-SHIRT` opens `T-shirt` and `TSHIRT` opens `tshirt`. A German book compares case as well, so there `E-MAIL` opens the first of `Email` and `E-Mail`, which is `Email`. Danish books keep the case of some letters, Turkish books tell `I` from `i`, Greek books ignore Greek case and Russian, Ukrainian and Bulgarian books Cyrillic case.

The Paperwhite 4 differs from the Paperwhite 5 only in which characters count as letters at the ends of a word, since it follows Unicode 9.0 where the Paperwhite 5 follows Unicode 12.1: a Georgian capital letter or a Myanmar tone mark at the end of a word stays on the Paperwhite 5 and is dropped on the Paperwhite 4. Firmware 5.19.6 picks among the returned labels by the default comparison rules whatever the book's language, so in a Polish book `WYKŁADNIĘ` opens `wykładnie` where the Paperwhite 5 opens `wykładnię`, and in a Russian dictionary `Абду-Салам` opens `абдусалам` where the Paperwhite 5 opens `абду-салам`; `kindling lookup` does not predict that, and only the unit tests simulate it.

The notes on stderr say how the word was formatted, the word searched for when a Russian dictionary lowercases it, and which labels the search returned, the first 20 of them when there are more. A note also says when the index holds Hangul jamo, whose weights are not modeled. The simulator opened the same label as the Paperwhite 4 on 19.7 million lookups in reader.dict's 23 monolingual dictionaries: every lookup in 13 of them and 50,000 or 100,000 in each of the others, in each dictionary as rebuilt from its source by this version, by an unreleased development version and by 0.45.1, whose indexes held labels out of order. The exceptions were 5,600 lookups in Japanese, where the device also looks up a word's dictionary form, 49 words made of characters added to Unicode after 9.0, which the Paperwhite 4 does not count as letters, and 1 Hangul jamo label. It did the same on the Paperwhite 4 and the Paperwhite 5 in small dictionaries in 16 languages and in spell-checker word lists in six languages, and on the Paperwhite 5 on 30,000 lookups sampled from each of the 23 dictionaries in each of those builds, except for 1,569 of the Japanese ones, where the device also looks up a word's dictionary form. The firmware 5.19.6 variant that the unit tests simulate matched firmware 5.19.6 on the same lookups, with the same Japanese exception. It does not model: the Japanese dictionary form and the retries with up to three trailing characters removed that the Paperwhite 4 and 5 apply in Japanese, Chinese and Korean books; the weights of the Hangul jamo; the parts of each language's comparison rules beyond those listed in [`src/lookup/collator.rs`](src/lookup/collator.rs) (a precomposed `é` does not equal `e` followed by a combining accent here, as it does on the devices); lookups from a book whose language differs from the dictionary's; and the Kindle 4's own differences, which are a narrower letter test at the ends of a word (it drops a final combining accent the Paperwhite 4 keeps), stop words such as `il` and `the` for which it shows no popup at all, and no Russian lowercasing. Implementation is in [`src/lookup.rs`](src/lookup.rs).

The MOBI header names the dictionary index at offset 0x18, but that pointer is verified before it is used and the index is otherwise found by its own signature, because a record number that was not adjusted for records inserted ahead of it lands on something else and every query then misses with nothing to say why (issue #49). A Kindle does not look past the header: in a file whose header names another record, or none, it opens nothing. So such a lookup is a miss, and the notes say why and what the index would open once the header is fixed. The dictionary languages in the header do not stop a lookup on the Paperwhite 4 and 5, which open words in a file that declares neither, and `kindling lookup` does the same; the Kindle 4 (firmware 4.1.4) opens nothing in a dictionary whose header declares no input language, which `kindling lookup` does not model. A miss says which kind of miss it is: a header a Kindle does not follow, a word that formats to nothing, an index that holds no matching headword, named along with its record and headword count, or a file that has no dictionary index at all. It also prints the first few headwords it decoded and the stored headwords where the search ended, because on someone else's dictionary the count alone is a dead end: headwords that come back as mojibake mean the label bytes were read wrong and no query could ever match, while headwords that read as ordinary words put the fault in the search instead. Notes about the file (a huffdic compression type, a stale index pointer, the headwords just described) go to stderr so stdout stays the single result line.

A production kindlegen dictionary carries both an SPL fold blob and a large ORDT table, and its labels are ORDT symbol sequences. Lookup used to take the fold blob as a sign that the labels were plain UTF-16, which is true only of kindling's own Greek dictionaries, and so read every label in a production dictionary as its raw symbol numbers: a 174685-headword German one resolved no query at all. It also now decodes kindlegen's expansion markers, the two-symbol form it stores ß, Œ, œ, Æ and æ in, which had left every headword containing one unreachable, 3433 of them in that dictionary (issue #49).

Dictionaries made by Mobipocket Creator, the tool kindlegen replaced, are read as well. Their index stores headwords as cp1252 text, one byte per character, under a header that can be as short as 164 bytes and names no index, and a busier index carries two control bytes per entry rather than one. kindling read those labels as UTF-16, so "keep" came back as two CJK characters, and for most Creator files it never found the index at all. A Kindle weighs such labels byte by byte through the weight table the index itself carries rather than by the Unicode weights above, and writes the tapped word in cp1252 to weigh it the same way, and the simulator does the same. The simulator does not follow a separate inflection index, which Creator and kindlegen both write and kindling does not, so in such a dictionary an inflected form resolves only if it is also a headword.

## Performance and Comparisons

### vs kindlegen

| Input | *kindlegen* | Kindling | Speedup |
|---|---|---|---|
| Greek dictionary (80K headwords, 452K entries) | 12+ hours, frequent OOM | 6 seconds | ~7,000x |
| Divine Comedy (138 illustrations, 29MB of images) | 19 seconds | 0.5 seconds | ~40x |
| Pepper & Carrot comic (20 images) | 1.4 seconds | 0.05 seconds | ~30x |

The ~7,000x dictionary speedup comes from skipping *kindlegen*'s complex inflection index computation (which scales superlinearly) and avoiding Rosetta 2 overhead on Apple Silicon. The gap is largest for heavily-inflected languages (Greek, Finnish, Turkish, Arabic) with hundreds of thousands of forms.

### vs KCC

| | KCC | Kindling |
|---|---|---|
| Installation | Python + PySide6 + Pillow + 7z + mozjpeg + ... | Single binary, no dependencies |
| Binary size | ~200MB+ (with dependencies) | ~5MB |
| Image processing | Python/Pillow (multiprocessing with pickling overhead) | Rust/image + rayon (parallel, zero serialization) |
| Headless/CI | No (GUI-only, CLI is an afterthought) | CLI-first, scriptable |
| Apple Silicon | Rosetta for some dependencies | Native |
| Comic conversion (200 pages) | ~30 seconds | ~3 seconds |
| Kindle Scribe | 1920px height limit (kindlegen restriction) | Full 2480px native, no height limit |
| Image format | PNG/JPEG (PNG causes blank pages on Scribe) | JPEG only (safest for all Kindle devices) |
| Volume splitting | Buggy size estimation, premature splits | Always single file |
| Webtoon support | Yes | Yes |
| Panel View | Yes | Yes |
| Manga RTL | Yes | Yes |
| ComicInfo.xml | Yes | Yes |
| Kindle Previewer compat | No (separate step) | Built-in (EPUB embedded by default, `--no-embed-source` to save space) |

## Ordered list markers

The Kindle lookup popup draws no list marker of its own from CSS or from an `<ol type>` attribute. It does honor `<li value="N">`, which is what kindlegen emits and what every ordered list gets here by default.

A level that declares a lettered or roman style, `<ol style="list-style-type:lower-alpha">` or the legacy `<ol type="a">`, is not written as a list at all: each item becomes a plain line with its marker as text, `a. `, `i. ` and so on, and every nested literal-marker level gains three non-breaking spaces of indentation (issue #56). That is the shape Amazon's own Oxford dictionary uses for the same job, distinguishing a sub-sense with a literal character rather than with list markup, and it is the same conclusion the dictionary stylesheet compiler reached: the popup renders literal text and ignores markup it would have to interpret. It also leaves no ordered item without a number. 0.44.0 wrote those levels as list items with no `value`, which firmware 5.18.1 draws with no marker of its own but 5.19.2 draws as `65535.`, so every `<li>` left in an ordered list now carries a `value`.

Letters count bijectively, so the twenty-seventh item is `aa` rather than `ba`, and roman numerals past 3999 fall back to the number instead of a wrong numeral.

## How inflection lookup works

Kindling places all lookupable terms (headwords + inflections) directly into the orthographic index. Each inflected form entry points to the same text position as its headword:

| Orth index entry | Points to |
|---|---|
| cat | text position of "cat" entry |
| cats | text position of "cat" entry |
| cat's | text position of "cat" entry |
| θάλασσα | text position of "θάλασσα" entry |
| θάλασσας | text position of "θάλασσα" entry |
| θάλασσες | text position of "θάλασσα" entry |
| θαλασσών | text position of "θάλασσα" entry |

Looking up any form on the Kindle finds the correct dictionary entry.

This means kindling writes **no separate inflection index**, and the inflection index field in the MOBI header (offset 28) is `0xFFFFFFFF`. kindlegen writes one and puts the `<idx:infl>` rules in it; kindling reaches the same place by the flatter route above. That field is the first thing anyone notices when comparing a kindling dictionary against a kindlegen one, and its absence looks like the reason inflected lookup would fail, so it is worth being explicit: it is not. `--headwords-only` is the flag that actually drops inflected forms from the index, and it is off by default.

*kindlegen* takes a different approach: a separate inflection INDX with compressed string transformation rules that map inflected forms back to headwords. This encoding is undocumented, limited to [255 inflections per entry](https://ebooks.stackexchange.com/questions/8461/kindlegen-dictionary-creation) (uint8 overflow), and adds complexity without benefit. Kindling has no per-entry limit.

## MOBI Format

Kindling works with the KF7/MOBI format used by Kindle e-readers. The key structures are:

- **PalmDB header**: Database name, record count, record offsets
- **Record 0**: PalmDOC header + MOBI header (264 bytes) + EXTH metadata + full name
- **Text records**: PalmDOC LZ77 compressed HTML with trailing bytes (`\x00\x81`). Kindling writes PalmDOC (type 2) or uncompressed (type 1), and reads HUFF/CDIC (type 17480) as well
- **INDX records**: Orthographic index with headword entries, character mapping, and sort tables
- **Image records**: JPEG/PNG with JFIF header patching for Kindle cover compatibility, each capped at 128 KB. The reader decodes an image record into a fixed buffer, and one over that limit closes the reading app when the page carrying it scrolls into view instead of just rendering blank (issue #25). Oversized images are re-encoded to fit at the same pixel dimensions and the untouched original moves into the HD container, which is what kindlegen does. The re-encode starts at whatever quality you asked for (`--jpeg-quality` on comics) rather than a fixed rung, and prints a line naming the image, the quality it came out at, and the new dimensions if quality alone could not get it under the cap and the geometry had to shrink too (issue #38)
- **KF8 section**: Dual-format output with BOUNDARY record, KF8 text, FDST, skeleton/fragment/NCX indexes. The FDST table is sized to the flows the book actually has (one for a book with no stylesheet, two with CSS), never declaring a zero-length flow, and EXTH 125 carries the real resource-record count; a hardcoded 2-flow table plus a fake count made a minimal no-CSS, no-image book fail to open on device with "Unable to Open Item"
- **HD container**: CONT/CRES records for high-DPI Kindle screens. Only images whose main record had to be re-encoded to fit the 128 KB cap get a real CRES slot holding the original; the rest are 4-byte placeholders, since a CRES that duplicates its own image record is pure file bloat. A book where nothing needed re-encoding gets no container at all, matching every kindlegen reference build
- **FLIS/FCIS/EOF**: Required format records

### Key format details

Much of the foundational MOBI format knowledge comes from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI). The dictionary-specific details below were worked out empirically while building this project.

- **Text record sizing**: Every PalmDOC text record (except the last) must satisfy two constraints simultaneously:
  1. **Exactly 4096 bytes of decompressed content** (matching the declared `text_record_size` in the PalmDOC header). Kindle firmware routes popup lookups by computing `record_idx = byte_offset / text_record_size`, treating `text_record_size` as a constant. Records that drift below 4096 (e.g. by backing off to UTF-8 character boundaries) accumulate a per-record offset error that misroutes popup queries to wrong entries, and the further into the alphabet the query, the worse the drift. Records significantly shorter than 4096 (e.g. by backing off to `<hr/>` entry separators) break routing entirely.
  2. **Each record individually decodes as valid UTF-8 and parseable HTML.** Kindle's library indexer parses each record independently and will silently refuse to index a dictionary above some threshold of mid-character or mid-tag splits. Basic dictionaries with ~16% bad records still index; pro dictionaries with ~25% bad records do not.

  Above roughly 266 MB of text the chunk size scales past 4096 (to 8192, then 16384) so the record count still fits the header's 16-bit field, and the header declares whatever size was used. It used to always declare 4096, which misrouted every lookup in a dictionary that large by a factor of two or more (issue #32).

  Kindling satisfies both constraints by emitting fixed-size 4096-byte chunks but inserting ASCII space padding at HTML inter-element gaps (between a `>` and the next `<`) so each chunk ends just past a complete tag close. The padding sits in HTML inter-element whitespace zones, which parsers collapse, so it has no rendering impact and never lands inside a text run in a way that breaks entry-position lookup: entries are anchored on the bytes they contributed to the blob, and the anchor is split at the same `><` junctions the padder uses so a padded entry still matches.
- **Trailing bytes** (`\x00\x81`): Every text record ends with a multi-byte flag byte (`0x00`) followed by a TBS byte (`0x81`) as the very last byte. The Kindle decompressor walks backward from the end of the record, consuming the TBS byte first (bit 1 of `extra_flags`), then the multi-byte tail (bit 0); this ordering is mandatory. Earlier kindling builds wrote these bytes in reverse order and produced a white screen on device.
- **HUFF/CDIC ("huffdic") compression**: text records compressed with a static Huffman code over a shared phrase dictionary, PalmDOC compression type 17480. One `HUFF` record carries the code tables and `huff_rec_count - 1` `CDIC` records carry the phrases; both are found through record numbers at MOBI header 0x60 and 0x64 that are relative to the section's own record 0, so the KF8 half of a dual-format file adds the boundary. kindlegen places the block after the whole index and immediately before the first image record, so the index records never move. The `HUFF` tables are a 256-entry lookup keyed by the top 8 bits of a 32-bit code window plus per-length `mincode`/`maxcode` bounds for the codes those 8 bits cannot resolve; a symbol's phrase index is `maxcode - code`, so symbols run backwards through each length's code range, and the bounds are stored pre-shifted, which overruns 32 bits at short code lengths and is why kindling computes them in `u64`. Every `CDIC` declares the *total* phrase count rather than its own, so a record contributes `min(1 << bits, total - loaded)` phrases and the last one's offset table has stale slots that must not be read. A phrase whose length word lacks the 0x8000 flag is itself a compressed bitstream and expands through the same decoder. Trailing data entries come off before decompression, exactly as for PalmDOC. Cross-checked against KindleUnpack, calibre and libmobi, and validated against a real kindlegen 2.9 huffdic file whose uncompressed twin ships beside it in libmobi's corpus (`scripts/validate_huffdic_fixtures.sh`).
- **DATP for huffdic text**: a PalmDOC record can be sized by decompressing it, but a huffdic record cannot without walking its whole bitstream, so a huffdic file also carries a `DATP` record listing the decoded length of every text record, and record 0 names it at 0x78 with a count of 1 at 0x7C. Its layout, read off kindlegen 2.9 output: the magic, a word of 12, the bytes 1 and 4, the text record count, the total text length, a zero word, a word whose meaning is not known, then one `u16` per text record. kindlegen puts it immediately before `FLIS`, which also makes it the last content record. calibre and KindleUnpack decode without it, but Mobipocket Reader for Windows calls a huffdic file "File corrupted" when it is missing, and, in a dictionary, when the unknown word is zero, though every other value tried opens: kindlegen's own files carry values like 64925 and 65376 there, and kindling writes 64925. The `HUFF` header points at its code tables twice, through four offsets: the prefix table and the per-length bounds big-endian, then the same two tables byte-swapped. Every reader here uses only the first pair; kindling writes both, as kindlegen does.
- **Inverted VWI**: Tag values use "high bit = stop" encoding (opposite of standard VWI).
- **SRCS record**: Must have 16-byte header (`SRCS` + length + size + count), pointed to by MOBI header offset 208. Required for Kindle Previewer.
- **Skeleton and fragment INDX (KF8)**: KF8 HTML is split into a "skeleton" per source file and one fragment per `<aid>` insert point. Skeleton entries carry a byte offset, length, and fragment count; fragment entries use a numeric decimal label (parsed as an integer) plus insert position, file number, sequence, and length.
- **Orth INDX header**: The orth INDX primary header declares index encoding `65002` (0xFDEA) and carries the input language's Windows primary LCID at offset 32. Japanese, Chinese, Korean and Arabic-script dictionaries store their labels as symbols of the generated tables below. Latin-script dictionaries not built with `--fold-accents`, and every other dictionary built with `--strict-accents`, store each character as a symbol of an exact per-character ORDT table. All other dictionaries store UTF-16BE labels. A Kindle reads a UTF-16BE label unit below the header's `oentries` count (offset 168) through the ORDT2 table, so the Greek and Latin `--fold-accents` builds, which embed the kindlegen ORDT/SPL blob, carry a 7-entry ORDT2 table that maps the values 0 to 6 to themselves. That keeps the markers that store `ß`, `æ` and `œ` intact; the blob's own 4-entry ORDT2 table turned them into other characters. Cyrillic, Hebrew, Armenian and other dictionaries carry no ORDT2 table, and their labels are read as stored.
- **Generated ORDT tables**: For Japanese, Chinese, Korean, and Arabic dictionaries, labels are sequences of one element per character indexing a generated ORDT table pair appended to the primary record (`ordt_type` at offset 164, count at 168, table offsets at 172/176). A character with a table symbol (kana, plus NUL/`%`/`_`) is stored as its symbol index; ORDT2 maps that symbol to the character's Unicode code point and ORDT1 to its gojuon collation weight, with katakana folded onto the matching hiragana weight (ヴ ヵ ヶ collate as う か け). The prolonged sound mark ー is folded before encoding: it becomes a vowel-specific marker (U+3095..U+3098, U+309F) carrying the preceding vowel's weight, propagating across consecutive ー, and staying an ignorable weight-0 symbol when no vowel is known (word start, after ん/ン, after ・, a halfwidth katakana or a character that is not kana; the halfwidth mark ｰ is never folded); the middle dot ・ and the iteration marks are kept as weight-0 symbols too. kindlegen folds the mark after a kana the same way, and a Kindle searches for a tapped ー that way too, so katakana names with long vowels resolve on device. A Kindle carries the vowel past ん, ン, ・ and characters that are not kana, takes it from halfwidth katakana, and folds ｰ the same way, so a label with the raw mark in those places, or with ｰ after kana, is not found by its own spelling. Every other character (kanji, Hangul, Arabic letters) is stored as an out-of-table literal: a label element is a literal exactly when its value is `>= oentries`, and a supplementary-plane character becomes its UTF-16 surrogate pair (two elements), matching kindlegen. The full hiragana and katakana blocks are embedded only when the dictionary contains kana; all-literal scripts get the minimal table kindlegen emits, three entries (NUL, `%`, `_`) plus a weighted space symbol when any headword contains a space, since a large kana table makes the firmware mis-collate them. Elements are one byte each (`ordt_type` 1) unless a literal is present or the table exceeds 256 symbols, in which case they are big-endian u16 (`ordt_type` 0). The firmware encodes a tapped word the same way, so kana fold and kanji match exactly. The ORDT table and headword labels are byte-identical to kindlegen for the all-literal scripts at fixture scale; for Japanese the kana symbol numbering differs (it has no effect, the firmware resolves kana by code point), while the literal code points and the collation order match. On production-size dictionaries kindlegen instead emits a large weighted collation table (160 entries for a 262k-entry Korean build, 617 for a 174k-entry German one; Amazon's own store-published Chinese dictionary carries the same canned-base-plus-hiragana table shape) and re-sorts the whole index by those weights. kindling deliberately does not reproduce that: on-device A/B testing for issue #22 showed the firmware resolves kindling's minimal-table layout correctly at production scale for Chinese, and for Korean both layouts fail identically because the firmware has no Korean lookup support, so the big table buys nothing. See `src/ordt.rs`.
- **Routing entries**: Each primary-INDX routing entry is `[1 byte label length][label bytes][2 byte big-endian record entry count]`. The trailing count is what lets Kindle's binary search pick the right data record for a lookup.
- **Dictionary links**: The lookup popup disables anchor links; see the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf), section 16.6.1. The guidelines say they still work when the dictionary is opened as a book, but kindling does not yet resolve them on the dictionary path, so today they do nothing there either (issue #54). Books are unaffected: their links resolve (issue #50).

### MOBI header fields

All offsets are relative to the MOBI magic (`MOBI` at byte 16 of Record 0).

| Offset | Size | Field | Notes |
|--------|------|-------|-------|
| 0 | 4 | Magic | Always `MOBI` |
| 4 | 4 | Header length | 264 bytes for Kindling output |
| 8 | 4 | MOBI type | 2 for both books and dictionaries |
| 12 | 4 | Text encoding | 65001 = UTF-8 |
| 16 | 4 | Unique ID | MD5 hash of title |
| 20 | 4 | File version | 6 for KF7 dictionaries, 8 for KF8 books |
| 24 | 4 | Orth index record | First INDX record for dictionary lookup. `0xFFFFFFFF` = no dictionary |
| 28 | 4 | Inflection index | `0xFFFFFFFF` (Kindling uses orth-only, no inflection INDX) |
| 64 | 4 | First non-book record | First record after text (images, INDX, etc.) |
| 68 | 4 | Full name offset | Byte offset of full title within Record 0 |
| 72 | 4 | Full name length | Length of full title in bytes |
| 76 | 4 | Language code | Locale code for book language |
| 80 | 4 | Input language | Dictionary source language locale code |
| 84 | 4 | Output language | Dictionary target language locale code |
| 88 | 4 | Min version | Minimum reader version required |
| 92 | 4 | First image record | First image record index |
| 112 | 4 | Capability marker | `0x50` for dictionaries (Kindle device recognition), `0x4850` for books (Kindle Previewer compatibility) |
| 208 | 4 | SRCS index | Record index of embedded EPUB source. `0xFFFFFFFF` = none |

### EXTH records

EXTH records are type-length-value metadata entries in Record 0, following the MOBI header.

| Record | Name | Used in | Value | Notes |
|--------|------|---------|-------|-------|
| 100 | Author | Both | UTF-8 string | |
| 103 | Description | Books | UTF-8 string | Maps to ComicInfo.xml `<Summary>` |
| 105 | Subject | Both | UTF-8 string | Maps to ComicInfo.xml `<Genre>`. A sideloaded dictionary has to carry one or the device does not list it in the lookup popup's dictionary selector: the OPF's `<dc:subject>` when it declares one, `Dictionaries` otherwise |
| 106 | Publishing date | Both | UTF-8 string | |
| 112 | Source identifier | Books | UTF-8 string | Calibre writes `calibre:<uuid>` here. Never written by kindling; it is not a series field |
| 504, 508, 517-519, 534 | No verified meaning | - | - | Proposed at various times as the series slot. None is written, and a test pins that: 504 is a second copy of the ASIN in the Amazon-delivered files available here, 534 is Amazon's own input-pipeline tag, and 508/517/518/519 appear in no file anyone here has. kindlegen writes no series record either, though its binary dates from 2015 and the Kindle library's Series feature is later, so that is evidence about kindlegen rather than about the container (issue #48) |
| 113 | ASIN | Books, comics | UTF-8 string | Written only alongside a 501. The pair names the device-side cover file (`system/thumbnails/thumbnail_<113>_<501>_portrait.jpg`), which is what the lock screen shows. Value is a UUID the EPUB already publishes, or one derived from its metadata so rebuilds stay stable (issue #26) |
| 121 | KF8 boundary | Books | u32 BE | Record index of KF8 Record 0 |
| 122 | Fixed layout | Books | `"true"` | Present only for fixed-layout content |
| 125 | (unknown) | Both | u32 BE | 1 for dictionaries, 21 for books |
| 131 | (unknown) | Both | u32 BE = 0 | |
| 201 | Cover offset | Books | u32 BE | Image record offset for cover |
| 202 | Thumbnail offset | Books | u32 BE | Image record offset for thumbnail |
| 204 | Creator platform | Both | u32 BE | 201 = Mac (*kindlegen* compat), 300 = Kindling |
| 205 | Creator major version | Both | u32 BE | |
| 206 | Creator minor version | Both | u32 BE | |
| 207 | Creator build | Both | u32 BE | |
| 300 | Fontsignature | Dicts | 242 bytes | LE USB/CSB bitfields + shifted codepoints. Tells firmware which Unicode ranges the dictionary covers |
| 307 | Resolution | Books | UTF-8 string | Fixed-layout viewport resolution (e.g. `"1072x1448"`) |
| 501 | Document type | Books, comics | ASCII string | See table below. Omitted by default for every content type. Its presence hides the reader's back-to-library toolbar (issue #15), so it is opt-in via `--doc-type`. Always omitted for dictionaries (*kindlegen* omits it; Kindle recognizes dicts via orth index + EXTH 547) |
| 524 | Language | Both | UTF-8 string | BCP47/ISO 639 language code |
| 525 | Writing mode | Both | UTF-8 string | `"horizontal-lr"` or `"horizontal-rl"` |
| 527 | Page progression | Books | UTF-8 string | Fixed-layout page direction |
| 531 | Dict input language | Dicts | UTF-8 string | Source language ISO 639 code (e.g. `"el"`) |
| 532 | Dict output language | Dicts | UTF-8 string | Target language ISO 639 code (e.g. `"en"`) |
| 535 | Creator string | Both | UTF-8 string | `"0730-890adc2"` for *kindlegen* compat, `"kindling-X.Y.Z"` with `--creator-tag` |
| 536 | HD image geometry | Books | UTF-8 string | `"WxH:start-end\|"` format for HD image container |
| 542 | Content hash | Both | 4 bytes | MD5 prefix of title |
| 547 | InMemory | Both | `"InMemory"` | Required. Activates dictionary lookup for dicts. Also written for books |

### EXTH 501 values (document type)

Controls where the content appears on the Kindle home screen.

| Value | Meaning | Notes |
|-------|---------|-------|
| `EBOK` | Books shelf | Warning: Amazon may auto-delete sideloaded EBOK files when the Kindle connects to WiFi, since it checks whether the ASIN is in the user's purchase history |
| `PDOC` | Documents shelf | Safe default for sideloaded content |

Dictionaries do NOT use EXTH 501. The Kindle identifies dictionaries by the combination of a valid orth index (MOBI header offset 24), EXTH 105 subject, EXTH 531/532 language records, and EXTH 547 `InMemory`. Adding an unrecognized EXTH 501 value (e.g. `"DICT"`) can prevent the Kindle from recognizing the file as a dictionary.

## Project layout

Standard Rust layout with `Cargo.toml` at the repo root:

```
kindling/
├── Cargo.toml                   # edition 2024, Rust 1.85+
├── src/
│   ├── lib.rs                   # Library crate root, public API for external consumers
│   ├── main.rs                  # CLI: build, comic, stardict, epub2, epub3, validate, repair, rewrite-metadata, thumbnail, dump, lookup, kindlegen-compat
│   ├── mobi.rs                  # PalmDB + MOBI record 0 + EXTH writer, UTF-8/tag-safe record splitter
│   ├── mobi_check.rs            # Post-build MOBI readback: PalmDB, EXTH, text-record sanity
│   ├── mobi_rewrite.rs          # In-place MOBI/AZW3 metadata and cover rewrite
│   ├── thumbnail.rs             # Cover thumbnail writer for a mounted Kindle (the `thumbnail` subcommand)
│   ├── mobi_dump.rs             # Structural dump of a MOBI/AZW3 (the `dump` subcommand)
│   ├── lookup.rs                # On-device lookup simulator (the `lookup` subcommand)
│   ├── lookup/                  # Simulator parts: word formatting by book language (format.rs) and per-language word equality (collator.rs)
│   ├── kf8.rs                   # KF8 section, BOUNDARY, FDST, skeleton/fragment indexes
│   ├── cncx.rs                  # CNCX string records for KF7/KF8 navigation
│   ├── nav.rs                   # EPUB nav document / NCX parsing for the on-device TOC
│   ├── indx.rs                  # Orthographic INDX records for dictionaries (ORDT/SPL sort tables)
│   ├── ordt.rs                  # ORDT tables, label encoding, and the character weights every other index is sorted by
│   ├── palmdoc.rs               # PalmDOC LZ77 compression
│   ├── huffcdic.rs              # HUFF/CDIC (huffdic) decompression, compression type 17480
│   ├── huffdic_encode.rs        # HUFF/CDIC compression: phrase dictionary, Huffman tables, bitstreams
│   ├── exth.rs                  # EXTH record encoding
│   ├── vwi.rs                   # Variable-width integer encoding
│   ├── links.rs                 # Internal link resolution shared by the filepos and kindle:pos writers
│   ├── dict_css.rs              # Compiles a dictionary stylesheet into the inline markup the lookup popup honors
│   ├── opf.rs                   # OPF and EPUB parsing (Method 1 and Method 2 covers)
│   ├── epub.rs                  # EPUB extraction for books and comics
│   ├── extracted.rs             # Normalized in-memory view of an extracted EPUB/OPF
│   ├── epub_build.rs            # EPUB2/EPUB3 output builders (generic book + EPUB3 dictionary layer)
│   ├── fonts.rs                 # EPUB font embedding for KF8, with IDPF/Adobe deobfuscation
│   ├── comic.rs                 # Comic pipeline (crop, split, enhance, Panel View)
│   ├── profile.rs               # Per-device comic profiles (screen size, gamma)
│   ├── cbr.rs                   # CBR (RAR) extraction via bsdtar
│   ├── moire.rs                 # Moire correction for color e-ink
│   ├── validate.rs              # KDP pre-flight driver; iterates `checks::CHECKS`
│   ├── checks/                  # One Rust module per rule cluster, all impl `Check`
│   ├── repair.rs                # Structural EPUB repair pass for Kindle ingest
│   ├── stardict.rs              # StarDict 2.4.2 builder (.ifo/.idx/.dict/.syn) for GoldenDict, KOReader, sdcv
│   ├── kdp_rules.rs             # Rule catalog (KPG_VERSION, Rule struct, RULES array)
│   ├── html_check.rs            # HTML/XHTML self-check for the assembled MOBI text blob
│   ├── ordt_greek.bin           # Embedded ORDT/SPL sort tables extracted from kindlegen output
│   └── tests.rs                 # Unit tests
├── tests/
│   ├── cli.rs                   # CLI smoke tests for validate, repair, rewrite-metadata, build, comic
│   ├── kindlegen_parity.rs      # Byte/field parity vs committed kindlegen reference .mobi files
│   ├── dict_languages.rs        # Per-language dictionary tests (en/el/fr/ru/tr/ja/zh/ko/ar)
│   ├── roundtrip.rs             # Structural round-trip of kindling output via inline MOBI reader
│   ├── huffdic.rs               # HUFF/CDIC decoding and the stale index pointer of issue #49
│   ├── links.rs                 # Footnote and cross-reference links resolve to the right element (issue #50)
│   ├── lookup.rs                # Lookup simulator against the language fixtures and small dictionaries built in the test
│   ├── stardict.rs              # StarDict bundle structure (.ifo/.idx/.dict/.syn)
│   ├── epub_conformance.rs      # EPUB2/EPUB3 output structure and dictionary-layer checks
│   ├── epub_tests_corpus.rs     # Opt-in w3c/epub-tests corpus harness (KINDLING_CORPUS_DIR)
│   ├── common/                  # Inline MOBI reader used by roundtrip and parity tests
│   └── fixtures/                # One fixture per rule cluster plus the parity/ subtree
└── target/release/kindling-cli  # compiled binary
```

## Testing

Tests run automatically on every push and pull request via [GitHub Actions](.github/workflows/test.yml). All `cargo` commands run from the repo root.

```bash
cargo test                    # full suite
cargo test -- --show-output   # include println! output
cargo test --test cli         # CLI smoke tests only
```

Two checks live outside the suite because they need tools CI does not carry. `scripts/readback_sweep.sh` builds every fixture in both formats and reads each result back with calibre, which is the one question kindling's own tests cannot ask: they can all pass on a file no reader can open. That is not hypothetical. The huffdic encoder round-tripped through kindling's decoder, and calibre's decoder read the same streams perfectly, while calibre's conversion pipeline lost more than half the text, because the pipeline strips each record's trailing entries first the way a device does and the records were missing them. The sweep currently builds 116 files and reads back all of them. It also builds every dictionary fixture as a StarDict bundle and looks its headwords up with `sdcv`, which has to be a binary-searching reader rather than a sequential one: an index sorted the wrong way leaves headwords in the file that only a binary search fails to reach, and both kindling's own tests and pyglossary read those indexes sequentially and accepted a broken one (issue #60). And `tests/epub_tests_corpus.rs` runs the validator over a [w3c/epub-tests](https://github.com/w3c/epub-tests) checkout: across its 205 tests every error-level rule that fires is correct, `R17.1` and `R6.3` on the scripting tests and `R7.1` on genuinely unlisted resources, so the validator has no known false positives on that corpus.

CI runs `cargo fmt --check`, the full test suite, and a third job that installs epubcheck and runs the EPUB conformance test that is `#[ignore]`d by default because it needs a JVM. That job exists because the structural tests cannot see a dangling href or an image resource missing from the archive; only epubcheck can, and its test stayed unrun until someone ran it by hand, at which point it found seven errors on one fixture (issues #55 and #58's neighbor, the images the exporter carried nowhere). The epubcheck release is pinned and its checksum verified before the jar is run.

The suite currently contains over 1000 tests spanning unit tests in `src/tests.rs` and per-cluster tests in `src/checks/`, CLI integration tests in `tests/cli.rs` that invoke the compiled `kindling-cli` binary against OPF/EPUB/MOBI fixtures under `tests/fixtures/`, structural round-trip tests in `tests/roundtrip.rs`, per-language dictionary tests in `tests/dict_languages.rs`, internal link tests in `tests/links.rs`, and kindlegen parity tests in `tests/kindlegen_parity.rs`. An opt-in corpus harness in `tests/epub_tests_corpus.rs` runs every test in a local [w3c/epub-tests](https://github.com/w3c/epub-tests) checkout through the validator to surface false positives and measure coverage; set `KINDLING_CORPUS_DIR` to the checkout path and run `cargo test --release --test epub_tests_corpus -- --ignored --nocapture`.

- **PalmDB and MOBI structure**: PalmDB header fields, record count and offset tables, MOBI header (magic, version, encoding, language, capability marker 0x50 vs 0x4850), text record count, image record ranges, boundary records, FLIS/FCIS/EOF/SRCS records, trailing byte order
- **Record 0 cross-checks**: MOBI header offsets are internally consistent with the PalmDOC header, EXTH block, full name, and image/INDX record indexes
- **Dictionary output**: Orth INDX presence and structure, headword count, EXTH 531/532/547 language and `InMemory` records, EXTH 201 cover embedding, compressed and uncompressed roundtrips
- **Book and KF8 output**: KF8-only `.azw3` output (default for non-dictionaries), legacy dual KF7+KF8 format via `--legacy-mobi` (BOUNDARY record, KF8 section version), image record JPEG magic, complete EXTH metadata set, SRCS embedding
- **EXTH records**: Every documented EXTH record in the table above is checked for both dictionaries and books, including KF8-only cases
- **HTML/XHTML validation**: Text blobs extracted from MOBI output are reparsed with a relaxed quick-xml pass plus a custom balanced-tag walker, catching unclosed tags, malformed `<hr/`, unclosed attribute quotes, and stray `<` / `>`
- **KDP validator**: each rule cluster under `src/checks/` ships unit tests alongside its module, asserting both the positive case (rule fires on bad input) and the negative case (clean input passes) for every rule id the cluster owns
- **CLI smoke test**: `tests/cli.rs` builds the `kindling-cli` binary via Cargo and runs `validate` against the clean fixtures (`clean_book`, `clean_dict`) plus one error fixture per Phase 2 rule cluster (`book_with_errors`, `book_with_warnings`, `parse_encoding_errors`, `legacy_dict_errors`, `fixed_layout_errors`, `fixed_layout_missing_opf`, `cross_refs_errors`, `filename_errors`, `css_forbidden_errors`, `image_integrity_errors`, `opf_grammar_errors`), asserting exit codes and that the expected rule ids appear in stdout
- **Comic pipeline**: Device profiles (including kpw5, scribe2025, kindle2024), spread detection and splitting, crop-before-split symmetry, margin cropping, auto-contrast, moire wiring for color devices, webtoon merge/split with overlap fallback, dark gutter detection, Panel View markup, manga RTL ordering and cover selection, JPEG quality, ComicInfo.xml parsing, EPUB image extraction
- **Comic CLI flags**: doc-type EBOK/PDOC, title/author/language overrides, `--legacy-mobi` opt-in for legacy dual-format output
- **Compression**: PalmDOC LZ77 compress/decompress roundtrips for various sizes and encodings, and HUFF/CDIC decoding (`tests/huffdic.rs`), where the committed huffdic fixtures must decompress byte for byte to the text of the PalmDOC dictionaries they were transcoded from, with unit coverage for truncated tables, a lone HUFF record, zero-length codes, and self-referential phrases
- **Regression tests**: Dictionary capability marker (0x50 vs 0x4850), JFIF density patching, RTL spread cover selection, dictionary text record trailing byte order
- **Structural round-trip tests** (`tests/roundtrip.rs`): build each of the three parity fixtures with `kindling-cli`, parse the result back with a minimal inline MOBI reader in `tests/common/mod.rs`, and assert the PalmDB header, MOBI header, EXTH, INDX / SKEL / FRAG records, and decompressed text blob have the exact shape we expect. These catch format-level regressions where libmobi would happily accept an output that does not round-trip.
- **Internal link tests** (`tests/links.rs`): build the `footnote_links` and `cover_page_links` fixtures in both output formats and follow every link the way a reader would, checking that each one lands on the element the source EPUB's fragment named. The fixture reuses fragment names across its documents on purpose, so a resolver that looks them up in one table across the whole book produces live links that go to the wrong place, and the assertions catch that rather than settling for "points at some element". The footnote round trip is checked in both directions: a chapter's marker reaches its own note, and that note's back-link reaches the marker that sent the reader there.
- **kindlegen byte/field parity tests** (`tests/kindlegen_parity.rs`): build the same inputs with `kindling-cli` and diff the output field-by-field against a committed kindlegen reference `.mobi`. Timestamp/UID fields (EXTH 112, 113, 204-207, etc.) are compared by presence only; core metadata (EXTH 100, 101, 524) must match exactly. Divergences are reported in a readable table via `cargo test -- --nocapture`.
- **Per-language dictionary tests** (`tests/dict_languages.rs`): build a dictionary for each supported language from its `tests/fixtures/langs/<code>/` fixture and assert the INDX/MOBI language ids and locale, that every entry has a non-empty text pointer (the first-entry white-page guard), that headwords round-trip through the labels, and the per-language collation. For the generated-ORDT languages (ja, zh, ko, ar) decode every per-character label back to its headword and assert byte parity with the committed kindlegen build: the ORDT table and orth headword labels are identical for the all-literal scripts, and the literal code points plus collation order match for Japanese.

### On-device checks

Some things only a Kindle can answer. Issues carrying `needs-device-check` have a
fix in the tree that nobody has watched run on hardware yet, and the tracker
holds them open until someone has. `tests/fixtures/device/generate.py` builds a
whole round of those checks in one go:

```bash
KINDLING=./target/release/kindling-cli python3 tests/fixtures/device/generate.py
cp tests/fixtures/device/build/ship/*.mobi /Volumes/Kindle/documents/
```

It writes six dictionaries, five books, three comics and a probe book listing
every word to tap. Every dictionary declares `en` to `en` and the probe
book is tagged `en`, because the lookup popup's picker only lists dictionaries
whose input language matches the book's language tag; that is what lets one book
drive all six. The output is gitignored and rebuilt from source each run.

These are deliberately not the repo's own fixtures. `clean_book` is a single
432-byte page, so "it opens but won't turn pages" looks like a bug and is just a
one-screen book, and the parity comic pages are flat color rectangles that read
as a rendering failure on e-ink. Both wasted a device round. The fixtures here
are built to be looked at, and each one is checked against a pre-fix binary
first so a test that cannot fail never reaches the hardware.

### Git hooks

The repo ships two optional hooks in `.githooks/` that run the CI checks before they can turn a build red. Enable them once per clone:

```bash
git config core.hooksPath .githooks
```

`pre-commit` refuses a commit whose staged Rust code is not rustfmt-clean. `pre-push` re-runs that check plus `cargo check --all-targets`; the full test suite stays in CI. Both take `--no-verify` if you need to get past them.

### kindlegen parity setup

The parity tests do NOT invoke kindlegen at test time. Instead, each parity fixture ships a committed `kindlegen_reference.mobi` alongside the OPF/EPUB/CBZ sources:

```
tests/fixtures/parity/
  simple_dict/
    simple_dict.opf                     # source
    content.html cover.jpg toc.ncx      # source
    kindlegen_reference.mobi            # committed kindlegen output
  simple_book/
    simple_book.opf ... chapter*.html ... cover.jpg
    kindlegen_reference.mobi
  simple_comic/
    simple_comic.cbz                    # kindling source
    simple_comic.epub                   # kindlegen source (wrapper around the same 3 JPEGs)
    page1.jpg page2.jpg page3.jpg
    kindlegen_reference.mobi
```

The test binary reads the committed `.mobi` directly, so any environment (fresh clone, CI, sandbox) can run the parity tests without installing kindlegen.

**Regenerating the references.** If you edit a fixture source file, the kindlegen reference will stall and the parity test will start complaining about spurious diffs. To rebuild all three references from the current source, run:

```bash
./scripts/regenerate_parity_fixtures.sh
```

The script locates kindlegen in this order:

1. `$KINDLEGEN_PATH` environment variable
2. `kindlegen` on `$PATH`
3. `$HOME/.local/bin/kindlegen`

and aborts with a helpful error if none are found. kindlegen is no longer distributed by Amazon, but a Linux binary is still mirrored at <https://github.com/tdtds/kindlegen/raw/master/exe/kindlegen>. Drop it into `~/.local/bin/kindlegen` and the regeneration script will find it.

The huffdic fixtures under `tests/fixtures/huffdic/` cannot come from kindlegen at all: the macOS kindlegen inside Kindle Previewer 3 segfaults on `-c2` for every input large enough to actually reach its huffdic path, so there is no reference build to make. They are transcoded from the committed kindlegen PalmDOC dictionaries instead, with `cargo run --example gen_huffdic_fixture`, and validated by `./scripts/validate_huffdic_fixtures.sh`, which checks them against KindleUnpack's and calibre's decoders and, more to the point, decodes a real kindlegen 2.9 huffdic file from libmobi's corpus and requires it to match the uncompressed twin that ships beside it.

**Legal note.** The committed `kindlegen_reference.mobi` files are Kindle-format builds of the repo's own fixture content, produced by running kindlegen on OPF/EPUB sources that live next to them. Amazon's copyright does not extend to the OUTPUT kindlegen produces from your own content, so these files are safe to commit. What you cannot commit is the kindlegen BINARY itself; it remains Amazon-proprietary and is only required to run `scripts/regenerate_parity_fixtures.sh` when a source fixture changes.

Parity fixture contents:
- `simple_dict/` - 5-headword Latin-script dictionary with inflections
- `simple_book/` - 3-chapter plain book with CSS and a JPEG cover
- `simple_comic/` - 3-page CBZ fed to kindling; matching fixed-layout EPUB wrapper fed to kindlegen during regeneration

## Known Kindle firmware issues

These are Amazon firmware bugs, not kindling bugs, but they affect sideloaded MOBI/AZW3 files and users should be aware of them.

### Sideloaded `.azw3` files show no library cover

When you copy a book to a Kindle over USB, the home-screen tile is drawn from a per-book thumbnail the firmware caches during indexing, not live from the file's cover. On current firmware (verified on Paperwhite 3 / 5.16 and Paperwhite 5 / 5.18, issue #20) that cache is populated from the embedded cover only for files with a `.mobi` extension; a KF8-only `.azw3` shows a gray placeholder even though its EXTH cover records (129/201/202) are correct and the cover renders fine inside the book. Byte-identical content shows the cover as `.mobi` and not as `.azw3`, so the firmware keys on the extension, not on anything in the records.

- Build with `--legacy-mobi`. It writes a true dual MOBI7+KF8 under a `.mobi` name, which is the only container Amazon's own toolchain ever puts that extension on, and it both shows the cover and opens. The cost is a second compressed copy of the text: about +13% on a comic, where images dominate and live once in the MOBI7 half, and roughly +90% on a text-only book.
- Do not name a KF8-only payload `.mobi`. That shape is one kindlegen never produces, and the firmware dispatches on the extension: it indexes the file, shows the cover, then refuses to open it with "The selected item could not be opened". Confirmed on a Paperwhite 3 (5.16.2.1.1) and a Paperwhite 5 (5.19.2) against byte-identical builds where the only variable was the name (issue #24). `--mobi-ext` used to do exactly this and is now a deprecated alias for `--legacy-mobi`; an explicit `-o book.mobi` on a default build still produces it, and kindling warns when you do.
- Stamping an ASIN plus `EBOK` does not fix this tile: with no matching Amazon purchase the cloud cover lookup fails and the shelf shows Amazon's "No image available" placeholder instead of gray. It does buy something else, though, so kindling offers it as `--doc-type ebok`: EXTH 113 plus 501 is what gates the lock screen cover (issue #26, verified on a Paperwhite 5 / 5.19.2). Run `kindling thumbnail` afterwards to overwrite the cached placeholder with the book's own art and get both. The older claim here, that any EXTH 501 value breaks reflowable-book home navigation (issue #15), was retested and is narrower than it looked.

### Blank pages on Kindle Scribe and Colorsoft

Kindle Scribe (all generations) and Colorsoft randomly render some pages as blank. This is the most-reported issue across comic converters and is caused by a firmware rendering bug. PNG images are far more affected than JPEG, and higher JPEG quality settings (larger per-page file size) may worsen the issue.

- Kindling uses JPEG by default, which helps
- Use `--jpeg-quality 70` to reduce per-page file size if you encounter blank pages
- This affects all converters, not just kindling

### Calibre Panel View corruption

Transferring MOBI files via Calibre can strip or corrupt Panel View metadata. Symptoms: the Panel View menu option disappears and tap-to-zoom stops working.

- Transfer files directly via USB instead of through Calibre
- Or use Calibre's "Send to device" without format conversion

### Firmware 5.19.2 sideloading regressions

Kindle firmware 5.19.2 introduced regressions for sideloaded fixed-layout content: Panel View disappeared, large margins appeared, and page turns became laggy. These issues were partially fixed in 5.19.3.0.1.

- Deregistering the Kindle temporarily resolves the issue
- Updating to firmware 5.19.3+ is recommended

## Acknowledgements

Thanks to the ebook-tooling community whose public documentation and reverse-engineering work made this project possible:

- [w3c/epubcheck](https://github.com/w3c/epubcheck) is the W3C's official EPUB validator. Most of kindling's Section 5-11 and 15-16 rules are direct ports of its checks, adapted to Kindle's constraints. Epubcheck's rule IDs (`OPF_*`, `RSC_*`, `HTM_*`, `NAV_*`, `NCX_*`, `CSS_*`, `MED_*`, `PKG_*`) are preserved in every ported rule's description so they stay traceable back to the source.
- [w3c/epub-tests](https://github.com/w3c/epub-tests) is the W3C EPUB 3 reading-system conformance corpus. Kindling's optional corpus harness (`tests/epub_tests_corpus.rs`) runs the entire corpus through the validator to surface false positives and measure coverage against real-world EPUB content.
- [KCC (Kindle Comic Converter)](https://github.com/ciromattia/kcc) by Ciro Mattia Gonano, with earlier work by [AcidWeb](https://github.com/AcidWeb), for pioneering comic-to-Kindle processing. Panel detection, webtoon handling, and device profile data informed kindling's comic pipeline.
- The [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI) and Developer's Corner forum for the foundational public documentation of the MOBI format. Dc5e's [KindleComicParser](https://www.mobileread.com/forums/showthread.php?t=192783) thread on fixed-layout binaries filled in gaps the wiki does not cover.
- Amazon's *kindlegen* (no longer maintained) is used as a reverse-engineering reference: its output files are diffed against kindling's to figure out the parts of the MOBI format Amazon never documented.
- The broader open-source MOBI tooling community for format notes, sample files, and online discussions.

## Related projects

- [Lemma](https://github.com/ciscoriordan/lemma) - Greek-English Kindle dictionary built with Kindling
- [Greenfield New Testament Greek-English Lexicon](https://github.com/open-greek/greenfield-nt-lexicon) - Public-domain Greek-English dictionary published as EPUB, Kindle MOBI, and StarDict with Kindling
- [LSJ Greek-English Lexicon](https://github.com/open-greek/lsj-lexicon) - Complete Perseus LSJ edition published as EPUB, Kindle MOBI, StarDict, and reusable JSONL

## Reporting bugs

The [issue tracker](https://github.com/ciscoriordan/kindling/issues) is the list of what is currently wrong with kindling and what is planned. [`.github/issue-policy.md`](.github/issue-policy.md) says what gets an issue, what the labels mean, and when one gets closed. The short version: a file that reproduces the problem is worth more than a description of it, and an issue closes when the behavior is confirmed fixed on a real device, not when the code lands.

Include the kindling version, the device and firmware if it is a device behavior, and whether the same thing happens with kindlegen or calibre output.

## AI policy

AI-assisted contributions are welcome: issues, investigations, and pull requests drafted with AI tools are all fine, and several of this project's own features were built that way. The one hard rule is that AI-generated code must be reviewed by a human before it is submitted. Read the diff, understand what it does, and be ready to answer questions about it in review. "The model wrote it" is not a substitute for a contributor who understands their own patch.

## Stargazers over time

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://raw.githubusercontent.com/ciscoriordan/kindling/star-history/star-history-dark.svg">
  <img alt="Star history chart for ciscoriordan/kindling" src="https://raw.githubusercontent.com/ciscoriordan/kindling/star-history/star-history-light.svg" width="100%">
</picture>

The chart is regenerated daily by [a workflow](.github/workflows/star-history.yml) that queries the GitHub API and commits the rendered SVGs to the `star-history` branch. (Third-party chart services like star-history.com stopped working for READMEs when GitHub [restricted the stargazers API](https://github.blog/changelog/2026-06-30-upcoming-access-restrictions-to-public-api-endpoints-and-ui-views/) to repository collaborators in June 2026.)

## License

MIT - © 2026 Francisco Riordan
