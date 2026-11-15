//! git's long runs in the app (ADR 0127, step 1b): each repository's run thread, its log,
//! what the thread reports, stopping a run, and what quitting ends.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or waits for
//! it: each repository's run thread, `oso-vcs-run`, does, and reports here.
//!
//! - A run belongs to its repository, not to the status it started from: killing the status
//!   leaves it running, and its log, `*git log: NAME*`, is the repository's. Revoking the
//!   project's trust stops it.
//! - One run at a time in a repository (Q8): a second is refused, naming `$` and `C-g`.
//! - The log is withheld from the agent channel, as a secret buffer is (Q5): a hook may print
//!   anything.
//! - The time a run has taken is said in the echo area once a second, unless something was
//!   said since.
//! - Quitting oso sends each run's group `SIGTERM`, and waits for nothing.

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{CommitKind, CommitSwitches};
use oso_core::project::ProjectId;

use super::super::{App, UserEvent};
use super::Ask;
use crate::commands::review::{MAX_NAME, one_line};
use crate::commands::vcs::RESTRICTED;
use crate::git::{Ender, TrustedRoot};
use crate::vcs::commit;
use crate::vcs::run::{RunEnd, RunReport, RunRequest, Runner};

/// Why a second run in a repository is refused (ADR 0127, step 1b, Q8).
pub(super) const RUNNING: &str =
    "git runs in this repository already: $ in its status shows it, C-g stops it";
/// The most characters of what git said that the echo area shows.
pub(super) const MAX_SAID: usize = 200;

/// What a repository's run thread sent the UI thread, as its `serial`th start.
pub(crate) struct RunEvent {
    project: ProjectId,
    serial: u64,
    report: RunReport,
}

impl std::fmt::Debug for RunEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunEvent")
            .field("serial", &self.serial)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// A repository as its runs know it.
pub(super) struct Repo {
    pub(super) project: ProjectId,
    root: TrustedRoot,
    /// Which start of the thread its reports must come from.
    serial: u64,
    runner: Option<Runner>,
    log: Option<BufferId>,
    pub(super) run: Option<Running>,
    /// A commit that waits for `HEAD`'s message.
    pub(super) reading: Option<Pending>,
}

/// A run under way.
pub(super) struct Running {
    pub(super) kind: CommitKind,
    started: Instant,
    /// What the echo area last said of it.
    echoed: String,
    ender: Option<Ender>,
    /// The message it commits, for the ring once it is in, and the buffer that holds it.
    pub(super) message: Option<String>,
    pub(super) draft: Option<BufferId>,
}

/// A commit asked for that waits for the message it starts from.
#[derive(Clone, Copy)]
pub(super) struct Pending {
    pub(super) kind: CommitKind,
    pub(super) switches: CommitSwitches,
}

impl App {
    /// Starts the commit `kind` in the repository at `index`, with `message` if it takes one,
    /// held by `draft`.
    pub(super) fn run_commit(
        &mut self,
        index: usize,
        kind: CommitKind,
        switches: CommitSwitches,
        message: Option<String>,
        draft: Option<BufferId>,
    ) {
        let pass = self
            .configuration
            .git()
            .pass_env
            .clone()
            .unwrap_or_default();
        let request = RunRequest::Commit {
            kind,
            switches,
            message: message.clone().map(String::into_bytes),
            pass,
        };
        if !self.ask_runner(index, request) {
            return;
        }
        let log = self.git_log(index);
        let echoed = format!("{}…", commit::title(kind));
        self.editor.echo(echoed.clone());
        self.editor.set_git_running(log, true);
        self.gits.commits.repos[index].run = Some(Running {
            kind,
            started: Instant::now(),
            echoed,
            ender: None,
            message,
            draft,
        });
        self.request_redraw();
    }

    /// Shows the log of the status `buffer`'s repository (`$`).
    pub(super) fn show_git_log(&mut self, status: BufferId) {
        let Some((project, root, _)) = self.trusted_status(status) else {
            return;
        };
        let index = self.git_repo(project, &root);
        let log = self.git_log(index);
        self.editor.show(log);
        self.request_redraw();
    }

