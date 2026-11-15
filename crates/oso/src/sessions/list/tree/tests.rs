//! The tree: projects, their worktrees in order and their sessions, external sessions under
//! their worktree or in `Other projects`, quiet worktrees, folds, the space in front, the
//! spinner on a worktree's row, and its cost.
//!
//! Must not depend on: `app` or `dispatch`; plain data in, text out.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oso_core::syntax::Highlight;

use super::super::{Entry, Group, Key, Line, List};
use super::{Project, Tree, Worktree};
use crate::sessions::state::{Reason, State};

const ASK: State = State::NeedsInput(Reason::Permission);
const DAY: Duration = Duration::from_secs(86_400);

fn wall() -> SystemTime {
    UNIX_EPOCH + DAY * 20_000
}

fn session(key: u64, name: &str, state: State, dir: &str) -> Entry {
    Entry {
        key: Key::Hosted(key),
        name: name.to_owned(),
        space: String::new(),
        space_id: None,
        branch: None,
        state,
        since: Instant::now(),
        recap: String::new(),
        pinned: false,
        archived: false,
        unasked: false,
        parent: None,
        dir: Some(PathBuf::from(dir)),
        repo: None,
    }
}

fn external(pid: u32, name: &str, dir: &str, repo: Option<&str>) -> Entry {
    Entry {
        key: Key::External(pid),
        repo: repo.map(PathBuf::from),
        ..session(0, name, State::Idle, dir)
    }
}

fn worktree(path: &str, branch: &str, made_days_ago: Option<u64>) -> Worktree {
    Worktree {
        path: PathBuf::from(path),
        branch: Some(branch.to_owned()),
        primary: false,
        made: made_days_ago.map(|days| wall() - DAY * u32::try_from(days).unwrap()),
        active: made_days_ago.map(|days| wall() - DAY * u32::try_from(days).unwrap()),
    }
}

fn tree() -> Tree {
    let main = Worktree {
        primary: true,
        ..worktree("/w/oso", "main", None)
    };
    Tree {
        spaces: Tree::of_projects(vec![Project {
            root: PathBuf::from("/w/oso"),
            name: "oso".to_owned(),
            worktrees: vec![
                worktree("/w/oso/.claude/worktrees/old-fix", "old-fix", Some(3)),
                main,
                worktree("/w/oso/.claude/worktrees/pane", "feature/pane", Some(1)),
                worktree("/w/oso/.claude/worktrees/stale", "stale", Some(30)),
            ],
        }])
        .spaces,
        front_space: Some(crate::sessions::spaces::SpaceId::from_random([0; 16])),
        front: Some(PathBuf::from("/w/oso/.claude/worktrees/pane")),
        wall: Some(wall()),
        ..Default::default()
    }
}

fn shown(list: &List, entries: &[Entry], width: usize) -> Vec<String> {
    let rendered = list.render(entries, width, Instant::now());
    for line in rendered.text.lines() {
        assert!(super::cells(line) <= width, "{line:?}");
    }
    rendered.text.lines().map(str::to_owned).collect()
}

#[test]
fn projects_hold_their_worktrees_main_first_then_newest_and_their_sessions() {
    let mut list = List::default();
    list.set_tree(tree());
    let entries = [
        session(1, "oso-bb", State::Working, "/w/oso"),
        session(2, "pane-c3", ASK, "/w/oso/.claude/worktrees/pane/src"),
        external(9, "oso-2", "/w/oso", Some("/w/oso")),
    ];
    let text = shown(&list, &entries, 44);
    assert!(
        text[0].starts_with("▾ oso") && text[0].ends_with("⚠ 1"),
        "{text:?}"
    );
    assert!(text[1].starts_with("  ▶ main  primary"), "{text:?}");
    assert!(text[2].starts_with("    ▶ oso-bb"), "{text:?}");
    // An external session in the worktree comes after oso's own.
    assert!(text[3].starts_with("    ○ oso-2"), "{text:?}");
    // The newest made next; its branch shows as it is not the directory's name.
    assert!(text[4].starts_with("  ⚠ pane  feature/pane"), "{text:?}");
    assert!(text[5].starts_with("    ⚠ pane-c3"), "{text:?}");
    // A worktree with no session: its mark says so, and it is folded with nothing to show.
    assert!(
        text[6].starts_with("  · old-fix") && text[6].ends_with("3d"),
        "{text:?}"
    );
    // One unchanged for a month waits in the quiet row, folded.
    assert_eq!(text[7].trim_end(), "  ▸ 1 quiet");
    assert_eq!(text.len(), 8, "{text:?}");
}

