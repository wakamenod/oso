//! Intra-line refinement (ADR 0122, "Intra-line refinement"; ADR 0127, step 1c): within a
//! change block, removed and added lines are paired in order and diffed by words with
//! `similar`, and the bytes that differ are what refinement marks. A git status uses it, and a
//! review's thread refines each file's hunks as it reads them ([`Refined`], step 3), within
//! [`FILE_BUDGET`] a file, so that laying a section out again refines nothing.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this works on bytes, on the
//! thread that reads a review's files or lays a git status out.
//!
//! - A word is a run of letters, digits, `_` and bytes past ASCII, so that a character of
//!   several bytes stays whole; white space runs are words too, and any other byte is one.
//! - Only blocks of at most [`MAX_SIDE`] lines a side, and lines of at most [`MAX_LINE`] bytes,
//!   are refined, each pair within [`DEADLINE`] and with at most [`MAX_SPANS`] spans a line. A pair that shares no word but white space is
//!   left plain: all of it changed, which its line's face says already.

use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::review::{Hunk, Side};
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline};

/// The spans of bytes of one line that refinement marks, in order.
pub(crate) type Spans = Vec<Range<usize>>;

/// The most lines a side of a block may have to be refined.
pub(crate) const MAX_SIDE: usize = 100;
/// The most bytes a line may have to be refined.
pub(crate) const MAX_LINE: usize = 1024;
/// How long one pair of lines may take.
#[cfg(not(test))]
pub(crate) const DEADLINE: Duration = Duration::from_millis(5);
/// How long one pair of lines may take in a test: a pair past the time is left plain, and a
/// test thread a loaded machine puts aside for 5 ms would see its lines plain, as a snapshot
/// of a review or a git status then shows. No test times a pair.
#[cfg(test)]
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);
/// The most spans a line may have refined; one with more stays plain, so that a frame draws
/// few of them a row.
pub(crate) const MAX_SPANS: usize = 64;
/// How long refining one file's hunks may take in a review: hunks past it stay plain.
pub(crate) const FILE_BUDGET: Duration = Duration::from_millis(50);

/// The spans of each removed and each added line that refinement marks, by byte from the line's
/// start, a line's line break left out; an empty list for a line left plain.
pub(crate) fn refine(removed: &[&[u8]], added: &[&[u8]]) -> (Vec<Spans>, Vec<Spans>) {
    refine_until(removed, added, None)
}

/// As [`refine`], the pairs met once `deadline` has passed left plain.
fn refine_until(
    removed: &[&[u8]],
    added: &[&[u8]],
    deadline: Option<Instant>,
) -> (Vec<Spans>, Vec<Spans>) {
    let mut old = vec![Vec::new(); removed.len()];
    let mut new = vec![Vec::new(); added.len()];
    if removed.len() > MAX_SIDE || added.len() > MAX_SIDE {
        return (old, new);
    }
    for (at, (from, to)) in removed.iter().zip(added).enumerate() {
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            break;
        }
        let until = deadline.map_or(now + DEADLINE, |deadline| deadline.min(now + DEADLINE));
        if let Some((a, b)) = pair(trim(from), trim(to), until) {
            (old[at], new[at]) = (a, b);
        }
    }
    (old, new)
}

/// What refinement marks in a reading's hunks, each hunk one change block, as a hunk with no
/// context is: by hunk, side and line within the hunk's side, the spans of bytes from the line's
/// start, its line break left out. Lines left plain take no room.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Refined {
    /// Each line with spans, in order of hunk, side (old first) and line, and where its spans
    /// start in `spans`; they end where the next line's start.
    lines: Vec<(u32, bool, u32, u32)>,
    spans: Vec<Range<u32>>,
}

impl Refined {
    /// Refines each of `hunks` that both removes and adds lines, until `deadline`: the hunks
    /// past it stay plain.
    pub(crate) fn of(hunks: &[Hunk], deadline: Instant) -> Refined {
        let mut out = Refined::default();
        for (at, hunk) in hunks.iter().enumerate() {
            if Instant::now() >= deadline {
                break;
            }
            let Ok(at) = u32::try_from(at) else { break };
            let sides = [hunk.removed.len(), hunk.added.len()];
            if sides.contains(&0) || sides.iter().any(|&side| side > MAX_SIDE) {
                continue;
            }
            let removed: Vec<&[u8]> = hunk.removed.iter().map(|line| &**line).collect();
            let added: Vec<&[u8]> = hunk.added.iter().map(|line| &**line).collect();
            let (old, new) = refine_until(&removed, &added, Some(deadline));
            out.add(at, false, &old);
            out.add(at, true, &new);
        }
        out
    }

