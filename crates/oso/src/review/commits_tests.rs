//! A pull request's commits and a commit's message against temporary repositories (ADR 0167):
//! the order, merges and commits that change nothing, commits not pushed, the limits, and the
//! words escaped.
//!
//! Must not depend on: `app`, `dispatch` or winit; git runs through the process module.

use std::fs;
use std::path::PathBuf;

use super::{
    MAX_COMMITS, MAX_MESSAGE_LINES, SinceAsk, list, message, patch_ids, read_listing, read_message,
    since,
};
use crate::git::{Git, Id};
use crate::review::tests::Repo;

/// A repository with `main` at one commit, and the empty hooks directory git runs with.
struct Bench {
    repo: Repo,
    hooks: PathBuf,
}

impl Bench {
    fn new(name: &str) -> Bench {
        let repo = Repo::new(name);
        let hooks = repo.dir.join("hooks");
        fs::create_dir(&hooks).unwrap();
        repo.write("a.txt", b"one\n");
        repo.commit("first");
        Bench { repo, hooks }
    }

    fn git(&self) -> Git {
        Git::new(&self.repo.root, &self.hooks).unwrap()
    }

    fn id(&self, revision: &str) -> Id {
        let out = self.repo.git(&["rev-parse", revision]);
        Id::parse(out.trim_ascii()).unwrap()
    }
}

#[test]
fn the_commits_are_listed_oldest_first_with_merges_and_empty_ones_marked() {
    let bench = Bench::new("commits-listed");
    let repo = &bench.repo;
    let base = bench.id("HEAD");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("b.txt", b"two\n");
    repo.commit("Add b");
    repo.git(&["commit", "-q", "--allow-empty", "-m", "Nothing"]);
    repo.git(&["switch", "-q", "main"]);
    repo.write("c.txt", b"three\n");
    repo.commit("Add c on main");
    repo.git(&["switch", "-q", "topic"]);
    repo.git(&["merge", "-q", "--no-edit", "main"]);
    repo.write("b.txt", b"two\nmore\n");
    repo.commit("More b");
    let head = bench.id("HEAD");
    let listed = list(&bench.git(), &base, &head, &head, &|| false).unwrap();
    assert!(!listed.more);
    let subjects: Vec<&str> = listed.commits.iter().map(|c| c.subject.as_str()).collect();
    // `main`'s own commit is reached through the merge.
    assert_eq!(subjects.len(), 5, "{subjects:?}");
    assert_eq!(subjects.first(), Some(&"Add b"));
    assert_eq!(subjects.last(), Some(&"More b"));
    let by = |subject: &str| {
        listed
            .commits
            .iter()
            .find(|c| c.subject == subject)
            .unwrap()
    };
    assert!(by("Nothing").empty && !by("Nothing").merge);
    assert!(by("Merge branch 'main' into topic").merge);
    assert!(!by("Add b").empty && !by("Add b").merge);
    assert!(!by("More b").unpushed);
    assert_eq!(by("Add b").author, "t");
    assert_eq!(by("Add b").date.len(), 10, "YYYY-MM-DD");
    assert_eq!(by("Add b").id, bench.id("topic~3"));
}

#[test]
fn commits_past_the_head_are_marked_not_pushed() {
    let bench = Bench::new("commits-unpushed");
    let repo = &bench.repo;
    let base = bench.id("HEAD");
    repo.write("b.txt", b"two\n");
    repo.commit("Pushed");
    let head = bench.id("HEAD");
    repo.write("b.txt", b"two\nthree\n");
    repo.commit("Not pushed");
    let tip = bench.id("HEAD");
    let listed = list(&bench.git(), &base, &tip, &head, &|| false).unwrap();
    let marks: Vec<(&str, bool)> = (listed.commits.iter())
        .map(|c| (c.subject.as_str(), c.unpushed))
        .collect();
    assert_eq!(marks, [("Pushed", false), ("Not pushed", true)]);
}

#[test]
fn a_stop_ends_the_listing() {
    let bench = Bench::new("commits-stopped");
    let base = bench.id("HEAD");
    bench.repo.write("b.txt", b"two\n");
    bench.repo.commit("b");
    let head = bench.id("HEAD");
    let stopped = list(&bench.git(), &base, &head, &head, &|| true);
    assert!(matches!(stopped, Err(super::ScopeError::Stopped)));
}

/// A line of the listing's output for a commit of id `n`, its subject `subject`.
fn line(n: usize, subject: &str) -> Vec<u8> {
    let id = format!("{n:040x}");
    let mut line = Vec::new();
    for field in [id.as_str(), &id, "", &id[..7], "Ann", "2026-11-20"] {
        line.extend_from_slice(field.as_bytes());
        line.push(0);
    }
    line.extend_from_slice(subject.as_bytes());
    line.push(b'\n');
    line
}