#[test]
fn every_other_external_session_goes_in_other_projects_folded_by_directory() {
    let mut list = List::default();
    list.set_tree(tree());
    let entries = [
        session(1, "oso-bb", State::Idle, "/w/oso"),
        external(7, "a", "/x/blog/posts", Some("/x/blog")),
        external(8, "b", "/x/blog", Some("/x/blog")),
        external(9, "c", "/y/scripts", None),
    ];
    let rendered = list.render(&entries, 44, Instant::now());
    let text: Vec<&str> = rendered.text.lines().collect();
    let other = text
        .iter()
        .position(|row| row.starts_with("▸ Other projects"));
    let other = other.unwrap_or_else(|| panic!("{text:?}"));
    assert!(text[other].ends_with('3'), "{text:?}");
    assert_eq!(rendered.lines[other], Line::Header(Group::Other));
    list.fold(&Line::Header(Group::Other));
    let text = shown(&list, &entries, 44);
    assert_eq!(text[other + 1].trim_end(), "  ▸ blog (2)");
    assert_eq!(text[other + 2].trim_end(), "  ▸ scripts (1)");
    list.fold(&Line::Header(Group::OtherDir(PathBuf::from("/x/blog"))));
    let text = shown(&list, &entries, 44);
    assert!(text[other + 2].starts_with("      ○ a"), "{text:?}");
}

#[test]
fn the_space_in_front_and_marks_take_their_faces_and_a_working_worktree_spins() {
    let mut list = List::default();
    list.set_tree(tree());
    list.set_spin(Some(1));
    let entries = [session(
        2,
        "pane-c3",
        State::Working,
        "/w/oso/.claude/worktrees/pane",
    )];
    let rendered = list.render(&entries, 44, Instant::now());
    let text = &rendered.text;
    let pane = text.find("⠙ pane").unwrap_or_else(|| panic!("{text}"));
    let face_at = |at: usize| {
        rendered
            .highlights
            .iter()
            .find(|(range, _)| range.start == at)
            .map(|(_, h)| *h)
    };
    assert_eq!(face_at(pane), Some(Highlight::SessionWorking));
    assert_eq!(face_at(pane + "⠙ ".len()), Some(Highlight::SessionFront));
    // The worktree's row and its session's both spin.
    assert_eq!(rendered.spinning.len(), 2, "{text}");
}

#[test]
fn a_worktree_with_no_running_session_of_osos_is_folded_with_its_count() {
    let mut list = List::default();
    list.set_tree(tree());
    let entries = [
        session(
            2,
            "pane-c3",
            State::Stopped,
            "/w/oso/.claude/worktrees/pane",
        ),
        external(9, "x", "/w/oso/.claude/worktrees/pane", Some("/w/oso")),
    ];
    let rendered = list.render(&entries, 44, Instant::now());
    let at = rendered
        .lines
        .iter()
        .position(|line| {
            *line
                == Line::Worktree(
                    list.tree().spaces[0].id.clone(),
                    PathBuf::from("/w/oso/.claude/worktrees/pane"),
                )
        })
        .unwrap();
    let row = rendered.text.lines().nth(at).unwrap();
    assert!(row.contains("(2)"), "{row}");
    assert!(!rendered.text.contains("pane-c3"), "{}", rendered.text);
    list.fold(&rendered.lines[at]);
    assert!(
        list.render(&entries, 44, Instant::now())
            .text
            .contains("pane-c3")
    );
}