    fn add(&mut self, hunk: u32, new: bool, lines: &[Spans]) {
        for (line, spans) in lines.iter().enumerate() {
            let Ok(line) = u32::try_from(line) else {
                return;
            };
            if spans.is_empty() {
                continue;
            }
            let start = u32::try_from(self.spans.len()).unwrap_or(u32::MAX);
            self.lines.push((hunk, new, line, start));
            // A line is at most `MAX_LINE` bytes, so each end fits.
            self.spans.extend(spans.iter().map(|span| {
                let at = |byte: usize| u32::try_from(byte).unwrap_or(u32::MAX);
                at(span.start)..at(span.end)
            }));
        }
    }

    /// The spans refinement marks in line `line` of `side` of hunk `hunk`, counted from 0 in
    /// the hunk's lines of that side. O(log lines refined).
    pub(crate) fn line(&self, hunk: usize, side: Side, line: usize) -> &[Range<u32>] {
        let (Ok(hunk), Ok(line)) = (u32::try_from(hunk), u32::try_from(line)) else {
            return &[];
        };
        let key = (hunk, side == Side::New, line);
        let Ok(at) = self
            .lines
            .binary_search_by_key(&key, |&(hunk, new, line, _)| (hunk, new, line))
        else {
            return &[];
        };
        let start = self.lines[at].3 as usize;
        let end = self
            .lines
            .get(at + 1)
            .map_or(self.spans.len(), |next| next.3 as usize);
        self.spans.get(start..end).unwrap_or_default()
    }

    /// The bytes it holds, as a review counts what it keeps.
    pub(crate) fn bytes(&self) -> usize {
        self.lines.len() * std::mem::size_of::<(u32, bool, u32, u32)>()
            + self.spans.len() * std::mem::size_of::<Range<u32>>()
    }
}

/// A line without its line break.
fn trim(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The spans that differ between `old` and `new`, or `None` to leave them plain.
fn pair(old: &[u8], new: &[u8], deadline: Instant) -> Option<(Spans, Spans)> {
    if old.len() > MAX_LINE || new.len() > MAX_LINE {
        return None;
    }
    let (a, b) = (words(old), words(new));
    let ta: Vec<&[u8]> = a.iter().map(|span| &old[span.clone()]).collect();
    let tb: Vec<&[u8]> = b.iter().map(|span| &new[span.clone()]).collect();
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
    let few = out_old.len() <= MAX_SPANS && out_new.len() <= MAX_SPANS;
    (shared && few).then_some((out_old, out_new))
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
    fn a_line_changed_word_by_word_past_the_spans_limit_stays_plain() {
        let old = "a.".repeat(100);
        let new = "b.".repeat(100);
        let (a, b) = refine(&[old.as_bytes()], &[new.as_bytes()]);
        assert!(a[0].is_empty() && b[0].is_empty());
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

    #[test]
    fn a_reading_s_hunks_are_refined_by_hunk_side_and_line_and_past_the_deadline_stay_plain() {
        use std::time::{Duration, Instant};

        use oso_core::review::{Hunk, Side};

        use super::Refined;

        let hunk = |removed: &[&[u8]], added: &[&[u8]]| Hunk {
            old_start: 1,
            new_start: 1,
            removed: removed.iter().map(|line| (*line).into()).collect(),
            added: added.iter().map(|line| (*line).into()).collect(),
        };
        let hunks = [
            hunk(&[b"a b\n"], &[]),
            hunk(&[b"one two\n", b"x y\n"], &[b"one 2\n", b"x z\n"]),
        ];
        let refined = Refined::of(&hunks, Instant::now() + Duration::from_secs(5));
        assert!(
            refined.line(0, Side::Old, 0).is_empty(),
            "a hunk with no added line"
        );
        assert_eq!(refined.line(1, Side::Old, 0), std::slice::from_ref(&(4..7)));
        assert_eq!(refined.line(1, Side::New, 0), std::slice::from_ref(&(4..5)));
        assert_eq!(refined.line(1, Side::Old, 1), std::slice::from_ref(&(2..3)));
        assert_eq!(refined.line(1, Side::New, 1), std::slice::from_ref(&(2..3)));
        assert!(refined.line(1, Side::New, 2).is_empty());
        assert!(refined.line(7, Side::New, 0).is_empty());
        assert!(refined.bytes() > 0);
        let late = Refined::of(&hunks, Instant::now());
        assert_eq!(late, Refined::default());
        assert_eq!(late.bytes(), 0);
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
