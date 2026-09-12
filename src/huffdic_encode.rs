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
//!
//! # Both byte orders
//!
//! The HUFF header carries four table offsets, not two. The first pair
//! points at the prefix table and the per-length bounds in big-endian; the
//! second pair points at the same two tables again, little-endian. Real
//! kindlegen 2.9 output carries both, and this writes both to match it.
//! calibre, KindleUnpack and libmobi read only the first pair, so none of
//! them can tell whether the second is there.
//!
//! The second pair was not what made Mobipocket Reader for Windows open a
//! kindling dictionary: it went on calling one that carried both pairs "File
//! corrupted". What it wanted was the DATP record, which is built in
//! `mobi.rs` (issue #49). The pairs are written anyway, because a file that
//! matches kindlegen leaves one less thing to suspect.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;

use crate::huffcdic::Huffdic;

/// Longest code the format can express: `mincode`/`maxcode` have 32 slots.
const MAX_CODE_LEN: usize = 32;

/// Ceiling on the phrase dictionary.
///
/// kindlegen's own files sit in the hundreds to low thousands, and 4096 was
/// chosen to match. That ceiling was costing a great deal: on Webster's
/// Unabridged, raising it to 32768 with everything else held fixed takes the
/// text from 74% of the PalmDOC text to 65%. A phrase only stays in the
/// dictionary if it earns back the `len + 4` bytes it occupies in a CDIC
/// (see `fit_model`), so the ceiling is not what stops the dictionary filling
/// with junk and there is no reason for it to sit this low.
///
/// It is not what settles the size either: on Webster the rounds converge on
/// about 44000 phrases and stay there whether this is 65536 or 131072,
/// because everything else on offer has gone negative. It is a ceiling
/// rather than no limit because each phrase costs a Huffman code, and a code
/// cannot exceed 32 bits.
const MAX_PHRASES: usize = 65536;

/// Longest multi-byte phrase the *sampler* offers. This is only the seed:
/// merges grow phrases from there, up to `MAX_MERGED_LEN`. Past 24 the
/// frequency of an exact repeat collapses, the sample costs proportionally
/// more to count, and the output stops moving.
const MAX_PHRASE_LEN: usize = 24;

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
    // Writes the exact chunks a real book produced, so that the `corpus_probe`
    // test below can re-measure the encoder on them without rebuilding the
    // book. A failed write is not worth losing the build over, since nothing
    // but that probe reads the file.
    if let Ok(path) = std::env::var("KINDLING_HUFF_DUMP") {
        let mut blob: Vec<u8> = Vec::new();
        for c in chunks {
            blob.extend_from_slice(&(c.len() as u32).to_be_bytes());
            blob.extend_from_slice(c);
        }
        let _ = std::fs::write(path, blob);
    }
    // Below this the phrase dictionary and its tables cost more than the
    // codes save. The HUFF record alone is 2.6 KB.
    if total < 64 * 1024 {
        return None;
    }

    let Fit {
        phrases,
        parsed,
        table,
    } = fit_model(chunks, total)?;

    let records: Vec<Vec<u8>> = parsed
        .par_iter()
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

/// Most rounds of "parse, price, re-select" to run.
///
/// Round 0 is the one-shot model this replaced: crude scores, greedy parse.
/// Each round after it re-prices every phrase against the code the previous
/// round produced, offers the merges the parse asked for, and re-selects.
/// Round 1 is worth about 8% of the output on its own and the rounds after
/// it between a fraction of a percent and a percent each, so this stops on
/// the improvement as well as on the count; Webster settles around round 20
/// and never reaches this ceiling.
const REFINE_ROUNDS: usize = 40;

/// Fraction of the dictionary that may be filled on an estimate in one
/// round, as a divisor.
///
/// This is the one thing holding the selector to what it has measured. An
/// estimate is optimistic in a way a measured frequency is not: it counts
/// overlapping matches no parse can all take, or counts two symbols sitting
/// next to each other that the merged phrase may not be reached for. Letting
/// every free slot fill with guesses costs a large book a little and a small
/// one everything, because a round that prunes most of the dictionary
/// refills it from the next half-million untested candidates and undoes its
/// own work: the 1.2 MB synthetic dictionary in the tests below finished at
/// 85% of PalmDOC that way, and at 18% with this in place. Halving rather
/// than discounting the estimates themselves, which was also tried: a
/// discount costs Webster 0.9% of the file and does this job less well.
const SPECULATIVE_SHARE: usize = 2;

