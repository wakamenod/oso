//! The directory editor's glue (ADR 0134): what its commands ask, the listings the lister's
//! threads build, and the watcher's looks at shown listings' directories.
//!
//! Must not depend on: key presses or how a frame is drawn. Directories are listed on the
//! lister's threads, never here; a listing's text goes in at O(1), and what a line names comes
//! from its row table, never its text.
//!
//! - Moving to another directory from a listing lists it in the same buffer, as dired-single
//!   does; `C-x d` and `C-x C-j` show a listing of the directory if one is open.
//! - A new text keeps each window's point on the name it was on, or on the name a command
//!   asked for, and waits while the editor's text must not change ([`Editor::reload_waits`]).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{BufferId, FileStamp};
use oso_core::editor::{Editor, Listing as Record, ListingRequest, ListingShown};
use oso_core::frame::Way;
use oso_core::selection::SelectionSet;

use super::App;
use crate::dired::{self, Answer, Options, Row};

/// The listings the app keeps, by buffer.
#[derive(Default)]
pub(super) struct Listings {
    by_buffer: HashMap<BufferId, Listing>,
    /// What the next listing shows: the last order and details chosen, as dired keeps them.
    options: Options,
    /// The newest listing built for each buffer while the editor's text had to wait.
    waiting: HashMap<BufferId, Answer>,
    /// The directories the watcher was last given.
    watched: Vec<(BufferId, PathBuf)>,
    next_generation: u64,
}

/// A listing buffer as the app knows it.
struct Listing {
    directory: PathBuf,
    options: Options,
    /// The newest request's: only its answers are shown.
    generation: u64,
    /// What each row's line names, from the listing shown.
    rows: Arc<[Row]>,
    header_lines: usize,
    /// The name point goes to once the next listing comes, as `^` and `C-x C-j` ask.
    point: Option<OsString>,
    /// The line point goes to once the next listing comes, as a restart carried it.
    line: Option<usize>,
    /// What the directory's metadata said when the listing shown was read, once it is whole.
    stamp: Option<Option<FileStamp>>,
    /// What the watcher last saw of it.
    seen: Option<Option<FileStamp>>,
}

impl Listing {
    fn row_at(&self, line: usize) -> Option<&Row> {
        self.rows.get(line.checked_sub(self.header_lines)?)
    }

    fn line_of(&self, name: &OsStr) -> Option<usize> {
        let index = self.rows.iter().position(|row| row.name == name)?;
        Some(self.header_lines + index)
    }
}

impl App {
    /// Does what the directory editor's commands asked.
    pub(super) fn serve_listings(&mut self) {
        self.dired
            .by_buffer
            .retain(|id, _| self.editor.listing(*id).is_some());
        for request in self.editor.take_listing_requests() {
            match request {
                ListingRequest::Open { directory, point } => {
                    self.open_listing_of(&directory, point)
                }
                ListingRequest::Visit {
                    buffer,
                    line,
                    other_window,
                } => self.visit_listed_row(buffer, line, other_window),
                ListingRequest::Up { buffer } => self.list_dir_above(buffer),
                ListingRequest::Revert { buffer } => self.list_again(buffer),
                ListingRequest::CopyNames {
                    buffer,
                    lines,
                    absolute,
                } => self.copy_listed_names(buffer, &lines, absolute),
                ListingRequest::ToggleDetails { buffer } => {
                    self.change_listing_options(buffer, |options| {
                        options.details = !options.details
                    });
                }
                ListingRequest::ToggleSort { buffer } => {
                    self.change_listing_options(buffer, |options| {
                        options.sort = options.sort.toggled()
                    });
                }
            }
        }
        self.show_waiting_listings();
    }

