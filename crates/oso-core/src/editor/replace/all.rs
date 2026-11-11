//! query-replace's `!` (ADR 0113): every match left, a slice a turn of the host's loop, so
//! that frames keep coming however many there are.
//!
//! Must not depend on: `command` or `keymap`, or the host.

use std::ops::Range;

use super::{ALL_SLICE, Replace, after};
use crate::buffer::Replacement;
use crate::editor::Editor;
use crate::motion;
use crate::search::{Found, KEY_BUDGET, LineTooLong};

/// Where `!` goes on after a slice.
enum Next {
    /// From here, at the next turn.
    From(usize),
    /// Nowhere: no match is left.
    Done,
    /// Nowhere: a regexp met a line too long to match in, which starts here.
    TooLong(usize),
}

impl Editor {
    /// Whether `!` has more to replace, for the host to call [`Editor::replace_continue`] at
    /// its next turn.
    pub fn replace_works(&self) -> bool {
        self.replace().is_some_and(Replace::replacing_all)
    }

    /// `!`'s next slice: the matches within a key's budget of text, at most [`ALL_SLICE`] of
    /// them, replaced as one edit. Ends the session after the last. Returns whether the text
    /// changed.
    pub fn replace_continue(&mut self) -> bool {
        let Some(state) = self.replace() else {
            return false;
        };
        let Some(from) = state.all_from else {
            return false;
        };
        let id = state.buffer;
        let (matches, next) = self.replace_slice(from);
        let texts: Vec<String> = matches
            .iter()
            .filter_map(|range| self.replacement_for(range))
            .collect();
        let edits: Vec<Replacement<'_>> = matches
            .iter()
            .zip(&texts)
            .map(|(range, text)| Replacement {
                range: range.clone(),
                text,
            })
            .collect();
        let changed = !edits.is_empty() && self.apply_edits(id, &edits).is_ok();
        // Where the next slice starts, after what this one put in.
        let shift: isize = matches
            .iter()
            .zip(&texts)
            .map(|(range, text)| text.len().cast_signed() - range.len().cast_signed())
            .sum();
        let revision = self.buffer(id).map(|buffer| buffer.revision());
        if let Some(state) = &mut self.replacing.session {
            if changed {
                state.replaced += matches.len();
            }
            if let Some(revision) = revision {
                state.revision = revision;
            }
            state.all_from = match next {
                Next::From(at) => Some(at.saturating_add_signed(shift)),
                Next::Done | Next::TooLong(_) => None,
            };
        }
        if let Some(last) = matches.last().zip(texts.last()) {
            // Its start moved by what the others changed; its end by that and its own.
            let end = last.0.start.saturating_add_signed(shift) + last.0.len();
            let display = self.current().1.display();
            let cursor = motion::cluster_start(&display, end);
            self.windows[self.selected].selections.move_primary(cursor);
            self.follow_cursor();
        }
        match next {
            Next::From(_) => {}
            Next::Done => self.replace_end(true),
            Next::TooLong(at) => self.replace_line_too_long(at.saturating_add_signed(shift)),
        }
        changed
    }

    /// The matches of `!`'s next slice from `from`, within a key's budget of text and at most
    /// [`ALL_SLICE`] of them, and where the slice after starts, if there is more.
    fn replace_slice(&self, from: usize) -> (Vec<Range<usize>>, Next) {
        let Some(state) = self.replace() else {
            return (Vec::new(), Next::Done);
        };
        let ranges = self.replace_scope();
        let Some(buffer) = self.buffer(state.buffer) else {
            return (Vec::new(), Next::Done);
        };
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let never = || false;
        let mut budget = KEY_BUDGET;
        let mut matches = Vec::new();
        for range in ranges.iter().filter(|range| range.end > from) {
            let mut start = range.start.max(from);
            while start < range.end {
                if matches.len() == ALL_SLICE || budget == 0 {
                    return (matches, Next::From(start));
                }
                match state
                    .needle
                    .find_in((text, invalid), start..range.end, budget, &never)
                {
                    Ok(Found::Match(found)) => {
                        budget = budget.saturating_sub(found.end - start);
                        start = after(&found);
                        matches.push(found);
                    }
                    Ok(Found::None) => {
                        budget = budget.saturating_sub(range.end - start);
                        break;
                    }
                    // A regexp's line longer than a slice's budget gets a slice of its own,
                    // read whole, or `!` would never pass it.
                    Ok(Found::Stopped { resume }) if resume == start && matches.is_empty() => {
                        let end = text.line_end(text.offset_to_point(start).row) + 1;
                        let line = start..end.min(range.end);
                        return match state.needle.find_in(
                            (text, invalid),
                            line.clone(),
                            usize::MAX,
                            &never,
                        ) {
                            Ok(Found::Match(found)) => {
                                let next = after(&found);
                                (vec![found], Next::From(next))
                            }
                            Err(LineTooLong(at)) => (matches, Next::TooLong(at)),
                            Ok(_) => (matches, Next::From(line.end)),
                        };
                    }
                    Ok(Found::Stopped { resume }) => return (matches, Next::From(resume)),
                    Err(LineTooLong(at)) => return (matches, Next::TooLong(at)),
                }
            }
        }
        (matches, Next::Done)
    }
}
