//! A status's thread over temporary repositories: reading each section, staging and unstaging
//! files and git's own hunks, a hunk git refuses, literal paths, the first commit, the roots it
//! refuses, and the commit sections and the push target (step 2).
//!
//! Must not depend on: `app`, `dispatch` or winit; these drive the thread's state and thread
//! directly, and run git only through the process module.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use super::{Report, Request, State, Worker};
use crate::git::TrustedRoot;
use crate::review::tests::Repo;
use crate::vcs::layout::{Laid, Row, Section, View};
use crate::vcs::stage::{self, Op, Target};

fn state(repo: &Repo) -> State {
    State {
        state_dir: Some(repo.dir.join("state")),
        ..State::default()
    }
}

/// What one request reported: its text, and everything else.
fn answer(state: &mut State, root: &Path, request: Request) -> (Laid, Vec<Report>) {
    let mut out = Vec::new();
    state.answer(
        vec![request],
        &TrustedRoot::for_tests(root.to_path_buf()),
        &mut out,
    );
    let mut laid = None;
    let mut other = Vec::new();
    for report in out {
        match report {
            Report::Laid { laid: one, .. } => laid = Some(*one),
            report => other.push(report),
        }
    }
    (laid.expect("every batch lays out a text"), other)
}

fn read(state: &mut State, root: &Path, view: &View) -> Laid {
    let (laid, other) = answer(
        state,
        root,
        Request::Read {
            generation: 1,
            view: view.clone(),
        },
    );
    assert!(other.is_empty(), "{other:?}");
    laid
}

/// Stages (`stage`) or unstages `targets` of `laid`, as the UI asks.
fn run(
    state: &mut State,
    root: &Path,
    laid: &Laid,
    targets: &[Target],
    stage: bool,
    view: &View,
) -> (Laid, Vec<Report>) {
    answer(
        state,
        root,
        Request::Stage {
            generation: 2,
            built: std::sync::Arc::clone(&laid.built),
            targets: targets.to_vec(),
            stage,
            view: view.clone(),
        },
    )
}

fn text(laid: &Laid) -> String {
    laid.text.text().to_string()
}

fn cached(repo: &Repo) -> String {
    String::from_utf8(repo.git(&["diff", "--cached"])).unwrap()
}

const TEN: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n";

/// A repository with `a.txt` committed, then changed near its start and near its end, so that
/// git writes two hunks.
fn two_hunks(name: &str) -> Repo {
    let repo = Repo::new(name);
    repo.write("a.txt", TEN.as_bytes());
    repo.commit("first");
    // Lines 2 and 14 only: a replace of "2\n" would change line 12 too.
    let changed = TEN
        .replacen("\n2\n", "\ntwo\n", 1)
        .replace("\n14\n", "\nfourteen\n");
    repo.write("a.txt", changed.as_bytes());
    repo
}

fn open_a(section: Section) -> View {
    let mut view = View::default();
    view.open_file(section, b"a.txt".to_vec());
    view
}

#[test]
fn the_stashes_show_after_the_staged_changes_and_a_row_names_its_stash() {
    let repo = Repo::new("vcs-stashes");
    repo.git(&["config", "core.autocrlf", "false"]);
    repo.write("a.txt", b"one\n");
    repo.commit("first");
    repo.write("a.txt", b"two\n");
    repo.git(&[
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "stash",
        "push",
        "-q",
        "-m",
        "tidy",
    ]);
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    let shown = text(&laid);
    assert!(
        shown.contains("Stashes (1)\nstash@{0} On main: tidy\n"),
        "{shown}"
    );
    let line = shown
        .lines()
        .position(|line| line.starts_with("stash@{0}"))
        .unwrap();
    let (section, stash) = laid.built.commit_at(line).expect("a stash's row");
    assert_eq!(section, Section::Stashes);
    let id = String::from_utf8(repo.git(&["rev-parse", "stash@{0}"])).unwrap();
    assert_eq!(stash.id, id.trim());
}

