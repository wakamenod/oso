//! The space level of `*Spaces*`' tree (ADR 0136, ADR 0140): spaces of several repositories,
//! the space in front, folds the person chose, and closed spaces.
//!
//! Must not depend on: `app` or `dispatch`; plain data in, text out.

use super::*;

/// A main checkout at `root`.
fn main_at(root: &str) -> Worktree {
    Worktree {
        primary: true,
        ..worktree(root, "main", None)
    }
}

/// A space of the tree called `name` with the repositories `projects`.
fn space(n: u8, name: &str, projects: Vec<Project>) -> super::super::SpaceRows {
    super::super::SpaceRows {
        id: crate::sessions::spaces::SpaceId::from_random([n; 16]),
        name: name.to_owned(),
        projects,
        directory: None,
        orchestrator: None,
    }
}

fn project(root: &str, worktrees: Vec<Worktree>) -> Project {
    Project {
        root: PathBuf::from(root),
        name: PathBuf::from(root)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        worktrees,
    }
}

#[test]
fn a_space_of_several_repositories_shows_each_with_its_worktrees_a_level_deeper() {
    let tree = Tree {
        spaces: vec![space(
            1,
            "fix-login",
            vec![
                project(
                    "/w/back",
                    vec![
                        main_at("/w/back"),
                        worktree("/w/back/wt/api", "fix/api", Some(1)),
                    ],
                ),
                project(
                    "/w/front",
                    vec![worktree("/w/front/wt/msg", "msg", Some(1))],
                ),
            ],
        )],
        wall: Some(wall()),
        ..Tree::default()
    };
    let id = tree.spaces[0].id.clone();
    let mut list = List::default();
    list.set_tree(tree);
    let mut entry = session(1, "api", State::Working, "/w/back/wt/api");
    entry.space_id = Some(id);
    let rendered = list.render(&[entry], 44, Instant::now());
    let text: Vec<&str> = rendered.text.lines().map(str::trim_end).collect();
    assert!(text[1].starts_with(" ▾ fix-login ─"), "{text:?}");
    // A repository's name ends with a `/` (ADR 0140).
    assert!(text[2].starts_with("   ▾ back/"), "{text:?}");
    assert!(text[3].starts_with("     ▸ main  primary"), "{text:?}");
    assert!(text[4].starts_with("     ▾ api  fix/api"), "{text:?}");
    assert!(text[5].starts_with("       └ ▶ api"), "{text:?}");
    assert!(text[6].starts_with("   ▾ front/"), "{text:?}");
    assert!(text[7].starts_with("     ▸ msg"), "{text:?}");
    assert!(matches!(rendered.lines[0], Line::Title));
    assert!(matches!(rendered.lines[1], Line::Space(_)));
    assert!(matches!(rendered.lines[2], Line::Repository(..)));
    let face_at = |at: usize| {
        (rendered.highlights.iter())
            .find(|(range, _)| range.start == at)
            .map(|(_, h)| *h)
    };
    let back = rendered.text.find("back/").unwrap();
    assert_eq!(face_at(back), Some(Highlight::SessionRepository));
    let space = rendered.text.find("fix-login").unwrap();
    assert_eq!(face_at(space), Some(Highlight::SessionSpace));
    // A repository folds in its space alone.
    list.fold(&rendered.lines[2].clone());
    let text = shown(&list, &[], 44);
    assert!(
        text[2].starts_with("   ▸ back/") && text[3].starts_with("   ▾ front/"),
        "{text:?}"
    );
}

#[test]
fn another_space_in_front_folds_the_quiet_ones_and_closed_spaces_go_last() {
    let busy_dir = "/w/b/wt/x";
    let mut tree = Tree {
        spaces: vec![
            space(1, "front", vec![project("/w/a", vec![main_at("/w/a")])]),
            space(2, "quiet", vec![project("/w/q", vec![main_at("/w/q")])]),
            space(
                3,
                "busy",
                vec![project("/w/b", vec![worktree(busy_dir, "x", Some(1))])],
            ),
        ],
        closed: vec![super::super::ClosedSpace {
            id: crate::sessions::spaces::SpaceId::from_random([9; 16]),
            name: "old".to_owned(),
            title: "An old issue".to_owned(),
        }],
        wall: Some(wall()),
        ..Tree::default()
    };
    tree.front_space = Some(tree.spaces[0].id.clone());
    let id = tree.spaces[2].id.clone();
    let mut list = List::default();
    list.set_tree(tree);
    let mut busy = session(1, "x", ASK, busy_dir);
    busy.space_id = Some(id);
    let text = shown(&list, &[busy], 44);
    let at = |start: &str| text.iter().position(|row| row.starts_with(start));
    assert!(at(" ▾ front ─").is_some(), "{text:?}");
    assert!(at(" ▸ quiet ─").is_some(), "{text:?}");
    assert!(at(" ▾ busy ─").is_some(), "{text:?}");
    let closed = at(" ▸ Archived spaces ─").unwrap_or_else(|| panic!("{text:?}"));
    assert!(text[closed].ends_with(" 1"), "{text:?}");
    assert_eq!(closed, text.len() - 1);
    list.fold(&Line::Header(Group::ArchivedSpaces));
    let text = shown(&list, &[], 44);
    assert!(
        text.last().unwrap().starts_with("   ■ old  An old issue"),
        "{text:?}"
    );
}

