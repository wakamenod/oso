//! What `*lsp-rename*` shows of a rename's edits before they apply (ADR 0138, step 14, Q3): each
//! file, and each changed line as it is and as it would be, built off the UI thread.
//!
//! Must not depend on: the window system or the UI thread's state; this reads the file
//! system for the preview's worker.
//!
//! - An open buffer's lines are its text's, as the server had it. A file no buffer holds is read
//!   as a list of a server's places reads one (`grep::read_lines`): never one that may hold
//!   secrets, reached from the server's root with no link followed, within a budget.
//! - Lines an edit spans, or that touch, are one group, shown whole before and after.
//! - Every line shown has its controls, and its bidi and invisible characters, written as
//!   `\u{…}`, and is cut at [`SHOWN_MAX`] characters, so that no text can pass for another
//!   line or hide what changes.

use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use oso_core::display_map::cell::is_invisible;
use oso_core::rope::Rope;
use oso_lsp::position::{Encoding, Position};
use oso_lsp::protocol::Extra;

use crate::grep::{RowLines, read_lines};

/// The most characters of a line shown.
const SHOWN_MAX: usize = 400;

/// The most bytes read of the files no buffer holds, for one preview.
const READ_MAX: u64 = 64 << 20;

/// What a preview is built from: who renames what to what, and each file's edits.
pub(super) struct Snapshot {
    pub(super) server: String,
    pub(super) old: String,
    pub(super) new: String,
    /// The server's root, as a URI reads back, which a file no buffer holds is read from.
    pub(super) root: PathBuf,
    pub(super) home: Option<PathBuf>,
    pub(super) files: Vec<FileSnap>,
}

/// One file's edits, and the path it is shown by.
pub(super) struct FileSnap {
    pub(super) shown: String,
    pub(super) source: Source,
}

/// Where a file's lines are: a buffer's text, its edits in its bytes; or the file, its edits as
/// the server counts them.
pub(super) enum Source {
    Text {
        text: Rope,
        edits: Vec<(Range<usize>, String)>,
    },
    File {
        path: PathBuf,
        edits: Vec<Extra>,
        encoding: Encoding,
    },
}

/// The preview's text, or `None` once `live` says it is stale.
pub(super) fn build(snapshot: &Snapshot, live: &dyn Fn() -> bool) -> Option<String> {
    let edits: usize = (snapshot.files.iter())
        .map(|file| file.source.count())
        .sum();
    let unopened = (snapshot.files.iter())
        .filter(|file| matches!(file.source, Source::File { .. }))
        .count();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} renames \u{2018}{}\u{2019} to \u{2018}{}\u{2019}: {} in {}{}",
        shown(&snapshot.server),
        shown(&snapshot.old),
        shown(&snapshot.new),
        plural(edits, "edit"),
        plural(snapshot.files.len(), "file"),
        match unopened {
            0 => String::new(),
            count => format!(" ({count} not open)"),
        }
    );
    out.push_str(
        "y applies it, each buffer's edits one step to undo; n or q leaves all as it is\n",
    );
    let real_home = crate::walk::real_home(snapshot.home.as_deref());
    let mut budget = READ_MAX;
    let mut part = crate::grep::part_buffer();
    for file in &snapshot.files {
        if !live() {
            return None;
        }
        let _ = writeln!(
            out,
            "\n{} ({})",
            shown(&file.shown),
            plural(file.source.count(), "edit")
        );
        let groups = match &file.source {
            Source::Text { text, edits } => text_groups(text, edits),
            Source::File {
                path,
                edits,
                encoding,
            } => {
                let reading = (
                    snapshot.root.as_path(),
                    real_home.as_deref(),
                    &mut part,
                    &mut budget,
                );
                file_groups(path, (edits, *encoding), reading, live)?
            }
        };
        for group in groups {
            write_group(&mut out, &group);
        }
    }
    Some(out)
}

/// Lines `first` onwards (from 0) as they are and as the edits leave them; `None` for lines that
/// could not be read, with the edits' new text.
struct Group {
    first: usize,
    lines: Option<(String, String)>,
    new_texts: Vec<String>,
}

impl Source {
    fn count(&self) -> usize {
        match self {
            Self::Text { edits, .. } => edits.len(),
            Self::File { edits, .. } => edits.len(),
        }
    }
}

/// The groups of a buffer's text's edits.
fn text_groups(text: &Rope, edits: &[(Range<usize>, String)]) -> Vec<Group> {
    let rows: Vec<(usize, usize)> = (edits.iter())
        .map(|(range, _)| {
            let first = text.offset_to_point(range.start).row;
            (first, text.offset_to_point(range.end).row.max(first))
        })
        .collect();
    spans(&rows)
        .into_iter()
        .map(|(first, last, members)| {
            let range = text.line_start(first)..text.line_end(last);
            let local: Vec<(Range<usize>, &str)> = (members.iter())
                .map(|&at| {
                    let (edit, new) = &edits[at];
                    let start = edit.start.clamp(range.start, range.end) - range.start;
                    let end = edit.end.clamp(range.start, range.end) - range.start;
                    (start..end.max(start), new.as_str())
                })
                .collect();
            let before = text.slice(range).to_string();
            let after = applied(&before, &local);
            Group {
                first,
                lines: Some((before, after)),
                new_texts: Vec::new(),
            }
        })
        .collect()
}

