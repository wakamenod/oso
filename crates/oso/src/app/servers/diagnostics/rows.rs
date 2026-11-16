//! The rows of `M-g d`'s list (ADR 0138, step 6b), built on a thread of their own from what was
//! reported when the list opened: the current buffer's first, by their place, then each other
//! file's by its path and line, open in a buffer or not, from every server, at most
//! [`DIAGNOSTIC_ROWS_MAX`] in all and [`ROWS_BYTES_MAX`] of text. Hints are left out, as the
//! counts leave them out (Q3).
//!
//! Must not depend on: the app, the window system or the servers' threads; this works on a
//! snapshot, and the rows go back to the editor.
//!
//! - A file's report is the server's for a file no document open with it holds; once one opens
//!   it, the server's thread forgets it, so a buffer's rows come from its own set.
//! - The reports' paths are compared with the root as a URI reads back, as the server's thread
//!   judged them, so that on Windows a drive letter in either case and a `\\?\` root match.
//! - A path longer than [`ROW_PATH_BYTES_MAX`] gives no row: a server may name any path, and
//!   each row holds its own.
//! - The files are sorted by path before any row is made, and each file's reports by line, so
//!   no more rows are ever made than the list keeps, however much the servers reported. A newer
//!   list stops the build.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::buffer::BufferId;
use oso_core::diagnostic::{Diagnostics, Severity};
use oso_core::editor::{DiagnosticRow, DiagnosticRowAt, DiagnosticRows, VisitColumn};
use oso_core::rope::Rope;
use oso_lsp::limits::DIAGNOSTIC_ROWS_MAX;
use oso_lsp::position::Encoding;
use oso_lsp::protocol::Published;

/// The most bytes of a message copied into a row, more than a row keeps of its first line
/// (`oso_core::project::MAX_ROW_BYTES`, 512), so that what the servers reported is never
/// copied whole.
const COPIED_BYTES: usize = 1024;

/// The longest path from the root a row is made for, as long as a walk's longest path.
const ROW_PATH_BYTES_MAX: usize = 4096;

/// The most bytes of paths and messages the rows hold in all; past it the list is cut, as at
/// [`DIAGNOSTIC_ROWS_MAX`]. The list's text and the folded copies matching makes stay within a
/// few times this.
const ROWS_BYTES_MAX: usize = 32 << 20;

/// How many rows are made between looks at whether a newer list has stopped this one.
const LOOK_EVERY: usize = 4096;

/// At most [`COPIED_BYTES`] of `message`, cut at a character: the row cuts its first line.
fn copied(message: &str) -> Box<str> {
    let mut end = message.len().min(COPIED_BYTES);
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message[..end].into()
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
/// was opened from and its cursor, the open buffers' sets, and the servers' reports of files no
/// document open with them holds, each with the encoding of its server's positions.
pub(crate) struct Snapshot {
    pub(crate) generation: u64,
    pub(crate) root: PathBuf,
    pub(crate) current: BufferId,
    pub(crate) cursor: usize,
    pub(crate) open: Vec<OpenSet>,
    pub(crate) files: Vec<(Arc<Path>, Arc<Published>, Encoding)>,
}

/// Where a file's rows come from.
enum Source<'a> {
    Open(&'a OpenSet),
    File(&'a Published, Encoding),
}

/// The rows being made, within their bounds.
struct Rows<'a> {
    rows: Vec<DiagnosticRow>,
    bytes: usize,
    cut: bool,
    live: &'a dyn Fn() -> bool,
}

impl Rows<'_> {
    /// Whether one more row of `bytes` fits; past a bound, or once the build is stale, the list
    /// is cut.
    fn room(&mut self, bytes: usize) -> bool {
        if self.cut {
            return false;
        }
        if self.rows.len().is_multiple_of(LOOK_EVERY) && !(self.live)() {
            self.cut = true;
            return false;
        }
        if self.rows.len() == DIAGNOSTIC_ROWS_MAX || self.bytes + bytes > ROWS_BYTES_MAX {
            self.cut = true;
            return false;
        }
        self.bytes += bytes;
        true
    }
}