/// Rounds with no worthwhile gain to sit through before stopping.
const STALL_PATIENCE: usize = 3;

/// Rounds stop once one of them shrinks the output by less than this
/// fraction. 1/1000 leaves about a quarter of a percent on the table against
/// running to exhaustion, for a third of the rounds.
const REFINE_FLOOR: usize = 1000;

/// Bytes of text the rounds may parse in total.
///
/// Every round re-parses the whole book, so the round count has to come down
/// as the book grows or a 200 MB dictionary would spend minutes here. This is
/// a budget rather than a clock so that the same input always produces the
/// same file. Webster, at 28 MB, is under the budget and stops on the
/// improvement instead, around round 20.
const REFINE_BUDGET: usize = REFINE_ROUNDS * 32 * 1024 * 1024;

/// Candidates kept per dictionary slot. The selector needs somewhere to
/// refill from when it drops a phrase, and a phrase that loses in round 1
/// may well be the best thing available in round 2.
const POOL_FACTOR: usize = 8;

/// A phrase the selector may put in the dictionary.
struct Candidate {
    bytes: Vec<u8>,
    /// Occurrences in the whole text: extrapolated from the sample for a
    /// phrase the sampler found, or the number of times a round's parse put
    /// the two halves next to each other for a phrase a merge proposed.
    /// Either way an over-count, because neither is a count of the times a
    /// parse would actually reach for the phrase; it is only ever used to
    /// rank candidates against each other, and the next round measures the
    /// truth for whatever gets a slot.
    est_occ: u64,
}

/// A finished fit: the phrase dictionary in symbol-index order, the symbol
/// stream for every chunk, and the code table the two agree on.
struct Fit {
    phrases: Vec<Vec<u8>>,
    parsed: Vec<Vec<u32>>,
    table: CodeTable,
}

