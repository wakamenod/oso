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

/// The line of `lines`, the list made again, that point goes to from line `on`, which showed
/// `was`: where that shows now, else the same line. A closed space's row goes to the heading of
/// the closed ones, where no key closes another (ADR 0136, "A chosen space is closed, as
/// amended"); and not the title, where no key acts (ADR 0140).
pub fn point_line(was: Option<&Line>, on: Option<usize>, lines: &[Line]) -> Option<usize> {
    let at = |wanted: &Line| lines.iter().position(|line| line == wanted);
    let closed = || match was {
        Some(Line::Space(_)) => at(&Line::Header(Group::ArchivedSpaces)),
        _ => None,
    };
    let line = was
        .and_then(at)
        .or_else(closed)
        .or(on.filter(|&line| line < lines.len()))?;
    match lines.get(line) {
        Some(Line::Title) if lines.len() > 1 => Some(line + 1),
        _ => Some(line),
    }
}

#[cfg(test)]
mod tests {
    use super::{Group, Line, SpaceId, point_line};

    fn space(n: u8) -> Line {
        Line::Space(SpaceId::from_random([n; 16]))
    }

    #[test]
    fn point_stays_on_its_row_and_off_the_title() {
        let lines = [Line::Title, space(2), space(1)];
        assert_eq!(point_line(Some(&space(1)), Some(1), &lines), Some(2));
        let gone = Line::ClosedSpace(SpaceId::from_random([9; 16]));
        assert_eq!(point_line(Some(&gone), Some(2), &lines), Some(2));
        assert_eq!(point_line(Some(&Line::Title), Some(0), &lines), Some(1));
        assert_eq!(point_line(Some(&gone), Some(7), &lines), None);
    }

    #[test]
    fn a_closed_spaces_row_goes_to_the_closed_ones_heading_not_the_next_space() {
        let archived = Line::Header(Group::ArchivedSpaces);
        let mut lines = vec![Line::Title, space(2), space(3), archived];
        assert_eq!(point_line(Some(&space(1)), Some(1), &lines), Some(3));
        // Open, its closed space's row is under it: the heading still.
        lines.push(Line::ClosedSpace(SpaceId::from_random([1; 16])));
        assert_eq!(point_line(Some(&space(1)), Some(1), &lines), Some(3));
        // Grouped by state, no space has a row, nor do the closed ones a heading.
        let by_state = [Line::Header(Group::Archived), Line::Header(Group::Other)];
        assert_eq!(point_line(Some(&space(1)), Some(1), &by_state), Some(1));
    }
}
