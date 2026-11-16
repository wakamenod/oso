//! git's long runs in the app (ADR 0127, step 1b): each repository's run thread, its log,
//! what the thread reports, stopping a run, and what quitting ends. A run is a commit, from
//! step 2 a push, a pull or a fetch, whose progress the echo area shows too, from step 3a a
//! continue, a skip or an abort of what is stopped, and from step 3b a stash's and a branch's.
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
//! - Quitting oso sends each run's group `SIGTERM`, then `SIGKILL`, and waits for nothing.

use std::time::Instant;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{CommitKind, CommitSwitches};
use oso_core::project::ProjectId;

use super::super::{App, UserEvent};
use super::Ask;
use crate::commands::review::{MAX_NAME, one_line};
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
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
    pub(super) root: TrustedRoot,
    /// Which start of the thread its reports must come from.
    serial: u64,
    runner: Option<Runner>,
    pub(super) log: Option<BufferId>,
    pub(super) run: Option<Running>,
    /// A commit that waits for `HEAD`'s message.
    pub(super) reading: Option<Pending>,
    /// The message buffer whose draft request waits for the last subjects (step 6a).
    pub(super) subjects_for: Option<BufferId>,
    /// The last commit that failed with its message buffer open, for a fix request (step 6a).
    pub(super) failed: Option<Failure>,
    /// The pushes waiting for their plan, prompt or question (step 2).
    pub(super) pushes: super::remote::Pushes,
    /// A skip or an abort waiting for its question's answer (step 3a).
    pub(super) sequence_asked: Option<super::sequence::SequenceAsked>,
    /// A stash's prompt and question waiting (step 3b).
    pub(super) stashes: super::stash::Stashes,
    /// A branch head's list, plan, prompt and question waiting (step 3b).
    pub(super) branches: super::branch::Branching,
}

/// A commit that failed, as a fix request names it.
pub(super) struct Failure {
    /// The message buffer it left.
    pub(super) draft: BufferId,
    pub(super) hooks: Vec<&'static str>,
    /// What it wrote, its command line first, as the log took it: its last
    /// [`MAX_RUN_OUTPUT`] bytes.
    pub(super) output: String,
}

/// The most bytes of a run's own output kept for a fix request: twice what one sends, so that
/// whole lines are left to cut from.
const MAX_RUN_OUTPUT: usize = 2 * crate::vcs::agent::MAX_FIX_BYTES;

/// A run under way.
pub(super) struct Running {
    /// The commit it makes, or `None` for a remote's run, a step of what is stopped, a stash's or
    /// a branch's.
    pub(super) kind: Option<CommitKind>,
    /// What the log and the echo area call it.
    pub(super) title: String,
    /// What the echo area says once a run that makes no commit went well.
    pub(super) done: String,
    pub(super) started: Instant,
    /// What the echo area last said of it.
    pub(super) echoed: String,
    /// The progress a remote's run showed last.
    pub(super) progress: Option<String>,
    /// The message it commits, for the ring once it is in, and the buffer that holds it.
    pub(super) message: Option<String>,
    pub(super) draft: Option<BufferId>,
    /// What it wrote, as [`Failure::output`] keeps it.
    pub(super) output: String,
}

/// A commit asked for that waits for the message it starts from.
#[derive(Clone, Copy)]
pub(super) struct Pending {
    pub(super) kind: CommitKind,
    pub(super) switches: CommitSwitches,
}

