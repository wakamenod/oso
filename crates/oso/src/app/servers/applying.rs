//! A server's edit that touches files whose text the server does not have, and its own
//! `workspace/applyEdit` (ADR 0138, step 13b), on the UI thread: such a file is the buffer that
//! holds it, unmodified, or opens through the loader, unshown, after the person's yes (Q3, Q4),
//! and the edit applies once each is loaded and judged.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a load or a
//! server.
//!
//! - Before anything opens: a file outside the server's root, in a `.git` directory, or whose
//!   name may hold secrets refuses the whole edit, as does a buffer that holds the file with
//!   changes the server has not seen, may hold secrets, or is still opening for the agent.
//! - Once loaded (`judge_loaded`, as the agent channel's open judges, `app/loads/opened.rs`): a file
//!   that may hold secrets, is not there, leads elsewhere through a link, out of the root, or is
//!   open under another name refuses it. So does a held buffer that changed since. What it
//!   opened then goes, unsaved; what it applied to is left unsaved, as Eglot leaves it.
//! - Each file opens at most [`DOCUMENT_BYTES_MAX`], what a server is ever sent, unshown: a
//!   buffer for another name of a file open, or a directory, is never shown for it (`loads`).
//! - The question names the counts, the first files and the action's title; `n`, or the
//!   question closing, leaves everything as it was. At most one edit waits: while one waits for
//!   its question, a newer one is refused, so that no `y` answers another.
//! - A server's `workspace/applyEdit` goes the same way, and its answer goes back once the edit
//!   is applied or refused. An edit whose server ended is refused.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::editor::{BufferEdit, Editor};
use oso_core::minibuffer::PromptId;
use oso_lsp::limits::DOCUMENT_BYTES_MAX;
use oso_lsp::server::{ApplyEdit, ServerEdit, UnsentEdit};

use super::super::App;
use super::super::loads::opened::{found_elsewhere, held_elsewhere};
use super::actions::{Picked, said_of};

/// Where an edit came from: a picked action, with the command to run once it applied; the
/// server's `workspace/applyEdit` of a token, which its outcome answers; or a rename, whose
/// files open with no question and whose edits, placed, go to its preview rather than apply:
/// `*lsp-rename*` is its question (step 14).
#[derive(Debug)]
pub(super) enum EditFrom {
    Action(Option<Picked>),
    Server(u64),
    Rename,
}

/// An edit that waits for the person's yes, or for the files it opened to load: its server, by
/// its root and entry, its title, where it came from, the edits placed so far, the files still
/// to place, the buffers it opened, and its question. `Debug` gives counts only: an edit's text
/// may quote what the server read.
pub(super) struct PendingEdit {
    server: (PathBuf, String),
    title: String,
    from: EditFrom,
    parts: Vec<BufferEdit>,
    files: Vec<(At, UnsentEdit)>,
    opened: Vec<BufferId>,
    question: Option<PromptId>,
}

/// Where a file an edit touches is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum At {
    /// In a buffer, unmodified, at this revision.
    Held(BufferId, u64),
    /// In a buffer still loading, which the edit waits for.
    Loading(BufferId),
    /// Nowhere: it opens after the person's yes.
    ToOpen,
    /// Opened for this edit.
    Opened(BufferId),
}

impl std::fmt::Debug for PendingEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingEdit")
            .field("parts", &self.parts.len())
            .field("files", &self.files.len())
            .field("opened", &self.opened.len())
            .field("question", &self.question)
            .finish_non_exhaustive()
    }
}

/// Where the file at `path` is, for an edit of the server whose root, as a URI reads back, is
/// `root`, `held` the buffer that visits that path, if one does; or why the edit is refused.
pub(super) fn judge_unsent(
    editor: &Editor,
    root: &Path,
    (path, held): (&Path, Option<BufferId>),
) -> Result<At, &'static str> {
    if !path.starts_with(root) {
        return Err("it edits a file outside the project");
    }
    if path
        .components()
        .any(|part| part == Component::Normal(".git".as_ref()))
    {
        return Err("it edits a file in a .git directory");
    }
    if Buffer::name_may_hold_secrets(path) {
        return Err("it edits a file that may hold secrets");
    }
    let Some(id) = held else {
        return Ok(At::ToOpen);
    };
    let buffer = editor.buffer(id).ok_or("a buffer it edits is gone")?;
    if buffer.agent_pending() {
        return Err("a file it edits is still opening for the agent");
    }
    // Judged once it is read: until then it may count as holding secrets.
    if waits(buffer) {
        return Ok(At::Loading(id));
    }
    if buffer.is_secret() {
        return Err("it edits a file that may hold secrets");
    }
    if buffer.is_modified() {
        return Err("a buffer it edits has changes the server has not seen");
    }
    Ok(At::Held(id, buffer.revision()))
}

