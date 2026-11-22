//! The index's lock in the app (ADR 0127, "A stale index.lock, as amended"): what a stopped or
//! failed run noted, what the echo area says of it, and `M-x git-remove-stale-lock`'s question.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here looks at the disk: the
//! repository's run thread looks at the lock again and takes it away.
//!
//! - Only a lock oso noted is offered, and the run thread takes it away only if it is still the
//!   same file: a git started elsewhere may own a newer one (the author's choice). With none
//!   noted, the command only says so.
//! - The question names the lock and how long ago it was made, so that the person can judge
//!   whether a git still runs.

use std::time::UNIX_EPOCH;

use oso_core::buffer::BufferId;

use super::super::App;
use super::Ask;
use super::discard::ago;
use super::runs::{MAX_SAID, RUNNING, Running};
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::discarded;
use crate::vcs::lock::{self, Noted};
use crate::vcs::run::RunRequest;

/// What the command says when oso noted no lock.
pub(super) const NONE_NOTED: &str = "oso noted no stale index.lock in this repository: it removes \
     only one a stop left or a run failed on";

/// A repository's noted lock, and the one the open question asks about.
#[derive(Default)]
pub(in crate::app) struct Locks {
    noted: Option<Noted>,
    asked: Option<Noted>,
}

impl App {
    /// A run in the repository at `index` left the lock `noted`, or failed on it.
    pub(super) fn lock_noted(&mut self, index: usize, noted: Noted) {
        let locks = &mut self.gits.commits.repos[index].locks;
        locks.noted = Some(noted);
        locks.asked = None;
    }

    /// What the echo area says of `run`, stopped in the repository at `index`: the lock it
    /// left, if it left one.
    pub(super) fn stopped_said(&self, index: usize, run: &Running) -> String {
        let title = &run.title;
        match &self.gits.commits.repos[index].locks.noted {
            Some(noted) if run.locked => format!("{title} stopped; {}", lock::left(&noted.lock)),
            _ => format!("{title} stopped"),
        }
    }

    /// The status at `status` was stopped and left the lock `noted`, or failed on it, as `said`.
    pub(super) fn status_locked(&mut self, status: usize, noted: Noted, said: &str) {
        let open = &self.gits.open[status];
        let (project, root) = (open.project, open.root.clone());
        let index = self.git_repo(project, &root);
        self.lock_noted(index, noted);
        self.editor.message(one_line(said, MAX_SAID));
    }

    /// `M-x git-remove-stale-lock` in `buffer`: asks about the lock noted in its repository,
    /// or, `confirmed`, has the run thread take away what the question asked about.
    pub(super) fn remove_lock(&mut self, buffer: BufferId, confirmed: bool) {
        let project = (self.editor.git_status_of(buffer))
            .or_else(|| {
                (self.gits.commits.repos.iter())
                    .find(|repo| repo.log == Some(buffer))
                    .map(|repo| repo.project)
            })
            .or_else(|| self.editor.git_graph_of(buffer))
            .or_else(|| self.editor.git_pr_of(buffer))
            .or_else(|| self.editor.buffer(buffer)?.project());
        let Some(project) = project else {
            return self.editor.message("Not in a project");
        };
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        let noted = (self.repo_of(project))
            .and_then(|index| self.gits.commits.repos[index].locks.noted.clone());
        let Some(noted) = noted else {
            return self.editor.message(NONE_NOTED);
        };
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        let index = self.git_repo(project, &root);
        let locks = &mut self.gits.commits.repos[index].locks;
        if confirmed {
            if locks.asked.take().as_ref() != Some(&noted) {
                return self.editor.message(
                    "The lock noted changed since the question: M-x git-remove-stale-lock again",
                );
            }
            let id = noted.seen.id;
            self.ask_runner(index, RunRequest::RemoveLock { id });
            return;
        }
        let question = question(&noted, discarded::now());
        locks.asked = Some(noted);
        // One line: a worktree's lock is named by its whole path.
        self.ask_bound(&question, "git-remove-stale-lock", ["remove", "keep"]);
    }

    /// The run thread took the lock away in the repository at `index`, or said why not, as
    /// `said`; the note stays if `kept`.
    pub(super) fn lock_removed(&mut self, index: usize, said: &str, kept: bool) {
        let project = self.gits.commits.repos[index].project;
        if !kept {
            self.gits.commits.repos[index].locks.noted = None;
        }
        self.editor.message(one_line(said, MAX_SAID));
        if let Some(status) = (self.gits.open.iter()).position(|open| open.project == project) {
            self.git_ask(status, Ask::Read);
        }
    }
}

/// The question about `noted` at `now`, in seconds since the epoch.
fn question(noted: &Noted, now: u64) -> String {
    let made = (noted.seen.made)
        .and_then(|made| made.duration_since(UNIX_EPOCH).ok())
        .map_or_else(
            || "at a time not known".to_owned(),
            |made| ago(made.as_secs(), now),
        );
    let shown = noted.lock.shown();
    format!("Remove {shown}, made {made}? Another git may own it")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::{Duration, UNIX_EPOCH};

    use super::question;
    use crate::vcs::lock::{Lock, Noted};

    #[test]
    fn the_question_names_the_lock_and_its_age() {
        let dir = std::env::temp_dir().join(format!("oso-lock-question-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let lock = Lock::at(&dir.join(".git/index"), &dir);
        std::fs::write(dir.join(".git/index.lock"), "").unwrap();
        let mut seen = lock.look().unwrap();
        seen.made = Some(UNIX_EPOCH + Duration::from_secs(1000));
        let noted = Noted {
            lock,
            seen,
            stopped: true,
        };
        assert_eq!(
            question(&noted, 1000 + 3 * 60),
            format!(
                "Remove {}, made 3 min ago? Another git may own it",
                Path::new(".git").join("index.lock").display()
            )
        );
        seen.made = None;
        let noted = Noted { seen, ..noted };
        assert!(question(&noted, 0).contains("made at a time not known"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
