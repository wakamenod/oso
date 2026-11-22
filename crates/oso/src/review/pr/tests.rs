//! A pull request's sides against temporary repositories and a stand-in `gh`: the merge-base,
//! the head's tree or the work tree, the head branch's worktree, a fork, a branch behind the
//! pull request, fetching what the clone lacks from a local remote as objects alone, what a
//! failed fetch says, a stop while `gh` runs, and what is refused.
//!
//! Must not depend on: a real `gh` or the network; the remote is a bare repository on disk.

#![cfg(unix)]

use std::cell::Cell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use oso_platform::process::Executable;

use super::{
    Asking, Meanwhile, PrFound, PullFrom, Waits, fetch_failure, fetch_words, head_worktree_with,
    plain_branch, read_sides, resolve_with,
};
use crate::git::{Gh, GhRepo, Git, Id, PullTarget};
use crate::review::scope::{New, ScopeError, Sides};
use crate::review::tests::Repo;

/// A repository, a stand-in `gh` that answers `pr view` with `view.json`, and the empty
/// directories it and git run with. While `view.hold` is there, for at most 30 s, `gh` holds
/// its answer; it marks `answered` as it gives it.
struct Bench {
    repo: Repo,
    hooks: PathBuf,
    answers: PathBuf,
}

impl Bench {
    fn new(name: &str) -> Bench {
        let repo = Repo::new(name);
        let hooks = repo.dir.join("hooks");
        let answers = repo.dir.join("answers");
        fs::create_dir(&hooks).unwrap();
        fs::create_dir(&answers).unwrap();
        let a = answers.display();
        let script = answers.join("gh");
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{a}/asked'\n\
                 i=0; while [ -e '{a}/view.hold' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done\n\
                 touch '{a}/answered'\ncat '{a}/view.json'\n\
                 cat '{a}/view.err' >&2 2>/dev/null\n\
                 exit $(cat '{a}/view.code' 2>/dev/null || echo 0)\n"
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        repo.write("a.txt", b"one\n");
        repo.commit("first");
        Bench {
            repo,
            hooks,
            answers,
        }
    }

    fn git(&self) -> Git {
        Git::new(&self.repo.root, &self.hooks).unwrap()
    }

