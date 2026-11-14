//! Directory listings (ADR 0134): each listing buffer's record, the text the host builds for it
//! off the UI thread, and what commands ask of the host for listings.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or files; the host lists
//! directories on its workers and hands each listing in whole.
//!
//! - A listing is an internal, read-only buffer in `Directory` mode that does not wrap. Unlike
//!   other internal buffers, a person goes to it with `C-x b`, walks to it with `C-x <left>`, and
//!   kills it with `C-x k`, as Emacs's dired buffers.
//! - Its text comes whole, with its display map, and goes in at O(1), as a file's load does.
//! - What a line names is the host's: the editor keeps only where names start, so that motion
//!   can go to the name's column, and the directory, so that a prompt for a path starts there.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::Editor;
use crate::buffer::{Buffer, BufferId, Mode};
use crate::load::LoadSnapshot;
use crate::secret;
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::WindowId;

/// What the editor knows of a listing buffer's text, which the host sets with it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    /// The directory it lists, as it was named.
    pub directory: PathBuf,
    /// The lines before the first row: the header.
    pub header_lines: usize,
    /// The column of the first byte of each row's name: the same for every row.
    pub name_column: usize,
    /// How many rows it shows, `.` and `..` among them.
    pub rows: usize,
    /// Whether the rows show their details, or only their names (`(`).
    pub details: bool,
    /// What the modeline says after the mode: the order, such as `by name`.
    pub note: String,
}

/// What a command asks of the host for a listing (ADR 0134).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListingRequest {
    /// List `directory` in the selected window, with point on the row of `point` if given:
    /// `C-x d`, and `C-x C-j` from a file.
    Open {
        directory: PathBuf,
        point: Option<OsString>,
    },
    /// Open what the row at `line` of the listing `buffer` names: a file in the window, or in
    /// another with `other_window`; a directory in place of the listing.
    Visit {
        buffer: BufferId,
        line: usize,
        other_window: bool,
    },
    /// List the directory above in place of the listing, with point on the one left.
    Up { buffer: BufferId },
    /// List the directory again: `g`, `revert-buffer`.
    Revert { buffer: BufferId },
    /// Copy the names of the rows at `lines` to the kill ring, or their absolute paths.
    CopyNames {
        buffer: BufferId,
        lines: Vec<usize>,
        absolute: bool,
    },
    /// Show the rows with their details or without (`(`).
    ToggleDetails { buffer: BufferId },
    /// Order the rows by name or by time (`s`).
    ToggleSort { buffer: BufferId },
}

/// The listings and what commands asked for them.
#[derive(Debug, Default)]
pub(super) struct Listings {
    records: HashMap<BufferId, Listing>,
    requests: Vec<ListingRequest>,
}

impl Editor {
    /// Asks the host for `request`.
    pub fn request_listing(&mut self, request: ListingRequest) {
        self.listings.requests.push(request);
    }

    /// What commands asked of the host for listings, oldest first.
    pub fn take_listing_requests(&mut self) -> Vec<ListingRequest> {
        std::mem::take(&mut self.listings.requests)
    }