/// Choose the phrase dictionary, the parse and the code together.
///
/// The three depend on each other: what a phrase is worth depends on the code
/// it gets, the code depends on how often the parse uses it, and the parse
/// depends on what the other phrases cost. One pass cannot settle that, so
/// this runs several.
///
/// Round 0 seeds the set with the phrases that remove the most bytes
/// (`occurrences * (len - 1)`) and parses greedily, because there is no code
/// yet to price anything against. Every round after it does four things:
///
/// 1. Parse as a shortest path over the code lengths the last round produced,
///    rather than longest-match-wins. A six-byte phrase used twice carries a
///    twenty-bit code, and spelling the same bytes out of three common
///    phrases often costs less.
/// 2. Price every phrase in the dictionary at what it actually saves: its
///    measured frequency times the bits its bytes would cost if they had to
///    be spelled out of the *other* phrases, less the bits its own code
///    costs, less the bytes it occupies in a CDIC. A phrase that cannot clear
///    zero is dropped for good.
/// 3. Offer the merges. Every pair of symbols this parse put next to each
///    other is a phrase the dictionary could hold instead of the two, and
///    those counts are what the parse did rather than what a sample of
///    overlapping substrings suggested. This is where the dictionary gets the
///    phrases the sampler never saw, including every phrase longer than the
///    sampler looks.
/// 4. Re-select: the measured phrases and the candidates compete on value for
///    the slots, with a cap on how much of the dictionary may turn over on an
///    estimate in one round (`SPECULATIVE_SHARE`).
///
/// On Webster's Unabridged the file goes 75.5% of PalmDOC after round 0, then
/// 67.5, 66.2, 65.5, ... , 56.4, 56.3 and stops; the merges in step 3 are
/// worth about seven points of that and nothing else comes close.
///
/// The smallest round wins; a later round is not guaranteed to be better.
fn fit_model(chunks: &[Vec<u8>], total: usize) -> Option<Fit> {
    let debug = std::env::var("KINDLING_HUFFDIC_DEBUG").is_ok();
    // Every constant this loop reads can be overridden from the environment,
    // so that a tuning sweep is a shell loop rather than a rebuild per value.
    // Nothing sets these in normal use and each falls back to its constant.
    let tune = |k: &str, d: usize| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(d)
    };
    let rounds = tune(
        "KH_ROUNDS",
        (REFINE_BUDGET / total.max(1)).clamp(4, REFINE_ROUNDS),
    );
    let max_phrases = tune("KH_PHRASES", MAX_PHRASES);
    let floor = tune("KH_FLOOR", REFINE_FLOOR);
    let merge_len = tune("KH_MLEN", MAX_MERGED_LEN).min(MAX_MERGED_LEN);
    let spec_share = tune("KH_SPEC", SPECULATIVE_SHARE).max(1);
    let t_start = std::time::Instant::now();

    let mut present = [false; 256];
    for chunk in chunks {
        for &b in chunk {
            present[b as usize] = true;
        }
    }
    // Every byte the text uses becomes a one-byte phrase, because there is no
    // other way to express a byte: without them a text with an unexpected
    // character could not be encoded at all. These are never dropped.
    let singles: Vec<Vec<u8>> = (0..256usize)
        .filter(|&b| present[b])
        .map(|b| vec![b as u8])
        .collect();
    if singles.is_empty() {
        return None;
    }
    let n_singles = singles.len();
    let room = max_phrases.saturating_sub(n_singles);
    if room == 0 {
        return None;
    }

    let pool = candidate_pool(
        chunks,
        total,
        max_phrases * tune("KH_POOL", POOL_FACTOR),
        tune("KH_PLEN", MAX_PHRASE_LEN),
    );

    // Selection state. `chosen` indexes `pool`; `dead` marks candidates that
    // have been measured and found not to pay, so one of those is never
    // reconsidered and the rounds cannot oscillate over it. `in_dict` is the
    // membership of `chosen`, so the refill can skip what is already in.
    let mut pool = pool;
    let mut chosen: Vec<usize> = (0..pool.len().min(room)).collect();
    let mut dead = vec![false; pool.len()];
    let mut in_dict = vec![false; pool.len()];
    for &pi in &chosen {
        in_dict[pi] = true;
    }
    // Every phrase already offered, so a pair two rounds both suggest is not
    // added to the pool twice.
    let mut seen: HashSet<Vec<u8>> = pool.iter().map(|c| c.bytes.clone()).collect();
    // Last code length seen for each phrase, indexed singles-then-pool. Zero
    // means the phrase has not been in a dictionary yet.
    let mut last_len = vec![0u8; n_singles + pool.len()];

    let mut best: Option<(usize, Fit)> = None;
    // Consecutive rounds that have not beaten the best by enough to be worth
    // another. A single worse round is not a reason to stop: the selector
    // swaps a large part of the dictionary at once and the round after a bad
    // swap is often the best yet.
    let mut flat = 0usize;

    for round in 0..=rounds {
        let mut phrases: Vec<&[u8]> = Vec::with_capacity(n_singles + chosen.len());
        let mut universe: Vec<usize> = Vec::with_capacity(n_singles + chosen.len());
        for (i, s) in singles.iter().enumerate() {
            phrases.push(s.as_slice());
            universe.push(i);
        }
        for &c in &chosen {
            phrases.push(pool[c].bytes.as_slice());
            universe.push(n_singles + c);
        }
        let trie = Trie::new(&phrases);

        // What a symbol costs in this round's parse. Round 0 has no code to
        // price against, so it parses greedily, exactly as the one-shot model
        // did. Later rounds carry the previous round's code lengths, and a
        // phrase that has never been in a dictionary is priced from the
        // occurrences the sample saw.
        let parsed: Option<Vec<Vec<u32>>> = if round == 0 {
            chunks.par_iter().map(|c| trie.parse_greedy(c)).collect()
        } else {
            let costs: Vec<u32> = universe
                .iter()
                .map(|&u| match last_len[u] {
                    0 => {
                        let occ = pool
                            .get(u.wrapping_sub(n_singles))
                            .map(|c| c.est_occ)
                            .unwrap_or(1);
                        est_code_len(occ, total as u64)
                    }
                    l => l as u32,
                })
                .collect();
            chunks
                .par_iter()
                .map(|c| trie.parse_dp(c, &costs))
                .collect()
        };
        let parsed = parsed?;

        let mut freq = vec![0u64; phrases.len()];
        for stream in &parsed {
            for &s in stream {
                freq[s as usize] += 1;
            }
        }

        let Some(lengths) = code_lengths(&freq) else {
            if debug {
                eprintln!("huffdic: round {round}, no code fits in 32 bits");
            }
            break;
        };
        let Some(table) = CodeTable::build(&lengths) else {
            if debug {
                eprintln!("huffdic: round {round}, code table could not be laid out");
            }
            break;
        };

        let record_bytes: usize = parsed
            .par_iter()
            .map(|stream| {
                let bits: usize = stream.iter().map(|&s| lengths[s as usize] as usize).sum();
                bits.div_ceil(8)
            })
            .sum();
        let dict_bytes: usize = phrases.iter().map(|p| p.len() + 4).sum::<usize>() + 2584;
        let size = record_bytes + dict_bytes;
        if debug {
            let symbols: u64 = freq.iter().sum();
            eprintln!(
                "huffdic: round {round}, {} phrases, {symbols} symbols, {size} bytes ({record_bytes} text + {dict_bytes} dict), pool {}, {:.1}s",
                phrases.len(),
                pool.len(),
                t_start.elapsed().as_secs_f64()
            );
        }

        let stalled = match best.as_ref() {
            Some(&(b, _)) if b.saturating_sub(size) < b / floor => {
                flat += 1;
                flat >= STALL_PATIENCE
            }
            _ => {
                flat = 0;
                false
            }
        };
        // The pairs this parse put next to each other are the next round's
        // candidates, and `parsed` is about to be moved into `best`.
        let pairs = if round == rounds || stalled {
            Vec::new()
        } else {
            adjacent_pairs(&parsed)
        };
        let improved = best.as_ref().is_none_or(|(b, _)| size < *b);
        if improved {
            let ordered: Vec<Vec<u8>> = table
                .symbol_of_index
                .iter()
                .map(|&s| phrases[s as usize].to_vec())
                .collect();
            best = Some((
                size,
                Fit {
                    phrases: ordered,
                    parsed,
                    table,
                },
            ));
        }
        if round == rounds || stalled {
            if debug && stalled {
                eprintln!("huffdic: round {round} gained too little, stopping");
            }
            break;
        }

        // Re-price. A phrase is worth the bits the bytes it covers would cost
        // if they had to be spelled out of the *other* phrases, less what its
        // own code costs, less the bytes it occupies in the CDIC.
        for (local, &u) in universe.iter().enumerate() {
            last_len[u] = lengths[local];
        }
        let costs: Vec<u32> = lengths.iter().map(|&l| l as u32).collect();
        let symbols: u64 = freq.iter().sum();
        let mut ranked: Vec<(i64, usize)> = chosen
            .par_iter()
            .enumerate()
            .map(|(k, &pi)| {
                let local = n_singles + k;
                let p = &pool[pi].bytes;
                let alone = trie.span_cost(p, &costs, local as u32);
                let value = freq[local] as i64 * (alone as i64 - costs[local] as i64)
                    - (p.len() as i64 + 4) * 8;
                (value, pi)
            })
            .collect();
        // A phrase that does not pay for itself is out for good. One that
        // pays but loses its slot on rank stays in the pool, because the code
        // it was priced against moves underneath it every round.
        for &(value, pi) in &ranked {
            in_dict[pi] = false;
            if value <= 0 {
                dead[pi] = true;
            }
        }
        ranked.retain(|&(value, _)| value > 0);

        // Offer the pairs. Two symbols the parse emitted next to each other
        // are a phrase the dictionary could hold instead of the two; the
        // count is what the parse actually did rather than what a sample of
        // overlapping substrings suggested, and merging is the only way a
        // phrase longer than the sampler ever looked at can appear at all.
        let mut offered: Vec<Candidate> = Vec::new();
        for &(a, b, count) in &pairs {
            let (pa, pb) = (phrases[a as usize], phrases[b as usize]);
            let len = pa.len() + pb.len();
            if len > merge_len {
                continue;
            }
            let occ = count as u64;
            let now = (costs[a as usize] + costs[b as usize]) as i64;
            let value =
                count as i64 * (now - est_code_len(occ, symbols) as i64) - (len as i64 + 4) * 8;
            if value <= 0 {
                continue;
            }
            let mut bytes = Vec::with_capacity(len);
            bytes.extend_from_slice(pa);
            bytes.extend_from_slice(pb);
            // A pair already offered keeps the count of the round that first
            // suggested it. Re-counting it against every later parse was
            // tried and came out slightly worse: once its two halves are
            // both in the dictionary the count says how often they sit
            // together, which is not the same as how often the merged phrase
            // would be reached for.
            if !seen.contains(bytes.as_slice()) {
                offered.push(Candidate {
                    bytes,
                    est_occ: occ,
                });
            }
        }
        // Separate from the loop above only because `phrases` borrows `pool`.
        for c in offered {
            seen.insert(c.bytes.clone());
            pool.push(c);
            dead.push(false);
            in_dict.push(false);
            last_len.push(0);
        }

        // Price everything still available the same way, but against the
        // occurrences the sample or the parse saw rather than a measured
        // frequency, and let the whole field compete for the slots. A phrase
        // only keeps its slot if nothing available is worth more.
        let mut refill: Vec<(i64, usize)> = (0..pool.len())
            .into_par_iter()
            .filter(|&pi| !dead[pi] && !in_dict[pi])
            .filter_map(|pi| {
                let c = &pool[pi];
                let occ = c.est_occ;
                let alone = trie.span_cost(&c.bytes, &costs, u32::MAX);
                let mine = est_code_len(occ, symbols);
                let value =
                    occ as i64 * (alone as i64 - mine as i64) - (c.bytes.len() as i64 + 4) * 8;
                if value > 0 { Some((value, pi)) } else { None }
            })
            .collect();
        // Highest value first, and the lower pool index on a tie, so the same
        // book always picks the same dictionary.
        let order = |a: &(i64, usize), b: &(i64, usize)| b.0.cmp(&a.0).then(a.1.cmp(&b.1));
        // Take only so many on an estimate at a time. Every one of these is
        // a guess that the next round measures, and filling every free slot
        // with guesses is how the dictionary ends up costing more in CDIC
        // bytes than the phrases save: a round that prunes half the
        // dictionary would refill it with the next half-million untested
        // candidates and undo its own work.
        // As a share of the dictionary that exists, not of the ceiling: a
        // round that has pruned to ten thousand phrases must not be handed
        // eight thousand guesses.
        let speculative = (chosen.len() / spec_share).max(64);
        if refill.len() > speculative {
            refill.select_nth_unstable_by(speculative, order);
            refill.truncate(speculative);
        }
        ranked.append(&mut refill);
        if ranked.len() > room {
            ranked.select_nth_unstable_by(room, order);
            ranked.truncate(room);
        }
        let mut next: Vec<usize> = ranked.into_iter().map(|(_, pi)| pi).collect();
        next.sort_unstable();
        if next == chosen {
            if debug {
                eprintln!("huffdic: round {round} changed nothing, stopping");
            }
            break;
        }
        for &pi in &next {
            in_dict[pi] = true;
        }
        chosen = next;
    }

    best.map(|(_, fit)| fit)
}

