/// Variable-width integer encoding helpers for MOBI format.
///
/// Two conventions exist:
/// - Forward VWI: high bit SET = more bytes follow (standard)
/// - Inverted VWI: high bit SET = last byte (used by kindlegen for tag values)

/// Encode an integer as a forward variable-width integer.
///
/// Each byte uses 7 data bits. High bit SET = more bytes follow.
/// Used for INDX header count fields.
#[allow(dead_code)]
pub fn encode_vwi(value: u32) -> Vec<u8> {
    if value < 0x80 {
        return vec![value as u8];
    }

    let mut result = Vec::new();
    let mut v = value;
    while v > 0 {
        result.push((v & 0x7F) as u8);
        v >>= 7;
    }
    result.reverse();
    // Set high bit on all bytes except the last
    let len = result.len();
    for b in result.iter_mut().take(len - 1) {
        *b |= 0x80;
    }
    result
}

/// Encode an integer as an inverted variable-width integer.
///
/// Each byte uses 7 data bits. High bit SET = last byte (stop).
/// High bit CLEAR = more bytes follow. This is the convention used
/// by kindlegen for tag values in INDX data record entries.
pub fn encode_vwi_inv(value: u32) -> Vec<u8> {
    let mut result = Vec::new();
    let mut v = value;
    loop {
        result.push((v & 0x7F) as u8);
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    result.reverse();
    // Set high bit on the LAST byte only (inverted from standard VWI)
    let last = result.len() - 1;
    result[last] |= 0x80;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_vwi_small() {
        assert_eq!(encode_vwi(0), vec![0x00]);
        assert_eq!(encode_vwi(1), vec![0x01]);
        assert_eq!(encode_vwi(127), vec![0x7F]);
        println!("  \u{2713} VWI encode: 0->[00], 1->[01], 127->[7F]");
    }

    #[test]
    fn test_encode_vwi_large() {
        assert_eq!(encode_vwi(128), vec![0x81, 0x00]);
        assert_eq!(encode_vwi(300), vec![0x82, 0x2C]);
        println!("  \u{2713} VWI encode: 128->[81,00], 300->[82,2C]");
    }

    #[test]
    fn test_encode_vwi_inv_small() {
        assert_eq!(encode_vwi_inv(0), vec![0x80]);
        assert_eq!(encode_vwi_inv(1), vec![0x81]);
        assert_eq!(encode_vwi_inv(127), vec![0xFF]);
        println!("  \u{2713} VWI inv encode: 0->[80], 1->[81], 127->[FF]");
    }

    #[test]
    fn test_encode_vwi_inv_large() {
        assert_eq!(encode_vwi_inv(128), vec![0x01, 0x80]);
        assert_eq!(encode_vwi_inv(300), vec![0x02, 0xAC]);
        println!("  \u{2713} VWI inv encode: 128->[01,80], 300->[02,AC]");
    }
}
