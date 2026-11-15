//! A listing's text, highlights and row table, from its entries in order (ADR 0134).
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the file system; [`build`] is a pure
//! function of what was read.
//!
//! Each row is the mark's column and a space, then with details the mode, the owner and group
//! when someone else owns any row, the size and the time, each column as wide as its widest,
//! then the name and a link's target. Every byte before the name is ASCII, so the name starts
//! at the same column, in bytes and cells, on every row.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use oso_core::load::{Builder, LoadSnapshot};
use oso_core::syntax::{Highlight, Highlights};
use oso_platform::file_meta::EntryKind;

use super::Row;
use super::rows::{decoded_len, push_name, size_text, time_text};

/// The most bytes of a link's target a row shows.
const MAX_TARGET: usize = 1024;

/// Where a link leads, as a look found; `Here` for an entry that is no link.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Leads {
    #[default]
    Here,
    Directory,
    Elsewhere,
    Missing,
    /// Not looked at yet, or it could not be told.
    Unknown,
}

/// An entry as a listing shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub(super) name: OsString,
    pub(super) kind: EntryKind,
    pub(super) mode: u32,
    /// Its mode column, as the platform writes it (`EntryMeta::mode_text`).
    pub(super) mode_text: String,
    pub(super) len: u64,
    pub(super) modified: Option<SystemTime>,
    /// Its owner's and group's names, only when someone other than this user owns it.
    pub(super) owner: Option<(Arc<str>, Arc<str>)>,
    /// A link's target, as its words say.
    pub(super) target: Option<PathBuf>,
    pub(super) leads: Leads,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            name: OsString::new(),
            kind: EntryKind::File,
            mode: 0,
            mode_text: String::new(),
            len: 0,
            modified: None,
            owner: None,
            target: None,
            leads: Leads::Here,
        }
    }
}

impl Entry {
    /// Whether `RET` on it lists it: a directory, or a link that leads to one.
    pub(super) fn enters(&self) -> bool {
        self.kind == EntryKind::Directory || self.leads == Leads::Directory
    }

    /// The face of its name.
    fn face(&self) -> Option<Highlight> {
        match self.kind {
            EntryKind::Directory => Some(Highlight::DiredDirectory),
            EntryKind::Link if self.leads == Leads::Missing => Some(Highlight::DiredBrokenSymlink),
            EntryKind::Link => Some(Highlight::DiredSymlink),
            EntryKind::Other => Some(Highlight::DiredSpecial),
            EntryKind::File if self.mode & 0o111 != 0 => Some(Highlight::DiredExecutable),
            EntryKind::File => None,
        }
    }
}

/// What [`build`] makes.
pub(super) struct Shown {
    pub(super) text: LoadSnapshot,
    pub(super) highlights: Highlights,
    pub(super) rows: Arc<[Row]>,
    pub(super) header_lines: usize,
    pub(super) name_column: usize,
    /// How many rows are marked.
    pub(super) marked: usize,
}

/// The time zone's offset at a moment, in seconds east of UTC.
pub(super) type Offsets<'a> = &'a mut dyn FnMut(SystemTime) -> i32;

/// What a listing shows beside its entries: the directory as the header line writes it, the
/// lines under it, whether rows show their details, and the names marked.
pub(super) struct Shape<'a> {
    pub(super) header: &'a [u8],
    pub(super) notes: &'a [String],
    pub(super) details: bool,
    pub(super) marked: &'a HashSet<OsString>,
}