    /// Shows a listing of `directory`, the one open if there is one, with point on `point`.
    fn open_listing_of(&mut self, directory: &Path, point: Option<OsString>) {
        let directory = lexically_clean(directory, self.editor.start_directory());
        if let Some(id) = self.editor.listing_of(&directory) {
            self.editor.show(id);
            if let Some(point) = point {
                self.put_listing_point(id, &point);
            }
            self.list_again(id);
            return;
        }
        let name = listing_name(&self.editor, &directory, None);
        let id = self.editor.add_listing(name, directory.clone());
        let mut header = directory.as_os_str().to_string_lossy().into_owned();
        header.push_str(":\n  listing…\n");
        self.editor.set_internal_text(id, &format!("  {header}"));
        self.editor.show(id);
        let listing = Listing {
            directory,
            options: self.dired.options,
            generation: 0,
            rows: Arc::from([]),
            header_lines: 0,
            point,
            line: None,
            stamp: None,
            seen: None,
        };
        self.dired.by_buffer.insert(id, listing);
        self.list_again(id);
    }

    /// A listing the previous instance held, made again here and listed, unshown: the windows
    /// that showed it show it as they take their place, with point on the line it was on
    /// (ADR 0134).
    #[cfg(feature = "dev")]
    pub(super) fn listing_handed_over(
        &mut self,
        entry: crate::dev::handoff::ListingEntry,
    ) -> BufferId {
        let options = Options {
            sort: if entry.by_date {
                dired::Sort::Time
            } else {
                dired::Sort::Name
            },
            details: entry.details,
        };
        let name = listing_name(&self.editor, &entry.directory, None);
        let id = self.editor.add_listing(name, entry.directory.clone());
        self.dired.options = options;
        let listing = Listing {
            directory: entry.directory,
            options,
            generation: 0,
            rows: Arc::from([]),
            header_lines: 0,
            point: None,
            line: Some(entry.line),
            stamp: None,
            seen: None,
        };
        self.dired.by_buffer.insert(id, listing);
        self.list_again(id);
        id
    }

    /// Opens what the row at `line` of `buffer` names.
    fn visit_listed_row(&mut self, buffer: BufferId, line: usize, other_window: bool) {
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        let Some(row) = listing.row_at(line).cloned() else {
            return self.editor.message("No file on this line");
        };
        let path = listing.directory.join(&row.name);
        if row.name == "." {
            return self.list_again(buffer);
        }
        if row.name == ".." {
            return self.list_dir_above(buffer);
        }
        if other_window {
            self.select_window_for_row();
        }
        if row.enters {
            if other_window {
                return self.open_listing_of(&path, None);
            }
            return self.move_listing_to(buffer, path, None);
        }
        self.editor.request_visit(path, oso_core::load::UNASKED);
        self.serve_files();
    }

    /// Selects the next tiled window, splitting the frame when it has one.
    fn select_window_for_row(&mut self) {
        let here = self.editor.window().id();
        self.editor.other_window(1);
        if self.editor.window().id() == here
            && let Err(error) = self.editor.split_window(Way::Down, true)
        {
            self.editor.message(error.to_string());
        }
    }

    /// Lists the directory above `buffer`'s in its place, with point on the one left.
    fn list_dir_above(&mut self, buffer: BufferId) {
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        let directory = listing.directory.clone();
        let (Some(parent), Some(name)) = (directory.parent(), directory.file_name()) else {
            return self.editor.message("No directory above");
        };
        self.move_listing_to(buffer, parent.to_path_buf(), Some(name.to_os_string()));
    }

    /// Lists `directory` in `buffer`, in place of the one it lists, as dired-single does.
    fn move_listing_to(&mut self, buffer: BufferId, directory: PathBuf, point: Option<OsString>) {
        // A listing of it open already is shown instead, so that no two list one directory.
        if let Some(open) = self.editor.listing_of(&directory)
            && open != buffer
        {
            self.editor.show(open);
            if let Some(point) = point {
                self.put_listing_point(open, &point);
            }
            return self.list_again(open);
        }
        if let Some(listing) = self.dired.by_buffer.get_mut(&buffer) {
            listing.directory = directory;
            listing.point = point;
            listing.stamp = None;
            listing.seen = None;
        }
        self.list_again(buffer);
    }

