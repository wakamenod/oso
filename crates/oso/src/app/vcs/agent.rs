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
//! - A draft request names an id of 32 random bits. `draft_set` answers only the newest request
//!   of a message buffer (Q3), within [`ASKED_FOR`], once, from a session in that buffer's
//!   worktree; it writes the buffer and commits nothing.

use std::path::Path;
use std::time::{Duration, Instant};

use oso_core::buffer::{BufferId, Replacement};
use oso_core::project::same_place;
use serde_json::{Value, json};

use super::super::App;
use super::commit::TOO_LONG;
use super::runs::RUNNING;
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::sessions::worker::SessionKey;
use crate::vcs::agent::{cleaned, draft_request, fix_request, with_draft};

/// How long a draft request may be answered.
pub(super) const ASKED_FOR: Duration = Duration::from_secs(15 * 60);
/// What a request says with no session to ask.
const NO_SESSION: &str = "No agent session runs in this worktree: C-x a c starts one";

/// A draft request sent for a message buffer.
pub(in crate::app) struct Asked {
    id: String,
    at: Instant,
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
    pub(super) fn subjects_read(&mut self, index: usize, subjects: &[String]) {
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
        let id = match request_id() {
            Ok(id) => id,
            Err(e) => {
                return self
                    .editor
                    .message(format!("Cannot make a request id: {e}"));
            }
        };
        let worktree = one_line(&root.display().to_string(), 400);
        let kind = self.gits.commits.drafts[at].kind;
        let text = draft_request(kind, &worktree, subjects, &id);
        if self.ask_session(key, text, "a draft; it comes back to this message") {
            self.gits.commits.drafts[at].asked = Some(Asked {
                id,
                at: Instant::now(),
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
        let worktree = one_line(&root.display().to_string(), 400);
        let text = fix_request(&worktree, &failed.hooks, &failed.output);
        let Some(key) = self.worktree_session(&root) else {
            return self.editor.message(NO_SESSION);
        };
        let _ = self.ask_session(key, text, "a fix; C-c C-c commits again once it is done");
    }

    /// Pastes `text` into session `key`, saying what comes of it, or why nothing was pasted;
    /// whether it was.
    fn ask_session(&mut self, key: SessionKey, text: String, then: &str) -> bool {
        let name = (self.hosting.table().get(key)).map_or_else(String::new, |s| s.name.clone());
        match self.send_to_session(key, text) {
            Ok(()) => {
                self.editor.message(format!(
                    "Asked {} for {then}: press Enter there",
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

    /// The session tool `draft_set` from the session `name` that runs in `runs_in`: puts
    /// `text` in the message whose newest request named `id`, keeping the person's own text
    /// below it as `#` lines (Q2).
    pub(in crate::app) fn draft_set(
        &mut self,
        runs_in: &Path,
        name: &str,
        id: &str,
        text: &str,
    ) -> Result<Value, String> {
        let draft = cleaned(text)?;
        let at = (self.gits.commits.drafts.iter())
            .position(|draft| draft.asked.as_ref().is_some_and(|asked| asked.id == id))
            .ok_or(
                "no commit message waits for a draft with that id: a newer request replaced it, it was answered, or the message is gone",
            )?;
        let (buffer, project) = {
            let draft = &self.gits.commits.drafts[at];
            (draft.buffer, draft.project)
        };
        let index = self.repo_of(project).ok_or(RESTRICTED)?;
        let repo = &self.gits.commits.repos[index];
        let found = Found {
            same_worktree: same_place(repo.root.path(), runs_in),
            trusted: !repo.root.revoked() && self.editor.project_trusted(project),
            runs: repo.run.is_some(),
            age: (self.gits.commits.drafts[at].asked.as_ref()).map(|asked| asked.at.elapsed()),
        };
        if let Err(why) = found.check() {
            // An answer from the right worktree uses the request up, whatever it says.
            if found.same_worktree {
                self.gits.commits.drafts[at].asked = None;
            }
            return Err(why.into());
        }
        self.gits.commits.drafts[at].asked = None;
        let now = self.message_text(buffer).ok_or(TOO_LONG)?;
        let (end, put) = with_draft(&now, &draft, name);
        let edit = Replacement {
            range: 0..end,
            text: &put,
        };
        self.editor
            .apply_edits(buffer, &[edit])
            .map_err(|e| format!("cannot put it in: {e}"))?;
        self.gits.commits.drafts[at].ring_at = None;
        self.highlight_drafts();
        let shown = (self.editor.buffer(buffer)).map_or_else(String::new, |b| b.name().to_owned());
        self.editor.message(format!(
            "A draft from {} is in {}: C-c C-c commits it, C-/ takes it back",
            one_line(name, 40),
            one_line(&shown, 80)
        ));
        self.request_redraw();
        Ok(json!({ "buffer": shown, "committed": false }))
    }
}

/// What `draft_set` found of the message its id named.
#[derive(Clone, Copy)]
struct Found {
    /// Whether the caller runs in the message's worktree.
    same_worktree: bool,
    trusted: bool,
    /// Whether a commit runs in the repository.
    runs: bool,
    /// How long ago the request was sent.
    age: Option<Duration>,
}

impl Found {
    /// Why the draft may not be put in, if it may not.
    fn check(self) -> Result<(), &'static str> {
        if !self.same_worktree {
            return Err("that commit message is another worktree's");
        }
        if !self.trusted {
            return Err(RESTRICTED);
        }
        if self.runs {
            return Err("its commit runs now; nothing was changed");
        }
        match self.age {
            Some(age) if age <= ASKED_FOR => Ok(()),
            _ => Err("that request is older than 15 minutes: the person asks again"),
        }
    }
}

/// A request id: 32 random bits, as 8 hex digits.
fn request_id() -> std::io::Result<String> {
    let mut bytes = [0u8; 4];
    oso_platform::random::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests;
