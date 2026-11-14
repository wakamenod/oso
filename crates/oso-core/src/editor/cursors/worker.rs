//! The part of a search for cursors a worker does (ADR 0091): asking it, taking in what it
//! found if the window's cursors and the text are as they were, and dropping it.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads. The host runs the job
//! isearch's worker runs.
//!
//! - A newer job of any kind, isearch's too, supersedes it, and its answer is then dropped.
//! - An answer for cursors that have moved since is dropped with nothing said, as the
//!   person has gone on. One for a text that changed says so in the echo area.
//! - While the buffer loads, a load only appends: an answer stands if every match it found
//!   ends far enough before where the text ended, that no chunk the load rebuilt reaches it.

use std::sync::atomic::Ordering;

use super::find::{All, Text, Toward, Wanted};
use super::{Ask, CursorError, Pending};
use crate::editor::Editor;
use crate::rope::MAX_CHUNK_BYTES;
use crate::search::{MatchWork, Matched, SearchJob};

/// What the echo area says while a worker searches.
const SEARCHING: &str = "Searching…";

/// How far before the end of a loading text the chunks a load appends to can reach: those
/// it rebuilds out to a grapheme boundary, and the last one, which it may join.
const LOAD_REACH: usize = 16 * MAX_CHUNK_BYTES;

impl Editor {
    /// Takes in what a worker found for a key that adds cursors, unless a newer job took its
    /// place, or the window's cursors or the text changed since. Returns whether what shows
    /// changed.
    pub(in crate::editor) fn cursors_reported(
        &mut self,
        generation: u64,
        matched: Matched,
    ) -> bool {
        let live = self.search_live.load(Ordering::Relaxed);
        let Some(marking) = &mut self.marking else {
            return false;
        };
        let Some(pending) = marking
            .pending
            .take_if(|pending| pending.generation == generation && generation == live)
        else {
            return false;
        };
        let (window, buffer, revision) = (marking.window, marking.buffer, marking.revision);
        self.clear_echo_saying(SEARCHING);
        let selected = &self.windows[self.selected];
        let moved = window != selected.id()
            || selected.buffer() != buffer
            || selected.selections != pending.selections;
        if moved {
            return true;
        }
        let unchanged = self
            .buffer(buffer)
            .is_some_and(|text| text.revision() == revision);
        let appended = pending.loading.is_some_and(|len| {
            let before = len.saturating_sub(LOAD_REACH);
            matched.ranges.iter().all(|range| range.end <= before)
        });
        if !unchanged && !appended {
            self.echo("Search dropped: the buffer changed");
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

    /// Whether a search for cursors waits on the worker.
    pub fn cursor_search_waits(&self) -> bool {
        self.marking
            .as_ref()
            .is_some_and(|marking| marking.pending.is_some())
    }

    /// Drops a search for cursors still waiting on the worker: every `C-g` does, also one
    /// that cancels a prefix, and every key that adds cursors before it searches.
    pub fn drop_cursor_search(&mut self) {
        let Some(pending) = self
            .marking
            .as_mut()
            .and_then(|marking| marking.pending.take())
        else {
            return;
        };
        self.clear_echo_saying(SEARCHING);
        // A newer job, isearch's, may have taken its place already.
        if pending.generation == self.search_live.load(Ordering::Relaxed) {
            self.supersede_search();
        }
    }

    /// Leaves the rest of a search for cursors to the worker, from `from`.
    pub(super) fn ask_worker(&mut self, ask: Ask, from: usize, (text, wanted): (Text, Wanted)) {
        let generation = self.supersede_search();
        // The worker has no time to keep to.
        let text = Text {
            until: None,
            on_key: false,
            ..text
        };
        let loading = self.current().1.loading().map(|_| text.rope.len());
        let work: MatchWork = match ask {
            Ask::All => Box::new(move |stop| {
                let mut left = usize::MAX;
                match text.all(&wanted, &mut left, stop) {
                    All::Done(matched) => Some(matched),
                    All::Stopped => None,
                }
            }),
            Ask::Add { toward, most } => {
                Box::new(move |stop| several(&text, &wanted, (toward, from), most, stop))
            }
            Ask::Skip(toward) => {
                Box::new(move |stop| several(&text, &wanted, (toward, from), 1, stop))
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
        self.echo(SEARCHING);
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
    from: (Toward, usize),
    most: usize,
    stop: &dyn Fn() -> bool,
) -> Option<Matched> {
    let (mut ranges, mut left) = (Vec::new(), usize::MAX);
    match text.several(wanted, from, (most, &mut left), stop, &mut ranges) {
        Some(_) => None,
        None => Some(Matched {
            total: ranges.len(),
            ranges,
        }),
    }
}
