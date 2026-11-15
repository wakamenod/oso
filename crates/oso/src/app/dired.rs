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

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::{BufferId, FileStamp};
use oso_core::editor::ListingRequest;
use oso_core::frame::Way;

use super::App;
use crate::dired::ops::Kind;
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
    /// The file operations (ADR 0134, step 1b-2).
    ops: ops::Ops,
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
    /// The directory the rows are of: `directory` once the listing asked for is shown, and the
    /// one before until then, so that no row's name is taken as one in another directory.
    rows_of: PathBuf,
    header_lines: usize,
    /// The name point goes to once the next listing comes, as `^` and `C-x C-j` ask.
    point: Option<OsString>,
    /// The line point goes to once the next listing comes, as a restart carried it.
    line: Option<usize>,
    /// What the directory's metadata said when the listing shown was read, once it is whole.
    stamp: Option<Option<FileStamp>>,
    /// What the watcher last saw of it.
    seen: Option<Option<FileStamp>>,
    /// The directory's device and inode, from the listing shown.
    identity: Option<(u64, u64)>,
    /// Whether it shows a whole listing of `directory`, which a new one replaces at once,
    /// without partial ones that would cut it short.
    whole: bool,
    /// The names marked, kept over a new listing of the same directory, each one the listing
    /// shows marked (ADR 0134).
    marked: Arc<HashSet<OsString>>,
    /// Whether the next whole listing inverts the marks, as `t` asked.
    invert: bool,
}

impl Listing {
    /// The row at `line`, while the rows shown are of the directory it lists, and only one whose
    /// name is one entry's, so that joining it to the directory names that entry.
    fn row_at(&self, line: usize) -> Option<&Row> {
        if self.rows_of != self.directory {
            return None;
        }
        let row = self.rows.get(line.checked_sub(self.header_lines)?)?;
        let bytes = row.name.as_encoded_bytes();
        let one = !bytes.is_empty()
            && !bytes.contains(&b'/')
            && !bytes.contains(&0)
            && (cfg!(unix) || !bytes.contains(&b'\\'));
        one.then_some(row)
    }
}

