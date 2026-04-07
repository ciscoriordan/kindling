# Kindling

<p align="center">
  <img width="700" alt="Kindling - Modern, cross-platform kindlegen replacement" src="kindling_logo.jpg">
</p>

Reverse-engineered Rust replacement for Amazon's *kindlegen*. 7,000x faster.

Kindling builds Kindle `.mobi` files from OPF/HTML or EPUB source. It supports both dictionary MOBIs (with full lookup index) and regular book MOBIs (with embedded images and KF8 dual-format output). The MOBI format is barely documented by Amazon - kindling was built by reverse-engineering *kindlegen* output byte by byte, with help from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI).

Amazon deprecated *kindlegen* in 2020. The only remaining copy lives inside Kindle Previewer 3's GUI, which can't run headless and takes 12+ hours for large dictionaries. Kindling builds the same dictionary in 6 seconds.

Kindling was originally built to generate [Lemma](https://github.com/ciscoriordan/lemma), a Greek-English Kindle dictionary with 80K headwords and 452K inflected forms.

Pre-built binaries for Mac (Apple Silicon, Intel), Linux (x86_64), and Windows (x86_64) are available on the [Releases](https://github.com/ciscoriordan/kindling/releases) page.

<p align="center">
  <img width="500" alt="Greek dictionary lookup on Kindle" src="kindle_test.jpg">
</p>

- Single static binary, no runtime dependencies
- **Dictionary MOBIs**: full orth index with headword + inflection lookup, ORDT/SPL sort tables, fontsignature
- **Book MOBIs**: EPUB or OPF input, embedded images, HD image container for high-DPI screens, KF8 dual-format (KF7+KF8), fixed-layout support
- Auto-detects dictionary vs book from content
- Drop-in *kindlegen* replacement: accepts the same CLI flags, prints compatible status codes
- Native performance: builds large dictionaries in seconds, not hours
- PalmDOC LZ77 compression, JFIF header patching for Kindle cover compatibility
- Optional EPUB source embedding (`--embed-source`) and build metadata (`--include-cmet`)

## Installation

Download the latest release for your platform from [Releases](https://github.com/ciscoriordan/kindling/releases):

- **Mac Apple Silicon** - `kindling-mac-apple-silicon`
- **Mac Intel** - `kindling-mac-intel`
- **Linux** - `kindling-linux`
- **Windows** - `kindling-windows.exe`

Or build from source:
```bash
cargo build --release
```

## Usage

### Build a MOBI dictionary

```bash
kindling build input.opf -o output.mobi
kindling build input.opf -o output.mobi --no-compress    # skip compression for fast dev builds
kindling build input.opf -o output.mobi --headwords-only # index headwords only (no inflections)
```

The input OPF must reference HTML files with `<idx:entry>`, `<idx:orth>`, and `<idx:iform>` markup following the Kindle Publishing Guidelines. Both headwords and inflected forms are indexed so that looking up any form on the Kindle finds the correct dictionary entry.

### Build a MOBI book

```bash
kindling build input.epub -o output.mobi
kindling build input.epub                         # output next to input as input.mobi
kindling build input.epub --no-hd-images          # skip HD image container
kindling build input.epub --embed-source          # embed original EPUB in MOBI
kindling build input.epub --include-cmet          # include build metadata
```

Kindling accepts EPUB files (standard zip-packaged EPUB) or OPF files as input. It auto-detects whether the content is a dictionary or a regular book by checking for `<idx:entry>` tags in the HTML. Book MOBIs include embedded images, HD image container (for high-DPI Kindle screens), and KF8 dual-format output for compatibility with all Kindle devices. Fixed-layout EPUBs (e.g., from [Kindle Comic Converter](https://github.com/ciromattia/kcc)) are detected automatically.

By default, kindling skips two optional records that *kindlegen* includes: SRCS (a copy of the original EPUB embedded in the MOBI) and CMET (build log metadata). The Kindle ignores both. Use `--embed-source` and `--include-cmet` to include them if needed.

### Kindlegen compatibility

Kindling can be used as a drop-in *kindlegen* replacement. It accepts the same CLI syntax and prints compatible status codes:

```bash
kindling input.epub                          # same as kindlegen
kindling input.epub -dont_append_source      # flag accepted and ignored
kindling input.epub -o output.mobi           # explicit output path
```

Tools that shell out to *kindlegen* (like KCC) can switch to kindling with minimal changes.

## How inflection lookup works

Kindle dictionary lookup searches the orthographic (orth) INDX for a matching headword. Kindling places all lookupable terms - both headwords and their inflected forms - directly into the orth index. Each inflected form entry points to the same text position as its headword, so looking up "cats" finds the "cat" entry.

### Why not use *kindlegen*'s inflection index?

*kindlegen* uses a two-index system: an orthographic INDX (type=0) for headwords, and a separate inflection INDX (type=2) that encodes transformation rules in a compact binary format. The inflection index stores compressed string transformation rules (prefix/suffix operations) that map inflected forms back to headwords. This encoding is undocumented and uses a complex binary format that was reverse-engineered by the MobileRead community.

Kindling takes a different approach: it places ALL lookupable terms (headwords + inflections) directly into the orthographic index, with each entry pointing to the correct text position. This means a headword like "cat" and all its forms ("cats", "cat's") each get their own orth entry pointing to the same dictionary text.

**Trade-offs**: Kindling's approach produces simpler, more maintainable code and is equally functional for Kindle word lookup. In practice it can actually produce smaller files, because it avoids the overhead of the inflection index structure. Kindling's orth-only approach also bypasses *kindlegen*'s undocumented 255-inflection-per-entry limit. *kindlegen* stores inflection rules in an internal uint8 field, causing it to silently discard rules beyond 255 per headword. Since kindling puts all forms directly in the orth index, there is no per-entry inflection limit. That said, 255 inflections per headword is still a reasonable practical guideline - Kindle devices have limited memory and very large index sizes can affect lookup performance.

## Performance

| | *kindlegen* | kindling |
|---|---|---|
| Greek dictionary (80K headwords, 452K index entries) | 12+ hours on Mac (Rosetta 2), frequently OOM | 6 seconds |
| Platform support | macOS x86_64 only (Rosetta on Apple Silicon), 32-bit Windows (crashes on large files), Linux binary no longer available | Mac Apple Silicon, Mac Intel, Linux, Windows |
| Inflection limit | 255 per headword (uint8 overflow, silently drops forms) | No limit |
| Automation | Requires Kindle Previewer GUI, no headless mode | Single binary, scriptable, CI-friendly |

### Output comparison

| Input | *kindlegen* | kindling | Speedup |
|---|---|---|---|
| Greek dictionary (80K headwords, 452K entries) | 12+ hours, frequent OOM | 6 seconds | ~7,000x |
| Divine Comedy (138 illustrations, 29MB of images) | 19 seconds | 0.5 seconds | ~40x |
| Pepper & Carrot comic (20 images) | 1.4 seconds | 0.05 seconds | ~30x |

The ~7,000x speedup is more or less a worst-case comparison, but it comes from several real factors. *kindlegen* builds a complex inflection index with compressed string transformation rules, which appears to scale superlinearly - smaller dictionaries finish much faster, but 452K inflections pushes it into 12+ hour territory. It also runs under Rosetta 2 on Apple Silicon and frequently exhausts memory, causing swapping or outright crashes. Kindling skips the inflection index entirely by placing all forms directly in the orth index, which reduces the problem to sorting and writing. The gap is largest for heavily-inflected languages (Greek, Finnish, Turkish, Arabic) with hundreds of thousands of forms.

## MOBI Format

Kindling works with the KF7/MOBI format used by Kindle e-readers for dictionary lookup. The key structures are:

- **PalmDB header**: Database name (derived from title: remove `()[]`, spaces to underscores, truncate to `first_12 + '-' + last_14` if >27 chars), record count, record offsets
- **Record 0**: PalmDOC header + MOBI header (264 bytes) + EXTH metadata + full name
- **Text records**: PalmDOC compressed HTML with `extra_data_flags=3` trailing bytes (`\x00\x81`). The `\x00` is the multibyte overlap marker (innermost, bit 0) and `\x81` is the TBS size byte (outermost, bit 1) with bit 7 set for self-delimiting VWI parsing. Dictionary markup (`idx:entry`, `idx:orth`, `idx:iform`) is stripped from the stored text, leaving clean display HTML.
- **INDX records**: 3 sub-indexes within the orth region:
  1. Headword entries (TAGX: tags 1=startpos, 2=textlen). Primary includes ORDT/SPL sort tables.
  2. Character mapping (TAGX: tag 37) - unique non-ASCII headword characters
  3. "default" index name (TAGX: tag 1) - identifies the lookup index
- **FLIS/FCIS/EOF**: Required V7 format records

### Key format details

Much of the foundational MOBI format knowledge comes from the [MobileRead wiki](https://wiki.mobileread.com/wiki/MOBI), which documents the community's reverse-engineering work over many years. The dictionary-specific details below were reverse-engineered from *kindlegen* output for this project.

- **Trailing bytes** (`\x00\x81`): Each text record ends with a multibyte marker + TBS byte. The TBS byte MUST have bit 7 set so the Kindle's backward VWI parser self-delimits. Using `\x01\x00` (wrong order, no bit 7) causes the parser to read into compressed data, destroying all text content and silently breaking lookup.
- **Inverted VWI**: Tag values use "high bit = stop" encoding (opposite of standard VWI). This is undocumented and was reverse-engineered by comparing against *kindlegen* output.
- **Labels**: UTF-16BE for non-ASCII headwords (Greek, Cyrillic, etc.), plain ASCII for ASCII-only labels.
- **No prefix compression**: *kindlegen* stores full label bytes for every INDX entry (99%+ have `prefix_len=0`). Kindling matches this.
- **INDX primary header offset 16**: Must be 2 (encoding indicator), not the actual data record count.
- **ORDT/SPL tables**: Appended to the primary INDX after IDXT. Contains ORDT1, ORDT2, and SPL1-SPL6 sections for Unicode collation. Header fields at offsets 56-176 must point to the SPL sections. Collation constants at offsets 84-144 are fixed values.
- **EXTH records** required for dictionary lookup:
  - **EXTH 300** (fontsignature): Windows FONTSIGNATURE struct with USB[4] + CSB[2] as little-endian uint32s (even inside big-endian MOBI), followed by 8 zero bytes and sorted unique non-ASCII headword codepoints as `(codepoint + 0x0400)` big-endian uint16. Tells the firmware which Unicode ranges the dictionary covers.
  - **EXTH 531/532**: Dictionary input/output language strings (e.g., "el", "en"). These are what make the Kindle recognize the file as a dictionary.
  - **EXTH 547** (`InMemory`): Required for dictionary lookup activation.
  - **EXTH 535**: Creator build tag (e.g., "0000-kdevbld").
  - **EXTH 542** (`Container_Id`): 4-byte content-dependent hash. Not a timestamp despite the MobileRead wiki claim.
  - **EXTH 204/205/206/207**: Creator software version fields.
- **PalmDB name**: Derived from `dc:title` by removing `()[]`, replacing spaces with underscores, and truncating to `first_12 + '-' + last_14` if longer than 27 characters.
- **Dictionary detection**: The Kindle identifies a file as a dictionary when the MOBI header orthographic index field (offset 0x28) is not 0xFFFFFFFF. MOBI type remains 2 (book), not a special dictionary type.

## Upcoming

- KCC ([Kindle Comic Converter](https://github.com/ciromattia/kcc)) integration - PR to add kindling as a backend
- Testing with more languages beyond Greek

## Attribution

Kindling was created by Francisco Riordan. Source code is available at https://github.com/ciscoriordan/kindling.

## License

MIT
