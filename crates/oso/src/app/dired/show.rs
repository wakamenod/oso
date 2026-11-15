//! The directory editor's listings as they come back (ADR 0134): each one shown if it is the
//! newest for its buffer, now or once the editor's text may change, with each window's point
//! kept on its name; and the watcher's looks at shown listings' directories.
//!
//! Must not depend on: key presses or how a frame is drawn. What a listing replaces, and one no
//! longer wanted, is freed off the UI thread, and no name is looked for row by row but the one a
//! command asked for, once the whole listing is in.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use oso_core::buffer::{BufferId, FileStamp};
use oso_core::editor::{Listing as Record, ListingShown};

use super::App;
use crate::dired::{self, Answer};

impl App {
    /// A lister's thread built a listing: shows it if it is the newest for its buffer, now or
    /// once the editor's text may change. Returns whether a frame is needed.
    pub(in crate::app) fn dired_answered(&mut self, answer: Answer) -> bool {
        let newest = self
            .dired
            .by_buffer
            .get(&answer.buffer)
            .is_some_and(|listing| listing.generation == answer.generation);
        if !newest {
            self.freer.free(answer);
            return false;
        }
        if self.editor.reload_waits() {
            let replaced = self.dired.waiting.insert(answer.buffer, answer);
            self.freer.free(replaced);
            return false;
        }
        self.show_built_listing(answer)
    }

    /// Shows the listings that waited, once nothing makes them wait.
    pub(super) fn show_waiting_listings(&mut self) {
        if self.dired.waiting.is_empty() || self.editor.reload_waits() {
            return;
        }
        let waiting: Vec<Answer> = (self.dired.waiting.drain())
            .map(|(_, answer)| answer)
            .collect();
        let mut shown = false;
        for answer in waiting {
            shown |= self.dired_answered(answer);
        }
        if shown {
            self.request_redraw();
        }
    }

    fn show_built_listing(&mut self, answer: Answer) -> bool {
        let built = match answer.built {
            Ok(built) => built,
            Err(why) => {
                self.editor
                    .message(format!("Cannot list the directory: {why}"));
                return false;
            }
        };
        let Some(listing) = self.dired.by_buffer.get_mut(&answer.buffer) else {
            self.freer.free(built);
            return false;
        };
        let sort = listing.options.sort;
        // A listing of the same directory again: each window stays on the name it was on, found
        // by what the rows are ordered by. One of another keeps no window's name.
        let refresh = listing.whole;
        let points = if refresh && built.done {
            self.editor.listing_points(answer.buffer)
        } else {
            Vec::new()
        };
        let old_rows = std::mem::replace(&mut listing.rows, Arc::clone(&built.rows));
        listing.rows_of.clone_from(&listing.directory);
        let old_header = std::mem::replace(&mut listing.header_lines, built.header_lines);
        let header = built.header_lines;
        // The name a command asked for, once the whole listing is in.
        let asked = if built.done {
            (listing.point.as_deref())
                .and_then(|name| dired::position_of_name(&built.rows, name))
                .map(|index| header + index)
                .or(listing.line)
        } else {
            None
        };
        let first = header + built.rows.len().min(2);
        let last = (header + built.rows.len()).saturating_sub(1);
        let line = asked.unwrap_or(first).min(last.max(header));
        let lines: Vec<_> = points
            .iter()
            .filter(|_| asked.is_none())
            .filter_map(|(window, line)| {
                let old = old_rows.get(line.checked_sub(old_header)?)?;
                let index = dired::position(&built.rows, sort, old)?;
                Some((*window, header + index))
            })
            .collect();
        let stale = built.done && whole_listing_in(listing, built.marked_names, built.stamp);
        let record = Record {
            directory: listing.directory.clone(),
            header_lines: header,
            name_column: built.name_column,
            rows: built.rows.len(),
            details: listing.options.details,
            order: sort,
            marked: built.marked,
        };
        let old_text = (self.editor.buffer(answer.buffer)).map(|shown| shown.text().clone());
        let refused = self.editor.set_listing_text(
            answer.buffer,
            built.text,
            ListingShown {
                record,
                highlights: built.highlights,
                resolved: built.resolved,
                line,
                lines,
            },
        );
        // What the listing replaced is freed off this thread.
        self.freer.free((old_text, refused, old_rows));
        self.buffers_changed();
        if stale {
            self.list_again(answer.buffer);
        }
        true
    }

    /// The directories of the shown listings, for the watcher, when they changed since it was
    /// last given them. Nothing is allocated while they have not.
    pub(in crate::app) fn listings_to_watch(&mut self) -> Option<Vec<(BufferId, PathBuf)>> {
        let shown = |id: &BufferId| self.editor.windows().any(|window| window.buffer() == *id);
        let watched = &self.dired.watched;
        let count = (self.dired.by_buffer.keys()).filter(|id| shown(id)).count();
        let same = count == watched.len()
            && watched.iter().all(|(id, directory)| {
                shown(id)
                    && (self.dired.by_buffer.get(id))
                        .is_some_and(|listing| listing.directory == *directory)
            });
        if same {
            return None;
        }
        let mut now: Vec<(BufferId, PathBuf)> = (self.dired.by_buffer.iter())
            .filter(|(id, _)| shown(id))
            .map(|(id, listing)| (*id, listing.directory.clone()))
            .collect();
        now.sort_unstable_by_key(|(id, _)| *id);
        self.dired.watched.clone_from(&now);
        Some(now)
    }

    /// What the watcher saw of the shown listings' directories: one that changed since it was
    /// listed is listed again, keeping each window's point on its name.
    pub(in crate::app) fn listing_stamps_seen(&mut self, seen: Vec<(BufferId, Option<FileStamp>)>) {
        let mut changed = Vec::new();
        for (id, stamp) in seen {
            let Some(listing) = self.dired.by_buffer.get_mut(&id) else {
                continue;
            };
            listing.seen = Some(stamp);
            // Once, until the listing asked for comes whole.
            if listing.stamp.is_some_and(|listed| listed != stamp) {
                listing.stamp = None;
                changed.push(id);
            }
        }
        for id in changed {
            self.list_again(id);
        }
    }
}

/// Notes that the whole listing asked for is in: what a command asked for is done, the marks it
/// inverted, as `t` asked, are the ones kept from now on, and the stamp read before it is the
/// listing's. Whether the watcher saw the directory change while it was read.
fn whole_listing_in(
    listing: &mut super::Listing,
    marked: Option<Arc<HashSet<OsString>>>,
    stamp: Option<FileStamp>,
) -> bool {
    listing.point = None;
    listing.line = None;
    if let Some(marked) = marked {
        listing.marked = marked;
        listing.invert = false;
    }
    listing.stamp = Some(stamp);
    listing.whole = true;
    listing.seen.is_some_and(|seen| seen != stamp)
}