impl App {
    /// Does what the directory editor's commands asked.
    pub(super) fn serve_listings(&mut self) {
        // A listing killed: its reading stops, and its rows are freed off this thread.
        let gone: Vec<BufferId> = (self.dired.by_buffer.keys())
            .filter(|id| self.editor.listing(**id).is_none())
            .copied()
            .collect();
        for id in gone {
            self.lister.forget_dired(id);
            let listing = self.dired.by_buffer.remove(&id);
            let waiting = self.dired.waiting.remove(&id);
            self.freer
                .free((listing.map(|listing| listing.rows), waiting));
        }
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
                ListingRequest::Mark { buffer, lines, on } => {
                    self.mark_listed_rows(buffer, &lines, on);
                }
                ListingRequest::UnmarkAll { buffer } => self.unmark_all(buffer),
                ListingRequest::Copy {
                    buffer,
                    lines,
                    target,
                } => self.ask_for_operation(buffer, Kind::Copy, &lines, target),
                ListingRequest::Rename {
                    buffer,
                    lines,
                    target,
                } => self.ask_for_operation(buffer, Kind::Rename, &lines, target),
                ListingRequest::Delete { buffer, lines } => {
                    self.ask_for_operation(buffer, Kind::Delete, &lines, PathBuf::new());
                }
                ListingRequest::Mkdir { buffer, path } => self.make_listed_directory(buffer, path),
                ListingRequest::DeleteConfirmed => self.operation_confirmed(false),
                ListingRequest::OverwriteConfirmed => self.operation_confirmed(true),
                ListingRequest::ToggleMarks { buffer } => self.toggle_marks(buffer),
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
        if let Some(id) = self.listing_of_directory(&directory) {
            self.editor.show(id);
            if let Some(listing) = self.dired.by_buffer.get_mut(&id) {
                listing.point = point.or(listing.point.take());
            }
            self.list_again(id);
            return;
        }
        let id = self.editor.add_listing(directory.clone());
        let mut header = b"  ".to_vec();
        dired::rows::push_name(&mut header, directory.as_os_str().as_encoded_bytes());
        header.extend_from_slice(":\n  listing…\n".as_bytes());
        self.editor
            .set_internal_text(id, &String::from_utf8_lossy(&header));
        self.editor.show(id);
        let listing = Listing {
            directory,
            options: self.dired.options,
            generation: 0,
            rows: Arc::from([]),
            rows_of: PathBuf::new(),
            header_lines: 0,
            point,
            line: None,
            stamp: None,
            seen: None,
            identity: None,
            whole: false,
            marked: Arc::default(),
            invert: false,
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
        added: &mut Vec<BufferId>,
    ) -> BufferId {
        let options = Options {
            sort: if entry.by_date {
                dired::Sort::Date
            } else {
                dired::Sort::Name
            },
            details: entry.details,
        };
        let id = self.editor.add_listing(entry.directory.clone());
        self.dired.options = options;
        let listing = Listing {
            directory: entry.directory,
            options,
            generation: 0,
            rows: Arc::from([]),
            rows_of: PathBuf::new(),
            header_lines: 0,
            point: None,
            line: Some(entry.line),
            stamp: None,
            seen: None,
            identity: None,
            whole: false,
            marked: Arc::default(),
            invert: false,
        };
        self.dired.by_buffer.insert(id, listing);
        self.list_again(id);
        added.push(id);
        id
    }

    /// Opens what the row at `line` of `buffer` names.
    fn visit_listed_row(&mut self, buffer: BufferId, line: usize, other_window: bool) {
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        if listing.rows_of != listing.directory {
            return self.editor.message("Still listing the directory");
        }
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
        if let Some(open) = self.listing_of_directory(&directory)
            && open != buffer
        {
            self.editor.show(open);
            if let Some(listing) = self.dired.by_buffer.get_mut(&open) {
                listing.point = point.or(listing.point.take());
            }
            return self.list_again(open);
        }
        if let Some(listing) = self.dired.by_buffer.get_mut(&buffer) {
            listing.directory = directory;
            listing.point = point;
            listing.stamp = None;
            listing.seen = None;
            listing.whole = false;
            listing.marked = Arc::default();
            listing.invert = false;
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
            partials: !listing.whole,
            marked: Arc::clone(&listing.marked),
            invert: listing.invert,
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

    /// The listing that lists `directory`, as the app knows it: it moves to the directory it
    /// lists next as soon as it is asked to.
    fn listing_of_directory(&self, directory: &Path) -> Option<BufferId> {
        (self.dired.by_buffer.iter())
            .find(|(_, listing)| listing.directory == directory)
            .map(|(id, _)| *id)
    }

    /// Copies the names of the rows at `lines`, or their absolute paths, as a kill.
    fn copy_listed_names(&mut self, buffer: BufferId, lines: &[usize], absolute: bool) {
        let Some(listing) = self.dired.by_buffer.get(&buffer) else {
            return;
        };
        let rows: Vec<&Row> = lines
            .iter()
            .filter_map(|line| listing.row_at(*line))
            .filter(|row| row.name != "." && row.name != "..")
            .collect();
        if rows.is_empty() {
            return self.editor.message("No file on this line");
        }
        // Copied as text, a name that is not UTF-8 would name another file, and a line break in
        // one would read as two.
        let mut names = Vec::with_capacity(rows.len());
        for row in rows {
            let name = if absolute {
                listing.directory.join(&row.name).into_os_string()
            } else {
                row.name.clone()
            };
            match name.into_string() {
                Ok(name) if !name.contains(['\n', '\r']) => names.push(name),
                _ => return self.editor.message("A name is not plain text; not copied"),
            }
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

mod marks;
mod ops;
mod show;
#[cfg(test)]
mod tests;
