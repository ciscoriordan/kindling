//! HUFF/CDIC ("huffdic") *compression*, PalmDOC compression type 17480.
//!
//! [`crate::huffcdic`] reads this format. This writes it, which nothing
//! outside kindlegen appears to do: calibre, KindleUnpack and libmobi all
//! decode huffdic and none of them encodes it (issue #49).
//!
//! Why bother, when PalmDOC LZ77 already works: a dictionary is the one book
//! shape where the same phrases recur across hundreds of thousands of
//! entries, and PalmDOC can only reach back 2047 bytes for a match. A static
//! Huffman code over a shared phrase dictionary reaches the whole book, which
//! is why every Amazon-published dictionary is built this way.
//!
//! # The format, as the decoder reads it back
//!
//! There are no literal-byte codes. Every symbol is an index into the phrase
//! dictionary, so the single bytes a text uses are themselves one-byte
//! phrases. A code's symbol index is `maxcode[len] - code`, which makes this
//! a canonical Huffman code that runs *backwards*: within one code length,
//! the lowest symbol index takes the highest code, and the shortest lengths
//! take the highest codes overall. Reading a real kindlegen file back:
//!
//! ```text
//!   len  3: codes    7..7    ->  index   0
//!   len  4: codes    9..13   ->  index   1..5
//!   len  5: codes   13..17   ->  index   6..10
//!   ...
//!   len 11: codes   16..33   ->  index  66..83
//!   len 15: codes    8..255  ->  index  84..331
//!   len 16: codes    0..15   ->  index 332..347
//! ```
//!
//! The chain is `highest_code(L+1) = lowest_code(L) * 2 - 1`, and a length
//! with no symbols still consumes its step: lengths 12, 13 and 14 above are
//! empty, and each stores `lowest * 2` so that its *shifted* `mincode` equals
//! the previous length's. That equality is what lets the decoder's
//! walk-the-length-up loop pass through an empty length without stopping in
//! it.
//!
//! `maxcode` is not stored as a code bound. It is stored as
//! `lowest_index_at_this_length + highest_code_at_this_length`, so that
//! subtracting the code yields the index directly.

use std::collections::HashMap;

use crate::huffcdic::Huffdic;

/// Longest code the format can express: `mincode`/`maxcode` have 32 slots.
const MAX_CODE_LEN: usize = 32;

/// Ceiling on the phrase dictionary. Each phrase costs a code, and the win
/// from a rarer phrase falls off long before this; kindlegen's own files sit
/// in the hundreds to low thousands.
const MAX_PHRASES: usize = 4096;

/// Longest multi-byte phrase considered. Past this the frequency of an exact
/// repeat collapses and the dictionary fills with near-duplicates.
const MAX_PHRASE_LEN: usize = 16;

/// Target size for one CDIC record. Phrase offsets inside a CDIC are 16-bit
/// and relative to offset 16, so a record can hold at most 64 KB whatever
/// happens; this keeps them near the size kindlegen writes.
const CDIC_TARGET_BYTES: usize = 4096;

/// A finished huffdic encoding: the model records and the compressed text.
pub(crate) struct Encoded {
    /// The single `HUFF` record.
    pub huff: Vec<u8>,
    /// The `CDIC` records, in order.
    pub cdics: Vec<Vec<u8>>,
    /// One compressed record per input chunk.
    pub records: Vec<Vec<u8>>,
}

