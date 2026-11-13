//! What the app keeps of its reviews (ADR 0122): each open review's threads and the text that
//! waits to go in, each project's session baseline and the threads taking one, the menu's
//! counting threads and the root they run in, and the choices over them that need no window.
//!
//! Must not depend on: the editor's state beyond what a caller hands in, key presses or how a
//! frame is drawn. Nothing here runs git: it starts, revokes and drops the threads that do.

use std::collections::VecDeque;
use std::sync::Arc;

use oso_core::editor::ReviewId;
use oso_core::project::ProjectId;
use oso_core::window::Screen;

use crate::review::count::Counter;
use crate::review::view::{Tell, Text, View};
use crate::review::{Id, Reading, Reviewer, Scope, TrustedRoot};

/// What the review's note says once its project is no longer trusted.
pub(super) const UNTRUSTED: &str = "the project is no longer trusted: M-x trust-project, then g";

/// An open review's threads, and the text its view built that waits to go in: one at most, as
/// the view builds the next only once this one went in.
pub(super) struct Running {
    pub(super) id: ReviewId,
    pub(super) project: ProjectId,
    pub(super) scope: Scope,
    pub(super) root: TrustedRoot,
    /// Which start of the review's thread this is: reports of an earlier one are dropped.
    pub(super) serial: u64,
    pub(super) reviewer: Option<Reviewer>,
    pub(super) view: View,
    pub(super) texts: VecDeque<Box<Text>>,
    /// Whether the root was revoked as its project stopped being trusted.
    pub(super) revoked: bool,
    /// The readings whose comments the review's own thread carries, in the order asked, which
    /// is the order it answers.
    pub(super) carrying: VecDeque<Arc<Reading>>,
    /// Whether the view was last told that agent cards fold.
    pub(super) folded: bool,
}

/// What opening a review that is open already does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reopen {
    /// Read its files again: the same scope.
    Refresh,
    /// Start its thread over from a new root, as its project was trusted again since it was
    /// revoked, with the scope asked for.
    Restart,
    /// Open the scope asked for on its thread: a session review's new baseline.
    Open,
}

impl Running {
    /// What opening `scope` again does to this review, which reviews the same thing.
    pub(super) fn reopen(&self, scope: &Scope) -> Reopen {
        if self.revoked {
            Reopen::Restart
        } else if self.scope == *scope {
            Reopen::Refresh
        } else {
            Reopen::Open
        }
    }
}

/// A thread taking a project's baseline, the root it runs in, and which it is: a report of one
/// replaced since is dropped.
pub(super) struct Baseliner {
    pub(super) project: ProjectId,
    pub(super) serial: u64,
    pub(super) root: TrustedRoot,
    /// Kept only for its thread, which ends as it is dropped.
    pub(super) _reviewer: Reviewer,
}

/// The app's reviews.
#[derive(Default)]
pub(in crate::app) struct Reviews {
    pub(super) open: Vec<Running>,
    /// Each project's session baseline, as its thread took it.
    pub(super) baselines: Vec<(ProjectId, Id)>,
    /// The threads taking a baseline, one per project at most.
    pub(super) baseliners: Vec<Baseliner>,
    /// The menu's counting threads, once the menu opened.
    pub(super) counter: Option<Counter>,
    /// The project the menu's counts were last started for, and the root they run in.
    pub(super) counted: Option<(ProjectId, TrustedRoot)>,
    /// The project a review of the branch waits on the base of, as `review-branch` asked
    /// before the base was known: it opens with the next base found there.
    pub(super) base_wanted: Option<ProjectId>,
    /// Whether the review menu was open when last looked at.
    pub(super) menu_open: bool,
    pub(super) next_serial: u64,
    /// Room for the paths of the files a review's windows show.
    pub(super) shown: Vec<Box<[u8]>>,
    /// Room for the rows a review window shows, to find the comments read.
    pub(super) screen: Screen,
}

impl Reviews {
    pub(super) fn index(&self, id: ReviewId) -> Option<usize> {
        self.open.iter().position(|running| running.id == id)
    }

    /// The open review `id`'s place, if `serial` is its thread's current start: `None` for a
    /// report of a thread replaced since, or of a review closed.
    pub(super) fn current(&self, id: ReviewId, serial: u64) -> Option<usize> {
        self.index(id)
            .filter(|&index| self.open[index].serial == serial)
    }

    pub(super) fn baseline(&self, project: ProjectId) -> Option<Id> {
        self.baselines
            .iter()
            .find_map(|(id, tree)| (*id == project).then(|| tree.clone()))
    }

    /// A new serial, for a thread started.
    pub(super) fn serial(&mut self) -> u64 {
        self.next_serial += 1;
        self.next_serial
    }

    /// Keeps `baseliner` as its project's one, dropping the one before.
    pub(super) fn add_baseliner(&mut self, baseliner: Baseliner) {
        self.baseliners
            .retain(|other| other.project != baseliner.project);
        self.baseliners.push(baseliner);
    }

    /// Whether a baseline report of `project` from the thread `serial` is the current one's,
    /// which is then done. False for a thread replaced or dropped since.
    pub(super) fn take_baseliner(&mut self, project: ProjectId, serial: u64) -> bool {
        let Some(index) = self
            .baseliners
            .iter()
            .position(|b| b.project == project && b.serial == serial)
        else {
            return false;
        };
        self.baseliners.remove(index);
        true
    }

    /// Takes the open review `id` out, with the text it left waiting, for the caller to free.
    pub(super) fn close(&mut self, id: ReviewId) -> Option<Running> {
        let index = self.index(id)?;
        Some(self.open.remove(index))
    }

    /// Revokes the root of every review, baseline and count whose project `trusted` no longer
    /// says is trusted: a review's once, saying so; a baseline's thread is dropped, and the
    /// counts are stopped.
    pub(super) fn revoke_untrusted(&mut self, trusted: impl Fn(ProjectId) -> bool) {
        for running in &mut self.open {
            if running.revoked || trusted(running.project) {
                continue;
            }
            running.root.revoke();
            running.revoked = true;
            running.view.tell(Tell::Said(vec![UNTRUSTED.to_owned()]));
        }
        self.baseliners.retain(|baseliner| {
            let kept = trusted(baseliner.project);
            if !kept {
                baseliner.root.revoke();
            }
            kept
        });
        if let Some((project, root)) = &self.counted
            && !trusted(*project)
        {
            root.revoke();
            if let Some(counter) = &self.counter {
                counter.stop();
            }
            self.counted = None;
            self.base_wanted = None;
        }
    }
}
