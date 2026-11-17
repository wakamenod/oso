//! What a frame shows of an open list of a server's work (ADR 0138, step 15), for a reader that
//! cannot see the frame, such as the agent channel's `state`: what it lists, the rows shown,
//! split into each row's path, line and text, the selected one, and the preview's rows.
//!
//! Must not depend on: the channel, the host or I/O; this reads only what the lists keep and
//! what the preview shows. What a reader may be given of it is the reader's to judge.
//!
//! - Only the rows a frame shows are given, at most the float's rows, and of each of the
//!   preview's rows the columns it draws, at most [`LISTED_LINE_BYTES`], so a view costs
//!   O(the float's rows) however long the list or its lines.
//! - The preview's rows are those [`Editor::preview_screen`] gives the frame: a line the
//!   preview wraps is several rows, and one it scrolls left to its place shows from there.
//! - A row of an open buffer names it, so that a reader can judge it, and every row its whole
//!   path. No row's place is mapped through the edits since; only the preview does that, for
//!   the row selected.

use std::path::{Path, PathBuf};

use crate::buffer::BufferId;
use crate::display_map::DisplayPoint;
use crate::editor::file_preview::Lister;
use crate::editor::{Editor, Preview};
use crate::sum_tree::Bias;
use crate::window::Screen;

use super::listed::{ListedAt, RowParts};

/// The most bytes of a row of the preview a view gives, cut at a character.
pub const LISTED_LINE_BYTES: usize = 256;

/// What an open list lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListedOf {
    /// The definitions `M-.` found.
    Definitions,
    /// The references `M-?` found.
    References,
    /// A project's diagnostics, `M-g d`.
    Diagnostics,
}

/// A row shown: its path as it shows it and whole, its line from 1, the rest of its text, and
/// the open buffer it is in, if one holds it. `Debug` says nothing of it.
pub struct ListedRow<'a> {
    pub shown: &'a str,
    pub path: PathBuf,
    pub line: usize,
    pub text: &'a str,
    pub buffer: Option<BufferId>,
}

impl std::fmt::Debug for ListedRow<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ListedRow").finish_non_exhaustive()
    }
}

/// What the preview beside the rows shows.
pub enum ListedPreview {
    /// Nothing yet, or nothing it can show.
    Nothing,
    /// What may hold secrets, of which nothing shows.
    Withheld,
    /// The rows it draws of the open buffer `buffer`, or of the file read for it if `None`:
    /// each its line, from 1 as in its file, and the columns it draws, at most
    /// [`LISTED_LINE_BYTES`]; the selected row's place is on the row `at`.
    Rows {
        buffer: Option<BufferId>,
        at: Option<usize>,
        rows: Vec<(usize, String)>,
    },
}

impl std::fmt::Debug for ListedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nothing => f.write_str("Nothing"),
            Self::Withheld => f.write_str("Withheld"),
            Self::Rows { .. } => f.debug_struct("Rows").finish_non_exhaustive(),
        }
    }
}

/// An open list as a frame shows it: what it lists, how many rows match of how many, whether
/// the server's answer or the host's rows were cut, whether its rows have yet to come, the rows
/// shown and which of them is selected, and the preview.
#[derive(Debug)]
pub struct ListedView<'a> {
    pub of: ListedOf,
    pub count: usize,
    pub total: usize,
    pub cut: bool,
    pub pending: bool,
    pub selected: Option<usize>,
    pub rows: Vec<ListedRow<'a>>,
    pub preview: ListedPreview,
}

