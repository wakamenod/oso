//! Git status buffers (ADR 0127): each one's record, the text the host builds for it off the UI
//! thread, and what commands ask of the host for them.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads, files or processes; the
//! host runs git on its workers and hands each status text in whole.
//!
//! - A status buffer is an internal, read-only buffer in `GitStatus` mode that does not wrap,
//!   one for each project, `*git: NAME*`. As magit's are, and as a directory's listing is, it is
//!   listed by `C-x b` and killed by `C-x k`.
//! - Its text comes whole, with its display map, and goes in at O(1), as a file's load does.
//! - What a line names (a section, a file, a hunk) is the host's: the editor keeps only the
//!   outline, the line and depth of each heading, which the commands that move between headings
//!   read, and a command sends the lines it means.
//! - A status is named as given, told apart from another buffer of that name by `<2>`, `<3>`…
//! - A commit's message is the person's to write, in a buffer of [`Mode::GitCommit`]; what the
//!   commit menu's head asks for comes to the host with the switches that were on (step 1b).
//! - What git's long runs wrote is a log of [`Mode::GitLog`], which the host adds to at its
//!   end and which keeps the last [`MAX_GIT_LOG`] bytes, in whole lines. The host notes while
//!   git runs, so that quitting names it.
//! - A repository's commits show as a graph, an internal buffer of [`Mode::GitGraph`] that the
//!   host fills as a status, one per project, `*graph: NAME*` (step 4).

use super::Editor;
use crate::buffer::{Buffer, BufferId, Mode};
use crate::load::LoadSnapshot;
use crate::project::ProjectId;
use crate::selection::SelectionSet;
use crate::syntax::Highlights;
use crate::window::WindowId;

/// The lines one selection covers in a status buffer: from `first` to `last`, both included,
/// and whether it is a region, which means its lines, or a cursor, which means what it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitLines {
    pub first: usize,
    pub last: usize,
    pub region: bool,
}

/// A heading of a status buffer's outline: a section's, a file's or a hunk's, at `line`, with
/// its depth, 0 for a section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GitHeading {
    pub line: usize,
    pub depth: u8,
}

/// The most bytes a log of git's runs keeps (ADR 0127): older lines go first.
pub const MAX_GIT_LOG: usize = 4 * 1024 * 1024;

/// Which commit a head of the commit menu makes (ADR 0127, step 1b).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitKind {
    /// A new commit of what is staged (`c`).
    New,
    /// `HEAD` made again with what is staged, its message edited (`a`).
    Amend,
    /// `HEAD` made again with what is staged, its message kept (`e`).
    Extend,
    /// `HEAD`'s message alone, edited (`w`).
    Reword,
    /// A `fixup!` commit of what is staged for the commit at point, made at once (`f`, step 2).
    Fixup,
    /// A `squash!` commit of what is staged for the commit at point, its message edited (`s`,
    /// step 2).
    Squash,
}

/// The commit menu's switches that were on as its head ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitSwitches {
    /// `--no-verify`: the `pre-commit` and `commit-msg` hooks do not run.
    pub no_verify: bool,
    /// `--signoff`.
    pub signoff: bool,
    /// `--allow-empty`.
    pub allow_empty: bool,
}

/// Where the push menu's head pushes the branch checked out (ADR 0127, step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushTarget {
    /// git's own push target, `@{push}` (`p`); with none, a remote is read in a prompt.
    PushRemote,
    /// The upstream (`u`); with none, a remote is read in a prompt and set as it.
    Upstream,
    /// A remote's branch read in a prompt (`e`).
    Elsewhere,
    /// What the prompt one of the others opened read: a remote, or `REMOTE/BRANCH`.
    Named(String),
    /// What the question before a forced push asked about, after the person's `y`.
    Asked,
}

/// The push menu's switches that were on as its head ran.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushSwitches {
    /// `--force-with-lease`, with the remote's tip as oso last saw it, after a question.
    pub force_with_lease: bool,
    /// `--no-verify`: the `pre-push` hook does not run.
    pub no_verify: bool,
}

/// What the fetch menu's head fetches from (ADR 0127, step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchFrom {
    /// The upstream's remote (`u`).
    Upstream,
    /// Every remote (`a`).
    All,
}

