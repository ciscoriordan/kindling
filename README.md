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

- **Dictionaries**: Full orth index with headword + inflection lookup, ORDT/SPL sort tables, generated CJK and Arabic collation tables, fontsignature
- **Books**: EPUB or OPF input, embedded images, embedded fonts (with IDPF/Adobe deobfuscation), hierarchical on-device TOC from the EPUB nav document (toc.ncx / nav.xhtml, including `file#anchor` entries and nested volume/chapter levels), user font switching kept working by stripping font-family from stylesheets, `<style>` blocks, and inline `style="..."` attributes when no fonts are embedded (`--force-user-fonts` to strip always), KF8-only (.azw3) by default with legacy dual-format (MOBI7+KF8) available via `--legacy-mobi`, HD image container, fixed-layout support
- **Comics**: Image folder, CBZ, CBR, or EPUB input, device-specific resizing, spread splitting, margin cropping, auto-contrast, moire correction for color e-ink, manga RTL, webtoon with overlap fallback, Panel View, KF8-only (.azw3) by default, metadata overrides
- **StarDict export**: `kindling stardict` builds a four-file StarDict bundle (`.ifo` / `.idx` / `.dict` / `.syn`) from the same OPF or EPUB dictionary input as `kindling build`, for use with GoldenDict, GoldenDict-ng, KOReader, sdcv, and other non-Kindle dictionary readers (see [StarDict export](#stardict-export))
- **EPUB export**: `kindling epub2` and `kindling epub3` build a reflowable EPUB from the same OPF or EPUB input, conformant to EPUB 2.0.1 and EPUB 3.3 respectively (epubcheck-clean). EPUB2 is always a plain book; EPUB3 is a plain book by default and emits an EPUB Dictionaries and Glossaries layer (Search Key Map, `dc:type=dictionary`, `epub:type` semantics) when the input is a dictionary (see [EPUB export](#epub-export))
- **EPUB repair**: `kindling repair` applies a small, byte-stable, idempotent set of structural fixes to an EPUB for cleaner Send-to-Kindle ingest (see [Repair](#repair))
- **Metadata rewrite**: `kindling rewrite-metadata` updates title, authors, publisher, description, language, ISBN, ASIN, publication date, tags, series, and cover image on an existing MOBI/AZW3 in place without rebuilding from source. Byte-stable on no-op, idempotent, refuses DRM files (see [Rewrite metadata](#rewrite-metadata))
- **Structural dump**: `kindling dump` prints the parsed structure of a MOBI/AZW3 (PalmDB, MOBI header, EXTH, INDX/ORDT tables, entry labels) as line-oriented `section.field = value` output, so two dumps can be compared with `diff` (see [Dump](#dump))
- **Lookup simulator**: `kindling lookup <dict.mobi> <word>` reproduces the on-device dictionary search against a built MOBI (accent/case folding for Latin and Greek, literal matching for CJK/Arabic, query-side case folding for Cyrillic) and reports which stored form resolves. It is a build-side regression check, not a hardware oracle (see [Lookup simulator](#lookup-simulator))
- **Build-time HTML self-check**: every `build` runs a two-pass HTML balance check on the assembled MOBI text blob and on each individual PalmDB text record after splitting, catching regressions like dangling tags, `<hr/` corruption, and bold/italic state leaking across record boundaries (see [Build-time self-check](#build-time-self-check))
- **UTF-8 and tag-safe record splitter**: text records end on HTML `<hr/>` entry boundaries where possible, otherwise back off past any unclosed `<` tag and any incomplete UTF-8 multi-byte character, so multi-byte characters are never truncated and chunks never end mid-tag
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
kindling-mobi = "0.26"
```

Then `use kindling::...`. Public API is defined in `src/lib.rs`.

## Usage

### Dictionaries

```bash
kindling-cli build input.opf -o output.mobi
kindling-cli build input.opf -o output.mobi --no-compress    # skip compression for fast dev builds
kindling-cli build input.opf -o output.mobi --headwords-only  # index headwords only (no inflections)
kindling-cli build input.opf -o output.mobi --no-kindle-limits  # skip per-letter HTML grouping
kindling-cli build input.opf -o output.mobi --no-validate     # skip KDP pre-flight validation
kindling-cli build input.opf -o output.mobi --fold-accents    # kindlegen-style accent folding (Latin default is exact, see below)
kindling-cli build input.opf -o output.mobi --strict-accents  # force exact accent match for any script
KINDLING_FOLD_ACCENTS=1 kindling-cli build input.opf -o output.mobi  # --fold-accents for wrappers that can't pass the flag (pyglossary/reader.dict)
kindling-cli lookup output.mobi rivière   # simulate the on-device lookup of a word
```

The input OPF must reference HTML files with `<idx:entry>`, `<idx:orth>`, and `<idx:iform>` markup following the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf). Both headwords and inflected forms are indexed so that looking up any form on the Kindle finds the correct dictionary entry.

Latin-script dictionaries default to exact accent matching. Every character is its own symbol, so the index keeps `ê` distinct from `e`, while accent and case variants share a collation weight so accented headwords sort adjacent to their base. The Kindle firmware collates Latin dictionaries with its own accent-and-case-folding lookup, so the labels are pre-sorted in that folded order and the distinct symbols then let it return the exact form. This is confirmed end to end on real firmware: an unaccented or uppercase query resolves the accented, lowercase headword (`riviere` and `RIVIÈRE` both resolve `rivière`, `meme` and `MÊME` both resolve `même`, `cafe` resolves `café`), each to its own distinct entry, while a non-headword or a bare prefix correctly finds nothing. An unaccented query with no exact headword still falls back to its accented neighbour (issue #8). The folded sort also fixes accent-initial headwords like Polish `świat`, which a raw byte-order sort placed after `z`, where the firmware's collation never looks.

The fold lowercases and strips the diacritics of Latin-1 and all of Latin Extended-A, so `é`→e, `à`→a, `ñ`→n and `ā`→a, `ł`→l, `ś`→s all collate to their base letter. The firmware's lookup collation also decomposes eth (`ð`→d), slashed-o (`ø`→o), thorn (`þ`→t), the micro sign (`µ`→m), and the florin (`ƒ`→f), so kindling folds those five to their base too, confirmed by the same on-device lookups. The Latin-1 folds are not hand-guessed: the SPL1 spelling table inside the committed kindlegen collation blob is a codepoint-indexed map of each character to its base letter, and a test re-extracts it and asserts kindling's fold reproduces it, so the two cannot drift.

Pass `--fold-accents` to instead embed the kindlegen-derived ORDT/SPL folding blob, so `meme` also matches `même` accent-insensitively, matching kindlegen; the labels are still folded-sorted so accent-initial headwords resolve. The same fold orders the labels in both modes, since the firmware collates a Latin dictionary the same folded way whether or not the blob is embedded. `--strict-accents` forces the exact collation for any script, including Greek and Cyrillic. Both flags have environment-variable forms, `KINDLING_FOLD_ACCENTS=1` and `KINDLING_STRICT_ACCENTS=1`, which are the only way to reach them when kindling runs under a wrapper that controls the command line (for example pyglossary's Mobi writer, used by reader.dict). Greek and Cyrillic dictionaries keep the folding default, since their scripts already sort the way the firmware collates them; none of this affects book builds.

Cyrillic dictionaries additionally get generated lookup aliases (issue #17): every indexed form with a combining stress mark (acute U+0301 or grave U+0300, as in `пробормота́в`) also gets its bare spelling as an extra index entry, and every form with uppercase letters gets a lowercased alias, so an all-caps abbreviation like `ФСБ` is found without the manual lowercase-variant workaround dictionaries needed under kindlegen. Aliases point at the same entry, dedupe against forms the source already ships, and are skipped under `--strict-accents`.

Japanese, Chinese, Korean, and the Arabic-script languages (`<DictionaryInLanguage>` of `ja`, `zh`, `ko`, `ar`, `fa`, `ur`, `ps`, `ug`, `sd`, or `ckb`) instead get per-dictionary generated ORDT collation tables, using a per-character encoding. Persian, Urdu, Pashto, Uyghur, Sindhi, and Central Kurdish reuse the same all-literal table as Arabic, each with its own neutral-primary MOBI locale. Each headword character becomes one label element: kana are collation symbols in an embedded ORDT table (so hiragana and katakana fold together, and ヴ collates as ウ), and every other character (kanji, Hangul, Arabic letters) is stored as a literal Unicode code point. The Japanese prolonged sound mark ー is special: it folds onto the preceding vowel (so ローゼマイン collates with a long `o`), exactly as kindlegen and the firmware's query normalization do; a raw ー never resolves on device. The firmware encodes a tapped word the same way, so the query matches the stored label. The kana tables are embedded only for dictionaries that actually contain kana; the all-literal scripts get the minimal table kindlegen emits, and the MOBI-header locale is the neutral primary LCID the firmware's normalization expects. This whole path is verified on a physical Kindle. The earlier 0.16.0/0.17.0 releases used a one-symbol-per-byte encoding reverse-engineered from kindlegen's output on toy dictionaries; that form only appears for tiny inputs and never matched on device, which is why issue #11 got worse before it got better. `--strict-accents` has no effect on generated-ORDT builds.

#### Supported dictionary languages

Languages exercised by the test suite, with their index layout and how far each has been verified:

| Language | Code | Flag | Index collation | Verified |
|---|---|---|---|---|
| Greek | `el` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/el.svg" width="20" alt="Greek flag"/> | UTF-16BE + accent-folding blob | On device (production dictionaries) and structural tests |
| English | `en` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/en.svg" width="20" alt="English flag"/> | Exact per-character ORDT | On device (community use) and structural tests |
| French | `fr` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/fr.svg" width="20" alt="French flag"/> | Exact per-character ORDT | On device and structural tests |
| Russian | `ru` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ru.svg" width="20" alt="Russian flag"/> | UTF-16BE (folding blob suppressed) + stress/case aliases | On device and structural tests |
| Turkish | `tr` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/tr.svg" width="20" alt="Turkish flag"/> | Exact per-character ORDT | On device and structural tests |
| Japanese | `ja` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ja.svg" width="20" alt="Japanese flag"/> | Generated ORDT (per-character) | On device; byte parity with kindlegen lookup keys |
| Chinese | `zh` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/zh.svg" width="20" alt="Chinese flag"/> | Generated ORDT (per-character) | On device; byte parity with kindlegen lookup keys |
| Korean | `ko` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ko.svg" width="20" alt="Korean flag"/> | Generated ORDT (per-character) | On device; byte parity with kindlegen lookup keys |
| Arabic | `ar` | <img src="https://raw.githubusercontent.com/ciscoriordan/svg-flags/main/circle/languages/ar.svg" width="20" alt="Arabic flag"/> | Generated ORDT (per-character) | On device; byte parity with kindlegen lookup keys |

Persian (`fa`), Urdu (`ur`), Pashto (`ps`), Uyghur (`ug`), Sindhi (`sd`), and Central Kurdish (`ckb`) route through the same generated all-literal ORDT as Arabic, each with its own neutral-primary MOBI locale. They share the device-verified `ar` collation path, so they are not in the table above as separate fixtures; on-device verification of the per-language locales is in progress.

Each language in the table has a committed fixture under `tests/fixtures/langs/<code>/` with a dictionary source, a kindling build, a kindlegen build, and a sideloadable test book (regenerate with `tests/fixtures/langs/generate.py`). `tests/dict_languages.rs` builds each dictionary with kindling and checks the language ids and locale, that every entry has a real text pointer, that the headwords round-trip through the on-disk labels, and the per-language collation; for the generated-ORDT scripts it also asserts byte parity of the ORDT table and headword labels against the committed kindlegen build (identical for the all-literal scripts, value-equivalent for Japanese). Languages not listed still build with the UTF-16BE layout and correct ids. The MOBI locale field maps Ancient Greek (`grc`) and a broad set of Latin- and Cyrillic-script languages to their own neutral Windows LCID rather than defaulting to English, so the firmware applies script-appropriate normalization; codes with no mapping still fall back to English. Unlike kindlegen, which aborts on a language it does not know, kindling builds a dictionary for any `dc:language`. If you ship a dictionary in one and lookups misbehave on device, please open an issue.

If the OPF references a cover image (Method 1 `<item properties="coverimage"/>` or Method 2 `<meta name="cover">`), Kindling embeds it in the dictionary MOBI via EXTH 201 so it shows up on the Kindle home screen next to regular books and comics.

Images referenced from entry HTML via `<img src="..."/>` but not declared in the OPF manifest are also embedded automatically. PyGlossary and other OEB 1.x-era tools commonly emit manifests that omit inline glyph GIFs referenced from within `<idx:entry>` blocks; kindlegen silently picks these up, and kindling matches that behavior so the glyphs render on device.

By default, dictionaries enforce Kindle publishing limits (`--kindle-limits`): entries are grouped by first letter to keep individual HTML sections under 30 MB, and a warning is printed if the total exceeds 300 sections. Use `--no-kindle-limits` to disable this and produce a single flat HTML blob.

Every `build` also runs the Kindle Publishing Guidelines validator as an automatic pre-flight step before writing the MOBI. If validation reports any errors, the build is aborted with exit code 1; warnings are printed but do not block the build. Pass `--no-validate` to skip the check. See [Validation](#validation) for the full list of rules.

### Books

```bash
kindling-cli build input.epub                          # output next to input as input.azw3 (KF8-only)
kindling-cli build input.epub -o output.azw3           # explicit output path
kindling-cli build input.epub --legacy-mobi            # opt into legacy dual MOBI7+KF8 (.mobi)
kindling-cli build input.epub --no-hd-images           # skip HD image container
kindling-cli build input.epub --no-embed-source        # smaller file, but breaks Kindle Previewer
kindling-cli build input.epub --kindle-limits          # warn about HTML files exceeding 30 MB
kindling-cli build input.epub --no-validate            # skip KDP pre-flight validation
kindling-cli build input.epub --force-user-fonts       # skip font embedding; Aa menu font always applies
```

Auto-detects dictionary vs book from the OPF's `DictionaryInLanguage` metadata. Book MOBIs include embedded images and an HD image container (for high-DPI Kindle screens). The original EPUB is embedded by default for Kindle Previewer compatibility (`--no-embed-source` to skip).

The on-device "Go To" table of contents is built from the EPUB navigation document, preferring the EPUB3 nav (`properties="nav"`) and falling back to the EPUB2 `toc.ncx`, so chapter names survive even when every chapter file carries the same generic `<title>` (issue #18). Entries that point at `file#anchor` targets inside a shared spine file each get their own TOC node at the anchor position, matching kindlegen's NCX. Files without a nav document fall back to per-file `<title>` labels.

Nested TOC levels are preserved (issue #19): nested `<ol>` lists in the EPUB3 nav (or nested `<navPoint>` elements in the NCX) become a hierarchical KF8 NCX with kindlegen's exact tag layout - breadth-first entry numbering, parent/first-child/last-child links, and subtree lengths, so a 卷/章 (volume/chapter) structure collapses and expands on device just like a Send-to-Kindle or Calibre conversion. The per-record TBS (trailing byte sequences) switch to the hierarchical strand encoding for these books, verified byte-for-byte against kindlegen on 2- and 3-level test books.

Kindle's renderer keeps any `font-family` the book CSS names over the reader's Aa menu choice, so a book whose stylesheets name fonts that are not even embedded (common in Chinese EPUBs) shows the font menu but never changes face. When a book embeds no usable fonts, kindling strips `font-family` declarations and dead `@font-face` rules from the stylesheets, inline `<style>` blocks, and per-element `style="..."` attributes so the Aa menu stays in control; declarations whose family stack includes the generic `monospace` are kept as plain `font-family: monospace` so code blocks stay fixed-pitch. Books that do embed fonts keep their CSS untouched by default (the publisher design wins); pass `--force-user-fonts` to skip font embedding and strip `font-family` anyway, mirroring KOReader's reader-first behavior.

Fonts declared in the manifest (TTF/OTF) are embedded as KF8 FONT resource records, with `@font-face` `src: url(...)` in the stylesheets rewritten to the matching `kindle:embed` resource, so publisher fonts survive conversion. EPUB font obfuscation (both the IDPF and Adobe schemes declared in `META-INF/encryption.xml`) is undone at build time; the embedded output is zlib-deflated and unobfuscated, matching kindlegen's own FONT records. WOFF/WOFF2 fonts are skipped with a warning since Kindle cannot render them. Note that the Kindle language also matters for on-device font choice: a book without a `<dc:language>` (or with an unrecognized tag) is treated as English, which hides the CJK font menu on Chinese/Japanese books, so kindling warns when that happens.

Non-dictionary builds default to KF8-only `.azw3`, because Amazon deprecated MOBI for Send-to-Kindle in August 2022 and modern Kindles prefer KF8-only. Dictionaries continue to build as dual-format MOBI7+KF8 `.mobi`, because Kindle's lookup popup requires the MOBI7 INDX structure and KF8 has no equivalent. Pass `--legacy-mobi` on a book build to opt back into the old dual-format `.mobi` output for pre-2012 Kindles; the flag is a no-op on dictionary builds. If you pass `-o foo.mobi` or `-o foo.azw3` explicitly, kindling respects whatever extension you chose.

Every `build` runs the Kindle Publishing Guidelines validator automatically before writing the MOBI. Findings are printed with severity, rule id, and file:line; the build is aborted on any error (warnings are advisory). Pass `--no-validate` to skip pre-flight entirely.

### Comics

```bash
kindling-cli comic input.cbz --device paperwhite                # output next to input as input.azw3
kindling-cli comic input.cbr -o output.azw3                     # CBR (RAR) input, explicit output
kindling-cli comic manga.epub --rtl                             # EPUB comic/manga
kindling-cli comic manga.cbz --rtl                              # manga (right-to-left)
kindling-cli comic webtoon/ --webtoon                           # webtoon (vertical strip)
kindling-cli comic input/ --no-split --crop 0                   # disable smart processing
kindling-cli comic input.cbz --title "My Comic" --language ja   # metadata overrides
kindling-cli comic input.cbz --doc-type ebok                    # appear under Books on Kindle
kindling-cli comic input.cbz --cover 3                          # use page 3 as cover
kindling-cli comic input.cbz --legacy-mobi                      # opt into legacy dual MOBI7+KF8 (.mobi)
kindling-cli comic input.cbz --embed-source                     # embed EPUB source (off by default, see note below)
```

Comics default to KF8-only `.azw3` for the same reason books do: Amazon deprecated MOBI for Send-to-Kindle in August 2022, and the legacy MOBI7 section in dual-format files is at best wasted bytes on modern Kindles. `--legacy-mobi` is the escape hatch for pre-2012 devices. If you pass `-o foo.mobi` explicitly, kindling respects your extension choice.

Comic builds do not embed the intermediate EPUB as a SRCS record by default (this changed in v0.7.7). Embedding duplicates every page image as a zipped EPUB inside the MOBI, which for a large comic produces a single PalmDB record over 100 MB. Kindle devices index the resulting file but then fail to open it with "Unable to Open Item". Pass `--embed-source` only when you need to round-trip through Kindle Previewer.

Converts image folders, CBZ files, CBR files, and EPUB files to Kindle-optimized MOBI with:
- **Device profiles**: *paperwhite*, *kpw5*, *oasis*, *scribe*, *scribe2025*, *kindle2024*, *basic*, *colorsoft*, *fire-hd-10*
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
| Cover | EXTH 201 (cover image offset in image pool) + EXTH 129 (KF8 cover URI) | Cover offset is 0-based index within image records starting at `first_image`. |
| Document type | EXTH 501 | Omitted entirely for reflowable books. Its mere presence (any value) makes the Kindle reader treat the book as a non-navigable document and hide the back-to-library toolbar, trapping the reader in the book (device-verified, issue #15). kindlegen writes none for books. Comics are fixed-layout (a different reader, unaffected) and set it via `--doc-type`: `PDOC` = Documents shelf, `EBOK` = Books shelf. |
- **Document type** (comics): `--doc-type ebok` to appear under Books instead of Documents on Kindle (default: `pdoc`). Reflowable books write no content-type record, because its presence breaks the reader's home navigation.
- **KF8-only by default**: comics output `.azw3` with only the KF8 section (no MOBI7); pass `--legacy-mobi` for the old dual-format behavior on pre-2012 Kindles

### Validation

```bash
kindling-cli validate input.opf             # print findings, exit 1 on errors
kindling-cli validate input.opf --strict    # exit 1 on any warning too
```

Validation also runs automatically as a pre-flight step inside every `kindling build` invocation (including kindlegen-compat mode `kindling input.opf`). Any validation errors abort the build with exit code 1; warnings are printed but do not block the build. Pass `--no-validate` to `build` to skip the pre-flight entirely. Comic builds (`kindling comic`) do not run the validator because comics have different structural requirements that the book-oriented rules do not cover.

Runs 117 pre-flight checks against the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf) (version 2026.1). Most rules are ports of the corresponding w3c/epubcheck checks; the rest are KDP-specific rules kindling adds on top. Rules are grouped by KPG section:

- **4 Cover image** (5 rules): internal cover must exist via Method 1 (`<item properties="coverimage"/>`) or Method 2 (`<meta name="cover">`), file must exist on disk, shortest side >= 500 px, no duplicate HTML cover page in the spine (`R4.1.1`-`R4.2.4`)
- **5 Navigation** (13 rules): NCX declared in manifest and referenced from `<spine toc>`, NCX and guide targets must resolve to manifest items, TOC recommended for books > 20 pages, NCX `dtb:uid` must match the OPF unique-identifier, page-list required when `epub:type="pagebreak"` is used, nav entries in spine order, no remote links in nav or NCX (`R5.1`-`R5.11`, epubcheck `NAV_003/010/011`, `NCX_001/004/006`, `OPF_032/050`)
- **6 HTML, CSS, and encoding** (19 rules): well-formed XHTML, no `<script>`, no nested `<p>`, filename case must match, XML 1.0 only, no external entities, `epub:` namespace URI must be correct (Vader Down bug), UTF-8 required for HTML and CSS, no forbidden `position` values, `@import`/`url()`/`@font-face` targets must resolve through the manifest, `@namespace` and unsupported `@media` features flagged (`R6.1`-`R6.17`, `R6.e1`, `R6.e2`, epubcheck `CSS_005`-`CSS_027`)
- **7 Manifest and spine integrity** (13 rules): declared media-types must match file bytes, every spine `itemref` must have a linear target, no duplicate `idref` or `href`, fallback chains must terminate at a renderable resource, deprecated media-types flagged, manifest cannot point at the OPF itself (`R7.1`-`R7.13`, epubcheck `OPF_003/013/029/033/034/035/037/040/041/042/043/074/099`)
- **8 OPF prefix and property grammar** (10 rules): `<package prefix>` syntax, reserved-prefix rebinding, manifest `properties` attributes must match the content's feature use, unknown or undeclared prefixes flagged (`R8.1`-`R8.10`, epubcheck `OPF_004/005/006/007/012/014/015/026/027/028`, EPUB 3 only)
- **9 Cross-references and dead links** (12 rules): fragment ids must exist in their target, fragments are rejected on non-SVG raster images and on manifest hrefs, `data:` and `file:` URLs refused, `..` path traversal blocked, manifest hrefs must name a resource (`R9.1`-`R9.12`, epubcheck `RSC_009/011/012/014/015/020/026/029/030/033`, `OPF_091/098`)
- **10 Text-heavy reflowable** (8 rules): supported image formats (JPEG, PNG, GIF, SVG), per-image size <= 127 KB, dimensions <= 5 megapixels, headers require valid JPEG/PNG/GIF end markers, image extension must match magic bytes, tables capped at 50 rows, heading alignment defaults only (`R10.3.1`, `R10.4.1`-`R10.4.5`, `R10.5.1`, epubcheck `MED_004`, `PKG_021/022`)
- **11 Fixed-layout** (9 rules, comic and textbook profiles only): OPF must declare `rendition:layout=pre-paginated`, XHTML must carry `<meta name="viewport">` with width and height, `rendition:spread`/`orientation`/`layout` values constrained, pages should contain image content, HD builds should carry `original-resolution` (`R11.1`-`R11.9`, epubcheck `OPF_011`, `HTM_046`-`HTM_053`)
- **13 OCF filenames** (5 rules): no OCF-forbidden characters (`< > : " | ? *` and controls), spaces and non-ASCII flagged, trailing dot rejected (Windows drops it), case-insensitive duplicate hrefs rejected (`R13.1`-`R13.5`)
- **15 Dictionaries** (14 rules, dict profile only): Amazon-legacy KDP format requires `DictionaryInLanguage`, `DictionaryOutLanguage`, `DefaultLookupIndex` matching an `idx:entry name`, at least one `idx:entry`, and non-empty `idx:orth value` (`R15.1`-`R15.7`). EPUB 3 dict rules gated on `package_version="3.0"` cover `epub:type="dictionary"`, `dc:type=dictionary`, Search Key Map Documents, and dictionary collections (`R15.e1`-`R15.e7`, epubcheck `OPF_078`-`OPF_084`)
- **16 OPF metadata and package identity** (8 rules): `<package unique-identifier>` must point at a real `<dc:identifier>`, `<dc:date>` must be W3CDTF syntax and a real calendar date, no empty Dublin Core elements, `opf:scheme="UUID"` must be RFC 4122, `<dc:language>` must be BCP47 (`R16.1`-`R16.8`, epubcheck `OPF_030/048/053/054/055/072/085/092`)
- **17/18.1 Unsupported tags** (1 rule): `<form>`, `<input>`, `<frame>`, `<iframe>`, `<canvas>`, `<object>`, etc. (`R17.1`)

Output: one line per finding with severity (`info`/`warning`/`error`), rule id (e.g. `R4.2.1`), KPG section, PDF page reference, message, and file:line where applicable, followed by a summary (`X errors, Y warnings, Z info`). Exit code is 0 on success, 1 if any errors are present (or any warnings in `--strict` mode).

The rule catalog is a single Rust const array in [`src/kdp_rules.rs`](src/kdp_rules.rs) with a `KPG_VERSION` constant and a `Rule` struct holding id, section, level, title, PDF page, description, and a profile mask (default, comic, dict, textbook). Each rule cluster lives in its own module under [`src/checks/<name>.rs`](src/checks/) and implements the `Check` trait; all active checks are registered in the `CHECKS` array in [`src/checks/mod.rs`](src/checks/mod.rs). Phase 2 added `fixed_layout`, `manifest_spine`, `opf_grammar`, `toc_extras`, `cross_refs`, `filenames`, `image_integrity`, `css_forbidden`, and `metadata` alongside the pilot clusters `parse_encoding` and `dict`. The pre-Phase-2 checks (`cover`, `navigation`, `nav_links`, `content`, `images`, `file_case`) are still `Check` impls. Updating the guidelines version touches `kdp_rules.rs` plus whatever `src/checks/*.rs` modules the affected rules live in.

### StarDict export

```bash
kindling-cli stardict input.opf                   # writes input-stardict/ next to the input
kindling-cli stardict input.epub -o my_dict       # explicit output directory
kindling-cli stardict input.opf -o my_dict --bookname "My Greek Dictionary" --author "Jane Doe" --date 2026-05-07
kindling-cli stardict input.opf --website "https://example.com/dict" --email "you@example.com" --description "License: CC-BY-SA 4.0."
```

`kindling stardict` reads the same OPF or EPUB dictionary input as `kindling build` and emits a four-file StarDict 2.4.2 bundle ready to drop into GoldenDict, GoldenDict-ng, KOReader, sdcv, or any other reader that consumes the format. The output directory contains:

- `<name>.ifo`: UTF-8 manifest with `bookname`, `wordcount`, `idxfilesize`, optional `synwordcount`, `author`, `email`, `website`, `description`, `date`, and `sametypesequence=h`. `bookname` / `author` / `date` default to the OPF's `dc:title` / `dc:creator` / `dc:date`; CLI flags override. `email`, `website`, and `description` have no OPF counterpart and are emitted only when supplied. StarDict 2.4.2 has no `license` field, so license info is conventionally folded into `description` (use `<br>` for line breaks). When the OPF declares `<DictionaryInLanguage>` and `<DictionaryOutLanguage>` and the bookname does not already contain a 2-3 letter hyphenated pair, kindling appends ` (in-out)` to the bookname so GoldenDict-ng / KOReader can parse the language pair and populate the "Translates from / to" fields (the StarDict spec has no formal source/target language slot, so embedding the codes in the bookname is the de-facto convention).
- `<name>.idx`: concatenation of `(headword\0, offset:u32be, size:u32be)`, sorted by `g_ascii_strcasecmp` (ASCII case-insensitive bytewise) so readers can binary-search.
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

`kindling epub2` and `kindling epub3` read the same OPF or EPUB input as `kindling build` and emit a reflowable EPUB. Both write a spec-conformant archive: the `mimetype` entry first and stored uncompressed, then `META-INF/container.xml`, then the `OEBPS/*` payload deflated. The output validates clean under epubcheck (EPUB 2.0.1 rules for `epub2`, EPUB 3.3 rules for `epub3`).

- `epub2` is always a plain, reflowable [EPUB 2.0.1](http://idpf.org/epub/20/spec/OPF_2.0.1_draft.htm) book (`<package version="2.0">` plus an NCX; the `version` attribute is `2.0`, `2.0.1` is the spec revision). It is never dictionary-aware: if the input carries Kindle dictionary markup (`<idx:*>` tags, `<DictionaryInLanguage>` metadata), the dictionary semantics are ignored and a plain readable book is produced. Dictionary output is an EPUB3-only feature by design.
- `epub3` is a generic [EPUB 3.3](https://www.w3.org/TR/epub-33/) book (`<package version="3.0">` plus an EPUB3 nav document; the `version` attribute is `3.0` for all EPUB 3.x, `3.3` is the spec revision) by default. When the input looks like a dictionary, it additionally emits an [EPUB Dictionaries and Glossaries](https://www.w3.org/TR/epub-dictionaries/) layer (a profile on top of EPUB 3.3, not part of EPUB 3.3 core): a single Search Key Map (`skm.xml`), `<dc:type>dictionary</dc:type>`, `source-language` / `target-language` metadata (also declared as `<dc:language>`), and `epub:type="dictionary"` / `epub:type="dictentry"` semantics in the content. Each entry's body is re-parsed and re-serialized as well-formed XHTML so it passes epubcheck's strict DICT profile.

The dictionary layer is selected automatically: if the OPF declares `<DictionaryInLanguage>` / `<DictionaryOutLanguage>` or `<dc:type>dictionary`, `epub3` emits a dictionary with those languages as source/target. Pass `--book` to force a plain book regardless, or `--dictionary SOURCE TARGET` to force the dictionary layer with explicit language codes (overriding both auto-detection and the OPF's own language fields). There is intentionally no EPUB2 dictionary mode.

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
kindling-cli rewrite-metadata input.azw3 --title "New Title" --report-json
```

`kindling rewrite-metadata` updates the EXTH metadata records (and optionally the cover image record) of an existing MOBI/AZW3 file without re-running the EPUB/OPF build pipeline. Supported fields: title (EXTH 503 plus full_name), multi-value author (100), publisher (101), description (103), language (524), ISBN (104), ASIN (504), publication date (106), multi-value subject/tag (105), series name (112), series index (113), and the cover image bytes. Book content records (text, non-cover images, indices, INDX/FLIS/FCIS) are never touched. Multi-value flags like `--author` and `--subject` accept repeats to accumulate values.

The pass is byte-stable on no-op: if the requested updates match what is already in the file, or if no field flags are passed at all, the output is a `fs::copy` of the input with identical bytes. Downstream library managers that use content-hash identity for books can therefore call `rewrite-metadata` unconditionally when a user opens the metadata editor and closes it without changes. It is idempotent: running with the same updates a second time reports zero changes and produces a byte-identical output. DRM-protected files (PalmDOC encryption byte set, or EXTH 401/402/403 present) are rejected with exit code 1 and not touched; no DRM removal code is linked or referenced.

Unknown EXTH records in the input are preserved unchanged, so tool-specific metadata written by Calibre or kindlegen survives the rewrite pass.

### Build-time self-check

Every `build` and `comic` run now performs an HTML self-check on the assembled MOBI text blob before writing the output file. The check catches regressions like dangling `<body>` / `<mbp:frameset>` tags, `<hr/` corruption, and unclosed attribute quotes that would otherwise reach a user's Kindle as a white screen.

The self-check runs in two passes: once on the full assembled blob (for structural corruption and tag balance at the document level), and once on each individual record after splitting (for per-record HTML balance, catching tag pairs like `<b>...</b>` that would straddle a record boundary and cause bold or italic state to leak). Kindle decodes each text record independently for pagination, so a single unbalanced record can corrupt rendering even when the assembled blob is well-formed. Together the two passes add ~50-200 ms to a large dictionary build. The check never aborts the build: when something is wrong, kindling prints a warning block pointing at the issue and writes the MOBI anyway, so you can still inspect the output.

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
```

Drop-in replacement. Same CLI syntax, same status codes (`:I1036:` on success, `:E23026:` on failure). The KDP pre-flight validator runs by default in kindlegen-compat mode too; pass `-no_validate` (or `--no-validate`) to skip it.

### Dump

```bash
kindling-cli dump input.mobi      # structural dump to stdout
kindling-cli dump input.azw3
```

`kindling dump` prints the parsed structure of a MOBI/AZW3 file one `section.field = value` line at a time: PalmDB and MOBI header fields, every EXTH record, the INDX and ORDT2 tables, and entry labels. Text and image records are summarized by length and magic only. The line-oriented output is designed so `diff -u` between two dumps surfaces semantic differences without drowning in absolute-offset noise, which is how the kindlegen parity work is done. It is a read-only inspection tool and never writes to the input.

### Lookup simulator

```bash
kindling-cli lookup dict.mobi rivière   # prints how the word would resolve on-device
```

`kindling lookup` simulates the Kindle firmware's dictionary search against a built MOBI and reports which stored form a tapped word resolves to (and its text position), or that nothing resolves. It reads the collation from the orth-index header and applies the matching normalization: accent and case folding for Latin and Greek, literal per-character matching for the generated-ORDT scripts (Japanese, Chinese, Korean, Arabic), and query-side case folding against as-stored labels for Cyrillic. So `riviere` and `RIVIÈRE` both resolve `rivière`, an all-caps `ФСБ` resolves only if a lowercase alias exists (issue #17), and the exit status is non-zero on a miss so it works as a scriptable assertion.

This is a build-side regression harness, not a hardware oracle: its fidelity is bounded by our understanding of the firmware, so it catches encode-side mistakes (label sort order, a missing alias, ORDT symbol numbering) but cannot discover unknown firmware behavior. The normalization it uses is grounded in Amazon's own data (the fold table lifted from the collation blob, and the ORDT tables embedded in the file itself) rather than invented. Implementation is in [`src/lookup.rs`](src/lookup.rs).

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

*kindlegen* takes a different approach: a separate inflection INDX with compressed string transformation rules that map inflected forms back to headwords. This encoding is undocumented, limited to [255 inflections per entry](https://ebooks.stackexchange.com/questions/8461/kindlegen-dictionary-creation) (uint8 overflow), and adds complexity without benefit. Kindling has no per-entry limit.

## MOBI Format

Kindling works with the KF7/MOBI format used by Kindle e-readers. The key structures are:

- **PalmDB header**: Database name, record count, record offsets
- **Record 0**: PalmDOC header + MOBI header (264 bytes) + EXTH metadata + full name
- **Text records**: PalmDOC LZ77 compressed HTML with trailing bytes (`\x00\x81`)
- **INDX records**: Orthographic index with headword entries, character mapping, and sort tables
- **Image records**: Raw JPEG/PNG with JFIF header patching for Kindle cover compatibility
- **KF8 section**: Dual-format output with BOUNDARY record, KF8 text, FDST, skeleton/fragment/NCX indexes
- **HD container**: CONT/CRES records for high-DPI Kindle screens
- **FLIS/FCIS/EOF**: Required format records

### Key format details

Much of the foundational MOBI format knowledge comes from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI). The dictionary-specific details below were worked out empirically while building this project.

- **Text record sizing**: Every PalmDOC text record (except the last) must satisfy two constraints simultaneously:
  1. **Exactly 4096 bytes of decompressed content** (matching the declared `text_record_size` in the PalmDOC header). Kindle firmware routes popup lookups by computing `record_idx = byte_offset / text_record_size`, treating `text_record_size` as a constant. Records that drift below 4096 (e.g. by backing off to UTF-8 character boundaries) accumulate a per-record offset error that misroutes popup queries to wrong entries, and the further into the alphabet the query, the worse the drift. Records significantly shorter than 4096 (e.g. by backing off to `<hr/>` entry separators) break routing entirely.
  2. **Each record individually decodes as valid UTF-8 and parseable HTML.** Kindle's library indexer parses each record independently and will silently refuse to index a dictionary above some threshold of mid-character or mid-tag splits. Basic dictionaries with ~16% bad records still index; pro dictionaries with ~25% bad records do not.

  Kindling satisfies both constraints by emitting fixed-size 4096-byte chunks but inserting ASCII space padding at HTML inter-element gaps (between a `>` and the next `<`) so each chunk ends just past a complete tag close. The padding sits in HTML inter-element whitespace zones, which parsers collapse, so it has no rendering impact and never lands inside `<b>headword</b>` text runs that would break entry-position lookup.
- **Trailing bytes** (`\x00\x81`): Every text record ends with a multi-byte flag byte (`0x00`) followed by a TBS byte (`0x81`) as the very last byte. The Kindle decompressor walks backward from the end of the record, consuming the TBS byte first (bit 1 of `extra_flags`), then the multi-byte tail (bit 0); this ordering is mandatory. Earlier kindling builds wrote these bytes in reverse order and produced a white screen on device.
- **Inverted VWI**: Tag values use "high bit = stop" encoding (opposite of standard VWI).
- **SRCS record**: Must have 16-byte header (`SRCS` + length + size + count), pointed to by MOBI header offset 208. Required for Kindle Previewer.
- **Skeleton and fragment INDX (KF8)**: KF8 HTML is split into a "skeleton" per source file and one fragment per `<aid>` insert point. Skeleton entries carry a byte offset, length, and fragment count; fragment entries use a numeric decimal label (parsed as an integer) plus insert position, file number, sequence, and length.
- **Orth INDX header**: The orth INDX primary header declares index encoding `65002` (0xFDEA) and carries the input language's Windows primary LCID at offset 32. For non-Japanese dictionaries labels are UTF-16BE: `ocnt`/`oentries` in the header tail are set so the label reader decodes the UTF-16BE bytes directly instead of translating them through a small embedded ORDT2 table (which cannot cover non-trivial scripts).
- **Generated ORDT tables**: For Japanese, Chinese, Korean, and Arabic dictionaries, labels are sequences of one element per character indexing a generated ORDT table pair appended to the primary record (`ordt_type` at offset 164, count at 168, table offsets at 172/176). A character with a table symbol (kana, plus NUL/`%`/`_`) is stored as its symbol index; ORDT2 maps that symbol to the character's Unicode code point and ORDT1 to its gojuon collation weight, with katakana folded onto the matching hiragana weight (ヴ ヵ ヶ collate as う か け). The prolonged sound mark ー is folded before encoding: it becomes a vowel-specific marker (U+3095..U+3098, U+309F) carrying the preceding vowel's weight, propagating across consecutive ー, and staying an ignorable weight-0 symbol only when no vowel precedes it (word start, after ん/ン); the middle dot ・ and the iteration marks are kept as weight-0 symbols too. This matches kindlegen and the firmware's own normalization of a tapped ー, so katakana names with long vowels resolve on device. Every other character (kanji, Hangul, Arabic letters) is stored as an out-of-table literal: a label element is a literal exactly when its value is `>= oentries`. The full hiragana and katakana blocks are embedded only when the dictionary contains kana; all-literal scripts get the minimal three-entry table kindlegen emits, since a large kana table makes the firmware mis-collate them. Elements are one byte each (`ordt_type` 1) unless a literal is present or the table exceeds 256 symbols, in which case they are big-endian u16 (`ordt_type` 0). The firmware encodes a tapped word the same way, so kana fold and kanji match exactly. The ORDT table and headword labels are byte-identical to kindlegen for the all-literal scripts; for Japanese the kana symbol numbering differs (it has no effect, the firmware resolves kana by code point), while the literal code points and the collation order match. See `src/ordt.rs`.
- **Routing entries**: Each primary-INDX routing entry is `[1 byte label length][label bytes][2 byte big-endian record entry count]`. The trailing count is what lets Kindle's binary search pick the right data record for a lookup.
- **Dictionary links**: Anchor links work when browsing the dictionary as a book, but are disabled in the lookup popup. See the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf), section 15.6.1.

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
| 105 | Subject | Books | UTF-8 string | Maps to ComicInfo.xml `<Genre>` |
| 106 | Publishing date | Both | UTF-8 string | |
| 112 | Series | Books | UTF-8 string | Calibre `calibre:series` |
| 113 | Series index | Books | UTF-8 string | Calibre `calibre:series_index` |
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
| 501 | Document type | Comics | ASCII string | See table below. Written only for comics. Omitted for reflowable books (its presence hides the reader's back-to-library toolbar, issue #15) and for dictionaries (*kindlegen* omits it; Kindle recognizes dicts via orth index + EXTH 547) |
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

Dictionaries do NOT use EXTH 501. The Kindle identifies dictionaries by the combination of a valid orth index (MOBI header offset 24), EXTH 531/532 language records, and EXTH 547 `InMemory`. Adding an unrecognized EXTH 501 value (e.g. `"DICT"`) can prevent the Kindle from recognizing the file as a dictionary.

## Project layout

Standard Rust layout with `Cargo.toml` at the repo root:

```
kindling/
├── Cargo.toml                   # edition 2024, Rust 1.85+
├── src/
│   ├── lib.rs                   # Library crate root, public API for external consumers
│   ├── main.rs                  # CLI: build, comic, stardict, epub2, epub3, validate, repair, rewrite-metadata, dump, kindlegen-compat
│   ├── mobi.rs                  # PalmDB + MOBI record 0 + EXTH writer, UTF-8/tag-safe record splitter
│   ├── mobi_check.rs            # Post-build MOBI readback: PalmDB, EXTH, text-record sanity
│   ├── mobi_rewrite.rs          # In-place MOBI/AZW3 metadata and cover rewrite
│   ├── mobi_dump.rs             # Structural dump of a MOBI/AZW3 (the `dump` subcommand)
│   ├── kf8.rs                   # KF8 section, BOUNDARY, FDST, skeleton/fragment indexes
│   ├── cncx.rs                  # CNCX (compiled NCX) records for KF8 navigation
│   ├── indx.rs                  # Orthographic INDX records for dictionaries (ORDT/SPL sort tables)
│   ├── ordt.rs                  # Generated ORDT collation tables and label encoding (ja/zh/ko/ar)
│   ├── palmdoc.rs               # PalmDOC LZ77 compression
│   ├── exth.rs                  # EXTH record encoding
│   ├── vwi.rs                   # Variable-width integer encoding
│   ├── opf.rs                   # OPF and EPUB parsing (Method 1 and Method 2 covers)
│   ├── epub.rs                  # EPUB extraction for books and comics
│   ├── extracted.rs             # Normalized in-memory view of an extracted EPUB/OPF
│   ├── epub_build.rs            # EPUB2/EPUB3 output builders (generic book + EPUB3 dictionary layer)
│   ├── comic.rs                 # Comic pipeline (crop, split, enhance, Panel View)
│   ├── profile.rs               # Per-device comic profiles (screen size, gamma)
│   ├── cbr.rs                   # CBR (RAR) extraction via bsdtar
│   ├── moire.rs                 # Moire correction for color e-ink
│   ├── validate.rs              # KDP pre-flight driver; iterates `checks::CHECKS`
│   ├── checks/                  # One Rust module per rule cluster, all impl `Check`
│   ├── repair.rs                # Structural EPUB repair pass for Kindle ingest
│   ├── stardict.rs              # StarDict 2.4.2 builder (.ifo/.idx/.dict/.syn) for GoldenDict, KOReader, sdcv
│   ├── kdp_rules.rs             # Rule catalog (KPG_VERSION, Rule struct, RULES array)
│   ├── html_check.rs            # HTML/XHTML self-check for assembled MOBI text blob and per-record balance
│   ├── ordt_greek.bin           # Embedded ORDT/SPL sort tables extracted from kindlegen output
│   └── tests.rs                 # Unit tests
├── tests/
│   ├── cli.rs                   # CLI smoke tests for validate, repair, rewrite-metadata, build, comic
│   ├── kindlegen_parity.rs      # Byte/field parity vs committed kindlegen reference .mobi files
│   ├── dict_languages.rs        # Per-language dictionary tests (en/el/fr/ru/tr/ja/zh/ko/ar)
│   ├── roundtrip.rs             # Structural round-trip of kindling output via inline MOBI reader
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

The suite currently contains over 810 tests spanning unit tests in `src/tests.rs` and per-cluster tests in `src/checks/`, CLI integration tests in `tests/cli.rs` that invoke the compiled `kindling-cli` binary against OPF/EPUB/MOBI fixtures under `tests/fixtures/`, structural round-trip tests in `tests/roundtrip.rs`, per-language dictionary tests in `tests/dict_languages.rs`, and kindlegen parity tests in `tests/kindlegen_parity.rs`. An opt-in corpus harness in `tests/epub_tests_corpus.rs` runs every test in a local [w3c/epub-tests](https://github.com/w3c/epub-tests) checkout through the validator to surface false positives and measure coverage; set `KINDLING_CORPUS_DIR` to the checkout path and run `cargo test --release --test epub_tests_corpus -- --ignored --nocapture`.

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
- **Compression**: PalmDOC LZ77 compress/decompress roundtrips for various sizes and encodings
- **Regression tests**: Dictionary capability marker (0x50 vs 0x4850), JFIF density patching, RTL spread cover selection, dictionary text record trailing byte order
- **Structural round-trip tests** (`tests/roundtrip.rs`): build each of the three parity fixtures with `kindling-cli`, parse the result back with a minimal inline MOBI reader in `tests/common/mod.rs`, and assert the PalmDB header, MOBI header, EXTH, INDX / SKEL / FRAG records, and decompressed text blob have the exact shape we expect. These catch format-level regressions where libmobi would happily accept an output that does not round-trip.
- **kindlegen byte/field parity tests** (`tests/kindlegen_parity.rs`): build the same inputs with `kindling-cli` and diff the output field-by-field against a committed kindlegen reference `.mobi`. Timestamp/UID fields (EXTH 112, 113, 204-207, etc.) are compared by presence only; core metadata (EXTH 100, 101, 524) must match exactly. Divergences are reported in a readable table via `cargo test -- --nocapture`.
- **Per-language dictionary tests** (`tests/dict_languages.rs`): build a dictionary for each supported language from its `tests/fixtures/langs/<code>/` fixture and assert the INDX/MOBI language ids and locale, that every entry has a non-empty text pointer (the first-entry white-page guard), that headwords round-trip through the labels, and the per-language collation. For the generated-ORDT languages (ja, zh, ko, ar) decode every per-character label back to its headword and assert byte parity with the committed kindlegen build: the ORDT table and orth headword labels are identical for the all-literal scripts, and the literal code points plus collation order match for Japanese.

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

**Legal note.** The committed `kindlegen_reference.mobi` files are Kindle-format builds of the repo's own fixture content, produced by running kindlegen on OPF/EPUB sources that live next to them. Amazon's copyright does not extend to the OUTPUT kindlegen produces from your own content, so these files are safe to commit. What you cannot commit is the kindlegen BINARY itself; it remains Amazon-proprietary and is only required to run `scripts/regenerate_parity_fixtures.sh` when a source fixture changes.

Parity fixture contents:
- `simple_dict/` - 5-headword Latin-script dictionary with inflections
- `simple_book/` - 3-chapter plain book with CSS and a JPEG cover
- `simple_comic/` - 3-page CBZ fed to kindling; matching fixed-layout EPUB wrapper fed to kindlegen during regeneration

## Known Kindle firmware issues

These are Amazon firmware bugs, not kindling bugs, but they affect sideloaded MOBI/AZW3 files and users should be aware of them.

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
