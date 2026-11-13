//! What a paste into a terminal is asked about (ADR 0120, Q13): whether it holds a line break,
//! and how many lines it is, counted as the terminal's thread sends them, a lone carriage
//! return being a line break too.
//!
//! Must not depend on: `app`; this only reads the text.

/// Whether `text` holds a line break a program would take as `RET`.
pub(crate) fn breaks(text: &str) -> bool {
    text.contains('\n') || text.contains('\r')
}

/// The lines `text` is, with `\r\n` and a lone `\r` ending a line as `\n` does.
pub(crate) fn lines(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut ends = 0;
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => {
                ends += 1;
                index += 1;
            }
            b'\r' | b'\n' => ends += 1,
            _ => {}
        }
        index += 1;
    }
    let open_last = !text.is_empty() && !text.ends_with(['\n', '\r']);
    ends + usize::from(open_last)
}

#[cfg(test)]
mod tests {
    use super::{breaks, lines};

    #[test]
    fn line_breaks_of_every_kind_count() {
        assert!(!breaks("one line"));
        assert!(breaks("a\nb") && breaks("a\rb"));
        assert_eq!(lines(""), 0);
        assert_eq!(lines("a"), 1);
        assert_eq!(lines("a\n"), 1);
        assert_eq!(lines("a\nb"), 2);
        assert_eq!(lines("a\r\nb\r\n"), 2);
        assert_eq!(lines("a\rb"), 2);
        assert_eq!(lines("a\r\rb"), 3);
    }
}
