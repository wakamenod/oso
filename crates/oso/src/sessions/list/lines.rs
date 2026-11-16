//! What the lines of `*Spaces*` show and what `TAB` folds (ADR 0131, ADR 0136): groups,
//! sessions, spaces, their repositories and worktrees.
//!
//! Must not depend on: `app`, `dispatch` or `config`; plain data.

use std::path::PathBuf;

use super::Key;
use crate::sessions::spaces::SpaceId;

/// A group of the list.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Sessions in one state, by its rank in the taxonomy.
    State(u8),
    /// External sessions in no project of the tree.
    Other,
    /// Those of them in this directory.
    OtherDir(PathBuf),
    /// A repository's worktrees with no session and no change for a week, in a space.
    Quiet(SpaceId, PathBuf),
    Archived,
    /// The closed spaces (ADR 0136).
    ArchivedSpaces,
}

/// What a line of the list shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// The tree's first row: the pane's name and the states that ask for the person
    /// (ADR 0140).
    Title,
    Header(Group),
    Session(Key),
    /// An open space of the tree (ADR 0136).
    Space(SpaceId),
    /// A repository of a space of several, by the space and the repository's root.
    Repository(SpaceId, PathBuf),
    /// A worktree of the tree, by its space and directory: a main checkout two spaces show
    /// is a row of each.
    Worktree(SpaceId, PathBuf),
    /// A closed space, in `Archived spaces`.
    ClosedSpace(SpaceId),
}

/// What `TAB` folds: a group, a session's nested sessions, a repository of a space, or a
/// worktree. A space's fold is the list's own choice, kept apart (ADR 0136, step 2).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Fold {
    Group(Group),
    Nested(Key),
    Repository(SpaceId, PathBuf),
    Worktree(SpaceId, PathBuf),
}