/// Compress `chunks` into huffdic records, or return `None` to say the caller
/// should keep PalmDOC.
///
/// `None` is returned whenever the result would not be an improvement or the
/// encoder cannot express the text: a code longer than 32 bits, a text too
/// small for a phrase dictionary to pay for itself, or output no smaller than
/// what PalmDOC already produced. Callers treat that as "use PalmDOC", so a
/// refusal here is never a build failure.
///
/// Every record is decoded back through [`crate::huffcdic`] before this
/// returns, and a mismatch also produces `None`. That check is the reason
/// this can be shipped without a device in hand: a dictionary that would not
/// round trip never reaches a file.
pub(crate) fn encode(chunks: &[Vec<u8>]) -> Option<Encoded> {
    let total: usize = chunks.iter().map(|c| c.len()).sum();
    // Below this the phrase dictionary and its tables cost more than the
    // codes save. The HUFF record alone is 1.3 KB.
    if total < 64 * 1024 {
        return None;
    }

    let phrases = select_phrases(chunks, total)?;
    let matcher = PhraseMatcher::new(&phrases);

    // Parse every chunk once to count how often each phrase is used. The
    // parse has to be the same one the second pass uses, or the code lengths
    // will not match the stream they are built for.
    let mut freq = vec![0u64; phrases.len()];
    let parsed: Vec<Vec<u32>> = chunks
        .iter()
        .map(|chunk| {
            let symbols = matcher.parse(chunk);
            for &s in &symbols {
                freq[s as usize] += 1;
            }
            symbols
        })
        .collect();

    let lengths = match code_lengths(&freq) {
        Some(l) => l,
        None => {
            if std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok() {
                eprintln!("huffdic: declined, no code fits in 32 bits");
            }
            return None;
        }
    };
    let table = match CodeTable::build(&lengths) {
        Some(t) => t,
        None => {
            if std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok() {
                eprintln!("huffdic: declined, code table could not be laid out");
            }
            return None;
        }
    };

    // The phrase dictionary is emitted in symbol-index order, which the code
    // assignment fixed. Reorder both the phrases and the parsed streams.
    let phrases: Vec<&[u8]> = table
        .symbol_of_index
        .iter()
        .map(|&s| phrases[s as usize].as_slice())
        .collect();

    let records: Vec<Vec<u8>> = parsed
        .iter()
        .map(|symbols| table.encode_stream(symbols))
        .collect();

    let huff = table.huff_record();
    let cdics = cdic_records(&phrases);

    // Refuse to make things worse. The model records ride in the file too, so
    // they count against the win.
    let compressed: usize = records.iter().map(|r| r.len()).sum::<usize>()
        + huff.len()
        + cdics.iter().map(|c| c.len()).sum::<usize>();
    if compressed >= total {
        if std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok() {
            eprintln!("huffdic: declined on size, {compressed} vs {total} raw");
        }
        return None;
    }

    let encoded = Encoded {
        huff,
        cdics,
        records,
    };
    if verify_round_trip(&encoded, chunks).is_none() {
        if std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok() {
            eprintln!("huffdic: declined, round trip failed");
        }
        return None;
    }
    if std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok() {
        eprintln!("huffdic: {compressed} bytes from {total} raw");
    }
    Some(encoded)
}

/// Decode everything just written and compare it with what went in.
///
/// This runs on every build, not only in tests. The cost is one extra
/// decompression pass over the text; the alternative is shipping a
/// dictionary whose every entry is unreadable, which no self-check downstream
/// would catch because the records are opaque bytes to all of them.
fn verify_round_trip(encoded: &Encoded, chunks: &[Vec<u8>]) -> Option<()> {
    let mut model_records: Vec<&[u8]> = Vec::with_capacity(1 + encoded.cdics.len());
    model_records.push(&encoded.huff);
    for c in &encoded.cdics {
        model_records.push(c);
    }
    // Record 0 of this slice IS the HUFF record, so the index is 0 and the
    // count covers it plus every CDIC.
    let model = Huffdic::from_records(&model_records, 0, 0, model_records.len() as u32).ok()?;
    for (i, chunk) in chunks.iter().enumerate() {
        let back = model.decompress(&encoded.records[i]).ok()?;
        if back != *chunk {
            return None;
        }
    }
    Some(())
}

// ---------------------------------------------------------------------------
// Phrase dictionary
// ---------------------------------------------------------------------------

