//! A regexp's search backward (ADR 0116), as Emacs's `re-search-backward` takes it: the match
//! that starts last, at a place or before it, and ends by another, one line at a time.
//!
//! Must not depend on: `buffer`, `editor` or a log.
//!
//! - A line is looked in from its end, in windows that double, so a line with no match is
//!   read about twice, and a match near the end is found in the first window.
//! - Every start in a window is tried, so a match that starts inside an earlier one is found,
//!   as Emacs tries each place.
//! - The end bound is a cut, as a range's end is forward (`cut`): a match ends by it, while
//!   `$` and `\b` there see what follows.

use std::ops::Range;
use std::time::Instant;

use super::cut::{self, Ending};
use super::{LOOK_EVERY, LineTooLong, PER_LINE, Regexp, char_after};
use crate::rope::Rope;
use crate::search::Found;
use crate::sum_tree::Bias;

/// How many bytes of starts the first window at a line's end holds.
const FIRST_WINDOW: usize = 256;

/// The work a search does, as `Scan` counts it, and when it must stop.
struct Effort<'a> {
    work: usize,
    limit: usize,
    looked: usize,
    until: Option<Instant>,
    stop: &'a dyn Fn() -> bool,
}

impl Effort<'_> {
    /// Whether to stop: past the limit or `until`, or as `stop` says, looked at every
    /// [`LOOK_EVERY`] of work.
    fn over(&mut self) -> bool {
        if self.work > self.limit || self.until.is_some_and(|until| Instant::now() >= until) {
            return true;
        }
        if self.work - self.looked > LOOK_EVERY {
            self.looked = self.work;
            return (self.stop)();
        }
        false
    }
}

/// How a look in one line ended.
enum InLine {
    Match(Range<usize>),
    None,
    Stopped,
}

/// The last match of `regexp` in `text` that starts at `start_at_most` or before and ends by
/// `before`, doing at most `limit` of work, until `until` if given, and stopping before a
/// line longer than the work left if `short_lines`. [`Found::Stopped`] gives the `before` to
/// go on with, `start_at_most` staying as it was.
///
/// # Errors
///
/// [`LineTooLong`] at a line longer than the regexp is matched in.
pub(crate) fn find_backward(
    regexp: &Regexp,
    text: &Rope,
    (before, start_at_most): (usize, usize),
    (limit, until, short_lines): (usize, Option<Instant>, bool),
    stop: &dyn Fn() -> bool,
) -> Result<Found, LineTooLong> {
    let before = text.clip_offset(before.min(text.len()), Bias::Left);
    let start_at_most = text.clip_offset(start_at_most.min(before), Bias::Left);
    let mut effort = Effort {
        work: 0,
        limit,
        looked: 0,
        until,
        stop,
    };
    let mut ending = Ending::default();
    let mut line = String::new();
    let mut row = text.offset_to_point(start_at_most).row;
    loop {
        let (start, end) = (text.line_start(row), text.line_end(row));
        // The first line ends at `before` if it is in it; the lines before it at their ends.
        let cut = before.min(end);
        if effort.over() {
            return Ok(Found::Stopped { resume: cut });
        }
        if end - start > regexp.longest {
            return Err(LineTooLong(start));
        }
        if short_lines && end - start + PER_LINE > effort.limit.saturating_sub(effort.work) {
            return Ok(Found::Stopped { resume: cut });
        }
        line.clear();
        line.extend(text.chunks_in_range(start..end));
        effort.work = effort.work.saturating_add(end - start + PER_LINE);
        let last = start_at_most.min(cut) - start;
        let cut_here = (&line[..], start, cut - start);
        match last_in_line(regexp, cut_here, last, &mut ending, &mut effort) {
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
    effort: &mut Effort<'_>,
) -> InLine {
    let floor = |at: usize| {
        let mut at = at.min(cut);
        while !line.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    // The starts looked at are below `end`, and at `lo` or after.
    let mut end = last + 1;
    let mut width = FIRST_WINDOW;
    loop {
        let lo = floor((end - 1).saturating_sub(width));
        let (mut from, mut best) = (lo, None);
        loop {
            if effort.over() {
                return InLine::Stopped;
            }
            let at = (line, line_start, cut);
            let Some(found) = cut::find(regexp, at, from, ending, &mut effort.work) else {
                break;
            };
            if found.start >= end {
                break;
            }
            from = char_after(&line[..cut], found.start);
            best = Some(found);
            if from >= end {
                break;
            }
        }
        if let Some(found) = best {
            return InLine::Match(found);
        }
        if lo == 0 {
            return InLine::None;
        }
        end = lo;
        width = width.saturating_mul(2);
    }
}