/// Whether `buffer` is not judged or read yet.
fn waits(buffer: &Buffer) -> bool {
    buffer.verdict() == Verdict::Unjudged || (!buffer.is_secret() && buffer.loading().is_some())
}

/// What the file at `path`, in buffer `id`, held at `revision` or loaded for the edit, comes to
/// once judged and read: the revision to place the edit at, or why the edit is refused. `roots`
/// are the server's root as a URI reads back and as it is.
pub(super) fn judge_loaded(
    editor: &Editor,
    (id, revision): (BufferId, Option<u64>),
    path: &Path,
    roots: (&Path, &Path),
) -> Result<u64, &'static str> {
    let buffer = editor.buffer(id).ok_or("a file it edits could not open")?;
    if buffer.is_secret() {
        return Err("it edits a file that may hold secrets");
    }
    match revision {
        Some(revision) if buffer.revision() != revision || buffer.is_modified() => {
            return Err("a buffer it edits changed while it waited");
        }
        Some(_) => {}
        None if buffer.is_modified() => {
            return Err("a buffer it edits has changes the server has not seen");
        }
        None => {
            if let Some(why) = found_elsewhere(buffer, path.file_name()) {
                return Err(why);
            }
            if held_elsewhere(editor, id).is_some() {
                return Err("a file it edits is open under another name");
            }
        }
    }
    // Where the file really is, links followed, is in the project too.
    let inside = buffer.baseline().is_some_and(|baseline| {
        baseline.location.starts_with(roots.0) || baseline.location.starts_with(roots.1)
    });
    if !inside {
        return Err("a file it edits leads outside the project");
    }
    Ok(buffer.revision())
}

/// The buffers that visit any of `paths`, in one pass over the buffers.
pub(super) fn holders(editor: &Editor, paths: &[&Path]) -> HashMap<PathBuf, BufferId> {
    let wanted: HashSet<&Path> = paths.iter().copied().collect();
    (editor.buffers())
        .filter_map(|(id, buffer)| {
            let path = buffer.path()?;
            wanted.contains(path).then(|| (path.to_path_buf(), id))
        })
        .collect()
}

/// The question before an edit of `count` files opens `unopened`, naming the first of them
/// from `root`, their controls and invisible characters written out.
fn question_of(
    name: &str,
    title: &str,
    (count, unopened): (usize, &[&Path]),
    root: &Path,
) -> String {
    let files = if count == 1 { "file" } else { "files" };
    let shown: Vec<String> = (unopened.iter().take(3))
        .map(|path| {
            let path = path.strip_prefix(root).unwrap_or(path);
            crate::channel::diagnostics::server_text(&path.to_string_lossy(), false)
        })
        .collect();
    let more = unopened.len().saturating_sub(shown.len());
    let and_more = if more > 0 {
        format!(" and {more} more")
    } else {
        String::new()
    };
    format!(
        "{name}: apply to {count} {files}, {} not open ({}{and_more})? \u{201c}{title}\u{201d}",
        unopened.len(),
        shown.join(", ")
    )
}