impl Editor {
    /// The innermost prompt as a frame shows it, if it lists a server's places or a project's
    /// diagnostics; `None` for any other prompt.
    pub fn listed_view(&self) -> Option<ListedView<'_>> {
        let (of, pending, cut) = match self.previewing()? {
            Lister::Xrefs => {
                let of = match self.xrefs_kind()? {
                    super::XrefKind::Definitions => ListedOf::Definitions,
                    super::XrefKind::References => ListedOf::References,
                };
                (of, self.xrefs_pending(), self.xrefs_cut())
            }
            Lister::Diagnostics => (
                ListedOf::Diagnostics,
                self.diagnostics_pending(),
                self.diagnostics_cut(),
            ),
            Lister::Search => return None,
        };
        let candidates = self.prompts.last()?.candidates()?;
        let mut rows = Vec::new();
        let mut selected = None;
        for (place, candidate) in candidates.shown(self.candidate_rows()) {
            let Some(item) = candidates.item_at(place) else {
                continue;
            };
            let text = candidate.text();
            let parts = match of {
                ListedOf::Diagnostics => self.diagnostic_parts(item, text),
                ListedOf::Definitions | ListedOf::References => self.xref_parts(item),
            };
            let Some(parts) = parts else {
                continue;
            };
            let (Some(shown), Some(rest)) = (text.get(..parts.shown), text.get(parts.text..))
            else {
                continue;
            };
            if candidates.selected() == Some(place) {
                selected = Some(rows.len());
            }
            rows.push(ListedRow {
                shown,
                buffer: self.buffer_holding(&parts),
                path: parts.path,
                line: parts.line,
                text: rest,
            });
        }
        Some(ListedView {
            of,
            count: candidates.len(),
            total: candidates.total(),
            cut,
            pending,
            selected,
            rows,
            preview: self.listed_preview(),
        })
    }

    /// The open buffer that holds a row's place: the one it was listed in, if it is still
    /// there, whether or not its place is still known, else the one that visits its file, as
    /// the preview finds one (none the channel is still opening). O(the buffers) at most, each
    /// a length check or a compare of the path's bytes.
    fn buffer_holding(&self, parts: &RowParts) -> Option<BufferId> {
        if let ListedAt::Open { buffer, .. } = parts.at
            && self.buffer(buffer).is_some()
        {
            return Some(buffer);
        }
        let path = parts.path.as_os_str();
        self.find_open_buffer(|_, buffer| buffer.path().map(Path::as_os_str) == Some(path))
    }

    /// The rows the preview of the innermost prompt draws, as [`Editor::preview_screen`] gives
    /// the frame them.
    fn listed_preview(&self) -> ListedPreview {
        let (display, text, cursor, buffer, before) = match self.preview() {
            Some(Preview::Buffer { id, cursor, .. }) => {
                let Some(held) = self.buffer(id) else {
                    return ListedPreview::Nothing;
                };
                (held.display(), held.text(), cursor, Some(id), 0)
            }
            Some(Preview::Excerpt { cursor, .. }) => {
                let (Some((text, display, _)), Some(excerpt)) =
                    (self.shown_excerpt(), self.read_excerpt())
                else {
                    return ListedPreview::Nothing;
                };
                // The excerpt's first line is its file's `first_line`.
                (display, text, cursor, None, excerpt.first_line() - 1)
            }
            Some(Preview::Withheld) => return ListedPreview::Withheld,
            Some(Preview::NotOpen | Preview::Text { .. }) | None => return ListedPreview::Nothing,
        };
        let mut screen = Screen::default();
        self.preview_screen(&mut screen);
        let rows = (screen.rows().iter())
            .map(|row| {
                let drawn = screen.drawn(row);
                let at = |column, bias| {
                    display.display_point_to_offset(DisplayPoint::new(row.line, column), bias)
                };
                let start = at(drawn.start, Bias::Left);
                // No more than a row keeps is copied of a long one.
                let end = at(drawn.end, Bias::Right).clamp(start, start + LISTED_LINE_BYTES);
                let end = text.clip_offset(end, Bias::Left).max(start);
                let shown = text.chunks_in_range(start..end).collect();
                (before + row.line + 1, shown)
            })
            .collect();
        let point = display.offset_to_display_point(cursor.min(text.len()));
        ListedPreview::Rows {
            buffer,
            at: screen.row_of(point),
            rows,
        }
    }
}

#[cfg(test)]
mod tests;
