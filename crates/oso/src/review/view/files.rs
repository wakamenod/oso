//! A review's files pane's text (ADR 0122, step 6b), laid out on the view thread beside the
//! review's while a window shows the pane: a title, then a row a file the review shows, in its
//! order, each a section of its own holding the file's header alone.
//!
//! Must not depend on: `app`, the editor or the review's own thread.
//!
//! A row is the file's status letter, `+n −m` padded to the widest, its comments as the
//! person's·the agent's, `!` when one is outdated, `•` while an agent's is unread, `✓` when
//! every hunk is decided or viewed and `◐` when some are, then its path, cut from the left to
//! the width so that its name stays, drawn as any path of the review is.

use std::fmt::Write as _;
use std::sync::Arc;

use oso_core::display_map::cell::classify;
use oso_core::review::{Author, RowKind, Shape};
use oso_core::rope::Rope;
use oso_core::syntax::Highlight;

use super::State;
use super::assemble::assemble;
use super::cards::Cards;
use super::panes::PaneText;
use super::section::{Rendered, Writer, shown_path};
use crate::review::{Content, Status};

/// What the files pane's text keeps between builds.
#[derive(Default)]
pub(super) struct FilesPane {
    /// The columns its text has, while it is built.
    width: Option<usize>,
    dirty: bool,
    text: Rope,
}

/// The fewest columns a path keeps, however narrow the pane.
const LEAST_PATH: usize = 8;

impl State {
    /// Builds the files pane's text `width` columns wide from now on, or no more with `None`.
    pub(super) fn show_files(&mut self, width: Option<usize>) {
        if width == self.files_pane.width {
            return;
        }
        if self.files_pane.width.is_none() {
            // A pane made anew takes its text whole.
            self.files_pane.text = Rope::new();
        }
        self.files_pane.width = width;
        self.files_pane.dirty = true;
    }

    /// The files pane's whole text, while it is built and something it shows changed.
    pub(super) fn files_text(&mut self) -> Option<Box<PaneText>> {
        let width = self.files_pane.width?;
        if !(self.dirty || self.files_pane.dirty) {
            return None;
        }
        self.files_pane.dirty = false;
        let title = match self.filter_note() {
            Some(note) => note,
            None => format!(
                "{} files \u{b7} / to filter",
                self.order.len() + self.gone.len()
            ),
        };
        let mut sections: Vec<Rendered> = Vec::with_capacity(self.order.len() + 1);
        let mut writer = Writer::new(b"");
        writer.line(
            RowKind::Note,
            &[(title.as_bytes(), Some(Highlight::DiffFileHeader))],
        );
        sections.push(writer.done());
        let mut rows: Vec<FileRow<'_>> = Vec::with_capacity(self.order.len());
        for path in self.order.iter().filter(|path| self.shows(path)) {
            let Some(file) = self.files.get(path) else {
                continue;
            };
            let shape = file
                .reading
                .as_ref()
                .and_then(|reading| match &reading.content {
                    Content::Text { shape, .. } => Some(shape),
                    _ => None,
                });
            rows.push(FileRow {
                path,
                status: Some(file.entry.status),
                shape,
                cards: &file.cards,
            });
        }
        for (path, gone) in self.gone.iter().filter(|(path, _)| self.shows(path)) {
            rows.push(FileRow {
                path,
                status: None,
                shape: None,
                cards: &gone.cards,
            });
        }
        let counts: Vec<String> = rows.iter().map(FileRow::counts).collect();
        let wide = counts
            .iter()
            .map(|count| count.chars().count())
            .max()
            .unwrap_or(0);
        for (row, count) in rows.iter().zip(&counts) {
            sections.push(row.render(count, wide, width));
        }
        let laid: Vec<&Rendered> = sections.iter().collect();
        let built = assemble(&laid, &[], &self.files_pane.text);
        self.files_pane.text = built.rope;
        Some(Box::new((built.text, built.rows, built.highlights)))
    }
}

#[cfg(test)]
impl State {
    /// The files pane's text as last built.
    pub(super) fn files_pane_text(&self) -> String {
        self.files_pane.text.to_string()
    }
}

/// What a file's row shows.
struct FileRow<'a> {
    path: &'a [u8],
    /// `None` for a file no longer changed.
    status: Option<Status>,
    shape: Option<&'a Arc<Shape>>,
    cards: &'a Cards,
}