#[test]
fn a_project_folds_and_names_from_git_are_cleaned() {
    let mut tree = tree();
    tree.spaces[0].projects[0].worktrees[2].branch = Some("evil\u{202E}\nbranch".to_owned());
    let mut list = List::default();
    list.set_tree(tree);
    let text = shown(&list, &[], 60);
    assert!(
        text.iter().any(|row| row.contains("evil branch")),
        "{text:?}"
    );
    let space = list.tree().spaces[0].id.clone();
    list.fold(&Line::Space(space));
    let text = shown(&list, &[], 60);
    assert_eq!(text.len(), 1);
    assert!(text[0].starts_with("▸ oso"));
}

#[test]
fn twenty_worktrees_forty_sessions_and_three_hundred_folded_externals_render_in_budget() {
    let t = Instant::now();
    let mut worktrees: Vec<Worktree> = (0..20)
        .map(|n| worktree(&format!("/w/oso/.claude/worktrees/wt-{n}"), "b", Some(n)))
        .collect();
    worktrees[0].primary = true;
    worktrees[0].path = PathBuf::from("/w/oso");
    let tree = Tree {
        spaces: Tree::of_projects(vec![Project {
            root: PathBuf::from("/w/oso"),
            name: "oso".to_owned(),
            worktrees,
        }])
        .spaces,
        front: None,
        wall: Some(wall()),
        ..Default::default()
    };
    let mut entries: Vec<Entry> = (0..40)
        .map(|n| {
            let dir = format!("/w/oso/.claude/worktrees/wt-{}", n % 20);
            session(
                n,
                &format!("session-{n}"),
                [ASK, State::Working, State::Idle][n as usize % 3],
                &dir,
            )
        })
        .collect();
    entries.extend((0..300).map(|n| external(10_000 + n, "x", &format!("/x/p{}", n % 30), None)));
    let mut list = List::default();
    list.set_tree(tree);
    list.set_spin(Some(0));
    let mut best = Duration::MAX;
    for _ in 0..10 {
        let start = Instant::now();
        let rendered = list.render(&entries, 44, t);
        best = best.min(start.elapsed());
        assert!(rendered.lines.len() > 60);
    }
    // A debug build is several times slower; the budget is a release build's (ADR 0131).
    assert!(cfg!(debug_assertions) || best.as_micros() < 500, "{best:?}");
}

#[test]
fn a_directory_is_held_by_its_deepest_worktree_and_a_project_found_by_its_root() {
    let tree = tree();
    let held = |dir: &str| {
        tree.holding(std::path::Path::new(dir))
            .map(|(project, worktree)| (project.name.as_str(), worktree.path.clone()))
    };
    assert_eq!(
        held("/w/oso/.claude/worktrees/pane/crates"),
        Some(("oso", PathBuf::from("/w/oso/.claude/worktrees/pane")))
    );
    assert_eq!(held("/w/oso/src"), Some(("oso", PathBuf::from("/w/oso"))));
    // A sibling whose name only starts the same is not inside.
    assert_eq!(held("/w/oso-other"), None);
    assert!(tree.project(std::path::Path::new("/w/oso")).is_some());
    assert!(tree.project(std::path::Path::new("/w/oso/src")).is_none());
}

#[test]
fn archived_sessions_take_a_sessions_indent_under_their_group() {
    let mut list = List::default();
    list.set_tree(tree());
    let gone = Entry {
        archived: true,
        ..session(9, "gone", State::Stopped, "/w/oso")
    };
    list.fold(&Line::Header(Group::Archived));
    let rows = shown(&list, &[gone], 40);
    let at = rows
        .iter()
        .position(|row| row.starts_with("▾ Archived"))
        .unwrap();
    assert!(rows[at + 1].starts_with("    ■ gone"), "{rows:#?}");
}