/// The rows of `snapshot`, in the list's order, with the first of the current buffer's at or
/// after its cursor selected, else the first row: `None` once `live` says a newer list stopped
/// this one.
pub(crate) fn build(snapshot: &Snapshot, live: &dyn Fn() -> bool) -> Option<DiagnosticRows> {
    let mut rows = Rows {
        rows: Vec::new(),
        bytes: 0,
        cut: false,
        live,
    };
    let mut selected = None;
    let relative = |path: &Path, root: &Path| -> Option<Box<str>> {
        let relative = path.strip_prefix(root).ok()?.to_str()?;
        (relative.len() <= ROW_PATH_BYTES_MAX).then(|| relative.into())
    };
    // The root as the servers' paths spell it.
    let server_root = oso_lsp::uri::from_path(&snapshot.root)
        .and_then(|uri| oso_lsp::uri::to_path(&uri))
        .unwrap_or_else(|| snapshot.root.clone());
    let current = snapshot
        .open
        .iter()
        .find(|open| open.buffer == snapshot.current);
    if let Some(open) = current
        && let Some(path) = relative(&open.path, &snapshot.root)
    {
        push_open(&mut rows, &path, open);
        selected = rows.rows.iter().position(|row| {
            matches!(row.at, DiagnosticRowAt::Open { offset, .. } if offset >= snapshot.cursor)
        });
    }
    // The rest by path: open buffers' sets, and the servers' reports of other files.
    let mut others: Vec<(Box<str>, Source<'_>)> = Vec::new();
    for open in &snapshot.open {
        if open.buffer != snapshot.current
            && let Some(path) = relative(&open.path, &snapshot.root)
        {
            others.push((path, Source::Open(open)));
        }
    }
    for (path, report, encoding) in &snapshot.files {
        if let Some(path) = relative(path, &server_root) {
            others.push((path, Source::File(report, *encoding)));
        }
    }
    others.sort_by(|(a, _), (b, _)| a.cmp(b));
    for (path, source) in &others {
        if rows.cut {
            break;
        }
        match source {
            Source::Open(open) => push_open(&mut rows, path, open),
            Source::File(report, encoding) => push_file(&mut rows, path, report, *encoding),
        }
    }
    if !live() {
        return None;
    }
    let selected = selected.or((!rows.rows.is_empty()).then_some(0));
    Some(DiagnosticRows::new(
        snapshot.generation,
        rows.rows,
        selected,
        rows.cut,
    ))
}

/// Pushes the rows of an open buffer's set, by place, while they fit.
fn push_open(rows: &mut Rows<'_>, path: &str, open: &OpenSet) {
    for diagnostic in open.set.iter() {
        if diagnostic.severity() == Severity::Hint {
            continue;
        }
        let message = copied(diagnostic.first_line());
        let (source, code) = (diagnostic.source(), diagnostic.code());
        if !rows.room(path.len() + message.len() + source.len() + code.len()) {
            return;
        }
        let offset = diagnostic.range().start;
        rows.rows.push(DiagnosticRow {
            path: path.into(),
            line: open.text.offset_to_point(offset).row + 1,
            at: DiagnosticRowAt::Open {
                buffer: open.buffer,
                offset,
                revision: open.revision,
            },
            severity: diagnostic.severity(),
            message,
            source: source.into(),
            code: code.into(),
        });
    }
}

/// Pushes the rows of a file's report, by line and column, while they fit.
fn push_file(rows: &mut Rows<'_>, path: &str, report: &Published, encoding: Encoding) {
    let mut kept: Vec<_> = (report.diagnostics.iter())
        .filter(|reported| reported.severity != Severity::Hint)
        .collect();
    kept.sort_by_key(|reported| (reported.start.line, reported.start.character));
    for reported in kept {
        let message = copied(&reported.message);
        let (source, code) = (&reported.source, &reported.code);
        if !rows.room(path.len() + message.len() + source.len() + code.len()) {
            return;
        }
        let character = reported.start.character as usize;
        rows.rows.push(DiagnosticRow {
            path: path.into(),
            line: reported.start.line as usize + 1,
            at: DiagnosticRowAt::File {
                column: match encoding {
                    Encoding::Utf8 => VisitColumn::Utf8(character),
                    Encoding::Utf16 => VisitColumn::Utf16(character),
                },
            },
            severity: reported.severity,
            message,
            source: source.as_str().into(),
            code: code.as_str().into(),
        });
    }
}

#[cfg(test)]
mod tests;
