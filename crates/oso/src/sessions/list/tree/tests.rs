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
        title: String::new(),
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
        pull: None,
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
    // The title, with the states that ask for the person among oso's own sessions.
    assert_eq!(text[0], format!(" Spaces{}⚠ 1  ▶ 1", " ".repeat(29)));
    // A space's row is a section: its name, a rule, its roll-up (ADR 0140).
    assert_eq!(text[1], format!(" ▾ oso {} ⚠ 1", "─".repeat(33)));
    // A worktree's row begins with its fold, its roll-up at the right before its age.
    assert!(text[2].starts_with("   ▾ main  primary"), "{text:?}");
    assert!(text[2].ends_with("▶  now"), "{text:?}");
    // Its sessions hang from it, the last on a corner.
    assert!(text[3].starts_with("     ├ ▶ oso-bb"), "{text:?}");
    // An external session in the worktree comes after oso's own.
    assert!(text[4].starts_with("     └ ○ oso-2"), "{text:?}");
    // The newest made next; its branch shows as it is not the directory's name.
    assert!(text[5].starts_with("   ▾ pane  feature/pane"), "{text:?}");
    assert!(text[6].starts_with("     └ ⚠ pane-c3"), "{text:?}");
    // A worktree with no session is folded, with no mark, nothing to show.
    assert!(
        text[7].starts_with("   ▸ old-fix") && text[7].ends_with("3d"),
        "{text:?}"
    );
    // One unchanged for a month waits in the quiet row, folded.
    assert_eq!(text[8].trim_end(), "   ▸ 1 quiet");
    assert_eq!(text.len(), 9, "{text:?}");
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
        .position(|row| row.starts_with(" ▸ Other projects ─"));
    let other = other.unwrap_or_else(|| panic!("{text:?}"));
    assert!(text[other].ends_with('3'), "{text:?}");
    assert_eq!(rendered.lines[other], Line::Header(Group::Other));
    list.fold(&Line::Header(Group::Other));
    let text = shown(&list, &entries, 44);
    assert_eq!(text[other + 1].trim_end(), "   ▸ blog (2)");
    assert_eq!(text[other + 2].trim_end(), "   ▸ scripts (1)");
    list.fold(&Line::Header(Group::OtherDir(PathBuf::from("/x/blog"))));
    let text = shown(&list, &entries, 44);
    assert!(text[other + 2].starts_with("       ○ a"), "{text:?}");
    // A section is drawn on its band.
    assert!(
        rendered.bands.sections.contains(&other),
        "{:?}",
        rendered.bands
    );
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
    let face_at = |at: usize| {
        rendered
            .highlights
            .iter()
            .find(|(range, _)| range.start == at)
            .map(|(_, h)| *h)
    };
    let space = text.find("oso ─").unwrap_or_else(|| panic!("{text}"));
    assert_eq!(face_at(space), Some(Highlight::SessionFront));
    let worktree = text.find("▾ pane").unwrap_or_else(|| panic!("{text}"));
    assert_eq!(
        face_at(worktree + "▾ ".len()),
        Some(Highlight::SessionFront)
    );
    let session = text.find("⠙ pane-c3").unwrap_or_else(|| panic!("{text}"));
    assert_eq!(face_at(session), Some(Highlight::SessionWorking));
    // The worktree's roll-up, at its row's right, and its session's mark both spin.
    let rows: Vec<&str> = text.lines().collect();
    assert!(rows[rendered.spinning[0]].ends_with("⠙  now"), "{text}");
    assert_eq!(rendered.spinning.len(), 2, "{text}");
    // The front worktree's row and its session's are on its tint (ADR 0140).
    assert_eq!(rendered.bands.tint, rendered.spinning, "{text}");
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
    assert_eq!(text.len(), 2, "{text:?}");
    assert!(text[1].starts_with(" ▸ oso ─"), "{text:?}");
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
        wall: Some(wall()),
        ..Default::default()
    };
    let front_space = tree.spaces[0].id.clone();
    let tree = Tree {
        front_space: Some(front_space),
        front: Some(PathBuf::from("/w/oso/.claude/worktrees/wt-1")),
        ..tree
    };
    // Half named after their worktree, so their titles label them; every fifth nested; the
    // front worktree tinted (ADR 0140): every path of a row is taken.
    let mut entries: Vec<Entry> = (0..40)
        .map(|n| {
            let dir = format!("/w/oso/.claude/worktrees/wt-{}", n % 20);
            let name = if n % 2 == 0 {
                format!("wt-{}", n % 20)
            } else {
                format!("session-{n}")
            };
            let mut entry = session(
                n,
                &name,
                [ASK, State::Working, State::Idle][n as usize % 3],
                &dir,
            );
            entry.title = "t".repeat(80);
            entry.parent = (n % 5 == 4 && n >= 20).then(|| Key::Hosted(n - 20));
            entry
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
        .position(|row| row.starts_with(" ▾ Archived ─"))
        .unwrap_or_else(|| panic!("{rows:#?}"));
    assert!(rows[at + 1].starts_with("     ■ gone"), "{rows:#?}");
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

mod spaces;

#[test]
fn a_worktree_s_row_shows_its_branch_s_pull_request() {
    let mut with = worktree("/r/.claude/worktrees/w", "feature", Some(1));
    with.pull = Some("PR #12 open\u{202e}".into());
    let (name, tag) = super::worktree::names(&with);
    assert_eq!(name, "w");
    let tag = tag.unwrap();
    assert!(tag.starts_with("feature, PR #12 open"), "{tag}");
    assert!(!tag.contains('\u{202e}'), "escaped: {tag}");
}
