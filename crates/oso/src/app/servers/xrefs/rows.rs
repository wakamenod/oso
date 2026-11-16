//! The rows of a list of a server's places (ADR 0138, step 8b), built on a thread of their own
//! from the answer and the open buffers' texts when it came: in the server's order, grouped by
//! file in the order each file first comes, each with its line's text.
//!
//! Must not depend on: the app, the window system or the servers' threads; this works on a
//! snapshot, and the rows go back to the editor.
//!
//! - A row of an open buffer reads its line from the buffer's text then; any other file is read
//!   as the preview reads it (`grep::read_lines`), once for all its rows, at most
//!   `SCAN_MAX` of it and [`READ_MAX`] of all the files; past them a row has no text.
//! - A file that may hold secrets, by its name or as its buffer was judged, gives no text.
//! - A row shows at most [`ROW_TEXT_BYTES`] of its line, its leading blanks left out, each
//!   invalid byte one U+FFFD.
//! - A path is shown from the server's root, or whole with the home directory as `~`.
//! - A newer list or the list's end stops the build, between files and within one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{DiagnosticRowAt, VisitColumn, XrefRow, XrefRows};
use oso_core::rope::Rope;

use crate::grep::{Leads, RowLines, read_lines};

/// The most bytes read of all the files of one list (step 8b's Q5).
pub(super) const READ_MAX: u64 = 256 << 20;

/// The most bytes of a line a row shows (step 8's Q4).
pub(super) const ROW_TEXT_BYTES: usize = 256;

/// A place of the answer, as the UI thread found it.
pub(super) enum Spot {
    /// In open buffer `buffer`, at `offset` of its text now, whose revision is `revision`;
    /// `secret` if the buffer may hold secrets.
    Open {
        path: PathBuf,
        buffer: BufferId,
        offset: usize,
        revision: u64,
        secret: bool,
    },
    /// In a file, at `line` from 1 and `column`, as the server counts them.
    File {
        path: PathBuf,
        line: usize,
        column: VisitColumn,
    },
}

impl Spot {
    fn path(&self) -> &Path {
        match self {
            Self::Open { path, .. } | Self::File { path, .. } => path,
        }
    }
}

/// What the rows are built from.
pub(super) struct Snapshot {
    pub(super) generation: u64,
    /// The server's root, as a URI reads back, which the places' paths are too.
    pub(super) root: PathBuf,
    /// The home directory as the environment spells it, shown as `~`, and its real place,
    /// under which nothing is read.
    pub(super) home: (Option<PathBuf>, Option<PathBuf>),
    pub(super) spots: Vec<Spot>,
    /// The open buffers' texts, by buffer.
    pub(super) texts: HashMap<BufferId, Rope>,
    /// The answer was cut at its limit.
    pub(super) cut: bool,
}

/// The rows of `snapshot`, or `None` once `live` says the list is stale.
pub(super) fn build(
    snapshot: &Snapshot,
    leads: Leads,
    live: &dyn Fn() -> bool,
) -> Option<XrefRows> {
    let order = grouped(&snapshot.spots);
    let lines = read_files(snapshot, &order, leads, live)?;
    let (shown_home, _) = &snapshot.home;
    let mut shown: HashMap<&Path, Box<str>> = HashMap::new();
    let rows = (order.iter().map(|&index| &snapshot.spots[index]))
        .map(|spot| {
            let path = spot.path();
            let shown = shown
                .entry(path)
                .or_insert_with(|| shown_path(path, &snapshot.root, shown_home.as_deref()))
                .clone();
            match spot {
                Spot::Open {
                    buffer,
                    offset,
                    revision,
                    secret,
                    ..
                } => {
                    let text = snapshot.texts.get(buffer);
                    let line = text.map_or(0, |text| text.offset_to_point(*offset).row);
                    let read = match text {
                        Some(text) if !secret => line_of(text, line),
                        _ => Box::from(""),
                    };
                    XrefRow {
                        shown,
                        path: path.to_path_buf(),
                        line: line + 1,
                        at: DiagnosticRowAt::Open {
                            buffer: *buffer,
                            offset: *offset,
                            revision: *revision,
                        },
                        text: read,
                    }
                }
                Spot::File { line, column, .. } => {
                    let read = (lines.get(path))
                        .and_then(|found| found.get(line))
                        .map_or_else(|| Box::from(""), |bytes| row_text(bytes));
                    XrefRow {
                        shown,
                        path: path.to_path_buf(),
                        line: *line,
                        at: DiagnosticRowAt::File { column: *column },
                        text: read,
                    }
                }
            }
        })
        .collect();
    live().then(|| XrefRows::new(snapshot.generation, rows, snapshot.cut))
}

