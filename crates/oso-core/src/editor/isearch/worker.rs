//! isearch's worker and its frames (ADR 0082): the job a search leaves for a thread, what
//! comes back from it, a search again when the text changed under it, and the cells the
//! matches in view take.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads. The host
//! runs the job; this only makes it and takes in what it reports.
//!
//! A newer job supersedes the one before through a generation, which the worker checks
//! every block of text, and whose reports are dropped here once it is not the live one.

use std::ops::Range;
use std::sync::atomic::Ordering;

use super::Tally;
use crate::display_map::DisplayPoint;
use crate::editor::Editor;
use crate::frame::{self, CellRect};
use crate::search::{self, Find, SearchJob, SearchReport};
use crate::sum_tree::Bias;

/// The most matches in view a frame highlights, so that their rectangles leave room for
/// the rest of the frame.
pub(super) const MAX_MATCHES_SHOWN: usize = 2048;

impl Editor {
    /// The job a worker is to do, once, for the host.
    pub fn take_search_job(&mut self) -> Option<SearchJob> {
        self.search_job.take()
    }

    /// Takes in what a worker found, unless a newer job has taken its place. Returns whether
    /// what shows changed.
    pub fn search_reported(&mut self, report: SearchReport) -> bool {
        let live = self.search_live.load(Ordering::Relaxed);
        if self.isearch_fresh() {
            return true;
        }
        let Some(state) = &mut self.isearch else {
            return false;
        };
        match report {
            SearchReport::Found { generation, at } if generation == live => {
                let Some(how) = state.searching.take() else {
                    return false;
                };
                match at {
                    Some(range) => self.isearch_matched(range, how, true),
                    None => state.failing = true,
                }
                true
            }
            SearchReport::Counted {
                generation,
                count,
                at,
            } if generation == live => {
                let Some(Tally::Counting { moves }) = state.tally else {
                    return false;
                };
                let index = match at {
                    Some(_) if count.total > 0 => {
                        let index = (count.before + 1).saturating_add_signed(moves);
                        index.clamp(1, count.total)
                    }
                    _ => 0,
                };
                state.tally = Some(Tally::Known {
                    index,
                    total: count.total,
                });
                true
            }
            _ => false,
        }
    }

    /// Searches again if the text changed since isearch last looked. Returns whether it did.
    /// The host calls this before it waits, so a load that grows the buffer is taken in.
    pub fn isearch_fresh(&mut self) -> bool {
        let Some(state) = &self.isearch else {
            return false;
        };
        let Some(buffer) = self.buffer(state.buffer) else {
            // The buffer is gone, and the search with it.
            self.isearch = None;
            self.supersede_search();
            return true;
        };
        let (revision, len) = (buffer.revision(), buffer.text().len());
        let Some(state) = &mut self.isearch else {
            return false;
        };
        if revision == state.revision {
            return false;
        }
        state.revision = revision;
        state.searching = None;
        let clip = |range: Option<Range<usize>>| {
            range
                .map(|range| range.start.min(len)..range.end.min(len))
                .filter(|range| !range.is_empty())
        };
        state.current = clip(state.current.take());
        for step in &mut state.steps {
            step.current = clip(step.current.take());
            step.cursor = step.cursor.min(len);
            step.tally = None;
        }
        self.isearch_typed();
        true
    }

    /// Asks a worker to go on with `find`, if any, and then to count the matches, for the
    /// current match unless it fails. Supersedes any job before.
    pub(super) fn isearch_count(&mut self, find: Option<Find>) {
        let generation = self.supersede_search();
        let Some(state) = &mut self.isearch else {
            return;
        };
        let Some(query) = state.query.clone() else {
            return;
        };
        if find.is_none() {
            state.tally = Some(Tally::Counting { moves: 0 });
        }
        let current = state
            .current
            .as_ref()
            .filter(|_| !state.failing)
            .map(|range| range.start);
        let id = state.buffer;
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        let snapshot = (buffer.text().clone(), buffer.invalid_bytes().clone());
        let live = (generation, self.search_live.clone());
        self.search_job = Some(SearchJob::new(snapshot, query, find, current, live));
    }

    /// Ends whatever a worker is doing, and returns the generation of the next job.
    pub(super) fn supersede_search(&mut self) -> u64 {
        self.search_job = None;
        let generation = self.search_live.load(Ordering::Relaxed).wrapping_add(1);
        self.search_live.store(generation, Ordering::Relaxed);
        generation
    }

    /// The cells the matches in view take in the tiled window's text area: the current one
    /// into `current`, and the others into `others`, both emptied first. Only the rows in
    /// view are searched, each widened by what the string can reach, so a frame costs as
    /// much as the view and the string, not the buffer.
    pub fn search_cells(&self, current: &mut Vec<CellRect>, others: &mut Vec<CellRect>) {
        current.clear();
        others.clear();
        let Some(state) = &self.isearch else {
            return;
        };
        let Some(query) = &state.query else {
            return;
        };
        let (window, buffer) = self.tiled();
        if window.buffer() != state.buffer {
            return;
        }
        let display = buffer.display();
        let (text, invalid) = (buffer.text(), buffer.invalid_bytes());
        let (area, scroll) = (self.frame.window.text, window.scroll());
        let rows = scroll.top
            ..scroll
                .top
                .saturating_add(area.rows)
                .min(display.row_count());
        let reach = query.max_bytes();
        // Each row's columns in view, widened, and joined where they meet.
        let mut spans: Vec<Range<usize>> = Vec::new();
        for row in rows {
            let offset = |column| {
                display.display_point_to_offset(DisplayPoint::new(row, column), Bias::Left)
            };
            let start = offset(scroll.left).saturating_sub(reach);
            let end = offset(scroll.left.saturating_add(area.columns)).saturating_add(reach);
            match spans.last_mut() {
                Some(last) if start <= last.end => last.end = last.end.max(end),
                _ => spans.push(start..end.min(text.len())),
            }
        }
        let shown = state.current.clone().filter(|_| !state.failing);
        let mut found = Vec::new();
        for span in spans {
            let room = MAX_MATCHES_SHOWN.saturating_sub(found.len());
            search::matches_in(text, invalid, query, span, room, &mut found);
        }
        // The current one alone, and the rest without what overlaps it.
        if let Some(range) = &shown {
            found.retain(|other| other.end <= range.start || other.start >= range.end);
            frame::match_cells(
                area,
                scroll,
                &display,
                std::slice::from_ref(range),
                None,
                current,
            );
        }
        frame::match_cells(area, scroll, &display, &found, None, others);
    }
}
