//! Writing a graph's text (ADR 0127, step 4): each commit's row of lane cells, short id, ref
//! labels, subject, author and date, their highlights, and the table of which commit each line
//! is.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, git or the file system; [`Text::push`] and
//! [`Text::laid`] are pure functions of what was read.
//!
//! - The rows are written once, as they are read: each page adds its own to what is kept, so
//!   that a page costs its rows and the copy of the text, not the layout of all those before.
//! - Each lane's cells take the face of its index, `GitLane0` to `GitLane7` in turn (Q2).
//! - A label is drawn in its kind's face: `HEAD` as a hash is, a local branch's and a remote's
//!   as the status draws them, a tag's in its own; past [`MAX_LABELS`] a row says how many more.
//! - Names, subjects and authors are escaped on one line, so that no commit can draw a row of
//!   its own or reorder what is around it.

use std::sync::Arc;

use oso_core::load::{Builder, LoadSnapshot};
use oso_core::syntax::{Highlight, Highlights};

use super::lanes::Cell;
use super::{Commit, Kind, MAX_COMMITS};
use crate::dired::rows::{decoded_len, push_name};

/// The faces of the lanes, in turn.
const LANES: [Highlight; 8] = [
    Highlight::GitLane0,
    Highlight::GitLane1,
    Highlight::GitLane2,
    Highlight::GitLane3,
    Highlight::GitLane4,
    Highlight::GitLane5,
    Highlight::GitLane6,
    Highlight::GitLane7,
];

/// The most labels a row shows.
pub(crate) const MAX_LABELS: usize = 32;

/// The note a graph cut at [`MAX_COMMITS`] ends with (Q4).
pub(crate) const MORE_NOTE: &str = "more: a narrower scope (l o) shows them";

/// What a graph's text shows, kept with it so that a line can be told back into its commit.
#[derive(Debug, Default)]
pub(crate) struct Built {
    /// Each line's commit's full id; `None` for a line of no commit.
    pub(crate) lines: Vec<Option<String>>,
    /// Whether a page can still be read: git has more, and the graph is short of its most.
    pub(crate) more: bool,
}

impl Built {
    /// The full id of the commit on `line`, if it is one's.
    pub(crate) fn commit_at(&self, line: usize) -> Option<&str> {
        self.lines.get(line)?.as_deref()
    }
}

/// A graph's text, highlights and table.
pub(crate) struct Laid {
    pub(crate) text: LoadSnapshot,
    pub(crate) highlights: Highlights,
    pub(crate) built: Arc<Built>,
}

/// The rows written so far: their bytes, their runs of faces, and each one's commit.
#[derive(Default)]
pub(crate) struct Text {
    bytes: Vec<u8>,
    len: usize,
    runs: Vec<(usize, Option<Highlight>)>,
    ids: Vec<String>,
}

impl Text {
    /// How many rows are written.
    pub(crate) fn rows(&self) -> usize {
        self.ids.len()
    }

    /// Writes the row of `commit`, with its lane `cells` and its `date`.
    pub(crate) fn push(&mut self, cells: &[Cell], commit: &Commit, date: &str) {
        let mut bytes = [0; 4];
        for cell in cells {
            let face = (cell.ch != ' ').then(|| LANES[cell.lane % LANES.len()]);
            self.part(cell.ch.encode_utf8(&mut bytes).as_bytes(), face);
        }
        self.part(commit.short.as_bytes(), Some(Highlight::GitHash));
        for label in commit.labels.iter().take(MAX_LABELS) {
            let face = match label.kind {
                Kind::Head => Highlight::GitHash,
                Kind::Local => Highlight::GitBranchLocal,
                Kind::Remote => Highlight::GitBranchRemote,
                Kind::Tag => Highlight::GitTag,
            };
            let mut name = b" ".to_vec();
            push_name(&mut name, label.name.as_bytes());
            self.part(&name, Some(face));
        }
        if commit.labels.len() > MAX_LABELS {
            let more = format!(" +{}", commit.labels.len() - MAX_LABELS);
            self.part(more.as_bytes(), Some(Highlight::GitDetail));
        }
        let mut subject = b" ".to_vec();
        push_name(&mut subject, &commit.subject);
        self.part(&subject, None);
        let mut author = b"  ".to_vec();
        push_name(&mut author, &commit.author);
        author.push(b' ');
        author.extend_from_slice(date.as_bytes());
        self.part(&author, Some(Highlight::GitDetail));
        self.part(b"\n", None);
        self.ids.push(commit.id.clone());
    }

    /// The whole text under the heading `title`; `more` if git has more commits, which a note
    /// says when [`MAX_COMMITS`] were read.
    pub(crate) fn laid(&self, title: &str, more: bool) -> Laid {
        let mut head = Text::default();
        head.part(title.as_bytes(), Some(Highlight::GitSectionHeading));
        head.part(b"\n", None);
        let full = self.rows() >= MAX_COMMITS;
        let mut tail = Text::default();
        if more && full {
            tail.part(MORE_NOTE.as_bytes(), Some(Highlight::GitDetail));
            tail.part(b"\n", None);
        }
        let mut builder = Builder::new(None, Default::default());
        for piece in [&head.bytes, &self.bytes, &tail.bytes] {
            builder.push(piece);
        }
        builder.finish();
        let text = builder.snapshot();
        let len = head.len + self.len + tail.len;
        let mut highlights = Highlights::new(text.text().len());
        debug_assert_eq!(text.text().len(), len);
        let runs = (head.runs.iter().chain(&self.runs).chain(&tail.runs)).copied();
        highlights.replace(0..len, runs.collect::<Vec<_>>());
        let mut lines = Vec::with_capacity(self.rows() + 2);
        lines.push(None);
        lines.extend(self.ids.iter().cloned().map(Some));
        if more && full {
            lines.push(None);
        }
        Laid {
            text,
            highlights,
            built: Arc::new(Built {
                lines,
                more: more && !full,
            }),
        }
    }

    /// Adds `bytes`, a part of one line but a line break of its own, in `face`.
    fn part(&mut self, bytes: &[u8], face: Option<Highlight>) {
        let len = decoded_len(bytes);
        self.bytes.extend_from_slice(bytes);
        self.len += len;
        match self.runs.last_mut() {
            Some((run, last)) if *last == face => *run += len,
            _ if len > 0 => self.runs.push((len, face)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
