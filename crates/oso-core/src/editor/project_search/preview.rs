//! The preview of a project's search (ADR 0114): the selected row's file around its line, with
//! the match highlighted, from an open buffer of that file or from what the host read of it.
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
//! - An excerpt keeps at most [`EXCERPT_LINES`] lines of at most [`EXCERPT_LINE`] bytes each;
//!   the selected row's line keeps as many around its match. Its display model is built where
//!   it is made, on the host's thread, so the UI thread measures nothing of it.
//! - The preview never wraps here; it scrolls left as far as needed for the match to show.
//! - Nothing of a request or an excerpt reaches a log; `Debug` gives a generation at most.

use std::fmt;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::buffer::BufferId;
use crate::display_map::{DisplayMap, DisplayPoint, DisplaySnapshot, TabWidth};
use crate::editor::visits::place_offset;
use crate::editor::{Editor, MatchCells, Preview};
use crate::frame;
use crate::project::decoded;
use crate::rope::Rope;
use crate::sum_tree::Bias;
use crate::window::Screen;

/// The most bytes of a line an excerpt keeps, cut at characters.
pub(crate) const EXCERPT_LINE: usize = 4096;

/// The most lines an excerpt keeps.
pub(crate) const EXCERPT_LINES: usize = 256;

/// How much of the selected row's line an excerpt keeps before its match, once the match is too
/// far into the line for the line's start to be kept with it.
const BEFORE_MATCH: usize = 1024;

/// What an excerpt shows where it leaves part of a line out.
const CUT: char = '…';

/// How much of a line around what a row draws is matched for the preview's highlights.
const AROUND_DRAWN: usize = EXCERPT_LINE;

/// A file the host should read for the preview of a search's row (ADR 0114). `Debug` gives its
/// generation only.
#[derive(Clone)]
pub struct ProjectPreviewRequest {
    /// The read it is, which [`Editor::preview_read`] is given back.
    pub generation: u64,
    /// The project's root, which the file is reached from, a directory at a time.
    pub root: PathBuf,
    /// The file's path from the root.
    pub path: Box<str>,
    /// The offset of the row's line in the file, in bytes, as the search found it.
    pub offset: u64,
    /// The row's line, from 1.
    pub line: usize,
    /// The column of the row's match, in characters from the line's start.
    pub column: usize,
    /// The rows the preview shows: about half of them before the line, the rest from it.
    pub rows: usize,
    /// The newest read's generation: the host stops once this has moved past
    /// [`ProjectPreviewRequest::generation`].
    pub live: Arc<AtomicU64>,
}

impl fmt::Debug for ProjectPreviewRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectPreviewRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// What the host read for a preview.
pub enum ProjectPreviewRead {
    /// The lines around the row's.
    Text(Excerpt),
    /// The file may hold secrets, as the host judged it when it read it: nothing of it shows.
    Withheld,
    /// It could not be read as the search read it: gone, no longer a regular file, or reached
    /// through a link. The preview shows nothing.
    Unreadable,
}

impl fmt::Debug for ProjectPreviewRead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text(_) => "Text",
            Self::Withheld => "Withheld",
            Self::Unreadable => "Unreadable",
        })
    }
}

