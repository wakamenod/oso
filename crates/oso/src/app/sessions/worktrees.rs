//! Worktree sessions in the app (ADR 0121 step 4): what the worktree commands ask, carried out
//! on the UI thread. `session-worktree` lists the project's branches, opens `*Brief*`, and on
//! `C-c C-c` makes the worktree and starts its session. `worktree-list` fills `*Worktrees*` row
//! by row. `worktree-remove` checks first, asks, and removes only a worktree nothing would be
//! lost from. Every git and `gh` call is a job on the `oso-sessions-git` threads, but a
//! removal's `git worktree remove`, which runs on `oso-sessions-remove` and which
//! `worktree-remove-stop` stops (`stop`). A made worktree is held by the issue space that made
//! it, where its session joins (ADR 0136).
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! What a job needs from the UI thread goes with it; nothing here touches the disk. Each job
//! takes the project's trust as it is when the job is asked for, not as it was when `*Brief*`
//! or `*Worktrees*` opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use oso_core::buffer::BufferId;
use oso_core::command::Effects;
use oso_core::editor::SessionRequest;
use oso_core::project::ProjectId;

use super::super::{App, Loop, UserEvent};

mod brief;
mod listing;
mod removal;
mod stop;
use super::watch::Watch;
use crate::git::TrustedRoot;
use crate::sessions::hosted::{Hosted, Run};
use crate::sessions::identity::SessionId;
use crate::sessions::state::State;
use crate::sessions::worktree::checks::{Held, HostedSession};
use crate::sessions::worktree::create::Handover;
use crate::sessions::worktree::jobs::{Jobs, Report, Snapshot, Trust};
use crate::sessions::worktree::list::{Row, RowText, Shown};

/// The name of the buffer a brief is written in.
const BRIEF: &str = "*Brief*";
/// The name of the list of worktrees.
const LIST: &str = "*Worktrees*";
/// How many ignored names the removal's question gives.
const QUESTION_NAMES: usize = 8;

/// What the app keeps of worktree sessions.
#[derive(Debug, Default)]
pub(in crate::app) struct Worktrees {
    /// The jobs, made with the first one.
    jobs: Option<Jobs>,
    next: u64,
    /// `*Brief*` buffers not sent yet.
    briefs: HashMap<BufferId, Brief>,
    /// Worktrees being made, by the job's token.
    creating: HashMap<u64, Brief>,
    /// The `*Worktrees*` buffer and its rows.
    list: Option<Listing>,
    /// The removal under way: checked, asked, or removing.
    removing: Option<Removing>,
    /// The running removal whose stop the person is asked about: its job's token and path.
    stopping: Option<(u64, PathBuf)>,
    /// The branch listing a prompt waits for, by its job's token.
    branch_wait: Option<u64>,
}

/// A worktree session's brief, before and while its worktree is made.
#[derive(Debug, Clone)]
struct Brief {
    project: ProjectId,
    branch: String,
    base: Option<String>,
    from: Option<Handover>,
    parent: Option<SessionId>,
    /// Where its worktree goes, for the check of unsent briefs.
    place: PathBuf,
    buffer: BufferId,
}

#[derive(Debug)]
struct Listing {
    token: u64,
    buffer: BufferId,
    project: ProjectId,
    /// The project's root, as the list names it.
    main: PathBuf,
    rows: Vec<Row>,
    /// Each row's lines, formatted once.
    lines: Vec<RowText>,
    pending: usize,
    shown: Shown,
}

#[derive(Debug)]
struct Removing {
    token: u64,
    project: ProjectId,
    path: PathBuf,
    /// What git ignores there, once checked: the question names it.
    ignored: Option<Vec<PathBuf>>,
}

impl App {
    /// Carries out what a worktree command asked.
    pub(in crate::app) fn worktree_request(&mut self, request: SessionRequest) {
        match request {
            SessionRequest::Worktree { branch, base } => self.open_brief(branch, base),
            SessionRequest::SendBrief(buffer) => self.send_brief(buffer),
            SessionRequest::CancelBrief(buffer) => self.cancel_brief(buffer),
            SessionRequest::WorktreeList => self.list_worktrees(),
            SessionRequest::WorktreeRemove(path) => self.remove_worktree(path),
            SessionRequest::WorktreeRemoveConfirmed => self.remove_confirmed(),
            SessionRequest::WorktreeRemoveStop => self.stop_removal(),
            SessionRequest::WorktreeRemoveStopConfirmed => self.stop_removal_confirmed(),
            SessionRequest::StartShared => self.start_shared(),
            other => log::debug!("not a worktree request: {other:?}"),
        }
        self.request_redraw();
    }

    /// The jobs, made with the first one.
    pub(in crate::app) fn worktree_jobs(&mut self) -> Jobs {
        let waker = Mutex::new(self.waker.clone());
        let state = self.hosting.directory.clone();
        self.hosting
            .worktrees
            .jobs
            .get_or_insert_with(|| {
                Jobs::new(state, move |report| {
                    waker
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .send(UserEvent::Worktree(Box::new(report)));
                })
            })
            .clone()
    }