    /// Stops what git runs in the repository of `buffer`, a status or its log: a commit, or a
    /// write to the index.
    pub(super) fn stop_git(&mut self, buffer: BufferId) {
        let project = self.editor.git_status_of(buffer).or_else(|| {
            (self.gits.commits.repos.iter())
                .find(|repo| repo.log == Some(buffer))
                .map(|repo| repo.project)
        });
        let Some(project) = project else {
            return;
        };
        if let Some(open) = self.gits.open.iter().find(|open| open.project == project)
            && let Some(worker) = &open.worker
        {
            worker.stop();
        }
        let Some(index) = self.repo_of(project) else {
            return;
        };
        let repo = &self.gits.commits.repos[index];
        if let (Some(run), Some(runner)) = (&repo.run, &repo.runner) {
            runner.stop();
            let title = commit::title(run.kind);
            self.editor.message(format!(
                "Stopping {title}: SIGTERM now, SIGKILL in {} s",
                crate::git::GRACE.as_secs()
            ));
        }
    }

    /// A repository's run thread reported.
    pub(in crate::app) fn git_run_reported(&mut self, event: RunEvent) {
        let RunEvent {
            project,
            serial,
            report,
        } = event;
        let index = (self.gits.commits.repos.iter())
            .position(|repo| repo.project == project && repo.serial == serial);
        let Some(index) = index else {
            return;
        };
        match report {
            RunReport::Message(read) => {
                let Some(pending) = self.gits.commits.repos[index].reading.take() else {
                    return;
                };
                self.editor
                    .clear_echo_saying("Reading the last commit's message…");
                match read {
                    Ok(text) => self.open_draft(project, pending.kind, pending.switches, &text),
                    Err(why) => self
                        .editor
                        .message(format!("git: {}", one_line(&why, MAX_SAID))),
                }
            }
            RunReport::Began { line, ender } => {
                if let Some(run) = self.gits.commits.repos[index].run.as_mut() {
                    run.ender = Some(ender);
                }
                let log = self.git_log(index);
                self.editor.append_git_log(log, &line);
            }
            RunReport::Output(text) => {
                let log = self.git_log(index);
                self.editor.append_git_log(log, &text);
            }
            RunReport::Elapsed(seconds) => {
                if let Some(run) = self.gits.commits.repos[index].run.as_mut() {
                    let now = format!("{}… {seconds} s", commit::title(run.kind));
                    self.editor.echo_unless_news(&run.echoed, now.clone());
                    run.echoed = now;
                }
            }
            RunReport::Ended(end) => self.run_ended(index, end),
        }
        self.request_redraw();
    }

    /// A run in the repository at `index` ended as `end`.
    fn run_ended(&mut self, index: usize, end: RunEnd) {
        let run = self.gits.commits.repos[index].run.take();
        let project = self.gits.commits.repos[index].project;
        let log = self.git_log(index);
        self.editor.set_git_running(log, false);
        let Some(run) = run else {
            if let RunEnd::Refused(why) = end {
                self.editor
                    .message(format!("git: {}", one_line(&why, MAX_SAID)));
            }
            return;
        };
        self.editor.clear_echo_saying(&run.echoed);
        let seconds = run.started.elapsed().as_secs();
        let (said, ended) = match end {
            RunEnd::Done => ("done", self.commit_done(&run)),
            RunEnd::Failed(why) => ("failed", why),
            RunEnd::Stopped => ("stopped", format!("{} stopped", commit::title(run.kind))),
            RunEnd::Refused(why) => ("not run", format!("git: {why}")),
        };
        self.editor
            .append_git_log(log, &format!("[{said} after {seconds} s]\n\n"));
        self.editor.message(one_line(&ended, MAX_SAID));
        // What git did, or a hook changed, is read: the person asked for the run.
        if let Some(status) = (self.gits.open.iter()).position(|open| open.project == project) {
            self.git_ask(status, Ask::Read);
        }
    }

    /// After commands and events: forgets logs that went, notes a run's message buffer that
    /// went, and stops the runs of projects no longer trusted.
    pub(super) fn sync_runs(&mut self, gone: &[BufferId]) {
        for repo in &mut self.gits.commits.repos {
            if repo
                .log
                .is_some_and(|log| self.editor.buffer(log).is_none())
            {
                repo.log = None;
            }
            if let Some(run) = repo.run.as_mut()
                && run.draft.is_some_and(|draft| gone.contains(&draft))
            {
                run.draft = None;
            }
            if !repo.root.revoked() && !self.editor.project_trusted(repo.project) {
                repo.root.revoke();
                // The run stops, and still reports how it ended.
                if let Some(runner) = &repo.runner {
                    runner.stop();
                }
            }
        }
    }

