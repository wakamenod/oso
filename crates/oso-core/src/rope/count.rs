//! Counting a stretch of text for its summary in one pass over its bytes: line breaks,
//! characters and the characters UTF-16 writes as surrogate pairs (ADR 0138).
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.

/// What one pass over a text's bytes counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Counts {
    /// `\n` bytes: `\r\n` holds one, and a lone `\r` is no break.
    pub(super) lines: usize,
    /// Unicode scalar values: the bytes that do not continue one.
    pub(super) chars: usize,
    /// Scalars of four UTF-8 bytes, which UTF-16 writes as two units: the only ones whose
    /// first byte is 0xF0 or more.
    pub(super) four_byte_chars: usize,
}

/// Bytes counted with 8-bit counters before they are added up, so the loop runs on vector
/// lanes; no counter can pass 255 in a block.
const BLOCK: usize = 255;

/// Counts `text`.
pub(super) fn counts(text: &str) -> Counts {
    let mut counts = Counts::default();
    for block in text.as_bytes().chunks(BLOCK) {
        let (mut lines, mut chars, mut four) = (0u8, 0u8, 0u8);
        for &byte in block {
            lines += u8::from(byte == b'\n');
            chars += u8::from(byte & 0xC0 != 0x80);
            four += u8::from(byte >= 0xF0);
        }
        counts.lines += usize::from(lines);
        counts.chars += usize::from(chars);
        counts.four_byte_chars += usize::from(four);
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::super::tests::text_strategy;
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn blocks_add_up() {
        let text = "a\n😀日".repeat(300);
        let counts = counts(&text);
        assert_eq!(counts.lines, 300);
        assert_eq!(counts.chars, 1200);
        assert_eq!(counts.four_byte_chars, 300);
        assert_eq!(super::counts(""), Counts::default());
    }

    proptest! {
        #[test]
        fn counts_match_naive_ones(text in text_strategy(200)) {
            let counts = counts(&text);
            prop_assert_eq!(counts.lines, text.matches('\n').count());
            prop_assert_eq!(counts.chars, text.chars().count());
            prop_assert_eq!(counts.chars + counts.four_byte_chars, text.encode_utf16().count());
        }
    }
}
