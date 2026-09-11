//! PalmDOC LZ77 compression.
//!
//! The PalmDOC compression is an LZ77 variant used in MOBI/PRC files.
//! Uses hash chain matching with the following constraints:
//! - Max distance: 2047
//! - Max match length: 10
//! - Min match length: 3
//!
//! # Why the parse is a shortest path and not longest-match-wins
//!
//! Greedy LZ77 takes the longest match it can find at every position. That is
//! the right default when a match always beats a literal, and in PalmDOC it
//! does not. The format has four encodings and their prices do not line up
//! with length:
//!
//! | encoding | output bytes | source bytes |
//! |---|---|---|
//! | a byte in `0x00`, or `0x09..=0x7F` | 1 | 1 |
//! | a space followed by `0x40..=0x7F` | 1 | 2 |
//! | a run of `n` bytes needing escape | 1 + n | n |
//! | a match | 2 | 3 to 10 |
//!
//! So a three-byte match saves one byte and a ten-byte match saves eight, at
//! the same price. Taking a three-byte match can step over a ten-byte match
//! that started one position later, and greedy does exactly that. A space and
//! the letter after it also cost one byte for the pair, which beats a
//! three-byte match outright and ties a four-byte one, so a match is not even
//! reliably better than the literals it replaces.
//!
//! Choosing the cheapest path through those edges instead is worth about 2%
//! of the compressed text. On 819200 bytes of Webster's Unabridged, greedy
//! spent 399022 bytes and the shortest path spends 390310; a whole build of
//! that dictionary goes from 15671732 bytes to 15371899.
//!
//! Greedy was not doing badly, which is the reason to say where the 2% comes
//! from. calibre's `compress_doc`, a mature C implementation of the same
//! format, spends 398988 on those same bytes: a tie with greedy to within a
//! rounding error, and 2.2% behind this. The gain is in the parse, not in
//! finding more matches.
//!
//! Nothing about the output format changes. Any PalmDOC reader decodes this
//! to the same bytes it decoded the greedy output to; only the choice of
//! which legal encoding to use at each position is different.

const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;
const MAX_CHAIN: usize = 64;
const MAX_DIST: usize = 2047;
const MAX_MATCH: usize = 10;
const MIN_MATCH: usize = 3;

/// What the encoder chose to do at one position, and where that lands next.
#[derive(Clone, Copy)]
enum Step {
    /// One byte, written as itself.
    Raw,
    /// A space and the printable byte after it, in one output byte.
    SpacePair,
    /// `n` bytes that need escaping, behind a one-byte count.
    Run(u8),
    /// A back reference: length, then distance.
    Match(u8, u16),
}

#[inline]
fn hash3(a: u8, b: u8, c: u8) -> usize {
    (((a as usize) << 10) ^ ((b as usize) << 5) ^ (c as usize)) & HASH_MASK
}

/// True for a byte that cannot be written as itself, because the decoder
/// reads it as a length count or as the first half of a two-byte token.
#[inline]
fn needs_escape(b: u8) -> bool {
    (0x01..=0x08).contains(&b) || b >= 0x80
}