    /// As oso ends: each run's group is sent `SIGTERM`, with no wait (ADR 0127, Q7).
    pub(in crate::app) fn end_git_runs(&self) {
        for repo in &self.gits.commits.repos {
            if let Some(ender) = repo.run.as_ref().and_then(|run| run.ender.as_ref()) {
                ender.end();
            }
        }
    }

    /// The index of `project`'s repository, made if it has none, with `root` as its root when
    /// the one it had was revoked.
    pub(super) fn git_repo(&mut self, project: ProjectId, root: &TrustedRoot) -> usize {
        if let Some(index) = self.repo_of(project) {
            let repo = &mut self.gits.commits.repos[index];
            if repo.root.revoked() && repo.run.is_none() {
                repo.root = root.clone();
                repo.runner = None;
            }
            return index;
        }
        self.gits.commits.repos.push(Repo {
            project,
            root: root.clone(),
            serial: 0,
            runner: None,
            log: None,
            run: None,
            reading: None,
        });
        self.gits.commits.repos.len() - 1
    }

    pub(super) fn repo_of(&self, project: ProjectId) -> Option<usize> {
        (self.gits.commits.repos.iter()).position(|repo| repo.project == project)
    }

    /// Asks the repository's run thread for `request`, starting it if it has none; says why
    /// not, and `false`, if it cannot.
    pub(super) fn ask_runner(&mut self, index: usize, request: RunRequest) -> bool {
        let repo = &self.gits.commits.repos[index];
        if repo.root.revoked() || !self.editor.project_trusted(repo.project) {
            self.editor.message(RESTRICTED);
            return false;
        }
        if repo.runner.is_none() {
            self.gits.commits.serial += 1;
            let serial = self.gits.commits.serial;
            let (project, root) = (repo.project, repo.root.clone());
            let waker = self.waker.clone();
            let started = Runner::start(root, super::super::reviews::state(), move |report| {
                waker.send(UserEvent::GitRun(RunEvent {
                    project,
                    serial,
                    report,
                }))
            });
            let repo = &mut self.gits.commits.repos[index];
            match started {
                Ok(runner) => (repo.runner, repo.serial) = (Some(runner), serial),
                Err(e) => {
                    self.editor
                        .message(format!("Cannot start the repository's git thread: {e}"));
                    return false;
                }
            }
        }
        let repo = &mut self.gits.commits.repos[index];
        if repo
            .runner
            .as_ref()
            .is_some_and(|runner| runner.ask(request))
        {
            return true;
        }
        repo.runner = None;
        self.editor
            .message("The repository's git thread has ended; try again");
        false
    }

    /// The log of the repository at `index`, made if it has none: withheld from the agent
    /// channel, as a hook may print anything.
    fn git_log(&mut self, index: usize) -> BufferId {
        let repo = &self.gits.commits.repos[index];
        if let Some(log) = repo.log.filter(|log| self.editor.buffer(*log).is_some()) {
            return log;
        }
        let project = repo.project;
        let name = self.unique_name(&format!("*git log: {}*", self.project_name(project)));
        let log = self.editor.add_internal_buffer(name, Mode::GitLog);
        if let Some(buffer) = self.editor.buffer_mut(log) {
            buffer.withhold_from_agents();
        }
        self.gits.commits.repos[index].log = Some(log);
        log
    }

    pub(super) fn project_name(&self, project: ProjectId) -> String {
        (self.editor.project(project))
            .map_or_else(String::new, |found| one_line(found.name(), MAX_NAME))
    }

    /// `name`, told apart from another buffer of that name by `<2>`, `<3>`…
    pub(super) fn unique_name(&self, name: &str) -> String {
        let mut unique = name.to_owned();
        let mut n = 1;
        while self.editor.buffer_named(&unique).is_some() {
            n += 1;
            unique = format!("{name}<{n}>");
        }
        unique
    }
}
