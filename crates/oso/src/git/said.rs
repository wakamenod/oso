//! Text from git's and `gh`'s runs as a person is shown it: any character that would change how
//! the text around it looks, or would not show, escaped, and the length cut.
//!
//! Must not depend on: processes, the file system, `app` or `dispatch`; a plain function over
//! text a run gave.

/// Text safe to show a person: control characters, the line and paragraph separators and the
/// bidirectional controls become `\u{…}` escapes. At most `max` characters of it, then `…`.
pub(crate) fn printable(text: &str, max: usize) -> String {
    use std::fmt::Write;
    let mut shown = String::with_capacity(text.len().min(max.saturating_mul(4)));
    let mut count = 0;
    for c in text.chars() {
        let code = u32::from(c);
        // `\u{`, the hex digits, `}`; or the character itself.
        let width = if hidden(c) {
            4 + (32 - code.leading_zeros()).div_ceil(4).max(1) as usize
        } else {
            1
        };
        if count + width > max {
            shown.push('…');
            return shown;
        }
        count += width;
        if width == 1 {
            shown.push(c);
        } else {
            let _ = write!(shown, "\\u{{{code:x}}}");
        }
    }
    shown
}

/// Whether `c` would change how the text around it looks, or would not show: C0, DEL and C1
/// controls, and what the display boxes (the bidirectional controls, the line and paragraph
/// separators and the other default-ignorable code points, as the zero-width spaces and the tag
/// characters), so that text an agent reads holds none of them unescaped either.
fn hidden(c: char) -> bool {
    c.is_control() || oso_core::display_map::cell::is_invisible(c)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{hidden, printable};

    #[test]
    fn printable_escapes_and_cuts() {
        assert_eq!(printable("plain é 日本", 100), "plain é 日本");
        assert_eq!(printable("a\nb\tc\u{7f}", 100), "a\\u{a}b\\u{9}c\\u{7f}");
        assert_eq!(
            printable("\u{9b}\u{2028}\u{202e}\u{2067}", 100),
            "\\u{9b}\\u{2028}\\u{202e}\\u{2067}"
        );
        assert_eq!(printable("abcdef", 3), "abc…");
        assert_eq!(printable("abc", 3), "abc");
        // An escape is not split.
        assert_eq!(printable("ab\u{1b}", 4), "ab…");
        assert_eq!(
            printable("a\u{200b}b\u{feff}c\u{e0041}", 100),
            "a\\u{200b}b\\u{feff}c\\u{e0041}"
        );
        assert_eq!(printable("x", 0), "…");
    }

    proptest! {
        #[test]
        fn printable_shows_no_control(text in any::<String>(), max in 0usize..64) {
            let shown = printable(&text, max);
            prop_assert!(shown.chars().count() <= max + 1);
            prop_assert!(!shown.chars().any(hidden));
        }
    }
}