#[test]
fn a_stopped_pick_shows_under_the_head_with_its_unmerged_path() {
    let repo = Repo::new("vcs-stopped");
    repo.git(&["config", "core.autocrlf", "false"]);
    repo.write("a.txt", b"one\n");
    repo.commit("first");
    repo.git(&["checkout", "-q", "-b", "side"]);
    repo.write("a.txt", b"side\n");
    repo.commit("from the side");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("a.txt", b"main\n");
    repo.commit("on main");
    assert!(!repo.git_ran(&["cherry-pick", "side"]).0);
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    let shown = text(&laid);
    let short = String::from_utf8(repo.git(&["rev-parse", "--short", "side"])).unwrap();
    let line = format!("Picking:  {} from the side\n", short.trim());
    assert!(shown.contains(&line), "{shown}");
    assert!(shown.contains("C-c C-s skips"), "{shown}");
    assert!(shown.contains("Unmerged paths (1)"), "{shown}");
    let stop = laid.built.stop.as_ref().expect("a stop in the table");
    assert_eq!(stop.doing, crate::vcs::sequence::Doing::CherryPick);
    // `s` on the section stages the resolved file, which leaves Unmerged paths.
    repo.write("a.txt", b"both\n");
    let targets = [Target::Section(Section::Unmerged)];
    let (staged, other) = run(
        &mut state,
        &repo.root,
        &laid,
        &targets,
        true,
        &View::default(),
    );
    assert!(
        other
            .iter()
            .all(|report| matches!(report, Report::Ran { .. })),
        "{other:?}"
    );
    let shown = text(&staged);
    assert!(!shown.contains("Unmerged paths"), "{shown}");
    assert!(shown.contains("Staged changes (1)"), "{shown}");
    repo.git(&["cherry-pick", "--abort"]);
    let laid = read(&mut state, &repo.root, &View::default());
    assert_eq!(laid.built.stop, None);
}

#[test]
fn a_status_shows_each_section() {
    let repo = Repo::new("vcs-sections");
    repo.write("kept.txt", b"one\n");
    repo.write("staged.txt", b"one\n");
    repo.commit("first");
    repo.write("kept.txt", b"two\n");
    repo.write("staged.txt", b"two\n");
    repo.git(&["add", "staged.txt"]);
    repo.write("new.txt", b"new\n");
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    let shown = text(&laid);
    assert!(shown.starts_with("Head:     main "), "{shown}");
    assert!(shown.contains("first"), "{shown}");
    assert!(
        shown.contains("Untracked files (1)\n?  new.txt\n"),
        "{shown}"
    );
    assert!(
        shown.contains("Unstaged changes (1)\nM  kept.txt  +1 -1\n"),
        "{shown}"
    );
    assert!(
        shown.contains("Staged changes (1)\nM  staged.txt  +1 -1\n"),
        "{shown}"
    );
}

