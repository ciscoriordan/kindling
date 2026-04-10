# Kindling

<img width="100%" alt="Kindling - The missing MOBI generator. Dictionaries, books, comics." src="images/kindling_social.jpg">

The missing Kindle toolkit. Dictionaries, books, and comics. Single static Rust binary, no dependencies, cross-platform.

Amazon deprecated *kindlegen* in 2020, leaving no supported way to build Kindle MOBIs. The only remaining copy is buried inside Kindle Previewer 3's GUI, can't run headless, and can take 12+ hours (or run out of memory entirely) for large dictionaries due to x86-only Rosetta 2 emulation on Apple Silicon, superlinear inflection index computation, and a 32-bit Windows build that crashes on large files. Kindling builds the same dictionary in 6 seconds.

For comics, [KCC](https://github.com/ciromattia/kcc) exists but requires Python, PySide6/Qt, Pillow, 7z, mozjpeg, psutil, pymupdf, and more. Installation is painful across platforms, there's no headless mode for CI, and Python image processing is slow. Kindling replaces all of that with a single statically-linked native binary, compiled from Rust.

Kindling was built by reverse-engineering Amazon's undocumented MOBI format byte by byte, with help from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI).

Pre-built binaries for Mac (Apple Silicon, Intel), Linux (x86_64), and Windows (x86_64): [Releases](https://github.com/ciscoriordan/kindling/releases)

<p align="center">
  <img width="400" alt="Greek dictionary lookup on Kindle" src="images/kindle_test.jpg">
  <img width="400" alt="Pepper & Carrot comic on Kindle" src="images/kindle_comic_test.jpg">
</p>

## Features

- **Dictionaries**: Full orth index with headword + inflection lookup, ORDT/SPL sort tables, fontsignature
- **Books**: EPUB or OPF input, embedded images, KF8 dual-format (KF7+KF8) or KF8-only (.azw3), HD image container, fixed-layout support
- **Comics**: Image folder, CBZ, or EPUB input, device-specific resizing, spread splitting, margin cropping, auto-contrast, moire correction for color e-ink, manga RTL, webtoon with overlap fallback, Panel View, metadata overrides
- Drop-in *kindlegen* replacement (same CLI flags, same status codes)
- Kindle Previewer compatible (EPUB source embedded by default)
- Comprehensive test suite with CI on every push (see [Testing](#testing))

## Installation

Download the latest release for your platform from [Releases](https://github.com/ciscoriordan/kindling/releases):

- **Mac Apple Silicon** - `kindling-cli-mac-apple-silicon`
- **Mac Intel** - `kindling-cli-mac-intel`
- **Linux** - `kindling-cli-linux`
- **Windows** - `kindling-cli-windows.exe`

Or build from source:
```bash
cd rust && cargo build --release
```

## Usage

### Dictionaries

```bash
kindling-cli build input.opf -o output.mobi
kindling-cli build input.opf -o output.mobi --no-compress    # skip compression for fast dev builds
kindling-cli build input.opf -o output.mobi --headwords-only  # index headwords only (no inflections)
kindling-cli build input.opf -o output.mobi --no-kindle-limits  # skip per-letter HTML grouping
```

The input OPF must reference HTML files with `<idx:entry>`, `<idx:orth>`, and `<idx:iform>` markup following the [Amazon Kindle Publishing Guidelines](http://kindlegen.s3.amazonaws.com/AmazonKindlePublishingGuidelines.pdf). Both headwords and inflected forms are indexed so that looking up any form on the Kindle finds the correct dictionary entry.

By default, dictionaries enforce Kindle publishing limits (`--kindle-limits`): entries are grouped by first letter to keep individual HTML sections under 30 MB, and a warning is printed if the total exceeds 300 sections. Use `--no-kindle-limits` to disable this and produce a single flat HTML blob.

### Books

```bash
kindling-cli build input.epub -o output.mobi
kindling-cli build input.epub                          # output next to input as input.mobi
kindling-cli build input.epub --kf8-only               # KF8-only output (.azw3), smaller files
kindling-cli build input.epub --kf8-only -o book.azw3  # explicit output path
kindling-cli build input.epub --no-hd-images           # skip HD image container
kindling-cli build input.epub --no-embed-source        # smaller file, but breaks Kindle Previewer
kindling-cli build input.epub --kindle-limits          # warn about HTML files exceeding 30 MB
```

Auto-detects dictionary vs book by checking for `<idx:entry>` tags. Book MOBIs include embedded images, HD image container (for high-DPI Kindle screens), and KF8 dual-format output. The original EPUB is embedded by default for Kindle Previewer compatibility (`--no-embed-source` to skip).

The `--kf8-only` flag outputs KF8-only format with `.azw3` extension instead of the default dual MOBI7+KF8 `.mobi`. KF8-only files are smaller (no redundant MOBI7 section) and handled better by Calibre. Dual format remains the default for maximum compatibility with older Kindle devices.

### Comics

```bash
kindling-cli comic input.cbz -o output.mobi --device paperwhite
kindling-cli comic manga.epub -o output.mobi --rtl              # EPUB comic/manga
kindling-cli comic manga.cbz -o output.mobi --rtl              # manga (right-to-left)
kindling-cli comic webtoon/ -o output.mobi --webtoon            # webtoon (vertical strip)
kindling-cli comic input/ -o output.mobi --no-split --no-crop   # disable smart processing
kindling-cli comic input.cbz --title "My Comic" --language ja   # metadata overrides
kindling-cli comic input.cbz --doc-type ebok                    # appear under Books on Kindle
kindling-cli comic input.cbz --cover 3                          # use page 3 as cover
```

Converts image folders, CBZ files, and EPUB files to Kindle-optimized MOBI with:
- **Device profiles**: *paperwhite*, *kpw5*, *oasis*, *scribe*, *scribe2025*, *kindle2024*, *basic*, *colorsoft*, *fire-hd-10*
- **Spread splitting**: Landscape images auto-split into two pages (disable: `--no-split`)
- **Margin cropping**: Uniform-color borders auto-removed (disable: `--no-crop`)
- **Auto-contrast**: Histogram stretching and gamma correction for e-ink (disable: `--no-enhance`)
- **Moire correction**: Rainbow artifact removal for color e-ink screens (Colorsoft), applied automatically to grayscale source images
- **Manga mode**: `--rtl` reverses page order and split direction
- **Webtoon mode**: `--webtoon` merges vertical strips and splits at panel gutters with overlap fallback to prevent content loss
- **Panel View**: Tap-to-zoom panel detection for Kindle (disable: `--no-panel-view`). Reading order configurable via `--panel-reading-order` (`horizontal-lr`, `horizontal-rl`, `vertical-lr`, `vertical-rl`); defaults to `horizontal-rl` with `--rtl`
- **EPUB support**: Fixed-layout EPUB comics extracted in spine order (correct page sequence)
- **ComicInfo.xml**: Auto-reads metadata and manga direction from CBZ files
- **Metadata overrides**: `--title`, `--author`, `--language`, `--cover` (page number or file path)
- **Document type**: `--doc-type ebok` to appear under Books instead of Documents on Kindle (default: `pdoc`)

### Kindlegen compatibility

```bash
kindling-cli input.epub                          # same as kindlegen
kindling-cli input.epub -dont_append_source      # flag accepted
kindling-cli input.epub -o output.mobi           # explicit output path
```

Drop-in replacement. Same CLI syntax, same status codes (`:I1036:` on success, `:E23026:` on failure).

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

Much of the foundational MOBI format knowledge comes from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI). The dictionary-specific details below were reverse-engineered from *kindlegen* output for this project.

- **Trailing bytes** (`\x00\x81`): The TBS byte MUST have bit 7 set for the Kindle's backward VWI parser to self-delimit. Using `\x01\x00` (wrong order, no bit 7) destroys all text content.
- **Inverted VWI**: Tag values use "high bit = stop" encoding (opposite of standard VWI).
- **SRCS record**: Must have 16-byte header (`SRCS` + length + size + count), pointed to by MOBI header offset 208. Required for Kindle Previewer.
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
| 501 | Document type | Books | ASCII string | See table below. **Not written for dictionaries** - *kindlegen* omits it for dicts and Kindle recognizes dicts via orth index + EXTH 547 instead |
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
| `EBOK` | Books shelf | **Warning**: Amazon may auto-delete sideloaded EBOK files when the Kindle connects to WiFi, since it checks whether the ASIN is in the user's purchase history |
| `PDOC` | Documents shelf | Safe default for sideloaded content |

Dictionaries do NOT use EXTH 501. The Kindle identifies dictionaries by the combination of a valid orth index (MOBI header offset 24), EXTH 531/532 language records, and EXTH 547 `InMemory`. Adding an unrecognized EXTH 501 value (e.g. `"DICT"`) can prevent the Kindle from recognizing the file as a dictionary.

## Testing

Tests run automatically on every push and pull request via [GitHub Actions](.github/workflows/test.yml).

```bash
cd rust && cargo test -- --show-output
```

The test suite covers:

- **MOBI structure**: PalmDB header, record offsets, MOBI header fields (magic, version, encoding, capability markers), FLIS/FCIS/EOF records
- **Dictionary output**: Orth index presence and structure, INDX records, headword count, EXTH 531/532/547 language records, compressed vs uncompressed roundtrips
- **Book output**: KF7+KF8 dual format (BOUNDARY record, KF8 section version), KF8-only output (.azw3), image record JPEG magic, EXTH metadata, SRCS embedding
- **Comic pipeline**: Device profiles (including kpw5, scribe2025, kindle2024), spread detection and splitting, crop-before-split symmetry, margin cropping, auto-contrast, moire wiring for color devices, webtoon merge/split with overlap fallback, dark gutter detection, Panel View markup, manga RTL ordering and cover selection, JPEG quality, ComicInfo.xml parsing, EPUB image extraction
- **Comic CLI flags**: doc-type EBOK/PDOC, title/author/language overrides
- **Compression**: PalmDOC LZ77 compress/decompress roundtrips for various sizes and encodings
- **Regression tests**: Dictionary capability marker (0x50 vs 0x4850), JFIF density patching, RTL spread cover selection

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

Kindling's comic mode was informed by the pioneering work of [AcidWeb](https://github.com/AcidWeb) on [KCC (Kindle Comic Converter)](https://github.com/ciromattia/kcc), who built the core architecture for panel detection, webtoon processing, and Kindle device compatibility over hundreds of commits. KCC remains the reference implementation for many comic-to-Kindle techniques.

The MOBI format knowledge beyond what the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI) covers was greatly aided by Dc5e's [KindleComicParser](https://www.mobileread.com/forums/showthread.php?t=192783), which provided detailed analysis of the binary format structures used by kindlegen for fixed-layout content.

## Related projects

- [Lemma](https://github.com/ciscoriordan/lemma) - Greek-English Kindle dictionary built with Kindling

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=ciscoriordan/kindling&type=Date)](https://star-history.com/#ciscoriordan/kindling&Date)

## License

MIT - © 2026 Francisco Riordan