#[test]
fn the_newest_commits_past_the_limit_are_kept_and_the_list_says_more() {
    // git gives them newest first.
    let out: Vec<u8> = (0..MAX_COMMITS + 1)
        .flat_map(|n| line(n + 1, "x"))
        .collect();
    let read = read_listing(&out, false);
    assert_eq!(read.listed.commits.len(), MAX_COMMITS);
    assert!(read.listed.more);
    let ids: Vec<&str> = read.listed.commits.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids[0],
        format!("{MAX_COMMITS:040x}"),
        "the oldest kept, first"
    );
    assert_eq!(
        ids[MAX_COMMITS - 1],
        format!("{:040x}", 1),
        "the newest, last"
    );
}

#[test]
fn a_cut_listing_keeps_the_newest_and_says_more() {
    // An old commit's huge subject cuts the output: what is lost is older still.
    let mut out = line(1, "head");
    out.extend_from_slice(&line(2, &"x".repeat(100))[..60]);
    let read = read_listing(&out, true);
    let subjects: Vec<&str> = read
        .listed
        .commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    assert_eq!(subjects, ["head"]);
    assert!(read.listed.more);
    // A line not as asked is left out.
    let read = read_listing(b"not\0a\0line\n", false);
    assert!(read.listed.commits.is_empty());
}

#[test]
fn a_line_whose_short_id_or_date_is_not_one_is_left_out() {
    let good = line(1, "good");
    let mut bad_date = line(2, "bad");
    let at = bad_date
        .windows(10)
        .position(|w| w == b"2026-11-20")
        .unwrap();
    bad_date[at..at + 10].copy_from_slice(b"today \xc2\xb7 x");
    let mut out = bad_date;
    out.extend_from_slice(&good);
    let read = read_listing(&out, false);
    let subjects: Vec<&str> = read
        .listed
        .commits
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    assert_eq!(subjects, ["good"]);
    assert!(read.listed.more);
}

#[test]
fn words_are_escaped_on_one_line() {
    let read = read_listing(
        &line(1, "Fix \u{1b}[31m red \u{202e}gnp.exe \u{7}    padded  "),
        false,
    );
    let subject = &read.listed.commits[0].subject;
    assert!(
        subject.ends_with("\\u{7} padded"),
        "white space runs are one: {subject:?}"
    );
    assert!(!subject.chars().any(char::is_control), "{subject:?}");
    assert!(subject.contains("\\u{1b}") && subject.contains("\\u{202e}"));
}

#[test]
fn a_commits_message_is_read_with_its_author_and_date() {
    let bench = Bench::new("commits-message");
    let repo = &bench.repo;
    repo.write("b.txt", b"two\n");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "-m",
        "Add b\n\nIt holds two.\n@@ -1 +1 @@ is no hunk",
    ]);
    let id = bench.id("HEAD");
    let read = message(&bench.git(), &id).unwrap();
    assert_eq!(read.short.len(), 7);
    assert_eq!(read.author, "t");
    assert_eq!(
        read.lines,
        ["Add b", "", "It holds two.", "@@ -1 +1 @@ is no hunk"]
    );
    assert_eq!((read.left_out, read.cut), (0, false));
}

#[test]
fn a_long_message_is_cut_saying_so_and_its_lines_escaped() {
    let mut out = b"abc1234\0Ann\x1b]0;x\x07\x002026-11-20\0".to_vec();
    for n in 0..MAX_MESSAGE_LINES + 5 {
        out.extend_from_slice(format!("line {n}\r\n").as_bytes());
    }
    let read = read_message(&out, true).unwrap();
    assert_eq!(read.lines.len(), MAX_MESSAGE_LINES);
    assert_eq!(read.left_out, 5);
    assert!(read.cut);
    assert!(!read.author.chars().any(char::is_control));
    assert!(
        read.lines
            .iter()
            .all(|line| !line.chars().any(char::is_control))
    );
    assert!(read_message(b"no fields", false).is_none());
}

/// A pull request's branch `topic` off `main`, two commits on it; its base and head.
fn topic(bench: &Bench) -> (Id, Id) {
    let repo = &bench.repo;
    let base = bench.id("HEAD");
    repo.git(&["switch", "-q", "-c", "topic"]);
    repo.write("b.txt", b"two\n");
    repo.commit("Add b");
    repo.write("c.txt", b"three\n");
    repo.commit("Add c");
    (base, bench.id("HEAD"))
}

#[test]
fn the_commits_after_the_head_then_are_new_when_it_is_an_ancestor() {
    let bench = Bench::new("since-ancestor");
    let (base, then) = topic(&bench);
    bench.repo.write("d.txt", b"four\n");
    bench.repo.commit("Add d");
    let now = bench.id("HEAD");
    let ask = SinceAsk {
        from: super::Checkpoint {
            base: base.clone(),
            head: then,
        },
        base,
        tip: now.clone(),
    };
    let found = since(&bench.git(), ask, &|| false).unwrap();
    assert!(!found.rebased && !found.gone);
    assert_eq!(
        found.new.into_iter().collect::<Vec<_>>(),
        [now.as_str().to_owned()]
    );
}

