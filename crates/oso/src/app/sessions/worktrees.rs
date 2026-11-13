//! Worktree sessions in the app (ADR 0121 step 4): what the worktree commands ask, carried out
//! on the UI thread. `session-worktree` lists the project's branches, opens `*Brief*`, and on
//! `C-c C-c` makes the worktree and starts its session. `worktree-list` fills `*Worktrees*` row
//! by row. `worktree-remove` checks first, asks, and removes only a worktree nothing would be
//! lost from. Every git and `gh` call is a job on the `oso-sessions-git` threads.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::command::Effects;
use oso_core::editor::SessionRequest;

use super::super::{App, Loop, UserEvent};

mod listing;
mod removal;
use super::hosting::Begin;
use crate::review::TrustedRoot;
use crate::sessions::hosted::Run;
use crate::sessions::identity::SessionId;
use crate::sessions::worktree::create::{From, Order};
use crate::sessions::worktree::jobs::{Jobs, Report, Snapshot};
use crate::sessions::worktree::list::{Row, Shown};
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
}

/// A worktree session's brief, before and while its worktree is made.
#[derive(Debug, Clone)]
struct Brief {
    root: TrustedRoot,
    branch: String,
    base: Option<String>,
    from: Option<From>,
    parent: Option<SessionId>,
    /// Where its worktree goes, for the check of unsent briefs.
    place: PathBuf,
    buffer: BufferId,
}

#[derive(Debug)]
struct Listing {
    token: u64,
    buffer: BufferId,
    root: TrustedRoot,
    rows: Vec<Row>,
    pending: usize,
    shown: Shown,
}

#[derive(Debug)]
struct Removing {
    token: u64,
    root: TrustedRoot,
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
    fn worktree_jobs(&mut self) -> Jobs {
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

    fn worktree_token(&mut self) -> u64 {
        self.hosting.worktrees.next += 1;
        self.hosting.worktrees.next
    }

    /// The selected buffer's project's root, if it is trusted to run git.
    fn trusted_here(&self) -> Result<TrustedRoot, String> {
        self.editor
            .current()
            .1
            .project()
            .and_then(|id| TrustedRoot::of(&self.editor, id))
            .ok_or_else(|| {
                "Worktree sessions need a trusted project's buffer: trust-project first".to_owned()
            })
    }

    /// Lists the current project's branches for the command that waits for them. The answer
    /// comes as a job's report; a refusal is answered at once.
    pub(in crate::app) fn list_branches_for_prompt(&mut self) -> Effects {
        match self.trusted_here() {
            Ok(root) => {
                self.worktree_jobs().branches(root);
                Effects::default()
            }
            Err(why) => self
                .dispatcher
                .branches_answered(&mut self.editor, Err(why)),
        }
    }

    /// A worktree job reported.
    pub(in crate::app) fn worktree_reported(&mut self, event_loop: &Loop<'_>, report: Report) {
        match report {
            Report::Branches { branches, .. } => {
                let effects = self
                    .dispatcher
                    .branches_answered(&mut self.editor, branches);
                self.apply(event_loop, effects);
            }
            Report::Step { token, step } => self.worktree_step(token, step),
            Report::Created { token, result } => self.worktree_created(token, result),
            Report::Listed { token, result } => self.worktrees_listed(token, result),
            Report::Row { token, looked } => self.worktree_row(token, *looked),
            Report::Checked {
                token,
                path,
                result,
            } => self.worktree_checked(token, &path, result),
            Report::Removed {
                token,
                path,
                result,
            } => self.worktree_removed(token, &path, result),
        }
        self.request_redraw();
    }

    /// `session-worktree`: opens `*Brief*` for a worktree on `branch`.
    fn open_brief(&mut self, branch: String, base: Option<String>) {
        let root = match self.trusted_here() {
            Ok(root) => root,
            Err(why) => return self.editor.message(why),
        };
        if let Some(open) = self.hosting.worktrees.briefs.keys().next().copied() {
            self.editor.show(open);
            return self
                .editor
                .message("A brief is being written: send it with C-c C-c or drop it with C-c C-k");
        }
        let starting = self.editor.window().buffer();
        let from = self.hosting.table.by_buffer(starting).map(|session| From {
            name: session.runs().then(|| session.name.clone()),
            id: session.id.as_str().to_owned(),
            directory: session.directory.clone(),
        });
        let parent = self.hosting.table.by_buffer(starting).map(|s| s.id.clone());
        let place = slug::slug(&branch)
            .map(|slug| root.path().join(".claude/worktrees").join(slug))
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
                root,
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
        let Some(text) = self.editor.buffer(buffer).map(|b| b.text().to_string()) else {
            return;
        };
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
        let token = self.worktree_token();
        let order = Order {
            branch: brief.branch.clone(),
            base: brief.base.clone(),
            brief: text,
            from: brief.from.clone(),
        };
        self.worktree_jobs()
            .create(token, brief.root.clone(), order);
        self.editor.message(format!(
            "Making the worktree for {}…",
            printable(&brief.branch, 80)
        ));
        self.hosting.worktrees.creating.insert(token, brief);
    }

    /// A create job reached `step`: the echo area says what comes next.
    fn worktree_step(&mut self, token: u64, step: crate::sessions::worktree::create::Step) {
        use crate::sessions::worktree::create::Step;
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
    fn worktree_created(
        &mut self,
        token: u64,
        result: Result<crate::sessions::worktree::create::Created, String>,
    ) {
        let Some(brief) = self.hosting.worktrees.creating.remove(&token) else {
            return;
        };
        let created = match result {
            Ok(created) => created,
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
        let trusted = brief.root.for_worktree(created.path.clone());
        self.begin_session(Begin {
            name: None,
            directory: Some(created.path),
            brief: Some(created.prompt),
            parent: brief.parent,
            check_shared: false,
            trusted: Some(trusted),
        });
    }

    /// What the UI thread holds that a worktree job cannot read: buffers with unsaved
    /// changes, unsent briefs and the hosted sessions.
    fn worktree_snapshot(&self) -> Snapshot {
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
        // Until the states come (ADR 0121 step 3), a session that runs counts as busy.
        let busy = sessions
            .iter()
            .filter(|session| session.runs())
            .map(|session| (session.directory.clone(), session.name.clone()))
            .collect();
        let hosted = sessions
            .iter()
            .filter_map(|session| match session.run {
                Run::Running { child } => child,
                _ => None,
            })
            .collect();
        Snapshot {
            unsaved,
            briefs,
            busy,
            hosted,
        }
    }
}
