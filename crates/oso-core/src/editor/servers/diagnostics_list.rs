//! The list of a project's diagnostics, `M-g d` (`lsp-diagnostics`, ADR 0138, step 6b), as
//! `consult-flymake` lists them: the prompt opens at once, waiting, and the host builds its rows
//! on a worker from what each server reported, the current buffer's first, then each other
//! file's by its path, with the preview of the selected row's place.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! takes the request, builds the rows and hands them over, and reads a file the preview asks
//! for.
//!
//! - The rows are what was reported when the list opened; a report that comes while it is open
//!   changes nothing. A row of an open buffer goes to its place as the buffer's edits since
//!   have moved it, clipped to its text.
//! - Each row reads `path:line: severity: message`, the message's first line with its runs of
//!   blanks as one, at most [`ROW_MESSAGE_BYTES`] of it, and the source and the code as its
//!   annotation. The input matches it as any
//!   completion does.
//! - Nothing of a row reaches a log; `Debug` gives a generation at most.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::buffer::{Buffer, BufferId};
use crate::complete::{Candidate, WORKER_PAST};
use crate::diagnostic::{Severity, first_line};
use crate::editor::file_preview::{Lister, PreviewAt, Wanted};
use crate::editor::{CompletionStyle, Editor, Preview};
use crate::minibuffer::{PromptError, PromptId};
use crate::project::MAX_ROW_BYTES as ROW_MESSAGE_BYTES;

use super::listed::{Listed, ListedAt, ListedHit, file_column};

/// The rows the host should build for the list of a project's diagnostics. `Debug` gives its
/// generation only.
#[derive(Clone)]
pub struct DiagnosticsRequest {
    /// The list it is for, which [`DiagnosticRows::new`] is given back.
    pub generation: u64,
    /// The project's root: its files' diagnostics are listed, by their paths from it.
    pub root: PathBuf,
    /// The buffer the list was opened from, whose rows come first.
    pub buffer: BufferId,
    /// Where its primary cursor was: the first of its diagnostics at or after it is selected.
    pub cursor: usize,
}

impl fmt::Debug for DiagnosticsRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticsRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A diagnostic the list shows: its file's path from the root, its line from 1, where it is,
/// its severity, its message, of which the row shows the first line, and its source and code.
/// `Debug` says nothing of it.
pub struct DiagnosticRow {
    pub path: Box<str>,
    pub line: usize,
    pub at: ListedAt,
    pub severity: Severity,
    pub message: Box<str>,
    pub source: Box<str>,
    pub code: Box<str>,
}

impl fmt::Debug for DiagnosticRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticRow").finish_non_exhaustive()
    }
}

/// What a row keeps beside its candidate. The path itself is read from the candidate's text,
/// which starts with it.
pub(in crate::editor) struct RowPlace {
    /// The bytes of the path at the start of the candidate's text.
    path: usize,
    line: usize,
    at: ListedAt,
}

/// The rows of a list of diagnostics, made into candidates where the host builds them, so the
/// UI thread formats none of them. `Debug` gives its generation only.
pub struct DiagnosticRows {
    generation: u64,
    items: Arc<[Candidate]>,
    places: Vec<RowPlace>,
    selected: Option<usize>,
    cut: bool,
}

impl fmt::Debug for DiagnosticRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticRows")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl DiagnosticRows {
    /// The rows of the list `generation`, in the order given, with the row `selected` selected,
    /// and `cut` if the host left some out at its limit. O(n) in what the rows hold, so the host
    /// makes it on a worker.
    pub fn new(
        generation: u64,
        rows: Vec<DiagnosticRow>,
        selected: Option<usize>,
        cut: bool,
    ) -> Self {
        let mut places = Vec::with_capacity(rows.len());
        let items: Arc<[Candidate]> = rows
            .into_iter()
            .map(|row| {
                // Runs of blanks are one, so that no text the server pads out passes as oso's.
                let message = first_line(&row.message).split_whitespace();
                let mut text = format!("{}:{}: {}:", row.path, row.line, row.severity.word());
                let start = text.len();
                for word in message {
                    if text.len() - start >= ROW_MESSAGE_BYTES {
                        break;
                    }
                    text.push(' ');
                    text.push_str(word);
                }
                if text.len() - start > ROW_MESSAGE_BYTES {
                    let mut end = start + ROW_MESSAGE_BYTES;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    text.truncate(end);
                    text.push('…');
                }
                // The source and the code, their runs of blanks as one too.
                let annotation = (row.source.split_whitespace())
                    .chain(row.code.split_whitespace())
                    .collect::<Vec<_>>()
                    .join(" ");
                places.push(RowPlace {
                    path: row.path.len(),
                    line: row.line,
                    at: row.at,
                });
                Candidate::new(text, "", annotation)
            })
            .collect();
        Self {
            generation,
            items,
            places,
            selected,
            cut,
        }
    }
}

/// What the list of diagnostics keeps beside its rows: the list's own, and the rows to build,
/// until the host takes it.
#[derive(Default)]
pub(in crate::editor) struct DiagnosticsList {
    pub(in crate::editor) listed: Listed<RowPlace>,
    request: Option<DiagnosticsRequest>,
}

impl fmt::Debug for DiagnosticsList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DiagnosticsList")
            .field("generation", &self.listed.generation)
            .finish_non_exhaustive()
    }
}

