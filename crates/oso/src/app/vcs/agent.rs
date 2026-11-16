//! Asking the worktree's agent session (ADR 0127, step 6a): a draft of a commit's message
//! (`C-c C-d`), given back with the session tool `draft_set`, and a fix for a commit that
//! failed (`C-c C-f` in its message, `F` in the log).
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: the last subjects are read on the repository's run thread.
//!
//! - A request goes as the person's paste into the session's terminal, through the one
//!   delivery path, not submitted: the person reads it in the session's input box and presses
//!   Enter. The agent sees nothing else: the log stays withheld from the channel (Q1).
//! - The session is the hosted one that runs in the repository's root, the one whose terminal
//!   was shown last if several do.
//! - A draft request names an id of 32 random bits, kept with the session it was pasted into;
//!   `super::draft` answers `draft_set` with it, and no file that pastes takes a tool's call.

use std::path::Path;
use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::project::same_place;

use super::super::App;
use super::runs::RUNNING;
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::sessions::worker::SessionKey;
use crate::vcs::agent::{draft_request, fix_request, one_line as escaped};

/// What a request says with no session to ask.
const NO_SESSION: &str = "No agent session runs in this worktree: C-x a c starts one";

/// A draft request sent for a message buffer, and the session it was pasted into, which alone
/// may answer it.
pub(in crate::app) struct Asked {
    pub(super) id: String,
    pub(super) at: Instant,
    pub(super) session: SessionKey,
}

impl App {
    /// The hosted session that runs in `root` with a terminal, the one shown last first.
    pub(super) fn worktree_session(&self, root: &Path) -> Option<SessionKey> {
        let order = self.editor.buffers_by_recency();
        let table = self.hosting.table();
        table
            .iter()
            .filter(|session| !session.archived && session.runs())
            .filter(|session| same_place(&session.directory, root))
            .filter_map(|session| {
                let buffer = session
                    .buffer
                    .filter(|b| self.editor.buffer(*b).is_some())?;
                let at = order
                    .iter()
                    .position(|b| *b == buffer)
                    .unwrap_or(usize::MAX);
                Some((at, session.key))
            })
            .min_by_key(|(at, _)| *at)
            .map(|(_, key)| key)
    }

    /// Asks the worktree's session for a draft of the message `buffer` (`C-c C-d`): the last
    /// subjects are read first, on the run thread.
    pub(super) fn draft_message(&mut self, buffer: BufferId) {
        let Some(at) = self.draft_index(buffer) else {
            return self
                .editor
                .message("This message's commit is gone: C-x k drops it");
        };
        let project = self.gits.commits.drafts[at].project;
        let Some(index) = self.repo_of(project) else {
            return self.editor.message(RESTRICTED);
        };
        let repo = &self.gits.commits.repos[index];
        if repo.root.revoked() || !self.editor.project_trusted(project) {
            return self.editor.message(RESTRICTED);
        }
        if repo.run.is_some() || repo.subjects_for.is_some() {
            return self.editor.message(RUNNING);
        }
        let root = repo.root.path().to_path_buf();
        if self.worktree_session(&root).is_none() {
            return self.editor.message(NO_SESSION);
        }
        if self.ask_runner(index, crate::vcs::run::RunRequest::Subjects) {
            self.gits.commits.repos[index].subjects_for = Some(buffer);
            self.editor.echo("Reading the last subjects…");
        }
    }

    /// The repository at `index` read its last `subjects`: the draft request that waited for
    /// them goes to the session.
    pub(super) fn subjects_read(
        &mut self,
        index: usize,
        subjects: &[String],
        id: Result<String, String>,
    ) {
        let Some(buffer) = self.gits.commits.repos[index].subjects_for.take() else {
            return;
        };
        self.editor.clear_echo_saying("Reading the last subjects…");
        let Some(at) = self.draft_index(buffer) else {
            return;
        };
        let root = self.gits.commits.repos[index].root.path().to_path_buf();
        let Some(key) = self.worktree_session(&root) else {
            return self.editor.message(NO_SESSION);
        };
        let id = match id {
            Ok(id) => id,
            Err(why) => return self.editor.message(why),
        };
        let worktree = escaped(&root.display().to_string(), 400);
        let kind = self.gits.commits.drafts[at].kind;
        let text = draft_request(kind, &worktree, subjects, &id);
        if self.ask_session(key, text, "a draft; it comes back to this message", "") {
            self.gits.commits.drafts[at].asked = Some(Asked {
                id,
                at: Instant::now(),
                session: key,
            });
        }
    }

    /// Asks the worktree's session to fix what made the last commit fail, from `buffer`: that
    /// commit's message, or the repository's log while the message is open (Q4).
    pub(super) fn fix_with_agent(&mut self, buffer: BufferId) {
        let project = match self.draft_index(buffer) {
            Some(at) => Some(self.gits.commits.drafts[at].project),
            None => (self.gits.commits.repos.iter())
                .find(|repo| repo.log == Some(buffer))
                .map(|repo| repo.project),
        };
        let Some(index) = project.and_then(|project| self.repo_of(project)) else {
            return self.editor.message("No commit failed here");
        };
        let repo = &self.gits.commits.repos[index];
        if repo.root.revoked() || !self.editor.project_trusted(repo.project) {
            return self.editor.message(RESTRICTED);
        }
        if repo.run.is_some() {
            return self.editor.message(RUNNING);
        }
        let failed = repo.failed.as_ref().filter(|failed| {
            self.draft_index(failed.draft).is_some()
                && (self.draft_index(buffer).is_none() || failed.draft == buffer)
        });
        let Some(failed) = failed else {
            return self
                .editor
                .message("No failed commit with its message open here");
        };
        let root = repo.root.path().to_path_buf();
        let Some(key) = self.worktree_session(&root) else {
            return self.editor.message(NO_SESSION);
        };
        let worktree = escaped(&root.display().to_string(), 400);
        let (text, dropped) = fix_request(&worktree, &failed.hooks, &failed.output);
        let note = match dropped {
            0 => String::new(),
            1 => "; 1 line that may hold a secret was left out".to_owned(),
            n => format!("; {n} lines that may hold secrets were left out"),
        };
        let then = "a fix; C-c C-c commits again once it is done";
        let _ = self.ask_session(key, text, then, &note);
    }

    /// Pastes `text` into session `key`, saying what comes of it and `note`, or why nothing was
    /// pasted; whether it was.
    fn ask_session(&mut self, key: SessionKey, text: String, then: &str, note: &str) -> bool {
        let name = (self.hosting.table().get(key)).map_or_else(String::new, |s| s.name.clone());
        let lines = text.lines().count();
        match self.send_to_session(key, text) {
            Ok(()) => {
                self.editor.message(format!(
                    "Asked {} for {then}, in {lines} lines: read them, then press Enter there{note}",
                    one_line(&name, 40)
                ));
                true
            }
            Err(why) => {
                self.editor.message(why);
                false
            }
        }
    }
}
