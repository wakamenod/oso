//! A server's edit that touches files whose text the server does not have, and its own
//! `workspace/applyEdit` (ADR 0138, step 13b), on the UI thread: such a file is the buffer that
//! holds it, unmodified, or opens through the loader, unshown, after the person's yes (Q3, Q4),
//! and the edit applies once each is loaded and judged.
//!
//! Must not depend on: the window system or the drawing; nothing here waits on a load or a
//! server.
//!
//! - A file outside the server's root, or whose name may hold secrets, refuses the whole edit
//!   before anything opens; one the loader then judges may hold secrets refuses it once loaded.
//!   A buffer that holds the file with changes the server has not seen refuses it too, as the
//!   server's positions are of the file.
//! - The question names the counts and the action's title: `n`, or the question closing, leaves
//!   everything as it was. The buffers opened are left unsaved, as Eglot leaves them.
//! - A server's `workspace/applyEdit` goes the same way, and its answer goes back once the edit
//!   is applied or refused.
//! - At most one edit waits; a newer one refuses the older.

use std::path::{Path, PathBuf};

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::BufferEdit;
use oso_core::minibuffer::PromptId;
use oso_lsp::server::{ApplyEdit, ServerEdit, UnsentEdit};

use super::super::App;

/// An edit that waits for the person's yes, or for the files it opened to load: its server, by
/// its root and entry, its title, the edits placed so far, the files still to place, what then
/// runs, and the token of the server's `workspace/applyEdit` it answers, if it is one. `Debug`
/// gives counts only: an edit's text may quote what the server read.
pub(super) struct Waiting {
    server: (PathBuf, String),
    title: String,
    parts: Vec<BufferEdit>,
    files: Vec<(Option<BufferId>, UnsentEdit)>,
    question: Option<PromptId>,
    then: Option<super::actions::Then>,
    token: Option<u64>,
}

impl std::fmt::Debug for Waiting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Waiting")
            .field("parts", &self.parts.len())
            .field("files", &self.files.len())
            .field("question", &self.question)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

impl App {
    /// Applies `edit`, `server`'s edit titled `title`, whole or not at all: at once if every
    /// file it touches is in a buffer, else after the question and the loads. `then` runs once
    /// it applied; `token` names the server's `workspace/applyEdit` the outcome answers.
    pub(super) fn apply_edit_of(
        &mut self,
        server: &(PathBuf, String),
        title: &str,
        edit: ServerEdit,
        (then, token): (Option<super::actions::Then>, Option<u64>),
    ) {
        if let Some(older) = self.servers.waiting.take() {
            self.edit_done(
                older.server.clone(),
                older.token,
                Err("a newer edit came".to_owned()),
            );
        }
        let name = server.1.clone();
        let Some(index) = self.servers.find(&server.0, &name) else {
            self.edit_done(server.clone(), token, Err("its server ended".to_owned()));
            return;
        };
        let running = &self.servers.running[index];
        let root = super::read_back(&running.root).unwrap_or_else(|| running.root.clone());
        let mut parts = Vec::with_capacity(edit.open.len());
        for open in edit.open {
            let Some(buffer) = running.buffer_of(open.document) else {
                return self.refuse_edit(server, title, token, "a buffer it edits is gone");
            };
            parts.push(BufferEdit {
                buffer,
                revision: open.revision,
                edits: open.edits,
            });
        }
        let mut files = Vec::with_capacity(edit.unsent.len());
        for unsent in edit.unsent {
            if !unsent.path.starts_with(&root) {
                return self.refuse_edit(
                    server,
                    title,
                    token,
                    "it edits a file outside the project",
                );
            }
            if Buffer::name_may_hold_secrets(&unsent.path) {
                return self.refuse_edit(
                    server,
                    title,
                    token,
                    "it edits a file that may hold secrets",
                );
            }
            let held = self.buffer_holding(&unsent.path);
            if held.is_some_and(|id| self.editor.buffer(id).is_some_and(Buffer::is_modified)) {
                return self.refuse_edit(
                    server,
                    title,
                    token,
                    "a buffer it edits has changes the server has not seen",
                );
            }
            files.push((held, unsent));
        }
        let unopened = files.iter().filter(|(held, _)| held.is_none()).count();
        let mut waiting = Waiting {
            server: server.clone(),
            title: title.to_owned(),
            parts,
            files,
            question: None,
            then,
            token,
        };
        if unopened == 0 {
            return self.finish_edit(waiting);
        }
        // Files to open: the person says yes first (Q4).
        let count = waiting.parts.len() + waiting.files.len();
        let question =
            format!("{name}: apply to {count} files, {unopened} not open? \u{201c}{title}\u{201d}");
        match self.editor.ask(question, "lsp-code-action-confirm") {
            Ok(prompt) => {
                waiting.question = Some(prompt);
                self.servers.waiting = Some(waiting);
            }
            Err(_) => self.refuse_edit(server, title, token, "a question is open already"),
        }
        self.request_redraw();
    }

