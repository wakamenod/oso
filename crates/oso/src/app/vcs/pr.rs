//! Pull requests in the app (ADR 0127, step 5a): the status's `PR:` line, `*pr: #N*` and the
//! list of open ones, what their keys ask, and opening a pull request in the browser.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs `gh` or git, or
//! lays out a text: each repository's thread, `oso-vcs-gh`, does, and a text goes in at O(1).
//!
//! - The `PR:` line is read when the person opens the status or types `g` in it, at most once
//!   every [`HEAD_EVERY`], never on a timer, and again right after oso's own push (Q3). Its
//!   answer goes in the status's view, which lays the line out.
//! - A missing or logged-out `gh` is said once a repository, and the status shows no line.
//! - A pull request's or a list's buffer is read when it is shown and on `g`; nothing polls.
//! - A review thread's file is visited only at a plain relative path under the root; an address
//!   is shown in the echo area before it opens, and only the pull request's own opens (Q5).
//! - A project no longer trusted runs nothing more: its thread's root is revoked.

use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{GitRequest, GitShown, PrAct};
use oso_core::project::ProjectId;

use super::super::{App, UserEvent};
use super::Ask;
use crate::commands::review::one_line;
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::pr::PrHead;
use crate::vcs::pr::work::{PrReport, PrRequest, PrWorker};
use crate::vcs::pr::write::{PrBuilt, PrLaid, PrRow};

/// The least time between two reads of a status's `PR:` line the person asked for.
const HEAD_EVERY: Duration = Duration::from_secs(30);

/// What a repository's `gh` thread sent the UI thread, as its `serial`th start.
pub(crate) struct PrEvent {
    project: ProjectId,
    serial: u64,
    report: PrReport,
}

impl std::fmt::Debug for PrEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrEvent")
            .field("serial", &self.serial)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// The repositories `gh` is asked about, and the pull requests' buffers.
#[derive(Default)]
pub(super) struct Prs {
    repos: Vec<PrRepo>,
    views: Vec<PrBuffer>,
    serial: u64,
    generation: u64,
}

/// A repository's `gh` thread, and what its status's `PR:` line last read.
struct PrRepo {
    project: ProjectId,
    root: TrustedRoot,
    serial: u64,
    worker: Option<PrWorker>,
    /// The generation of the last head line asked, and when.
    head_asked: Option<(u64, Instant)>,
    /// What was said of the last failure, so that it is said once.
    said: Option<String>,
}

/// A pull request's buffer, or a list of them.
struct PrBuffer {
    buffer: BufferId,
    project: ProjectId,
    /// The pull request it shows, or the branch's; none for a list.
    number: Option<u64>,
    list: bool,
    /// Whether the next text goes in at its checks.
    at_checks: bool,
    asked: u64,
    /// Whether the next text is the first since the buffer was shown, so point starts at its top.
    fresh: bool,
    built: Arc<PrBuilt>,
    /// A text that came while the editor's text must not change.
    waiting: Option<Box<PrLaid>>,
}

impl App {
    /// What a pull request's command asks.
    pub(super) fn git_pr_request(&mut self, request: GitRequest) {
        match request {
            GitRequest::PrView {
                project,
                number,
                checks,
            } => self.show_pr(project, number, false, checks),
            GitRequest::PrList { project } => self.show_pr(project, None, true, false),
            GitRequest::PrOpen { project } => {
                self.ask_pr(project, PrRequest::Address);
            }
            GitRequest::PrAct { buffer, act, line } => self.pr_act(buffer, act, line),
            // The others are not pull requests' (`requests::concern`).
            _ => {}
        }
    }