impl App {
    /// Starts the commit `kind` in the repository at `index`, with `message` if it takes one,
    /// held by `draft`; a fixup or a squash for the commit `target`.
    pub(super) fn run_commit(
        &mut self,
        index: usize,
        (kind, switches): (CommitKind, CommitSwitches),
        message: Option<String>,
        draft: Option<BufferId>,
        target: Option<String>,
    ) {
        let request = RunRequest::Commit {
            kind,
            switches,
            message: message.clone().map(String::into_bytes),
            pass: self.pass_env(),
            target,
        };
        if !self.ask_runner(index, request) {
            return;
        }
        let log = self.git_log(index);
        let echoed = format!("{}…", commit::title(kind));
        self.editor.echo(echoed.clone());
        self.editor.set_git_running(log, true);
        let repo = &mut self.gits.commits.repos[index];
        repo.failed = None;
        repo.run = Some(Running {
            kind: Some(kind),
            title: commit::title(kind).to_owned(),
            done: String::new(),
            started: Instant::now(),
            echoed,
            progress: None,
            message,
            draft,
            output: String::new(),
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
            let title = &run.title;
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
                    // Echoed, not logged: git's words may quote the repository.
                    Err(why) => self
                        .editor
                        .echo(format!("git: {}", one_line(&why, MAX_SAID))),
                }
            }
            RunReport::Subjects { subjects, id } => self.subjects_read(index, &subjects, id),
            RunReport::Began { line } => {
                self.keep_output(index, &line);
                let log = self.git_log(index);
                self.editor.append_git_log(log, &line);
            }
            RunReport::Output(text) => {
                self.keep_output(index, &text);
                let log = self.git_log(index);
                self.editor.append_git_log(log, &text);
                // Only a log on screen needs a frame for it.
                if !self.editor.windows().any(|window| window.buffer() == log) {
                    return;
                }
            }
            RunReport::Elapsed(seconds) => self.echo_run(index, Some(seconds), None),
            RunReport::Progress(progress) => self.echo_run(index, None, Some(progress)),
            RunReport::Planned(planned) => self.push_planned(index, planned),
            RunReport::Branches(read) => self.branches_listed(index, read),
            RunReport::DeletePlanned { name, plan } => self.delete_planned(index, name, plan),
            RunReport::Ended(end) => self.run_ended(index, end),
            RunReport::Discarded(read) => self.discarded_read(index, read),
            RunReport::Restored(done) => self.restored(index, done),
        }
        self.request_redraw();
    }

    /// Says in the echo area how the run under way in the repository at `index` goes: its
    /// title, its `progress` if it has new, and the `seconds` it has taken, unless something
    /// was said since.
    fn echo_run(&mut self, index: usize, seconds: Option<u64>, progress: Option<String>) {
        let Some(run) = self.gits.commits.repos[index].run.as_mut() else {
            return;
        };
        if progress.is_some() {
            run.progress = progress;
        }
        let seconds = seconds.unwrap_or_else(|| run.started.elapsed().as_secs());
        let now = match &run.progress {
            Some(progress) => format!("{}… {progress} {seconds} s", run.title),
            None => format!("{}… {seconds} s", run.title),
        };
        self.editor.echo_unless_news(&run.echoed, now.clone());
        run.echoed = now;
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
                    .echo(format!("git: {}", one_line(&why, MAX_SAID)));
            }
            return;
        };
        self.editor.clear_echo_saying(&run.echoed);
        let seconds = run.started.elapsed().as_secs();
        let (said, ended) = match end {
            RunEnd::Done if run.kind.is_none() => ("done", run.done.clone()),
            RunEnd::Done => ("done", self.commit_done(&run)),
            RunEnd::Failed { said, .. } if run.kind.is_none() => ("failed", said),
            RunEnd::Failed { said, hooks } => {
                ("failed", self.commit_failed(index, run, said, hooks))
            }
            RunEnd::Stopped => ("stopped", format!("{} stopped", run.title)),
            RunEnd::Refused(why) => ("not run", format!("git: {why}")),
        };
        self.editor
            .append_git_log(log, &format!("[{said} after {seconds} s]\n\n"));
        // Echoed, not logged: a summary is the person's text (ADR 0028), and git's words may
        // quote the repository.
        self.editor.echo(one_line(&ended, MAX_SAID));
        // What git did, or a hook changed, is read: the person asked for the run.
        if let Some(status) = (self.gits.open.iter()).position(|open| open.project == project) {
            self.git_ask(status, Ask::Read);
        }
    }

    /// Keeps `text` as what the run under way in the repository at `index` wrote, its last
    /// [`MAX_RUN_OUTPUT`] bytes, from a line's start.
    fn keep_output(&mut self, index: usize, text: &str) {
        let Some(run) = self.gits.commits.repos[index].run.as_mut() else {
            return;
        };
        run.output.push_str(text);
        crate::vcs::agent::keep_end(&mut run.output, MAX_RUN_OUTPUT);
    }

    /// What a commit in the repository at `index` that failed leaves: with its message buffer
    /// open, what a fix request names, and the echo area's word of `C-c C-f` when the worktree
    /// has a session to ask.
    fn commit_failed(
        &mut self,
        index: usize,
        run: Running,
        said: String,
        hooks: Vec<&'static str>,
    ) -> String {
        let Some(draft) = run
            .draft
            .filter(|draft| self.editor.buffer(*draft).is_some())
        else {
            return said;
        };
        self.gits.commits.repos[index].failed = Some(Failure {
            draft,
            hooks,
            output: run.output,
        });
        let root = self.gits.commits.repos[index].root.path().to_path_buf();
        if self.worktree_session(&root).is_some() {
            format!("{said}; C-c C-f asks the session to fix it")
        } else {
            said
        }
    }

    /// After commands and events: forgets logs that went, notes a run's message buffer that
    /// went, and stops the runs of projects no longer trusted.
    pub(super) fn sync_runs(&mut self, gone: &[BufferId]) {
        for index in 0..self.gits.commits.repos.len() {
            let repo = &mut self.gits.commits.repos[index];
            let log_gone = repo
                .log
                .is_some_and(|log| self.editor.buffer(log).is_none());
            if log_gone {
                repo.log = None;
            }
            if let Some(run) = repo.run.as_mut()
                && run.draft.is_some_and(|draft| gone.contains(&draft))
            {
                run.draft = None;
            }
            if !self.editor.project_trusted(repo.project) {
                // The status's revocation may have come first: the run stops all the same, on
                // its thread too, and still reports how it ended.
                repo.root.revoke();
                if repo.run.is_some()
                    && let Some(runner) = &repo.runner
                {
                    runner.stop();
                }
            }
            // A log killed while git runs comes back, so that quitting still names the run.
            if log_gone && self.gits.commits.repos[index].run.is_some() {
                let log = self.git_log(index);
                self.editor.set_git_running(log, true);
            }
        }
    }

    /// As oso ends: each run's group is sent `SIGTERM`, then `SIGKILL`, with no wait (ADR 0127,
    /// Q7 and its follow-up).
    pub(in crate::app) fn end_git_runs(&self) {
        for repo in &self.gits.commits.repos {
            if let Some(runner) = &repo.runner {
                runner.end();
            }
        }
    }

    /// A run that makes no commit, called `title`, began in the repository at `index`: a
    /// remote's (step 2), or a step of what is stopped (step 3a); `done` is what the echo area
    /// says once it went well.
    pub(super) fn run_began(&mut self, index: usize, title: String, done: String) {
        let echoed = format!("{title}…");
        self.editor.echo(echoed.clone());
        let log = self.git_log(index);
        self.editor.set_git_running(log, true);
        let repo = &mut self.gits.commits.repos[index];
        repo.run = Some(Running {
            kind: None,
            title,
            done,
            started: Instant::now(),
            echoed,
            progress: None,
            message: None,
            draft: None,
            output: String::new(),
        });
        self.request_redraw();
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
            subjects_for: None,
            failed: None,
            pushes: super::remote::Pushes::default(),
            sequence_asked: None,
            stashes: super::stash::Stashes::default(),
            branches: super::branch::Branching::default(),
        });
        self.gits.commits.repos.len() - 1
    }

    /// Whether git runs in `project`'s repository, or a commit waits for its message, a push for
    /// its plan, or a branch's head for its list or its delete's plan (step 3b).
    pub(super) fn repo_busy(&self, project: ProjectId) -> bool {
        (self.repo_of(project)).is_some_and(|index| {
            let repo = &self.gits.commits.repos[index];
            repo.run.is_some()
                || repo.reading.is_some()
                || repo.pushes.planning.is_some()
                || repo.branches.planning()
        })
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
    pub(super) fn git_log(&mut self, index: usize) -> BufferId {
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
        self.stand_in_root(log, project);
        self.gits.commits.repos[index].log = Some(log);
        log
    }

    /// `buffer`, made for `project`, stands in its root, as its status does (ADR 0141).
    pub(super) fn stand_in_root(&mut self, buffer: BufferId, project: ProjectId) {
        if let Some(root) = self
            .editor
            .project(project)
            .map(|found| found.root().to_path_buf())
        {
            self.editor.set_buffer_directory(buffer, root);
        }
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