#[test]
fn a_hunk_stages_and_unstages_alone() {
    let repo = two_hunks("vcs-hunk");
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
    };
    let (_, said) = stage::ops(&laid.built, &[target], true).unwrap();
    assert_eq!(said, "Staged a hunk");
    let (_, reports) = run(&mut state, &repo.root, &laid, &[target], true, &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    let index = cached(&repo);
    assert!(index.contains("+two"), "{index}");
    assert!(!index.contains("fourteen"), "{index}");
    // And back, from the staged section's own hunk.
    let view = open_a(Section::Staged);
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::Hunk {
        section: Section::Staged,
        file: 0,
        hunk: 0,
    };
    let (laid, reports) = run(&mut state, &repo.root, &laid, &[target], false, &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert_eq!(cached(&repo), "");
    assert!(!text(&laid).contains("Staged changes"));
}

#[test]
fn lines_of_a_hunk_stage_and_unstage_alone() {
    let repo = Repo::new("vcs-lines");
    repo.write("a.txt", b"one\ntwo\nthree\n");
    repo.commit("first");
    repo.write("a.txt", b"one\nTWO\nthree\nfour\n");
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    // The one hunk is ` one`, `-two`, `+TWO`, ` three`, `+four`: `+four` alone.
    let target = Target::Lines {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
        first: 4,
        last: 4,
    };
    let (_, reports) = run(&mut state, &repo.root, &laid, &[target], true, &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    let index = String::from_utf8(repo.git(&["show", ":a.txt"])).unwrap();
    assert_eq!(index, "one\ntwo\nthree\nfour\n");
    // From the staged side, `+four` unstages by itself again.
    let view = open_a(Section::Staged);
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::Lines {
        section: Section::Staged,
        file: 0,
        hunk: 0,
        first: 3,
        last: 3,
    };
    let (_, reports) = run(&mut state, &repo.root, &laid, &[target], false, &view);
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert_eq!(cached(&repo), "");
    // The work tree never changed.
    let work = std::fs::read_to_string(repo.root.join("a.txt")).unwrap();
    assert_eq!(work, "one\nTWO\nthree\nfour\n");
}

#[test]
fn a_hunk_whose_file_moved_on_is_refused() {
    let repo = two_hunks("vcs-refused");
    let mut state = state(&repo);
    let view = open_a(Section::Unstaged);
    let laid = read(&mut state, &repo.root, &view);
    let target = Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
    };
    // Someone stages a change to the same lines meanwhile: the hunk no longer applies.
    repo.write("a.txt", b"1\nzwei\n3\n");
    repo.git(&["add", "a.txt"]);
    let before = cached(&repo);
    let (_, reports) = run(&mut state, &repo.root, &laid, &[target], true, &view);
    let [Report::Failed { why }] = reports.as_slice() else {
        panic!("refused: {reports:?}");
    };
    assert!(why.starts_with("git refused the hunk"), "{why}");
    assert_eq!(cached(&repo), before);
}

#[test]
fn files_stage_and_unstage_whole_with_their_names_taken_literally() {
    let repo = Repo::new("vcs-literal");
    repo.write("base.txt", b"base\n");
    repo.commit("first");
    let star = if cfg!(windows) { "a[b].txt" } else { "a*.txt" };
    repo.write(star, b"star\n");
    repo.write("ab.txt", b"ab\n");
    repo.write("a[b].md", b"bracket\n");
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    let index = laid
        .built
        .files(Section::Untracked)
        .iter()
        .position(|file| file.path == star.as_bytes())
        .unwrap();
    let target = Target::File {
        section: Section::Untracked,
        file: index,
    };
    let (laid, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[target],
        true,
        &View::default(),
    );
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    let names = String::from_utf8(repo.git(&["diff", "--cached", "--name-only"])).unwrap();
    assert_eq!(names.trim(), star);
    let (_, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[Target::Section(Section::Staged)],
        false,
        &View::default(),
    );
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert_eq!(cached(&repo), "");
}

#[test]
fn before_the_first_commit_unstaging_takes_the_file_out_of_the_index() {
    let repo = Repo::new("vcs-initial");
    repo.write("first.txt", b"first\n");
    let mut state = state(&repo);
    let laid = read(&mut state, &repo.root, &View::default());
    assert!(text(&laid).contains("(no commits yet)"));
    let (laid, _) = run(
        &mut state,
        &repo.root,
        &laid,
        &[Target::Section(Section::Untracked)],
        true,
        &View::default(),
    );
    assert!(
        text(&laid).contains("Staged changes (1)\nA  first.txt"),
        "{}",
        text(&laid)
    );
    let (laid, reports) = run(
        &mut state,
        &repo.root,
        &laid,
        &[Target::Section(Section::Staged)],
        false,
        &View::default(),
    );
    assert!(
        matches!(reports.as_slice(), [Report::Ran { .. }]),
        "{reports:?}"
    );
    assert!(
        text(&laid).contains("Untracked files (1)"),
        "{}",
        text(&laid)
    );
}

#[test]
fn a_file_that_may_hold_secrets_is_never_read_for_hunks() {
    let repo = Repo::new("vcs-secret");
    repo.write(".env", b"TOKEN=one\n");
    repo.commit("first");
    repo.write(".env", b"TOKEN=two\n");
    let mut state = state(&repo);
    let mut view = View::default();
    view.open_file(Section::Unstaged, b".env".to_vec());
    let laid = read(&mut state, &repo.root, &view);
    assert!(state.hunks.is_empty());
    let shown = text(&laid);
    assert!(!shown.contains("TOKEN"), "{shown}");
    assert!(shown.contains("M  .env  +1 -1"), "{shown}");
    let row = laid
        .built
        .rows
        .iter()
        .position(|row| matches!(row, Row::File { .. }))
        .unwrap();
    let targets = stage::targets(
        &laid.built,
        &[oso_core::editor::GitLines {
            first: row,
            last: row,
            region: false,
        }],
    )
    .unwrap();
    let (ops, _) = stage::ops(&laid.built, &targets, true).unwrap();
    assert_eq!(ops, [Op::Update(vec![b".env".to_vec()])]);
}

#[test]
fn a_root_below_the_top_of_its_repository_finds_none() {
    let repo = Repo::new("vcs-not-top");
    repo.write("sub/a.txt", b"a\n");
    repo.commit("first");
    let mut state = state(&repo);
    let (laid, reports) = answer(
        &mut state,
        &repo.root.join("sub"),
        Request::Read {
            generation: 1,
            view: View::default(),
        },
    );
    let [Report::Failed { why }] = reports.as_slice() else {
        panic!("refused: {reports:?}");
    };
    // git never looks above the root for a repository, so it finds none.
    assert!(why.contains("not a git repository"), "{why}");
    assert!(text(&laid).contains("git status failed"));
    assert!(state.git.is_none());
}

#[test]
fn a_hooks_directory_that_is_not_empty_is_refused() {
    let repo = Repo::new("vcs-hooks");
    let hooks = repo.dir.join("state/vcs/hooks");
    std::fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    let hooks = oso_platform::privacy::PrivateDir::open(hooks).unwrap();
    std::fs::write(hooks.path().join("pre-commit"), b"#!/bin/sh\n").unwrap();
    let mut state = state(&repo);
    let (_, reports) = answer(
        &mut state,
        &repo.root,
        Request::Read {
            generation: 1,
            view: View::default(),
        },
    );
    let [Report::Failed { why }] = reports.as_slice() else {
        panic!("refused: {reports:?}");
    };
    assert!(why.contains("not empty"), "{why}");
}

#[test]
fn the_thread_runs_nothing_for_a_revoked_root_and_takes_a_batch_as_one() {
    let repo = two_hunks("vcs-thread");
    let root = TrustedRoot::for_tests(repo.root.clone());
    let (sent, got) = mpsc::channel();
    let worker = Worker::start(root.clone(), Some(repo.dir.join("state")), move |report| {
        sent.send(report).is_ok()
    })
    .unwrap();
    for generation in 1..=3 {
        assert!(worker.ask(Request::Read {
            generation,
            view: View::default(),
        }));
    }
    let wait = Duration::from_secs(20);
    let mut newest = 0;
    while newest < 3 {
        match got.recv_timeout(wait).unwrap() {
            Report::Laid { generation, .. } => newest = generation,
            other => panic!("{other:?}"),
        }
    }
    root.revoke();
    assert!(worker.ask(Request::Read {
        generation: 4,
        view: View::default(),
    }));
    let Report::Failed { why } = got.recv_timeout(wait).unwrap() else {
        panic!("a revoked root reads nothing");
    };
    assert!(why.contains("no longer trusted"), "{why}");
}

mod discard;
mod guards;
mod lock;

#[test]
fn the_commit_sections_show_what_the_upstream_lacks_and_holds() {
    let repo = Repo::new("work-commits");
    for (key, value) in [
        ("user.name", "t"),
        ("user.email", "t@t"),
        ("commit.gpgsign", "false"),
    ] {
        repo.git(&["config", key, value]);
    }
    let mut state = state(&repo);
    // No commit yet: no commit section, and no push line.
    let laid = read(&mut state, &repo.root, &View::default());
    assert!(!text(&laid).contains("Recent commits"), "{}", text(&laid));
    std::fs::write(repo.root.join("a.txt"), "one\n").unwrap();
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-q", "-m", "first"]);
    // Without Windows' verbatim prefix, which git cannot take.
    let bare = crate::git::for_git(&repo.dir.join("remote.git"));
    let bare = bare.to_str().unwrap();
    repo.git(&["init", "-q", "--bare", "-b", "main", bare]);
    repo.git(&["remote", "add", "origin", bare]);
    repo.git(&["push", "-q", "-u", "origin", "main"]);
    repo.git(&["commit", "-q", "--allow-empty", "-m", "second"]);
    let laid = read(&mut state, &repo.root, &View::default());
    let shown = text(&laid);
    assert!(shown.contains("Unpushed to origin/main (1)\n"), "{shown}");
    assert!(!shown.contains("Unpulled from"), "{shown}");
    assert!(shown.contains("Recent commits (2)\n"), "{shown}");
    // `@{push}` is the upstream here, so no push line.
    assert!(!shown.contains("Push:"), "{shown}");
    // The head's line names the subject too: the row is the one under the heading.
    let heading = (shown.lines())
        .position(|line| line.starts_with("Unpushed to"))
        .unwrap();
    let line = heading + 1;
    let (section, commit) = laid.built.commit_at(line).unwrap();
    assert_eq!(section, Section::Unpushed);
    let head = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    assert_eq!(commit.id, head.trim());
    // A push target elsewhere gets its own line. git resolves no `@{push}` for `simple` to
    // another remote than the upstream's, so `current`.
    repo.git(&["remote", "add", "fork", bare]);
    repo.git(&["config", "remote.pushDefault", "fork"]);
    repo.git(&["config", "push.default", "current"]);
    repo.git(&["fetch", "-q", "fork"]);
    let laid = read(&mut state, &repo.root, &View::default());
    assert!(
        text(&laid).contains("Push:     fork/main\n"),
        "{}",
        text(&laid)
    );
    assert!(matches!(
        laid.built.rows.get(line),
        Some(Row::Commit { .. }) | Some(_)
    ));
}
