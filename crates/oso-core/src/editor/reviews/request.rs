//! What commands ask of the host for reviews (ADR 0122): what the host runs git, threads or
//! files for, which a command cannot.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads; the editor queues them, and
//! the host takes them after each command.

use super::{ReviewId, ScopeKind};
use crate::buffer::BufferId;
use crate::project::ProjectId;
use crate::review::Side;

/// What commands ask of the host for reviews.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewRequest {
    /// Open a review of `scope` in `project`; `revision` is a commit's, a range's `FROM..TO`
    /// or the branch's base, as the person gave it or the menu found it. A branch with none
    /// has its base found first.
    Open {
        project: ProjectId,
        scope: ScopeKind,
        revision: Option<String>,
    },
    /// Take a baseline of the project's work tree for the session scope.
    Baseline { project: ProjectId },
    /// Read the review's files again now.
    Refresh(ReviewId),
    /// Build the file's section again, as its comments changed.
    Redraw { review: ReviewId, path: Box<[u8]> },
    /// Go to line `number` of `side` of the file in its source, or in its old side when
    /// `old_side`.
    Visit {
        review: ReviewId,
        path: Box<[u8]>,
        side: Side,
        number: u32,
        old_side: bool,
    },
    /// The review was closed: stop its threads.
    Close(ReviewId),
    /// Accept hunks `hunks` of the file at `path`, by their index in the model's reading:
    /// stage them, or unstage them in the staged scope (ADR 0122, step 5).
    Accept {
        review: ReviewId,
        path: Box<[u8]>,
        hunks: Vec<usize>,
    },
    /// Reject hunks `hunks` of the file at `path`: revert them in the file through its buffer,
    /// or unstage them in the staged scope.
    Reject {
        review: ReviewId,
        path: Box<[u8]>,
        hunks: Vec<usize>,
    },
    /// Mark hunks `hunks` of the file at `path` viewed, or clear the mark when every one has it.
    Viewed {
        review: ReviewId,
        path: Box<[u8]>,
        hunks: Vec<usize>,
    },
    /// Undo the review's last decision.
    Undo(ReviewId),
    /// Gather the person's comments that were not sent into a message, shown in
    /// `*review message*` before anything goes to an agent session (step 12).
    Send(ReviewId),
    /// Paste the review message `buffer` holds into its review's session, or into the session
    /// named `to`, and mark the comments it took sent.
    MessageSend {
        buffer: BufferId,
        to: Option<String>,
    },
    /// Drop the review message `buffer` holds, sending nothing.
    MessageDrop { buffer: BufferId },
    /// Open again the saved review the host listed as this candidate (ADR 0122, step 6).
    Resume(String),
    /// Remove the saved review the host listed as this candidate, now.
    Forget(String),
    /// Add what a message made from a pull request's checks or threads left out, above the end
    /// of what GitHub said (`C-c C-t`, ADR 0127, step 6b).
    MessageTake { buffer: BufferId },
}
