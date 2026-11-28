//! Motion by sentences, as Emacs's `forward-sentence` and `backward-sentence` move with
//! `sentence-end-double-space` off (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot only.

use super::paragraph::{paragraph_end, paragraph_start};
use super::text;
use super::{Edge, WalkBudget, cluster_start, next_cluster};
use crate::display_map::DisplaySnapshot;
use crate::selection::Selection;

/// What ends a sentence before a space or a line's end.
const ENDS: [char; 4] = ['.', '?', '!', '…'];

/// What may close a sentence after its end, before the space.
const CLOSERS: [char; 7] = ['"', '\'', ')', ']', '”', '’', '»'];

/// What ends a sentence with no space after it.
const WIDE_ENDS: [char; 4] = ['。', '！', '？', '．'];

/// Whether `c` may follow a sentence before the next: the space or the line's end the end needs
/// is one of these, and so is what `backward-sentence` passes after it.
fn is_gap(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

/// Moves `selection` over `n` sentences, back if `n` is negative: forward to just after the
/// end of the sentence, closing quotes and brackets with it, back to the start of one. A
/// paragraph's end ends one too. A step at the buffer's edge fails with it, and one past the
/// budget leaves the cursor where the step before it did.
pub fn forward_sentence(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    text::steps(display, selection, n, |at, forward| {
        if forward {
            sentence_end(display, at, budget)
        } else {
            sentence_start(display, at, budget)
        }
    })
}

/// Where the sentence after `at`, before the buffer's end, ends, as Emacs's `forward-sentence`
/// finds it: the first end in what is left of the paragraph, or the paragraph's text's end.
pub(super) fn sentence_end(
    display: &DisplaySnapshot,
    at: usize,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let limit = paragraph_text_end(display, at, budget)?;
    if !text::seek(budget) {
        return None;
    }
    // Where an end, and the closers after it, were read up to; and whether it was a wide one.
    let mut end: Option<(usize, bool)> = None;
    let mut found = None;
    let mut read = at;
    for (offset, c) in text::after(display, at, budget) {
        if offset >= limit {
            break;
        }
        read = offset + c.len_utf8();
        end = match end {
            Some((stop, false)) if is_gap(c) => {
                found = Some(stop);
                break;
            }
            Some((stop, true)) if !WIDE_ENDS.contains(&c) => {
                found = Some(stop);
                break;
            }
            Some((_, false)) if CLOSERS.contains(&c) => Some((read, false)),
            _ if ENDS.contains(&c) => Some((read, false)),
            _ if WIDE_ENDS.contains(&c) => Some((read, true)),
            _ => None,
        };
    }
    if found.is_none() {
        if read < limit {
            return None;
        }
        // The paragraph's end is a line's end, which ends a sentence.
        found = end.map(|(stop, _)| stop);
    }
    Some(match found {
        Some(stop) => {
            let _ = text::seek(budget);
            text::cluster_boundary(display, stop)
        }
        None => limit,
    })
}

/// Where the sentence before `at`, after the buffer's start, starts, as Emacs's
/// `backward-sentence` finds it: after the last end before `at` in the paragraph, and the
/// space after it, unless that reaches `at`; then after the end before; else the paragraph's
/// text's start.
pub(super) fn sentence_start(
    display: &DisplaySnapshot,
    at: usize,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let text_start = paragraph_text_start(display, at, budget)?;
    let limit = super::line_start(display, display.offset_to_display_point(text_start).row);
    if !text::seek(budget) {
        return None;
    }
    let here = text::after(display, at, budget).next().map(|(_, c)| c);
    if !text::seek(budget) {
        return None;
    }
    // What follows the character read last: its offset, the character there, and where the
    // gap that starts there ends, no further than `at`.
    let mut next = (at, here, at);
    // The same, past the closers that follow the character read last.
    let mut past_closers = next;
    let mut read = at;
    for (offset, c) in text::before(display, at, budget) {
        if offset < limit {
            break;
        }
        read = offset;
        let gap_end = if is_gap(c) { next.2 } else { offset };
        let found = if ENDS.contains(&c) {
            let (_, after, gap_end) = past_closers;
            after.is_none_or(is_gap).then_some(gap_end)
        } else if WIDE_ENDS.contains(&c) && !next.1.is_some_and(|c| WIDE_ENDS.contains(&c)) {
            Some(next.2)
        } else {
            None
        };
        if let Some(start) = found.filter(|&start| start < at) {
            return Some(cluster_start(display, start));
        }
        next = (offset, Some(c), gap_end);
        if !CLOSERS.contains(&c) {
            past_closers = next;
        }
    }
    (read <= limit).then(|| text_start.min(at))
}

/// Where the text of the paragraph at `at` ends, as Emacs's `end-of-paragraph-text`: before
/// the line break that ends its last line; from there, the next paragraph's.
fn paragraph_text_end(
    display: &DisplaySnapshot,
    at: usize,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let len = display.len();
    let mut from = at;
    loop {
        let end = paragraph_end(display, from, false, budget)?;
        let point = display.offset_to_display_point(end);
        let end = if point.column == 0 && point.row > 0 && end > from {
            super::line_end(display, point.row - 1)
        } else {
            end
        };
        if end > from {
            return Some(end);
        }
        match next_cluster(display, end) {
            Some(next) if next < len => from = next,
            next => return Some(next.unwrap_or(len)),
        }
    }
}

/// Where the text of the paragraph before `at` starts, as Emacs's `start-of-paragraph-text`:
/// past the spaces and line breaks after the paragraph's start, or from the blank lines that
/// reach `at`, the paragraph's before.
fn paragraph_text_start(
    display: &DisplaySnapshot,
    at: usize,
    budget: &mut WalkBudget,
) -> Option<usize> {
    let mut from = at;
    loop {
        let start = paragraph_start(display, from, false, budget)?;
        if !text::seek(budget) {
            return None;
        }
        let mut text = start;
        let mut chars = text::after(display, start, budget);
        loop {
            match chars.next() {
                Some((offset, c)) if is_gap(c) => text = offset + c.len_utf8(),
                Some(_) => break,
                None if text < display.len() => return None,
                None => break,
            }
        }
        if text >= from && start < from {
            if start == 0 {
                return Some(0);
            }
            from = start;
            continue;
        }
        return Some(cluster_start(display, text));
    }
}

#[cfg(test)]
mod tests;