/// How the pull menu's head pulls (ADR 0127, step 3a).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PullHow {
    /// Fast-forward only, with no switch on.
    FfOnly,
    /// Rebase the branch onto what came (`-r`).
    Rebase,
    /// Fast-forward, else a merge commit with git's message (`-m`).
    Merge,
}

/// What the stash menu's head asks, or what a prompt or a question it opened gave back (ADR
/// 0127, step 3b).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StashAction {
    /// Stash the changes, untracked files too if `untracked` (`z z`); a message is read first.
    Push { untracked: bool },
    /// The message the prompt `Push` opened read, empty for git's own.
    Message(String),
    /// Apply the stash at point, keeping it (`z a`).
    Apply,
    /// Apply the stash at point and drop it (`z p`).
    Pop,
    /// Drop the stash at point, after a question (`z k`).
    Drop,
    /// The stash the prompt one of the others opened read, as `stash@{N}`.
    Picked(String),
    /// What the question before dropping asked about, after the person's `y`.
    Confirmed,
}

/// What the branch menu's head asks, or what a prompt or a question it opened gave back (ADR
/// 0127, step 3b).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BranchAction {
    /// Check out a branch read in a prompt, local or a remote's (`b b`).
    Checkout,
    /// Make a branch at a revision and name read in prompts, checking it out if `checkout`
    /// (`b c`, `b n`).
    Create { checkout: bool },
    /// Rename a branch read in a prompt (`b m`).
    Rename,
    /// Delete a branch read in a prompt, asking first if it is not merged (`b k`).
    Delete,
    /// The branch or revision the prompt one of the others opened read.
    Picked(String),
    /// The new name the prompt one of the others opened read.
    Named(String),
    /// What the question before a forced delete asked about, after the person's `y`.
    Confirmed,
}

/// Which commits a graph shows (ADR 0127, step 4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GraphScope {
    /// The branch checked out, or `HEAD` (`l l`, `C-x v l`).
    Current,
    /// Every branch, remote branch and tag (`l a`).
    All,
    /// A ref read in a prompt over the branches, `HEAD` first (`l o`).
    Pick,
    /// What that prompt read: a ref it listed, or any revision typed.
    Named(String),
}

/// What a key asks of the commit on a graph's line, or of the whole graph (ADR 0127, step 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GraphAct {
    /// Review the commit, read-only (`RET`).
    Review,
    /// Copy its full id (`w`).
    CopyId,
    /// Read the graph again (`g`).
    Refresh,
}

