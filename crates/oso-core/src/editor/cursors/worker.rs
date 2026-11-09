//! The part of a search for cursors a worker does (ADR 0091): asking it, taking in what it
//! found if the window's cursors and the text are as they were, and dropping it on `C-g`.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host runs the job
//! isearch's worker runs.

use std::sync::atomic::Ordering;

use super::find::{Text, Toward, Wanted};
use super::{Ask, CursorError, Pending};
use crate::editor::Editor;
use crate::search::{MatchWork, Matched, SearchJob};

impl Editor {
    /// Takes in what a worker found for a key that adds cursors, unless the window's
    /// cursors or the text changed since. Returns whether what shows changed.
    pub(in crate::editor) fn cursors_reported(
        &mut self,
        generation: u64,
        matched: Matched,
    ) -> bool {
        let Some(marking) = &mut self.marking else {
            return false;
        };
        let Some(pending) = marking
            .pending
            .take_if(|pending| pending.generation == generation)
        else {
            return false;
        };
        let (window, buffer, revision) = (marking.window, marking.buffer, marking.revision);
        let len = self.buffer(buffer).map_or(0, |buffer| buffer.text().len());
        let same = window == self.selected
            && self.windows[window].buffer() == buffer
            && self.windows[window].selections == pending.selections
            && (pending.loading
                || self
                    .buffer(buffer)
                    .is_some_and(|buffer| buffer.revision() == revision))
            && matched.ranges.iter().all(|range| range.end <= len);
        if !same {
            self.say_found("Search dropped: the buffer changed".into());
            return true;
        }
        let done = match pending.ask {
            Ask::Add { toward, most } => {
                let found = self.put_found(toward, false, &matched.ranges);
                found.and(if matched.ranges.len() < most {
                    Err(CursorError::NoMore)
                } else {
                    Ok(())
                })
            }
            Ask::Skip(toward) => self.put_found(toward, true, &matched.ranges),
            Ask::All => self.put_all(&matched),
        };
        if let Err(error) = done {
            self.say_found(error.to_string());
        }
        true
    }

    /// Drops a search for cursors still waiting on the worker, as every `C-g` does.
    pub(in crate::editor) fn drop_cursor_search(&mut self) {
        let Some(pending) = self
            .marking
            .as_mut()
            .and_then(|marking| marking.pending.take())
        else {
            return;
        };
        // A newer job, isearch's, may have taken its place already.
        if pending.generation == self.search_live.load(Ordering::Relaxed) {
            self.supersede_search();
        }
    }

    /// Leaves the rest of a search for cursors to the worker, from `from`.
    pub(super) fn ask_worker(&mut self, ask: Ask, from: usize, (text, wanted): (Text, Wanted)) {
        let generation = self.supersede_search();
        let loading = self.current().1.loading().is_some();
        let work: MatchWork = match ask {
            Ask::All => Box::new(move |stop| text.all(&wanted, stop)),
            Ask::Add { toward, most } => {
                Box::new(move |stop| several(&text, &wanted, toward, from, most, stop))
            }
            Ask::Skip(toward) => {
                Box::new(move |stop| several(&text, &wanted, toward, from, 1, stop))
            }
        };
        let live = (generation, self.search_live.clone());
        self.search_job = Some(SearchJob::matches(work, live));
        let selections = self.windows[self.selected].selections.clone();
        if let Some(marking) = &mut self.marking {
            marking.pending = Some(Pending {
                generation,
                ask,
                selections,
                loading,
            });
        }
        self.echo("Searching…");
    }

    /// Says what a search for cursors found, in the echo area alone while a buffer that may
    /// hold secrets is shown, since it tells something of its text.
    fn say_found(&mut self, text: String) {
        if self.shows_secret() {
            self.echo(text);
        } else {
            self.message(text);
        }
    }
}

/// Up to `most` occurrences `toward` either way from `from`, with no budget but `stop`.
fn several(
    text: &Text,
    wanted: &Wanted,
    toward: Toward,
    from: usize,
    most: usize,
    stop: &dyn Fn() -> bool,
) -> Option<Matched> {
    let mut ranges = Vec::new();
    let mut limit = usize::MAX;
    match text.several(
        wanted,
        (toward, from),
        (most, &mut limit),
        stop,
        &mut ranges,
    ) {
        Some(_) => None,
        None => Some(Matched {
            total: ranges.len(),
            ranges,
        }),
    }
}
