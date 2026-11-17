//! Committing in the app (ADR 0127, step 1b): the commit menu's heads, the message buffers and
//! the ring of messages. The runs themselves are [`super::runs`]'s.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it.
//!
//! - A commit's message is a buffer of the person's, `*commit: NAME*`, in `GitCommit` mode,
//!   highlighted here as it changes. The agent channel reads it, but its keys are the
//!   person's and the channel's commands change nothing in it (Q6); only the session tool
//!   `draft_set` puts a draft in, for the request the person pasted (step 6a, `super::draft`).
//! - What git would refuse anyway is refused before it runs, so that a hook is never blamed for
//!   it: nothing staged, an amend before the first commit, an empty message, a second message
//!   for the repository, and a run while another goes.
//! - The ring keeps the last 20 messages committed or cancelled, for `M-p` and `M-n`.

use std::sync::Arc;

use oso_core::buffer::{Buffer, BufferId, Mode, Replacement};
use oso_core::editor::{CommitKind, CommitSwitches, GitRequest};
use oso_core::project::ProjectId;
use oso_core::window::WindowId;

use super::super::App;
use super::runs::{Pending, RUNNING, Repo, Running};
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::Built;
use crate::vcs::commit::{self, Ring, Staged};
use crate::vcs::layout::Section;
use crate::vcs::run::RunRequest;

/// Why a message's text is not taken.
pub(super) const TOO_LONG: &str = "The message is longer than git is given: 1 MiB";

/// The repositories git runs in, the messages being written, and the ring.
#[derive(Default)]
pub(in crate::app) struct Commits {
    pub(super) repos: Vec<Repo>,
    pub(super) drafts: Vec<Draft>,
    ring: Ring,
    pub(super) serial: u64,
    /// The discards asked about, and the lists of what they kept (step 1c).
    pub(super) discards: super::discard::Discards,
}

/// A commit's message being written.
pub(super) struct Draft {
    pub(super) buffer: BufferId,
    pub(super) project: ProjectId,
    pub(super) kind: CommitKind,
    switches: CommitSwitches,
    /// Which message of the ring it shows, `None` for the person's own.
    pub(super) ring_at: Option<usize>,
    /// The revision its highlights were made for.
    lit: Option<u64>,
    /// The newest draft request sent for it, which alone `draft_set` may answer (step 6a).
    pub(super) asked: Option<super::agent::Asked>,
    /// The commit a squash is for (step 2).
    target: Option<String>,
}