    /// Adds an empty listing buffer named `name`, shown in no window yet, which does not wrap.
    pub fn add_listing(&mut self, name: impl Into<String>, directory: PathBuf) -> BufferId {
        let id = self.add_internal_buffer(name, Mode::Directory);
        // Judged by its path's words until its real place is known, so that no text of it shows
        // before then.
        let verdict = secret::judge_name(&directory);
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_wraps(false);
            buffer.set_listing_verdict(verdict);
        }
        let record = Listing {
            directory,
            details: true,
            ..Listing::default()
        };
        self.listings.records.insert(id, record);
        id
    }

    /// The record of the listing buffer `id`, if it is one and is still there.
    pub fn listing(&self, id: BufferId) -> Option<&Listing> {
        self.buffer(id)?;
        self.listings.records.get(&id)
    }

    /// Every listing buffer still there, with its record.
    pub fn listings(&self) -> impl Iterator<Item = (BufferId, &Listing)> {
        self.listings
            .records
            .iter()
            .filter(|(id, _)| self.buffer(**id).is_some())
            .map(|(id, record)| (*id, record))
    }

    /// The listing buffer that lists `directory`, if one does.
    pub fn listing_of(&self, directory: &Path) -> Option<BufferId> {
        self.listings()
            .find(|(_, record)| record.directory == directory)
            .map(|(id, _)| id)
    }

    /// Each window's id and the buffer line its cursor is on, for the windows that show the
    /// listing `id`, so that the host can tell which name each is on before it puts in a new
    /// text.
    pub fn listing_points(&self, id: BufferId) -> Vec<(WindowId, usize)> {
        let Some(buffer) = self.buffer(id) else {
            return Vec::new();
        };
        let text = buffer.text();
        self.windows
            .iter()
            .filter(|window| window.buffer() == id)
            .map(|window| {
                let cursor = window.selections.primary().cursor;
                (window.id(), text.offset_to_point(cursor).row)
            })
            .collect()
    }

    /// Puts in the whole text of the listing `id`, built off the UI thread, with its record and
    /// highlights. O(1) for the text: each window that shows it has its cursor at the name of
    /// the line `lines` gives for it, else of `line`, and a window that will come back to it
    /// at `line` too. No undo. It renames the buffer `name`, and judges it from the
    /// directory's real place, `resolved` (`None` if that could not be found), as a file is
    /// judged: a listing inside `.ssh` is withheld as such a file's buffer is. The host keeps
    /// the highlights given back, which replaced, to free them off the UI thread.
    pub fn set_listing_text(
        &mut self,
        id: BufferId,
        text: LoadSnapshot,
        shown: ListingShown,
    ) -> Option<Highlights> {
        let ListingShown {
            record,
            highlights,
            name,
            resolved,
            line,
            lines,
        } = shown;
        let column = record.name_column;
        let verdict = secret::judge(Some(&record.directory), resolved.as_deref());
        let Some(buffer) = self.buffer_mut(id).filter(|buffer| is_listing(buffer)) else {
            return Some(highlights);
        };
        buffer.apply_load(text);
        buffer.set_listing_verdict(verdict);
        buffer.set_mode_note(record.note.clone());
        let old = buffer.take_generated_highlights();
        let refused = if buffer.text().len() == highlights.len() {
            buffer.set_generated_highlights(highlights).or(old)
        } else {
            Some(highlights)
        };
        let start_of = |buffer: &Buffer, line: usize| {
            let line = line.min(buffer.text().max_point().row);
            let start = buffer.text().line_start(line);
            let room = buffer.text().line_len(line);
            start + column.min(room)
        };
        let Some(buffer) = self.buffer(id) else {
            return refused;
        };
        let default = start_of(buffer, line);
        let placed: Vec<(WindowId, usize)> = lines
            .iter()
            .map(|(window, line)| (*window, start_of(buffer, *line)))
            .collect();
        for window in &mut self.windows {
            if let Some(place) = window.places_mut().get_mut(id) {
                place.selections = SelectionSet::new(default);
            }
            if window.buffer() != id {
                continue;
            }
            let at = placed
                .iter()
                .find(|(shown, _)| *shown == window.id())
                .map_or(default, |(_, at)| *at);
            window.selections = SelectionSet::new(at);
        }
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_name(name);
        }
        self.listings.records.insert(id, record);
        self.loaded(id);
        if self.window().buffer() == id {
            self.follow_cursor();
        }
        refused
    }
}

/// What [`Editor::set_listing_text`] puts in beside the text.
#[derive(Debug)]
pub struct ListingShown {
    pub record: Listing,
    /// The highlights, built with the text, covering it whole.
    pub highlights: Highlights,
    /// The buffer's name.
    pub name: String,
    /// The directory's real place and each link's target on the way to it, or `None` if it
    /// could not be found.
    pub resolved: Option<Vec<PathBuf>>,
    /// The line a window's cursor goes to unless `lines` says.
    pub line: usize,
    /// The line each window's cursor goes to.
    pub lines: Vec<(WindowId, usize)>,
}

/// Whether `buffer` is a listing.
pub(super) fn is_listing(buffer: &Buffer) -> bool {
    buffer.is_internal() && buffer.mode() == Mode::Directory
}

#[cfg(test)]
mod tests;
