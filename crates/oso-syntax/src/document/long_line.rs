//! Long lines: a text with a line longer than [`MAX_LINE_BYTES`] is not parsed, and stays plain
//! (ADR 0147). Such lines are minified or generated code, whose dense tokens make a tree of
//! about 100 times the text, against about 40 for source.
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - The whole text is read once, before a parse from nothing: one pass over its bytes.
//! - An edit is checked on the lines it touched only, as no other line changed.

use oso_core::rope::Rope;
use oso_core::syntax::Splice;

/// The longest line parsed, in bytes, without its line break: a piece's size. Source lines are
/// far shorter, and a minified bundle's run to hundreds of KB (measured in ADR 0147's As built).
pub(crate) const MAX_LINE_BYTES: usize = 16 << 10;

/// Whether some line of `text` is longer than `max` bytes.
pub(super) fn any(text: &Rope, max: usize) -> bool {
    let mut run = 0;
    for chunk in text.chunks() {
        for &byte in chunk.as_bytes() {
            if byte == b'\n' {
                run = 0;
            } else {
                run += 1;
                if run > max {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether a line `splices` touched in `after`, the text they made, is longer than `max` bytes.
/// The splices are ascending and apart, in the text before them.
pub(super) fn touched(after: &Rope, splices: &[Splice], max: usize) -> bool {
    let mut delta = 0isize;
    splices.iter().any(|splice| {
        let start = splice.start.saturating_add_signed(delta).min(after.len());
        delta += splice.new_len as isize - splice.old_len as isize;
        let end = (start + splice.new_len).min(after.len());
        let (first, last) = (
            after.offset_to_point(start).row,
            after.offset_to_point(end).row,
        );
        (first..=last).any(|row| after.line_len(row) > max)
    })
}

#[cfg(test)]
mod tests {
    use oso_core::rope::Rope;
    use oso_core::syntax::Splice;

    use super::{MAX_LINE_BYTES, any, touched};

    #[test]
    fn a_line_past_the_limit_is_found_in_any_chunk() {
        let at_limit = "x".repeat(MAX_LINE_BYTES);
        let any = |text: &str| any(&Rope::from(text), MAX_LINE_BYTES);
        assert!(!any(&format!("a\n{at_limit}\nb")));
        let past = format!("{}\n{at_limit}y\n", "short line\n".repeat(500));
        assert!(any(&past));
        // The last line, with no break after it.
        assert!(any(&format!("a\n{at_limit}y")));
    }

    #[test]
    fn an_edit_is_judged_on_the_lines_it_touched() {
        let half = "x".repeat(MAX_LINE_BYTES / 2 + 1);
        let before = format!("{half}\n{half}\n");
        // Deleting the line break between the halves makes one line past the limit.
        let joined = format!("{half}{half}\n");
        let splice = Splice {
            start: half.len(),
            old_len: 1,
            new_len: 0,
        };
        assert!(touched(
            &Rope::from(joined.as_str()),
            &[splice],
            MAX_LINE_BYTES
        ));
        // A key elsewhere touches only short lines.
        let typed = format!("{half}\n{half}\nz");
        let key = Splice {
            start: before.len(),
            old_len: 0,
            new_len: 1,
        };
        assert!(!touched(
            &Rope::from(typed.as_str()),
            &[key],
            MAX_LINE_BYTES
        ));
    }
}
