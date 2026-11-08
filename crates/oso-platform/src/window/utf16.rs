//! Ranges an input method gives in UTF-16 code units, as UTF-8 byte ranges or character
//! ranges of oso's text.
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

/// The characters of `text` that `utf16` covers, cut to character boundaries as
/// [`utf8_range`] cuts them, with those units as cut. `None` when they start past its end, as
/// a range outside the text being composed does.
#[cfg(all(target_os = "macos", feature = "native-window"))]
pub(crate) fn char_range(text: &str, utf16: Range<usize>) -> Option<(Range<usize>, Range<usize>)> {
    if utf16.start > utf16_len(text) {
        return None;
    }
    // The characters and units before the first boundary at or after `units`.
    let at = |units: usize| {
        let (mut chars, mut counted) = (0, 0);
        for c in text.chars() {
            if counted >= units {
                break;
            }
            counted += c.len_utf16();
            chars += 1;
        }
        (chars, counted)
    };
    let (start, start_units) = at(utf16.start);
    let (end, end_units) = at(utf16.end.max(utf16.start));
    Some((start..end, start_units..end_units))
}

/// How many UTF-16 code units `text` has, as AppKit counts ranges.
pub(crate) fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    #[cfg(all(target_os = "macos", feature = "native-window"))]
    use super::char_range;
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

    #[cfg(all(target_os = "macos", feature = "native-window"))]
    #[test]
    fn utf16_ranges_become_character_ranges_on_boundaries() {
        assert_eq!(char_range("かな", 1..2), Some((1..2, 1..2)));
        assert_eq!(
            char_range("a😀b", 1..3),
            Some((1..2, 1..3)),
            "a surrogate pair"
        );
        assert_eq!(
            char_range("a😀b", 2..3),
            Some((2..2, 3..3)),
            "inside a pair"
        );
        assert_eq!(
            char_range("ab", 2..2),
            Some((2..2, 2..2)),
            "the caret at the end"
        );
        assert_eq!(
            char_range("ab", 1..usize::MAX),
            Some((1..2, 1..2)),
            "cut to it"
        );
        let (start, end) = (2, 1);
        assert_eq!(char_range("ab", start..end), Some((2..2, 2..2)), "reversed");
        assert_eq!(char_range("ab", 3..4), None, "past it");
        assert_eq!(char_range("", 0..0), Some((0..0, 0..0)));
        // Any range, as another process may give, stays inside the text.
        let text = "a😀かe\u{301}";
        let (chars, units) = (text.chars().count(), utf16_len(text));
        for start in (0..=units + 1).chain([usize::MAX]) {
            for end in (0..=units + 1).chain([usize::MAX]) {
                if let Some((c, u)) = char_range(text, start..end) {
                    assert!(c.start <= c.end && c.end <= chars, "{start}..{end}");
                    assert!(u.start <= u.end && u.end <= units, "{start}..{end}");
                } else {
                    assert!(start > units);
                }
            }
        }
    }
}