/// The spots' indices in the server's order, grouped by file in the order each file first
/// comes.
fn grouped(spots: &[Spot]) -> Vec<usize> {
    let mut groups: HashMap<&Path, usize> = HashMap::new();
    let mut keyed: Vec<(usize, usize)> = (spots.iter().enumerate())
        .map(|(index, spot)| {
            let next = groups.len();
            (*groups.entry(spot.path()).or_insert(next), index)
        })
        .collect();
    // Stable: within a file, the server's order.
    keyed.sort_by_key(|(group, _)| *group);
    keyed.into_iter().map(|(_, index)| index).collect()
}

/// The lines the files' rows show, by path and line, each file read once, in the rows' order,
/// within [`READ_MAX`] in all.
fn read_files<'a>(
    snapshot: &'a Snapshot,
    order: &[usize],
    leads: Leads,
    live: &dyn Fn() -> bool,
) -> Option<HashMap<&'a Path, HashMap<usize, Vec<u8>>>> {
    let mut wanted: Vec<(&Path, Vec<usize>)> = Vec::new();
    let mut at: HashMap<&Path, usize> = HashMap::new();
    for &index in order {
        let Spot::File { path, line, .. } = &snapshot.spots[index] else {
            continue;
        };
        let slot = *at.entry(path).or_insert_with(|| {
            wanted.push((path, Vec::new()));
            wanted.len() - 1
        });
        wanted[slot].1.push(*line);
    }
    let (_, real_home) = &snapshot.home;
    let mut budget = READ_MAX;
    let mut led: HashMap<PathBuf, Option<Vec<PathBuf>>> = HashMap::new();
    let mut found = HashMap::new();
    for (path, mut lines) in wanted {
        if !live() {
            return None;
        }
        if budget == 0 {
            break;
        }
        if Buffer::name_may_hold_secrets(path) {
            continue;
        }
        let Some((root, relative)) = reached_from(path, &snapshot.root) else {
            continue;
        };
        lines.sort_unstable();
        lines.dedup();
        let leads = led.entry(root.to_path_buf()).or_insert_with(|| leads(root));
        let read = read_lines(
            (root, leads.as_deref()),
            relative,
            &lines,
            real_home.as_deref(),
            &mut budget,
            live,
        )?;
        if let RowLines::Read(read) = read {
            found.insert(path, read.into_iter().collect());
        }
    }
    Some(found)
}

/// The root a file is read from and its path from there: the server's root for a file under
/// it, else its own directory.
fn reached_from<'a>(path: &'a Path, root: &'a Path) -> Option<(&'a Path, &'a Path)> {
    match path.strip_prefix(root) {
        Ok(relative) if relative.file_name().is_some() => Some((root, relative)),
        _ => Some((path.parent()?, Path::new(path.file_name()?))),
    }
}

/// `path` as a row shows it: from `root`, or whole with `home` as `~`.
fn shown_path(path: &Path, root: &Path, home: Option<&Path>) -> Box<str> {
    if let Ok(relative) = path.strip_prefix(root)
        && relative.file_name().is_some()
    {
        return relative.to_string_lossy().into();
    }
    if let Some(home) = home
        && let Ok(relative) = path.strip_prefix(home)
        && relative.file_name().is_some()
    {
        let separator = std::path::MAIN_SEPARATOR;
        return format!("~{separator}{}", relative.to_string_lossy()).into();
    }
    path.to_string_lossy().into()
}

/// The text a row shows of line `row` of `text`.
fn line_of(text: &Rope, row: usize) -> Box<str> {
    let start = text.line_start(row);
    let end = text.line_end(row).min(start + crate::grep::LINE_KEPT);
    let mut bytes = Vec::with_capacity(end - start);
    for chunk in text.chunks_in_range(start..end) {
        bytes.extend_from_slice(chunk.as_bytes());
    }
    row_text(&bytes)
}

/// At most [`ROW_TEXT_BYTES`] of `line`, its leading blanks and a carriage return at its end
/// left out, cut at a character, with `…` where more follows.
fn row_text(line: &[u8]) -> Box<str> {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let start = line
        .iter()
        .position(|byte| !matches!(byte, b' ' | b'\t'))
        .unwrap_or(line.len());
    let line = &line[start..];
    let mut end = line.len().min(ROW_TEXT_BYTES);
    // Back to a character's start, so the cut never splits one.
    while end < line.len() && end > 0 && (line[end] & 0xC0) == 0x80 {
        end -= 1;
    }
    let (text, _) = oso_core::project::row_text(&line[..end], 0..0);
    if end < line.len() {
        format!("{text}…").into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests;