/// What a command asks of the host for git (ADR 0127).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitRequest {
    /// Show the status buffer of `project` in the selected window, read again: `C-x g`.
    Status { project: ProjectId },
    /// Read the status again: `g`.
    Refresh { buffer: BufferId },
    /// Show or hide what the heading at or above `line` holds: a file's hunks, or a section's
    /// rows (`TAB`).
    Toggle { buffer: BufferId, line: usize },
    /// Stage what each of `lines` is on (`s`).
    Stage {
        buffer: BufferId,
        lines: Vec<GitLines>,
    },
    /// Unstage what each of `lines` is on (`u`).
    Unstage {
        buffer: BufferId,
        lines: Vec<GitLines>,
    },
    /// Stage every unstaged change shown (`S`).
    StageAll { buffer: BufferId },
    /// Unstage every staged change shown (`U`).
    UnstageAll { buffer: BufferId },
    /// Visit the file `line` is about, at the line of the file it shows, if any (`RET`).
    Visit {
        buffer: BufferId,
        line: usize,
        column: usize,
    },
    /// Make the commit `kind` in the repository of the status `buffer`, with `switches` (the
    /// commit menu's heads); a fixup or a squash is for the commit on `line`, point's.
    Commit {
        buffer: BufferId,
        kind: CommitKind,
        switches: CommitSwitches,
        line: usize,
    },
    /// Commit with the message `buffer` holds (`C-c C-c`).
    CommitFinish { buffer: BufferId },
    /// Drop the commit's message `buffer`, keeping its text for `M-p` (`C-c C-k`).
    CommitCancel { buffer: BufferId },
    /// Put an older (`M-p`) or a newer (`M-n`) message of those kept in `buffer`, a commit's
    /// message.
    CommitRing { buffer: BufferId, older: bool },
    /// Show the log of git's runs in the repository of `buffer`, a status (`$`).
    ShowLog { buffer: BufferId },
    /// Stop what git runs in the repository of `buffer`, a status or its log (`C-g`, `k`).
    Stop { buffer: BufferId },
    /// Discard what each of `lines` is on in the status `buffer` (`k`), after the person's
    /// `y` to the question it asks (`confirmed`), keeping a copy first (step 1c).
    Discard {
        buffer: BufferId,
        lines: Vec<GitLines>,
        confirmed: bool,
    },
    /// Write back the copy at `line` of the list of what discards kept, `buffer` (`RET`), after
    /// the person's `y` (`confirmed`).
    RestoreDiscarded {
        buffer: BufferId,
        line: usize,
        confirmed: bool,
    },
    /// Show what discards kept in the repository of `buffer`: a status, that list, or a
    /// buffer of the project (`M-x git-discarded`, `g` in the list).
    ShowDiscarded { buffer: BufferId },
    /// Ask the agent session of the repository's worktree for a draft of the commit's message
    /// `buffer`, which it gives back with the session tool `draft_set` (`C-c C-d`, step 6a).
    DraftMessage { buffer: BufferId },
    /// Ask the agent session of the repository's worktree to fix what made its last commit
    /// fail, from `buffer`, that commit's message or the log (`C-c C-f`, `F`, step 6a).
    FixWithAgent { buffer: BufferId },
    /// Push the branch checked out in the repository of the status `buffer` to `target`, with
    /// `switches` (the push menu's heads, step 2).
    Push {
        buffer: BufferId,
        target: PushTarget,
        switches: PushSwitches,
    },
    /// Pull the upstream into the branch checked out, as `how` says (`F u`, step 2; the
    /// switches from step 3a).
    Pull { buffer: BufferId, how: PullHow },
    /// Fetch from `from`, pruning what the remote deleted (`f u`, `f a`, step 2).
    Fetch { buffer: BufferId, from: FetchFrom },
    /// Go on with the merge, rebase, pick, revert or `git am` stopped in the repository of the
    /// status `buffer` (`C-c C-c`, step 3a).
    Continue { buffer: BufferId },
    /// Skip the commit the rebase, pick, revert or `git am` stopped at, after the person's `y`
    /// to the question it asks (`confirmed`) (`C-c C-s`, step 3a).
    Skip { buffer: BufferId, confirmed: bool },
    /// Abort what is stopped, after the person's `y` to the question it asks (`confirmed`)
    /// (`C-c C-k`, step 3a).
    Abort { buffer: BufferId, confirmed: bool },
    /// What the stash menu's head `action` asks in the repository of the status `buffer`, for
    /// the stash on `line`, point's (step 3b).
    Stash {
        buffer: BufferId,
        action: StashAction,
        line: usize,
    },
    /// What the branch menu's head `action` asks in the repository of the status `buffer`
    /// (step 3b).
    Branch {
        buffer: BufferId,
        action: BranchAction,
    },
    /// Show the graph of `project`'s commits that `scope` names, in its graph buffer (step 4).
    Graph {
        project: ProjectId,
        scope: GraphScope,
    },
    /// Show every ref in the graph `buffer`, or the branch checked out again (`a`).
    GraphToggle { buffer: BufferId },
    /// What `act` asks of the commit on `line` of the graph `buffer`.
    GraphAct {
        buffer: BufferId,
        act: GraphAct,
        line: usize,
    },
}

/// A status buffer's record, or a graph buffer's (step 4): which it is, its buffer's mode says.
#[derive(Debug)]
struct Status {
    project: ProjectId,
    outline: Vec<GitHeading>,
}

/// The status buffers and what commands asked for them.
#[derive(Debug, Default)]
pub(super) struct Statuses {
    records: Vec<(BufferId, Status)>,
    requests: Vec<GitRequest>,
    /// The logs of the repositories git runs in now.
    running: Vec<BufferId>,
}

/// What [`Editor::set_git_text`] puts in beside the text.
#[derive(Debug)]
pub struct GitShown {
    /// The highlights, built with the text, covering it whole.
    pub highlights: Highlights,
    pub outline: Vec<GitHeading>,
    /// The line a window's cursor goes to unless `lines` says.
    pub line: usize,
    /// The line each window's cursor goes to.
    pub lines: Vec<(WindowId, usize)>,
}

impl Editor {
    /// Asks the host for `request`.
    pub fn request_git(&mut self, request: GitRequest) {
        self.git.requests.push(request);
    }

