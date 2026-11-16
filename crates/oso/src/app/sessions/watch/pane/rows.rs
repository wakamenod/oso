//! The row at point in `*Spaces*` as a place (ADR 0131, step 2b): where `claude` starts from
//! it, the repository `session-worktree` makes a worktree of, the base that worktree starts at,
//! the session it is a child of, and the space it is in (ADR 0136, step 2), from a space's,
//! repository's, worktree's or session's row. `C-x a c` and `C-x a w` there take it as `c` and
//! `w` do (the author, 2026-11-15).
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::path::{Path, PathBuf};

use oso_core::buffer::Mode;

use crate::app::App;
use crate::sessions::list::{Group, Key, Line, Project, Tree, Worktree};
use crate::sessions::spaces::SpaceId;
use crate::sessions::worker::SessionKey;

/// What a row with no place says.
const NO_PLACE: &str = "This row is in no project or worktree";

/// What a new worktree made from a row starts at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Base {
    /// `HEAD`, as `session-worktree` has it elsewhere: from a project's row.
    Head,
    /// The branch of the row's worktree.
    Branch(String),
    /// The row's worktree is on no branch: `HEAD`, and the echo area says so (the author,
    /// 2026-11-15).
    Detached,
}

/// The place a row of `*Spaces*` stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct RowPlace {
    /// Where a session started from it runs: a project's main checkout, a worktree, a session's
    /// worktree, else the session's directory.
    pub directory: PathBuf,
    /// The repository of its project in the tree.
    pub repo: Option<PathBuf>,
    pub base: Base,
    /// The hosted session on the row: the parent of a worktree session made from it.
    pub session: Option<SessionKey>,
    /// The space the row is in, which a session started from it joins (ADR 0136, step 2).
    pub space: Option<SpaceId>,
}

impl RowPlace {
    /// The base a new worktree from the row starts at when none is read: its worktree's
    /// branch, else `HEAD`.
    pub(in crate::app) fn base(&self) -> Option<&str> {
        match &self.base {
            Base::Branch(branch) => Some(branch),
            Base::Head | Base::Detached => None,
        }
    }

    /// The repository of the row's project, or why there is none.
    pub(in crate::app) fn repo(&self) -> Result<&Path, &'static str> {
        self.repo.as_deref().ok_or(NO_PLACE)
    }

    /// Whether the row's worktree is on no branch.
    pub(in crate::app) fn detached(&self) -> bool {
        self.base == Base::Detached
    }
}

impl App {
    /// The place of the row at point while `*Spaces*` is selected, `None` elsewhere.
    pub(in crate::app) fn sessions_row_place(&self) -> Option<Result<RowPlace, &'static str>> {
        if self.editor.current().1.mode() != Mode::Sessions {
            return None;
        }
        Some(self.row_place(self.editor.cursor_line()).ok_or(NO_PLACE))
    }

    /// Puts the space of the row `place` in front, so that a session or worktree started from
    /// it joins that space, not the one in front before (ADR 0136, step 2).
    pub(in crate::app) fn enter_row_space(&mut self, place: &RowPlace) {
        let at = place
            .space
            .as_ref()
            .and_then(|id| self.watch.spaces.find(id));
        if let Some(at) = at.filter(|&at| self.watch.spaces.current() != Some(at)) {
            self.enter_space(at);
        }
    }

    /// The place of line `line` of `*Spaces*`.
    pub(in crate::app) fn row_place(&self, line: usize) -> Option<RowPlace> {
        let hosted = |key: SessionKey| Some(self.hosting.table().get(key)?.directory.clone());
        let external = |key: &Key| {
            self.watch
                .externals
                .iter()
                .find(|e| e.key == *key)?
                .dir
                .clone()
        };
        place_of(
            self.sessions_line(line)?,
            self.watch.list.tree(),
            hosted,
            external,
        )
    }
}

/// The place of the row `line` of `tree`: none for a group's row but an `Other projects`
/// directory's. `hosted` and `external` give a session's directory.
fn place_of(
    line: &Line,
    tree: &Tree,
    hosted: impl Fn(SessionKey) -> Option<PathBuf>,
    external: impl Fn(&Key) -> Option<PathBuf>,
) -> Option<RowPlace> {
    match line {
        // A space of one repository's row stands for it; one of several's for none, as which
        // is the person's to say on a repository's row.
        Line::Space(id) => match tree.spaces.iter().find(|space| space.id == *id)? {
            space if space.projects.len() == 1 => Some(repository(id, &space.projects[0])),
            _ => None,
        },
        Line::Repository(id, root) => {
            let space = tree.spaces.iter().find(|space| space.id == *id)?;
            let project = space.projects.iter().find(|p| p.root == *root)?;
            Some(repository(id, project))
        }
        Line::ClosedSpace(_) => None,
        Line::Worktree(id, path) => {
            let (project, worktree) = tree.holding_in(id, path)?;
            let place = in_worktree(project.root.clone(), worktree, None);
            Some(RowPlace {
                space: Some(id.clone()),
                ..place
            })
        }
        Line::Session(key) => {
            let (dir, session) = match key {
                Key::Hosted(hosted_key) => (hosted(*hosted_key)?, Some(*hosted_key)),
                Key::External(_) => (external(key)?, None),
            };
            Some(match tree.holding(&dir) {
                Some((project, worktree)) => in_worktree(project.root.clone(), worktree, session),
                None => RowPlace {
                    directory: dir,
                    repo: None,
                    base: Base::Head,
                    session,
                    space: None,
                },
            })
        }
        Line::Header(Group::OtherDir(dir)) => Some(RowPlace {
            directory: dir.clone(),
            repo: None,
            base: Base::Head,
            session: None,
            space: None,
        }),
        Line::Header(_) | Line::Title => None,
    }
}

/// The place of `project`'s row in space `id`: its main checkout, as the space shows it, else
/// its root.
fn repository(id: &SpaceId, project: &Project) -> RowPlace {
    let main = project.worktrees.iter().find(|w| w.primary);
    RowPlace {
        directory: main.map_or_else(|| project.root.clone(), |main| main.path.clone()),
        repo: Some(project.root.clone()),
        base: Base::Head,
        session: None,
        space: Some(id.clone()),
    }
}

/// The place of `worktree` of the repository `repo`, for `session` on its row if any.
fn in_worktree(repo: PathBuf, worktree: &Worktree, session: Option<SessionKey>) -> RowPlace {
    let base = match &worktree.branch {
        Some(branch) => Base::Branch(branch.clone()),
        None => Base::Detached,
    };
    RowPlace {
        directory: worktree.path.clone(),
        repo: Some(repo),
        base,
        session,
        space: None,
    }
}

#[cfg(test)]
mod tests;