impl FileRow<'_> {
    /// `+n −m`, or nothing for a file with no reading.
    fn counts(&self) -> String {
        let Some(shape) = self.shape else {
            return String::new();
        };
        let (added, removed) = (shape.hunks().iter()).fold((0, 0), |(added, removed), hunk| {
            (added + hunk.added.len(), removed + hunk.removed.len())
        });
        format!("+{added} \u{2212}{removed}")
    }

    /// The row, `width` columns wide, its counts padded to `wide` characters.
    fn render(&self, counts: &str, wide: usize, width: usize) -> Rendered {
        let mut writer = Writer::new(self.path);
        let letter = self.status.map_or(" ", status_letter);
        let letter_face = match self.status {
            Some(Status::Added) => Highlight::DiffAdded,
            Some(Status::Deleted) => Highlight::DiffRemoved,
            Some(_) => Highlight::DiffHunkHeader,
            None => Highlight::Outdated,
        };
        let pad = " ".repeat(wide.saturating_sub(counts.chars().count()));
        let (added, removed) = counts.split_once(' ').unwrap_or((counts, ""));
        let comments = &self.cards.comments;
        let person = (comments.iter())
            .filter(|c| c.author() == Author::Person)
            .count();
        let agent = comments.len() - person;
        let mut said = String::new();
        if !comments.is_empty() {
            let _ = write!(said, " {person}\u{b7}{agent}");
        }
        let outdated = if comments.iter().any(|c| c.is_outdated()) {
            "!"
        } else {
            ""
        };
        let unread = (comments.iter()).any(|c| c.author() == Author::Agent && !c.is_read());
        let unread = if unread { "\u{2022}" } else { "" };
        let mark = self.mark();
        let used = 2
            + wide
            + said.chars().count()
            + outdated.len()
            + unread.chars().count()
            + mark.chars().count()
            + 2;
        let mut path = Vec::new();
        shown_path(self.path, &mut path);
        let path = cut_left(&path, width.saturating_sub(used).max(LEAST_PATH));
        let path_face = self.status.is_none().then_some(Highlight::Outdated);
        let space: &[u8] = b" ";
        writer.line(
            RowKind::File,
            &[
                (letter.as_bytes(), Some(letter_face)),
                (space, None),
                (pad.as_bytes(), None),
                (added.as_bytes(), Some(Highlight::DiffAdded)),
                (if removed.is_empty() { b"" } else { space }, None),
                (removed.as_bytes(), Some(Highlight::DiffRemoved)),
                (said.as_bytes(), Some(Highlight::CommentPerson)),
                (outdated.as_bytes(), Some(Highlight::Outdated)),
                (unread.as_bytes(), Some(Highlight::CommentAgent)),
                (mark.as_bytes(), Some(Highlight::Decided)),
                (b"  ", None),
                (&path, path_face),
            ],
        );
        writer.done()
    }

    /// ` ✓` when every hunk of the reading shown is decided or viewed, ` ◐` when some are, as
    /// the model's marks say of that reading.
    fn mark(&self) -> &'static str {
        let Some(shape) = self.shape else {
            return "";
        };
        let same = (self.cards.reading.as_ref()).is_some_and(|reading| Arc::ptr_eq(reading, shape));
        let hunks = &self.cards.marks.hunks;
        let marked = hunks
            .iter()
            .filter(|m| m.decision.is_some() || m.viewed)
            .count();
        match (same, marked) {
            (false, _) | (_, 0) => "",
            _ if marked == hunks.len() => " \u{2713}",
            _ => " \u{25d0}",
        }
    }
}

/// The letter a status is shown by.
fn status_letter(status: Status) -> &'static str {
    match status {
        Status::Added => "A",
        Status::Copied => "C",
        Status::Deleted => "D",
        Status::Modified => "M",
        Status::Renamed => "R",
        Status::TypeChanged => "T",
        Status::Unmerged => "U",
        Status::Unknown => "?",
    }
}

/// `path` cut from the left to at most `cells` columns, a `…` standing for what went: its end
/// stays, and an invalid byte counts as the 4 cells `\xNN` takes.
pub(super) fn cut_left(path: &[u8], cells: usize) -> Vec<u8> {
    let mut pieces: Vec<(&[u8], usize)> = Vec::new();
    for chunk in path.utf8_chunks() {
        for (at, c) in chunk.valid().char_indices() {
            let bytes = &chunk.valid().as_bytes()[at..at + c.len_utf8()];
            pieces.push((bytes, classify(c.encode_utf8(&mut [0; 4])).1));
        }
        for byte in chunk.invalid() {
            pieces.push((std::slice::from_ref(byte), 4));
        }
    }
    let total: usize = pieces.iter().map(|(_, cells)| cells).sum();
    if total <= cells {
        return path.to_vec();
    }
    let mut kept = Vec::new();
    let mut used = 1;
    for (bytes, width) in pieces.iter().rev() {
        if used + width > cells {
            break;
        }
        used += width;
        kept.push(*bytes);
    }
    let mut out = "\u{2026}".as_bytes().to_vec();
    for bytes in kept.iter().rev() {
        out.extend_from_slice(bytes);
    }
    out
}