    pub(in crate::app) fn worktree_token(&mut self) -> u64 {
        self.hosting.worktrees.next += 1;
        self.hosting.worktrees.next
    }

    /// The selected buffer's project, if it is trusted to run git.
    pub(in crate::app) fn project_here(&self) -> Result<ProjectId, String> {
        // In `*Spaces*`, the repository of the row at point (ADR 0131, step 2b).
        if let Some(place) = self.sessions_row_place() {
            let place = place?;
            return self.trusted_project_of(place.repo()?).ok_or_else(|| {
                "Worktree sessions need a trusted buffer of that project: trust-project first"
                    .to_owned()
            });
        }
        // A space's orchestrator runs in no repository: its space's one trusted repository,
        // else the one `space-use-repo` chose (ADR 0136, step 4).
        let buffer = self.editor.window().buffer();
        if let Some(root) = self.orchestrator_repository(buffer) {
            let root = root?;
            return self.trusted_project_of(&root).ok_or_else(|| {
                "Worktree sessions need a trusted buffer of that repository: open one of its files"
                    .to_owned()
            });
        }
        let project = self.editor.current().1.project();
        project
            .filter(|&id| self.editor.project_trusted(id))
            .ok_or_else(|| {
                "Worktree sessions need a trusted project's buffer: trust-project first".to_owned()
            })
    }

    /// The trust a job for `project` runs under, as it is now.
    pub(in crate::app) fn trust_for(&self, project: ProjectId) -> Result<Trust, String> {
        let root = TrustedRoot::of(&self.editor, project).ok_or_else(|| {
            "Worktree sessions need a trusted project: trust-project first".to_owned()
        })?;
        Ok(Trust {
            root,
            trusted: self.trusted_list(),
        })
    }

    /// Asks for the current project's branches when a command waits for them, and says so in
    /// the echo area. A refusal is answered at once. Returns what that answer did.
    pub(in crate::app) fn sync_branch_wait(&mut self) -> Effects {
        if !self.dispatcher.take_branches_request() {
            return Effects::default();
        }
        match self.project_here().and_then(|id| self.trust_for(id)) {
            Ok(trust) => {
                let token = self.worktree_token();
                self.hosting.worktrees.branch_wait = Some(token);
                self.worktree_jobs().branches(token, trust);
                self.editor.echo("Listing branches…");
                Effects {
                    redraw: true,
                    ..Effects::default()
                }
            }
            Err(why) => self
                .dispatcher
                .branches_answered(&mut self.editor, Err(why)),
        }
    }

    /// A worktree job reported.
    pub(in crate::app) fn worktree_reported(&mut self, event_loop: &Loop<'_>, report: Report) {
        if let Report::WorkMerged {
            key,
            judgment,
            merged,
        } = report
        {
            return self.session_work_judged(key, judgment, merged);
        }
        // A worktree a listing found clean and merged is the work of its sessions merged (ADR
        // 0121, "Merged work is not ready for review, as amended"): the person's listing speaks
        // for every session there, an agent's only for those of its own space.
        if let Some((token, path, looked_at)) = merged_work_at(&report) {
            let person = (self.hosting.worktrees.list.as_ref()).is_some_and(|l| l.token == token);
            let keys = if person {
                self.sessions_at(&path)
            } else {
                self.agent_listing_sessions(token, &path)
            };
            self.session_work_merged(&keys, looked_at);
        }
        // A job an agent's session tool asked for (ADR 0121, step 6).
        // Its handlers ask for a frame only when they show something.
        let Some(report) = self.session_tool_report(report) else {
            return;
        };
        match report {
            Report::Branches { token, branches } => {
                // Only the listing the newest wait asked for opens a prompt.
                if self.hosting.worktrees.branch_wait != Some(token) {
                    return;
                }
                self.hosting.worktrees.branch_wait = None;
                let effects = self
                    .dispatcher
                    .branches_answered(&mut self.editor, branches);
                self.apply(event_loop, effects);
            }
            Report::Tree { root, result } => self.tree_listed(root, result),
            Report::Step { token, step } => self.worktree_step(token, step),
            Report::Created { token, result } => self.worktree_created(token, result),
            Report::Listed { token, result } => self.worktrees_listed(token, result),
            Report::Row { token, looked } => self.worktree_row(token, *looked),
            Report::Merged {
                token,
                path,
                merged,
                ..
            } => self.worktree_merged(token, &path, merged),
            Report::Checked {
                token,
                path,
                result,
            } => self.worktree_checked(token, &path, result),
            Report::Clear { token, path, go } => self.worktree_clear(token, &path, go),
            Report::Removed {
                token,
                path,
                result,
            } => self.worktree_removed(token, &path, result),
            // The person's listing ends as its rows come; only an agent's waits for the end.
            Report::ListEnded { .. } | Report::Prepared { .. } | Report::WorkMerged { .. } => {}
        }
        self.request_redraw();
    }

