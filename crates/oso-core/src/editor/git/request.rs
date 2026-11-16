//! What git's commands ask of the host (ADR 0127): the request, and the choices each menu's
//! heads and keys carry with it, from the commit menu's kinds to a graph's acts.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads, files or processes; these
//! are plain values a command hands the host.

use crate::buffer::BufferId;
use crate::project::ProjectId;

use super::GitLines;

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
    /// Mark or unmark the commit, for a range or a cherry-pick (`m`, step 4b).
    Mark,
    /// Check out the branch whose label is on the line, or the commit, detached (`c`).
    Checkout,
    /// Make a branch at the commit, its name read in a prompt (`b`).
    Branch,
    /// Make a tag at the commit, its name read in a prompt (`t`), annotated with a message
    /// read after it if `annotated` (`C-u t`).
    Tag { annotated: bool },
    /// Cherry-pick the commit, or the marked ones, oldest first, after a question (`C`).
    CherryPick,
    /// Revert the commit after a question (`V`).
    Revert,
    /// What the question one of the others asked asked about, after the person's `y`.
    Confirmed,
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
    /// What a prompt a graph's act opened read, over the graph `buffer` (step 4b).
    GraphAnswer { buffer: BufferId, text: String },
}
