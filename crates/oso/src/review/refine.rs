//! Intra-line refinement (ADR 0122, "Intra-line refinement"; ADR 0127, step 1c): within a
//! change block, removed and added lines are paired in order and diffed by words with
//! `similar`, and the bytes that differ are what refinement marks. A git status uses it now,
//! and a review's view will.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this works on bytes, on the
//! thread that lays a text out.
//!
//! - A word is a run of letters, digits, `_` and bytes past ASCII, so that a character of
//!   several bytes stays whole; white space runs are words too, and any other byte is one.
//! - Only blocks of at most [`MAX_SIDE`] lines a side, and lines of at most [`MAX_LINE`] bytes,
//!   are refined, each pair within [`DEADLINE`]. A pair that shares no word but white space is
//!   left plain: all of it changed, which its line's face says already.

use std::ops::Range;
use std::time::{Duration, Instant};

use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};

/// The spans of bytes of one line that refinement marks, in order.
pub(crate) type Spans = Vec<Range<usize>>;

/// The most lines a side of a block may have to be refined.
pub(crate) const MAX_SIDE: usize = 100;
/// The most bytes a line may have to be refined.
pub(crate) const MAX_LINE: usize = 1024;
/// How long one pair of lines may take.
pub(crate) const DEADLINE: Duration = Duration::from_millis(5);

/// The spans of each removed and each added line that refinement marks, by byte from the line's
/// start, a line's line break left out; an empty list for a line left plain.
pub(crate) fn refine(removed: &[&[u8]], added: &[&[u8]]) -> (Vec<Spans>, Vec<Spans>) {
    let mut old = vec![Vec::new(); removed.len()];
    let mut new = vec![Vec::new(); added.len()];
    if removed.len() > MAX_SIDE || added.len() > MAX_SIDE {
        return (old, new);
    }
    for (at, (from, to)) in removed.iter().zip(added).enumerate() {
        if let Some((a, b)) = pair(trim(from), trim(to)) {
            (old[at], new[at]) = (a, b);
        }
    }
    (old, new)
}

/// A line without its line break.
fn trim(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The spans that differ between `old` and `new`, or `None` to leave them plain.
fn pair(old: &[u8], new: &[u8]) -> Option<(Spans, Spans)> {
    if old.len() > MAX_LINE || new.len() > MAX_LINE {
        return None;
    }
    let (a, b) = (words(old), words(new));
    let deadline = Instant::now() + DEADLINE;
    let texts = |line: &'_ [u8], spans: &[Range<usize>]| -> Vec<Vec<u8>> {
        spans
            .iter()
            .map(|span| line[span.clone()].to_vec())
            .collect()
    };
    let (ta, tb) = (texts(old, &a), texts(new, &b));
    let ops = capture_diff_slices_deadline(Algorithm::Myers, &ta, &tb, Some(deadline));
    if Instant::now() >= deadline {
        return None;
    }
    let mut shared = false;
    let (mut out_old, mut out_new) = (Vec::new(), Vec::new());
    for op in ops {
        if let DiffOp::Equal { old_index, len, .. } = op {
            shared |= ta[old_index..old_index + len]
                .iter()
                .any(|word| !word.iter().all(u8::is_ascii_whitespace));
            continue;
        }
        let (from, to) = (op.old_range(), op.new_range());
        push(&mut out_old, &a, from);
        push(&mut out_new, &b, to);
    }
    shared.then_some((out_old, out_new))
}

/// Adds the bytes of the words `range` of `spans` to `out`, joined to the span before if they
/// meet.
fn push(out: &mut Vec<Range<usize>>, spans: &[Range<usize>], range: Range<usize>) {
    if range.is_empty() {
        return;
    }
    let bytes = spans[range.start].start..spans[range.end - 1].end;
    match out.last_mut() {
        Some(last) if last.end == bytes.start => last.end = bytes.end,
        _ => out.push(bytes),
    }
}

/// The words of `line`, as spans of bytes, in order, covering it whole.
fn words(line: &[u8]) -> Vec<Range<usize>> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Word,
        Space,
        Other,
    }
    let class = |byte: u8| {
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80 {
            Class::Word
        } else if byte.is_ascii_whitespace() {
            Class::Space
        } else {
            Class::Other
        }
    };
    let mut out: Vec<Range<usize>> = Vec::new();
    for (at, &byte) in line.iter().enumerate() {
        let kind = class(byte);
        match out.last_mut() {
            Some(last) if kind != Class::Other && class(line[last.start]) == kind => {
                last.end = at + 1;
            }
            _ => out.push(at..at + 1),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::{MAX_LINE, refine, words};

    #[test]
    fn a_pair_marks_the_words_that_changed() {
        let (old, new) = refine(
            &[b"let total = sum(a, b);\n"],
            &[b"let total = add(a, c);\n"],
        );
        assert_eq!(old, [vec![12..15, 19..20]]);
        assert_eq!(new, [vec![12..15, 19..20]]);
    }

    #[test]
    fn a_character_of_several_bytes_stays_whole() {
        let old = "名前 = 古い\n".as_bytes();
        let new = "名前 = 新しい\n".as_bytes();
        let (a, b) = refine(&[old], &[new]);
        assert_eq!(a[0].as_slice(), std::slice::from_ref(&(9..15)));
        assert_eq!(b[0].as_slice(), std::slice::from_ref(&(9..18)));
    }

    #[test]
    fn lines_with_nothing_in_common_too_long_or_unpaired_stay_plain() {
        let (a, b) = refine(&[b"alpha beta\n", b"extra\n"], &[b"gamma delta\n"]);
        assert!(a.iter().chain(&b).all(Vec::is_empty));
        assert_eq!((a.len(), b.len()), (2, 1));
        let long = vec![b'x'; MAX_LINE + 1];
        let (a, _) = refine(&[&long], &[b"x\n"]);
        assert!(a[0].is_empty());
        let many: Vec<&[u8]> = vec![b"a b\n"; super::MAX_SIDE + 1];
        let (a, _) = refine(&many, &[b"a c\n"]);
        assert!(a.iter().all(Vec::is_empty));
    }

    proptest! {
        #[test]
        fn words_cover_a_line_and_spans_stay_inside_it(
            old in proptest::collection::vec(any::<u8>(), 0..80),
            new in proptest::collection::vec(any::<u8>(), 0..80),
        ) {
            let spans = words(&old);
            let mut at = 0;
            for span in &spans {
                prop_assert_eq!(span.start, at);
                at = span.end;
            }
            prop_assert_eq!(at, old.len());
            let (a, b) = refine(&[&old], &[&new]);
            for (spans, line) in [(&a[0], &old), (&b[0], &new)] {
                for span in spans {
                    prop_assert!(span.start < span.end && span.end <= line.len());
                }
            }
        }
    }
}
