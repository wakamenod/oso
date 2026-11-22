//! What the app keeps of its reviews (ADR 0122): each open review's threads and the text that
//! waits to go in, each project's session baseline and the threads taking one, the menu's
//! counting threads and the root they run in, the message waiting to be sent and the session
//! picked for each review (step 12), and the choices over them that need no window.
//!
//! Must not depend on: the editor's state beyond what a caller hands in, key presses or how a
//! frame is drawn. Nothing here runs git: it starts, revokes and drops the threads that do.

use std::collections::VecDeque;
use std::sync::Arc;

use oso_core::editor::{ReviewArea, ReviewId};
use oso_core::project::ProjectId;
use oso_core::window::{Screen, ViewSize, WindowId};

use crate::git::{Id, TrustedRoot};
use crate::review::count::Counter;
use crate::review::view::{Tell, Text, View};
use crate::review::{Reading, Reviewer, Scope};

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
    /// How it shows: unified or as its two panes (ADR 0122, step 11).
    pub(super) layout: super::layout::Layout,
    /// What marks it stale, and its refresh once the marks settle (step 4).
    pub(super) refresher: super::watch::Refresher,
    /// Its decisions, for undo, and what waits for git or a save (step 5).
    pub(super) decisions: super::decide::Decisions,
    /// Whether the view was last told that agent cards fold.
    pub(super) folded: bool,
    /// Whether agent cards fold in the text the review buffer holds, which may still be one
    /// built before the view was told otherwise.
    pub(super) shown_folded: bool,
    /// What `gh` last said of the pull request it reviews (step 12b).
    pub(super) pr: Option<crate::review::pr::PrFound>,
    /// Whether every file of a listing was read once, after which a file's first reading is
    /// new to the person (step 4).
    pub(super) read_once: bool,
    /// How it is saved across restarts (step 6).
    pub(super) saving: super::keep::Saving,
    /// What its view was told of its files pane and filter (step 6b).
    pub(super) files: super::files::Told,
    /// The latest jump to the source asked of its thread (step 3, part three).
    pub(super) jumping: Option<super::visit::Jumping>,
    /// The buffers showing its sides read-only (step 3, part three).
    pub(super) sides: Vec<oso_core::buffer::BufferId>,
    /// The files of the open buffers that may hold secrets its view was last told of (step 13).
    pub(super) held: Arc<std::collections::HashSet<std::path::PathBuf>>,
    /// A pull request's commits, as its thread listed them (ADR 0167).
    pub(super) walk: Option<super::walk::Walk>,
    /// The pull request review this one reviews a commit of, and that commit's index.
    pub(super) pull: Option<(ReviewId, usize)>,
    /// The pull request its saved file names, to link it to once that pull request's commits
    /// are listed.
    pub(super) pull_wanted: Option<u64>,
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
    /// A review of `scope` in `project`, opened now, its thread not started yet.
    pub(super) fn new(
        id: ReviewId,
        project: ProjectId,
        scope: Scope,
        root: TrustedRoot,
        view: View,
    ) -> Running {
        Running {
            id,
            project,
            scope,
            root,
            serial: 0,
            reviewer: None,
            view,
            texts: VecDeque::new(),
            revoked: false,
            carrying: VecDeque::new(),
            layout: super::layout::Layout::default(),
            refresher: super::watch::Refresher::default(),
            decisions: super::decide::Decisions::default(),
            folded: false,
            shown_folded: false,
            read_once: false,
            saving: super::keep::Saving::default(),
            files: super::files::Told::default(),
            pr: None,
            jumping: None,
            sides: Vec::new(),
            held: Arc::default(),
            walk: None,
            pull: None,
            pull_wanted: None,
        }
    }

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
    /// Room for the byte ranges every window shows, for the sides on screen (step 9).
    pub(super) ranges: Vec<(oso_core::buffer::BufferId, std::ops::Range<usize>)>,
    /// What each open review's view was last told about highlighting its sides (step 9).
    pub(super) sides: std::collections::HashMap<ReviewId, super::highlight::Told>,
    /// The areas of the frame reviews show in, as last looked at, and what each wants.
    pub(super) areas: Vec<ReviewArea>,
    pub(super) judge: super::judge::Judge,
    /// Room for the rows a review window shows, to find the comments read.
    pub(super) screen: Screen,
    /// Whether the person's own input came since reading marks were last looked for: what the
    /// agent channel brings into view is not read.
    pub(super) person_acted: bool,
    /// The selected buffer and its revision as the person's input came: a hunk is seen only in
    /// the text they had in front of them, not in one put in after their key (step 4).
    pub(super) acted_on: Option<(oso_core::buffer::BufferId, u64, usize)>,
    /// Whether oso's window is hidden, when no review looks (step 4).
    pub(super) occluded: bool,
    /// The selected window, its first row's offset, its size and its buffer's revision when
    /// reading marks were last looked for: the same view of the same text shows nothing new.
    pub(super) looked: Option<(WindowId, usize, ViewSize, u64)>,
    /// The review message waiting to be sent, and the session picked for each review (step 12).
    pub(super) sending: super::send::Sending,
    /// A question's Enter that waits, and one held for an orchestrator being resumed
    /// (ADR 0163).
    pub(super) asking: super::ask::Asking,
    /// A start in a space between its steps (ADR 0163).
    pub(super) starting: super::start::Starting,
    /// The saved reviews' store and list (step 6).
    pub(super) keep: super::keep::Keep,
    /// The reviews a restart carried, waiting for their projects (step 6).
    pub(super) handed: Vec<super::handed::Handed>,
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
