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
//! - A listing is named after its directory, `crates/`, told apart from another of the same name
//!   by the directories above it, as files are (`crates/<oso>`).
//! - A mark is the first character of its row, changed in place with its faces (`*`), which
//!   moves no cursor.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::Editor;
use crate::buffer::{self, Buffer, BufferId, Mode, Replacement};
use crate::load::LoadSnapshot;
use crate::rope::Rope;
use crate::secret;
use crate::selection::SelectionSet;
use crate::syntax::{Highlight, Highlights};
use crate::window::WindowId;

/// How a listing orders its rows, directories first in each (ADR 0134).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListingOrder {
    /// By name, ignoring ASCII case.
    #[default]
    Name,
    /// By modification time, newest first.
    Date,
}

impl ListingOrder {
    /// What the modeline says of it, as dired's `Dired by name`.
    pub fn note(self) -> &'static str {
        match self {
            Self::Name => "by name",
            Self::Date => "by date",
        }
    }

    /// The other order, as `s` toggles them.
    #[must_use]
    pub fn toggled(self) -> Self {
        match self {
            Self::Name => Self::Date,
            Self::Date => Self::Name,
        }
    }
}

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
    pub order: ListingOrder,
    /// How many rows are marked.
    pub marked: usize,
}

impl Listing {
    /// Where the name of the row at `line` of `text` starts, or the line's end if it is shorter,
    /// as a header or a note line is.
    pub fn name_start(&self, text: &Rope, line: usize) -> usize {
        let line = line.min(text.max_point().row);
        text.line_start(line) + self.name_column.min(text.line_len(line))
    }

    /// What the modeline says after the mode: the order, and how many rows are marked.
    fn note(&self) -> String {
        match self.marked {
            0 => self.order.note().to_owned(),
            marked => format!("{}  {marked} marked", self.order.note()),
        }
    }
}

/// A row whose mark changes (ADR 0134): its line, whether it is marked now, and the face of its
/// name, of `name_len` bytes, which an unmarked row takes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RowMark {
    pub line: usize,
    pub marked: bool,
    pub face: Option<Highlight>,
    pub name_len: usize,
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
    /// List the directory again: `g`.
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
    /// Mark the rows at `lines`, or unmark them (`m`, `u`, `DEL`).
    Mark {
        buffer: BufferId,
        lines: Vec<usize>,
        on: bool,
    },
    /// Unmark every row (`U`).
    UnmarkAll { buffer: BufferId },
    /// Mark every row not marked and unmark those that are (`t`).
    ToggleMarks { buffer: BufferId },
    /// Copy the marked rows, or the rows at `lines` if none is, to `target` (`C`).
    Copy {
        buffer: BufferId,
        lines: Vec<usize>,
        target: PathBuf,
    },
    /// Rename or move the marked rows, or the rows at `lines`, to `target` (`R`).
    Rename {
        buffer: BufferId,
        lines: Vec<usize>,
        target: PathBuf,
    },
    /// Delete the marked rows, or the rows at `lines`, once asked (`D`).
    Delete { buffer: BufferId, lines: Vec<usize> },
    /// Make the directory `path` with its parents (`+`).
    Mkdir { buffer: BufferId, path: PathBuf },
    /// The question about deleting was answered yes: check it all again, then delete.
    DeleteConfirmed,
    /// The question about overwriting was answered yes: check it all again, then copy or move.
    OverwriteConfirmed,
}

/// The listings and what commands asked for them.
#[derive(Debug, Default)]
pub(super) struct Listings {
    records: HashMap<BufferId, Listing>,
    requests: Vec<ListingRequest>,
    /// A file operation runs, which `C-g` may stop.
    running: bool,
    /// `C-g` asked it to stop.
    stop: bool,
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

    /// Notes whether a file operation runs (ADR 0134): while one does, `C-g` with nothing else
    /// to do asks it to stop ([`Editor::quit`]).
    pub fn set_operation_running(&mut self, running: bool) {
        self.listings.running = running;
        self.listings.stop = false;
    }

    /// Whether `C-g` asked the running file operation to stop, once.
    pub fn take_operation_stop(&mut self) -> bool {
        std::mem::take(&mut self.listings.stop)
    }

