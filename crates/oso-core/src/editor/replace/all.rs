//! query-replace's `!` (ADR 0113): every match left, a slice a turn of the host's loop, so
//! that frames keep coming however many there are.
//!
//! Must not depend on: `command` or `keymap`, or the host.

use std::ops::Range;
use std::time::{Duration, Instant};

use super::{ALL_SLICE, Replace};
use crate::buffer::Replacement;
use crate::editor::Editor;
use crate::motion;
use crate::search::{Found, KEY_BUDGET, Line, LineTooLong, MAX_LINE};

/// The most time a slice of `!` looks for matches, as a pattern can read far past each.
const SLICE_TIME: Duration = Duration::from_millis(3);

/// Where `!` goes on after a slice.
enum Next {
    /// From here, at the next turn, and whether a match ended there.
    From((usize, bool)),
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

    /// `!`'s next slice: the matches within a key's budget of work, at most [`ALL_SLICE`] of
    /// them, replaced as one edit, which counts as edited whole for whitespace cleanup, so
    /// that a long `!` does not grow the edited ranges by one for each match. A line a slice
    /// reads is kept for the next while only `!` changes it. Ends the session after the last.
    pub fn replace_continue(&mut self) {
        let Some(state) = self.replace() else {
            return;
        };
        let Some(from) = state.all_from else {
            return;
        };
        let id = state.buffer;
        let revision = self.buffer(id).map(|buffer| buffer.revision());
        let kept = self
            .replacing
            .session
            .as_mut()
            .and_then(|state| state.line_kept.take())
            .filter(|(_, at)| Some(*at) == revision)
            .map(|(line, _)| line);
        let (matches, next, line) = self.replace_slice(from, kept);
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
        // The line read stays right through this slice's own edits, if they are all in it.
        let line = line.filter(|line| {
            let end = line.start + line.text.len();
            matches
                .iter()
                .all(|found| found.start >= line.start && found.end <= end)
        });
        let line = line.map(|line| {
            let made: Vec<(Range<usize>, &str)> = matches
                .iter()
                .cloned()
                .zip(texts.iter().map(String::as_str))
                .collect();
            line.edited(&made)
        });
        let revision = self.buffer(id).map(|buffer| buffer.revision());
        if let Some(state) = &mut self.replacing.session {
            if changed {
                state.replaced += matches.len();
            }
            if let Some(revision) = revision {
                state.revision = revision;
                // Edits that did not go in leave the line as it was read, unlike its copy.
                state.line_kept = line
                    .filter(|_| changed || matches.is_empty())
                    .map(|line| (line, revision));
            }
            state.all_from = match next {
                Next::From((at, ended)) => Some((at.saturating_add_signed(shift), ended)),
                Next::Done | Next::TooLong(_) => None,
            };
        }
        if let Some((first, last)) = matches.first().zip(matches.last()) {
            // Its start moved by what the others changed; its end by that and its own.
            let end = last.start.saturating_add_signed(shift) + last.len();
            if changed && let Some(buffer) = self.buffer_mut(id) {
                buffer.cover_edited(first.start..end);
            }
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
    }

    /// The matches of `!`'s next slice from `from`, within a key's budget of work and at most
    /// [`ALL_SLICE`] of them, where the slice after starts, and the line it ended in. A line
    /// longer than a slice's budget gets a slice of its own: read whole, and a budget of work
    /// past that, or `!` would never pass it.
    fn replace_slice(
        &self,
        from: (usize, bool),
        kept: Option<Line>,
    ) -> (Vec<Range<usize>>, Next, Option<Line>) {
        let Some(state) = self.replace() else {
            return (Vec::new(), Next::Done, None);
        };
        let ranges = self.replace_scope();
        let Some(buffer) = self.buffer(state.buffer) else {
            return (Vec::new(), Next::Done, None);
        };
        let snapshot = (buffer.text(), buffer.invalid_bytes());
        let mut matches = Vec::new();
        let until = Instant::now() + SLICE_TIME;
        let mut scan = state.needle.scan(snapshot, &ranges, from, KEY_BUDGET, kept);
        scan.until(until);
        let mut whole_line = false;
        let next = loop {
            if matches.len() == ALL_SLICE {
                break Next::From(scan.place());
            }
            match scan.next(&|| false) {
                Ok(Found::Match(found)) => matches.push(found),
                Ok(Found::None) => break Next::Done,
                Ok(Found::Stopped { .. }) if matches.is_empty() && !whole_line => {
                    let place = scan.place();
                    if place.0 != from.0 {
                        break Next::From(place);
                    }
                    let line = scan.into_line();
                    scan = state
                        .needle
                        .scan(snapshot, &ranges, place, MAX_LINE + KEY_BUDGET, line);
                    // Its line is read whole, and then it goes on for the slice's time.
                    scan.until(Instant::now() + SLICE_TIME);
                    whole_line = true;
                }
                Ok(Found::Stopped { .. }) => break Next::From(scan.place()),
                Err(LineTooLong(at)) => break Next::TooLong(at),
            }
        };
        (matches, next, scan.into_line())
    }
}
