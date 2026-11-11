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

/// How many bytes are read back at a time.
const WINDOW: usize = 256;

fn is_blank(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

/// Where the run of bytes for which `blank` holds that ends at `end` starts.
fn run_before(text: &Rope, end: usize, blank: impl Fn(u8) -> bool) -> usize {
    let mut bytes = Vec::with_capacity(WINDOW);
    let mut at = end;
    while at > 0 {
        let from = text.clip_offset(at.saturating_sub(WINDOW), Bias::Left);
        bytes.clear();
        for chunk in text.chunks_in_range(from..at) {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        let run = bytes.iter().rev().take_while(|&&byte| blank(byte)).count();
        if run < bytes.len() {
            return at - run;
        }
        at = from;
    }
    0
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

/// The trailing whitespace of each line `ranges` touch, ascending and apart, to delete. A
/// cursor in `keep` keeps what is before it on its line. With `rules.final_lines`, the
/// blank lines at the end go too, if a range reaches them. `ranges` and `keep` ascend.
pub(crate) fn trailing_whitespace(
    text: &Rope,
    ranges: &[Range<usize>],
    keep: &[usize],
    rules: TrimRules,
) -> Vec<Range<usize>> {
    let mut ends = Vec::new();
    for range in ranges {
        let range =
            text.clip_offset(range.start, Bias::Left)..text.clip_offset(range.end, Bias::Left);
        line_ends(text, &range, &mut ends);
    }
    ends.sort_unstable();
    ends.dedup();
    let mut deletions: Vec<Range<usize>> = Vec::new();
    for end in ends {
        let start = run_before(text, end, is_blank);
        if start == end {
            continue;
        }
        let after_text = start > 0 && run_before(text, start, |byte| byte == b'\n') == start;
        let spaces = text
            .chunks_in_range(start..end)
            .all(|chunk| chunk.bytes().all(|b| b == b' '));
        if rules.markdown && after_text && spaces && end - start >= 2 {
            continue;
        }
        // The last cursor in the run keeps what is before it.
        let first = keep.partition_point(|&cursor| cursor < start);
        let last = keep.partition_point(|&cursor| cursor <= end);
        let from = if first < last { keep[last - 1] } else { start };
        if from < end {
            deletions.push(from..end);
        }
    }
    if rules.final_lines
        && let Some(blank) = final_blank_lines(text)
        && ranges.iter().any(|range| range.end >= blank.start)
    {
        // A blank line there was a run of its own.
        deletions.retain(|deletion| deletion.start < blank.start);
        deletions.push(blank);
    }
    deletions
}

/// The blank lines at the end of `text`, after the line break that ends its last line with
/// something on it: `None` if there are none, or nothing but whitespace.
fn final_blank_lines(text: &Rope) -> Option<Range<usize>> {
    let content_end = run_before(text, text.len(), |byte| {
        is_blank(byte) || byte == b'\r' || byte == b'\n'
    });
    if content_end == 0 {
        return None;
    }
    let row = text.offset_to_point(content_end).row;
    let after_break = text.line_start(row + 1);
    (after_break < text.len() && after_break > content_end).then(|| after_break..text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: (usize, usize) = (0, usize::MAX);

    /// `text` cleaned on `ranges`, each a start and an end, clamped to it.
    fn trimmed(text: &str, ranges: &[(usize, usize)], keep: &[usize], rules: TrimRules) -> String {
        let rope = Rope::from(text);
        let ranges: Vec<_> = ranges
            .iter()
            .map(|&(start, end)| start.min(text.len())..end.min(text.len()))
            .collect();
        let mut out = text.to_owned();
        for deletion in trailing_whitespace(&rope, &ranges, keep, rules)
            .iter()
            .rev()
        {
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
}