    /// Asks the running file operation to stop, if one runs and was not asked already: what
    /// `C-g` does with nothing else to do.
    pub(super) fn stop_operation(&mut self) -> bool {
        let asked = self.listings.running && !self.listings.stop;
        self.listings.stop |= asked;
        asked
    }

    /// The directory `C` and `R` offer first (dired's `dired-dwim-target`): that of the next
    /// tiled window, in `C-x o`'s order, that shows a listing, or, with `this_one` or none,
    /// the selected window's listing's. `None` outside a listing.
    pub fn listing_target(&self, this_one: bool) -> Option<PathBuf> {
        let here = self.window();
        let own = self.listing(here.buffer())?.directory.clone();
        if this_one {
            return Some(own);
        }
        let order: Vec<WindowId> = self.frame().windows().iter().map(|(id, _)| *id).collect();
        let at = order.iter().position(|id| *id == here.id()).unwrap_or(0);
        let after = order
            .iter()
            .cycle()
            .skip(at + 1)
            .take(order.len().saturating_sub(1));
        let other = after
            .filter_map(|id| self.window_by_id(*id))
            .find_map(|window| self.listing(window.buffer()));
        Some(other.map_or(own, |listing| listing.directory.clone()))
    }

    /// The file at `from` was renamed or moved to `to`, or a directory holding it was: each
    /// buffer visiting a file there now visits it at its new place, keeping its text and its
    /// edits, and is judged again by its new name, never less secret than before (ADR 0134).
    pub fn file_moved(&mut self, from: &Path, to: &Path) {
        let moved: Vec<(BufferId, PathBuf)> = (self.buffers.iter())
            .filter_map(|(id, buffer)| {
                let rest = buffer.path()?.strip_prefix(from).ok()?;
                let new = if rest.as_os_str().is_empty() {
                    to.to_path_buf()
                } else {
                    to.join(rest)
                };
                Some((*id, new))
            })
            .collect();
        for (id, path) in moved {
            let old_base = (self.buffer(id).and_then(Buffer::path)).map(buffer::file_base);
            if let Some(buffer) = self.buffer_mut(id) {
                buffer.moved_to(path.clone());
            }
            // Those that shared the old name, and those that share the new one, are told apart
            // again.
            if let Some(old_base) = old_base {
                self.name_files_like(&old_base);
            }
            self.name_files_like(&buffer::file_base(&path));
        }
    }

    /// Adds an empty listing buffer for `directory`, named after it, shown in no window yet,
    /// which does not wrap.
    pub fn add_listing(&mut self, directory: PathBuf) -> BufferId {
        let id = self.add_internal_buffer(String::new(), Mode::Directory);
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
        self.name_listings();
        id
    }

    /// Drops the record of the listing `id`, as its buffer goes, and names the rest again.
    pub(super) fn forget_listing(&mut self, id: BufferId) {
        if self.listings.records.remove(&id).is_some() {
            self.name_listings();
        }
    }

