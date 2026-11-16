//! The rows of `M-g d`'s list (ADR 0138, step 6b), built on a thread of their own from what was
//! reported when the list opened: the current buffer's first, by their place, then each other
//! file's by its path and line, open in a buffer or not, at most [`DIAGNOSTIC_ROWS_MAX`] in all.
//! Hints are left out, as the counts leave them out (Q3).
//!
//! Must not depend on: the app, the window system or the servers' threads; this works on a
//! snapshot, and the rows go back to the editor.
//!
//! - An open buffer's own set is what its rows show, the one it follows the edits with; a file
//!   report for a path an open buffer holds is left out.
//! - The files are sorted by path before any row is made, and each file's reports by line, so
//!   no more rows are ever made than the list keeps, however much the servers reported.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::diagnostic::{Diagnostics, Severity};
use oso_core::editor::{DiagnosticRow, DiagnosticRowAt, DiagnosticRows, VisitColumn};
use oso_core::rope::Rope;
use oso_lsp::limits::DIAGNOSTIC_ROWS_MAX;
use oso_lsp::position::Encoding;
use oso_lsp::protocol::Published;

/// The most bytes of a message's first line copied into a row, more than a row keeps of it
/// (`ROW_MESSAGE_BYTES` in `oso_core::editor`'s list, 512), so that what the servers reported
/// is never copied whole.
const COPIED_BYTES: usize = 1024;

/// The first line of `message`, as a row shows it, cut to [`COPIED_BYTES`] at a character.
fn copied(message: &str) -> Box<str> {
    let line = message
        .split(['\n', '\r', '\u{85}', '\u{2028}', '\u{2029}'])
        .next()
        .unwrap_or(message);
    let mut end = line.len().min(COPIED_BYTES);
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line[..end].into()
}

/// An open buffer's set, with its text and revision when the list opened.
pub(crate) struct OpenSet {
    pub(crate) buffer: BufferId,
    pub(crate) path: PathBuf,
    pub(crate) text: Rope,
    pub(crate) revision: u64,
    pub(crate) set: Diagnostics,
}

/// What the rows are built from: the list's generation, the project's root, the buffer the list
/// was opened from and its cursor, the open buffers' sets, and the reports of files no buffer
/// holds, each with the encoding of its server's positions.
pub(crate) struct Snapshot {
    pub(crate) generation: u64,
    pub(crate) root: PathBuf,
    pub(crate) current: BufferId,
    pub(crate) cursor: usize,
    pub(crate) open: Vec<OpenSet>,
    pub(crate) files: Vec<(PathBuf, Arc<Published>, Encoding)>,
}

/// Where a file's rows come from.
enum Source<'a> {
    Open(&'a OpenSet),
    File(&'a Published, Encoding),
}

/// The rows of `snapshot`, in the list's order, with the first of the current buffer's at or
/// after its cursor selected, else the first row.
pub(crate) fn build(snapshot: &Snapshot) -> DiagnosticRows {
    let mut rows = Vec::new();
    let mut cut = false;
    let mut selected = None;
    let relative = |path: &Path| -> Option<Box<str>> {
        Some(path.strip_prefix(&snapshot.root).ok()?.to_str()?.into())
    };
    let current = snapshot
        .open
        .iter()
        .find(|open| open.buffer == snapshot.current);
    if let Some(open) = current
        && let Some(path) = relative(&open.path)
    {
        cut |= push_open(&mut rows, &path, open);
        selected = rows.iter().position(|row: &DiagnosticRow| {
            matches!(row.at, DiagnosticRowAt::Open { offset, .. } if offset >= snapshot.cursor)
        });
    }
    // The rest by path: open buffers' sets, and the reports of files no buffer holds.
    let mut others: Vec<(Box<str>, Source<'_>)> = Vec::new();
    for open in &snapshot.open {
        if open.buffer != snapshot.current
            && let Some(path) = relative(&open.path)
        {
            others.push((path, Source::Open(open)));
        }
    }
    for (path, report, encoding) in &snapshot.files {
        let held = snapshot.open.iter().any(|open| &open.path == path);
        if !held && let Some(relative) = relative(path) {
            others.push((relative, Source::File(report, *encoding)));
        }
    }
    others.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (path, source) in &others {
        if cut {
            break;
        }
        cut |= match source {
            Source::Open(open) => push_open(&mut rows, path, open),
            Source::File(report, encoding) => push_file(&mut rows, path, report, *encoding),
        };
    }
    let selected = selected.or((!rows.is_empty()).then_some(0));
    DiagnosticRows::new(snapshot.generation, rows, selected, cut)
}

/// Pushes the rows of an open buffer's set, by place, and says whether the list is full.
fn push_open(rows: &mut Vec<DiagnosticRow>, path: &str, open: &OpenSet) -> bool {
    for diagnostic in open.set.iter() {
        if diagnostic.severity() == Severity::Hint {
            continue;
        }
        if rows.len() == DIAGNOSTIC_ROWS_MAX {
            return true;
        }
        let offset = diagnostic.range().start;
        rows.push(DiagnosticRow {
            path: path.into(),
            line: open.text.offset_to_point(offset).row + 1,
            at: DiagnosticRowAt::Open {
                buffer: open.buffer,
                offset,
                revision: open.revision,
            },
            severity: diagnostic.severity(),
            message: copied(diagnostic.first_line()),
            source: diagnostic.source().into(),
            code: diagnostic.code().into(),
        });
    }
    false
}

/// Pushes the rows of a file's report, by line and column, and says whether the list is full.
fn push_file(
    rows: &mut Vec<DiagnosticRow>,
    path: &str,
    report: &Published,
    encoding: Encoding,
) -> bool {
    let mut kept: Vec<_> = (report.diagnostics.iter())
        .filter(|reported| reported.severity != Severity::Hint)
        .collect();
    kept.sort_by_key(|reported| (reported.start.line, reported.start.character));
    for reported in kept {
        if rows.len() == DIAGNOSTIC_ROWS_MAX {
            return true;
        }
        let character = reported.start.character as usize;
        rows.push(DiagnosticRow {
            path: path.into(),
            line: reported.start.line as usize + 1,
            at: DiagnosticRowAt::File {
                column: match encoding {
                    Encoding::Utf8 => VisitColumn::Utf8(character),
                    Encoding::Utf16 => VisitColumn::Utf16(character),
                },
            },
            severity: reported.severity,
            message: copied(&reported.message),
            source: reported.source.as_str().into(),
            code: reported.code.as_str().into(),
        });
    }
    false
}

#[cfg(test)]
mod tests;