impl App {
    /// Applies `edit`, `server`'s edit titled `title`, whole or not at all: at once if every
    /// file it touches is in a buffer, else after the question and the loads.
    pub(super) fn apply_edit_of(
        &mut self,
        server: &(PathBuf, String),
        title: &str,
        edit: ServerEdit,
        from: EditFrom,
    ) {
        let asking = self.servers.pending.as_ref();
        if asking.is_some_and(|pending| pending.question.is_some()) {
            self.freer.free(edit);
            return self.refuse_edit(server, title, &from, "another edit waits for your answer");
        }
        if let Some(older) = self.servers.pending.take() {
            self.drop_pending(older, "a newer edit came");
        }
        let Some(index) = self.servers.find(&server.0, &server.1) else {
            self.freer.free(edit);
            return self.refuse_edit(server, title, &from, "its server ended");
        };
        let running = &self.servers.running[index];
        let root = super::read_back(&running.root).unwrap_or_else(|| running.root.clone());
        let mut parts = Vec::with_capacity(edit.open.len());
        for open in edit.open {
            let Some(buffer) = running.buffer_of(open.document) else {
                return self.refuse_edit(server, title, &from, "a buffer it edits is gone");
            };
            parts.push(BufferEdit {
                buffer,
                revision: open.revision,
                edits: open.edits,
            });
        }
        let paths: Vec<&Path> = (edit.unsent.iter())
            .map(|unsent| unsent.path.as_path())
            .collect();
        let held = holders(&self.editor, &paths);
        let mut files = Vec::with_capacity(edit.unsent.len());
        for unsent in edit.unsent {
            let found = held.get(&unsent.path).copied();
            match judge_unsent(&self.editor, &root, (&unsent.path, found)) {
                Ok(at) => files.push((at, unsent)),
                Err(why) => return self.refuse_edit(server, title, &from, why),
            }
        }
        let unopened: Vec<&Path> = (files.iter())
            .filter(|(at, _)| *at == At::ToOpen)
            .map(|(_, unsent)| unsent.path.as_path())
            .collect();
        let count = parts.len() + files.len();
        // A rename's preview was its question (step 14).
        let renames = matches!(from, EditFrom::Rename);
        let question = (!unopened.is_empty() && !renames)
            .then(|| question_of(&server.1, title, (count, &unopened), &root));
        let mut pending = PendingEdit {
            server: server.clone(),
            title: title.to_owned(),
            from,
            parts,
            files,
            opened: Vec::new(),
            question: None,
        };
        let Some(question) = question else {
            if renames {
                return self.open_and_step(pending);
            }
            self.servers.pending = Some(pending);
            return self.step_pending_edit();
        };
        // Files to open: the person says yes first (Q4).
        match self.editor.ask(question, "lsp-code-action-confirm") {
            Ok(prompt) => {
                pending.question = Some(prompt);
                self.servers.pending = Some(pending);
            }
            Err(_) => self.drop_pending(pending, "too many prompts are open"),
        }
        self.request_redraw();
    }

    /// `lsp-code-action-confirm`, the question's `y`: opens the files the waiting edit needs.
    pub(super) fn confirm_server_edit(&mut self) {
        // Only the `y` of its own question, which has closed by now.
        let answered = (self.servers.pending.as_ref())
            .and_then(|pending| pending.question)
            .is_some_and(|question| !self.editor.prompts().iter().any(|p| p.id() == question));
        if !answered {
            return;
        }
        let Some(mut pending) = self.servers.pending.take() else {
            return;
        };
        pending.question = None;
        self.open_and_step(pending);
    }

    /// Opens the files `pending` needs that no buffer holds, unshown, and steps it.
    fn open_and_step(&mut self, mut pending: PendingEdit) {
        for (at, unsent) in &mut pending.files {
            if *at != At::ToOpen {
                continue;
            }
            let limit = u64::try_from(DOCUMENT_BYTES_MAX).unwrap_or(u64::MAX);
            let Some(id) = self.visit(unsent.path.clone(), limit) else {
                return self.drop_pending(pending, "a file it edits could not open");
            };
            *at = At::Opened(id);
            pending.opened.push(id);
        }
        self.servers.pending = Some(pending);
        self.step_pending_edit();
    }

    /// Whether buffer `id` opened for a waiting edit: its load shows nothing (`loads`).
    pub(in crate::app) fn opened_for_an_edit(&self, id: BufferId) -> bool {
        (self.servers.pending.as_ref()).is_some_and(|pending| pending.opened.contains(&id))
    }

    /// After each event: the waiting edit applies once its files are judged and read, or goes
    /// once its question closed with no yes, or its server ended.
    pub(super) fn step_pending_edit(&mut self) {
        let Some(pending) = &self.servers.pending else {
            return;
        };
        let gone = self
            .servers
            .find(&pending.server.0, &pending.server.1)
            .is_none();
        // Closed with no yes: `y` would have taken it first.
        let declined = pending
            .question
            .is_some_and(|question| !self.editor.prompts().iter().any(|p| p.id() == question));
        if gone || declined {
            if let Some(pending) = self.servers.pending.take() {
                let why = if gone {
                    "its server ended"
                } else {
                    "not applied"
                };
                self.drop_pending(pending, why);
            }
            return;
        }
        if pending.question.is_some() {
            return;
        }
        let waiting = pending.files.iter().any(|(at, _)| match at {
            At::Loading(id) | At::Opened(id) => self.editor.buffer(*id).is_some_and(waits),
            At::Held(..) | At::ToOpen => false,
        });
        if !waiting && let Some(pending) = self.servers.pending.take() {
            self.finish_edit(pending);
        }
    }

