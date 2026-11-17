//! What a frame shows of an open list of a server's work (ADR 0138, step 15), for a reader that
//! cannot see the frame, such as the agent channel's `state`: what it lists, the rows shown,
//! split into each row's path, line and text, the selected one, and the preview's lines.
//!
//! Must not depend on: the channel, the host or I/O; this reads only what the lists keep and
//! what the preview shows. What a reader may be given of it is the reader's to judge.
//!
//! - Only the rows a frame shows are given, at most the float's rows, and each line of the
//!   preview at most [`LISTED_LINE_BYTES`], so a view costs O(the float's rows) however long the list.
//! - A row of an open buffer names it as it is now, so that a reader can judge it, and every
//!   row its whole path.

use std::path::{Path, PathBuf};

use crate::buffer::BufferId;
use crate::editor::{Editor, Preview};
use crate::rope::Rope;

/// The most bytes of a line of the preview a view gives, cut at a character.
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
/// the open buffer whose place it is, if one is now. `Debug` says nothing of it.
pub struct ListedRow<'a> {
    pub path: &'a str,
    pub file: PathBuf,
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
    /// Lines from the line `first_line`, from 1, of the open buffer `buffer`, or of the file
    /// read for it if `None`, the selected row's place on the line `at` of them, each at most
    /// [`LISTED_LINE_BYTES`].
    Lines {
        buffer: Option<BufferId>,
        first_line: usize,
        at: Option<usize>,
        lines: Vec<String>,
    },
}

impl std::fmt::Debug for ListedPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Nothing => f.write_str("Nothing"),
            Self::Withheld => f.write_str("Withheld"),
            Self::Lines { .. } => f.debug_struct("Lines").finish_non_exhaustive(),
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

/// What a list keeps of a row, for its view: the bytes of the path that starts its text, its
/// whole path, its line, and the open buffer it is in now, as its list finds it.
pub(super) struct RowOf {
    pub(super) path: usize,
    pub(super) file: PathBuf,
    pub(super) line: usize,
    pub(super) open: Option<BufferId>,
}

impl Editor {
    /// The innermost prompt as a frame shows it, if it lists a server's places or a project's
    /// diagnostics; `None` for any other prompt.
    pub fn listed_view(&self) -> Option<ListedView<'_>> {
        let (of, pending, cut) = if self.lists_xrefs() {
            let (kind, pending, cut) = self.xrefs_state();
            let of = match kind? {
                super::XrefKind::Definitions => ListedOf::Definitions,
                super::XrefKind::References => ListedOf::References,
            };
            (of, pending, cut)
        } else if self.lists_diagnostics() {
            let (pending, cut) = self.diagnostics_state();
            (ListedOf::Diagnostics, pending, cut)
        } else {
            return None;
        };
        let candidates = self.prompts.last()?.candidates()?;
        let mut rows = Vec::new();
        let mut selected = None;
        for (place, candidate) in candidates.shown(self.candidate_rows()) {
            let Some(item) = candidates.item_at(place) else {
                continue;
            };
            let text = candidate.text();
            let row = match of {
                ListedOf::Diagnostics => self.diagnostic_row_of(item, text),
                ListedOf::Definitions | ListedOf::References => self.xref_row_of(item, text),
            };
            let Some(row) = row else {
                continue;
            };
            let (Some(path), Some(rest)) = (text.get(..row.path), text.get(row.path..)) else {
                continue;
            };
            if candidates.selected() == Some(place) {
                selected = Some(rows.len());
            }
            let buffer = row.open.or_else(|| self.buffer_of_file(&row.file));
            rows.push(ListedRow {
                path,
                file: row.file,
                line: row.line,
                text: after_line(rest, row.line),
                buffer,
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

    /// The open buffer that visits `file`, if one does.
    fn buffer_of_file(&self, file: &Path) -> Option<BufferId> {
        self.buffers()
            .find(|(_, buffer)| buffer.path() == Some(file))
            .map(|(id, _)| id)
    }

    /// What the preview of the innermost prompt shows, as lines.
    fn listed_preview(&self) -> ListedPreview {
        let rows = self
            .prompt_layout()
            .map_or(0, |layout| layout.regions.preview.rows);
        match self.preview() {
            Some(Preview::Buffer { id, start, cursor }) => {
                let Some(buffer) = self.buffer(id) else {
                    return ListedPreview::Nothing;
                };
                lines_of(buffer.text(), (start, cursor), rows, Some(id), 0)
            }
            Some(Preview::Excerpt { cursor, .. }) => {
                let (Some((text, _, start)), Some(excerpt)) =
                    (self.shown_excerpt(), self.read_excerpt())
                else {
                    return ListedPreview::Nothing;
                };
                // The excerpt's first line is its file's `first_line`.
                lines_of(text, (start, cursor), rows, None, excerpt.first_line() - 1)
            }
            Some(Preview::Withheld) => ListedPreview::Withheld,
            Some(Preview::NotOpen | Preview::Text { .. }) | None => ListedPreview::Nothing,
        }
    }
}

/// The `rows` lines of `text` from the one that holds `start`, the one that holds `cursor`
/// marked, numbered from 1 past `before` lines that `text` leaves out.
fn lines_of(
    text: &Rope,
    (start, cursor): (usize, usize),
    rows: usize,
    buffer: Option<BufferId>,
    before: usize,
) -> ListedPreview {
    let first = text.offset_to_point(start.min(text.len())).row;
    let hit = text.offset_to_point(cursor.min(text.len())).row;
    let last = (first + rows).min(text.max_point().row + 1);
    let lines = (first..last)
        .map(|row| {
            // No more than a cut keeps is copied of a long line.
            let start = text.line_start(row);
            let end = text.line_end(row).min(start + LISTED_LINE_BYTES);
            let end = text.clip_offset(end, crate::sum_tree::Bias::Left);
            text.slice(start..end).to_string()
        })
        .collect();
    ListedPreview::Lines {
        buffer,
        first_line: before + first + 1,
        at: hit.checked_sub(first).filter(|at| *at < last - first),
        lines,
    }
}

/// What follows `:line:` and a blank in what follows a row's path.
fn after_line(rest: &str, line: usize) -> &str {
    let number = format!(":{line}:");
    let rest = rest.strip_prefix(number.as_str()).unwrap_or(rest);
    rest.strip_prefix(' ').unwrap_or(rest)
}

#[cfg(test)]
mod tests;
