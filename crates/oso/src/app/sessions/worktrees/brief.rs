//! Writing a worktree session's brief (ADR 0121 step 4): `session-worktree` opens `*Brief*`,
//! `C-c C-c` makes the worktree through a create job and starts its session with the brief, and
//! `C-c C-k` drops it. An issue's `w` opens it with the issue's brief (ADR 0127, step 5b-1).
//!
//! Must not depend on: key presses or how a frame is drawn; the worktree is made by a job on
//! the `oso-sessions-git` threads, never here.

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::project::ProjectId;

use super::super::super::App;
use super::super::hosting::{Begin, Then};
use super::{BRIEF, Brief};
use crate::app::sessions::withheld_draft;
use crate::git::{TrustedRoot, printable};
use crate::sessions::worktree::brief::MAX_BRIEF;
use crate::sessions::worktree::create::{Created, Handover, Order, Step};
use crate::sessions::worktree::slug;

impl App {
    /// `session-worktree`: opens `*Brief*` for a worktree on `branch`.
    pub(super) fn open_brief(&mut self, branch: String, base: Option<String>) {
        match self.project_here() {
            Ok(project) => self.start_brief(project, branch, base, ""),
            Err(why) => self.editor.message(why),
        }
    }

    /// Opens `*Brief*` for a worktree session of `project` on `branch`, from `base` or `HEAD`,
    /// starting from `text`: empty for `session-worktree`'s, an issue's for its `w` (ADR 0127,
    /// step 5b-1).
    pub(in crate::app) fn start_brief(
        &mut self,
        project: ProjectId,
        branch: String,
        base: Option<String>,
        text: &str,
    ) {
        let Some(root) = self
            .editor
            .project(project)
            .map(|found| found.root().to_owned())
        else {
            return;
        };
        let open = self.hosting.worktrees.briefs.keys().next().copied();
        if let Some(open) = open {
            self.editor.show(open);
            return self
                .editor
                .message("A brief is being written: send it with C-c C-c or drop it with C-c C-k");
        }
        // From a row of `*Spaces*`, its session and its worktree's branch (ADR 0131).
        let row = self.sessions_row_place().and_then(Result::ok);
        if let Some(row) = &row {
            self.enter_row_space(row);
        }
        let base = base.or_else(|| row.as_ref()?.base().map(str::to_owned));
        let detached = if base.is_none() && row.as_ref().is_some_and(|row| row.detached()) {
            " (that worktree is on no branch: this one starts at HEAD)"
        } else {
            ""
        };
        let starting = self.editor.window().buffer();
        let session = match &row {
            Some(row) => row.session.and_then(|key| self.hosting.table.get(key)),
            None => self.hosting.table.by_buffer(starting),
        };
        let from = session.map(|session| Handover {
            name: session.runs().then(|| session.name.clone()),
            id: session.id.as_str().to_owned(),
            directory: session.directory.clone(),
            space: self.orchestrated_space(&session.id),
            reads: Vec::new(),
        });
        let parent = session.map(|s| s.id.clone());
        let place = slug::slug(&branch)
            .map(|slug| root.join(".claude/worktrees").join(slug))
            .unwrap_or_default();
        let id = withheld_draft(&mut self.editor, BRIEF, Mode::Brief, text);
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
            "Write the brief for {shown}{detached}: C-c C-c makes the worktree and starts its session, C-c C-k drops it"
        ));
    }

    /// `brief-send`: makes the worktree, then starts its session with the brief.
    pub(super) fn send_brief(&mut self, buffer: BufferId) {
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
    pub(super) fn worktree_step(&mut self, token: u64, step: Step) {
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
    pub(super) fn cancel_brief(&mut self, buffer: BufferId) {
        if self.hosting.worktrees.briefs.remove(&buffer).is_some() {
            self.editor.kill_buffer(buffer);
            self.editor.message("Brief dropped");
        }
    }

    /// A create job ended: its session starts, or why not is said.
    pub(super) fn worktree_created(
        &mut self,
        token: u64,
        result: Result<(Created, TrustedRoot), String>,
    ) {
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
        // The worktree is held by the space of the session that asked for it, when its
        // repository is one of that space's, else by the space it joins (ADR 0136): its
        // session then joins the same.
        self.hold_made_worktree(brief.parent.as_ref(), &created.path, &created.main);
        let begin = Begin {
            origin: crate::sessions::identity::Origin::Person,
            name: None,
            directory: Some(created.path),
            brief: Some(created.prompt),
            parent: brief.parent,
            check_shared: false,
            trusted: Some(trusted),
            worktree: true,
            space: None,
            behind: false,
        };
        self.begin_session(begin, Then::Nothing);
    }
}
