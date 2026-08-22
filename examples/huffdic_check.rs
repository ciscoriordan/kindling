//! Compare the text of two MOBI files record for record, whatever compression
//! each of them uses.
//!
//! Usage: cargo run --example huffdic_check -- a.mobi b.mobi
//!
//! Exists because the interesting question about a HUFF/CDIC decoder is not
//! "did it run" but "did it produce the right bytes", and the only way to
//! answer that without trusting the decoder is to hand it a file whose text is
//! already known from somewhere else. Two such pairs exist:
//! `tests/fixtures/huffdic/*.mobi` against the PalmDOC dictionaries they were
//! transcoded from, and libmobi's `sample-unicode-huffdic.mobi` against its
//! `sample-unicode-uncompressed.mobi` twin, which is a real kindlegen 2.9
//! build nobody here produced. `scripts/validate_huffdic_fixtures.sh` runs
//! both.

use kindling::huffcdic::{self, Huffdic};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: huffdic_check <a.mobi> <b.mobi>");
        std::process::exit(2);
    }
    let a = text_of(&args[1]);
    let b = text_of(&args[2]);
    if a == b {
        println!("match: both files hold the same {} bytes of text", a.len());
        return;
    }
    eprintln!("MISMATCH: {} bytes vs {} bytes", a.len(), b.len());
    if let Some(i) = (0..a.len().min(b.len())).find(|&i| a[i] != b[i]) {
        let lo = i.saturating_sub(40);
        eprintln!("first difference at byte {i}");
        eprintln!(
            "  {}: {:?}",
            args[1],
            String::from_utf8_lossy(&a[lo..(i + 40).min(a.len())])
        );
        eprintln!(
            "  {}: {:?}",
            args[2],
            String::from_utf8_lossy(&b[lo..(i + 40).min(b.len())])
        );
    } else {
        eprintln!("one is a prefix of the other");
    }
    std::process::exit(1);
}

/// Concatenated text of every text record in the MOBI6 section.
fn text_of(path: &str) -> Vec<u8> {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"));
    let count = u16::from_be_bytes([data[76], data[77]]) as usize;
    let offsets: Vec<usize> = (0..count)
        .map(|i| u32::from_be_bytes(data[78 + i * 8..82 + i * 8].try_into().unwrap()) as usize)
        .collect();
    let records: Vec<&[u8]> = (0..count)
        .map(|i| {
            let end = if i + 1 < count {
                offsets[i + 1]
            } else {
                data.len()
            };
            &data[offsets[i]..end]
        })
        .collect();

    let record0 = records[0];
    let compression = u16::from_be_bytes([record0[0], record0[1]]);
    let text_records = u16::from_be_bytes([record0[8], record0[9]]) as usize;
    let extra_flags = u32::from_be_bytes(record0[240..244].try_into().unwrap());
    let model = Huffdic::load(&records, 0).unwrap_or_else(|e| panic!("{path}: {e}"));
    println!(
        "{path}: compression {compression}, {text_records} text records, {} phrases",
        model.as_ref().map(|m| m.phrase_count()).unwrap_or(0)
    );

    let mut out = Vec::new();
    for (i, record) in records.iter().enumerate().take(text_records + 1).skip(1) {
        let body = huffcdic::text_body(record, extra_flags);
        let piece = match (&model, compression) {
            (Some(m), _) => m
                .decompress(body)
                .unwrap_or_else(|e| panic!("{path} record {i}: {e}")),
            (None, 2) => palmdoc_decompress(body),
            (None, _) => body.to_vec(),
        };
        out.extend_from_slice(&piece);
    }
    out
}

/// PalmDOC LZ77, for the uncompressed-or-PalmDOC side of the comparison.
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
