//! A list's preview of a file at a place (ADR 0114, ADR 0138): the selected row's file around
//! its line, from an open buffer of that file or from what the host read of it, for a project's
//! search, the list of diagnostics and the list of a server's places, each with its own
//! [`FilePreview`].
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host reads the
//! file and hands over an [`Excerpt`] of it.
//!
//! - A row of a file that may hold secrets is never previewed, and nothing of it is asked for.
//! - An open buffer whose path is the row's file is shown from memory. Any other file is read by
//!   the host, once per row selected; a newer selection moves the generation, which the host
//!   reads before each step and each part it reads, and a read that comes for an older one is
//!   dropped. Once read, an open buffer of the very same file, by device and inode, is shown
//!   instead, as `RET` would show it.
//! - A place is the offset of its line in the file, as a search found it, or only its line, as
//!   a server gives it, which the host finds by counting line breaks from the file's start, as
//!   a buffer counts them: a lone `\r` breaks no line.
//! - An excerpt keeps at most [`EXCERPT_LINES`] lines of at most [`EXCERPT_LINE`] bytes each;
//!   the selected row's line keeps as many around its place. Its display model is built where
//!   it is made, on the host's thread, so the UI thread measures nothing of it.
//! - The preview never wraps here; it scrolls left as far as needed for the place to show.
//! - Nothing of a request or an excerpt reaches a log; `Debug` gives a generation at most.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::buffer::BufferId;
use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
use crate::editor::visits::{VisitColumn, column_offset, file_identity};
use crate::editor::{Editor, Preview};
use crate::project::{CUT, cut_bounds, decoded_len, push_cut};
use crate::rope::Rope;

/// The most bytes of a line an excerpt keeps, cut at characters. The host's reader reads no
/// more than this and a line break of each line after the row's (`PER_ROW` in the `oso`
/// crate's `grep/preview.rs`); the two must change together.
pub(crate) const EXCERPT_LINE: usize = 4096;

/// The most lines an excerpt keeps.
pub(crate) const EXCERPT_LINES: usize = 256;

/// How much of the selected row's line an excerpt keeps before its place, once the place is
/// too far into the line for the line's start to be kept with it.
const BEFORE_PLACE: usize = 1024;

/// Where in its file a row's place is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewAt {
    /// The line from 1 whose start is `offset` bytes into the file, as a search found it, and
    /// `column` characters into it.
    Offset {
        offset: u64,
        line: usize,
        column: usize,
    },
    /// The line from 1, as a server gives it, which the host finds from the file's start, and
    /// the column on it as the server counts it.
    Line { line: usize, column: VisitColumn },
}

impl PreviewAt {
    /// The place's line, from 1.
    pub fn line(self) -> usize {
        match self {
            Self::Offset { line, .. } | Self::Line { line, .. } => line,
        }
    }

    /// The place's column, as what gave it counts.
    pub fn column(self) -> VisitColumn {
        match self {
            Self::Offset { column, .. } => VisitColumn::Chars(column),
            Self::Line { column, .. } => column,
        }
    }
}

/// A file the host should read for the preview of a list's row (ADR 0114, ADR 0138). `Debug`
/// gives its generation only.
#[derive(Clone)]
pub struct FilePreviewRequest {
    /// The read it is, which [`Editor::preview_read`] is given back.
    pub generation: u64,
    /// The directory the file is reached from, a directory at a time: the project's root.
    pub root: PathBuf,
    /// The file's path from the root.
    pub path: Box<str>,
    /// The row's place in it.
    pub at: PreviewAt,
    /// The rows the preview shows: about half of them before the line, the rest from it.
    pub rows: usize,
    /// The newest read's generation: the host stops once this has moved past
    /// [`FilePreviewRequest::generation`].
    pub live: Arc<AtomicU64>,
}

impl fmt::Debug for FilePreviewRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilePreviewRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What the host read for a preview.
pub enum FilePreviewRead {
    /// The lines around the row's.
    Text(Excerpt),
    /// The file may hold secrets, as the host judged it when it read it: nothing of it shows.
    Withheld,
    /// It could not be read as the list's rows reached it: gone, no longer a regular file,
    /// reached through a link, or its line too far in. The preview shows nothing.
    Unreadable,
}

impl fmt::Debug for FilePreviewRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text(_) => "Text",
            Self::Withheld => "Withheld",
            Self::Unreadable => "Unreadable",
        })
    }
}

/// The lines of a file around a row's line, as the preview shows them, with the row's place in
/// them and the file's device and inode. `Debug` says nothing of it.
pub struct Excerpt {
    text: Rope,
    display: DisplayMap,
    /// Where the row's place is in `text`.
    cursor: usize,
    identity: Option<(u64, u64)>,
}

impl fmt::Debug for Excerpt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Excerpt").finish_non_exhaustive()
    }
}

