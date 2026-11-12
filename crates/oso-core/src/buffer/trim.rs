//! Whitespace cleanup (ADR 0113): the spaces and tabs at the ends of the lines some ranges
//! touch, and the blank lines at the end of a text, as deletions to make.
//!
//! Must not depend on: `window`, `editor` or `command`; this sees only text. It costs the
//! ranges' length plus a seek for each, and the whitespace it reads back over.

use std::ops::Range;

use crate::rope::Rope;
use crate::sum_tree::Bias;

/// What a cleanup keeps, and how far it goes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TrimRules {
    /// Two spaces or more after text end a line in Markdown, and stay.
    pub(crate) markdown: bool,
    /// The blank lines at the end go too, down to one line break, if a range reaches them.
    pub(crate) final_lines: bool,
}

/// What a cleanup found: what to delete, and the whitespace a cursor kept, which stays edited
/// so that a later save cleans it once the cursor has gone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Trim {
    /// The ranges to delete, ascending and apart.
    pub(crate) deletions: Vec<Range<usize>>,
    /// The whitespace kept before a cursor, ascending and apart, in the offsets before the
    /// deletions.
    pub(crate) kept: Vec<Range<usize>>,
}

/// The cleanup would read back over more than its budget allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TooLong;

/// How many more bytes a cleanup may read back over, if it has a bound.
pub(crate) struct Reads(Option<usize>);

impl Reads {
    /// No bound: on a thread of its own.
    pub(crate) fn unbounded() -> Self {
        Self(None)
    }

    /// At most `bytes`, as on the UI thread.
    pub(crate) fn at_most(bytes: usize) -> Self {
        Self(Some(bytes))
    }