/// Longest phrase a merge may build.
///
/// A phrase costs its own length in every CDIC whether the parse reaches for
/// it or not, and `Trie::span_cost` prices a span in time quadratic in this,
/// so it buys nothing to set it far above the longest boilerplate a
/// dictionary entry actually repeats.
const MAX_MERGED_LEN: usize = 64;

/// Pairs below this many occurrences are not worth carrying into the pool.
const MIN_PAIR_COUNT: u32 = 4;

/// Count the symbol pairs the parse put next to each other.
///
/// Open-addressed rather than a `HashMap` because this runs once per round
/// over every symbol in the book, and because a fixed table iterates in a
/// fixed order: the same book has to produce the same dictionary twice.
fn adjacent_pairs(parsed: &[Vec<u32>]) -> Vec<(u32, u32, u32)> {
    // 4M slots hold the distinct pairs of a book far larger than this one,
    // and cap the memory for one larger still. Once the table is full it only
    // counts pairs it has already seen, which costs the tail of the candidate
    // list, not correctness.
    const SLOTS: usize = 1 << 22;
    let mask = SLOTS - 1;
    let mut keys = vec![u64::MAX; SLOTS];
    let mut vals = vec![0u32; SLOTS];
    let mut used = 0usize;
    let limit = SLOTS * 7 / 10;

    for stream in parsed {
        let mut i = 0usize;
        while i + 1 < stream.len() {
            let (a, b) = (stream[i], stream[i + 1]);
            let key = ((a as u64) << 32) | b as u64;
            let mut slot = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & mask;
            loop {
                let k = keys[slot];
                if k == key {
                    vals[slot] = vals[slot].saturating_add(1);
                    break;
                }
                if k == u64::MAX {
                    if used < limit {
                        keys[slot] = key;
                        vals[slot] = 1;
                        used += 1;
                    }
                    break;
                }
                slot = (slot + 1) & mask;
            }
            // A run of one symbol can only be merged every other position, so
            // counting every adjacency would promise twice what the merge can
            // deliver.
            i += if a == b { 2 } else { 1 };
        }
    }

    let mut out = Vec::new();
    for slot in 0..SLOTS {
        if keys[slot] != u64::MAX && vals[slot] >= MIN_PAIR_COUNT {
            out.push((
                (keys[slot] >> 32) as u32,
                (keys[slot] & 0xFFFF_FFFF) as u32,
                vals[slot],
            ));
        }
    }
    out
}