    /// `lsp-code-action-confirm`, the question's `y`: opens the files the waiting edit needs.
    pub(super) fn confirm_server_edit(&mut self) {
        let Some(mut waiting) = self.servers.waiting.take() else {
            return;
        };
        waiting.question = None;
        for (held, unsent) in &mut waiting.files {
            if held.is_none() {
                *held = self.visit(unsent.path.clone(), crate::loader::UNASKED);
            }
        }
        if waiting.files.iter().any(|(held, _)| held.is_none()) {
            let (server, title, token) =
                (waiting.server.clone(), waiting.title.clone(), waiting.token);
            return self.refuse_edit(&server, &title, token, "a file it edits could not open");
        }
        self.servers.waiting = Some(waiting);
        self.edit_moves();
    }

    /// After each event: the waiting edit applies once its files are loaded, or goes once its
    /// question closed with no yes.
    pub(super) fn edit_moves(&mut self) {
        let Some(waiting) = &self.servers.waiting else {
            return;
        };
        if let Some(question) = waiting.question {
            if !self
                .editor
                .prompts()
                .iter()
                .any(|prompt| prompt.id() == question)
            {
                let waiting = self.servers.waiting.take();
                if let Some(waiting) = waiting {
                    let (server, title, token) = (waiting.server, waiting.title, waiting.token);
                    self.refuse_edit(&server, &title, token, "not applied");
                }
            }
            return;
        }
        let loaded = waiting.files.iter().all(|(held, _)| {
            held.and_then(|id| self.editor.buffer(id))
                .is_none_or(|buffer| buffer.loading().is_none())
        });
        if loaded && let Some(waiting) = self.servers.waiting.take() {
            self.finish_edit(waiting);
        }
    }

    /// Places the waiting edit's files in their buffers' texts and applies it all.
    fn finish_edit(&mut self, waiting: Waiting) {
        let Waiting {
            server,
            title,
            mut parts,
            files,
            then,
            token,
            ..
        } = waiting;
        for (held, unsent) in files {
            let Some(buffer) = held.and_then(|id| self.editor.buffer(id).map(|b| (id, b))) else {
                return self.refuse_edit(&server, &title, token, "a file it edits could not open");
            };
            let (id, buffer) = buffer;
            if buffer.is_secret() {
                return self.refuse_edit(
                    &server,
                    &title,
                    token,
                    "it edits a file that may hold secrets",
                );
            }
            let text = buffer.text();
            let edits = (unsent.edits.into_iter())
                .map(|(start, end, new)| {
                    (
                        oso_lsp::position::to_range(text, start, end, unsent.encoding),
                        new,
                    )
                })
                .collect();
            parts.push(BufferEdit {
                buffer: id,
                revision: buffer.revision(),
                edits,
            });
        }
        let name = server.1.clone();
        let outcome = match self.editor.apply_server_edit(parts) {
            Ok(applied) => {
                let edits = if applied.edits == 1 { "edit" } else { "edits" };
                let buffers = if applied.buffers == 1 {
                    "buffer"
                } else {
                    "buffers"
                };
                self.editor.message_unlogged(format!(
                    "{name}: {} {edits} in {} {buffers}: \u{201c}{title}\u{201d}",
                    applied.edits, applied.buffers
                ));
                Ok(())
            }
            Err(why) => {
                self.editor
                    .message_unlogged(format!("{name}: {why}: \u{201c}{title}\u{201d}"));
                Err(why.to_owned())
            }
        };
        let applied = outcome.is_ok();
        self.edit_done(server, token, outcome);
        if applied && let Some(then) = then {
            self.run_then(then);
        }
        self.request_redraw();
    }

    /// Says why the edit titled `title` changes nothing, and answers its server if it asked.
    fn refuse_edit(
        &mut self,
        server: &(PathBuf, String),
        title: &str,
        token: Option<u64>,
        why: &str,
    ) {
        let name = &server.1;
        self.editor
            .message_unlogged(format!("{name}: {why}: \u{201c}{title}\u{201d}"));
        self.edit_done(server.clone(), token, Err(why.to_owned()));
        self.request_redraw();
    }

    /// Answers `server`'s `workspace/applyEdit` of `token` with `outcome`, if the edit was one.
    fn edit_done(
        &mut self,
        server: (PathBuf, String),
        token: Option<u64>,
        outcome: Result<(), String>,
    ) {
        if let (Some(token), Some(index)) = (token, self.servers.find(&server.0, &server.1)) {
            self.servers.running[index]
                .server
                .answer_apply_edit(token, outcome);
        }
    }

    /// Server `index`'s `workspace/applyEdit` while a command it ran waited (Q5): applied as the
    /// command's action's edit, its label or the action's title naming it.
    pub(super) fn server_applies_edit(&mut self, index: usize, asked: ApplyEdit) {
        let running = &self.servers.running[index];
        let server = (running.root.clone(), running.entry.name.clone());
        let title = (asked.label)
            .or_else(|| self.servers.running_command.clone())
            .unwrap_or_default();
        match asked.edit {
            Ok(edit) => self.apply_edit_of(&server, &title, edit, (None, Some(asked.token))),
            Err(why) => self.refuse_edit(&server, &title, Some(asked.token), &why),
        }
    }

    /// The buffer that visits `path`, if one does.
    fn buffer_holding(&self, path: &Path) -> Option<BufferId> {
        (self.editor.buffers())
            .find(|(_, buffer)| buffer.path() == Some(path))
            .map(|(id, _)| id)
    }
}