impl Editor {
    /// Opens the list of the current buffer's project's diagnostics, `label` and its name its
    /// prompt's label. It opens empty and waiting; the host takes the request
    /// ([`Editor::take_diagnostics_request`]) and its rows come to
    /// [`Editor::diagnostics_listed`].
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] outside a project and a directory that stands for one, for a
    /// root whose path is not UTF-8, or while a list is open already. Nothing changes.
    pub fn open_diagnostics_list(&mut self, label: &str) -> Result<PromptId, PromptError> {
        let (root, name) = self
            .project_files_root()
            .map_err(|_| PromptError::Invalid)?;
        if root.to_str().is_none() || self.servers.list.listed.prompt.is_some() {
            return Err(PromptError::Invalid);
        }
        let (window, _) = self.current();
        let (buffer, cursor) = (window.buffer(), window.selections.primary().cursor);
        let style = CompletionStyle {
            given_order: true,
            preview: true,
            ..CompletionStyle::default()
        };
        let id = self.open_completion_as(
            format!("{label} in {name}:"),
            "lsp-diagnostics",
            Arc::from([]),
            style,
        )?;
        let generation = self.servers.list.listed.generation + 1;
        self.servers.list.request = Some(DiagnosticsRequest {
            generation,
            root: root.clone(),
            buffer,
            cursor,
        });
        self.servers.list.listed.open(id, generation, root);
        Ok(id)
    }

    /// The rows the host should build, once.
    pub fn take_diagnostics_request(&mut self) -> Option<DiagnosticsRequest> {
        self.servers.list.request.take()
    }

    /// Takes in the rows the host built: dropped, on the worker if many, unless they are for the
    /// list open now.
    pub fn diagnostics_listed(&mut self, rows: DiagnosticRows) {
        let Some(id) = self.servers.list.listed.waiting_for(rows.generation) else {
            if rows.items.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(rows));
            }
            return;
        };
        let DiagnosticRows {
            items,
            places,
            selected,
            cut,
            ..
        } = rows;
        self.servers.list.listed.take(places, cut);
        self.show_listed(id, items, selected);
    }

    /// Whether the innermost prompt lists diagnostics.
    pub(in crate::editor) fn lists_diagnostics(&self) -> bool {
        self.servers.list.listed.innermost(self)
    }

    /// Whether the innermost prompt lists diagnostics and their rows have not come.
    pub(in crate::editor) fn diagnostics_pending(&self) -> bool {
        self.lists_diagnostics() && self.servers.list.listed.pending
    }

    /// Whether the innermost prompt lists diagnostics and the host left some out.
    pub(in crate::editor) fn diagnostics_cut(&self) -> bool {
        self.lists_diagnostics() && self.servers.list.listed.cut
    }

    /// The selected row's item, its path from the root and its place, if the innermost prompt
    /// lists diagnostics.
    fn selected_diagnostic(&self) -> Option<(usize, &Path, &RowPlace)> {
        if !self.lists_diagnostics() {
            return None;
        }
        let candidates = self.prompts.last()?.candidates()?;
        let item = candidates.selected_item()?;
        let text = candidates.get(candidates.selected()?)?.text();
        let place = self.servers.list.listed.places.get(item)?;
        Some((item, Path::new(text.get(..place.path)?), place))
    }

    /// Where the open buffer of the row `item` is now (see [`Listed::open_now`]).
    fn open_place(&self, item: usize, at: ListedAt) -> Option<(BufferId, usize)> {
        self.servers.list.listed.open_now(self, item, at)
    }

    /// The selected row, as the preview of a file wants it: none for a row of an open buffer,
    /// which shows from memory.
    pub(in crate::editor) fn diagnostic_wanted(&self) -> Option<Wanted<'_>> {
        let (item, path, place) = self.selected_diagnostic()?;
        if self.open_place(item, place.at).is_some() {
            return None;
        }
        let root = &self.servers.list.listed.root;
        Some(Wanted {
            key: (self.servers.list.listed.generation, item),
            root,
            path,
            at: PreviewAt::Line {
                line: place.line,
                column: file_column(place.at),
            },
            // The server's thread drops a report of such a file, and the reader withholds one
            // it judges so; nothing of one is asked for either, as for a search's row.
            secret: Buffer::name_may_hold_secrets(&root.join(path)),
        })
    }

    /// What the preview of the innermost prompt's selected row shows, if it lists diagnostics.
    pub(in crate::editor) fn diagnostics_preview(&self) -> Option<Preview> {
        let (item, _, place) = self.selected_diagnostic()?;
        match self.open_place(item, place.at) {
            Some((id, offset)) => self.buffer_preview_at(id, offset),
            None => self.file_row_preview(Lister::Diagnostics),
        }
    }

    /// The whole path of the innermost prompt's selected row, if it lists diagnostics: its place
    /// is kept for the command ([`Editor::take_diagnostic_hit`]). Nothing goes in a history.
    pub(in crate::editor) fn diagnostics_answer(&mut self) -> Option<String> {
        let (item, path, place) = self.selected_diagnostic()?;
        let whole = self.servers.list.listed.root.join(path);
        let answer = whole.to_str()?.to_owned();
        let (at, line) = (place.at, place.line);
        let hit = self
            .servers
            .list
            .listed
            .hit_of(self, (item, at), (whole, line));
        self.servers.list.listed.hit = Some(hit);
        Some(answer)
    }

    /// The row the last list's `RET` took, once.
    pub fn take_diagnostic_hit(&mut self) -> Option<ListedHit> {
        self.servers.list.listed.hit.take()
    }

    /// Forgets the list of diagnostics, once its prompt has closed. Its places hold nothing on
    /// the heap of their own: one free, here.
    pub(in crate::editor) fn forget_diagnostics_list(&mut self, closed: PromptId) {
        self.servers.list.listed.close(closed);
    }
}

#[cfg(test)]
mod tests;