#[test]
fn after_a_rebase_the_commits_whose_patch_the_review_then_had_are_not_new() {
    let bench = Bench::new("since-rebased");
    let repo = &bench.repo;
    let (base, then) = topic(&bench);
    // main moves on; topic is rebased onto it, and one more commit comes.
    repo.git(&["switch", "-q", "main"]);
    repo.write("m.txt", b"main\n");
    repo.commit("Main moves");
    let new_base = bench.id("HEAD");
    repo.git(&["switch", "-q", "topic"]);
    repo.git(&["rebase", "-q", "main"]);
    repo.write("d.txt", b"four\n");
    repo.commit("Add d");
    let now = bench.id("HEAD");
    let ask = SinceAsk {
        from: super::Checkpoint { base, head: then },
        base: new_base,
        tip: now.clone(),
    };
    let found = since(&bench.git(), ask, &|| false).unwrap();
    assert!(found.rebased && !found.gone && !found.cut);
    assert_eq!(
        found.new.into_iter().collect::<Vec<_>>(),
        [now.as_str().to_owned()]
    );
    assert_eq!(
        found.patched.len(),
        3,
        "Add b, Add c and Add d each have a patch"
    );
}

#[test]
fn a_head_then_that_the_clone_no_longer_holds_is_gone() {
    let bench = Bench::new("since-gone");
    let (base, now) = topic(&bench);
    let ask = SinceAsk {
        from: super::Checkpoint {
            base: base.clone(),
            head: Id::parse(&[b'e'; 40]).unwrap(),
        },
        base,
        tip: now,
    };
    let found = since(&bench.git(), ask, &|| false).unwrap();
    assert!(found.gone && found.new.is_empty());
}

#[test]
fn a_commits_patch_id_stays_through_a_rebase() {
    let bench = Bench::new("patch-ids");
    let repo = &bench.repo;
    let (base, head) = topic(&bench);
    let (before, cut) = patch_ids(&bench.git(), &base, &head, &|| false).unwrap();
    assert!(!cut);
    assert_eq!(before.len(), 2);
    repo.git(&["switch", "-q", "main"]);
    repo.write("m.txt", b"main\n");
    repo.commit("Main moves");
    let new_base = bench.id("HEAD");
    repo.git(&["switch", "-q", "topic"]);
    repo.git(&["rebase", "-q", "main"]);
    let (after, _) = patch_ids(&bench.git(), &new_base, &bench.id("HEAD"), &|| false).unwrap();
    let mut a: Vec<&String> = before.values().collect();
    let mut b: Vec<&String> = after.values().collect();
    a.sort();
    b.sort();
    assert_eq!(a, b, "the same patches, new commits");
    assert!(before.keys().all(|commit| !after.contains_key(commit)));
}

/// Rebases `topic` onto a `main` that moved, then changes `b.txt`'s white space alone in a new
/// commit; the base then and now, the head then and now.
fn rebased_with_a_space(bench: &Bench) -> (Id, Id, Id, Id) {
    let repo = &bench.repo;
    let (base, then) = topic(bench);
    repo.git(&["switch", "-q", "main"]);
    repo.write("m.txt", b"main\n");
    repo.commit("Main moves");
    let new_base = bench.id("HEAD");
    repo.git(&["switch", "-q", "topic"]);
    repo.git(&["rebase", "-q", "main"]);
    repo.write("b.txt", b"two \n");
    repo.commit("Only a space");
    (base, new_base, then, bench.id("HEAD"))
}

#[test]
fn a_change_of_white_space_alone_after_a_rebase_is_new() {
    let bench = Bench::new("since-space");
    let (base, new_base, then, now) = rebased_with_a_space(&bench);
    let ask = SinceAsk {
        from: super::Checkpoint { base, head: then },
        base: new_base,
        tip: now.clone(),
    };
    let found = since(&bench.git(), ask, &|| false).unwrap();
    assert!(found.rebased);
    assert!(found.new.contains(now.as_str()), "{:?}", found.new);
}

#[cfg(unix)]
#[test]
fn reading_patches_runs_no_diff_program_and_takes_no_message_line() {
    use std::os::unix::fs::PermissionsExt;
    let bench = Bench::new("patch-ids-guarded");
    let repo = &bench.repo;
    let marker = repo.dir.join("ran");
    let script = repo.dir.join("driver");
    fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\ncat \"$1\"\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let driver = script.to_str().unwrap();
    repo.git(&["config", "diff.external", driver]);
    repo.git(&["config", "diff.x.textconv", driver]);
    repo.git(&["config", "diff.x.command", driver]);
    // A message line that reads as a commit's id, under a format that prints the body bare.
    repo.git(&["config", "format.pretty", "format:%H%n%B"]);
    repo.write(".gitattributes", b"*.txt diff=x\n");
    repo.commit("Attributes");
    let base = bench.id("HEAD");
    repo.write("b.txt", b"two\n");
    repo.git(&["add", "-A"]);
    let forged = format!("Add b\n\n{}", "f".repeat(40));
    repo.git(&["commit", "-q", "-m", &forged]);
    let head = bench.id("HEAD");
    let (ids, cut) = super::patch_ids(&bench.git(), &base, &head, &|| false).unwrap();
    assert!(!marker.exists(), "a diff program ran");
    assert!(!cut);
    assert_eq!(ids.keys().collect::<Vec<_>>(), [&head.as_str().to_owned()]);
}