    /// `gh pr view` answers pull request `number` from `base` (on `main`) to `head` (on
    /// `branch`).
    fn answer(&self, number: u64, base: &str, head: &str, branch: &str, fork: bool) {
        let json = format!(
            r#"{{"number":{number},"title":"Add\u001b[31m x","baseRefName":"main",
            "baseRefOid":"{base}","headRefName":"{branch}","headRefOid":"{head}",
            "isCrossRepository":{fork}}}"#
        );
        fs::write(self.answers.join("view.json"), json).unwrap();
    }

    fn gh(&self) -> Gh {
        Gh::at(Executable::absolute(self.answers.join("gh")).unwrap())
    }

    /// What `PullTarget::of` would find, with `remote` as the remote.
    fn target(&self, remote: &str) -> PullTarget {
        let branch = String::from_utf8(self.repo.git(&["symbolic-ref", "--short", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        PullTarget {
            github: GhRepo::from_url("https://github.com/o/r.git", None).unwrap(),
            branch: Some(branch),
            remote: remote.into(),
        }
    }

    /// Resolves `number`, counting how often the review was asked to answer what waits, and
    /// the fetches it was told of.
    fn resolve_counting(
        &self,
        number: u64,
        remote: &str,
        stop: &dyn Fn() -> bool,
    ) -> (Result<(Sides, PrFound), ScopeError>, usize, Vec<u64>) {
        self.resolve_asking(number, remote, stop, &mut None)
    }

    /// [`Bench::resolve_counting`], with the run of `gh` a stop left waiting in `asking`.
    fn resolve_asking(
        &self,
        number: u64,
        remote: &str,
        stop: &dyn Fn() -> bool,
        asking: &mut Option<Asking>,
    ) -> (Result<(Sides, PrFound), ScopeError>, usize, Vec<u64>) {
        let (gh, target) = (self.gh(), self.target(remote));
        let from = PullFrom {
            gh: &gh,
            cwd: &self.hooks,
            target: &target,
        };
        let mut idle = 0;
        let mut fetched = Vec::new();
        let mut tell = |meanwhile: Meanwhile<'_>| {
            match meanwhile {
                Meanwhile::Idle => idle += 1,
                Meanwhile::Fetching { number, .. } => fetched.push(number),
            }
            true
        };
        let mut waits = Waits {
            stop,
            tell: &mut tell,
        };
        let resolved = resolve_with(&self.git(), &from, number, asking, &mut waits);
        (resolved, idle, fetched)
    }

    /// Where `number`'s head is checked out, as a start in a space finds it before opening.
    fn head_worktree(&self, number: u64) -> Option<PathBuf> {
        let (gh, target) = (self.gh(), self.target("origin"));
        let from = PullFrom {
            gh: &gh,
            cwd: &self.hooks,
            target: &target,
        };
        let mut tell = |_: Meanwhile<'_>| true;
        let mut waits = Waits {
            stop: &|| false,
            tell: &mut tell,
        };
        let found = head_worktree_with(&self.git(), &from, number, &mut waits).unwrap();
        found.map(|path| fs::canonicalize(path).unwrap())
    }

    fn resolve(&self, number: u64, remote: &str) -> Result<(Sides, PrFound), ScopeError> {
        self.resolve_counting(number, remote, &|| false).0
    }

    fn asked(&self) -> String {
        fs::read_to_string(self.answers.join("asked")).unwrap_or_default()
    }
}

fn tip(repo: &Repo, revision: &str) -> String {
    String::from_utf8(repo.git(&["rev-parse", revision]))
        .unwrap()
        .trim()
        .to_owned()
}

fn id(text: &str) -> Id {
    Id::parse(text.as_bytes()).unwrap()
}

/// `feature` leaves `main` at its first commit, and `main` moves on after.
fn branched(repo: &Repo) -> (String, String, String) {
    let fork = tip(repo, "HEAD");
    repo.git(&["checkout", "-q", "-b", "feature"]);
    repo.write("b.txt", b"two\n");
    repo.commit("feature");
    let head = tip(repo, "HEAD");
    repo.git(&["checkout", "-q", "main"]);
    repo.write("c.txt", b"three\n");
    repo.commit("main moves on");
    (fork, tip(repo, "HEAD"), head)
}

#[test]
fn the_old_side_is_where_the_head_left_the_base_and_the_new_its_head() {
    let bench = Bench::new("pr-sides");
    let (fork, base, head) = branched(&bench.repo);
    bench.answer(7, &base, &head, "feature", false);
    let (sides, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(sides.old, Some(id(&fork)), "not the base, which moved on");
    assert_eq!(sides.new, New::Tree(id(&head)));
    assert_eq!(found.number, 7);
    assert!(!found.head_checked_out);
    assert_eq!(found.worktree, None, "no worktree has feature");
    assert_eq!(found.head_branch, "feature");
    // The title is kept as gh gave it, for the echo area to escape.
    assert!(found.title.contains('\u{1b}'));
    let asked = bench.asked();
    assert!(
        asked.starts_with("pr view 7 --repo github.com/o/r --json number,title,"),
        "{asked}"
    );
}

#[test]
fn comments_go_to_the_worktree_the_head_branch_is_checked_out_in_holding_its_head() {
    let bench = Bench::new("pr-worktree");
    let (_, base, head) = branched(&bench.repo);
    let tree = bench.repo.dir.join("feature-tree");
    let path = tree.to_str().unwrap();
    bench.repo.git(&["worktree", "add", "-q", path, "feature"]);
    bench.answer(7, &base, &head, "feature", false);
    let (_, found) = bench.resolve(7, "origin").unwrap();
    let worktree = found.worktree.unwrap();
    assert_eq!(
        fs::canonicalize(worktree).unwrap(),
        fs::canonicalize(&tree).unwrap()
    );
    // A fork's branch of that name is another branch.
    bench.answer(7, &base, &head, "feature", true);
    let (_, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(found.worktree, None);
    // A branch of that name at a commit that does not hold the pull request's head is
    // another branch too.
    bench.answer(7, &base, &base, "feature", false);
    let (_, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(found.worktree, None);
}

#[test]
fn a_head_branch_checked_out_here_holding_the_head_shows_the_work_tree() {
    let bench = Bench::new("pr-here");
    let (fork, base, head) = branched(&bench.repo);
    bench.repo.git(&["checkout", "-q", "feature"]);
    bench.answer(7, &base, &head, "feature", false);
    let (sides, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(sides.old, Some(id(&fork)));
    assert_eq!(sides.new, New::WorkTree { untracked: true });
    assert!(found.head_checked_out);
    assert_eq!(
        found.worktree.map(|p| fs::canonicalize(p).unwrap()),
        Some(fs::canonicalize(&bench.repo.root).unwrap())
    );
    // A fork's head of the same name is not what is checked out.
    bench.answer(7, &base, &head, "feature", true);
    let (sides, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(sides.new, New::Tree(id(&head)));
    assert!(!found.head_checked_out);
}

#[test]
fn a_head_branch_checked_out_behind_the_pull_request_shows_its_head_commit() {
    let bench = Bench::new("pr-behind");
    let (_, base, _) = branched(&bench.repo);
    // The pull request's head is one commit past the local `feature`, as after a push the
    // person has not pulled.
    bench.repo.git(&["checkout", "-q", "feature"]);
    bench.repo.write("pushed.txt", b"not pulled\n");
    bench.repo.commit("pushed elsewhere");
    let pushed = tip(&bench.repo, "HEAD");
    bench.repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    bench.answer(7, &base, &pushed, "feature", false);
    let (sides, found) = bench.resolve(7, "origin").unwrap();
    assert_eq!(sides.new, New::Tree(id(&pushed)), "the pushed commit shows");
    assert!(!found.head_checked_out);
    assert_eq!(found.worktree, None);
}

/// A bare repository beside `bench`'s holding its commits, `refs/pull/7/head` at
/// `feature`, as GitHub keeps one, and `main`; then those commits made unreachable in
/// `bench`'s own repository, as a clone that never fetched them has none.
fn remote_holding(bench: &Bench) -> (String, String) {
    let (_, base, head) = branched(&bench.repo);
    let bare = bench.repo.dir.join("upstream.git");
    let root = bench.repo.root.to_str().unwrap();
    bench
        .repo
        .git(&["clone", "-q", "--bare", root, bare.to_str().unwrap()]);
    let in_bare = |args: &[&str]| {
        let mut all = vec!["--git-dir", bare.to_str().unwrap()];
        all.extend(args);
        bench.repo.git(&all)
    };
    in_bare(&["update-ref", "refs/pull/7/head", &head]);
    in_bare(&["branch", "-q", "-D", "feature"]);
    // The clone forgets both: main back to its first commit, feature gone, the objects pruned.
    bench.repo.git(&["reset", "-q", "--hard", "HEAD~1"]);
    bench.repo.git(&["branch", "-q", "-D", "feature"]);
    bench
        .repo
        .git(&["reflog", "expire", "--expire=now", "--all"]);
    bench.repo.git(&["gc", "-q", "--prune=now"]);
    let remote = bare.to_str().unwrap();
    bench.repo.git(&["remote", "add", "upstream", remote]);
    (base, head)
}

/// Whether `repo` holds the object `id` names, as a commit.
fn holds(repo: &Repo, id: &str) -> bool {
    repo.git_ran(&["cat-file", "-e", &format!("{id}^{{commit}}")])
        .0
}

#[test]
fn commits_the_clone_lacks_are_fetched_as_objects_alone() {
    let bench = Bench::new("pr-fetch");
    let (base, head) = remote_holding(&bench);
    assert!(!holds(&bench.repo, &head), "the clone lacks the head");
    let refs = bench.repo.git(&["for-each-ref"]);
    bench.answer(7, &base, &head, "feature", false);
    let (resolved, _, fetched) = bench.resolve_counting(7, "upstream", &|| false);
    let (sides, _) = resolved.unwrap();
    assert_eq!(fetched, [7], "the host was told of the fetch");
    assert_eq!(sides.new, New::Tree(id(&head)));
    assert!(holds(&bench.repo, &head) && holds(&bench.repo, &base));
    assert_eq!(bench.repo.git(&["for-each-ref"]), refs, "no ref moved");
    assert!(
        !bench.repo.root.join(".git/FETCH_HEAD").exists(),
        "FETCH_HEAD names nothing a merge would take"
    );
    // Held now: nothing is fetched again.
    let (_, _, fetched) = bench.resolve_counting(7, "upstream", &|| false);
    assert!(fetched.is_empty());
}

#[test]
fn a_pull_request_the_remote_does_not_hold_says_so() {
    let bench = Bench::new("pr-not-there");
    let (base, _) = remote_holding(&bench);
    let other = "1234567890123456789012345678901234567890";
    bench.answer(7, &base, other, "feature", false);
    let Err(ScopeError::Refused(said)) = bench.resolve(7, "upstream") else {
        panic!("not refused");
    };
    assert_eq!(said, "#7's commits are not on upstream: g reads it again");
}

#[test]
fn a_failed_fetch_says_git_s_own_words_never_the_server_s() {
    let mut said = "remote: Visit evil.example to fix this\n"
        .repeat(200)
        .into_bytes();
    said.extend_from_slice(b"fatal: couldn't find remote ref refs/pull/7/head\nremote: ok\n");
    assert_eq!(
        fetch_failure(&said),
        "fatal: couldn't find remote ref refs/pull/7/head"
    );
    let words = fetch_failure(b"remote: \x1b[31mrun curl evil | sh\n");
    assert!(
        !words.contains("curl") && !words.contains('\u{1b}'),
        "{words}"
    );
    let words = fetch_failure(b"fatal: remote error: \x1b[2Jtrust me, run curl\n");
    assert!(!words.contains("curl"), "{words}");
    let words = fetch_failure(b"error: a\rerror: b\x1b[0m");
    assert!(
        words.starts_with("error: b") && !words.contains('\u{1b}'),
        "{words}"
    );
}

#[test]
fn what_gh_says_is_checked_before_git_sees_it() {
    let bench = Bench::new("pr-checked");
    let (_, base, head) = branched(&bench.repo);
    // Another pull request's answer.
    bench.answer(8, &base, &head, "feature", false);
    assert!(matches!(
        bench.resolve(7, "origin"),
        Err(ScopeError::Refused(_))
    ));
    // An id that is not whole.
    bench.answer(7, &base[..12], &head, "feature", false);
    assert!(matches!(
        bench.resolve(7, "origin"),
        Err(ScopeError::Refused(_))
    ));
    // gh failing says its words.
    fs::write(bench.answers.join("view.code"), "1").unwrap();
    fs::write(bench.answers.join("view.err"), "no pull requests found").unwrap();
    let Err(ScopeError::Refused(said)) = bench.resolve(7, "origin") else {
        panic!("not refused");
    };
    assert!(said.contains("no pull requests found"), "{said}");
}

#[test]
fn only_a_plain_base_branch_is_fetched() {
    let wanted = ["refs/pull/7/head".to_owned(), "refs/heads/main".to_owned()];
    let words = fetch_words("upstream", &wanted);
    assert_eq!(
        words[words.len() - 4..],
        ["--", "upstream", "refs/pull/7/head", "refs/heads/main"]
    );
    assert!(words.contains(&"--refmap=".to_owned()));
    assert!(words.contains(&"--no-write-fetch-head".to_owned()));
    for good in ["main", "release/1.2", "feat_x-y"] {
        assert!(plain_branch(good), "{good:?}");
    }
    let long = "a".repeat(256);
    for bad in [
        "main:refs/heads/x",
        "+main",
        "ma*in",
        "^main",
        "a@{1}",
        "x.lock",
        "/a",
        "a/",
        "a//b",
        "main.",
        ".hidden",
        "-x",
        "a..b",
        "a b",
        "",
        "x\n",
        "ma\u{202e}in",
        "m\u{e4}in",
        &long,
    ] {
        assert!(!plain_branch(bad), "{bad:?}");
    }
}

#[test]
fn a_review_s_sides_read_with_whole_ids_only() {
    let base = "a".repeat(40);
    let head = "b".repeat(64);
    let json = format!(
        r#"{{"number":7,"title":"T","baseRefName":"main","baseRefOid":"{base}",
        "headRefName":"feature","headRefOid":"{head}","isCrossRepository":false}}"#
    );
    let sides = read_sides(json.as_bytes()).unwrap();
    assert_eq!(sides.number, 7);
    assert_eq!((sides.base, sides.head), (id(&base), id(&head)));
    assert_eq!(
        (&*sides.base_branch, &*sides.head_branch),
        ("main", "feature")
    );
    assert!(!sides.fork);
    for (from, to) in [
        (format!(r#""{head}""#), r#""abc""#.to_owned()),
        (format!(r#""{head}""#), format!(r#""{}""#, "g".repeat(40))),
        (format!(r#""{head}""#), format!(r#""{}""#, "B".repeat(40))),
        ("false".to_owned(), r#""no""#.to_owned()),
        (r#""main""#.to_owned(), "5".to_owned()),
    ] {
        let changed = json.replacen(&from, &to, 1);
        assert_ne!(changed, json, "{from}");
        assert_eq!(read_sides(changed.as_bytes()), None, "{to}");
    }
}

#[test]
fn a_stop_ends_the_wait_for_gh_at_once_answering_carries_meanwhile() {
    let bench = Bench::new("pr-stop");
    let (_, base, head) = branched(&bench.repo);
    bench.answer(7, &base, &head, "feature", false);
    // `gh` holds its answer, so the stop comes while it is waited for, however slowly a loaded
    // machine runs what comes before: a clock started there once ran past `gh`'s answer.
    let hold = bench.answers.join("view.hold");
    fs::write(&hold, "").unwrap();
    // The first look is before `gh` is asked; the stop comes on the sixth, while it waits.
    let looks = Cell::new(0);
    let stop = || {
        looks.set(looks.get() + 1);
        looks.get() > 5
    };
    let (resolved, idle, _) = bench.resolve_counting(7, "origin", &stop);
    assert!(matches!(resolved, Err(ScopeError::Stopped)), "{resolved:?}");
    assert!(
        !bench.answers.join("answered").exists(),
        "the stop waited for gh's answer"
    );
    fs::remove_file(&hold).unwrap();
    assert!(idle >= 3, "carries were answered while gh ran: {idle}");
    // A stop before anything runs asks nothing.
    let bench = Bench::new("pr-stop-first");
    let (resolved, _, _) = bench.resolve_counting(7, "origin", &|| true);
    assert!(matches!(resolved, Err(ScopeError::Stopped)));
    assert_eq!(bench.asked(), "", "gh was not asked");
}

#[test]
fn a_run_of_gh_a_stop_left_waiting_is_taken_up_not_started_again() {
    let bench = Bench::new("pr-taken-up");
    let (_, base, head) = branched(&bench.repo);
    bench.answer(7, &base, &head, "feature", false);
    // `gh` holds its answer through the three stops, however long a loaded machine takes
    // between them.
    let hold = bench.answers.join("view.hold");
    fs::write(&hold, "").unwrap();
    let mut asking = None;
    for _ in 0..3 {
        // The first look is before `gh` is asked; the stop comes while it is waited for.
        let looks = Cell::new(0);
        let stop = || {
            looks.set(looks.get() + 1);
            looks.get() > 2
        };
        let (resolved, _, _) = bench.resolve_asking(7, "origin", &stop, &mut asking);
        assert!(matches!(resolved, Err(ScopeError::Stopped)));
        assert!(asking.is_some(), "the run waits to be taken up");
    }
    fs::remove_file(&hold).unwrap();
    let (resolved, _, _) = bench.resolve_asking(7, "origin", &|| false, &mut asking);
    assert!(resolved.is_ok(), "{resolved:?}");
    assert!(asking.is_none());
    assert_eq!(bench.asked().lines().count(), 1, "gh was asked once");
}

#[test]
fn a_start_in_a_space_finds_the_worktree_holding_the_head_before_opening() {
    let bench = Bench::new("pr-head-worktree");
    let (_, base, head) = branched(&bench.repo);
    bench.answer(7, &base, &head, "feature", false);
    assert_eq!(bench.head_worktree(7), None, "no worktree has feature");
    let tree = bench.repo.dir.join("feature-tree");
    let path = tree.to_str().unwrap();
    bench.repo.git(&["worktree", "add", "-q", path, "feature"]);
    assert_eq!(
        bench.head_worktree(7),
        Some(fs::canonicalize(&tree).unwrap())
    );
    // A fork's head of that name is another branch.
    bench.answer(7, &base, &head, "feature", true);
    assert_eq!(bench.head_worktree(7), None);
    // A branch of that name that does not hold the head is another branch too.
    bench.answer(7, &base, &base, "feature", false);
    assert_eq!(bench.head_worktree(7), None);
    // Checked out here, holding the head: the root itself.
    bench.repo.git(&["worktree", "remove", "-f", path]);
    bench.repo.git(&["checkout", "-q", "feature"]);
    bench.answer(7, &base, &head, "feature", false);
    assert_eq!(
        bench.head_worktree(7),
        Some(fs::canonicalize(&bench.repo.root).unwrap())
    );
}