    /// Asks the lister for `buffer`'s listing again.
    fn list_again(&mut self, buffer: BufferId) {
        self.dired.next_generation += 1;
        let generation = self.dired.next_generation;
        let Some(listing) = self.dired.by_buffer.get_mut(&buffer) else {
            return;
        };
        listing.generation = generation;
        self.lister.list_for_dired(dired::Request {
            buffer,
            generation,
            directory: listing.directory.clone(),
            options: listing.options,
            home: self.editor.home().map(Path::to_path_buf),
        });
    }

    fn change_listing_options(&mut self, buffer: BufferId, change: impl FnOnce(&mut Options)) {
        let Some(listing) = self.dired.by_buffer.get_mut(&buffer) else {
            return;
        };
        change(&mut listing.options);
        self.dired.options = listing.options;
        self.list_again(buffer);
    }

    /// Puts point in every window showing `buffer` on the row of `name`, if it has one.
    fn put_listing_point(&mut self, buffer: BufferId, name: &OsStr) {
        let Some(line) = self
            .dired
            .by_buffer
            .get(&buffer)
            .and_then(|listing| listing.line_of(name))
        else {
            if let Some(listing) = self.dired.by_buffer.get_mut(&buffer) {
                listing.point = Some(name.to_os_string());
            }
            return;
        };
        let column = self
            .editor
            .listing(buffer)
            .map_or(0, |record| record.name_column);
        let (window, shown) = self.editor.current_mut();
        if window.buffer() != buffer {
            return;
        }
        let text = shown.text();
        let at = text.line_start(line) + column.min(text.line_len(line));
        window.selections = SelectionSet::new(at);
        self.editor.follow_cursor();
    }

    /// Copies the names of the rows at `lines`, or their absolute paths, as a kill.
    fn copy_listed_names(&mut self, buffer: BufferId, lines: &[usize], absolute: bool) {
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        let names: Vec<String> = lines
            .iter()
            .filter_map(|line| listing.row_at(*line))
            .filter(|row| row.name != "." && row.name != "..")
            .map(|row| {
                let name = if absolute {
                    listing.directory.join(&row.name).into_os_string()
                } else {
                    row.name.clone()
                };
                name.to_string_lossy().into_owned()
            })
            .collect();
        if names.is_empty() {
            return self.editor.message("No file on this line");
        }
        let text = names.join(" ");
        let secret = self
            .editor
            .buffer(buffer)
            .is_some_and(|shown| shown.is_secret());
        self.editor.kill_new(&text, secret);
        self.write_clipboard();
        if !secret {
            self.editor.message(text);
        }
    }

    /// A lister's thread built a listing: shows it if it is the newest for its buffer, now or
    /// once the editor's text may change. Returns whether a frame is needed.
    pub(super) fn dired_answered(&mut self, answer: Answer) -> bool {
        let newest = self
            .dired
            .by_buffer
            .get(&answer.buffer)
            .is_some_and(|listing| listing.generation == answer.generation);
        if !newest {
            return false;
        }
        if self.editor.reload_waits() {
            self.dired.waiting.insert(answer.buffer, answer);
            return false;
        }
        self.show_built_listing(answer)
    }