impl Excerpt {
    /// The excerpt of `bytes`, whole lines of a file from its line `first_line`, from 1, around
    /// the row's line `line`, whose place is at `column`, past the line's end at its end, and of
    /// the file whose device and inode are `identity`. Each invalid byte is one U+FFFD, one
    /// character and one UTF-16 unit, and a `\r` before a line break is dropped, as a row shows
    /// them; see the module's notes for the bounds. Builds its display model, O(n) in what it
    /// keeps, so the host's thread makes it.
    pub fn new(
        bytes: &[u8],
        first_line: usize,
        (line, column): (usize, VisitColumn),
        identity: Option<(u64, u64)>,
    ) -> Self {
        let hit = line.checked_sub(first_line);
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let mut text = String::with_capacity(bytes.len().min(EXCERPT_LINES * EXCERPT_LINE));
        let mut cursor = None;
        for (index, line) in bytes
            .split(|&byte| byte == b'\n')
            .take(EXCERPT_LINES)
            .enumerate()
        {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if index > 0 {
                text.push('\n');
            }
            if Some(index) == hit {
                cursor = Some(push_around(&mut text, line, column));
            } else {
                push_cut(
                    &mut text,
                    line,
                    cut_bounds(line, 0, EXCERPT_LINE.min(line.len())),
                );
            }
        }
        let cursor = cursor.unwrap_or(text.len());
        let text = Rope::from(text.as_str());
        let display = DisplayMap::new(&text, TabWidth::default());
        Self {
            text,
            display,
            cursor,
            identity,
        }
    }
}

/// Pushes the part of `line` around its place at `column` onto `text`, and gives where the
/// place is in `text`.
fn push_around(text: &mut String, line: &[u8], column: VisitColumn) -> usize {
    let at = byte_of_column(line, column);
    let start = if at + BEFORE_PLACE <= EXCERPT_LINE {
        0
    } else {
        at - BEFORE_PLACE
    };
    let end = (start + EXCERPT_LINE).min(line.len());
    let (start, end) = cut_bounds(line, start, end);
    let before = text.len();
    push_cut(text, line, (start, end));
    let mut cursor = before + if start > 0 { CUT.len_utf8() } else { 0 };
    cursor += decoded_len(&line[start..at.clamp(start, end)]);
    cursor
}

/// The byte of `line` at `column`, at a character's start, each invalid byte one character and
/// one UTF-16 unit; the line's end past its last. A column inside a character is at its start.
pub(crate) fn byte_of_column(line: &[u8], column: VisitColumn) -> usize {
    // What each character counts for, and how far into the line the column is.
    let (left, width): (usize, fn(char) -> usize) = match column {
        VisitColumn::Chars(chars) => (chars, |_| 1),
        VisitColumn::Utf8(bytes) => (bytes, char::len_utf8),
        VisitColumn::Utf16(units) => (units, char::len_utf16),
    };
    let mut left = left;
    let mut at = 0;
    for chunk in line.utf8_chunks() {
        for (index, char) in chunk.valid().char_indices() {
            let counts = width(char);
            if left < counts {
                return at + index;
            }
            left -= counts;
        }
        at += chunk.valid().len();
        for _ in chunk.invalid() {
            if left == 0 {
                return at;
            }
            left -= 1;
            at += 1;
        }
    }
    line.len()
}

/// What a list keeps for the preview of its rows' files.
#[derive(Default)]
pub(super) struct FilePreview {
    /// The newest read's generation, shared with the host.
    live: Arc<AtomicU64>,
    /// The row asked for last, by its list's generation and its item.
    asked: Option<(u64, usize)>,
    /// An open buffer showed the row asked for, so nothing was read: once it goes, it is.
    covered: bool,
    /// What came for the row asked for, and the revision the frame draws it at.
    read: Option<(FilePreviewRead, u64)>,
    /// The revision the last excerpt was given, so that each is shaped as its own.
    revision: u64,
}

impl FilePreview {
    /// Stops the read under way, and forgets what was read: the rows it was for have gone.
    pub(super) fn reset(&mut self) {
        self.live.fetch_add(1, Ordering::Relaxed);
        self.asked = None;
        self.covered = false;
        self.read = None;
    }

    /// The row asked for last, by its list's generation and its item.
    pub(super) fn asked(&self) -> Option<(u64, usize)> {
        self.asked
    }

    /// Asks afresh for the row `key`, of a list whose rows are no file (ADR 0145): the read
    /// before stops, and this gives the new read's generation and the counter the host reads.
    pub(super) fn ask(&mut self, key: (u64, usize)) -> (u64, Arc<AtomicU64>) {
        self.reset();
        self.asked = Some(key);
        (self.live.load(Ordering::Relaxed), Arc::clone(&self.live))
    }