/// Builds a listing: the header, a line for each note, then a row for each of `entries`, in
/// order, `.` and `..` among them, a marked one with `*` in its first column.
pub(super) fn build(
    shape: &Shape<'_>,
    entries: &[&Entry],
    now: SystemTime,
    offsets: Offsets<'_>,
) -> Shown {
    let Shape {
        header,
        notes,
        details,
        marked,
    } = *shape;
    let mut out = Out::default();
    out.line(b"  ", None);
    let mut path = Vec::with_capacity(header.len() + 1);
    push_name(&mut path, header);
    path.push(b':');
    out.part(&path, Some(Highlight::DiredHeader));
    out.end_line();
    for note in notes {
        out.line(b"  ", None);
        out.part(note.as_bytes(), Some(Highlight::DiredDetails));
        out.end_line();
    }
    let columns = details.then(|| Columns::of(entries, now, offsets));
    let mut rows = Vec::with_capacity(entries.len());
    let mut name_column = 2;
    let mut count = 0;
    for (index, entry) in entries.iter().enumerate() {
        let dot = entry.name == "." || entry.name == "..";
        let is_marked = !dot && marked.contains(&entry.name);
        if is_marked {
            count += 1;
            out.part(b"*", Some(Highlight::DiredMark));
            out.part(b" ", None);
        } else {
            out.line(b"  ", None);
        }
        if let Some(columns) = &columns {
            let detail = columns.row(index, entry);
            name_column = 2 + detail.len();
            out.part(detail.as_bytes(), Some(Highlight::DiredDetails));
        }
        let mut name = Vec::new();
        push_name(&mut name, entry.name.as_encoded_bytes());
        let face = if is_marked {
            Some(Highlight::DiredMarked)
        } else {
            entry.face()
        };
        out.part(&name, face);
        if let (Some(target), true) = (&entry.target, details) {
            let mut arrow = b" -> ".to_vec();
            let words = target.as_os_str().as_encoded_bytes();
            push_name(&mut arrow, &words[..words.len().min(MAX_TARGET)]);
            if words.len() > MAX_TARGET {
                arrow.extend_from_slice("…".as_bytes());
            }
            out.part(&arrow, None);
        }
        out.end_line();
        rows.push(Row {
            name: entry.name.clone(),
            enters: entry.enters(),
            modified: entry.modified,
            face: entry.face(),
            name_len: decoded_len(&name),
        });
    }
    let (text, highlights) = out.finish();
    Shown {
        text,
        highlights,
        rows: rows.into(),
        header_lines: 1 + notes.len(),
        name_column,
        marked: count,
    }
}

/// Each detail column's width and every row's text in it, which depend on all rows.
struct Columns {
    modes: Vec<String>,
    owners: Option<(usize, usize)>,
    sizes: Vec<String>,
    size_width: usize,
    times: Vec<String>,
    mode_width: usize,
}

impl Columns {
    fn of(entries: &[&Entry], now: SystemTime, offsets: Offsets<'_>) -> Self {
        let modes: Vec<String> = entries
            .iter()
            .map(|entry| entry.mode_text.clone())
            .collect();
        let sizes: Vec<String> = entries
            .iter()
            .map(|entry| match entry.kind {
                EntryKind::Directory => "-".to_owned(),
                _ => size_text(entry.len),
            })
            .collect();
        let times = entries
            .iter()
            .map(|entry| {
                let offset = entry.modified.map_or(0, &mut *offsets);
                time_text(entry.modified, now, offset)
            })
            .collect();
        let owners = entries
            .iter()
            .filter_map(|entry| entry.owner.as_ref())
            .map(|(user, group)| (user.len(), group.len()))
            .reduce(|a, b| (a.0.max(b.0), a.1.max(b.1)));
        Self {
            mode_width: modes.iter().map(String::len).max().unwrap_or(0),
            size_width: sizes.iter().map(String::len).max().unwrap_or(0),
            modes,
            owners,
            sizes,
            times,
        }
    }

    /// The details of the row at `index`, up to its name.
    fn row(&self, index: usize, entry: &Entry) -> String {
        let mut text = format!("{:<width$}  ", self.modes[index], width = self.mode_width);
        if let Some((users, groups)) = self.owners {
            match &entry.owner {
                Some((user, group)) => {
                    text += &format!("{user:<users$} {group:<groups$}  ");
                }
                None => text += &" ".repeat(users + 1 + groups + 2),
            }
        }
        text += &format!(
            "{:>width$}  {}  ",
            self.sizes[index],
            self.times[index],
            width = self.size_width
        );
        text
    }
}

/// The text as it is built, and its highlights' runs, in the decoded text's bytes.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
    builder: Option<Builder>,
    runs: Vec<(usize, Option<Highlight>)>,
    len: usize,
}

impl Out {
    /// Starts a line with `prefix`.
    fn line(&mut self, prefix: &[u8], face: Option<Highlight>) {
        self.part(prefix, face);
    }

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

    fn end_line(&mut self) {
        self.part(b"\n", None);
        // The builder takes whole lines, so that no sequence is cut between two pieces.
        if self.bytes.len() >= 64 * 1024 {
            let builder = self
                .builder
                .get_or_insert_with(|| Builder::new(None, Default::default()));
            builder.push(&self.bytes);
            self.bytes.clear();
        }
    }

    fn finish(mut self) -> (LoadSnapshot, Highlights) {
        let mut builder = self
            .builder
            .take()
            .unwrap_or_else(|| Builder::new(None, Default::default()));
        builder.push(&self.bytes);
        builder.finish();
        let text = builder.snapshot();
        let mut highlights = Highlights::new(text.text().len());
        debug_assert_eq!(text.text().len(), self.len);
        highlights.replace(0..self.len, self.runs);
        (text, highlights)
    }
}

#[cfg(test)]
mod tests;