    /// What commands asked of the host for git, oldest first.
    pub fn take_git_requests(&mut self) -> Vec<GitRequest> {
        std::mem::take(&mut self.git.requests)
    }

    /// Adds an empty status buffer for `project`, named `name`, shown in no window yet, which
    /// does not wrap.
    pub fn add_git_status(&mut self, project: ProjectId, name: impl Into<String>) -> BufferId {
        self.add_git_buffer(project, name.into(), Mode::GitStatus)
    }

    /// Adds an empty graph buffer for `project`, named `name`, shown in no window yet, which
    /// does not wrap (step 4).
    pub fn add_git_graph(&mut self, project: ProjectId, name: impl Into<String>) -> BufferId {
        self.add_git_buffer(project, name.into(), Mode::GitGraph)
    }

    /// The project of the graph buffer `buffer`, if it is one and is still there.
    pub fn git_graph_of(&self, buffer: BufferId) -> Option<ProjectId> {
        is_git_graph(self.buffer(buffer)?).then_some(())?;
        self.git_record(buffer).map(|status| status.project)
    }

    /// The graph buffer of `project`, if one is open.
    pub fn git_graph_buffer(&self, project: ProjectId) -> Option<BufferId> {
        (self.git.records.iter())
            .filter(|(id, _)| self.buffer(*id).is_some_and(is_git_graph))
            .find(|(_, status)| status.project == project)
            .map(|(id, _)| *id)
    }