#[test]
fn ten_spaces_of_three_repositories_and_three_hundred_externals_render_in_budget() {
    let t = Instant::now();
    let spaces: Vec<super::super::SpaceRows> = (0..10u8)
        .map(|s| {
            let projects = (0..3)
                .map(|r| {
                    let root = format!("/w/s{s}/r{r}");
                    let mut worktrees = vec![main_at(&root)];
                    worktrees
                        .extend((0..4).map(|w| worktree(&format!("{root}/wt/{w}"), "b", Some(w))));
                    project(&root, worktrees)
                })
                .collect();
            space(s, &format!("space-{s}"), projects)
        })
        .collect();
    let tree = Tree {
        spaces,
        wall: Some(wall()),
        ..Tree::default()
    };
    let mut entries: Vec<Entry> = (0..40)
        .map(|n| {
            let mut entry = session(
                n,
                &format!("s{n}"),
                State::Working,
                &format!("/w/s{}/r{}/wt/{}", n % 10, n % 3, n % 4),
            );
            entry.space = format!("space-{}", n % 10);
            entry
        })
        .collect();
    entries.extend((0..300).map(|n| external(10_000 + n, "x", &format!("/x/p{}", n % 30), None)));
    let mut list = List::default();
    list.set_tree(tree);
    let mut best = Duration::MAX;
    for _ in 0..10 {
        let start = Instant::now();
        let rendered = list.render(&entries, 44, t);
        best = best.min(start.elapsed());
        assert!(rendered.lines.len() > 100);
    }
    // A debug build is several times slower; the budget is a release build's (ADR 0131).
    assert!(cfg!(debug_assertions) || best.as_micros() < 500, "{best:?}");
}

#[test]
fn a_space_folded_by_the_person_stays_folded_whatever_its_default_comes_to() {
    let tree = Tree {
        spaces: vec![
            space(1, "front", vec![project("/w/a", vec![main_at("/w/a")])]),
            space(
                2,
                "other",
                vec![project("/w/o", vec![worktree("/w/o/wt/x", "x", Some(1))])],
            ),
        ],
        front_space: Some(crate::sessions::spaces::SpaceId::from_random([1; 16])),
        wall: Some(wall()),
        ..Tree::default()
    };
    let other = tree.spaces[1].id.clone();
    let mut list = List::default();
    list.set_tree(tree);
    assert!(
        shown(&list, &[], 44)
            .iter()
            .any(|row| row.starts_with(" ▸ other ─"))
    );
    // Opened by the person: it stays open when a session of it starts working, and after.
    list.fold(&Line::Space(other.clone()));
    let mut working = session(1, "x", State::Working, "/w/o/wt/x");
    working.space_id = Some(other.clone());
    for entries in [vec![], vec![working.clone()], vec![]] {
        let text = shown(&list, &entries, 44);
        assert!(
            text.iter().any(|row| row.starts_with(" ▾ other ─")),
            "{text:?}"
        );
    }
    // Folded by the person while it works: it stays folded.
    let _ = shown(&list, &[working.clone()], 44);
    list.fold(&Line::Space(other));
    let text = shown(&list, &[working], 44);
    assert!(
        text.iter().any(|row| row.starts_with(" ▸ other ─")),
        "{text:?}"
    );
}

#[test]
fn a_session_in_a_main_checkout_two_spaces_show_goes_under_its_own() {
    let tree = Tree {
        spaces: vec![
            space(1, "first", vec![project("/w/oso", vec![main_at("/w/oso")])]),
            space(
                2,
                "second",
                vec![project("/w/oso", vec![main_at("/w/oso")])],
            ),
        ],
        wall: Some(wall()),
        ..Tree::default()
    };
    let second = tree.spaces[1].id.clone();
    let mut list = List::default();
    list.set_tree(tree);
    let mut entry = session(1, "mine", State::Idle, "/w/oso");
    entry.space_id = Some(second);
    let text = shown(&list, &[entry], 44);
    let at = |start: &str| text.iter().position(|row| row.starts_with(start)).unwrap();
    assert!(at("     └ ○ mine") > at(" ▾ second ─"), "{text:?}");
    assert_eq!(text.iter().filter(|row| row.contains("mine")).count(), 1);
}

#[test]
fn a_closed_spaces_sessions_wait_with_the_archived_ones() {
    let gone = crate::sessions::spaces::SpaceId::from_random([9; 16]);
    let tree = Tree {
        spaces: vec![space(
            1,
            "open",
            vec![project("/w/a", vec![main_at("/w/a")])],
        )],
        closed: vec![super::super::ClosedSpace {
            id: gone.clone(),
            name: "gone".to_owned(),
            title: String::new(),
        }],
        wall: Some(wall()),
        ..Tree::default()
    };
    let mut list = List::default();
    list.set_tree(tree);
    let mut entry = session(1, "left", State::Stopped, "/w/a");
    entry.space_id = Some(gone);
    let text = shown(&list, &[entry], 44);
    assert!(
        text.iter()
            .any(|row| row.starts_with(" ▸ Archived ─") && row.ends_with(" 1")),
        "{text:?}"
    );
}