    /// Shows the listings that waited, once nothing makes them wait.
    fn show_waiting_listings(&mut self) {
        if self.dired.waiting.is_empty() || self.editor.reload_waits() {
            return;
        }
        let waiting: Vec<Answer> = self
            .dired
            .waiting
            .drain()
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
            return false;
        };
        // Each window stays on the name it was on, or goes to the one asked for.
        let points = self.editor.listing_points(answer.buffer);
        let old_rows = std::mem::replace(&mut listing.rows, Arc::clone(&built.rows));
        let old_header = std::mem::replace(&mut listing.header_lines, built.header_lines);
        let line_of = |name: &OsStr| {
            let index = built.rows.iter().position(|row| row.name == name)?;
            Some(built.header_lines + index)
        };
        let asked = listing.point.as_deref().and_then(line_of).or(listing.line);
        let first = built.header_lines + built.rows.len().min(2);
        let last = (built.header_lines + built.rows.len()).saturating_sub(1);
        let line = asked.unwrap_or(first).min(last.max(built.header_lines));
        let lines: Vec<_> = points
            .iter()
            .filter(|_| asked.is_none())
            .filter_map(|(window, line)| {
                let name = &old_rows.get(line.checked_sub(old_header)?)?.name;
                Some((*window, line_of(name)?))
            })
            .collect();
        // What was asked for holds until the whole listing is in.
        let mut stale = false;
        if built.done {
            listing.point = None;
            listing.line = None;
            listing.stamp = Some(built.stamp);
            // The watcher saw it change while it was read.
            stale = listing.seen.is_some_and(|seen| seen != built.stamp);
        }
        let record = Record {
            directory: listing.directory.clone(),
            header_lines: built.header_lines,
            name_column: built.name_column,
            rows: built.rows.len(),
            details: listing.options.details,
            note: listing.options.sort.note().to_owned(),
        };
        let directory = listing.directory.clone();
        let name = listing_name(&self.editor, &directory, Some(answer.buffer));
        let old_text = self
            .editor
            .buffer(answer.buffer)
            .map(|shown| shown.text().clone());
        let refused = self.editor.set_listing_text(
            answer.buffer,
            built.text,
            ListingShown {
                record,
                highlights: built.highlights,
                name,
                resolved: built.resolved.or_else(|| Some(vec![directory])),
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

    /// The directories of the shown listings, for the watcher, when they changed.
    pub(super) fn watched_listings(&mut self) -> Option<Vec<(BufferId, PathBuf)>> {
        let shown: Vec<(BufferId, PathBuf)> = self
            .dired
            .by_buffer
            .iter()
            .filter(|(id, _)| self.editor.windows().any(|window| window.buffer() == **id))
            .map(|(id, listing)| (*id, listing.directory.clone()))
            .collect();
        let mut sorted = shown;
        sorted.sort_unstable_by_key(|(id, _)| *id);
        if sorted == self.dired.watched {
            return None;
        }
        self.dired.watched.clone_from(&sorted);
        Some(sorted)
    }

    /// What the watcher saw of the shown listings' directories: one that changed since it was
    /// listed is listed again, keeping each window's point on its name.
    pub(super) fn listings_watched(&mut self, seen: Vec<(BufferId, Option<FileStamp>)>) {
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

/// A listing buffer's name: the directory's own name ending in `/`, as `crates/`, with the
/// directories above it in angle brackets when another buffer has that name, as files' are.
fn listing_name(editor: &Editor, directory: &Path, own: Option<BufferId>) -> String {
    let base = directory.file_name().map_or_else(
        || directory.to_string_lossy().into_owned(),
        |name| format!("{}/", name.to_string_lossy()),
    );
    let taken = |name: &str| editor.buffer_named(name).is_some_and(|id| Some(id) != own);
    if !taken(&base) {
        return base;
    }
    let mut above: Vec<String> = Vec::new();
    for parent in directory.ancestors().skip(1) {
        let Some(name) = parent.file_name() else {
            break;
        };
        above.insert(0, name.to_string_lossy().into_owned());
        let named = format!("{base}<{}>", above.join("/"));
        if !taken(&named) {
            return named;
        }
    }
    let mut count = 2;
    loop {
        let named = format!("{base}<{count}>");
        if !taken(&named) {
            return named;
        }
        count += 1;
    }
}

/// `directory` made absolute from `start` and cleaned by its words: `.` dropped and `..` taking
/// the name before it, as Emacs's `expand-file-name` does, with no look at the disk.
fn lexically_clean(directory: &Path, start: &Path) -> PathBuf {
    let joined = if directory.is_absolute() {
        directory.to_path_buf()
    } else {
        start.join(directory)
    };
    let mut clean = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

#[cfg(test)]
mod tests;