#[test]
fn a_session_started_without_asking_is_marked_under_its_worktree() {
    let mut list = List::default();
    list.set_tree(tree());
    let mut unasked = session(2, "pane-c3", State::Idle, "/w/oso/.claude/worktrees/pane");
    unasked.unasked = true;
    let entries = [session(1, "oso-bb", State::Working, "/w/oso"), unasked];
    let text = shown(&list, &entries, 44);
    let row = text.iter().find(|row| row.contains("pane-c3")).unwrap();
    assert!(row.contains("pane-c3  unasked"), "{text:?}");
    let other = text.iter().find(|row| row.contains("oso-bb")).unwrap();
    assert!(!other.contains("unasked"), "{text:?}");
}

/// A main checkout at `root`.
fn main_at(root: &str) -> Worktree {
    Worktree {
        primary: true,
        ..worktree(root, "main", None)
    }
}

/// A space of the tree called `name` with the repositories `projects`.
fn space(n: u8, name: &str, projects: Vec<Project>) -> super::SpaceRows {
    super::SpaceRows {
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
    assert!(text[0].starts_with("▾ fix-login"), "{text:?}");
    assert!(text[1].starts_with("  ▾ back"), "{text:?}");
    assert!(text[2].starts_with("    · main  primary"), "{text:?}");
    assert!(text[3].starts_with("    ▶ api  fix/api"), "{text:?}");
    assert!(text[4].starts_with("      ▶ api"), "{text:?}");
    assert!(text[5].starts_with("  ▾ front"), "{text:?}");
    assert!(text[6].starts_with("    · msg"), "{text:?}");
    assert!(matches!(rendered.lines[0], Line::Space(_)));
    assert!(matches!(rendered.lines[1], Line::Repository(..)));
    // A repository folds in its space alone.
    list.fold(&rendered.lines[1].clone());
    let text = shown(&list, &[], 44);
    assert!(
        text[1].starts_with("  ▸ back") && text[2].starts_with("  ▾ front"),
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
        closed: vec![super::ClosedSpace {
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
    assert!(at("▾ front").is_some(), "{text:?}");
    assert!(at("▸ quiet").is_some(), "{text:?}");
    assert!(at("▾ busy").is_some(), "{text:?}");
    let closed = at("▸ Archived spaces (1)").unwrap_or_else(|| panic!("{text:?}"));
    assert_eq!(closed, text.len() - 1);
    list.fold(&Line::Header(Group::ArchivedSpaces));
    let text = shown(&list, &[], 44);
    assert!(
        text.last().unwrap().starts_with("  ■ old  An old issue"),
        "{text:?}"
    );
}

#[test]
fn ten_spaces_of_three_repositories_and_three_hundred_externals_render_in_budget() {
    let t = Instant::now();
    let spaces: Vec<super::SpaceRows> = (0..10u8)
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
            .any(|row| row.starts_with("▸ other"))
    );
    // Opened by the person: it stays open when a session of it starts working, and after.
    list.fold(&Line::Space(other.clone()));
    let mut working = session(1, "x", State::Working, "/w/o/wt/x");
    working.space_id = Some(other.clone());
    for entries in [vec![], vec![working.clone()], vec![]] {
        let text = shown(&list, &entries, 44);
        assert!(
            text.iter().any(|row| row.starts_with("▾ other")),
            "{text:?}"
        );
    }
    // Folded by the person while it works: it stays folded.
    let _ = shown(&list, &[working.clone()], 44);
    list.fold(&Line::Space(other));
    let text = shown(&list, &[working], 44);
    assert!(
        text.iter().any(|row| row.starts_with("▸ other")),
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
    assert!(at("    ○ mine") > at("▾ second"), "{text:?}");
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
        closed: vec![super::ClosedSpace {
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
        text.iter().any(|row| row.starts_with("▸ Archived 1")),
        "{text:?}"
    );
}