/// The lines of a file around a search's row, as the preview shows them, with the place of its
/// match and the file's device and inode. `Debug` says nothing of it.
pub struct Excerpt {
    text: Rope,
    display: DisplayMap,
    /// Where the row's match starts in `text`.
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
    /// the row's line `line`, whose match is at `column`, in characters, and of the file whose
    /// device and inode are `identity`. Each invalid byte is one U+FFFD and a `\r` before a line
    /// break is dropped, as a row shows them; see the module's notes for the bounds. Builds its
    /// display model, O(n) in what it keeps, so the host's thread makes it.
    pub fn new(
        bytes: &[u8],
        first_line: usize,
        (line, column): (usize, usize),
        identity: Option<(u64, u64)>,
    ) -> Self {
        let hit = line.checked_sub(first_line);
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let mut text = String::new();
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
                push_cut(&mut text, line, 0, EXCERPT_LINE.min(line.len()));
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

/// Pushes `line[start..end]` onto `text`, `start` and `end` moved off any character they cut,
/// with [`CUT`] where part of the line is left out.
fn push_cut(text: &mut String, line: &[u8], start: usize, end: usize) {
    let (start, end) = boundaries(line, start, end);
    if start > 0 {
        text.push(CUT);
    }
    text.push_str(&decoded(&line[start..end]));
    if end < line.len() {
        text.push(CUT);
    }
}

/// `start` and `end` within `line`, moved off a character they would cut in two: at most three
/// bytes of it are left out.
fn boundaries(line: &[u8], mut start: usize, mut end: usize) -> (usize, usize) {
    let continues = |at: usize| line.get(at).is_some_and(|byte| byte & 0xC0 == 0x80);
    for _ in 0..3 {
        if start > 0 && continues(start) {
            start += 1;
        }
        if end < line.len() && continues(end) {
            end -= 1;
        }
    }
    (start, end.max(start))
}

/// Pushes the part of `line` around its match at `column`, in characters as a row counts them,
/// onto `text`, and gives where the match starts in `text`.
fn push_around(text: &mut String, line: &[u8], column: usize) -> usize {
    let at = byte_of_column(line, column);
    let start = if at + BEFORE_MATCH <= EXCERPT_LINE {
        0
    } else {
        at - BEFORE_MATCH
    };
    let end = (start + EXCERPT_LINE).min(line.len());
    let (start, end) = boundaries(line, start, end);
    let before = text.len();
    push_cut(text, line, start, end);
    let mut cursor = before + if start > 0 { CUT.len_utf8() } else { 0 };
    cursor += decoded(&line[start..at.clamp(start, end)]).len();
    cursor
}

/// The byte of `line` at the character `column`, each invalid byte one character; the line's end
/// past its last.
fn byte_of_column(line: &[u8], column: usize) -> usize {
    let mut left = column;
    let mut at = 0;
    for chunk in line.utf8_chunks() {
        for (index, _) in chunk.valid().char_indices() {
            if left == 0 {
                return at + index;
            }
            left -= 1;
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

/// What the preview keeps for the prompt that searches a project.
#[derive(Default)]
pub(super) struct SearchPreview {
    /// The newest read's generation, shared with the host.
    live: Arc<AtomicU64>,
    /// The row asked for last, by its search's generation and its item.
    asked: Option<(u64, usize)>,
    /// The read to start, until the host takes it.
    request: Option<ProjectPreviewRequest>,
    /// What came for the row asked for, and the revision the frame draws it at.
    read: Option<(ProjectPreviewRead, u64)>,
    /// The revision the last excerpt was given, so that each is shaped as its own.
    revision: u64,
}

impl SearchPreview {
    /// Stops the read under way, and forgets what was read: the rows it was for have gone.
    pub(super) fn reset(&mut self) {
        self.live.fetch_add(1, Ordering::Relaxed);
        self.asked = None;
        self.request = None;
        self.read = None;
    }
}

impl Editor {
    /// The row of the innermost prompt selected, if it searches a project: its search's
    /// generation and its item.
    fn selected_search_row(&self) -> Option<(u64, usize)> {
        if !self.searches_project() {
            return None;
        }
        let item = self.prompts.last()?.candidates()?.selected_item()?;
        Some((self.project_search.generation, item))
    }

    /// The selected row's file from the root, and its place.
    fn selected_search_place(&self) -> Option<(PathBuf, &super::Place)> {
        let (_, item) = self.selected_search_row()?;
        let candidates = self.prompts.last()?.candidates()?;
        let text = candidates.get(candidates.selected()?)?.text();
        let place = self.project_search.places.get(item)?;
        Some((PathBuf::from(text.get(..place.path)?), place))
    }

    /// An open buffer of the file at `path`, as it is spelled, that shows what it holds: none
    /// that the agent channel is still opening (ADR 0085).
    fn open_buffer_at(&self, path: &std::path::Path) -> Option<BufferId> {
        self.buffers()
            .find(|(_, buffer)| !buffer.agent_pending() && buffer.path() == Some(path))
            .map(|(id, _)| id)
    }

    /// An open buffer of the file whose device and inode are `identity`.
    fn open_buffer_of(&self, identity: (u64, u64)) -> Option<BufferId> {
        self.buffers()
            .find(|(_, buffer)| {
                !buffer.agent_pending()
                    && buffer.baseline().and_then(|base| base.file?.identity) == Some(identity)
            })
            .map(|(id, _)| id)
    }

    /// The read the host should start for the selected row's preview, once per row selected:
    /// none for a row of a file that may hold secrets, one an open buffer shows, or while the
    /// float has no room for a preview.
    pub fn take_preview_request(&mut self) -> Option<ProjectPreviewRequest> {
        let key = self.selected_search_row()?;
        if self.project_search.preview.asked == Some(key) {
            return self.project_search.preview.request.take();
        }
        let rows = self
            .prompt_layout()
            .map(|layout| layout.regions.preview)
            .filter(|region| region.rows > 0 && region.columns > 0)?
            .rows;
        let (path, place) = self.selected_search_place()?;
        let (line, column, offset, secret) = (place.line, place.column, place.offset, place.secret);
        let whole = self.project_search.root.join(&path);
        let open = self.open_buffer_at(&whole).is_some();
        let preview = &mut self.project_search.preview;
        preview.reset();
        preview.asked = Some(key);
        if secret || open {
            return None;
        }
        Some(ProjectPreviewRequest {
            generation: preview.live.load(Ordering::Relaxed),
            root: self.project_search.root.clone(),
            path: path.to_str()?.into(),
            offset,
            line,
            column,
            rows,
            live: Arc::clone(&preview.live),
        })
    }

    /// Takes in what the host read for the preview `generation`: dropped unless it is the
    /// newest.
    pub fn preview_read(&mut self, generation: u64, read: ProjectPreviewRead) {
        let preview = &mut self.project_search.preview;
        if preview.asked.is_none() || preview.live.load(Ordering::Relaxed) != generation {
            return;
        }
        preview.revision += 1;
        preview.read = Some((read, preview.revision));
    }

    /// What the preview of the innermost prompt's selected row shows, if it searches a project.
    pub(in crate::editor) fn search_preview(&self) -> Option<Preview> {
        let (path, place) = self.selected_search_place()?;
        if place.secret {
            return Some(Preview::Withheld);
        }
        let (line, column) = (place.line, place.column);
        let whole = self.project_search.root.join(&path);
        if let Some(id) = self.open_buffer_at(&whole) {
            return self.buffer_preview(id, line, column);
        }
        let preview = &self.project_search.preview;
        if preview.asked != self.selected_search_row() {
            return None;
        }
        match &preview.read {
            Some((ProjectPreviewRead::Text(excerpt), revision)) => {
                match excerpt
                    .identity
                    .and_then(|identity| self.open_buffer_of(identity))
                {
                    Some(id) => self.buffer_preview(id, line, column),
                    None => Some(Preview::Excerpt {
                        revision: *revision,
                        cursor: excerpt.cursor,
                    }),
                }
            }
            Some((ProjectPreviewRead::Withheld, _)) => Some(Preview::Withheld),
            Some((ProjectPreviewRead::Unreadable, _)) | None => None,
        }
    }

    /// The preview of the open buffer `id` at `line`, from 1, and `column`, in characters, each
    /// as far as its text goes, with that line about halfway down; nothing of it if it may hold
    /// secrets.
    fn buffer_preview(&self, id: BufferId, line: usize, column: usize) -> Option<Preview> {
        let buffer = self.buffer(id)?;
        if buffer.verdict().is_withheld() {
            return Some(Preview::Withheld);
        }
        let cursor = place_offset(buffer.text(), line.saturating_sub(1), column);
        Some(Preview::Buffer {
            id,
            start: self.preview_start(&buffer.display(), cursor),
            cursor,
        })
    }

    /// Where a preview whose cursor is at `cursor` starts: the line about halfway up the
    /// preview's rows from the cursor's.
    fn preview_start(&self, display: &DisplaySnapshot, cursor: usize) -> usize {
        let rows = self
            .prompt_layout()
            .map_or(0, |layout| layout.regions.preview.rows);
        let row = display.offset_to_display_point(cursor).row;
        crate::motion::line_start(display, row.saturating_sub(rows / 2))
    }

    /// The display model of the excerpt the preview shows, if it shows one.
    pub fn preview_excerpt_display(&self) -> Option<DisplaySnapshot> {
        let Some(Preview::Excerpt { .. }) = self.search_preview() else {
            return None;
        };
        match &self.project_search.preview.read {
            Some((ProjectPreviewRead::Text(excerpt), _)) => Some(excerpt.display.snapshot()),
            _ => None,
        }
    }

    /// The excerpt the preview shows, its text and its display model, and where it starts.
    pub(in crate::editor) fn shown_excerpt(&self) -> Option<(&Rope, DisplaySnapshot, usize)> {
        let Some(Preview::Excerpt { .. }) = self.search_preview() else {
            return None;
        };
        match &self.project_search.preview.read {
            Some((ProjectPreviewRead::Text(excerpt), _)) => {
                let display = excerpt.display.snapshot();
                let start = self.preview_start(&display, excerpt.cursor);
                Some((&excerpt.text, display, start))
            }
            _ => None,
        }
    }

    /// The column the preview of a search scrolls to, so that the match at `cursor` shows in
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

    /// The cells the matches in view take in a search's preview, as `screen` shows it
    /// ([`Editor::preview_screen`]), into `cells`, emptied first: the selected row's match as the
    /// current one, the query's other matches on the lines shown as the rest.
    pub fn preview_cells(&self, screen: &Screen, cells: &mut MatchCells) {
        cells.current.clear();
        cells.others.clear();
        let Some(query) = self.project_search.query.as_deref() else {
            return;
        };
        let Some(region) = self.prompt_layout().map(|layout| layout.regions.preview) else {
            return;
        };
        let (text, display, cursor) = match self.search_preview() {
            Some(Preview::Buffer { id, cursor, .. }) => match self.buffer(id) {
                Some(buffer) => (buffer.text(), buffer.display(), cursor),
                None => return,
            },
            Some(Preview::Excerpt { cursor, .. }) => match self.shown_excerpt() {
                Some((text, display, _)) => (text, display, cursor),
                None => return,
            },
            _ => return,
        };
        let mut found: Vec<Range<usize>> = Vec::new();
        let mut line = String::new();
        let mut ranges = Vec::new();
        let mut last_line = None;
        for row in screen.rows().iter().take(region.rows) {
            // Each line once, however many rows it takes.
            if last_line == Some(row.line) {
                continue;
            }
            last_line = Some(row.line);
            let offset = |column| {
                display.display_point_to_offset(DisplayPoint::new(row.line, column), Bias::Left)
            };
            let drawn = screen.drawn(row);
            let (line_start, line_end) = (text.line_start(row.line), text.line_end(row.line));
            let start = offset(drawn.start)
                .saturating_sub(AROUND_DRAWN)
                .max(line_start);
            let end = offset(drawn.end).saturating_add(AROUND_DRAWN).min(line_end);
            line.clear();
            for chunk in text.chunks_in_range(start..end) {
                line.push_str(chunk);
            }
            query.highlights(&line, &mut ranges);
            found.extend(
                ranges
                    .iter()
                    .map(|range| start + range.start..start + range.end),
            );
        }
        let current = found
            .iter()
            .position(|range| range.start <= cursor && cursor < range.end);
        if let Some(index) = current {
            let range = found.remove(index);
            frame::match_cells(region, screen, &display, &[range], &mut cells.current);
        }
        frame::match_cells(region, screen, &display, &found, &mut cells.others);
    }
}

#[cfg(test)]
mod tests;