/// The longest match at every position, with the distance it was found at.
///
/// Greedy only needs this at the positions it lands on, and skips the rest.
/// A shortest path needs every position, because the cheapest route through
/// the record may well enter one of the positions greedy stepped over.
///
/// A shorter match is always available wherever a longer one is: a prefix of
/// a match at distance `d` still matches at distance `d`, overlapping runs
/// included. So the length recorded here is a ceiling and every length from
/// [`MIN_MATCH`] up to it can be encoded, which is what lets the search below
/// consider them without storing a distance per length.
fn find_matches(data: &[u8]) -> (Vec<u8>, Vec<u16>) {
    let length = data.len();
    let mut lens = vec![0u8; length];
    let mut dists = vec![0u16; length];
    if length < MIN_MATCH {
        return (lens, dists);
    }

    let mut head = vec![-1i32; HASH_SIZE];
    let mut prev = vec![-1i32; MAX_DIST + 1];

    for i in 0..length.saturating_sub(2) {
        let h = hash3(data[i], data[i + 1], data[i + 2]);
        let mut candidate = head[h];
        let mut chain_count = 0;
        let mut best_len = 0usize;
        let mut best_dist = 0usize;

        while candidate >= 0 && chain_count < MAX_CHAIN {
            let cand = candidate as usize;
            let dist = i - cand;
            if dist > MAX_DIST || dist == 0 {
                break;
            }
            if data[cand] == data[i] {
                let mut match_len = 0;
                while match_len < MAX_MATCH
                    && i + match_len < length
                    && data[i + match_len] == data[cand + (match_len % dist)]
                {
                    match_len += 1;
                }
                if match_len >= MIN_MATCH && match_len > best_len {
                    best_len = match_len;
                    best_dist = dist;
                    if best_len == MAX_MATCH {
                        break;
                    }
                }
            }
            let next = prev[cand % (MAX_DIST + 1)];
            if next >= 0 && (next as usize) >= i.saturating_sub(MAX_DIST) {
                candidate = next;
                chain_count += 1;
            } else {
                break;
            }
        }

        lens[i] = best_len as u8;
        dists[i] = best_dist as u16;

        prev[i % (MAX_DIST + 1)] = head[h];
        head[h] = i as i32;
    }

    (lens, dists)
}