    /// Adds an internal buffer of `mode` for `project`, named `name`, told apart from another of
    /// that name by `<2>`, `<3>`…, which does not wrap, and its record.
    fn add_git_buffer(&mut self, project: ProjectId, name: String, mode: Mode) -> BufferId {
        let mut unique = name.clone();
        let mut n = 1;
        while self.buffer_named(&unique).is_some() {
            n += 1;
            unique = format!("{name}<{n}>");
        }
        let id = self.add_internal_buffer(unique, mode);
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_wraps(false);
        }
        let outline = Vec::new();
        (self.git.records).push((id, Status { project, outline }));
        id
    }

    /// The project of the status buffer `buffer`, if it is one and is still there.
    pub fn git_status_of(&self, buffer: BufferId) -> Option<ProjectId> {
        is_git_status(self.buffer(buffer)?).then_some(())?;
        self.git_record(buffer).map(|status| status.project)
    }

    /// The status buffer of `project`, if one is open.
    pub fn git_status_buffer(&self, project: ProjectId) -> Option<BufferId> {
        self.git_statuses()
            .find(|(_, of)| *of == project)
            .map(|(id, _)| id)
    }

    /// Every status buffer still there, with its project.
    pub fn git_statuses(&self) -> impl Iterator<Item = (BufferId, ProjectId)> {
        (self.git.records.iter())
            .filter(|(id, _)| self.buffer(*id).is_some_and(is_git_status))
            .map(|(id, status)| (*id, status.project))
    }

    /// The headings of the status buffer `buffer`, in order: empty for any other buffer.
    pub fn git_outline(&self, buffer: BufferId) -> &[GitHeading] {
        self.git_record(buffer)
            .map_or(&[], |status| status.outline.as_slice())
    }

    /// Each window's id and the buffer line its cursor is on, for the windows that show the
    /// status buffer `id`, so that the host can tell what each is on before it puts in a new
    /// text.
    pub fn git_points(&self, id: BufferId) -> Vec<(WindowId, usize)> {
        self.cursor_rows(id)
    }

    /// Puts in the whole text of the status buffer `id`, or of a graph buffer, built off the UI
    /// thread, with its
    /// outline and highlights. O(1) for the text: each window that shows it has its cursor at
    /// the start of the line `lines` gives for it, else of `line`, and a window that will come
    /// back to it at `line` too. No undo. The host keeps the highlights given back, which the
    /// new ones replaced or which did not fit, to free them off the UI thread.
    pub fn set_git_text(
        &mut self,
        id: BufferId,
        text: LoadSnapshot,
        shown: GitShown,
    ) -> Option<Highlights> {
        let GitShown {
            highlights,
            outline,
            line,
            lines,
        } = shown;
        if self.git_record(id).is_none() {
            return Some(highlights);
        }
        let fits = |buffer: &&mut Buffer| is_git_status(buffer) || is_git_graph(buffer);
        let Some(buffer) = self.buffer_mut(id).filter(fits) else {
            return Some(highlights);
        };
        // Taken first, as the load would drop them here, on the UI thread.
        let old = buffer.take_generated_highlights();
        buffer.apply_load(text);
        let refused = if buffer.text().len() == highlights.len() {
            buffer.set_generated_highlights(highlights).or(old)
        } else {
            Some(highlights)
        };
        let Some(buffer) = self.buffer(id) else {
            return refused;
        };
        let text = buffer.text();
        let start_of = |line: usize| text.line_start(line.min(text.max_point().row));
        let default = start_of(line);
        let placed: Vec<(WindowId, usize)> = lines
            .iter()
            .map(|(window, line)| (*window, start_of(*line)))
            .collect();
        for window in &mut self.windows {
            if let Some(place) = window.places_mut().get_mut(id) {
                place.selections = SelectionSet::new(default);
            }
            if window.buffer() != id {
                continue;
            }
            let at = placed
                .iter()
                .find(|(shown, _)| *shown == window.id())
                .map_or(default, |(_, at)| *at);
            window.selections = SelectionSet::new(at);
        }
        if let Some((_, status)) = self.git.records.iter_mut().find(|(of, _)| *of == id) {
            status.outline = outline;
        }
        self.loaded(id);
        if self.window().buffer() == id {
            self.follow_cursor();
        }
        refused
    }

    /// Adds `text` at the end of the log of git's runs `id`, an internal buffer of
    /// [`Mode::GitLog`], taking out the oldest lines past [`MAX_GIT_LOG`] bytes. Cursors at the
    /// end stay at the end. Nothing happens to another buffer.
    pub fn append_git_log(&mut self, id: BufferId, text: &str) {
        let Some(buffer) = self.buffer(id) else {
            return;
        };
        if !is_git_log(buffer) || text.is_empty() {
            return;
        }
        let rope = buffer.text();
        let len = rope.len();
        // A text past the whole keeps its last lines alone.
        let text = if text.len() > MAX_GIT_LOG {
            let from = text.ceil_char_boundary(text.len() - MAX_GIT_LOG);
            let from = text[from..].find('\n').map_or(from, |at| from + at + 1);
            &text[from..]
        } else {
            text
        };
        let over = (len + text.len()).saturating_sub(MAX_GIT_LOG);
        let oldest = if over == 0 {
            0
        } else if over >= len {
            len
        } else {
            let row = rope.offset_to_point(over).row;
            let start = rope.line_start(row);
            if start == over {
                start
            } else {
                rope.line_start(row + 1).max(over)
            }
        };
        self.trim_and_append(id, oldest, text);
    }

    /// Notes whether git runs in the repository whose log is `log` (ADR 0127): `C-x C-c` names
    /// it, and quitting when idle waits for it.
    pub fn set_git_running(&mut self, log: BufferId, running: bool) {
        self.git.running.retain(|id| *id != log);
        if running {
            self.git.running.push(log);
        }
    }

    /// The logs of the repositories git runs in now, still there, in the order the runs began.
    pub fn git_runs(&self) -> impl Iterator<Item = BufferId> + '_ {
        (self.git.running.iter().copied()).filter(|id| self.buffer(*id).is_some())
    }

    /// Drops the record of the status buffer `id`, as its buffer goes.
    pub(super) fn forget_git_status(&mut self, id: BufferId) {
        self.git.records.retain(|(of, _)| *of != id);
    }

    fn git_record(&self, id: BufferId) -> Option<&Status> {
        (self.git.records.iter())
            .find(|(of, _)| *of == id)
            .map(|(_, status)| status)
    }
}

/// Whether `buffer` is a status buffer.
pub(super) fn is_git_status(buffer: &Buffer) -> bool {
    buffer.is_internal() && buffer.mode() == Mode::GitStatus
}

/// Whether `buffer` is a graph buffer (step 4).
pub(super) fn is_git_graph(buffer: &Buffer) -> bool {
    buffer.is_internal() && buffer.mode() == Mode::GitGraph
}

/// Whether `buffer` is a log of git's runs, which `C-x b` lists and `C-x k` kills as a status.
pub(super) fn is_git_log(buffer: &Buffer) -> bool {
    buffer.is_internal() && buffer.mode() == Mode::GitLog
}

#[cfg(test)]
mod tests;
