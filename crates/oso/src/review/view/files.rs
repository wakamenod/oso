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

use oso_core::display_map::text_clusters;
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
    /// The title and rows it was last built of, to send nothing when they are the same.
    title: String,
    rows: Vec<Arc<Rendered>>,
}

/// A file's row as last laid out, with what it shows: laid out again only when that changed.
pub(super) struct PaneRow {
    key: RowKey,
    rendered: Arc<Rendered>,
}

/// What a file's row shows, its path aside, which is the file's own.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RowKey {
    status: Option<Status>,
    counts: Option<(usize, usize)>,
    person: usize,
    agent: usize,
    outdated: bool,
    unread: bool,
    mark: &'static str,
    width: usize,
    wide: usize,
}

/// The fewest columns a path keeps, however narrow the pane.
const LEAST_PATH: usize = 8;

impl State {
    /// Builds the files pane's text `width` columns wide from now on, or no more with `None`.
    pub(super) fn show_files(&mut self, width: Option<usize>) {
        if width == self.files_pane.width {
            return;
        }
        if self.files_pane.width.is_none() || width.is_none() {
            // A pane made anew takes its text whole.
            self.files_pane.text = Rope::new();
            self.files_pane.rows.clear();
            self.files_pane.title.clear();
        }
        self.files_pane.width = width;
        self.files_pane.dirty = true;
    }

