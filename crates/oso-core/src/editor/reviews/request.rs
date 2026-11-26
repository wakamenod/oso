//! What commands ask of the host for reviews (ADR 0122): what the host runs git, threads or
//! files for, which a command cannot.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads; the editor queues them, and
//! the host takes them after each command.

use std::ops::Range;

use super::{ReviewId, ScopeKind};
use crate::buffer::BufferId;
use crate::project::ProjectId;
use crate::review::Side;

/// Where a jump from a review's line goes (ADR 0122, "Jump to the source").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisitTo {
    /// Its file now, at the line carried to it.
    Source,
    /// The line's own side as the review read it, read-only.
    Side,
    /// The old side as the review read it, read-only.
    OldSide,
}

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
    /// Show the unchanged lines `show` of the file's old side too, by number, besides the three
    /// around each hunk; with `None`, only those three again (ADR 0122, step 13).
    Context {
        review: ReviewId,
        path: Box<[u8]>,
        show: Option<Range<u32>>,
    },
    /// Go to line `number` of `side` of the file, where `to` says.
    Visit {
        review: ReviewId,
        path: Box<[u8]>,
        side: Side,
        number: u32,
        to: VisitTo,
    },
    /// The review was closed: stop its threads.
    Close(ReviewId),
    /// `review-layout` (ADR 0160, ADR 0184): with `menu`, open the review menu; else go back
    /// from the layout's slot to the review, restore the windows from before the layout, or
    /// continue the review the person left last, as where the person is says, and open the
    /// review menu where no review was left or saved.
    Layout { menu: bool },
    /// `q` buried a review: put back the windows from before the review layout, if the frame's
    /// tree is one (ADR 0160).
    Quit,
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
    /// Ask `question` about what the primary cursor of the selected window is on in the
    /// review, or the lines its region spans: sent at once to the space's orchestrator, or to
    /// the review's session outside one, and its terminal shown beside the review (ADR 0163).
    Ask { review: ReviewId, question: String },
    /// A step of starting a review in a space whose orchestrator is recorded, from the space's
    /// repositories (ADR 0163): the host finds the project and opens the next step's menu.
    Start(ReviewStart),
    /// Show another review of the pull request `review` is, or is a commit of (ADR 0167): the
    /// host opens a commit's review, or shows the one open, in the review layout, and selects
    /// it, unless the selected window shows the pull request's commit list, which stays
    /// selected, a step from it going from the review it shows last.
    Step { review: ReviewId, to: Step },
    /// Records the pull request's head that `review`, of it or of one of its commits, shows
    /// as the person's last review of it, from which what changed since is shown (ADR 0167,
    /// step 2).
    Checkpoint { review: ReviewId },
}

/// Where a step through a pull request's commits goes (ADR 0167).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The next commit that changes something: from the whole pull request, its first.
    Next,
    /// The commit before that changes something: from its first, the whole pull request; from
    /// the whole, its last.
    Previous,
    /// The whole pull request.
    Whole,
    /// Its commit of this index, oldest first, as its commit list shows it.
    Commit(usize),
    /// What changed since the person's last review of it (step 2).
    Since,
    /// Its commits from the first index to the second, as `r` reviews a range (step 2).
    Range(usize, usize),
}

/// A step of starting a review in a space (ADR 0163), as `review-repository`,
/// `review-worktree` and `review-checkout` ask it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewStart {
    /// The space in front's repository of this name, as its prompt lists it.
    Repository(String),
    /// The chosen repository's worktree at this path, as its prompt lists it.
    Worktree(String),
    /// The selected buffer's worktree of the chosen repository, else its main checkout.
    Checkout,
}