/// Compress data using PalmDOC LZ77 compression.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let length = data.len();
    if length == 0 {
        return Vec::new();
    }
    let (lens, dists) = find_matches(data);

    // Cheapest cost from each position to the end, and the step that achieves
    // it. Solved backwards so that every position a step lands on is already
    // priced. `u32::MAX` marks a position not yet reached; every byte has at
    // least one legal encoding, so no reachable position keeps it.
    let mut cost = vec![u32::MAX; length + 1];
    let mut step = vec![Step::Raw; length];
    cost[length] = 0;

    for i in (0..length).rev() {
        let b = data[i];
        let mut best = u32::MAX;
        let mut best_step = Step::Raw;

        // One byte as itself. Everything outside the escaped set qualifies,
        // and that includes a plain space.
        if !needs_escape(b) {
            best = 1 + cost[i + 1];
            best_step = Step::Raw;
        }

        // A space and the printable byte after it, both in one output byte.
        // This is the only encoding that beats one byte per source byte
        // without a back reference, and on English prose it fires constantly.
        if b == 0x20 && i + 1 < length && (0x40..=0x7F).contains(&data[i + 1]) {
            let c = 1 + cost[i + 2];
            if c < best {
                best = c;
                best_step = Step::SpacePair;
            }
        }

        // A run of bytes that need escaping, behind one count byte. Longer is
        // not automatically better: the run has to stop where the cheapest
        // route wants it to, which may be short of the eight it could hold.
        let mut k = 0usize;
        while k < 8 && i + k < length && needs_escape(data[i + k]) {
            k += 1;
            let c = (1 + k) as u32 + cost[i + k];
            if c < best {
                best = c;
                best_step = Step::Run(k as u8);
            }
        }

        // A match, two bytes whatever its length, so every length worth
        // considering is considered rather than only the longest.
        let max_len = (lens[i] as usize).min(MAX_MATCH);
        for l in MIN_MATCH..=max_len {
            let c = 2 + cost[i + l];
            if c < best {
                best = c;
                best_step = Step::Match(l as u8, dists[i]);
            }
        }

        cost[i] = best;
        step[i] = best_step;
    }

    let mut output = Vec::with_capacity(length);
    let mut i = 0usize;
    while i < length {
        match step[i] {
            Step::Raw => {
                output.push(data[i]);
                i += 1;
            }
            Step::SpacePair => {
                output.push(data[i + 1] ^ 0x80);
                i += 2;
            }
            Step::Run(n) => {
                let n = n as usize;
                output.push(n as u8);
                output.extend_from_slice(&data[i..i + n]);
                i += n;
            }
            Step::Match(len, dist) => {
                let encoded = ((dist as usize) << 3) | (len as usize - MIN_MATCH);
                output.push(0x80 | ((encoded >> 8) & 0x3F) as u8);
                output.push((encoded & 0xFF) as u8);
                i += len as usize;
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_empty() {
        assert_eq!(compress(b""), Vec::<u8>::new());
        println!("  \u{2713} Compressing empty input yields empty output");
    }

    #[test]
    fn test_compress_short() {
        let compressed = compress(b"hello");
        assert!(!compressed.is_empty());
        println!(
            "  \u{2713} Compressed 5 bytes to {} bytes",
            compressed.len()
        );
    }

    #[test]
    fn test_compress_repeated() {
        let data = b"abcabcabcabc";
        let compressed = compress(data);
        // Compressed should be smaller than original due to LZ77 matches
        assert!(compressed.len() <= data.len());
        println!(
            "  \u{2713} Repeated data: {} -> {} bytes",
            data.len(),
            compressed.len()
        );
    }

    /// Decode exactly the way the format specifies, so the tests below can
    /// assert on real bytes rather than on the encoder agreeing with itself.
    fn decompress(compressed: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < compressed.len() {
            let b = compressed[i];
            i += 1;
            match b {
                0x00 | 0x09..=0x7F => out.push(b),
                0x01..=0x08 => {
                    out.extend_from_slice(&compressed[i..i + b as usize]);
                    i += b as usize;
                }
                0x80..=0xBF => {
                    let pair = (((b as usize) << 8) | compressed[i] as usize) & 0x3FFF;
                    i += 1;
                    let (dist, len) = (pair >> 3, (pair & 0x7) + MIN_MATCH);
                    for _ in 0..len {
                        let at = out.len() - dist;
                        out.push(out[at]);
                    }
                }
                0xC0..=0xFF => {
                    out.push(b' ');
                    out.push(b ^ 0x80);
                }
            }
        }
        out
    }

    /// Ordinary words, arranged so that longest-match-wins loses.
    ///
    /// Greedy spends 55 bytes here. Three times over, the longest match at a
    /// position steps across the start of a longer one, and paying a literal
    /// or a shorter match first gets the whole run for the same two bytes.
    /// Nothing about the string is contrived except the word order.
    #[test]
    fn a_short_match_never_blocks_a_long_one() {
        let data = b"defined definition more common the text definition defined \
to a again text definition cat";
        let compressed = compress(data);
        assert_eq!(decompress(&compressed).as_slice(), data.as_slice());
        assert!(
            compressed.len() <= 52,
            "greedy spends 55 bytes on this; got {}",
            compressed.len()
        );
    }

    /// Whatever the encoder chooses, a reader has to get the input back.
    ///
    /// Covers the byte classes that pick different encodings: plain ASCII,
    /// the space-plus-letter pair, the bytes that have to be escaped because
    /// the decoder reads them as counts or tokens, and a run long enough to
    /// exercise overlapping matches.
    #[test]
    fn every_byte_class_round_trips() {
        let cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"a".to_vec(),
            (0u8..=255).collect(),
            (0u8..=255).chain(0u8..=255).collect(),
            vec![0x00; 50],
            vec![0x80; 50],
            (0..300).map(|i| (i % 7 + 1) as u8).collect(),
            "Greek \u{3b1}\u{3b2}\u{3b3} and more \u{3b1}\u{3b2}\u{3b3} again"
                .as_bytes()
                .to_vec(),
            b"the quick brown fox jumps over the quick brown fox".to_vec(),
        ];
        for case in cases {
            let got = decompress(&compress(&case));
            assert_eq!(got, case, "round trip failed on {} bytes", case.len());
        }
    }
}