    /// Names every listing after its directory, `crates/`, telling apart those of one name by
    /// the directories above them, as files are (`crates/<oso>`), and apart from every other
    /// buffer's name.
    fn name_listings(&mut self) {
        let mut listed: Vec<(BufferId, PathBuf)> = (self.listings.records.iter())
            .filter(|(id, _)| self.buffer(**id).is_some())
            .map(|(id, record)| (*id, record.directory.clone()))
            .collect();
        listed.sort_unstable_by_key(|(id, _)| *id);
        let others: Vec<String> = (self.buffers.iter())
            .filter(|(id, buffer)| {
                !listed.iter().any(|(listing, _)| listing == id) && buffer.path().is_none()
            })
            .map(|(_, buffer)| buffer.name().to_owned())
            .collect();
        // The names files take, without the slash, and then with it after the directory's own.
        let taken: Vec<&str> = others
            .iter()
            .filter_map(|name| name.strip_suffix('/'))
            .collect();
        let paths: Vec<&Path> = listed.iter().map(|(_, path)| path.as_path()).collect();
        let names = buffer::distinct_names(&paths, &taken);
        for ((id, path), name) in listed.iter().zip(names) {
            let named = match path.file_name() {
                Some(base) => {
                    let base = base.to_string_lossy();
                    match name.strip_prefix(base.as_ref()) {
                        Some(rest) => format!("{base}/{rest}"),
                        None => name,
                    }
                }
                // The root, named by its whole path.
                None => name,
            };
            if let Some(buffer) = self.buffer_mut(*id) {
                buffer.set_name(named);
            }
        }
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

    /// Marks or unmarks `rows` of the listing `id` in place: each row's first character becomes
    /// `*` or a space, in the mark's face, and its name takes the marked face or its own back.
    /// `marked` is how many rows are marked now, which the modeline says. No cursor moves and no
    /// undo; O(log n) a row. A line that is no row's, or out of the text, is left alone.
    pub fn set_listing_marks(&mut self, id: BufferId, rows: &[RowMark], marked: usize) {
        let Some(record) = self.listings.records.get(&id).cloned() else {
            return;
        };
        let Some(buffer) = self.buffer(id).filter(|buffer| is_listing(buffer)) else {
            return;
        };
        let text = buffer.text();
        let first = record.header_lines;
        let last = first + record.rows;
        let rows: Vec<(usize, &RowMark)> = rows
            .iter()
            .filter(|row| (first..last).contains(&row.line) && text.line_len(row.line) > 0)
            .map(|row| (text.line_start(row.line), row))
            .collect();
        let edits: Vec<Replacement<'_>> = rows
            .iter()
            .map(|(start, row)| Replacement {
                range: *start..start + 1,
                text: if row.marked { "*" } else { " " },
            })
            .collect();
        let names: Vec<(usize, usize, &RowMark)> = rows
            .iter()
            .map(|(start, row)| (*start, record.name_start(text, row.line), *row))
            .collect();
        let staged = buffer.stage_internal(&edits);
        // The highlights go around the edit, which keeps no generated ones: each mark is one
        // byte for one, so they cover the text after it as before.
        let highlights = self
            .buffer_mut(id)
            .and_then(Buffer::take_generated_highlights);
        if let Some(staged) = staged {
            let views = self.moved_views(id, &staged.display(), staged.steps(), staged.rewrap());
            if let Some(buffer) = self.buffer_mut(id) {
                buffer.commit_internal(staged);
            }
            self.commit_views(id, views);
        }
        let Some(buffer) = self.buffer_mut(id) else {
            return;
        };
        if let Some(mut highlights) = highlights {
            for (start, name, row) in names {
                let mark = row.marked.then_some(Highlight::DiredMark);
                highlights.replace(start..start + 1, [(1, mark)]);
                let face = if row.marked {
                    Some(Highlight::DiredMarked)
                } else {
                    row.face
                };
                highlights.replace(name..name + row.name_len, [(row.name_len, face)]);
            }
            let _ = buffer.set_generated_highlights(highlights);
        }
        let record = Listing { marked, ..record };
        buffer.set_mode_note(record.note());
        self.listings.records.insert(id, record);
    }

    /// Puts in the whole text of the listing `id`, built off the UI thread, with its record and
    /// highlights. O(1) for the text: each window that shows it has its cursor at the name of
    /// the line `lines` gives for it, else of `line`, and a window that will come back to it
    /// at `line` too. No undo. It names the buffer after the directory, and judges it from the
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
            resolved,
            line,
            lines,
        } = shown;
        let verdict = secret::judge(Some(&record.directory), resolved.as_deref());
        let Some(buffer) = self.buffer_mut(id).filter(|buffer| is_listing(buffer)) else {
            return Some(highlights);
        };
        buffer.apply_load(text);
        buffer.set_listing_verdict(verdict);
        buffer.set_mode_note(record.note());
        let old = buffer.take_generated_highlights();
        let refused = if buffer.text().len() == highlights.len() {
            buffer.set_generated_highlights(highlights).or(old)
        } else {
            Some(highlights)
        };
        let start_of = |buffer: &Buffer, line: usize| record.name_start(buffer.text(), line);
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
        let moved = self
            .listing(id)
            .is_none_or(|old| old.directory != record.directory);
        self.listings.records.insert(id, record);
        if moved {
            self.name_listings();
        }
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
