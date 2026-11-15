//! Which worktrees each space shows: its own, the main checkout in the first space of the
//! repository or where a session of the space runs, and a worktree no space holds in the first
//! space only.
//!
//! Must not depend on: `app`; plain data.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{SpaceInput, space_rows};
use crate::sessions::list::Worktree;
use crate::sessions::spaces::SpaceId;

fn worktree(path: &str, primary: bool) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        primary,
        ..Worktree::default()
    }
}

fn shown(rows: &[crate::sessions::list::SpaceRows], space: usize) -> Vec<Vec<String>> {
    rows[space]
        .projects
        .iter()
        .map(|p| {
            p.worktrees
                .iter()
                .map(|w| w.path.display().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn each_worktree_shows_in_one_space_and_the_main_checkout_where_it_is_used() {
    let listed: HashMap<PathBuf, Vec<Worktree>> = HashMap::from([
        (
            PathBuf::from("/b"),
            vec![
                worktree("/b", true),
                worktree("/b/wt/fix", false),
                worktree("/b/wt/outside", false),
            ],
        ),
        (
            PathBuf::from("/f"),
            vec![worktree("/f", true), worktree("/f/wt/msg", false)],
        ),
    ]);
    let (a, b) = (SpaceId::from_random([1; 16]), SpaceId::from_random([2; 16]));
    let a_held = [PathBuf::from("/b/wt/fix"), PathBuf::from("/f/wt/msg")];
    let spaces = [
        SpaceInput {
            id: &a,
            name: "fix-login",
            members: vec![Path::new("/b"), Path::new("/f")],
            held: &a_held,
            sessions: HashSet::new(),
            directory: None,
        },
        SpaceInput {
            id: &b,
            name: "oso",
            members: vec![Path::new("/b")],
            held: &[],
            sessions: HashSet::from([PathBuf::from("/b")]),
            directory: None,
        },
    ];
    let rows = space_rows(&spaces, &[], &listed);
    assert_eq!(
        shown(&rows, 0),
        [
            vec!["/b", "/b/wt/fix", "/b/wt/outside"],
            vec!["/f", "/f/wt/msg"]
        ],
        "the first space of each repository shows its main checkout and what no space holds"
    );
    assert_eq!(
        shown(&rows, 1),
        [vec!["/b"]],
        "the main checkout where a session of the space runs, nothing another holds"
    );
    let idle = [SpaceInput {
        sessions: HashSet::new(),
        ..spaces.into_iter().nth(1).unwrap()
    }];
    let rows = space_rows(&idle, &[], &listed);
    assert_eq!(
        rows[0].projects[0].worktrees.len(),
        3,
        "alone, it is the first"
    );
}

#[test]
fn a_closed_spaces_worktree_and_one_of_a_repository_taken_out_show_where_they_should() {
    let listed: HashMap<PathBuf, Vec<Worktree>> = HashMap::from([(
        PathBuf::from("/b"),
        vec![
            worktree("/b", true),
            worktree("/b/wt/old", false),
            worktree("/b/wt/moved", false),
        ],
    )]);
    let (open, gone, taken) = (
        SpaceId::from_random([1; 16]),
        SpaceId::from_random([2; 16]),
        SpaceId::from_random([3; 16]),
    );
    let gone_held = [PathBuf::from("/b/wt/old")];
    let taken_held = [PathBuf::from("/b/wt/moved")];
    let spaces = [
        SpaceInput {
            id: &open,
            name: "open",
            members: vec![Path::new("/b")],
            held: &[],
            sessions: HashSet::new(),
            directory: None,
        },
        // Its repository was taken out: what it held is no one's.
        SpaceInput {
            id: &taken,
            name: "taken",
            members: vec![],
            held: &taken_held,
            sessions: HashSet::new(),
            directory: None,
        },
    ];
    let closed = [SpaceInput {
        id: &gone,
        name: "gone",
        members: vec![Path::new("/b")],
        held: &gone_held,
        sessions: HashSet::new(),
        directory: None,
    }];
    let rows = space_rows(&spaces, &closed, &listed);
    assert_eq!(shown(&rows, 0), [vec!["/b", "/b/wt/moved"]]);
}
