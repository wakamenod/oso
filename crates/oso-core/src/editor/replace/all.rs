//! query-replace's `!` (ADR 0113): every match left, a slice a turn of the host's loop, so
//! that frames keep coming however many there are.
//!
//! Must not depend on: `command` or `keymap`, or the host.

use std::ops::Range;

use super::{ALL_SLICE, Replace, after};
use crate::buffer::Replacement;
use crate::editor::Editor;
use crate::motion;
use crate::search::{self, Found, KEY_BUDGET};

impl Editor {
    /// Whether `!` has more to replace, for the host to call [`Editor::replace_continue`] at
    /// its next turn.
    pub fn replace_works(&self) -> bool {
        self.replace().is_some_and(Replace::replacing_all)
    }

    /// `!`'s next slice: the matches within a key's budget of text, at most [`ALL_SLICE`] of
    /// them, replaced as one edit, which counts as edited whole for whitespace cleanup, so
    /// that a long `!` does not grow the edited ranges by one for each match. Ends the session
    /// after the last.
    pub fn replace_continue(&mut self) {
        let Some(state) = self.replace() else {
            return;
        };
        let Some(from) = state.all_from else {
            return;
        };
        let (id, preserve) = (state.buffer, state.preserve_case);
        let to = state.to.clone();
        let (matches, next) = self.replace_slice(from);
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let text = buffer.text();
        let texts: Vec<String> = matches
            .iter()
            .map(|range| {
                if preserve {
                    let matched: String = text.chunks_in_range(range.clone()).collect();
                    search::casified(&matched, &to).into_owned()
                } else {
                    to.clone()
                }
            })
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
            state.all_from = next.map(|at| at.saturating_add_signed(shift));
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
        if next.is_none() {
            self.replace_end(true);
        }
    }

    /// The matches of `!`'s next slice from `from`, within a key's budget of text and at most
    /// [`ALL_SLICE`] of them, and where the slice after starts, if there is more.
    fn replace_slice(&self, from: usize) -> (Vec<Range<usize>>, Option<usize>) {
        let Some(state) = self.replace() else {
            return (Vec::new(), None);
        };
        let ranges = self.replace_scope();
        let Some(buffer) = self.buffer(state.buffer) else {
            return (Vec::new(), None);
        };
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let never = || false;
        let mut budget = KEY_BUDGET;
        let mut matches = Vec::new();
        for range in ranges.iter().filter(|range| range.end > from) {
            let mut start = range.start.max(from);
            while start < range.end {
                if matches.len() == ALL_SLICE || budget == 0 {
                    return (matches, Some(start));
                }
                match search::find_in(
                    text,
                    invalid,
                    &state.query,
                    start..range.end,
                    budget,
                    &never,
                ) {
                    Found::Match(found) => {
                        budget = budget.saturating_sub(found.end - start);
                        start = after(text, &found);
                        matches.push(found);
                    }
                    Found::None => {
                        budget = budget.saturating_sub(range.end - start);
                        break;
                    }
                    Found::Stopped { resume } => return (matches, Some(resume)),
                }
            }
        }
        (matches, None)
    }
}
