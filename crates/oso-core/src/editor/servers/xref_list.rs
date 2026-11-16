//! The list of a language server's places, as `consult-xref` lists them (ADR 0138, Go to
//! definition and back, step 8b): several definitions `M-.` found, or the references `M-?` found,
//! one row each, in the server's order grouped by file, with the preview of the selected row's
//! place. The host opens it once the answer needs it, builds its rows on a worker and hands them
//! over; `RET` gives the row to `lsp-go-to-xref`.
//!
//! Must not depend on: `command` or `keymap`, or the host's servers, threads or I/O; the host
//! builds the rows, reads a file the preview asks for and makes the jump.
//!
//! - Each row reads `path:line: text`: the path from the server's root, or whole with the home
//!   directory as `~` outside it, and the line's text as the host read it, none for a file that
//!   may hold secrets. The input matches it as any completion does.
//! - A row of an open buffer goes to its place as the buffer's edits since have moved it, as
//!   every list of a server's work does (`listed`); any other goes to the server's line and
//!   column.
//! - A file outside the root is previewed from its own directory, so one right in a directory
//!   that is or holds the home directory shows nothing, as the reader reads under no such root.
//! - Nothing of a row reaches a log; `Debug` gives a generation at most.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::buffer::Buffer;
use crate::complete::{Candidate, WORKER_PAST};
use crate::editor::file_preview::{Lister, PreviewAt, Wanted};
use crate::editor::{CompletionStyle, Editor, Preview};
use crate::minibuffer::{PromptError, PromptId};

use super::listed::{Listed, ListedAt, ListedHit, file_column};

/// What a list of a server's places lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrefKind {
    /// The definitions `M-.` found.
    Definitions,
    /// The references `M-?` found.
    References,
}

impl XrefKind {
    fn label(self) -> &'static str {
        match self {
            Self::Definitions => "Definitions:",
            Self::References => "References:",
        }
    }
}

/// A place the list shows: its file's path as the row shows it and whole, its line from 1, where
/// it is, and the line's text as the host read it, empty if it did not. `Debug` says nothing of
/// it.
pub struct XrefRow {
    pub shown: Box<str>,
    pub path: PathBuf,
    pub line: usize,
    pub at: ListedAt,
    pub text: Box<str>,
}

impl fmt::Debug for XrefRow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XrefRow").finish_non_exhaustive()
    }
}

/// What a row keeps beside its candidate.
pub(in crate::editor) struct XrefPlace {
    path: PathBuf,
    line: usize,
    at: ListedAt,
}

/// The rows of a list of places, made into candidates where the host builds them, so the UI
/// thread formats none of them. `Debug` gives its generation only.
pub struct XrefRows {
    generation: u64,
    items: Arc<[Candidate]>,
    places: Vec<XrefPlace>,
    cut: bool,
}

impl fmt::Debug for XrefRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XrefRows")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl XrefRows {
    /// The rows of the list `generation`, in the order given, and `cut` if the server's answer
    /// was cut at its limit. O(n) in what the rows hold, so the host makes it on a worker.
    pub fn new(generation: u64, rows: Vec<XrefRow>, cut: bool) -> Self {
        let mut places = Vec::with_capacity(rows.len());
        let items: Arc<[Candidate]> = rows
            .into_iter()
            .map(|row| {
                let mut text = format!("{}:{}:", row.shown, row.line);
                if !row.text.is_empty() {
                    text.push(' ');
                    text.push_str(&row.text);
                }
                places.push(XrefPlace {
                    path: row.path,
                    line: row.line,
                    at: row.at,
                });
                Candidate::new(text, "", "")
            })
            .collect();
        Self {
            generation,
            items,
            places,
            cut,
        }
    }
}

/// What the list of places keeps beside its rows: the server's root is its root.
pub(in crate::editor) type XrefList = Listed<XrefPlace>;