    fn take(&mut self, bytes: usize) -> Result<(), TooLong> {
        match &mut self.0 {
            Some(left) if *left < bytes => Err(TooLong),
            Some(left) => {
                *left -= bytes;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

fn is_blank(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

/// Where the run of bytes for which `blank` holds that ends at `end` starts. It reads back a
/// few bytes first, and twice as many each time after, so that a short run costs little.
fn run_before(
    text: &Rope,
    end: usize,
    blank: impl Fn(u8) -> bool,
    reads: &mut Reads,
) -> Result<usize, TooLong> {
    let mut window = 16;
    let mut at = end;
    while at > 0 {
        let from = text.clip_offset(at.saturating_sub(window), Bias::Left);
        reads.take(at - from)?;
        let mut run = 0;
        let mut all = true;
        // Chunks come in order; the run is what ends the last one.
        for chunk in text.chunks_in_range(from..at) {
            let bytes = chunk.as_bytes();
            let tail = bytes.iter().rev().take_while(|&&byte| blank(byte)).count();
            if tail == bytes.len() {
                run += tail;
            } else {
                run = tail;
                all = false;
            }
        }
        if !all || from == 0 {
            return Ok(at - run);
        }
        at = from;
        window = (window * 2).min(4096);
    }
    Ok(0)
}

/// The byte before `offset`, if any.
fn byte_before(text: &Rope, offset: usize) -> Option<u8> {
    let from = text.clip_offset(offset.checked_sub(1)?, Bias::Left);
    text.chunks_in_range(from..offset)
        .last()
        .and_then(|chunk| chunk.bytes().last())
}

/// The end of the line `offset` is on, before its line break.
fn line_end_at(text: &Rope, offset: usize) -> usize {
    text.line_end(text.offset_to_point(offset).row)
}

/// The ends of the lines `range` touches, before their line breaks, ascending. A range
/// that ends right after a line break does not touch the line after it.
fn line_ends(text: &Rope, range: &Range<usize>, ends: &mut Vec<usize>) {
    let mut at = range.start;
    let mut before = None;
    for chunk in text.chunks_in_range(range.clone()) {
        for (index, byte) in chunk.bytes().enumerate() {
            if byte == b'\n' {
                let newline = at + index;
                let end = if before == Some(b'\r') {
                    newline - 1
                } else {
                    newline
                };
                ends.push(end);
            }
            before = Some(byte);
        }
        at += chunk.len();
    }
    if range.is_empty() || before != Some(b'\n') {
        ends.push(line_end_at(text, range.end));
    }
}

/// The trailing whitespace of each line `ranges` touch, to delete. A cursor in `keep` keeps
/// what is before it on its line. With `rules.final_lines`, the blank lines at the end go
/// too, if a range reaches them, but for what a cursor keeps there. `ranges` and `keep`
/// ascend. Reading back over whitespace counts against `reads`.
pub(crate) fn trailing_whitespace(
    text: &Rope,
    ranges: &[Range<usize>],
    keep: &[usize],
    rules: TrimRules,
    reads: &mut Reads,
) -> Result<Trim, TooLong> {
    let mut ends = Vec::new();
    for range in ranges {
        let range =
            text.clip_offset(range.start, Bias::Left)..text.clip_offset(range.end, Bias::Left);
        line_ends(text, &range, &mut ends);
    }
    ends.sort_unstable();
    ends.dedup();
    let mut trim = Trim::default();
    for end in ends {
        let start = run_before(text, end, is_blank, reads)?;
        if start == end {
            continue;
        }
        let after_text = byte_before(text, start).is_some_and(|byte| byte != b'\n');
        let spaces = text
            .chunks_in_range(start..end)
            .all(|chunk| chunk.bytes().all(|b| b == b' '));
        if rules.markdown && after_text && spaces && end - start >= 2 {
            continue;
        }
        let from = last_cursor_in(keep, start..end).unwrap_or(start);
        if from > start {
            trim.kept.push(start..from);
        }
        if from < end {
            trim.deletions.push(from..end);
        }
    }
    if rules.final_lines
        && let Some(blank) = final_blank_lines(text, reads)?
        && ranges.iter().any(|range| range.end >= blank.start)
    {
        // A blank line there was a run of its own.
        trim.deletions
            .retain(|deletion| deletion.start < blank.start);
        trim.kept.retain(|kept| kept.start < blank.start);
        match last_cursor_in(keep, blank.clone()) {
            // The whitespace before it on its line stays; the blank lines before and after
            // its line go.
            Some(cursor) => {
                let line = text.line_start(text.offset_to_point(cursor).row);
                if line > blank.start {
                    trim.deletions.push(blank.start..line);
                }
                if cursor > line {
                    trim.kept.push(line..cursor);
                }
                if cursor < blank.end {
                    trim.deletions.push(cursor..blank.end);
                }
            }
            None => trim.deletions.push(blank),
        }
    }
    Ok(trim)
}

/// How far apart two deletions may be and still go in one span of [`as_spans`].
const SPAN_GAP: usize = 4096;

/// `deletions`, ascending and apart, as fewer replacements: each span from a deletion's
/// start to a later one's end, where no gap between them passes [`SPAN_GAP`], with its text
/// less what they delete. Many deletions then cost a few splices.
pub(crate) fn as_spans(text: &Rope, deletions: &[Range<usize>]) -> Vec<(Range<usize>, String)> {
    let mut spans: Vec<(Range<usize>, String)> = Vec::new();
    let mut from = 0;
    for (index, deletion) in deletions.iter().enumerate() {
        let next = deletions.get(index + 1);
        if next.is_some_and(|next| next.start - deletion.end <= SPAN_GAP) {
            continue;
        }
        let group = &deletions[from..=index];
        let span = group[0].start..deletion.end;
        let mut kept = String::new();
        let mut at = span.start;
        for deleted in group {
            kept.extend(text.chunks_in_range(at..deleted.start));
            at = deleted.end;
        }
        spans.push((span, kept));
        from = index + 1;
    }
    spans
}

/// The last cursor of `keep`, which ascends, within `range`, its ends included.
fn last_cursor_in(keep: &[usize], range: Range<usize>) -> Option<usize> {
    let first = keep.partition_point(|&cursor| cursor < range.start);
    let last = keep.partition_point(|&cursor| cursor <= range.end);
    (first < last).then(|| keep[last - 1])
}

/// The blank lines at the end of `text`, after the line break that ends its last line with
/// something on it: `None` if there are none, or nothing but whitespace.
fn final_blank_lines(text: &Rope, reads: &mut Reads) -> Result<Option<Range<usize>>, TooLong> {
    let content_end = run_before(
        text,
        text.len(),
        |byte| is_blank(byte) || byte == b'\r' || byte == b'\n',
        reads,
    )?;
    if content_end == 0 {
        return Ok(None);
    }
    let row = text.offset_to_point(content_end).row;
    let after_break = text.line_start(row + 1);
    Ok((after_break < text.len() && after_break > content_end).then(|| after_break..text.len()))
}

#[cfg(test)]
#[expect(
    clippy::single_range_in_vec_init,
    reason = "lists of ranges, which may hold one"
)]
mod tests {
    use super::*;

    /// A length past the windows reads back in, for runs that take several.
    const WINDOW: usize = 8192;

    const ALL: (usize, usize) = (0, usize::MAX);

    /// `text` cleaned on `ranges`, each a start and an end, clamped to it.
    fn trimmed(text: &str, ranges: &[(usize, usize)], keep: &[usize], rules: TrimRules) -> String {
        let rope = Rope::from(text);
        let ranges: Vec<_> = ranges
            .iter()
            .map(|&(start, end)| start.min(text.len())..end.min(text.len()))
            .collect();
        let mut out = text.to_owned();
        let trim =
            trailing_whitespace(&rope, &ranges, keep, rules, &mut Reads::unbounded()).unwrap();
        for deletion in trim.deletions.iter().rev() {
            out.replace_range(deletion.clone(), "");
        }
        out
    }

    fn all(text: &str) -> String {
        trimmed(
            text,
            &[ALL],
            &[],
            TrimRules {
                markdown: false,
                final_lines: true,
            },
        )
    }

    #[test]
    fn spaces_and_tabs_go_from_the_ends_of_lines() {
        assert_eq!(all("a  \nb\t \r\nc \u{3000}\nd  "), "a\nb\r\nc \u{3000}\nd");
    }

    #[test]
    fn only_the_lines_a_range_touches() {
        let text = "a  \nb  \nc  \n";
        // An edit on the second line, and a deletion at the third's start.
        assert_eq!(
            trimmed(text, &[(5, 6), (8, 8)], &[], TrimRules::default()),
            "a  \nb\nc\n"
        );
        // A range that ends after a line break leaves the line after it.
        assert_eq!(
            trimmed(text, &[(4, 8)], &[], TrimRules::default()),
            "a  \nb\nc  \n"
        );
    }

    #[test]
    fn a_cursor_keeps_what_is_before_it_on_its_line() {
        let text = "foo   \nbar  \n";
        assert_eq!(
            trimmed(text, &[ALL], &[4], TrimRules::default()),
            "foo \nbar\n"
        );
        assert_eq!(
            trimmed(text, &[ALL], &[6], TrimRules::default()),
            "foo   \nbar\n"
        );
    }

    #[test]
    fn markdown_keeps_two_spaces_after_text() {
        let rules = TrimRules {
            markdown: true,
            final_lines: false,
        };
        let text = "line  \nlast \nend \t \n   \n";
        assert_eq!(trimmed(text, &[ALL], &[], rules), "line  \nlast\nend\n\n");
    }

    #[test]
    fn blank_lines_at_the_end_go_down_to_one_line_break() {
        assert_eq!(all("a\n\n \n\t\n"), "a\n");
        assert_eq!(all("a  \r\n\r\n"), "a\r\n");
        assert_eq!(all("a\n"), "a\n");
        assert_eq!(all("a"), "a");
        // Nothing but whitespace stays as lines with nothing on them.
        assert_eq!(all(" \n\n"), "\n\n");
        // Only if a range reaches them.
        let text = "a  \n\n\n";
        let rules = TrimRules {
            markdown: false,
            final_lines: true,
        };
        assert_eq!(trimmed(text, &[(0, 1)], &[], rules), "a\n\n\n");
        assert_eq!(trimmed(text, &[(5, 5)], &[], rules), "a  \n");
    }

    #[test]
    fn long_runs_are_read_back_window_by_window() {
        let text = format!("x{}\n", " ".repeat(3 * WINDOW + 7));
        assert_eq!(all(&text), "x\n");
        let japanese = format!("日本{}\n", " ".repeat(WINDOW));
        assert_eq!(all(&japanese), "日本\n");
    }

    #[test]
    fn what_a_cursor_keeps_is_said_so_it_stays_edited() {
        let rope = Rope::from("x foo  \nbar  \n");
        let rules = TrimRules::default();
        let trim =
            trailing_whitespace(&rope, &[0..14], &[6], rules, &mut Reads::unbounded()).unwrap();
        assert_eq!(trim.deletions, [6..7, 11..13]);
        assert_eq!(trim.kept, [5..6]);
    }

    #[test]
    fn a_cursor_in_the_blank_lines_at_the_end_keeps_its_line() {
        let rules = TrimRules {
            markdown: false,
            final_lines: true,
        };
        assert_eq!(trimmed("a\n\n    ", &[ALL], &[7], rules), "a\n    ");
        assert_eq!(trimmed("a\n\n  \n\n", &[ALL], &[5], rules), "a\n  ");
        let rope = Rope::from("a\n\n    ");
        let trim =
            trailing_whitespace(&rope, &[0..7], &[7], rules, &mut Reads::unbounded()).unwrap();
        assert_eq!(trim.kept, [3..7]);
    }

    #[test]
    fn reading_back_past_the_budget_says_so() {
        let text = format!("x{}", "\n".repeat(10 * WINDOW));
        let rope = Rope::from(text.as_str());
        let rules = TrimRules {
            markdown: false,
            final_lines: true,
        };
        let end = rope.len();
        let ranges = [end..end];
        let bounded = trailing_whitespace(&rope, &ranges, &[], rules, &mut Reads::at_most(WINDOW));
        assert_eq!(bounded, Err(TooLong));
        let unbounded =
            trailing_whitespace(&rope, &ranges, &[], rules, &mut Reads::unbounded()).unwrap();
        assert_eq!(unbounded.deletions, [2..end]);
        // A long run of line breaks before a line's whitespace is not read.
        let text = format!("{}  ", "\n".repeat(10 * WINDOW));
        let rope = Rope::from(text.as_str());
        let end = rope.len();
        let trim = trailing_whitespace(
            &rope,
            &[end..end],
            &[],
            TrimRules::default(),
            &mut Reads::at_most(WINDOW),
        )
        .unwrap();
        assert_eq!(trim.deletions, [end - 2..end]);
    }
}