/// Roughly the code length a symbol seen `occ` times out of `total` gets.
fn est_code_len(occ: u64, total: u64) -> u32 {
    if occ == 0 || total == 0 {
        return 24;
    }
    let bits = -((occ as f64 / total as f64).log2());
    bits.clamp(1.0, 30.0).round() as u32
}

/// Count repeated substrings and keep the most promising ones.
///
/// The pool is several times larger than the dictionary, because the selector
/// needs somewhere to refill from once it starts dropping phrases.
fn candidate_pool(
    chunks: &[Vec<u8>],
    total: usize,
    pool_cap: usize,
    max_len: usize,
) -> Vec<Candidate> {
    // Count substrings on a sample rather than the whole text: a 200 MB
    // dictionary would otherwise build a hash map of hundreds of millions of
    // entries to answer a question a few megabytes answers just as well.
    // 1 MB, not the 4 MB this used to take. A bigger sample is not a better
    // seed: the merges in `fit_model` supply the phrases the sample is too
    // coarse to see, and a sample four times the size only lowers the bar
    // that "at least four occurrences" sets, so the seed fills with phrases
    // that occur once per megabyte. On Webster, 4 MB costs 14 KB of output
    // over 1 MB, twice the encode, and 2.5 GB of the peak memory, because
    // this map holds every distinct substring of every length in the sample.
    const SAMPLE_BYTES: usize = 1024 * 1024;
    let stride = (total / SAMPLE_BYTES).max(1);

    let mut counts: HashMap<&[u8], u32> = HashMap::new();
    for (i, chunk) in chunks.iter().enumerate() {
        if i % stride != 0 {
            continue;
        }
        for start in 0..chunk.len() {
            let max_len = max_len.min(chunk.len() - start);
            for len in 2..=max_len {
                let s = &chunk[start..start + len];
                *counts.entry(s).or_insert(0) += 1;
            }
        }
    }

    let mut scored: Vec<(u64, &[u8], u32)> = counts
        .iter()
        .filter(|(s, c)| **c >= 4 && s.len() >= 2)
        .map(|(s, c)| (*c as u64 * (s.len() as u64 - 1), *s, *c))
        .collect();
    // Longest first within an equal score, so the parse has the bigger win
    // available when both fit, and then by the bytes themselves: the counts
    // come out of a `HashMap` whose iteration order is seeded per process, so
    // without a total order the same book would not build the same file
    // twice.
    scored.sort_unstable_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.len().cmp(&a.1.len()))
            .then(a.1.cmp(b.1))
    });
    scored.truncate(pool_cap);

    scored
        .into_iter()
        .map(|(_, s, c)| Candidate {
            bytes: s.to_vec(),
            est_occ: c as u64 * stride as u64,
        })
        .collect()
}