    /// What was read for the row `key`, as the preview shows it, once it came.
    pub(super) fn shown(&self, key: (u64, usize)) -> Option<Preview> {
        if self.asked != Some(key) {
            return None;
        }
        match &self.read {
            Some((FilePreviewRead::Text(excerpt), revision)) => Some(Preview::Excerpt {
                revision: *revision,
                cursor: excerpt.cursor,
            }),
            Some((FilePreviewRead::Withheld, _)) => Some(Preview::Withheld),
            Some((FilePreviewRead::Unreadable, _)) | None => None,
        }
    }
}

/// The lists that preview their rows' files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lister {
    /// A project's search (ADR 0114).
    Search,
    /// The list of a project's diagnostics, `M-g d` (ADR 0138).
    Diagnostics,
    /// The list of a server's places, `M-.`'s and `M-?`'s (ADR 0138, step 8b).
    Xrefs,
}

/// The selected row of a list, as its preview wants it: its key, its list's generation and its
/// item, the root and the path from it of its file, its place, and whether its file may hold
/// secrets.
pub(super) struct Wanted<'a> {
    pub(super) key: (u64, usize),
    pub(super) root: &'a Path,
    pub(super) path: &'a Path,
    pub(super) at: PreviewAt,
    pub(super) secret: bool,
}

impl Editor {
    /// The innermost prompt's list, if it previews its rows' files.
    pub(super) fn previewing(&self) -> Option<Lister> {
        if self.searches_project() {
            Some(Lister::Search)
        } else if self.lists_diagnostics() {
            Some(Lister::Diagnostics)
        } else if self.lists_xrefs() {
            Some(Lister::Xrefs)
        } else {
            None
        }
    }

    fn file_preview(&self, lister: Lister) -> &FilePreview {
        match lister {
            Lister::Search => &self.project_search.preview,
            Lister::Diagnostics => &self.servers.list.listed.preview,
            Lister::Xrefs => &self.servers.xrefs.preview,
        }
    }

    fn file_preview_mut(&mut self, lister: Lister) -> &mut FilePreview {
        match lister {
            Lister::Search => &mut self.project_search.preview,
            Lister::Diagnostics => &mut self.servers.list.listed.preview,
            Lister::Xrefs => &mut self.servers.xrefs.preview,
        }
    }

