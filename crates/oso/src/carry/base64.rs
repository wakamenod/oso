//! Base64, standard alphabet with padding: how an item carries a terminal's state, which is
//! bytes, in the text that items are (ADR 0120, step 11). Run off the UI thread.
//!
//! Must not depend on: anything but std.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// What each byte is worth in base64, or `NOT` for one that is not in the alphabet.
const NOT: u8 = 0xff;
const VALUES: [u8; 256] = {
    let mut values = [NOT; 256];
    let mut at = 0;
    while at < 64 {
        values[ALPHABET[at] as usize] = at as u8;
        at += 1;
    }
    values
};

/// How long `len` bytes are in base64.
pub fn base64_len(len: usize) -> usize {
    len.div_ceil(3) * 4
}

/// `bytes` in base64, with padding.
pub fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(base64_len(bytes.len()));
    for chunk in bytes.chunks(3) {
        let at = |i: usize| chunk.get(i).copied().map_or(0, u32::from);
        let n = (at(0) << 16) | (at(1) << 8) | at(2);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> shift) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The bytes `text` holds in base64, or `None` if it is not that.
pub fn unbase64(text: &str) -> Option<Vec<u8>> {
    let text = text.as_bytes();
    if !text.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let quads = text.len() / 4;
    for (n, quad) in text.chunks_exact(4).enumerate() {
        let padding = quad.iter().rev().take_while(|&&c| c == b'=').count();
        if padding > 2 || (padding > 0 && n + 1 != quads) {
            return None;
        }
        let mut bits = 0u32;
        for &c in &quad[..4 - padding] {
            let value = VALUES[usize::from(c)];
            if value == NOT {
                return None;
            }
            bits = (bits << 6) | u32::from(value);
        }
        bits <<= 6 * padding;
        let bytes = bits.to_be_bytes();
        out.extend_from_slice(&bytes[1..4 - padding]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::{base64, base64_len, unbase64};

    #[test]
    fn base64_goes_both_ways_and_refuses_what_it_is_not() {
        for len in 0..10 {
            let bytes: Vec<u8> = (0..len).map(|n| (n * 37 + 200) as u8).collect();
            let text = base64(&bytes);
            assert_eq!(text.len(), base64_len(len));
            assert_eq!(unbase64(&text).unwrap(), bytes);
        }
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        for bad in ["TWF", "TW=u", "T===", "TQ==TWFu", "TW-u"] {
            assert!(unbase64(bad).is_none(), "{bad}");
        }
    }
}