/// Prefix trie over the phrase set: one walk from a position yields every
/// phrase that matches there, which is what both the greedy parse and the
/// shortest-path parse need.
///
/// Children live in one open-addressed table keyed by `(node, byte)` rather
/// than a 256-wide array per node, which would be 64 MB of mostly-empty
/// pointers for a dictionary this size.
struct Trie {
    keys: Vec<u64>,
    vals: Vec<u32>,
    mask: usize,
    /// Symbol ending at each node, or `u32::MAX` when no phrase ends here.
    sym: Vec<u32>,
}

impl Trie {
    fn new(phrases: &[&[u8]]) -> Trie {
        let nodes = phrases.iter().map(|p| p.len()).sum::<usize>() + 1;
        let cap = (nodes * 2).next_power_of_two().max(16);
        let mut t = Trie {
            keys: vec![u64::MAX; cap],
            vals: vec![0; cap],
            mask: cap - 1,
            sym: vec![u32::MAX; nodes + 1],
        };
        let mut next = 1u32;
        for (i, p) in phrases.iter().enumerate() {
            let mut node = 0u32;
            for &b in *p {
                let key = ((node as u64) << 8) | b as u64;
                let (slot, hit) = t.slot(key);
                if hit {
                    node = t.vals[slot];
                } else {
                    t.keys[slot] = key;
                    t.vals[slot] = next;
                    node = next;
                    next += 1;
                }
            }
            t.sym[node as usize] = i as u32;
        }
        t
    }

    /// Slot for `key`, and whether it is already occupied by that key. A key
    /// is `(node << 8) | byte` and so never reaches the empty sentinel.
    #[inline]
    fn slot(&self, key: u64) -> (usize, bool) {
        let mut i = (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) as usize & self.mask;
        loop {
            let k = self.keys[i];
            if k == key {
                return (i, true);
            }
            if k == u64::MAX {
                return (i, false);
            }
            i = (i + 1) & self.mask;
        }
    }

