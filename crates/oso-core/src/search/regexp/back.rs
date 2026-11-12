//! A regexp's search backward (ADR 0116), as Emacs's `re-search-backward` takes it: the match
//! that starts last, at a place or before it, and ends by another, one line at a time.
//!
//! Must not depend on: `buffer`, `editor` or a log.
//!
//! - A line is looked in from its end, in windows that double, so a line with no match is
//!   read about twice, and a match near the end is found in the first window.
//! - In the window that holds a match, the last start is found by halving: whether a match
//!   starts at a place or after it (and before the window's end) only turns false as the place
//!   moves on, so a few looks find it, not one a start.
//! - The end bound is a cut, as a range's end is forward (`cut`): a match ends by it, while
//!   `$` and `\b` there see what follows.

use std::ops::Range;

use super::cut::{self, Ending, Unbuilt};
use super::{Effort, Line, LineTooLong, Regexp, char_after, floor_boundary};
use crate::rope::Rope;
use crate::search::{Found, Limits};
use crate::sum_tree::Bias;

/// How many bytes of starts the first window at a line's end holds.
const FIRST_WINDOW: usize = 256;

/// How a look in one line ended.
enum InLine {
    Match(Range<usize>),
    None,
    Stopped,
}

/// The last match of `regexp` in `text` that starts at `start_at_most` or before and ends by
/// `before`, within `limits`. `kept` is a line a search before this one read, which it takes
/// if it is the one to look in, and the line it read last goes back there. [`Found::Stopped`]
/// gives the `before` to go on with, `start_at_most` staying as it was.
///
/// # Errors
///
/// [`LineTooLong`] at a line longer than the regexp is matched in.
pub(crate) fn find_backward(
    regexp: &Regexp,
    text: &Rope,
    (before, start_at_most): (usize, usize),
    (limits, stop): (Limits, &dyn Fn() -> bool),
    kept: &mut Option<Line>,
) -> Result<Found, LineTooLong> {
    let before = text.clip_offset(before.min(text.len()), Bias::Left);
    let start_at_most = text.clip_offset(start_at_most.min(before), Bias::Left);
    let mut effort = Effort::new(limits);
    let mut ending = Ending::default();
    let mut row = text.offset_to_point(start_at_most).row;
    loop {
        let (start, end) = (text.line_start(row), text.line_end(row));
        // The first line ends at `before` if it is in it; the lines before it at their ends.
        let cut = before.min(end);
        if effort.over(stop) {
            return Ok(Found::Stopped { resume: cut });
        }
        let unread = kept.as_ref().is_none_or(|line| line.start != start);
        if unread && !Line::read(text, start, (regexp.longest, &mut effort), kept)? {
            return Ok(Found::Stopped { resume: cut });
        }
        let Some(line) = kept.as_ref() else {
            return Ok(Found::Stopped { resume: cut });
        };
        let last = start_at_most.min(cut) - start;
        let here = (&line.text[..], start, cut - start);
        match last_in_line(regexp, here, last, &mut ending, (&mut effort, stop)) {
            InLine::Match(found) => {
                return Ok(Found::Match(start + found.start..start + found.end));
            }
            InLine::Stopped => return Ok(Found::Stopped { resume: cut }),
            InLine::None => {}
        }
        let Some(above) = row.checked_sub(1) else {
            return Ok(Found::None);
        };
        row = above;
    }
}

/// The match in `line`, a line's text that starts at `line_start`, that starts last at
/// `last` or before and ends by `cut`, looked for from the end in windows that double.
fn last_in_line(
    regexp: &Regexp,
    (line, line_start, cut): (&str, usize, usize),
    last: usize,
    ending: &mut Ending,
    (effort, stop): (&mut Effort, &dyn Fn() -> bool),
) -> InLine {
    // The first match from `from`; `Err` where the search stops here, as `effort` says, or
    // for the worker to build the regexps for a cut.
    let mut look = |from: usize| -> Result<Option<Range<usize>>, Unbuilt> {
        if effort.over(stop) {
            return Err(Unbuilt);
        }
        cut::find(regexp, (line, line_start, cut), from, ending, effort)
    };
    // The starts looked at are below `end`, and at `lo` or after.
    let mut end = last + 1;
    let mut width = FIRST_WINDOW;
    loop {
        let lo = floor_boundary(&line[..cut], (end - 1).saturating_sub(width));
        let mut best = match look(lo) {
            Err(Unbuilt) => return InLine::Stopped,
            Ok(Some(found)) if found.start < end => found,
            Ok(_) if lo == 0 => return InLine::None,
            Ok(_) => {
                end = lo;
                width = width.saturating_mul(2);
                continue;
            }
        };
        // No match starts in `hi..end`; halve what lies between `best` and it.
        let mut hi = end;
        loop {
            let after = char_after(&line[..cut], best.start);
            if after >= hi {
                return InLine::Match(best);
            }
            let mid = floor_boundary(&line[..cut], after + (hi - after) / 2).max(after);
            match look(mid) {
                Err(Unbuilt) => return InLine::Stopped,
                Ok(Some(found)) if found.start < hi => best = found,
                Ok(_) => hi = mid,
            }
        }
    }
}