    /// What the UI thread holds about the worktree at `path` alone: buffers with unsaved
    /// changes under it, an unsent brief for it, and its hosted sessions that are not
    /// [`settled`]. The registry's half of check 4 is the job's to read.
    pub(in crate::app) fn held_at(&self, path: &Path) -> Held {
        let unsaved = self
            .editor
            .buffers()
            .filter(|(_, buffer)| buffer.is_modified())
            .filter(|(_, buffer)| buffer.path().is_some_and(|file| file.starts_with(path)))
            .count();
        let worktrees = &self.hosting.worktrees;
        let unsent = worktrees
            .briefs
            .values()
            .chain(worktrees.creating.values())
            .any(|brief| brief.place.starts_with(path));
        let busy = running_sessions(self.hosting.table.all(), &self.watch)
            .into_iter()
            .filter(|session| !session.settled && session.directory.starts_with(path))
            .map(|session| session.name)
            .collect();
        Held {
            unsaved,
            unsent,
            busy,
        }
    }

    /// What the UI thread holds that a worktree job cannot read: buffers with unsaved
    /// changes, unsent briefs and the hosted sessions.
    pub(in crate::app) fn worktree_snapshot(&self) -> Snapshot {
        let unsaved = self
            .editor
            .buffers()
            .filter(|(_, buffer)| buffer.is_modified())
            .filter_map(|(_, buffer)| buffer.path().map(Path::to_path_buf))
            .collect();
        let worktrees = &self.hosting.worktrees;
        let briefs = worktrees
            .briefs
            .values()
            .chain(worktrees.creating.values())
            .map(|brief| brief.place.clone())
            .collect();
        let sessions = self.hosting.table.all();
        Snapshot {
            unsaved,
            briefs,
            sessions: running_sessions(sessions, &self.watch),
            hosted: hosted_pids(sessions),
        }
    }

    /// The name of the space that holds the worktree at `path`, if one does.
    fn space_of(&self, path: &Path) -> Option<String> {
        let spaces = self.watch.spaces();
        let at = spaces.holding(path)?;
        spaces.get(at).map(|space| space.name().to_owned())
    }
}

/// The worktree a listing's `report` found clean with its branch merged, a row or `gh`'s word
/// on one: the listing's token, the path, and when the job read its changes.
fn merged_work_at(report: &Report) -> Option<(u64, PathBuf, Instant)> {
    match report {
        Report::Row { token, looked } if looked.work_merged() => {
            Some((*token, looked.listed.path.clone(), looked.looked_at))
        }
        Report::Merged {
            token,
            path,
            merged,
            clean: Some(looked_at),
        } if merged.is_merged() => Some((*token, path.clone(), *looked_at)),
        _ => None,
    }
}

/// A hosted session's state as the UI thread knows it: a running session's as the sessions
/// worker last told it (ADR 0121 step 3), while that agrees that it runs. `None` while a
/// session starts or its live state is not known: a session that runs then counts as busy.
pub(in crate::app) fn hosted_state(session: &Hosted, watch: &Watch) -> Option<State> {
    match session.run {
        Run::Stopped => Some(State::Stopped),
        Run::Failed => Some(State::Failed),
        Run::Starting => None,
        Run::Running { .. } => watch
            .state(session.key)
            .filter(|state| !matches!(state, State::Stopped | State::Failed)),
    }
}

/// Whether a session in `state` is settled for check 4, and is stopped by a removal whose checks
/// hold: idle, done or ready for review. A removal's checks 2 and 5 hold only for work committed
/// and merged (ADR 0121, "Merged work is not ready for review, as amended").
pub(in crate::app) fn settled(state: Option<State>) -> bool {
    matches!(
        state,
        Some(State::Idle | State::Done | State::ReadyForReview)
    )
}

/// The hosted sessions that run, neither failed nor stopped, each with whether it is
/// [`settled`]: check 4's half that the UI thread knows.
fn running_sessions(sessions: &[Hosted], watch: &Watch) -> Vec<HostedSession> {
    sessions
        .iter()
        .filter(|session| session.runs())
        .filter_map(|session| {
            let state = hosted_state(session, watch);
            if matches!(state, Some(State::Failed | State::Stopped)) {
                return None;
            }
            Some(HostedSession {
                directory: session.directory.clone(),
                name: session.name.clone(),
                pid: match session.run {
                    Run::Running { child } => child,
                    _ => None,
                },
                settled: settled(state),
            })
        })
        .collect()
}

/// The process ids of the `claude`s oso hosts.
fn hosted_pids(sessions: &[Hosted]) -> Vec<u32> {
    sessions
        .iter()
        .filter_map(|session| match session.run {
            Run::Running { child } => child,
            _ => None,
        })
        .collect()
}