    /// Asks for the `PR:` line of `project`'s status: at most once every [`HEAD_EVERY`] unless
    /// `now`, as after oso's own push.
    pub(super) fn ask_pr_head(&mut self, project: ProjectId, now: bool) {
        let recent = (self.pr_repo(project))
            .and_then(|index| self.gits.prs.repos[index].head_asked)
            .is_some_and(|(_, at)| at.elapsed() < HEAD_EVERY);
        if recent && !now {
            return;
        }
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        if let Some(index) = self.ask_pr(project, PrRequest::Head { generation }) {
            self.gits.prs.repos[index].head_asked = Some((generation, Instant::now()));
        }
    }

    /// Shows the buffer of `project`'s pull request `number`, or the branch's, or the list if
    /// `list`, and reads it again.
    fn show_pr(&mut self, project: ProjectId, number: Option<u64>, list: bool, checks: bool) {
        let found = (self.gits.prs.views.iter())
            .position(|view| view.project == project && view.number == number && view.list == list)
            .filter(|&index| {
                self.editor
                    .git_view_of(self.gits.prs.views[index].buffer)
                    .is_some()
            });
        let index = found.unwrap_or_else(|| {
            let name = match (list, number) {
                (true, _) => format!("*pull requests: {}*", self.project_name(project)),
                (false, Some(number)) => format!("*pr: #{number}*"),
                (false, None) => format!("*pr: {}*", self.project_name(project)),
            };
            let mode = if list { Mode::GitPrList } else { Mode::GitPr };
            let buffer = self.editor.add_git_view(project, name, mode);
            self.gits.prs.views.push(PrBuffer {
                buffer,
                project,
                number,
                list,
                at_checks: false,
                asked: 0,
                fresh: true,
                built: Arc::default(),
                waiting: None,
            });
            self.gits.prs.views.len() - 1
        });
        let view = &mut self.gits.prs.views[index];
        view.at_checks = checks;
        view.fresh = true;
        let buffer = view.buffer;
        self.editor.show(buffer);
        self.read_pr(index);
        self.request_redraw();
    }

    fn read_pr(&mut self, index: usize) {
        self.gits.prs.generation += 1;
        let generation = self.gits.prs.generation;
        let view = &mut self.gits.prs.views[index];
        view.asked = generation;
        let (project, number, list) = (view.project, view.number, view.list);
        let request = if list {
            PrRequest::List { generation }
        } else {
            PrRequest::View { generation, number }
        };
        if self.ask_pr(project, request).is_some() {
            self.editor.echo("Asking gh…");
        }
    }

