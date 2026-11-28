//! Motion by pages, as Emacs's `forward-page` and `backward-page` move with their default
//! `page-delimiter`: a form feed at a line's start (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot only.

use super::text;
use super::{Edge, WalkBudget};
use crate::display_map::DisplaySnapshot;
use crate::selection::Selection;

/// The page delimiter, at a line's start.
const FORM_FEED: char = '\u{c}';

/// Moves `selection` over `n` pages, back if `n` is negative: forward to just after the next
/// form feed at a line's start, or the buffer's end; back to just after the one before, or
/// the buffer's start. A step at the buffer's edge fails with it, and one past the budget
/// leaves the cursor where the step before it did.
pub fn forward_page(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    text::steps(display, selection, n, |at, forward| {
        if forward {
            page_end(display, at, budget)
        } else {
            page_start(display, at, budget)
        }
    })
}

/// Where the page after `at`, before the buffer's end, ends, as Emacs's `forward-page`: just
/// after the next delimiter, passing one that `at` is just before at a line's start.
fn page_end(display: &DisplaySnapshot, at: usize, budget: &mut WalkBudget) -> Option<usize> {
    if !text::enter(budget) {
        return None;
    }
    let line_start = at == 0 || text::before(display, at, budget).next()?.1 == '\n';
    if !text::seek(budget) {
        return None;
    }
    let len = display.len();
    let mut chars = text::after(display, at, budget);
    let mut read = at;
    let mut after_break = false;
    if line_start {
        // As Emacs: a delimiter must not match where the motion starts.
        let (offset, c) = chars.next()?;
        read = offset + c.len_utf8();
        after_break = c == '\n';
    }
    for (offset, c) in chars {
        if after_break && c == FORM_FEED {
            return Some(offset + c.len_utf8());
        }
        read = offset + c.len_utf8();
        after_break = c == '\n';
    }
    (read >= len).then_some(len)
}

/// Where the page before `at`, after the buffer's start, starts, as Emacs's `backward-page`:
/// just after the delimiter before it, passing the one that ends at `at`.
fn page_start(display: &DisplaySnapshot, at: usize, budget: &mut WalkBudget) -> Option<usize> {
    if !text::enter(budget) {
        return None;
    }
    let (last, before_last) = {
        let mut back = text::before(display, at, budget);
        let last = back.next()?;
        let before_last = back.next();
        if last.0 > 0 && before_last.is_none() {
            return None;
        }
        (last, before_last)
    };
    // As Emacs: past the delimiter that ends at `at`, then one character back, which the
    // delimiter found must end at or before.
    let passed = last.1 == FORM_FEED && before_last.is_none_or(|(_, c)| c == '\n');
    let from = match (passed, before_last) {
        (true, Some((offset, _))) => offset,
        (true, None) => return Some(0),
        (false, _) => last.0,
    };
    if from == 0 {
        return Some(0);
    }
    if !text::seek(budget) {
        return None;
    }
    // A form feed waiting for what is before it: a line break or the buffer's start.
    let mut waiting = None;
    let mut read = from;
    for (offset, c) in text::before(display, from, budget) {
        if waiting.is_some() && c == '\n' {
            return waiting;
        }
        waiting = (c == FORM_FEED).then_some(offset + c.len_utf8());
        read = offset;
    }
    if read > 0 {
        return None;
    }
    Some(waiting.unwrap_or(0))
}

#[cfg(test)]
mod tests;
