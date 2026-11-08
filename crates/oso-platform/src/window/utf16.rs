//! Ranges an input method gives in UTF-16 code units, as UTF-8 byte ranges and character
//! ranges of oso's text, found in one walk. Another process's numbers are not trusted: each
//! is cut to the text and to character boundaries.
//!
//! Must not depend on: any backend or OS binding; plain functions, tested without a window.

use std::ops::Range;

/// Where a range of UTF-16 code units falls in a text: its bytes, characters and units, each
/// cut to the text and rounded up to a character boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Located {
    pub(crate) bytes: Range<usize>,
    pub(crate) chars: Range<usize>,
    pub(crate) units: Range<usize>,
    /// The range starts past the text's end.
    pub(crate) past_end: bool,
}

/// Where `utf16` falls in `text`, found in one walk that stops at its end. An end before the
/// start counts as the start.
pub(crate) fn locate(text: &str, utf16: Range<usize>) -> Located {
    let end = utf16.end.max(utf16.start);
    // The place after the characters walked so far: bytes, characters and units.
    let (mut byte, mut chars, mut units) = (0, 0, 0);
    let mut start = None;
    let mut rest = text.chars();
    loop {
        if start.is_none() && units >= utf16.start {
            start = Some((byte, chars, units));
        }
        if start.is_some() && units >= end {
            break;
        }
        let Some(c) = rest.next() else {
            break;
        };
        byte += c.len_utf8();
        chars += 1;
        units += c.len_utf16();
    }
    let past_end = start.is_none();
    let (start_byte, start_char, start_unit) = start.unwrap_or((byte, chars, units));
    Located {
        bytes: start_byte..byte,
        chars: start_char..chars,
        units: start_unit..units,
        past_end,
    }
}

/// The UTF-8 byte range of `text` that a range of UTF-16 code units names, as [`locate`]
/// finds it. An input method sends these.
pub(crate) fn utf8_range(text: &str, utf16: Range<usize>) -> Range<usize> {
    locate(text, utf16).bytes
}

/// How many UTF-16 code units `text` has, as AppKit counts ranges.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::{Located, locate, utf8_range, utf16_len};

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

    #[test]
    fn utf16_ranges_are_located_in_one_walk() {
        let at = |bytes, chars, units| Located {
            bytes,
            chars,
            units,
            past_end: false,
        };
        assert_eq!(locate("かな", 1..2), at(3..6, 1..2, 1..2));
        assert_eq!(
            locate("a😀b", 1..3),
            at(1..5, 1..2, 1..3),
            "a surrogate pair"
        );
        assert_eq!(locate("a😀b", 2..3), at(5..5, 2..2, 3..3), "inside a pair");
        assert_eq!(
            locate("ab", 2..2),
            at(2..2, 2..2, 2..2),
            "the caret at the end"
        );
        assert_eq!(
            locate("ab", 1..usize::MAX),
            at(1..2, 1..2, 1..2),
            "cut to it"
        );
        assert!(locate("ab", 3..4).past_end);
        assert!(!locate("", 0..0).past_end);
    }

    #[test]
    fn any_range_stays_inside_the_text() {
        // As another process may give: ends near the text's and the largest there is.
        let text = "a😀かe\u{301}";
        let (chars, units) = (text.chars().count(), utf16_len(text));
        for start in (0..=units + 1).chain([usize::MAX]) {
            for end in (0..=units + 1).chain([usize::MAX]) {
                let found = locate(text, start..end);
                assert_eq!(found.past_end, start > units, "{start}..{end}");
                let (bytes, chars_found, units_found) = (found.bytes, found.chars, found.units);
                assert!(bytes.start <= bytes.end && bytes.end <= text.len());
                assert!(text.is_char_boundary(bytes.start) && text.is_char_boundary(bytes.end));
                assert!(chars_found.start <= chars_found.end && chars_found.end <= chars);
                assert!(units_found.start <= units_found.end && units_found.end <= units);
            }
        }
    }
}
