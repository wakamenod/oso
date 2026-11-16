//! Which part of the app carries out each git request (ADR 0127): the status's own, a commit's and
//! its log's, a remote's, a stopped run's, a stash's, a branch's, or a graph's. One match over every request,
//! so that a new one is placed here before it builds, never dropped at run time.
//!
//! Must not depend on: key presses, how a frame is drawn, or git; this only sorts requests.

use oso_core::editor::GitRequest;

/// The part of the app that carries out a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Concern {
    /// The status buffer: reading, folding, staging, visiting.
    Status,
    /// Committing, its log, discarding and the worktree's agent session.
    Commit,
    /// Pushing, pulling and fetching (step 2).
    Remote,
    /// Continuing, skipping and aborting what is stopped, a sequence as magit calls it (step
    /// 3a); not `GitRequest::Stop`, which stops a run and is a commit's.
    Sequence,
    /// Stashing, applying, popping and dropping (step 3b).
    Stash,
    /// Checking out, creating, renaming and deleting branches (step 3b).
    Branch,
    /// The graph of a repository's commits (step 4).
    Graph,
}

/// Where `request` is carried out.
pub(super) fn concern(request: &GitRequest) -> Concern {
    match request {
        GitRequest::Status { .. }
        | GitRequest::Refresh { .. }
        | GitRequest::Toggle { .. }
        | GitRequest::Stage { .. }
        | GitRequest::Unstage { .. }
        | GitRequest::StageAll { .. }
        | GitRequest::UnstageAll { .. }
        | GitRequest::Visit { .. } => Concern::Status,
        GitRequest::Commit { .. }
        | GitRequest::CommitFinish { .. }
        | GitRequest::CommitCancel { .. }
        | GitRequest::CommitRing { .. }
        | GitRequest::ShowLog { .. }
        | GitRequest::Stop { .. }
        | GitRequest::Discard { .. }
        | GitRequest::RestoreDiscarded { .. }
        | GitRequest::ShowDiscarded { .. }
        | GitRequest::DraftMessage { .. }
        | GitRequest::FixWithAgent { .. } => Concern::Commit,
        GitRequest::Push { .. } | GitRequest::Pull { .. } | GitRequest::Fetch { .. } => {
            Concern::Remote
        }
        GitRequest::Continue { .. } | GitRequest::Skip { .. } | GitRequest::Abort { .. } => {
            Concern::Sequence
        }
        GitRequest::Stash { .. } => Concern::Stash,
        GitRequest::Branch { .. } => Concern::Branch,
        GitRequest::Graph { .. } | GitRequest::GraphToggle { .. } | GitRequest::GraphAct { .. } => {
            Concern::Graph
        }
    }
}
