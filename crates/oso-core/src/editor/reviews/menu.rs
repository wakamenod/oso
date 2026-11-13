//! What the review menu shows of each project (ADR 0117, "Live content", ADR 0122): each
//! scope's count from the menu's latest opening, the last scope reviewed, when the session's
//! baseline was taken, and the branch's base.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or git; the host counts on
//! threads of its own and hands each count in.

use super::{Reviews, ScopeKind};
use crate::editor::Editor;
use crate::project::ProjectId;

/// How many files a scope differs in, as the menu shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// Being counted.
    Running,
    Files(usize),
    /// More than a review lists.
    More,
    /// The count could not be made.
    Failed,
}

/// What the review menu shows of one project: each scope's count from the menu's latest
/// opening, the last scope reviewed, when the session's baseline was taken, and the branch's
/// base.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScopeMenu {
    counts: [Option<Count>; ScopeKind::COUNT],
    last: Option<ScopeKind>,
    /// Seconds since the Unix epoch.
    baseline: Option<u64>,
    base: Option<String>,
    generation: u64,
}

impl ScopeMenu {
    /// The count of `scope` from this opening, `None` if none was asked for.
    pub fn count(&self, scope: ScopeKind) -> Option<Count> {
        self.counts[scope.index()]
    }

    /// The scope last reviewed in the project.
    pub fn last(&self) -> Option<ScopeKind> {
        self.last
    }

    /// When the session's baseline was taken, in seconds since the Unix epoch.
    pub fn baseline_taken(&self) -> Option<u64> {
        self.baseline
    }

    /// The base the branch scope compares with, once the host found it.
    pub fn base(&self) -> Option<&str> {
        self.base.as_deref()
    }

    /// The opening the counts are of.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Reviews {
    fn menu_mut(&mut self, project: ProjectId) -> &mut ScopeMenu {
        let index = match self.menus.iter().position(|(id, _)| *id == project) {
            Some(index) => index,
            None => {
                self.menus.push((project, ScopeMenu::default()));
                self.menus.len() - 1
            }
        };
        &mut self.menus[index].1
    }
}

impl Editor {
    /// What the review menu shows of `project`, once it opened for it.
    pub fn scope_menu(&self, project: ProjectId) -> Option<&ScopeMenu> {
        let (_, menu) = self.reviews.menus.iter().find(|(id, _)| *id == project)?;
        Some(menu)
    }

    /// Starts a new opening of the review menu for `project`: every count and the branch's
    /// base are forgotten, as the branch may have changed, and the counts of `scopes` are
    /// running. Returns the opening's generation, which the counts that come back name; counts
    /// of an earlier one are dropped.
    pub fn start_scope_counts(&mut self, project: ProjectId, scopes: &[ScopeKind]) -> u64 {
        let menu = self.reviews.menu_mut(project);
        menu.generation += 1;
        menu.base = None;
        menu.counts = [None; ScopeKind::COUNT];
        for scope in scopes {
            menu.counts[scope.index()] = Some(Count::Running);
        }
        menu.generation
    }

    /// Takes the count of `scope` in `project` from the opening `generation`. False, changing
    /// nothing, for an earlier opening's.
    pub fn set_scope_count(
        &mut self,
        project: ProjectId,
        generation: u64,
        scope: ScopeKind,
        count: Count,
    ) -> bool {
        let menu = self.reviews.menu_mut(project);
        if menu.generation != generation {
            return false;
        }
        menu.counts[scope.index()] = Some(count);
        true
    }

    /// Takes the base the branch scope compares with in `project`, found for the opening
    /// `generation`; an earlier opening's is dropped.
    pub fn set_branch_base(&mut self, project: ProjectId, generation: u64, base: Option<String>) {
        let menu = self.reviews.menu_mut(project);
        if menu.generation == generation {
            menu.base = base;
        }
    }

    /// Says when the session's baseline of `project` was taken, in seconds since the Unix
    /// epoch, or that it has none.
    pub fn set_baseline_taken(&mut self, project: ProjectId, at: Option<u64>) {
        self.reviews.menu_mut(project).baseline = at;
    }

    /// Remembers `scope` as the last one reviewed in `project`.
    pub fn set_last_scope(&mut self, project: ProjectId, scope: ScopeKind) {
        self.reviews.menu_mut(project).last = Some(scope);
    }
}