/// Choose the phrases the code will be built over.
///
/// Every byte the text actually uses becomes a one-byte phrase, because there
/// is no other way to express a byte: without them a text with an unexpected
/// character could not be encoded at all. On top of those go the repeated
/// substrings that save the most, scored by `occurrences * (len - 1)`, which
/// is the bytes a phrase removes from the stream.
fn select_phrases(chunks: &[Vec<u8>], total: usize) -> Option<Vec<Vec<u8>>> {
    let mut singles = [false; 256];
    for chunk in chunks {
        for &b in chunk {
            singles[b as usize] = true;
        }
    }
    let mut phrases: Vec<Vec<u8>> = (0..256usize)
        .filter(|&b| singles[b])
        .map(|b| vec![b as u8])
        .collect();
    if phrases.is_empty() {
        return None;
    }

    // Count substrings on a sample rather than the whole text: a 200 MB
    // dictionary would otherwise build a hash map of hundreds of millions of
    // entries to answer a question a few megabytes answers just as well.
    const SAMPLE_BYTES: usize = 4 * 1024 * 1024;
    let stride = (total / SAMPLE_BYTES).max(1);

    let mut counts: HashMap<&[u8], u32> = HashMap::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i % stride != 0 {
            continue;
        }
        for start in 0..chunk.len() {
            let max_len = MAX_PHRASE_LEN.min(chunk.len() - start);
            for len in 2..=max_len {
                let s = &chunk[start..start + len];
                *counts.entry(s).or_insert(0) += 1;
            }
        }
    }

    let mut scored: Vec<(u64, &[u8])> = counts
        .into_iter()
        .filter(|(s, c)| *c >= 4 && s.len() >= 2)
        .map(|(s, c)| (c as u64 * (s.len() as u64 - 1), s))
        .collect();
    // Longest first within an equal score, so the greedy parse has the
    // bigger win available when both fit.
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.len().cmp(&a.1.len())));

    let room = MAX_PHRASES.saturating_sub(phrases.len());
    for (_, s) in scored.into_iter().take(room) {
        phrases.push(s.to_vec());
    }
    Some(phrases)
}

/// Greedy longest-match lookup over the phrase set.
struct PhraseMatcher {
    /// Phrases that start with a given byte, longest first, as
    /// `(bytes, symbol)`.
    by_first: Vec<Vec<(Vec<u8>, u32)>>,
    /// The one-byte phrase for each byte value, which always exists for a
    /// byte the text contains and is the fallback when nothing longer fits.
    single: [Option<u32>; 256],
}

impl PhraseMatcher {
    fn new(phrases: &[Vec<u8>]) -> Self {
        let mut by_first: Vec<Vec<(Vec<u8>, u32)>> = vec![Vec::new(); 256];
        let mut single = [None; 256];
        for (i, p) in phrases.iter().enumerate() {
            let first = p[0] as usize;
            if p.len() == 1 {
                single[first] = Some(i as u32);
            } else {
                by_first[first].push((p.clone(), i as u32));
            }
        }
        for list in &mut by_first {
            list.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        }
        PhraseMatcher { by_first, single }
    }

