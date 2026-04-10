/// PalmDOC LZ77 compression.
///
/// The PalmDOC compression is an LZ77 variant used in MOBI/PRC files.
/// Uses hash chain matching with the following constraints:
/// - Max distance: 2047
/// - Max match length: 10
/// - Min match length: 3

const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;
const MAX_CHAIN: usize = 64;
const MAX_DIST: usize = 2047;
const MAX_MATCH: usize = 10;
const MIN_MATCH: usize = 3;

#[inline]
fn hash3(a: u8, b: u8, c: u8) -> usize {
    (((a as usize) << 10) ^ ((b as usize) << 5) ^ (c as usize)) & HASH_MASK
}

/// Compress data using PalmDOC LZ77 compression.
pub fn compress(data: &[u8]) -> Vec<u8> {
    let length = data.len();
    let mut output = Vec::with_capacity(length);

    // Hash chain structures
    let mut head = vec![-1i32; HASH_SIZE];
    let mut prev = vec![0i32; MAX_DIST + 1];

    let mut i = 0;

    while i < length {
        let mut best_dist: usize = 0;
        let mut best_len: usize = 0;

        // Try hash chain match if we have at least 3 bytes ahead
        if i >= 1 && i + 2 < length {
            let h = hash3(data[i], data[i + 1], data[i + 2]);
            let mut candidate = head[h];
            let mut chain_count = 0;

            while candidate >= 0 && chain_count < MAX_CHAIN {
                let cand = candidate as usize;
                let dist = i - cand;
                if dist > MAX_DIST {
                    break;
                }

                // Verify candidate matches (hash collisions possible)
                if data[cand] == data[i] {
                    let mut match_len = 0;
                    while match_len < MAX_MATCH
                        && i + match_len < length
                        && data[i + match_len] == data[cand + (match_len % dist)]
                    {
                        match_len += 1;
                    }

                    if match_len >= MIN_MATCH && match_len > best_len {
                        best_dist = dist;
                        best_len = match_len;
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

            // Update hash chain for current position
            prev[i % (MAX_DIST + 1)] = head[h];
            head[h] = i as i32;
        } else if i + 2 < length {
            // i == 0, just seed the hash table
            let h = hash3(data[i], data[i + 1], data[i + 2]);
            prev[i % (MAX_DIST + 1)] = head[h];
            head[h] = i as i32;
        }

        if best_len >= MIN_MATCH {
            // Encode as LZ77 pair: 2 bytes
            let encoded = (best_dist << 3) | (best_len - 3);
            let byte1 = 0x80 | ((encoded >> 8) & 0x3F) as u8;
            let byte2 = (encoded & 0xFF) as u8;
            output.push(byte1);
            output.push(byte2);

            // Update hash chain for skipped positions
            let limit = std::cmp::min(i + best_len, length.saturating_sub(2));
            for j in (i + 1)..limit {
                let hj = hash3(data[j], data[j + 1], data[j + 2]);
                prev[j % (MAX_DIST + 1)] = head[hj];
                head[hj] = j as i32;
            }
            i += best_len;
        } else if data[i] == 0x00 {
            // Literal null byte
            output.push(0x00);
            i += 1;
        } else if data[i] == 0x20 && i + 1 < length && (0x40..=0x7F).contains(&data[i + 1]) {
            // Space + printable char optimization
            output.push(data[i + 1] ^ 0x80);
            i += 2;
        } else if (0x01..=0x08).contains(&data[i]) || data[i] >= 0x80 {
            // Bytes needing literal encoding: pack up to 8
            let mut literal_count = 0;
            while literal_count < 8
                && i + literal_count < length
                && (data[i + literal_count] <= 0x08 || data[i + literal_count] >= 0x80)
            {
                literal_count += 1;
                // Stop if next byte could be part of a match or space+char
                if literal_count < 8 && i + literal_count < length {
                    let b = data[i + literal_count];
                    if (0x09..=0x7F).contains(&b) && b != 0x20 {
                        break;
                    }
                }
            }
            if literal_count == 0 {
                literal_count = 1;
            }

            output.push(literal_count as u8);
            output.extend_from_slice(&data[i..i + literal_count]);
            i += literal_count;
        } else {
            // Regular ASCII byte (0x09-0x7F)
            output.push(data[i]);
            i += 1;
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
        println!("  \u{2713} Compressed 5 bytes to {} bytes", compressed.len());
    }

    #[test]
    fn test_compress_repeated() {
        let data = b"abcabcabcabc";
        let compressed = compress(data);
        // Compressed should be smaller than original due to LZ77 matches
        assert!(compressed.len() <= data.len());
        println!("  \u{2713} Repeated data: {} -> {} bytes", data.len(), compressed.len());
    }
}