    /// The selected row of `lister`'s list, if it is one of a file.
    fn wanted_row(&self, lister: Lister) -> Option<Wanted<'_>> {
        match lister {
            Lister::Search => self.search_wanted(),
            Lister::Diagnostics => self.diagnostic_wanted(),
            Lister::Xrefs => self.xref_wanted(),
        }
    }

    /// An open buffer of the file at `path` from `root`, as it is spelled, that shows what it
    /// holds.
    pub(super) fn find_open_buffer_at(&self, root: &Path, path: &Path) -> Option<BufferId> {
        // The name first, which rules out most buffers without walking their paths.
        self.find_open_buffer(|_, buffer| {
            buffer.path().is_some_and(|whole| {
                whole.file_name() == path.file_name()
                    && whole
                        .strip_prefix(root)
                        .is_ok_and(|relative| relative == path)
            })
        })
    }

    /// The read the host should start for the selected row's preview, once per row selected:
    /// none for a row of a file that may hold secrets, one an open buffer shows, or while the
    /// float has no room for a preview. A row an open buffer showed is read once that buffer
    /// has gone.
    pub fn take_preview_request(&mut self) -> Option<FilePreviewRequest> {
        let lister = self.previewing()?;
        let wanted = self.wanted_row(lister)?;
        let key = wanted.key;
        let preview = self.file_preview(lister);
        if preview.asked == Some(key)
            && !(preview.covered && self.find_open_buffer_at(wanted.root, wanted.path).is_none())
        {
            return None;
        }
        let rows = self
            .prompt_layout()
            .map(|layout| layout.regions.preview)
            .filter(|region| region.rows > 0 && region.columns > 0)?
            .rows;
        let open = self.find_open_buffer_at(wanted.root, wanted.path).is_some();
        let (at, secret) = (wanted.at, wanted.secret);
        let path: Box<str> = wanted.path.to_str()?.into();
        let root = wanted.root.to_path_buf();
        let preview = self.file_preview_mut(lister);
        preview.reset();
        preview.asked = Some(key);
        preview.covered = open && !secret;
        if secret || open {
            return None;
        }
        Some(FilePreviewRequest {
            generation: preview.live.load(Ordering::Relaxed),
            root,
            path,
            at,
            rows,
            live: Arc::clone(&preview.live),
        })
    }

    /// Takes in what the host read for the preview `generation`: dropped unless it is the
    /// newest of the innermost prompt's list.
    pub fn preview_read(&mut self, generation: u64, read: FilePreviewRead) {
        let Some(lister) = self.previewing() else {
            return;
        };
        let preview = self.file_preview(lister);
        if preview.asked.is_none() || preview.live.load(Ordering::Relaxed) != generation {
            return;
        }
        // The very file of an open buffer that withholds keeps nothing of what was read.
        let read = match read {
            FilePreviewRead::Text(excerpt) if self.withheld_file(excerpt.identity) => {
                FilePreviewRead::Withheld
            }
            read => read,
        };
        let preview = self.file_preview_mut(lister);
        preview.revision += 1;
        preview.read = Some((read, preview.revision));
    }

    /// What the preview of `lister`'s selected row of a file shows.
    pub(super) fn file_row_preview(&self, lister: Lister) -> Option<Preview> {
        let wanted = self.wanted_row(lister)?;
        if wanted.secret {
            return Some(Preview::Withheld);
        }
        let (line, column) = (wanted.at.line(), wanted.at.column());
        if let Some(id) = self.find_open_buffer_at(wanted.root, wanted.path) {
            return self.buffer_preview(id, line, column);
        }
        let preview = self.file_preview(lister);
        if preview.asked != Some(wanted.key) {
            return None;
        }
        match &preview.read {
            Some((FilePreviewRead::Text(excerpt), revision)) => {
                let same = |_, buffer: &_| {
                    excerpt
                        .identity
                        .is_some_and(|identity| file_identity(buffer) == Some(identity))
                };
                match self.find_open_buffer(same) {
                    Some(id) => self.buffer_preview(id, line, column),
                    None => Some(Preview::Excerpt {
                        revision: *revision,
                        cursor: excerpt.cursor,
                    }),
                }
            }
            Some((FilePreviewRead::Withheld, _)) => Some(Preview::Withheld),
            Some((FilePreviewRead::Unreadable, _)) | None => None,
        }
    }

    /// The preview of the open buffer `id` at `line`, from 1, and `column`, each as far as its
    /// text goes, with that line about halfway down; nothing of it if it may hold secrets.
    fn buffer_preview(&self, id: BufferId, line: usize, column: VisitColumn) -> Option<Preview> {
        let cursor = column_offset(self.buffer(id)?.text(), line.saturating_sub(1), column);
        self.buffer_preview_at(id, cursor)
    }

    /// The excerpt read for the selected row, if one was: what [`Preview::Excerpt`] shows,
    /// unless an open buffer of the same file shows in its place.
    pub(in crate::editor) fn read_excerpt(&self) -> Option<&Excerpt> {
        let lister = self.previewing()?;
        // A search's row of a conversation has no file, but has an excerpt (ADR 0145).
        let key = match lister {
            Lister::Search => self.selected_search_row()?,
            Lister::Diagnostics | Lister::Xrefs => self.wanted_row(lister)?.key,
        };
        let preview = self.file_preview(lister);
        if preview.asked != Some(key) {
            return None;
        }
        match &preview.read {
            Some((FilePreviewRead::Text(excerpt), _)) => Some(excerpt),
            _ => None,
        }
    }

    /// The display model of the excerpt read for the selected row, which
    /// [`Preview::Excerpt`] shows: none while the preview is anything else, as where an open
    /// buffer of the same file shows, or withholds, in its place.
    pub fn preview_excerpt_display(&self) -> Option<DisplaySnapshot> {
        if !matches!(self.preview(), Some(Preview::Excerpt { .. })) {
            return None;
        }
        self.read_excerpt()
            .map(|excerpt| excerpt.display.snapshot())
    }

    /// Whether an open buffer of the file whose device and inode are `identity` withholds what it
    /// holds, as one that may hold secrets.
    fn withheld_file(&self, identity: Option<(u64, u64)>) -> bool {
        let Some(identity) = identity else {
            return false;
        };
        self.find_open_buffer(|_, buffer| {
            file_identity(buffer) == Some(identity) && buffer.verdict().is_withheld()
        })
        .is_some()
    }

    /// The excerpt read for the selected row, its text and its display model, and where the
    /// preview starts it.
    pub(in crate::editor) fn shown_excerpt(&self) -> Option<(&Rope, DisplaySnapshot, usize)> {
        let excerpt = self.read_excerpt()?;
        let display = excerpt.display.snapshot();
        let start = self.preview_start(&display, excerpt.cursor);
        Some((&excerpt.text, display, start))
    }

    /// The column a list's preview of a file scrolls to, so that the place at `cursor` shows in
    /// `columns`: 0 while it is within the first three quarters, and otherwise with a third of
    /// the columns before it.
    pub(in crate::editor) fn preview_left(
        display: &DisplaySnapshot,
        cursor: usize,
        columns: usize,
    ) -> usize {
        let column = display.offset_to_display_point(cursor).column;
        if column < columns - columns / 4 {
            0
        } else {
            column.saturating_sub(columns / 3)
        }
    }
}

mod wrapped;

#[cfg(test)]
mod tests;