impl App {
    /// Carries out what the commands of committing and of the log asked.
    pub(super) fn git_commit_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::Commit {
                buffer,
                kind,
                switches,
                line,
            } => self.start_commit(buffer, (kind, switches), line),
            GitRequest::CommitFinish { buffer } => self.finish_commit(buffer),
            GitRequest::CommitCancel { buffer } => self.cancel_commit(buffer),
            GitRequest::CommitRing { buffer, older } => self.walk_ring(buffer, older),
            GitRequest::ShowLog { buffer } => self.show_git_log(buffer),
            GitRequest::Stop { buffer } => self.stop_git(buffer),
            GitRequest::Discard {
                buffer,
                lines,
                confirmed,
            } => self.discard_git(buffer, &lines, confirmed),
            GitRequest::RestoreDiscarded {
                buffer,
                line,
                confirmed,
            } => self.restore_discarded(buffer, line, confirmed),
            GitRequest::ShowDiscarded { buffer } => self.show_discarded(buffer),
            GitRequest::DraftMessage { buffer } => self.draft_message(buffer),
            GitRequest::FixWithAgent { buffer } => self.fix_with_agent(buffer),
            // The others are not a commit's (`requests::concern`).
            _ => {}
        }
    }

    /// The status `buffer`'s project, root and table, if its project is trusted; else says so.
    pub(super) fn trusted_status(
        &mut self,
        buffer: BufferId,
    ) -> Option<(ProjectId, TrustedRoot, Arc<Built>)> {
        let index = self.git_index(buffer)?;
        let open = &self.gits.open[index];
        if open.revoked || !self.editor.project_trusted(open.project) {
            self.editor.message(RESTRICTED);
            return None;
        }
        Some((open.project, open.root.clone(), Arc::clone(&open.built)))
    }

    fn start_commit(
        &mut self,
        status: BufferId,
        (kind, switches): (CommitKind, CommitSwitches),
        line: usize,
    ) {
        let Some((project, root, built)) = self.trusted_status(status) else {
            return;
        };
        if self.refused_while_stopped(&built) {
            return;
        }
        let target = if commit::needs_target(kind) {
            match built.commit_at(line) {
                Some((Section::Unpushed | Section::Recent, found)) => Some(found.id.clone()),
                Some(_) => {
                    return self.editor.message(
                        "That commit is not under HEAD: f and s act on one in Unpushed or \
                         Recent commits",
                    );
                }
                None => {
                    return self
                        .editor
                        .message("f and s act on a commit in Unpushed or Recent commits");
                }
            }
        } else {
            None
        };
        let drafts = &self.gits.commits.drafts;
        if let Some(draft) = drafts.iter().find(|draft| draft.project == project) {
            let buffer = draft.buffer;
            self.editor.show(buffer);
            return self
                .editor
                .message("A commit's message is open here: C-c C-c commits, C-c C-k drops it");
        }
        let index = self.git_repo(project, &root);
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        if kind != CommitKind::New && built.branch.oid.is_none() {
            return self
                .editor
                .message("Nothing to amend: the repository has no commit yet");
        }
        let nothing = built.files(Section::Staged).is_empty();
        let needs_staged = matches!(
            kind,
            CommitKind::New | CommitKind::Extend | CommitKind::Fixup | CommitKind::Squash
        );
        if needs_staged && nothing && !switches.allow_empty {
            return self.editor.message(
                "Nothing staged: s stages a change, or -e in the commit menu commits nothing",
            );
        }
        match kind {
            CommitKind::Extend | CommitKind::Fixup => {
                self.run_commit(index, (kind, switches), None, None, target);
            }
            CommitKind::New | CommitKind::Squash => {
                self.open_draft(project, kind, switches, "");
                if let Some(draft) = self.gits.commits.drafts.last_mut() {
                    draft.target = target;
                }
            }
            CommitKind::Amend | CommitKind::Reword => {
                if self.ask_runner(index, RunRequest::Message) {
                    self.gits.commits.repos[index].reading = Some(Pending { kind, switches });
                    self.editor.echo("Reading the last commit's message…");
                }
            }
        }
    }

    /// Shows a new message buffer for the commit `kind` of `project`, starting from `message`.
    pub(super) fn open_draft(
        &mut self,
        project: ProjectId,
        kind: CommitKind,
        switches: CommitSwitches,
        message: &str,
    ) {
        let built = (self.gits.open.iter())
            .find(|open| open.project == project)
            .map(|open| Arc::clone(&open.built))
            .unwrap_or_default();
        let branch = (built.branch.head.as_deref()).map(String::from_utf8_lossy);
        let files = built.files(Section::Staged);
        let staged: Vec<Staged<'_>> = (files.iter().take(commit::MAX_LISTED))
            .map(|file| Staged {
                letter: char::from(file.letters[0]),
                path: &file.path,
            })
            .collect();
        let text = commit::template(message, branch.as_deref(), &staged, files.len(), kind);
        let name = self.unique_name(&format!("*commit: {}*", self.project_name(project)));
        let mut buffer = Buffer::from_text(name, &text);
        buffer.set_mode(Mode::GitCommit);
        let id = self.editor.add_buffer(buffer);
        self.stand_in_root(id, project);
        self.editor.show(id);
        self.gits.commits.drafts.push(Draft {
            buffer: id,
            project,
            kind,
            switches,
            ring_at: None,
            lit: None,
            asked: None,
            target: None,
        });
        self.highlight_drafts();
        self.editor
            .echo("C-c C-c commits, C-c C-k cancels, M-p brings back an earlier message");
        self.request_redraw();
    }

    fn finish_commit(&mut self, buffer: BufferId) {
        let Some(at) = self.draft_index(buffer) else {
            return self
                .editor
                .message("This message's commit is gone: C-x k drops it");
        };
        let Some(text) = self.message_text(buffer) else {
            return self.editor.message(TOO_LONG);
        };
        let message = commit::message_of(&text);
        // A squash's message may be empty: git's `squash!` line is its subject.
        if message.is_empty() && self.gits.commits.drafts[at].kind != CommitKind::Squash {
            return self
                .editor
                .message("The message is empty: write one, or C-c C-k cancels");
        }
        let Draft {
            project,
            kind,
            switches,
            ..
        } = self.gits.commits.drafts[at];
        let target = self.gits.commits.drafts[at].target.clone();
        let Some(index) = self.repo_of(project) else {
            return self.editor.message(RESTRICTED);
        };
        if self.gits.commits.repos[index].run.is_some() {
            return self.editor.message(RUNNING);
        }
        self.run_commit(index, (kind, switches), Some(message), Some(buffer), target);
    }

    fn cancel_commit(&mut self, buffer: BufferId) {
        let Some(at) = self.draft_index(buffer) else {
            return;
        };
        let project = self.gits.commits.drafts[at].project;
        let runs = (self.repo_of(project))
            .and_then(|index| self.gits.commits.repos[index].run.as_ref())
            .is_some_and(|run| run.draft == Some(buffer));
        if runs {
            return self
                .editor
                .message("Its commit runs: C-g in the status or the log stops it");
        }
        // A text longer than a message is not kept.
        if let Some(text) = self.message_text(buffer) {
            self.gits.commits.ring.keep(commit::message_of(&text));
        }
        self.drop_draft(buffer);
        self.editor
            .message("Commit cancelled; M-p in the next message brings it back");
        self.request_redraw();
    }

    /// Puts the ring's message before (`older`) or after the one `buffer` shows in its place,
    /// keeping the `#` lines.
    fn walk_ring(&mut self, buffer: BufferId, older: bool) {
        let Some(at) = self.draft_index(buffer) else {
            return;
        };
        let ring = &self.gits.commits.ring;
        let next = ring.step(self.gits.commits.drafts[at].ring_at, older);
        let Some((next, kept)) = next.and_then(|n| Some((n, ring.get(n)?.to_owned()))) else {
            let which = if older { "older" } else { "newer" };
            return self.editor.message(format!("No {which} message kept"));
        };
        let Some(text) = self.message_text(buffer) else {
            return self.editor.message(TOO_LONG);
        };
        let end = commit::message_end(&text);
        let mut put = kept.trim_end().to_owned();
        put.push_str(if end < text.len() { "\n\n" } else { "\n" });
        let edit = Replacement {
            range: 0..end,
            text: &put,
        };
        if let Err(e) = self.editor.apply_edits(buffer, &[edit]) {
            return self.editor.message(format!("Cannot put it in: {e}"));
        }
        let count = self.gits.commits.ring.len();
        self.gits.commits.drafts[at].ring_at = Some(next);
        self.editor
            .message(format!("Message {} of {count} kept", next + 1));
        self.highlight_drafts();
        self.request_redraw();
    }

    /// What a commit that went in leaves: its message in the ring, no message buffer, and what
    /// the echo area says.
    pub(super) fn commit_done(&mut self, run: &Running) -> String {
        if let Some(draft) = run.draft {
            self.drop_draft(draft);
        }
        let fold = "git rebase -i --autosquash in a shell folds it in";
        match &run.message {
            _ if run.kind == Some(CommitKind::Fixup) => format!("Made a fixup! commit: {fold}"),
            _ if run.kind == Some(CommitKind::Squash) => format!("Made a squash! commit: {fold}"),
            Some(message) => {
                let summary = one_line(commit::summary(message), 80);
                self.gits.commits.ring.keep(message.clone());
                format!("Committed: {summary}")
            }
            None => "Extended the last commit".to_owned(),
        }
    }

    /// After commands and events: forgets message buffers that went, looks at the runs, and
    /// highlights the messages that changed.
    pub(super) fn sync_commits(&mut self) {
        let editor = &self.editor;
        let gone: Vec<BufferId> = (self.gits.commits.drafts.iter())
            .filter(|draft| editor.buffer(draft.buffer).is_none())
            .map(|draft| draft.buffer)
            .collect();
        (self.gits.commits.drafts).retain(|draft| !gone.contains(&draft.buffer));
        self.sync_runs(&gone);
        self.highlight_drafts();
    }

    /// Highlights each message buffer whose text changed since, as git-commit mode does, or
    /// drops its highlights while highlighting is off.
    pub(super) fn highlight_drafts(&mut self) {
        let on = self.editor.font_lock();
        for at in 0..self.gits.commits.drafts.len() {
            let id = self.gits.commits.drafts[at].buffer;
            let Some(shown) = self.editor.buffer(id) else {
                continue;
            };
            let revision = shown.revision();
            let done = self.gits.commits.drafts[at].lit == Some(revision);
            if done || shown.mode() != Mode::GitCommit {
                continue;
            }
            let text = shown.text();
            let lit = (on && text.len() <= commit::MAX_HIGHLIGHTED)
                .then(|| commit::highlights(&text.to_string()));
            if let Some(buffer) = self.editor.buffer_mut(id) {
                match lit {
                    Some(lit) => {
                        buffer.take_highlights(revision, lit);
                    }
                    None => buffer.drop_highlights(),
                }
            }
            self.gits.commits.drafts[at].lit = Some(revision);
        }
    }

    /// Forgets the message buffer `buffer` and kills it, each window that showed it showing its
    /// repository's status again, as magit goes back to the status.
    fn drop_draft(&mut self, buffer: BufferId) {
        let project = (self.draft_index(buffer)).map(|at| self.gits.commits.drafts[at].project);
        self.gits
            .commits
            .drafts
            .retain(|draft| draft.buffer != buffer);
        self.close_message(buffer, project);
    }

    /// Kills a message's `buffer`, a commit's or a pull request's, showing `project`'s status in
    /// each window that showed it.
    pub(super) fn close_message(&mut self, buffer: BufferId, project: Option<ProjectId>) {
        if let Some(status) = project.and_then(|project| self.editor.git_status_buffer(project)) {
            let showing: Vec<WindowId> = (self.editor.windows())
                .filter(|window| window.buffer() == buffer)
                .map(|window| window.id())
                .collect();
            for window in showing {
                self.editor.show_in(window, status);
            }
        }
        self.editor.kill_buffer(buffer);
    }

    /// The text of `buffer`, unless it is gone or longer than git is given a message: its length
    /// is looked at before anything is copied.
    pub(super) fn message_text(&self, buffer: BufferId) -> Option<String> {
        let shown = self.editor.buffer(buffer)?;
        (shown.text().len() <= commit::MAX_MESSAGE).then(|| shown.text().to_string())
    }

    pub(super) fn draft_index(&self, buffer: BufferId) -> Option<usize> {
        (self.gits.commits.drafts.iter()).position(|draft| draft.buffer == buffer)
    }
}