    #[inline]
    fn child(&self, node: u32, b: u8) -> Option<u32> {
        let (slot, hit) = self.slot(((node as u64) << 8) | b as u64);
        if hit { Some(self.vals[slot]) } else { None }
    }

    /// Longest match wins. This is the bootstrap parse, used before there is
    /// a code to price alternatives against.
    fn parse_greedy(&self, data: &[u8]) -> Option<Vec<u32>> {
        let mut out = Vec::with_capacity(data.len() / 2);
        let mut at = 0usize;
        while at < data.len() {
            let mut node = 0u32;
            let mut j = at;
            let mut take = 0usize;
            let mut sym = 0u32;
            while j < data.len() {
                match self.child(node, data[j]) {
                    Some(c) => {
                        node = c;
                        j += 1;
                    }
                    None => break,
                }
                let s = self.sym[node as usize];
                if s != u32::MAX {
                    take = j - at;
                    sym = s;
                }
            }
            // Only reachable if a byte has no one-byte phrase, which
            // `fit_model` guarantees against.
            if take == 0 {
                return None;
            }
            out.push(sym);
            at += take;
        }
        Some(out)
    }

    /// Shortest path over `cost`: the split of `data` that costs the fewest
    /// bits under the code lengths handed in.
    ///
    /// Greedy longest-match is not this. A six-byte phrase used twice in the
    /// book carries a 20-bit code, and spelling the same six bytes out of
    /// three common phrases can easily cost less; greedy takes the long one
    /// anyway, and the frequency it lends that phrase keeps it in the
    /// dictionary for another round.
    fn parse_dp(&self, data: &[u8], cost: &[u32]) -> Option<Vec<u32>> {
        let n = data.len();
        let mut best = vec![u32::MAX; n + 1];
        let mut pick = vec![(0u16, 0u32); n + 1];
        best[n] = 0;
        for i in (0..n).rev() {
            let mut node = 0u32;
            let mut j = i;
            let mut bits = u32::MAX;
            let mut take = 0u16;
            let mut sym = 0u32;
            while j < n {
                match self.child(node, data[j]) {
                    Some(c) => {
                        node = c;
                        j += 1;
                    }
                    None => break,
                }
                let s = self.sym[node as usize];
                if s != u32::MAX {
                    let rest = best[j];
                    if rest != u32::MAX {
                        let c = cost[s as usize] + rest;
                        // Ties go to the longer phrase: same bits, fewer
                        // symbols, and a shorter stream to walk next round.
                        if c <= bits {
                            bits = c;
                            take = (j - i) as u16;
                            sym = s;
                        }
                    }
                }
            }
            if take == 0 {
                return None;
            }
            best[i] = bits;
            pick[i] = (take, sym);
        }
        let mut out = Vec::with_capacity(n / 2);
        let mut at = 0usize;
        while at < n {
            let (take, sym) = pick[at];
            out.push(sym);
            at += take as usize;
        }
        Some(out)
    }

