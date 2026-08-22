//! One-shot generator for the tests/fixtures/huffdic/ MOBI files.
//!
//! Run once with `cargo run --example gen_huffdic_fixture` and commit the
//! result. Generates, from the committed kindlegen English dictionary:
//! - tests/fixtures/huffdic/en_huffdic.mobi
//! - tests/fixtures/huffdic/en_huffdic_stale_index.mobi
//!
//! Why this exists. kindling reads HUFF/CDIC ("huffdic", PalmDOC compression
//! 17480) but never writes it, and the obvious way to get a fixture -
//! `kindlegen -c2` - is not available: the macOS kindlegen inside Kindle
//! Previewer 3 segfaults in its huffdic path on every input large enough to
//! actually reach it, so the reference build cannot produce one here. This
//! generator instead rewrites an existing PalmDOC dictionary into huffdic,
//! leaving every other byte alone: same INDX records, same EXTH, same images.
//! The encoder below is deliberately minimal (it exists to make fixtures, not
//! to compete with kindlegen's 4096-pass phrase mining) but its output is
//! format-legal, and was checked byte-for-byte against KindleUnpack's
//! independent `HuffcdicReader` before being committed.
//!
//! The second fixture is the shape of issue #49: identical to the first except
//! that the MOBI header's orth index record number was left pointing where the
//! index used to be, before the HUFF and CDIC records pushed it along. Every
//! lookup in such a file misses, and nothing about the miss says why.

use std::collections::BinaryHeap;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ---------------------------------------------------------------------
// PalmDOC LZ77 decompression (the fixture source is PalmDOC compressed)
// ---------------------------------------------------------------------

fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() * 4);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let n = (c as usize).min(src.len() - i);
                out.extend_from_slice(&src[i..i + n]);
                i += n;
            }
            0x00 | 0x09..=0x7F => out.push(c),
            0x80..=0xBF => {
                if i >= src.len() {
                    break;
                }
                let pair = ((c as usize) << 8) | src[i] as usize;
                i += 1;
                let dist = (pair >> 3) & 0x07FF;
                let len = (pair & 0x07) + 3;
                if dist == 0 || dist > out.len() {
                    break;
                }
                for _ in 0..len {
                    out.push(out[out.len() - dist]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------
// Huffman code lengths
// ---------------------------------------------------------------------

/// Node for the code-length pass. Ordered so `BinaryHeap` pops the lowest
/// weight first, with the index as a deterministic tiebreak.
#[derive(PartialEq, Eq)]
struct Node {
    weight: u64,
    id: usize,
}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .weight
            .cmp(&self.weight)
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Code length per symbol. Frequencies are floored so no symbol drops out of
/// the tree, and the floor is raised until the deepest code fits the 5-bit
/// length field the HUFF table has room for.
fn code_lengths(freqs: &[u64]) -> Vec<u8> {
    let n = freqs.len();
    assert!(n >= 2, "need at least two symbols");
    let mut floor = 1u64;
    loop {
        let weights: Vec<u64> = freqs.iter().map(|&f| f + floor).collect();
        let mut parent = vec![usize::MAX; 2 * n];
        let mut heap: BinaryHeap<Node> = (0..n)
            .map(|i| Node {
                weight: weights[i],
                id: i,
            })
            .collect();
        let mut next = n;
        while heap.len() > 1 {
            let a = heap.pop().unwrap();
            let b = heap.pop().unwrap();
            parent[a.id] = next;
            parent[b.id] = next;
            heap.push(Node {
                weight: a.weight + b.weight,
                id: next,
            });
            next += 1;
        }
        let mut lens = vec![0u8; n];
        let mut deepest = 0u8;
        for i in 0..n {
            let mut depth = 0u8;
            let mut cur = i;
            while parent[cur] != usize::MAX {
                cur = parent[cur];
                depth += 1;
            }
            lens[i] = depth;
            deepest = deepest.max(depth);
        }
        // 8 bits is the shortest code that can safely pad the last byte, and
        // 24 keeps the pre-shifted bounds well inside their fields.
        if (8..=24).contains(&deepest) {
            return lens;
        }
        assert!(deepest > 8, "alphabet too small to need an 8-bit code");
        floor *= 4;
    }
}

// ---------------------------------------------------------------------
// Code assignment
// ---------------------------------------------------------------------

/// Codes and the per-length bounds the HUFF tables are built from.
struct Codes {
    /// `(code, length)` per symbol.
    code: Vec<(u32, u8)>,
    /// Symbol ids in phrase-dictionary order.
    order: Vec<usize>,
    /// Smallest and largest code of each length, and the dictionary index of
    /// the symbol holding the largest. Zero for unused lengths.
    first: [u32; 33],
    last: [u32; 33],
    base: [u32; 33],
    used: [bool; 33],
}

/// Assign canonical Huffman codes, then complement them.
///
/// The decoder walks code lengths upward while the window sits *below*
/// `mincode[len]`, which only terminates on the right length if `mincode` is
/// non-increasing - that is, if short codes occupy the top of the window and
/// long ones the bottom. Plain canonical assignment gives the opposite order,
/// and complementing every code within its length flips it while leaving the
/// code prefix-free.
fn assign_codes(lens: &[u8]) -> Codes {
    let maxlen = *lens.iter().max().unwrap() as usize;
    let mut count = [0u32; 33];
    for &l in lens {
        count[l as usize] += 1;
    }

    // plain canonical codes, ascending within and across lengths
    let mut next = [0u32; 34];
    let mut c = 0u32;
    for l in 1..=maxlen {
        next[l] = c;
        c = (c + count[l]) << 1;
    }

    let mut plain = vec![0u32; lens.len()];
    for l in 1..=maxlen {
        let mut c = next[l];
        for (sym, &sl) in lens.iter().enumerate() {
            if sl as usize == l {
                plain[sym] = c;
                c += 1;
            }
        }
    }

    let mut code = vec![(0u32, 0u8); lens.len()];
    for (sym, &l) in lens.iter().enumerate() {
        let mask = if l == 32 { u32::MAX } else { (1u32 << l) - 1 };
        code[sym] = (mask ^ plain[sym], l);
    }

    let mut order = Vec::with_capacity(lens.len());
    let mut first = [0u32; 33];
    let mut last = [0u32; 33];
    let mut base = [0u32; 33];
    let mut used = [false; 33];
    for l in 1..=maxlen {
        if count[l] == 0 {
            continue;
        }
        // complementing reversed the order, so the largest code is first
        let mut syms: Vec<usize> = lens
            .iter()
            .enumerate()
            .filter(|&(_, &sl)| sl as usize == l)
            .map(|(s, _)| s)
            .collect();
        syms.sort_by_key(|&s| std::cmp::Reverse(code[s].0));
        used[l] = true;
        base[l] = order.len() as u32;
        first[l] = code[*syms.last().unwrap()].0;
        last[l] = code[syms[0]].0;
        order.extend(syms);
    }

    Codes {
        code,
        order,
        first,
        last,
        base,
        used,
    }
}

// ---------------------------------------------------------------------
// HUFF and CDIC records
// ---------------------------------------------------------------------

fn build_huff(codes: &Codes) -> Vec<u8> {
    // dict2: per code length, the raw mincode and the raw maxcode. The
    // decoder reads a symbol's dictionary index as `maxcode - code`, so the
    // stored maxcode is the length's largest code plus the dictionary index it
    // starts at.
    let mut min_raw = [0u32; 33];
    let mut max_raw = [0u32; 33];
    // Above every 32-bit window value, so an unused length short of the first
    // real one never stops the walk.
    let mut prev_window: u64 = 1u64 << 32;
    for l in 1..=32usize {
        if codes.used[l] {
            min_raw[l] = codes.first[l];
            max_raw[l] = codes.base[l] + codes.last[l];
            prev_window = (codes.first[l] as u64) << (32 - l);
        } else {
            // Repeat the previous length's window so the length is invisible
            // to the walk rather than a boundary in it.
            min_raw[l] = (prev_window >> (32 - l)) as u32;
            max_raw[l] = 0;
        }
    }

    // dict1: 256 entries keyed by the top 8 bits of the window. A code of 8
    // bits or fewer covers whole 8-bit prefixes, so each prefix either belongs
    // to exactly one such code (terminal) or lies entirely under codes longer
    // than 8 bits (non-terminal, and the walk resolves it).
    let mut owner = [0u8; 256];
    let mut long_min = [0u8; 256];
    for (sym, &(c, l)) in codes.code.iter().enumerate() {
        let _ = sym;
        let lo = (c as u64) << (32 - l as u32);
        let hi = ((c as u64 + 1) << (32 - l as u32)) - 1;
        if l <= 8 {
            for p in (lo >> 24)..=(hi >> 24) {
                owner[p as usize] = l;
            }
        } else {
            let p = (lo >> 24) as usize;
            if long_min[p] == 0 || l < long_min[p] {
                long_min[p] = l;
            }
        }
    }

    let off1: u32 = 0x18;
    let off2: u32 = off1 + 256 * 4;
    let mut rec = Vec::with_capacity(0x18 + 256 * 4 + 64 * 4);
    rec.extend_from_slice(b"HUFF\x00\x00\x00\x18");
    rec.extend_from_slice(&off1.to_be_bytes());
    rec.extend_from_slice(&off2.to_be_bytes());
    rec.resize(off1 as usize, 0);
    for p in 0..256usize {
        let v = if owner[p] != 0 {
            let l = owner[p];
            (max_raw[l as usize] << 8) | 0x80 | l as u32
        } else {
            let l = if long_min[p] != 0 { long_min[p] } else { 24 };
            l as u32
        };
        rec.extend_from_slice(&v.to_be_bytes());
    }
    for l in 1..=32usize {
        rec.extend_from_slice(&min_raw[l].to_be_bytes());
        rec.extend_from_slice(&max_raw[l].to_be_bytes());
    }
    rec
}

/// Build the CDIC records. `phrases[sym]` is `(bytes, already_expanded)`;
/// a phrase with the flag clear is stored as a compressed stream and expands
/// through the decoder itself, which is a path worth having a fixture for.
fn build_cdics(phrases: &[(Vec<u8>, bool)], order: &[usize], bits: u32) -> Vec<Vec<u8>> {
    let total = order.len() as u32;
    let per = 1usize << bits;
    let mut recs = Vec::new();
    let mut done = 0usize;
    while done < order.len() {
        let n = per.min(order.len() - done);
        let mut offsets = Vec::with_capacity(n);
        let mut body: Vec<u8> = Vec::new();
        for i in 0..n {
            let (data, expanded) = &phrases[order[done + i]];
            assert!(data.len() < 0x8000, "phrase too long for a CDIC slot");
            offsets.push((2 * n + body.len()) as u16);
            let flagged = data.len() as u16 | if *expanded { 0x8000 } else { 0 };
            body.extend_from_slice(&flagged.to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut rec = Vec::with_capacity(16 + 2 * n + body.len());
        rec.extend_from_slice(b"CDIC\x00\x00\x00\x10");
        rec.extend_from_slice(&total.to_be_bytes());
        rec.extend_from_slice(&bits.to_be_bytes());
        for o in offsets {
            rec.extend_from_slice(&o.to_be_bytes());
        }
        rec.extend_from_slice(&body);
        recs.push(rec);
        done += n;
    }
    recs
}

// ---------------------------------------------------------------------
// Bitstream
// ---------------------------------------------------------------------

struct BitWriter {
    acc: u64,
    nbits: u32,
    out: Vec<u8>,
}

impl BitWriter {
    fn new() -> Self {
        BitWriter {
            acc: 0,
            nbits: 0,
            out: Vec::new(),
        }
    }

    fn write(&mut self, code: u32, len: u8) {
        self.acc = (self.acc << len as u32) | code as u64;
        self.nbits += len as u32;
        while self.nbits >= 8 {
            self.nbits -= 8;
            self.out.push(((self.acc >> self.nbits) & 0xFF) as u8);
            self.acc &= (1u64 << self.nbits) - 1;
        }
    }

    /// Pad the final byte with the leading bits of a code longer than 7, so
    /// the decoder reads one code too long to fit in the bits that remain and
    /// stops instead of emitting a phantom symbol.
    fn finish(mut self, pad: (u32, u8)) -> Vec<u8> {
        if self.nbits > 0 {
            let need = 8 - self.nbits;
            let bits = (pad.0 >> (pad.1 as u32 - need)) & ((1u32 << need) - 1);
            self.write(bits, need as u8);
        }
        self.out
    }
}

fn encode(symbols: &[usize], codes: &Codes, pad_sym: usize) -> Vec<u8> {
    let mut bw = BitWriter::new();
    for &s in symbols {
        let (c, l) = codes.code[s];
        bw.write(c, l);
    }
    bw.finish(codes.code[pad_sym])
}

// ---------------------------------------------------------------------
// MOBI rewrite
// ---------------------------------------------------------------------

fn u16_be(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn u32_be(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

/// record 0 offsets holding a PalmDB record number as a u32.
///
/// 192 is deliberately absent: in MOBI6 that is a pair of u16s (first and
/// last content record), not one u32, and adding to it as a u32 would move
/// only the second of the two.
const RECORD_NUMBER_FIELDS: [usize; 18] = [
    40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 108, 112, 120, 200, 208, 224, 244,
];

fn trailing_data_len(record: &[u8], flags: u32) -> usize {
    let mut end = record.len();
    for bit in (1..16).rev() {
        if flags & (1 << bit) == 0 || end == 0 {
            continue;
        }
        let mut size = 0usize;
        for &b in &record[end.saturating_sub(4)..end] {
            if b & 0x80 != 0 {
                size = 0;
            }
            size = (size << 7) | (b & 0x7F) as usize;
        }
        end -= size.min(end);
    }
    if flags & 1 != 0 && end > 0 {
        end -= ((record[end - 1] & 3) as usize + 1).min(end);
    }
    record.len() - end
}

/// Where the HUFF and CDIC records go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// What kindlegen does: after the whole index block and immediately before
    /// the first image record, so `huff_rec_index == first_image_index -
    /// huff_rec_count` and the index records do not move. Read off libmobi's
    /// `sample-unicode-huffdic.mobi`, a real kindlegen 2.9 huffdic build
    /// (28 text records, HUFF at 32, images from 35).
    BeforeImages,
    /// Directly after the text records, which pushes the index block along.
    /// Only used to build the deliberately-broken fixture.
    AfterText,
}

/// Rewrite a PalmDOC MOBI as huffdic.
///
/// When `shift_index_pointer` is false the orth index record number is left
/// pointing where the index used to be. Combined with `Placement::AfterText`
/// that produces a dictionary whose index is intact and unreachable, which is
/// the failure issue #49 describes. It is a constructed instance of that
/// failure, not observed kindlegen output: kindlegen puts the compression
/// records past the index precisely so the index never moves.
fn to_huffdic(data: &[u8], placement: Placement, shift_index_pointer: bool) -> Vec<u8> {
    let num = u16_be(data, 76) as usize;
    let offsets: Vec<usize> = (0..num)
        .map(|i| u32_be(data, 78 + i * 8) as usize)
        .collect();
    let attrs: Vec<[u8; 4]> = (0..num)
        .map(|i| {
            let o = 78 + i * 8 + 4;
            [data[o], data[o + 1], data[o + 2], data[o + 3]]
        })
        .collect();
    let records: Vec<&[u8]> = (0..num)
        .map(|i| {
            let end = if i + 1 < num {
                offsets[i + 1]
            } else {
                data.len()
            };
            &data[offsets[i]..end]
        })
        .collect();

    let record0 = records[0];
    assert_eq!(u16_be(record0, 0), 2, "source must be PalmDOC compressed");
    let text_records = u16_be(record0, 8) as usize;
    let mobi_len = u32_be(record0, 20) as usize;
    let extra_flags = u32_be(record0, 240);

    // 1. take the text apart, keeping each record's trailing regions verbatim
    let mut plain = Vec::with_capacity(text_records);
    let mut trailers = Vec::with_capacity(text_records);
    for i in 1..=text_records {
        let raw = records[i];
        let t = trailing_data_len(raw, extra_flags);
        plain.push(palmdoc_decompress(&raw[..raw.len() - t]));
        trailers.push(raw[raw.len() - t..].to_vec());
    }

    // 2. the phrase dictionary: all 256 bytes, plus whichever short n-grams
    //    the text repeats most. Real kindlegen mines these far harder; this
    //    only has to produce a legal file with more than one CDIC record.
    let whole: Vec<u8> = plain.concat();
    let mut counts: std::collections::HashMap<&[u8], u32> = std::collections::HashMap::new();
    for n in [6usize, 4, 3] {
        let mut i = 0;
        while i + n < whole.len() {
            *counts.entry(&whole[i..i + n]).or_insert(0) += 1;
            i += 3;
        }
    }
    let mut ranked: Vec<(&[u8], u32)> = counts.into_iter().filter(|&(_, c)| c > 4).collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    ranked.truncate(400);

    let mut phrases: Vec<Vec<u8>> = (0..256u32).map(|b| vec![b as u8]).collect();
    phrases.extend(ranked.iter().map(|(g, _)| g.to_vec()));

    let mut by_bytes: std::collections::HashMap<&[u8], usize> = std::collections::HashMap::new();
    for (i, p) in phrases.iter().enumerate() {
        by_bytes.entry(p.as_slice()).or_insert(i);
    }
    let longest = phrases.iter().map(|p| p.len()).max().unwrap();

    let tokenize = |buf: &[u8]| -> Vec<usize> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < buf.len() {
            let mut hit = None;
            let mut l = longest.min(buf.len() - i);
            while l > 1 {
                if let Some(&sym) = by_bytes.get(&buf[i..i + l]) {
                    hit = Some((sym, l));
                    break;
                }
                l -= 1;
            }
            match hit {
                Some((sym, l)) => {
                    out.push(sym);
                    i += l;
                }
                None => {
                    out.push(buf[i] as usize);
                    i += 1;
                }
            }
        }
        out
    };

    let tokens: Vec<Vec<usize>> = plain.iter().map(|p| tokenize(p)).collect();
    let mut freqs = vec![0u64; phrases.len()];
    for t in &tokens {
        for &s in t {
            freqs[s] += 20;
        }
    }
    let lens = code_lengths(&freqs);
    let codes = assign_codes(&lens);
    let pad_sym = (0..lens.len()).max_by_key(|&s| lens[s]).unwrap();

    // Store every third multi-byte phrase as a compressed stream of
    // single-byte symbols, so the fixture exercises the decoder's nested
    // expansion path. Single-byte symbols are always stored literally, so a
    // packed phrase can never reference itself.
    let payloads: Vec<(Vec<u8>, bool)> = phrases
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i >= 256 && i % 3 == 0 {
                let syms: Vec<usize> = p.iter().map(|&b| b as usize).collect();
                (encode(&syms, &codes, pad_sym), false)
            } else {
                (p.clone(), true)
            }
        })
        .collect();

    let huff = build_huff(&codes);
    // 7 index bits so a few hundred phrases need several CDIC records, the
    // same as a real dictionary needs for its tens of thousands.
    let cdics = build_cdics(&payloads, &codes.order, 7);
    let streams: Vec<Vec<u8>> = tokens.iter().map(|t| encode(t, &codes, pad_sym)).collect();

    // 3. splice in the compression records
    let first_image = u32_be(record0, 108) as usize;
    let insert_at = match placement {
        Placement::BeforeImages if first_image > text_records && first_image < num => first_image,
        _ => 1 + text_records,
    };
    let inserted = 1 + cdics.len();
    let mut out_records: Vec<Vec<u8>> = Vec::with_capacity(num + inserted);
    out_records.push(record0.to_vec());
    for i in 0..text_records {
        let mut r = streams[i].clone();
        r.extend_from_slice(&trailers[i]);
        out_records.push(r);
    }
    for r in &records[1 + text_records..insert_at] {
        out_records.push(r.to_vec());
    }
    out_records.push(huff);
    out_records.extend(cdics);
    for r in &records[insert_at..] {
        out_records.push(r.to_vec());
    }
    let mut out_attrs: Vec<[u8; 4]> = attrs[..insert_at].to_vec();
    out_attrs.extend(std::iter::repeat_n(attrs[0], inserted));
    out_attrs.extend_from_slice(&attrs[insert_at..]);

    // 4. every record number past the insertion point moves along with it
    let r0 = &mut out_records[0];
    r0[0..2].copy_from_slice(&crate_compression().to_be_bytes());
    for &off in RECORD_NUMBER_FIELDS.iter() {
        if off + 4 > 16 + mobi_len {
            continue;
        }
        // The orth index pointer is what the stale fixture leaves behind.
        if off == 40 && !shift_index_pointer {
            continue;
        }
        let v = u32_be(r0, off);
        if v != u32::MAX && v != 0 && v as usize >= insert_at {
            r0[off..off + 4].copy_from_slice(&(v + inserted as u32).to_be_bytes());
        }
    }
    // first/last content record, a pair of u16s at 192 and 194.
    for off in [192usize, 194] {
        let v = u16_be(r0, off) as usize;
        if v != 0 && v >= insert_at {
            r0[off..off + 2].copy_from_slice(&((v + inserted) as u16).to_be_bytes());
        }
    }
    r0[112..116].copy_from_slice(&(insert_at as u32).to_be_bytes());
    r0[116..120].copy_from_slice(&(inserted as u32).to_be_bytes());

    // 5. rebuild the PalmDB header and record list
    let n = out_records.len();
    let mut out = data[..78].to_vec();
    out[76..78].copy_from_slice(&(n as u16).to_be_bytes());
    let mut body_start = 78 + n * 8;
    body_start += (4 - body_start % 4) % 4;
    let mut table = Vec::with_capacity(n * 8);
    let mut body = Vec::new();
    for i in 0..n {
        table.extend_from_slice(&((body_start + body.len()) as u32).to_be_bytes());
        table.extend_from_slice(&out_attrs[i]);
        body.extend_from_slice(&out_records[i]);
    }
    out.extend_from_slice(&table);
    out.resize(body_start, 0);
    out.extend_from_slice(&body);
    out
}

fn crate_compression() -> u16 {
    kindling::huffcdic::COMPRESSION_HUFFDIC
}

fn main() {
    let root = repo_root();
    let dir = root.join("tests/fixtures/huffdic");
    std::fs::create_dir_all(&dir).unwrap();

    // The English dictionary is one text record; the Japanese one is two, and
    // its labels go through a generated ORDT table, so between them the pair
    // covers decoding across records (phrases the first record memoized are
    // reused by the second) and both label encodings.
    for (source, name, placement, shift) in [
        (
            "tests/fixtures/langs/en/en-kindlegen.mobi",
            "en_huffdic.mobi",
            Placement::BeforeImages,
            true,
        ),
        (
            "tests/fixtures/langs/en/en-kindlegen.mobi",
            "en_huffdic_stale_index.mobi",
            Placement::AfterText,
            false,
        ),
        (
            "tests/fixtures/langs/ja/ja-kindlegen.mobi",
            "ja_huffdic.mobi",
            Placement::BeforeImages,
            true,
        ),
    ] {
        let path = root.join(source);
        let data =
            std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let out = to_huffdic(&data, placement, shift);
        let out_path = dir.join(name);
        std::fs::write(&out_path, &out).unwrap();
        println!("wrote {} ({} bytes)", out_path.display(), out.len());
    }
}