    /// Asks `project`'s `gh` thread for `request`, starting it if need be; its repository's
    /// index, if the thread took it.
    fn ask_pr(&mut self, project: ProjectId, request: PrRequest) -> Option<usize> {
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            self.editor.message(RESTRICTED);
            return None;
        };
        let index = self.pr_repo(project).unwrap_or_else(|| {
            self.gits.prs.repos.push(PrRepo {
                project,
                root: root.clone(),
                serial: 0,
                worker: None,
                head_asked: None,
                said: None,
            });
            self.gits.prs.repos.len() - 1
        });
        let repo = &mut self.gits.prs.repos[index];
        if repo.root.revoked() {
            repo.root = root;
            repo.worker = None;
        }
        if repo.worker.is_none() {
            self.start_pr_worker(index);
        }
        let asked =
            (self.gits.prs.repos[index].worker.as_ref()).is_some_and(|worker| worker.ask(request));
        if !asked {
            self.gits.prs.repos[index].worker = None;
            self.editor.message("gh's thread had ended: ask again");
            return None;
        }
        Some(index)
    }

    fn start_pr_worker(&mut self, index: usize) {
        self.gits.prs.serial += 1;
        let serial = self.gits.prs.serial;
        let repo = &mut self.gits.prs.repos[index];
        repo.serial = serial;
        let project = repo.project;
        let waker = self.waker.clone();
        let started = PrWorker::start(
            repo.root.clone(),
            super::super::reviews::state(),
            move |report| {
                waker.send(UserEvent::Pr(PrEvent {
                    project,
                    serial,
                    report,
                }))
            },
        );
        match started {
            Ok(worker) => self.gits.prs.repos[index].worker = Some(worker),
            Err(e) => self
                .editor
                .message(format!("Cannot start gh's thread: {e}")),
        }
    }

    fn pr_repo(&self, project: ProjectId) -> Option<usize> {
        (self.gits.prs.repos.iter()).position(|repo| repo.project == project)
    }

    /// A repository's `gh` thread reported.
    pub(in crate::app) fn pr_reported(&mut self, event: PrEvent) {
        let PrEvent {
            project,
            serial,
            report,
        } = event;
        let Some(index) = self.pr_repo(project) else {
            return self.freer.free(report);
        };
        if self.gits.prs.repos[index].serial != serial {
            return self.freer.free(report);
        }
        match report {
            PrReport::Head { generation, head } => self.pr_head_read(index, generation, head),
            PrReport::View { generation, laid } => self.pr_text(generation, laid),
            PrReport::List { generation, laid } => {
                self.pr_text(generation, laid.map(|(_, laid)| laid));
            }
            PrReport::Address(Ok((url, number))) => self.open_pr_address(project, url, number),
            PrReport::Address(Err(why)) | PrReport::Opened(why) => {
                self.editor.message(one_line(&why, super::MAX_MESSAGE));
            }
        }
        self.request_redraw();
    }

    fn pr_head_read(
        &mut self,
        index: usize,
        generation: u64,
        head: Result<Option<PrHead>, String>,
    ) {
        let repo = &mut self.gits.prs.repos[index];
        if repo.head_asked.is_none_or(|(asked, _)| asked != generation) {
            return;
        }
        let project = repo.project;
        let head = match head {
            Ok(head) => {
                repo.said = None;
                head
            }
            Err(why) => {
                // Said once, not on every read: the status shows no line meanwhile.
                if repo.said.as_deref() != Some(why.as_str()) {
                    self.editor
                        .message(format!("PR: {}", one_line(&why, super::MAX_MESSAGE)));
                    self.gits.prs.repos[index].said = Some(why);
                }
                None
            }
        };
        let head = head.map(Arc::new);
        if let Some(status) = (self.gits.open.iter()).position(|open| open.project == project)
            && self.gits.open[status].view.pr != head
        {
            self.gits.open[status].view.pr = head;
            self.git_ask(status, Ask::Layout);
        }
    }

    fn pr_text(&mut self, generation: u64, laid: Result<Box<PrLaid>, String>) {
        let Some(index) = (self.gits.prs.views.iter()).position(|view| view.asked == generation)
        else {
            return self.freer.free(laid);
        };
        self.editor.clear_echo_saying("Asking gh…");
        match laid {
            Ok(laid) if self.editor.reload_waits() => {
                let old = self.gits.prs.views[index].waiting.replace(laid);
                self.freer.free(old);
            }
            Ok(laid) => self.put_in_pr(index, *laid),
            Err(why) => self.editor.message(one_line(&why, super::MAX_MESSAGE)),
        }
    }

    fn put_in_pr(&mut self, index: usize, laid: PrLaid) {
        let PrLaid {
            text,
            highlights,
            built,
        } = laid;
        let view = &mut self.gits.prs.views[index];
        let buffer = view.buffer;
        let (line, lines) = if std::mem::take(&mut view.fresh) {
            let line = if view.at_checks { built.checks_line } else { 0 };
            (line, Vec::new())
        } else {
            let lines = self.editor.git_points(buffer);
            (lines.first().map_or(0, |(_, line)| *line), lines)
        };
        let old_text =
            (self.editor.buffer(buffer)).map(|shown| (shown.text().clone(), shown.display()));
        let shown = GitShown {
            highlights,
            outline: Vec::new(),
            line,
            lines,
        };
        let refused = self.editor.set_git_text(buffer, text, shown);
        let old = std::mem::replace(&mut self.gits.prs.views[index].built, built);
        self.freer.free((old, old_text, refused));
        self.buffers_changed();
    }

    /// After each event: forgets killed buffers, and repositories no longer trusted, revoking
    /// their roots, and puts in a text that waited.
    pub(super) fn sync_prs(&mut self) {
        let editor = &self.editor;
        self.gits
            .prs
            .views
            .retain(|view| editor.git_view_of(view.buffer).is_some());
        for repo in &mut self.gits.prs.repos {
            if repo.worker.is_some() && !editor.project_trusted(repo.project) {
                repo.root.revoke();
                repo.worker = None;
            }
        }
        if self.editor.reload_waits() {
            return;
        }
        for index in 0..self.gits.prs.views.len() {
            if let Some(laid) = self.gits.prs.views[index].waiting.take() {
                self.put_in_pr(index, *laid);
            }
        }
    }

    /// What `act` asks of `line` of the pull request's buffer, or list, `buffer`.
    fn pr_act(&mut self, buffer: BufferId, act: PrAct, line: usize) {
        let Some(index) = (self.gits.prs.views.iter()).position(|view| view.buffer == buffer)
        else {
            return;
        };
        let view = &self.gits.prs.views[index];
        let project = view.project;
        let row = view.built.row(line).clone();
        let pull = view.built.pull.clone();
        match (act, row) {
            (PrAct::Refresh, _) => self.read_pr(index),
            (PrAct::Visit, PrRow::Thread { path, line }) => {
                self.visit_pr_thread(project, &path, line)
            }
            (PrAct::Visit, PrRow::Check { link }) if !link.is_empty() => {
                self.editor
                    .message(format!("{} (w copies it)", one_line(&link, 300)));
            }
            (PrAct::Visit, PrRow::Pull { number, .. }) => {
                self.show_pr(project, Some(number), false, false)
            }
            (PrAct::Visit, _) => self.editor.message("Nothing here to visit"),
            (PrAct::Open, PrRow::Pull { number, url }) => {
                self.open_pr_address(project, url, number)
            }
            (PrAct::Open, _) => match pull {
                Some((number, url)) => self.open_pr_address(project, url, number),
                None => self.editor.message("No pull request here"),
            },
            (PrAct::CopyLink, PrRow::Check { link }) if !link.is_empty() => {
                self.copy_pr_link(&link)
            }
            (PrAct::CopyLink, PrRow::Pull { url, .. }) => self.copy_pr_link(&url),
            (PrAct::CopyLink, _) => match pull {
                Some((_, url)) => self.copy_pr_link(&url),
                None => self.editor.message("No address here"),
            },
        }
    }

    /// Shows `url`, pull request `number`'s address, and asks the thread to open it, which it
    /// does only if it is that pull request's own (Q5).
    fn open_pr_address(&mut self, project: ProjectId, url: String, number: u64) {
        self.editor
            .message(format!("Opening {}", one_line(&url, 300)));
        self.ask_pr(project, PrRequest::Open { url, number });
    }

    fn copy_pr_link(&mut self, url: &str) {
        self.editor.kill_new(url, false);
        self.write_clipboard();
        self.editor
            .message(format!("Copied {}", one_line(url, 300)));
    }

    /// Visits `path`, a review thread's file as GitHub names it, at `line`, if it is a plain
    /// relative path under `project`'s root.
    fn visit_pr_thread(&mut self, project: ProjectId, path: &str, line: Option<u64>) {
        let plain = crate::vcs::pr::is_plain_path(path);
        let Some(root) = TrustedRoot::of(&self.editor, project).filter(|_| plain) else {
            return self
                .editor
                .message("That file is not one under the project's root");
        };
        let path = root.path().join(path);
        let at = line.and_then(|line| usize::try_from(line.saturating_sub(1)).ok());
        match at {
            Some(line) => self
                .editor
                .request_visit_at(path, oso_core::load::UNASKED, line, 0),
            None => self.editor.request_visit(path, oso_core::load::UNASKED),
        }
        self.serve_files();
    }
}
