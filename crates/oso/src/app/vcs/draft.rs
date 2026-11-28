//! The session tool `draft_set` (ADR 0127, step 6a): a draft of a commit's message from the
//! session a request was pasted into, put above the message's `#` lines, the person's own text
//! kept below as `#` lines (Q2).
//!
//! Must not depend on: terminals, pastes or processes: a tool's call writes the message buffer
//! alone, and commits nothing. Requests are pasted by [`super::agent`].
//!
//! - Only the newest request of a message buffer (Q3), from the session it was pasted into, in
//!   that buffer's worktree of a trusted project, while no commit runs there, within
//!   [`ASKED_FOR`], once; never into one that a secret was yanked into.

use std::path::Path;
use std::time::Duration;

use oso_core::buffer::{Buffer, Replacement};
use oso_core::project::same_place;
use serde_json::{Value, json};

use super::super::App;
use super::commit::TOO_LONG;
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::sessions::worker::SessionKey;
use crate::vcs::agent::{cleaned, with_draft};

/// How long a draft request may be answered.
pub(super) const ASKED_FOR: Duration = Duration::from_secs(15 * 60);

impl App {
    /// The session tool `draft_set` from the session `name` that runs in `runs_in`: puts
    /// `text` in the message whose newest request named `id`, keeping the person's own text
    /// below it as `#` lines (Q2).
    pub(in crate::app) fn draft_set(
        &mut self,
        caller: SessionKey,
        runs_in: &Path,
        name: &str,
        id: &str,
        text: &str,
    ) -> Result<Value, String> {
        let draft = cleaned(text)?;
        let at = (self.gits.commits.drafts.iter())
            .position(|asked| asked.asked.as_ref().is_some_and(|asked| asked.id == id))
            .ok_or(
                "no commit message waits for a draft with that id: a newer request replaced it, it was answered, or the message is gone",
            )?;
        let (buffer, project) = {
            let message = &self.gits.commits.drafts[at];
            (message.buffer, message.project)
        };
        let index = self.repo_of(project).ok_or(RESTRICTED)?;
        let repo = &self.gits.commits.repos[index];
        let found = Found {
            same_worktree: same_place(repo.root.path(), runs_in)
                && (self.gits.commits.drafts[at].asked.as_ref())
                    .is_some_and(|asked| asked.session == caller),
            trusted: !repo.root.revoked() && self.editor.project_trusted(project),
            runs: repo.run.is_some(),
            secret: self.editor.buffer(buffer).is_some_and(Buffer::is_secret),
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
        let (end, put) = with_draft(&now, &draft, name).inspect_err(|why| {
            self.editor.message(format!(
                "A draft from {} was refused: {why}",
                one_line(name, 40)
            ));
        })?;
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
    /// Whether a yank of a secret entry turned the message secret: it is withheld from the
    /// agent, which is told nothing of it, not even its size.
    secret: bool,
    /// How long ago the request was sent.
    age: Option<Duration>,
}

impl Found {
    /// Why the draft may not be put in, if it may not.
    fn check(self) -> Result<(), &'static str> {
        if !self.same_worktree {
            return Err("that commit message is another worktree's, or asked another session");
        }
        if !self.trusted {
            return Err(RESTRICTED);
        }
        if self.runs {
            return Err("its commit runs now; nothing was changed");
        }
        if self.secret {
            return Err("that commit message takes no draft now; nothing was changed");
        }
        match self.age {
            Some(age) if age <= ASKED_FOR => Ok(()),
            _ => Err("that request is older than 15 minutes: the person asks again"),
        }
    }
}

#[cfg(test)]
mod tests;