    /// Places the waiting edit's files in their buffers' texts and applies it all.
    fn finish_edit(&mut self, mut pending: PendingEdit) {
        let Some(index) = self.servers.find(&pending.server.0, &pending.server.1) else {
            return self.drop_pending(pending, "its server ended");
        };
        let real_root = self.servers.running[index].root.clone();
        let root = super::read_back(&real_root).unwrap_or_else(|| real_root.clone());
        for (at, unsent) in std::mem::take(&mut pending.files) {
            let (id, revision) = match at {
                At::Held(id, revision) => (id, Some(revision)),
                At::Loading(id) | At::Opened(id) => (id, None),
                At::ToOpen => return self.drop_pending(pending, "a file it edits did not open"),
            };
            let roots = (root.as_path(), real_root.as_path());
            let revision = match judge_loaded(&self.editor, (id, revision), &unsent.path, roots) {
                Ok(revision) => revision,
                Err(why) => return self.drop_pending(pending, why),
            };
            let Some(text) = self.editor.buffer(id).map(Buffer::text) else {
                return self.drop_pending(pending, "a file it edits could not open");
            };
            let edits = (unsent.edits.into_iter())
                .map(|(start, end, new)| {
                    let range = oso_lsp::position::to_range(text, start, end, unsent.encoding);
                    (range, new)
                })
                .collect();
            pending.parts.push(BufferEdit {
                buffer: id,
                revision,
                edits,
            });
        }
        let name = pending.server.1.clone();
        let parts = std::mem::take(&mut pending.parts);
        // A rename's edits, placed in the texts it opened, go to its preview (step 14).
        if matches!(pending.from, EditFrom::Rename) {
            let opened = std::mem::take(&mut pending.opened);
            return self.rename_placed(&pending.server, parts, opened);
        }
        match self.editor.apply_server_edit(parts) {
            Ok(applied) => {
                let edits = if applied.edits == 1 { "edit" } else { "edits" };
                let buffers = if applied.buffers == 1 {
                    "buffer"
                } else {
                    "buffers"
                };
                let said = format!("{} {edits} in {} {buffers}", applied.edits, applied.buffers);
                self.editor
                    .message_unlogged(said_of(&name, &said, &pending.title));
                self.edit_answered(&pending.server, &pending.from, Ok(()));
                if let EditFrom::Action(Some(then)) = pending.from {
                    self.servers.command_next = Some(then);
                }
            }
            Err(why) => self.drop_pending(pending, why),
        }
        self.request_redraw();
    }

    /// Kills the buffers an edit opened, unsaved and unshown.
    pub(super) fn kill_opened(&mut self, opened: &[BufferId]) {
        for id in opened {
            self.cancel_load(*id);
            self.editor.kill_buffer(*id);
        }
    }

    /// Whether a rename's edit waits for its files to open (step 14).
    pub(super) fn rename_opening(&self) -> bool {
        (self.servers.pending.as_ref())
            .is_some_and(|pending| matches!(pending.from, EditFrom::Rename))
    }

    /// Drops a rename's edit that waits for its files, what it opened with it, saying `why`.
    pub(super) fn drop_rename_opening(&mut self, why: &str) {
        if self.rename_opening()
            && let Some(pending) = self.servers.pending.take()
        {
            self.drop_pending(pending, why);
        }
    }

    /// Refuses the waiting edit with `why`: what it opened goes, and its server hears why if it
    /// asked.
    fn drop_pending(&mut self, pending: PendingEdit, why: &str) {
        self.kill_opened(&pending.opened);
        self.refuse_edit(&pending.server, &pending.title, &pending.from, why);
        self.freer.free(pending.files);
    }

    /// Says why the edit titled `title` changes nothing, and answers its server if it asked.
    fn refuse_edit(&mut self, server: &(PathBuf, String), title: &str, from: &EditFrom, why: &str) {
        self.editor.message_unlogged(said_of(&server.1, why, title));
        self.edit_answered(server, from, Err(why.to_owned()));
        self.request_redraw();
    }

    /// Answers `server`'s `workspace/applyEdit` with `outcome`, if the edit was one.
    fn edit_answered(
        &mut self,
        server: &(PathBuf, String),
        from: &EditFrom,
        outcome: Result<(), String>,
    ) {
        let index = self.servers.find(&server.0, &server.1);
        if let (EditFrom::Server(token), Some(index)) = (from, index) {
            self.servers.running[index]
                .server
                .answer_apply_edit(*token, outcome);
        }
    }

    /// Server `index`'s `workspace/applyEdit` while a command it ran waited (Q5): applied as the
    /// command's action's edit, named by its label, else that action's title.
    pub(super) fn server_applies_edit(&mut self, index: usize, asked: ApplyEdit) {
        let running = &self.servers.running[index];
        let server = (running.root.clone(), running.entry.name.clone());
        let title = asked.label.unwrap_or_default();
        let from = EditFrom::Server(asked.token);
        match asked.edit {
            Ok(edit) => self.apply_edit_of(&server, &title, edit, from),
            Err(why) => self.refuse_edit(&server, &title, &from, &why),
        }
    }
}

#[cfg(test)]
mod tests;
