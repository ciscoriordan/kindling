# Kindling

<p align="center">
  <img width="700" alt="Kindling - Modern, cross-platform kindlegen replacement" src="kindling_logo.jpg">
</p>

Reverse-engineered Rust replacement for Amazon's *kindlegen*. 7,000x faster.

Kindling builds Kindle dictionary `.mobi` files from OPF/HTML source. It produces the same MOBI V7 format that *kindlegen* does, with working dictionary lookup on Kindle hardware. The MOBI format is undocumented - kindling was built by reverse-engineering *kindlegen* output byte by byte.

Amazon deprecated *kindlegen* in 2020. The only remaining copy lives inside Kindle Previewer 3's GUI, which can't run headless and takes 12+ hours for large dictionaries. Kindling builds the same dictionary in 6 seconds.

Pre-built binaries for Mac (Apple Silicon, Intel), Linux (x86_64), and Windows (x86_64) are available on the [Releases](https://github.com/ciscoriordan/kindling/releases) page.

<p align="center">
  <img width="500" alt="Greek dictionary lookup on Kindle" src="kindle_test.jpg">
</p>

- Single static binary, no runtime dependencies
- Native performance: builds large dictionaries in seconds, not hours
- MOBI V7 format with FLIS/FCIS records, matching *kindlegen* output
- INDX orth index with 3 sub-indexes: headword entries, character mapping, and "default" index name
- ORDT/SPL sort tables for firmware binary search compatibility
- EXTH 300 fontsignature (LE USB/CSB bitfields + Unicode character list)
- HTML stripping: `idx:entry/idx:orth/idx:iform` markup removed from stored text
- Multi-record INDX with automatic splitting for large dictionaries
- PalmDOC LZ77 compression

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

## How inflection lookup works

Kindle dictionary lookup searches the orthographic (orth) INDX for a matching headword. Kindling places all lookupable terms - both headwords and their inflected forms - directly into the orth index. Each inflected form entry points to the same text position as its headword, so looking up "cats" finds the "cat" entry.

### Why not use *kindlegen*'s inflection index?

*kindlegen* uses a two-index system: an orthographic INDX (type=0) for headwords, and a separate inflection INDX (type=2) that encodes transformation rules in a compact binary format. The inflection index stores compressed string transformation rules (prefix/suffix operations) that map inflected forms back to headwords. This encoding is undocumented and uses a complex binary format that was reverse-engineered by the MobileRead community.

Kindling takes a different approach: it places ALL lookupable terms (headwords + inflections) directly into the orthographic index, with each entry pointing to the correct text position. This means a headword like "cat" and all its forms ("cats", "cat's") each get their own orth entry pointing to the same dictionary text.

**Trade-offs**: Kindling's approach produces simpler, more maintainable code and is equally functional for Kindle word lookup. In practice it can actually produce smaller files, because it avoids the overhead of the inflection index structure. Kindling's orth-only approach also bypasses *kindlegen*'s undocumented 255-inflection-per-entry limit. *kindlegen* stores inflection rules in an internal uint8 field, causing it to silently discard rules beyond 255 per headword. Since kindling puts all forms directly in the orth index, there is no per-entry inflection limit.

## Performance

| | *kindlegen* | kindling |
|---|---|---|
| Greek dictionary (80K headwords, 452K index entries) | 12+ hours on Mac (Rosetta 2), frequently OOM | 6 seconds |
| Platform support | macOS x86_64 only (Rosetta on Apple Silicon), 32-bit Windows (crashes on large files), Linux binary no longer available | Mac Apple Silicon, Mac Intel, Linux, Windows |
| Inflection limit | 255 per headword (uint8 overflow, silently drops forms) | No limit |
| Automation | Requires Kindle Previewer GUI, no headless mode | Single binary, scriptable, CI-friendly |

*kindlegen* runs under Rosetta 2 on Apple Silicon Macs, adding overhead to an already slow build. For highly-inflected languages (Greek, Finnish, Turkish, Arabic), dictionaries with hundreds of thousands of inflected forms can take 12+ hours or exhaust memory entirely. *kindlegen*'s 32-bit Windows build is limited to ~2 GB of address space and crashes on large dictionaries. The Linux binary was a 32-bit i386 build that is no longer available from Amazon.

## MOBI Format

Kindling works with the KF7/MOBI format used by Kindle e-readers for dictionary lookup. The key structures are:

- **PalmDB header**: Database name (derived from title: remove `()[]`, spaces to underscores, truncate to `first_12 + '-' + last_14` if >27 chars), record count, record offsets
- **Record 0**: PalmDOC header + MOBI header (264 bytes, V7) + EXTH metadata + full name
- **Text records**: PalmDOC compressed HTML with `extra_data_flags=3` trailing bytes (`\x00\x81`). The `\x00` is the multibyte overlap marker (innermost, bit 0) and `\x81` is the TBS size byte (outermost, bit 1) with bit 7 set for self-delimiting VWI parsing. Dictionary markup (`idx:entry`, `idx:orth`, `idx:iform`) is stripped from the stored text, leaving clean display HTML.
- **INDX records**: 3 sub-indexes within the orth region:
  1. Headword entries (TAGX: tags 1=startpos, 2=textlen). Primary includes ORDT/SPL sort tables.
  2. Character mapping (TAGX: tag 37) - unique non-ASCII headword characters
  3. "default" index name (TAGX: tag 1) - identifies the lookup index
- **FLIS/FCIS/EOF**: Required V7 format records

### Key format details

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
  - **EXTH 204/205/206/207**: Creator software version fields. Kindling reports as kindlegen Mac (202) v2.9.
- **PalmDB name**: Derived from `dc:title` by removing `()[]`, replacing spaces with underscores, and truncating to `first_12 + '-' + last_14` if longer than 27 characters.
- **Dictionary detection**: The Kindle identifies a file as a dictionary when the MOBI header orthographic index field (offset 0x28) is not 0xFFFFFFFF. MOBI type remains 2 (book), not a special dictionary type.

## License

MIT
