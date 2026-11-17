//! Staging a review's hunks: the patch's context, counts and fragments, a last line with no
//! line break, quoted paths, a hunk that moved, and against a temporary repository, a hunk
//! staged by its context below one staged before, unstaged with the same patch, and refused
//! once in.
//!
//! Must not depend on: the app or the person's git configuration.

use std::sync::Arc;

use oso_core::review::{Base, Hunk};

use super::{Apply, CHANGED, Stage, patch};
use crate::git::Id;
use crate::review::tests::Repo;
use crate::review::work::Reader;
use crate::review::{Content, Reading, Report, Request, Scope};

fn hunk(old_start: u32, new_start: u32, removed: &[&str], added: &[&str]) -> Hunk {
    let lines = |lines: &[&str]| lines.iter().map(|line| line.as_bytes().into()).collect();
    Hunk {
        old_start,
        new_start,
        removed: lines(removed),
        added: lines(added),
    }
}

/// `lines` numbered lines, `1\n` to `{lines}\n`.
fn numbered(lines: usize) -> String {
    (1..=lines).map(|n| format!("{n}\n")).collect()
}

fn text(patch: &[u8]) -> String {
    String::from_utf8(patch.to_vec()).unwrap()
}

#[test]
fn a_hunk_goes_with_three_lines_around_it_and_its_counts() {
    let old = numbered(10);
    let patch = patch(
        b"a.rs",
        old.as_bytes(),
        &[hunk(5, 5, &["5\n"], &["five\n", "5b\n"])],
    );
    assert_eq!(
        text(&patch.unwrap()),
        "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
         @@ -2,7 +2,8 @@\n 2\n 3\n 4\n-5\n+five\n+5b\n 6\n 7\n 8\n"
    );
}

#[test]
fn hunks_whose_context_meets_share_a_fragment_and_others_shift() {
    let old = numbered(30);
    let hunks = [
        hunk(3, 3, &["3\n"], &[]),
        hunk(8, 7, &[], &["new\n"]),
        hunk(25, 25, &["25\n"], &["25!\n"]),
    ];
    let patch = text(&patch(b"a.rs", old.as_bytes(), &hunks).unwrap());
    let headers: Vec<&str> = patch
        .lines()
        .filter(|line| line.starts_with("@@"))
        .collect();
    // 3 and 8 meet within twice the context; 25 stands apart, one line on as 3 went and new came.
    assert_eq!(headers, ["@@ -1,10 +1,10 @@", "@@ -22,7 +22,7 @@"]);
    assert!(patch.contains(" 7\n+new\n 8\n"), "{patch}");
}

#[test]
fn a_last_line_with_no_line_break_says_so_and_paths_are_quoted() {
    let old = "one\ntwo";
    let patch = patch(
        b"a \"b\"\n.rs",
        old.as_bytes(),
        &[hunk(2, 2, &["two"], &["2"])],
    );
    let patch = text(&patch.unwrap());
    assert!(
        patch.starts_with("diff --git \"a/a \\\"b\\\"\\n.rs\" \"b/a \\\"b\\\"\\n.rs\"\n"),
        "{patch}"
    );
    assert!(
        patch.ends_with("-two\n\\ No newline at end of file\n+2\n\\ No newline at end of file\n"),
        "{patch}"
    );
}

#[test]
fn a_hunk_whose_lines_moved_builds_nothing() {
    let old = numbered(10);
    let moved = patch(
        b"a.rs",
        old.as_bytes(),
        &[hunk(4, 4, &["5\n"], &["five\n"])],
    );
    assert_eq!(moved, Err(CHANGED.to_owned()));
    let past = patch(b"a.rs", old.as_bytes(), &[hunk(11, 11, &["11\n"], &[])]);
    assert_eq!(past, Err(CHANGED.to_owned()));
}

/// Every report `request` gets.
fn answer(reader: &mut Reader, request: Request) -> Vec<Report> {
    let mut reports = Vec::new();
    reader.answer(request, &|| false, &mut || None, &mut |report| {
        reports.push(report);
        true
    });
    reports
}

/// The reading of `path` the uncommitted scope gives.
fn reading(reader: &mut Reader, path: &str) -> Arc<Reading> {
    let reports = answer(reader, Request::Open(Scope::Uncommitted));
    reports
        .into_iter()
        .find_map(|report| match report {
            Report::File { reading, .. } if &*reading.entry.path == path.as_bytes() => {
                Some(reading)
            }
            _ => None,
        })
        .unwrap()
}

/// Stages hunk `index` of `reading`, or the patch given, and says what it did.
fn stage(reader: &mut Reader, reading: &Reading, apply: Apply, reverse: bool) -> super::Staged {
    let stage = Stage {
        serial: 7,
        path: reading.entry.path.clone(),
        apply,
        reverse,
    };
    match answer(reader, Request::Stage(stage)).pop() {
        Some(Report::Staged { serial: 7, staged }) => staged,
        other => panic!("{other:?}"),
    }
}

fn hunks_of(reading: &Reading) -> (Id, Vec<Hunk>) {
    let Content::Text { shape, .. } = &reading.content else {
        panic!("no text");
    };
    let Base::Blob(id) = shape.base() else {
        panic!("no old blob");
    };
    (Id::parse(id).unwrap(), shape.hunks().to_vec())
}

#[test]
fn a_hunk_is_staged_by_its_context_below_one_staged_before_and_unstaged_back() {
    let repo = Repo::new("stage");
    repo.write("a.rs", numbered(30).as_bytes());
    repo.commit("base");
    let mut changed = numbered(30).replace("\n4\n", "\nfour\nfour more\n");
    changed = changed.replace("\n25\n", "\n25!\n");
    repo.write("a.rs", changed.as_bytes());
    let mut reader = Reader::new(&repo.root, Some(&repo.dir.join("state")));
    let reading = reading(&mut reader, "a.rs");
    let (old, hunks) = hunks_of(&reading);
    assert_eq!(hunks.len(), 2);
    // The top one first: the index's lines then run one on from the reading's old side.
    let top = Apply::Hunks {
        old: old.clone(),
        hunks: vec![hunks[0].clone()],
    };
    stage(&mut reader, &reading, top, false).unwrap();
    let bottom = Apply::Hunks {
        old,
        hunks: vec![hunks[1].clone()],
    };
    let patch = stage(&mut reader, &reading, bottom, false).unwrap();
    let staged = String::from_utf8(repo.git(&["diff", "--cached"])).unwrap();
    assert!(
        staged.contains("+four more\n") && staged.contains("+25!\n"),
        "{staged}"
    );
    // In twice is refused; back out with the same patch, then the top one stays alone.
    let again = stage(
        &mut reader,
        &reading,
        Apply::Patch(Arc::clone(&patch)),
        false,
    );
    assert!(again.unwrap_err().starts_with("git refused the hunk"));
    stage(&mut reader, &reading, Apply::Patch(patch), true).unwrap();
    let staged = String::from_utf8(repo.git(&["diff", "--cached"])).unwrap();
    assert!(
        staged.contains("+four more\n") && !staged.contains("25!"),
        "{staged}"
    );
}