/// The groups of a file's edits, its lines read from `root`; `None` once `live` says the
/// preview is stale.
fn file_groups(
    path: &Path,
    (edits, encoding): (&[Extra], Encoding),
    (root, home, part, budget): (&Path, Option<&Path>, &mut Vec<u8>, &mut u64),
    live: &dyn Fn() -> bool,
) -> Option<Vec<Group>> {
    let row = |position: &Position| position.line as usize;
    let rows: Vec<(usize, usize)> = (edits.iter())
        .map(|(start, end, _)| (row(start), row(end).max(row(start))))
        .collect();
    let groups = spans(&rows);
    let mut wanted: Vec<usize> = (groups.iter())
        .flat_map(|(first, last, _)| *first + 1..=*last + 1)
        .collect();
    wanted.sort_unstable();
    wanted.dedup();
    let read = match path.strip_prefix(root) {
        Ok(relative) => {
            let leads = crate::app::greps::leads(root);
            let found = read_lines(
                (root, leads.as_deref()),
                (relative, &wanted),
                home,
                (part, budget),
                live,
            )?;
            match found {
                RowLines::Read(lines, _) => lines,
                RowLines::Withheld | RowLines::Unreadable => Vec::new(),
            }
        }
        Err(_) => Vec::new(),
    };
    let line_of = |number: usize| {
        (read.iter())
            .find(|(at, _)| *at == number)
            .map(|(_, bytes)| bytes)
    };
    Some(
        groups
            .into_iter()
            .map(|(first, last, members)| {
                let lines: Option<Vec<&Vec<u8>>> = (first + 1..=last + 1).map(line_of).collect();
                // A line kept whole only: one cut where it was read shows no true after.
                let whole = lines
                    .filter(|lines| lines.iter().all(|line| line.len() < crate::grep::LINE_KEPT));
                let lines = whole.map(|lines| {
                    let joined = (lines.iter())
                        .map(|line| String::from_utf8_lossy(line).into_owned())
                        .collect::<Vec<_>>()
                        .join("\n");
                    let mini = Rope::from(joined.as_str());
                    let local: Vec<(Range<usize>, &str)> = (members.iter())
                        .map(|&at| {
                            let (start, end, new) = &edits[at];
                            let moved = |position: &Position| {
                                let line = position.line.saturating_sub(first as u32);
                                Position::new(line, position.character)
                            };
                            let start = oso_lsp::position::to_offset(&mini, moved(start), encoding);
                            let end = oso_lsp::position::to_offset(&mini, moved(end), encoding);
                            (start..end.max(start), new.as_str())
                        })
                        .collect();
                    let after = applied(&joined, &local);
                    (joined, after)
                });
                let new_texts = match lines {
                    Some(_) => Vec::new(),
                    None => (members.iter()).map(|&at| edits[at].2.clone()).collect(),
                };
                Group {
                    first,
                    lines,
                    new_texts,
                }
            })
            .collect(),
    )
}

/// The edits' rows, each `(first, last)`, joined where they overlap or touch: each group's first
/// and last row, and its edits by their place, in the order of their rows.
fn spans(rows: &[(usize, usize)]) -> Vec<(usize, usize, Vec<usize>)> {
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by_key(|&at| rows[at]);
    let mut groups: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for at in order {
        let (first, last) = rows[at];
        match groups.last_mut() {
            Some(group) if first <= group.1 + 1 => {
                group.1 = group.1.max(last);
                group.2.push(at);
            }
            _ => groups.push((first, last, vec![at])),
        }
    }
    groups
}

/// `text` with `edits`, ranges of its bytes, made; one that overlaps another before it is left
/// out, as the edit itself is refused then.
fn applied(text: &str, edits: &[(Range<usize>, &str)]) -> String {
    let mut sorted: Vec<&(Range<usize>, &str)> = edits.iter().collect();
    sorted.sort_by_key(|(range, _)| (range.start, range.end));
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, new) in sorted {
        let start = text.floor_char_boundary(range.start.min(text.len()));
        let end = text.floor_char_boundary(range.end.min(text.len()));
        if start < at {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(new);
        at = end.max(start);
    }
    out.push_str(&text[at..]);
    out
}

/// Writes `group`'s lines, each by its number from 1, as they are (`-`) and as they would be
/// (`+`).
fn write_group(out: &mut String, group: &Group) {
    match &group.lines {
        Some((before, after)) => {
            for (at, line) in before.split('\n').enumerate() {
                let _ = writeln!(out, "{:>6} - {}", group.first + at + 1, shown(line));
            }
            for (at, line) in after.split('\n').enumerate() {
                let _ = writeln!(out, "{:>6} + {}", group.first + at + 1, shown(line));
            }
        }
        None => {
            let _ = writeln!(
                out,
                "{:>6}   (the line could not be read whole; the new text is)",
                group.first + 1
            );
            for text in &group.new_texts {
                for line in text.split('\n') {
                    let _ = writeln!(out, "{:>6} + {}", group.first + 1, shown(line));
                }
            }
        }
    }
}

/// `text` on one line: its controls and its bidi and invisible characters as `\u{…}`, cut at
/// [`SHOWN_MAX`] characters.
pub(super) fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(SHOWN_MAX * 4));
    for (count, c) in text.chars().enumerate() {
        if count == SHOWN_MAX {
            out.push('…');
            break;
        }
        if c.is_control() || is_invisible(c) {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
        } else {
            out.push(c);
        }
    }
    out
}

fn plural(count: usize, noun: &str) -> String {
    match count {
        1 => format!("1 {noun}"),
        count => format!("{count} {noun}s"),
    }
}

#[cfg(test)]
mod tests;
