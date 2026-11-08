//! Ranges an input method gives in UTF-16 code units, as UTF-8 byte ranges of oso's text.
//! Another process's numbers are not trusted: each is cut to the text and to character
//! boundaries.
//!
//! Must not depend on: any backend or OS binding; plain functions, tested without a window.

use std::ops::Range;

/// The UTF-8 byte range of `text` that a range of UTF-16 code units names, cut to the text
/// and to character boundaries. An input method sends these, and another process's numbers
/// are not trusted.
pub(crate) fn utf8_range(text: &str, utf16: Range<usize>) -> Range<usize> {
    let byte_at = |units: usize| {
        let mut counted = 0;
        for (byte, c) in text.char_indices() {
            if counted >= units {
                return byte;
            }
            counted += c.len_utf16();
        }
        text.len()
    };
    let start = byte_at(utf16.start);
    start..byte_at(utf16.end).max(start)
}

/// How many UTF-16 code units `text` has, as AppKit counts ranges.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::{utf8_range, utf16_len};

    #[test]
    fn utf16_ranges_become_byte_ranges_on_boundaries() {
        assert_eq!(utf8_range("かな", 1..2), 3..6);
        assert_eq!(utf8_range("かな", 2..2), 6..6, "the caret at the end");
        assert_eq!(utf8_range("a😀b", 1..3), 1..5, "a surrogate pair");
        assert_eq!(
            utf8_range("a😀b", 2..3),
            5..5,
            "inside a pair, cut to a boundary"
        );
        assert_eq!(utf8_range("ab", 5..9), 2..2, "past the end");
        let (start, end) = (2, 1);
        assert_eq!(utf8_range("ab", start..end), 2..2, "reversed");
        assert_eq!(utf16_len("a😀か"), 4);
    }
}
