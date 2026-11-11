//! query-replace's answers at a match (ADR 0113): replacing, skipping, staying, going back
//! and undoing, and the answers given while a worker looked, run in order once it finds.
//!
//! Must not depend on: `command` or `keymap`, or the host.

use std::ops::Range;

use super::state::Visit;
use super::{ReplaceAnswer, after};
use crate::buffer::Replacement;
use crate::editor::Editor;
use crate::motion;

impl Editor {
    /// Runs the answers given while a worker looked, in order, until one waits again.
    pub(super) fn replace_run_pending(&mut self) {
        while let Some(state) = &mut self.replacing.session {
            if state.searching.is_some() || state.all_from.is_some() {
                return;
            }
            let Some(answer) = state.pending.pop_front() else {
                return;
            };
            self.replace_answer(answer);
        }
    }

    /// Replaces `range` with the session's replacement, in its case if it preserves case.
    /// Returns where the replacement is now.
    fn replace_one(&mut self, range: Range<usize>) -> Option<Range<usize>> {
        let id = self.replace()?.buffer;
        let to = self.replacement_for(&range)?;
        let edit = Replacement {
            range: range.clone(),
            text: &to,
        };
        self.apply_edits(id, &[edit]).ok()?;
        let revision = self.buffer(id).map(|buffer| buffer.revision());
        let state = self.replacing.session.as_mut()?;
        state.replaced += 1;
        if let Some(revision) = revision {
            state.revision = revision;
        }
        let now = range.start..range.start + to.len();
        state.visits.push(Visit {
            found: range,
            replaced: Some(now.clone()),
        });
        Some(now)
    }

    /// Takes `answer` at the match asked about. Answers given while a worker looks wait for
    /// it, and those given while `!` replaces are dropped.
    pub fn replace_answer(&mut self, answer: ReplaceAnswer) {
        let Some(state) = &mut self.replacing.session else {
            return;
        };
        if state.all_from.is_some() {
            return;
        }
        if state.searching.is_some() && answer != ReplaceAnswer::Exit {
            state.pending.push_back(answer);
            return;
        }
        state.help = answer == ReplaceAnswer::Help;
        let (current, stay) = (state.current.clone(), state.stay.clone());
        match answer {
            ReplaceAnswer::Help => {}
            ReplaceAnswer::Exit => self.replace_end(true),
            ReplaceAnswer::Replace | ReplaceAnswer::Skip if stay.is_some() => {
                let at = stay.map_or(0, |stay| stay.end);
                self.replace_find(at);
            }
            ReplaceAnswer::Replace => {
                if let Some(now) = current.and_then(|range| self.replace_one(range)) {
                    self.replace_find(after(&now));
                }
            }
            ReplaceAnswer::Skip => {
                if let Some(range) = current {
                    if let Some(state) = &mut self.replacing.session {
                        state.skipped += 1;
                        state.visits.push(Visit {
                            found: range.clone(),
                            replaced: None,
                        });
                    }
                    self.replace_find(after(&range));
                }
            }
            ReplaceAnswer::ReplaceAndExit => {
                if let Some(range) = current {
                    self.replace_one(range);
                }
                self.replace_end(true);
            }
            ReplaceAnswer::ReplaceAndStay => {
                if let Some(now) = current.and_then(|range| self.replace_one(range)) {
                    self.replace_stay(now);
                }
            }
            ReplaceAnswer::Back => self.replace_back(),
            ReplaceAnswer::Undo => self.replace_undo(),
            ReplaceAnswer::All => {
                let from = match (current, stay) {
                    (Some(range), _) => range.start,
                    (None, Some(stay)) => stay.end,
                    (None, None) => return,
                };
                if let Some(state) = &mut self.replacing.session {
                    state.all_from = Some(from);
                    state.current = None;
                }
                self.replace_continue();
            }
        }
    }

    /// Stays at `range`: the cursor at its end, and the next answer moves on from there.
    fn replace_stay(&mut self, range: Range<usize>) {
        if let Some(state) = &mut self.replacing.session {
            state.current = None;
            state.stay = Some(range.clone());
        }
        let display = self.current().1.display();
        let cursor = motion::cluster_start(&display, range.end);
        self.windows[self.selected].selections.move_primary(cursor);
        self.follow_cursor();
    }

    /// `^`: back to the match before, as it is now, without undoing it.
    fn replace_back(&mut self) {
        let Some(state) = &mut self.replacing.session else {
            return;
        };
        let Some(visit) = state.visits.last().cloned() else {
            self.echo("No previous match");
            return;
        };
        match visit.replaced {
            Some(now) => self.replace_stay(now),
            None => {
                if let Some(state) = &mut self.replacing.session {
                    state.visits.pop();
                    state.skipped = state.skipped.saturating_sub(1);
                }
                self.replace_at(visit.found);
            }
        }
    }

    /// `u`: undoes the last replacement, and asks about its match again.
    fn replace_undo(&mut self) {
        let Some(state) = &mut self.replacing.session else {
            return;
        };
        let Some(last) = state
            .visits
            .iter()
            .rposition(|visit| visit.replaced.is_some())
        else {
            self.echo("Nothing to undo");
            return;
        };
        let visit = state.visits[last].clone();
        if self.undo().is_err() {
            self.echo("Nothing to undo");
            return;
        }
        let revision = self.current().1.revision();
        if let Some(state) = &mut self.replacing.session {
            let undone = state.visits.split_off(last);
            state.skipped -= undone
                .iter()
                .filter(|visit| visit.replaced.is_none())
                .count();
            state.replaced -= 1;
            state.revision = revision;
        }
        self.replace_at(visit.found);
    }
}