impl Editor {
    /// Opens the list of the places of `kind` of the host's list `generation`, from the server
    /// of `root`, empty and waiting: its rows come to [`Editor::xref_listed`].
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] while a list of places is open already. Nothing changes.
    pub fn open_xref_list(
        &mut self,
        kind: XrefKind,
        generation: u64,
        root: PathBuf,
    ) -> Result<PromptId, PromptError> {
        if self.servers.xrefs.prompt.is_some() {
            return Err(PromptError::Invalid);
        }
        let style = CompletionStyle {
            given_order: true,
            preview: true,
            ..CompletionStyle::default()
        };
        let id = self.open_completion_as(kind.label(), "lsp-xrefs", Arc::from([]), style)?;
        self.servers.xrefs.open(id, generation, root);
        Ok(id)
    }

    /// Takes in the rows the host built: dropped, on the worker if many, unless they are for the
    /// list open now.
    pub fn xref_listed(&mut self, rows: XrefRows) {
        let Some(id) = self.servers.xrefs.waiting_for(rows.generation) else {
            if rows.items.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(rows));
            }
            return;
        };
        let XrefRows {
            items, places, cut, ..
        } = rows;
        self.servers.xrefs.take(places, cut);
        self.show_listed(id, items, None);
    }

    /// Whether the innermost prompt lists a server's places.
    pub(in crate::editor) fn lists_xrefs(&self) -> bool {
        self.servers.xrefs.innermost(self)
    }

    /// Whether the innermost prompt lists a server's places and their rows have not come.
    pub(in crate::editor) fn xrefs_pending(&self) -> bool {
        self.lists_xrefs() && self.servers.xrefs.pending
    }

    /// Whether the innermost prompt lists a server's places and its answer was cut.
    pub(in crate::editor) fn xrefs_cut(&self) -> bool {
        self.lists_xrefs() && self.servers.xrefs.cut
    }

    /// The selected row's item and place, if the innermost prompt lists a server's places.
    fn selected_xref(&self) -> Option<(usize, &XrefPlace)> {
        if !self.lists_xrefs() {
            return None;
        }
        let item = self.prompts.last()?.candidates()?.selected_item()?;
        Some((item, self.servers.xrefs.places.get(item)?))
    }

    /// The selected row, as the preview of a file wants it: none for a row of an open buffer,
    /// which shows from memory. A file under the server's root is reached from it, any other
    /// from its own directory, as the host's rows read it (`reached_from` in the `oso` crate's
    /// `app/servers/xrefs/rows.rs`); the two must agree.
    pub(in crate::editor) fn xref_wanted(&self) -> Option<Wanted<'_>> {
        let (item, place) = self.selected_xref()?;
        let list = &self.servers.xrefs;
        if list.open_now(self, item, place.at).is_some() {
            return None;
        }
        let root = list.root.as_path();
        let (root, path) = match place.path.strip_prefix(root) {
            Ok(path) if path.file_name().is_some() => (root, path),
            _ => (place.path.parent()?, Path::new(place.path.file_name()?)),
        };
        Some(Wanted {
            key: (list.generation, item),
            root,
            path,
            at: PreviewAt::Line {
                line: place.line,
                column: file_column(place.at),
            },
            // The row shows no text of such a file, and the reader withholds one it judges so.
            secret: Buffer::name_may_hold_secrets(&place.path),
        })
    }

    /// What the preview of the innermost prompt's selected row shows, if it lists a server's
    /// places.
    pub(in crate::editor) fn xrefs_preview(&self) -> Option<Preview> {
        let (item, place) = self.selected_xref()?;
        match self.servers.xrefs.open_now(self, item, place.at) {
            Some((id, offset)) => self.buffer_preview_at(id, offset),
            None => self.file_row_preview(Lister::Xrefs),
        }
    }

    /// The whole path of the innermost prompt's selected row, if it lists a server's places:
    /// its place is kept for the host ([`Editor::take_xref_hit`]). Nothing goes in a history.
    pub(in crate::editor) fn xrefs_answer(&mut self) -> Option<String> {
        let (item, place) = self.selected_xref()?;
        let answer = place.path.to_str()?.to_owned();
        let (at, whole) = ((item, place.at), (place.path.clone(), place.line));
        let hit = self.servers.xrefs.hit_of(self, at, whole);
        self.servers.xrefs.hit = Some(hit);
        Some(answer)
    }

    /// The row the last list of places' `RET` took, once.
    pub fn take_xref_hit(&mut self) -> Option<ListedHit> {
        self.servers.xrefs.hit.take()
    }

    /// Forgets the list of places, once its prompt has closed; its places, each with its path,
    /// are freed off the UI thread.
    pub(in crate::editor) fn forget_xref_list(&mut self, closed: PromptId) {
        if let Some(places) = self.servers.xrefs.close(closed)
            && !places.is_empty()
        {
            self.free_on_the_worker(Box::new(places));
        }
    }
}

#[cfg(test)]
mod tests;