    /// Bits to spell `span` out of the phrase set, ignoring the phrase
    /// `forbid` when it would cover the whole span.
    ///
    /// This is what a phrase is measured against: drop it from the dictionary
    /// and its occurrences have to be spelled some other way, and the
    /// difference is the only saving it can honestly claim.
    fn span_cost(&self, span: &[u8], cost: &[u32], forbid: u32) -> u32 {
        let n = span.len();
        let mut best = [u32::MAX; MAX_MERGED_LEN + 1];
        best[n] = 0;
        for i in (0..n).rev() {
            let mut node = 0u32;
            let mut j = i;
            let mut bits = u32::MAX;
            while j < n {
                match self.child(node, span[j]) {
                    Some(c) => {
                        node = c;
                        j += 1;
                    }
                    None => break,
                }
                let s = self.sym[node as usize];
                if s == u32::MAX || (s == forbid && i == 0 && j == n) {
                    continue;
                }
                if best[j] != u32::MAX {
                    bits = bits.min(cost[s as usize] + best[j]);
                }
            }
            best[i] = bits;
        }
        best[0]
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
        let longest = (1..=MAX_CODE_LEN).rfind(|&l| used[l])?;
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
        // The same two tables again, little-endian, after the big-endian pair.
        let off3 = off2 + 32 * 8;
        let off4 = off3 + 256 * 4;

        let mut rec = Vec::with_capacity(off4 as usize + 32 * 8);
        rec.extend_from_slice(b"HUFF");
        rec.extend_from_slice(&HEADER_LEN.to_be_bytes());
        rec.extend_from_slice(&off1.to_be_bytes());
        rec.extend_from_slice(&off2.to_be_bytes());
        // Where the little-endian copies start. These are not reserved words:
        // they were written as zero here, copied from a fixture that had them
        // zero, while real kindlegen output points them at a second,
        // byte-swapped copy of both tables (issue #49).
        rec.extend_from_slice(&off3.to_be_bytes());
        rec.extend_from_slice(&off4.to_be_bytes());

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
        debug_assert_eq!(rec.len(), off3 as usize);
        // Every table word again, byte-swapped. kindlegen writes both orders,
        // and which one a reader uses is the reader's choice.
        let big_endian = rec[off1 as usize..off3 as usize].to_vec();
        for word in big_endian.chunks_exact(4) {
            rec.extend(word.iter().rev());
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
fn cdic_records(phrases: &[Vec<u8>]) -> Vec<Vec<u8>> {
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

    /// Development probe: run the encoder over a dumped corpus and print the
    /// sizes. Skipped unless `KINDLING_HUFF_CORPUS` names a dump file.
    #[test]
    fn corpus_probe() {
        let Ok(path) = std::env::var("KINDLING_HUFF_CORPUS") else {
            return;
        };
        let blob = std::fs::read(&path).unwrap();
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let mut at = 0usize;
        while at + 4 <= blob.len() {
            let n = u32::from_be_bytes(blob[at..at + 4].try_into().unwrap()) as usize;
            at += 4;
            chunks.push(blob[at..at + n].to_vec());
            at += n;
        }
        let raw: usize = chunks.iter().map(|c| c.len()).sum();
        let palm: usize = chunks
            .iter()
            .map(|c| crate::palmdoc::compress(c).len())
            .sum();
        let t0 = std::time::Instant::now();
        let e = encode(&chunks).expect("corpus should compress");
        // With KINDLING_HUFF_OUT set, write the encoding out as well, so a tool
        // that is not kindling can assemble and read it.
        if let Ok(dir) = std::env::var("KINDLING_HUFF_OUT") {
            let dir = std::path::Path::new(&dir);
            std::fs::write(dir.join("huff.bin"), &e.huff).unwrap();
            for (i, c) in e.cdics.iter().enumerate() {
                std::fs::write(dir.join(format!("cdic_{i}.bin")), c).unwrap();
            }
            for (i, r) in e.records.iter().enumerate() {
                std::fs::write(dir.join(format!("rec_{i}.bin")), r).unwrap();
            }
        }
        let secs = t0.elapsed().as_secs_f64();
        let huff: usize = e.records.iter().map(|r| r.len()).sum::<usize>()
            + e.huff.len()
            + e.cdics.iter().map(|c| c.len()).sum::<usize>();
        // The Webster file carries ~1949998 bytes that are not text records.
        let overhead = 1_949_998usize;
        println!(
            "PROBE raw {raw} palmdoc {palm} huffdic {huff} text-ratio {:.2}% file-ratio~ {:.2}% secs {secs:.1}",
            huff as f64 * 100.0 / palm as f64,
            (huff + overhead) as f64 * 100.0 / (palm + overhead) as f64,
        );
    }

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
    /// opposite case: its entries are near-duplicates of their neighbors, so
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
        // 256 prefix entries plus 32 bound pairs after a 24-byte header, then
        // the same two tables little-endian, the way kindlegen writes them
        // (issue #49).
        assert_eq!(encoded.huff.len(), 24 + 2 * (256 * 4 + 32 * 8));
        let word =
            |o: usize| u32::from_be_bytes(encoded.huff[o..o + 4].try_into().unwrap()) as usize;
        let (be1, be2, le1, le2) = (word(8), word(12), word(16), word(20));
        assert_eq!((be1, be2, le1, le2), (24, 1048, 1304, 2328));
        let swapped = |from: usize, len: usize| -> Vec<u8> {
            encoded.huff[from..from + len]
                .chunks_exact(4)
                .flat_map(|w| w.iter().rev().copied())
                .collect()
        };
        assert_eq!(
            &encoded.huff[le1..le1 + 1024],
            swapped(be1, 1024).as_slice()
        );
        assert_eq!(&encoded.huff[le2..le2 + 256], swapped(be2, 256).as_slice());
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