    /// Split one chunk into symbols, taking the longest phrase that matches
    /// at each position.
    ///
    /// Greedy rather than optimal. An optimal parse would need the code
    /// lengths, which are not known until after the parse has produced the
    /// frequencies, so it would take another round.
    fn parse(&self, data: &[u8]) -> Vec<u32> {
        let mut out = Vec::with_capacity(data.len() / 2);
        let mut at = 0usize;
        while at < data.len() {
            let first = data[at] as usize;
            let mut taken = None;
            for (bytes, symbol) in &self.by_first[first] {
                if data[at..].starts_with(bytes) {
                    taken = Some((bytes.len(), *symbol));
                    break;
                }
            }
            let (len, symbol) = match taken {
                Some(t) => t,
                // Guaranteed present: every byte in the text got a one-byte
                // phrase in select_phrases.
                None => (1, self.single[first].expect("one-byte phrase")),
            };
            out.push(symbol);
            at += len;
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Huffman code lengths
// ---------------------------------------------------------------------------

/// Code length per symbol, or `None` when no code within 32 bits exists.
///
/// A symbol that never occurs still needs a length, because the tables cover
/// every index; unused symbols are given the longest length, where they cost
/// nothing.
fn code_lengths(freq: &[u64]) -> Option<Vec<u8>> {
    // Smooth the counts so a very skewed text cannot push a code past 32
    // bits, retrying with progressively flatter weights. Flattening costs a
    // fraction of a bit per symbol and is far cheaper than giving up.
    for smoothing in [0u64, 1, 4, 16, 64] {
        let weights: Vec<u64> = freq.iter().map(|&f| f + smoothing + 1).collect();
        let lengths = huffman(&weights);
        if lengths
            .iter()
            .all(|&l| (1..=MAX_CODE_LEN as u8).contains(&l))
        {
            return Some(lengths);
        }
    }
    None
}

/// Textbook Huffman over a min-heap, returning only the code lengths.
fn huffman(weights: &[u64]) -> Vec<u8> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let n = weights.len();
    if n == 1 {
        return vec![1];
    }
    // Nodes 0..n are leaves; the rest are internal.
    let mut parent = vec![usize::MAX; 2 * n];
    let mut heap: BinaryHeap<Reverse<(u64, usize)>> = weights
        .iter()
        .enumerate()
        .map(|(i, &w)| Reverse((w, i)))
        .collect();
    let mut next = n;
    while heap.len() > 1 {
        let Reverse((wa, a)) = heap.pop().unwrap();
        let Reverse((wb, b)) = heap.pop().unwrap();
        parent[a] = next;
        parent[b] = next;
        heap.push(Reverse((wa + wb, next)));
        next += 1;
    }
    (0..n)
        .map(|i| {
            let mut depth = 0u32;
            let mut at = i;
            while parent[at] != usize::MAX {
                at = parent[at];
                depth += 1;
            }
            depth.min(255) as u8
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Code assignment and the HUFF record
// ---------------------------------------------------------------------------

/// The assigned code table, plus the symbol order it implies.
struct CodeTable {
    /// `(code, length)` per original symbol.
    codes: Vec<(u32, u8)>,
    /// Original symbol at each phrase-dictionary index.
    symbol_of_index: Vec<u32>,
    /// Stored `mincode` per length, index 1..=32.
    mincode: [u32; MAX_CODE_LEN + 1],
    /// Stored `maxcode` per length: index base plus highest code.
    maxcode: [u32; MAX_CODE_LEN + 1],
    /// Whether any symbol has this length.
    used: [bool; MAX_CODE_LEN + 1],
}

impl CodeTable {
    /// Lay the codes out descending-canonically, as the decoder reads them.
    fn build(lengths: &[u8]) -> Option<CodeTable> {
        // Symbols in index order: by length, then by original symbol so the
        // layout is deterministic.
        let mut order: Vec<u32> = (0..lengths.len() as u32).collect();
        order.sort_by_key(|&s| (lengths[s as usize], s));

        let mut counts = [0usize; MAX_CODE_LEN + 1];
        for &l in lengths {
            counts[l as usize] += 1;
        }

        let mut codes = vec![(0u32, 0u8); lengths.len()];
        let mut mincode = [0u32; MAX_CODE_LEN + 1];
        let mut maxcode = [0u32; MAX_CODE_LEN + 1];
        let mut used = [false; MAX_CODE_LEN + 1];

        let mut index = 0usize;
        let mut cursor = 0usize;
        // Highest code available at the current length. The first used length
        // may start anywhere, so seed it from that length's full range.
        let mut prev_lowest: Option<u32> = None;
        for len in 1..=MAX_CODE_LEN {
            // Highest code still available at this length. Every length
            // consumes its doubling step whether or not it holds symbols,
            // and once the lowest code reaches 0 the space is spent: there is
            // nothing below it for a longer code to occupy.
            let top: i64 = match prev_lowest {
                None => (1i64 << len) - 1,
                Some(low) => (low as i64) * 2 - 1,
            };
            if top > u32::MAX as i64 {
                return None;
            }
            let count = counts[len];
            if top < 0 {
                if count > 0 {
                    return None;
                }
                mincode[len] = 0;
                maxcode[len] = 0;
                prev_lowest = Some(0);
                continue;
            }
            let top = top as u32;
            if count == 0 {
                // Store the boundary so the shifted mincode is unchanged from
                // the previous length, which is what lets the decoder's walk
                // step over this length rather than stopping in it.
                let low = top as u64 + 1;
                if low > u32::MAX as u64 {
                    return None;
                }
                mincode[len] = low as u32;
                maxcode[len] = 0;
                prev_lowest = Some(low as u32);
                continue;
            }
            if (count as u64) > top as u64 + 1 {
                // More symbols than codes left at this length.
                return None;
            }
            let lowest = top - (count as u32 - 1);
            used[len] = true;
            mincode[len] = lowest;
            maxcode[len] = index as u32 + top;

            // Lowest index takes the highest code.
            for k in 0..count {
                let symbol = order[cursor + k];
                let code = top - k as u32;
                codes[symbol as usize] = (code, len as u8);
            }
            cursor += count;
            index += count;
            prev_lowest = Some(lowest);
        }
        if cursor != lengths.len() {
            return None;
        }

        // Padding at the end of a record is zero bits, and the decoder reads
        // them as the start of a code. That is only safe when the all-zeros
        // prefix belongs to a code longer than the seven bits a partial byte
        // can leave behind, otherwise the padding decodes as a real symbol
        // and the record gains a spurious phrase.
        let longest = (1..=MAX_CODE_LEN).filter(|&l| used[l]).next_back()?;
        if longest < 8 || mincode[longest] != 0 {
            return None;
        }

        Some(CodeTable {
            codes,
            symbol_of_index: order,
            mincode,
            maxcode,
            used,
        })
    }

    /// Write the bitstream for one record's symbols, zero-padded to a byte.
    fn encode_stream(&self, symbols: &[u32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(symbols.len());
        let mut acc: u64 = 0;
        let mut bits: u32 = 0;
        for &s in symbols {
            let (code, len) = self.codes[s as usize];
            acc = (acc << len as u32) | code as u64;
            bits += len as u32;
            while bits >= 8 {
                bits -= 8;
                out.push(((acc >> bits) & 0xFF) as u8);
            }
        }
        if bits > 0 {
            out.push(((acc << (8 - bits)) & 0xFF) as u8);
        }
        out
    }

    /// Build the `HUFF` record: the 256-entry prefix table then the 32
    /// `(mincode, maxcode)` pairs.
    fn huff_record(&self) -> Vec<u8> {
        const HEADER_LEN: u32 = 24;
        let off1 = HEADER_LEN;
        let off2 = off1 + 256 * 4;

        let mut rec = Vec::with_capacity(off2 as usize + 32 * 8);
        rec.extend_from_slice(b"HUFF");
        rec.extend_from_slice(&HEADER_LEN.to_be_bytes());
        rec.extend_from_slice(&off1.to_be_bytes());
        rec.extend_from_slice(&off2.to_be_bytes());
        // Two reserved words kindlegen leaves zero.
        rec.extend_from_slice(&0u32.to_be_bytes());
        rec.extend_from_slice(&0u32.to_be_bytes());

        for prefix in 0..256u32 {
            // A code of eight bits or fewer is fully determined by these
            // eight bits, so the entry answers outright. Anything longer only
            // gets a floor to start the length walk from.
            let mut entry: u32 = 0;
            for len in 1..=8usize {
                if !self.used[len] {
                    continue;
                }
                let candidate = prefix >> (8 - len);
                if candidate >= self.mincode[len] && candidate <= self.top_code(len) {
                    entry = (self.maxcode[len] << 8) | 0x80 | len as u32;
                    break;
                }
            }
            if entry == 0 {
                // Not resolvable from eight bits, so the entry carries a
                // floor for the decoder's walk instead. The walk steps the
                // length up while `code < mincode[len]`, and `mincode` is
                // non-increasing once shifted, so a LARGER code resolves at a
                // SHORTER length. The floor therefore has to come from the
                // largest code this prefix can carry, not the smallest:
                // starting from the smallest would begin the walk past the
                // right length and stop there, decoding the wrong symbol.
                let largest = (((prefix as u64) + 1) << 24) - 1;
                let mut floor = MAX_CODE_LEN;
                for len in 9..=MAX_CODE_LEN {
                    if !self.used[len] {
                        continue;
                    }
                    if largest >= (self.mincode[len] as u64) << (32 - len) {
                        floor = len;
                        break;
                    }
                }
                entry = floor as u32;
            }
            rec.extend_from_slice(&entry.to_be_bytes());
        }

        for len in 1..=MAX_CODE_LEN {
            rec.extend_from_slice(&self.mincode[len].to_be_bytes());
            rec.extend_from_slice(&self.maxcode[len].to_be_bytes());
        }
        rec
    }

    /// Highest code in use at `len`.
    fn top_code(&self, len: usize) -> u32 {
        // maxcode is index base + top code, and the base is the index of the
        // symbol holding the top code, so the difference recovers it.
        let count = self
            .codes
            .iter()
            .filter(|(_, l)| *l as usize == len)
            .count() as u32;
        self.mincode[len] + count - 1
    }
}

/// Split the phrase dictionary into `CDIC` records.
///
/// Every record declares the same index-bit width and holds exactly
/// `1 << bits` phrases except the last, which is what the decoder assumes
/// when it works out how many to read from each.
fn cdic_records(phrases: &[&[u8]]) -> Vec<Vec<u8>> {
    let total: usize = phrases.iter().map(|p| p.len() + 2).sum();
    let average = (total / phrases.len().max(1)).max(1);
    // Largest power of two whose phrases fit the target record size, kept
    // inside the 16-bit offsets a CDIC uses.
    let mut bits = 0u32;
    while bits < 16
        && (1usize << (bits + 1)) * average + 16 + (1 << (bits + 1)) * 2 <= CDIC_TARGET_BYTES
    {
        bits += 1;
    }
    let per_record = 1usize << bits;

    let mut out = Vec::new();
    for group in phrases.chunks(per_record) {
        let mut rec = Vec::with_capacity(CDIC_TARGET_BYTES);
        rec.extend_from_slice(b"CDIC");
        rec.extend_from_slice(&16u32.to_be_bytes());
        rec.extend_from_slice(&(phrases.len() as u32).to_be_bytes());
        rec.extend_from_slice(&bits.to_be_bytes());

        // Offsets are relative to byte 16 and follow the header, so the data
        // starts after the whole offset table.
        let mut offsets: Vec<u16> = Vec::with_capacity(group.len());
        let mut data: Vec<u8> = Vec::new();
        let table_bytes = group.len() * 2;
        for p in group {
            offsets.push((table_bytes + data.len()) as u16);
            // 0x8000 marks the phrase as stored literally. Without it the
            // decoder would treat these bytes as a nested bitstream.
            data.extend_from_slice(&((p.len() as u16) | 0x8000).to_be_bytes());
            data.extend_from_slice(p);
        }
        for o in offsets {
            rec.extend_from_slice(&o.to_be_bytes());
        }
        rec.extend_from_slice(&data);
        out.push(rec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text with enough repetition to look like a dictionary, and enough of
    /// it to clear the size floor.
    fn dictionary_like() -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        for i in 0..600 {
            let body = format!(
                "<idx:entry><idx:orth value=\"word{i:05}\"><b>word{i:05}</b></idx:orth>\
                 <p>A definition of the {i}th word, with the ordinary filler a \
                 dictionary entry carries so the phrases repeat.</p></idx:entry>\
                 <hr/><mbp:pagebreak/>"
            );
            chunks.push(body.into_bytes());
        }
        chunks
    }

    #[test]
    fn round_trips_through_the_decoder() {
        let chunks = dictionary_like();
        let encoded = encode(&chunks).expect("dictionary-like text should compress");

        let mut model: Vec<&[u8]> = vec![&encoded.huff];
        for c in &encoded.cdics {
            model.push(c);
        }
        let huffdic = Huffdic::from_records(&model, 0, 0, model.len() as u32)
            .expect("our own tables must load");
        for (i, chunk) in chunks.iter().enumerate() {
            let back = huffdic
                .decompress(&encoded.records[i])
                .unwrap_or_else(|e| panic!("record {i} failed to decode: {e}"));
            assert_eq!(back, *chunk, "record {i} did not round trip");
        }
    }

    #[test]
    fn beats_the_text_it_compressed() {
        let chunks = dictionary_like();
        let raw: usize = chunks.iter().map(|c| c.len()).sum();
        let encoded = encode(&chunks).expect("should compress");
        let total: usize = encoded.records.iter().map(|r| r.len()).sum::<usize>()
            + encoded.huff.len()
            + encoded.cdics.iter().map(|c| c.len()).sum::<usize>();
        assert!(
            total < raw,
            "huffdic made it bigger: {total} bytes from {raw}"
        );
    }

    /// A dictionary whose entries differ from one another, which is what a
    /// real one looks like. `dictionary_like` above is deliberately the
    /// opposite case: its entries are near-duplicates of their neighbours, so
    /// PalmDOC's 2047-byte back-reference window handles them very well and
    /// huffdic loses. Both cases are real, which is why the builder tries
    /// both and keeps the smaller.
    fn varied_dictionary() -> Vec<Vec<u8>> {
        // A small deterministic PRNG, so the corpus is fixed across runs.
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let vocab: Vec<&str> = "the of and a to in is was that for on with as by at from \
             an be this which have or had not but were noun verb adjective adverb plural \
             archaic figurative colloquial transitive intransitive sense usage compare"
            .split_whitespace()
            .collect();
        let mut chunks = Vec::new();
        for i in 0..4000u32 {
            let head = format!("hw{:05}", i);
            let n = 15 + (next() % 45) as usize;
            let body: Vec<&str> = (0..n)
                .map(|_| vocab[(next() % vocab.len() as u64) as usize])
                .collect();
            chunks.push(
                format!(
                    "<idx:entry><idx:orth value=\"{head}\"><b>{head}</b></idx:orth>\
                     <p><i>noun</i> {}.</p></idx:entry><hr/><mbp:pagebreak/>",
                    body.join(" ")
                )
                .into_bytes(),
            );
        }
        chunks
    }

    /// The point of the format: a dictionary's phrases recur across the whole
    /// book and PalmDOC can only reach back 2047 bytes.
    #[test]
    fn beats_palmdoc_on_a_varied_dictionary() {
        let chunks = varied_dictionary();
        let raw: usize = chunks.iter().map(|c| c.len()).sum();
        let palmdoc: usize = chunks
            .iter()
            .map(|c| crate::palmdoc::compress(c).len())
            .sum();

        let encoded = encode(&chunks).expect("varied dictionary text should compress");
        let huffdic: usize = encoded.records.iter().map(|r| r.len()).sum::<usize>()
            + encoded.huff.len()
            + encoded.cdics.iter().map(|c| c.len()).sum::<usize>();

        println!(
            "  raw {raw}  palmdoc {palmdoc} ({:.1}%)  huffdic {huffdic} ({:.1}%)",
            palmdoc as f64 * 100.0 / raw as f64,
            huffdic as f64 * 100.0 / raw as f64
        );
        assert!(
            huffdic < palmdoc,
            "huffdic {huffdic} should beat palmdoc {palmdoc} on varied dictionary text"
        );
    }

    #[test]
    fn round_trips_a_varied_dictionary_too() {
        let chunks = varied_dictionary();
        let encoded = encode(&chunks).unwrap();
        let mut model: Vec<&[u8]> = vec![&encoded.huff];
        for c in &encoded.cdics {
            model.push(c);
        }
        let huffdic = Huffdic::from_records(&model, 0, 0, model.len() as u32).unwrap();
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(
                huffdic.decompress(&encoded.records[i]).unwrap(),
                *chunk,
                "record {i} did not round trip"
            );
        }
    }

    #[test]
    fn declines_rather_than_shipping_something_worse() {
        // Too small for the tables to pay for themselves.
        let tiny = vec![b"hello".to_vec()];
        assert!(encode(&tiny).is_none());

        // Incompressible: random bytes have no repeated phrases, so nothing
        // beats storing them.
        let mut state = 0x2545F4914F6CDD1Du64;
        let noise: Vec<u8> = (0..200_000)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect();
        assert!(
            encode(&[noise]).is_none(),
            "random bytes must fall back to PalmDOC rather than grow"
        );
    }

    #[test]
    fn the_tables_match_the_shape_a_kindlegen_file_uses() {
        let chunks = dictionary_like();
        let encoded = encode(&chunks).unwrap();
        assert_eq!(&encoded.huff[..4], b"HUFF");
        assert_eq!(
            u32::from_be_bytes(encoded.huff[4..8].try_into().unwrap()),
            24
        );
        // 256 prefix entries plus 32 bound pairs, after a 24-byte header.
        assert_eq!(encoded.huff.len(), 24 + 256 * 4 + 32 * 8);
        assert!(!encoded.cdics.is_empty());
        for c in &encoded.cdics {
            assert_eq!(&c[..4], b"CDIC");
            assert_eq!(u32::from_be_bytes(c[4..8].try_into().unwrap()), 16);
            // Every record declares the whole dictionary's phrase count.
            let declared = u32::from_be_bytes(c[8..12].try_into().unwrap());
            assert!(declared > 0);
            let bits = u32::from_be_bytes(c[12..16].try_into().unwrap());
            assert!(bits <= 16, "index bits must fit the format");
        }
    }

    #[test]
    fn every_prefix_entry_is_legal() {
        // The decoder rejects a zero code length outright, and rejects a
        // length of eight or fewer that is not marked terminal.
        let chunks = dictionary_like();
        let encoded = encode(&chunks).unwrap();
        let off1 = 24usize;
        for i in 0..256 {
            let v = u32::from_be_bytes(
                encoded.huff[off1 + i * 4..off1 + i * 4 + 4]
                    .try_into()
                    .unwrap(),
            );
            let codelen = v & 0x1F;
            let term = v & 0x80 != 0;
            assert!(codelen > 0, "prefix {i} has code length 0");
            assert!(
                codelen > 8 || term,
                "prefix {i} has length {codelen} but is not terminal"
            );
        }
    }
}
