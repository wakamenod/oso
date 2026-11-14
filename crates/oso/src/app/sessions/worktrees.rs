//! Worktree sessions in the app (ADR 0121 step 4): what the worktree commands ask, carried out
//! on the UI thread. `session-worktree` lists the project's branches, opens `*Brief*`, and on
//! `C-c C-c` makes the worktree and starts its session. `worktree-list` fills `*Worktrees*` row
//! by row. `worktree-remove` checks first, asks, and removes only a worktree nothing would be
//! lost from. Every git and `gh` call is a job on the `oso-sessions-git` threads. A worktree
//! session's space opens, nested under its repository's, as its session starts.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! What a job needs from the UI thread goes with it; nothing here touches the disk. Each job
//! takes the project's trust as it is when the job is asked for, not as it was when `*Brief*`
//! or `*Worktrees*` opened.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::command::Effects;
use oso_core::editor::SessionRequest;
use oso_core::project::ProjectId;

use super::super::{App, Loop, UserEvent};

mod listing;
mod removal;
use super::hosting::Begin;
use super::watch::Watch;
use crate::review::TrustedRoot;
use crate::sessions::hosted::{Hosted, Run};
use crate::sessions::identity::SessionId;
use crate::sessions::state::State;
use crate::sessions::worktree::brief::MAX_BRIEF;
use crate::sessions::worktree::checks::{Held, HostedSession};
use crate::sessions::worktree::create::{Created, Handover, Order, Step};
use crate::sessions::worktree::jobs::{Jobs, Report, Snapshot, Trust};
use crate::sessions::worktree::list::{Row, RowText, Shown};
use crate::sessions::worktree::porcelain::printable;
use crate::sessions::worktree::slug;

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
            Report::Step { token, step } => self.worktree_step(token, step),
            Report::Created { token, result } => self.worktree_created(token, result),
            Report::Listed { token, result } => self.worktrees_listed(token, result),
            Report::Row { token, looked } => self.worktree_row(token, *looked),
            Report::Merged {
                token,
                path,
                merged,
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
            Report::ListEnded { .. } | Report::Prepared { .. } => {}
        }
        self.request_redraw();
    }

    /// `session-worktree`: opens `*Brief*` for a worktree on `branch`.
    fn open_brief(&mut self, branch: String, base: Option<String>) {
        let (project, root) = match self.project_here() {
            Ok(id) => match self.editor.project(id) {
                Some(found) => (id, found.root().to_owned()),
                None => return,
            },
            Err(why) => return self.editor.message(why),
        };
        if let Some(open) = self.hosting.worktrees.briefs.keys().next().copied() {
            self.editor.show(open);
            return self
                .editor
                .message("A brief is being written: send it with C-c C-c or drop it with C-c C-k");
        }
        let starting = self.editor.window().buffer();
        let from = self
            .hosting
            .table
            .by_buffer(starting)
            .map(|session| Handover {
                name: session.runs().then(|| session.name.clone()),
                id: session.id.as_str().to_owned(),
                directory: session.directory.clone(),
            });
        let parent = self.hosting.table.by_buffer(starting).map(|s| s.id.clone());
        let place = slug::slug(&branch)
            .map(|slug| root.join(".claude/worktrees").join(slug))
            .unwrap_or_default();
        let mut buffer = Buffer::from_text(BRIEF, "");
        buffer.set_mode(Mode::Brief);
        // The person's brief: no agent reads or rewrites it before it is sent.
        buffer.withhold_from_agents();
        let id = self.editor.add_buffer(buffer);
        self.editor.show(id);
        let shown = printable(&branch, 80);
        self.hosting.worktrees.briefs.insert(
            id,
            Brief {
                project,
                branch,
                base,
                from,
                parent,
                place,
                buffer: id,
            },
        );
        self.editor.message(format!(
            "Write the brief for {shown}: C-c C-c makes the worktree and starts its session, C-c C-k drops it"
        ));
    }

    /// `brief-send`: makes the worktree, then starts its session with the brief.
    fn send_brief(&mut self, buffer: BufferId) {
        let Some(brief) = self.hosting.worktrees.briefs.get(&buffer).cloned() else {
            return self
                .editor
                .message("This brief belongs to no worktree session");
        };
        let Some(text) = self.editor.buffer(buffer).map(Buffer::text) else {
            return;
        };
        // Measured before it is copied.
        if text.len() > MAX_BRIEF {
            return self.editor.message("The brief is over 64 KiB");
        }
        let text = text.to_string();
        if text.trim().is_empty() {
            return self.editor.message("The brief is empty");
        }
        if self
            .hosting
            .worktrees
            .creating
            .values()
            .any(|c| c.buffer == buffer)
        {
            return self.editor.message("This worktree is being made");
        }
        let trust = match self.trust_for(brief.project) {
            Ok(trust) => trust,
            Err(why) => return self.editor.message(why),
        };
        let token = self.worktree_token();
        let order = Order {
            branch: brief.branch.clone(),
            base: brief.base.clone(),
            brief: text,
            from: brief.from.clone(),
            start_at: None,
        };
        self.worktree_jobs().create(token, trust, order);
        self.editor.message(format!(
            "Making the worktree for {}…",
            printable(&brief.branch, 80)
        ));
        self.hosting.worktrees.creating.insert(token, brief);
    }

    /// A create job reached `step`: the echo area says what comes next.
    fn worktree_step(&mut self, token: u64, step: Step) {
        let Some(brief) = self.hosting.worktrees.creating.get(&token) else {
            return;
        };
        let branch = printable(&brief.branch, 80);
        let said = match step {
            Step::Checked => format!("Adding the worktree for {branch}…"),
            Step::Added => format!("Copying .worktreeinclude's files for {branch}…"),
            Step::Copied => format!("Starting the session for {branch}…"),
        };
        self.editor.message(said);
    }

    /// `brief-cancel`: drops the brief unsent.
    fn cancel_brief(&mut self, buffer: BufferId) {
        if self.hosting.worktrees.briefs.remove(&buffer).is_some() {
            self.editor.kill_buffer(buffer);
            self.editor.message("Brief dropped");
        }
    }

    /// A create job ended: its session starts, or why not is said.
    fn worktree_created(&mut self, token: u64, result: Result<(Created, TrustedRoot), String>) {
        let Some(brief) = self.hosting.worktrees.creating.remove(&token) else {
            return;
        };
        let (created, trusted) = match result {
            Ok(made) => made,
            // The brief stays, to send again.
            Err(why) => return self.editor.message(why),
        };
        self.hosting.worktrees.briefs.remove(&brief.buffer);
        self.editor.kill_buffer(brief.buffer);
        let copied = match (&created.include_note, created.copied.files) {
            (Some(note), _) => format!("; {note}"),
            (None, 0) => String::new(),
            (None, n) => format!("; {n} files copied by .worktreeinclude"),
        };
        let made = if created.made_branch {
            "new branch"
        } else {
            "branch"
        };
        self.editor.message(format!(
            "Worktree {} made on {made} {}{copied}",
            printable(&created.path.display().to_string(), 200),
            printable(&created.branch, 80),
        ));
        // Its space, nested under the repository's (ADR 0121, Q8), among step 3's: the one
        // `*Sessions*` and `space-switch` show, which places the session by its root.
        let spaces = self.watch.spaces_mut();
        spaces.add(&created.main, None);
        spaces.add(&created.path, Some(&created.main));
        self.begin_session(Begin {
            name: None,
            directory: Some(created.path),
            brief: Some(created.prompt),
            parent: brief.parent,
            check_shared: false,
            trusted: Some(trusted),
        });
    }

    /// What the UI thread holds about the worktree at `path` alone: buffers with unsaved
    /// changes under it, an unsent brief for it, and its hosted sessions whose screen's state
    /// is not idle or done. The registry's half of check 4 is the job's to read.
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

    /// The name of the space of the worktree at `path`, if it has one.
    fn space_of(&self, path: &Path) -> Option<String> {
        let spaces = self.watch.spaces();
        let at = spaces.find(path)?;
        spaces.get(at).map(|space| space.name.clone())
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

/// The hosted sessions that run, neither failed nor stopped, each with whether its screen's
/// state is idle or done: check 4's half that the UI thread knows.
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
                settled: matches!(state, Some(State::Idle | State::Done)),
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