    /// The files pane's whole text, while it is built and something it shows changed. A row
    /// is laid out again only when what it shows changed, and nothing is sent when no row
    /// nor the title did.
    pub(super) fn files_text(&mut self) -> Option<Box<PaneText>> {
        let width = self.files_pane.width?;
        if !(self.dirty || self.files_pane.dirty) {
            return None;
        }
        let title = match self.filter_note() {
            Some(note) => note,
            None => format!(
                "{} files \u{b7} / to filter",
                self.order.len() + self.gone.len()
            ),
        };
        let filtered = self.filtering.on();
        // The widest `+n −m`, which every row pads to.
        let mut wide = 0;
        for path in &self.order {
            if let Some(file) = self.files.get(path)
                && !(filtered && file.judged.hidden())
                && let Some(shape) = shape_of(file)
            {
                wide = wide.max(counts_width(counts(shape)));
            }
        }
        let mut rows: Vec<Arc<Rendered>> = Vec::with_capacity(self.order.len() + 1);
        for path in &self.order {
            let Some(file) = self.files.get_mut(path) else {
                continue;
            };
            if filtered && file.judged.hidden() {
                continue;
            }
            let shape = shape_of(file).cloned();
            let row = FileRow {
                path,
                status: Some(file.entry.status),
                shape: shape.as_ref(),
                cards: &file.cards,
            };
            rows.push(row.cached(&mut file.pane_row, wide, width));
        }
        for (path, gone) in &mut self.gone {
            if filtered && gone.judged.hidden() {
                continue;
            }
            let row = FileRow {
                path,
                status: None,
                shape: None,
                cards: &gone.cards,
            };
            rows.push(row.cached(&mut gone.pane_row, wide, width));
        }
        let pane = &mut self.files_pane;
        let same = !pane.dirty
            && pane.title == title
            && pane.rows.len() == rows.len()
            && (pane.rows.iter().zip(&rows)).all(|(then, now)| Arc::ptr_eq(then, now));
        pane.dirty = false;
        if same {
            return None;
        }
        let mut writer = Writer::new(b"");
        writer.line(
            RowKind::Note,
            &[(title.as_bytes(), Some(Highlight::DiffFileHeader))],
        );
        let head = writer.done();
        let laid: Vec<&Rendered> = std::iter::once(&head)
            .chain(rows.iter().map(|row| &**row))
            .collect();
        let built = assemble(&laid, &[], &pane.text);
        pane.text = built.rope;
        pane.title = title;
        pane.rows = rows;
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

/// The shape of the reading a file shows, if it is text.
fn shape_of(file: &super::file::File) -> Option<&Arc<Shape>> {
    match &file.reading.as_ref()?.content {
        Content::Text { shape, .. } => Some(shape),
        _ => None,
    }
}

/// The lines a reading puts in and takes out.
fn counts(shape: &Shape) -> (usize, usize) {
    (shape.hunks().iter()).fold((0, 0), |(added, removed), hunk| {
        (added + hunk.added.len(), removed + hunk.removed.len())
    })
}

/// The characters `+n −m` takes.
fn counts_width((added, removed): (usize, usize)) -> usize {
    let digits = |n: usize| n.checked_ilog10().map_or(1, |log| log as usize + 1);
    digits(added) + digits(removed) + 3
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
    /// The row, from `kept` when what it shows is the same, else laid out and kept there.
    fn cached(&self, kept: &mut Option<PaneRow>, wide: usize, width: usize) -> Arc<Rendered> {
        let key = self.key(wide, width);
        if let Some(row) = kept.as_ref().filter(|row| row.key == key) {
            return Arc::clone(&row.rendered);
        }
        let rendered = Arc::new(self.render(&key));
        *kept = Some(PaneRow {
            key,
            rendered: Arc::clone(&rendered),
        });
        rendered
    }

    /// What the row shows: O(the file's hunks and comments), allocating nothing.
    fn key(&self, wide: usize, width: usize) -> RowKey {
        let comments = &self.cards.comments;
        let person = (comments.iter())
            .filter(|c| c.author() == Author::Person)
            .count();
        RowKey {
            status: self.status,
            counts: self.shape.map(|shape| counts(shape)),
            person,
            agent: comments.len() - person,
            outdated: comments.iter().any(|c| c.is_outdated()),
            unread: (comments.iter()).any(|c| c.author() == Author::Agent && !c.is_read()),
            mark: self.mark(),
            width,
            wide,
        }
    }

    /// The row as `key` says, `key.width` columns wide, its counts padded to `key.wide`.
    fn render(&self, key: &RowKey) -> Rendered {
        let mut writer = Writer::new(self.path);
        let letter = self.status.map_or(" ", status_letter);
        let letter_face = match self.status {
            Some(Status::Added) => Highlight::DiffAdded,
            Some(Status::Deleted) => Highlight::DiffRemoved,
            Some(_) => Highlight::DiffHunkHeader,
            None => Highlight::Outdated,
        };
        let (added, removed) = match key.counts {
            Some((added, removed)) => (format!("+{added}"), format!("\u{2212}{removed}")),
            None => (String::new(), String::new()),
        };
        let shown = key.counts.map_or(0, counts_width);
        let pad = " ".repeat(key.wide.saturating_sub(shown));
        let mut said = String::new();
        if key.person + key.agent > 0 {
            let _ = write!(said, " {}\u{b7}{}", key.person, key.agent);
        }
        let outdated = if key.outdated { "!" } else { "" };
        let unread = if key.unread { "\u{2022}" } else { "" };
        let used = 2
            + key.wide
            + said.chars().count()
            + outdated.len()
            + unread.chars().count()
            + key.mark.chars().count()
            + 2;
        let mut path = Vec::new();
        shown_path(self.path, &mut path);
        let path = cut_left(&path, key.width.saturating_sub(used).max(LEAST_PATH));
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
                (key.mark.as_bytes(), Some(Highlight::Decided)),
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
/// stays, whole grapheme clusters as the display counts them, an invalid byte as the 4 cells
/// `\xNN` takes. Costs O(`cells`), whatever the path's length: only its last bytes are read.
pub(super) fn cut_left(path: &[u8], cells: usize) -> Vec<u8> {
    // No cluster takes less than a cell for 4 of its bytes but marks on a base, so these
    // bytes hold what fits, and more.
    let mut start = path
        .len()
        .saturating_sub(cells.saturating_mul(4).saturating_add(16));
    while start > 0 && start < path.len() && path[start] & 0xc0 == 0x80 {
        start += 1;
    }
    let tail = &path[start..];
    let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
    let mut at = 0;
    for chunk in tail.utf8_chunks() {
        for cluster in text_clusters(chunk.valid(), 0) {
            pieces.push((
                at + cluster.offset,
                cluster.text.len(),
                cluster.cells.max(1),
            ));
        }
        at += chunk.valid().len();
        for _ in chunk.invalid() {
            pieces.push((at, 1, 4));
            at += 1;
        }
    }
    let total: usize = pieces.iter().map(|(_, _, cells)| cells).sum();
    if start == 0 && total <= cells {
        return path.to_vec();
    }
    let mut used = 1;
    let mut from = tail.len();
    for (offset, _, width) in pieces.iter().rev() {
        if used + width > cells {
            break;
        }
        used += width;
        from = *offset;
    }
    let mut out = "\u{2026}".as_bytes().to_vec();
    out.extend_from_slice(&tail[from..]);
    out
}
