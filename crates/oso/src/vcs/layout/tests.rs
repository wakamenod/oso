//! The status's layout: its sections and rows as a text grid, folding, opening a file, the
//! limits, finding a place again in another text, and the commit sections (step 2).
//!
//! Must not depend on: `app`, `dispatch` or winit; these are pure functions' tests.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::syntax::Highlight;

use super::{Head, Hunks, Laid, Row, SECRET, Section, View, lay_out, sections};
use crate::vcs::history::{Commit, Log};
use crate::vcs::patch;
use crate::vcs::status::{self, Branch};

const OID: &str = "1111111111111111111111111111111111111111";

const PATCH: &[u8] = b"diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,3 @@ fn main() {
 one
-two
+deux
 three
@@ -10,2 +10,2 @@
 ten
-eleven
+onze
";

fn record(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(part);
        out.push(0);
    }
    out
}

fn status() -> status::Status {
    status::parse(&record(&[
        format!("# branch.oid {OID}").as_bytes(),
        b"# branch.head main",
        b"# branch.upstream origin/main",
        b"# branch.ab +2 -0",
        b"1 MM N... 100644 100644 100644 aaaa bbbb src/a.rs",
        b"1 .M N... 100644 100644 100644 aaaa bbbb .env",
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 new.rs",
        b"old.rs",
        b"? notes.txt",
        b"u UU N... 100644 100644 100644 100644 a b c both.rs",
    ]))
}

fn head() -> Head {
    Head {
        branch: Branch {
            oid: Some(OID.into()),
            head: Some(b"main".to_vec()),
            upstream: Some(b"origin/main".to_vec()),
            ahead_behind: Some((2, 0)),
        },
        commit: Some(("1111111".into(), b"Add the first file".to_vec())),
        ..Head::default()
    }
}

fn counts() -> HashMap<Vec<u8>, status::Counts> {
    HashMap::from([
        (b"src/a.rs".to_vec(), Some((2, 2))),
        (b".env".to_vec(), Some((1, 0))),
        (b"new.rs".to_vec(), Some((0, 0))),
    ])
}

fn laid(view: &View) -> Laid {
    let secret = |path: &[u8]| path == b".env";
    let sections = sections(&status(), &counts(), &counts(), &secret);
    let hunks = HashMap::from([(
        (Section::Unstaged, b"src/a.rs".to_vec()),
        Hunks::Patch(Arc::new(patch::parse(PATCH, false))),
    )]);
    lay_out(&head(), &[], sections, &hunks, view)
}

/// Each line with what its row is, and under it a mark for each character's face.
fn grid(laid: &Laid) -> String {
    let text = laid.text.text().to_string();
    let mut out = String::new();
    let mut at = 0;
    for (number, line) in text.split('\n').enumerate() {
        let kind = match laid.built.rows.get(number) {
            Some(Row::Other) => "other",
            Some(Row::Heading(_)) => "heading",
            Some(Row::File { .. }) => "file",
            Some(Row::Detail { .. }) => "detail",
            Some(Row::Hunk { .. }) => "hunk",
            Some(Row::Line { .. }) => "line",
            Some(Row::More(_)) => "more",
            Some(Row::Commit { .. }) => "commit",
            None => "end",
        };
        let marks: String = line
            .char_indices()
            .map(|(offset, _)| match laid.highlights.at(at + offset) {
                None => ' ',
                Some(Highlight::GitSectionHeading) => 'H',
                Some(Highlight::GitBranchLocal) => 'b',
                Some(Highlight::GitBranchRemote) => 'r',
                Some(Highlight::GitHash) => 'h',
                Some(Highlight::GitDetail) => 'c',
                Some(Highlight::DiffHunkHeader) => '@',
                Some(Highlight::DiffAdded) => '+',
                Some(Highlight::DiffRemoved) => '-',
                Some(_) => '?',
            })
            .collect();
        out.push_str(&format!("{kind:<8}|{line}\n"));
        if !marks.trim().is_empty() {
            out.push_str(&format!("{:<8}|{}\n", "", marks.trim_end()));
        }
        at += line.len() + 1;
    }
    out
}

#[test]
fn grid_tests_status_sections_in_magits_order() {
    let laid = laid(&View::default());
    insta::assert_snapshot!("git_status_sections", grid(&laid));
    assert_eq!(laid.text.text().len(), laid.highlights.len());
    let headings: Vec<usize> = laid
        .outline
        .iter()
        .filter(|h| h.depth == 0)
        .map(|h| h.line)
        .collect();
    assert_eq!(headings.len(), 4);
    for line in headings {
        assert!(matches!(laid.built.rows[line], Row::Heading(_)));
    }
}

#[test]
fn grid_tests_status_an_open_file_shows_its_hunks() {
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"src/a.rs".to_vec());
    view.open_file(Section::Unstaged, b".env".to_vec());
    view.folded.insert(Section::Untracked);
    let laid = laid(&view);
    insta::assert_snapshot!("git_status_open_file", grid(&laid));
    let hunks = laid.outline.iter().filter(|h| h.depth == 2).count();
    assert_eq!(hunks, 2);
}

#[test]
fn a_file_that_may_hold_secrets_shows_no_hunks() {
    let mut view = View::default();
    view.open_file(Section::Unstaged, b".env".to_vec());
    let secret = |path: &[u8]| path == b".env";
    let sections = sections(&status(), &counts(), &counts(), &secret);
    let env = sections
        .iter()
        .find(|(of, _)| *of == Section::Unstaged)
        .and_then(|(_, files)| files.iter().find(|file| file.path == b".env"))
        .unwrap();
    assert_eq!(env.whole_only, Some(SECRET));
    assert!(!env.reads);
    // Even a patch handed in is not shown.
    let hunks = HashMap::from([(
        (Section::Unstaged, b".env".to_vec()),
        Hunks::Patch(Arc::new(patch::parse(PATCH, false))),
    )]);
    let laid = lay_out(&head(), &[], sections, &hunks, &view);
    let text = laid.text.text().to_string();
    assert!(!text.contains("deux"));
    assert!(text.contains(SECRET));
}

#[test]
fn the_head_says_detached_and_no_commits_yet() {
    let head = Head {
        branch: Branch::default(),
        commit: None,
        ..Head::default()
    };
    let laid = lay_out(&head, &[], Vec::new(), &HashMap::new(), &View::default());
    assert_eq!(
        laid.text.text().to_string(),
        "Head:     (detached) (no commits yet)\n"
    );
}

#[test]
fn a_section_past_its_limit_counts_the_rest() {
    let mut records = Vec::new();
    for index in 0..super::MAX_FILES + 3 {
        records.push(format!("? f{index}"));
    }
    let parts: Vec<&[u8]> = records.iter().map(|r| r.as_bytes()).collect();
    let status = status::parse(&record(&parts));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let laid = lay_out(&head(), &[], sections, &HashMap::new(), &View::default());
    let text = laid.text.text().to_string();
    assert!(text.contains("… and 3 more files not shown"));
    assert!(matches!(
        laid.built
            .rows
            .iter()
            .rev()
            .find(|row| !matches!(row, Row::Other)),
        Some(Row::More(Section::Untracked))
    ));
}

#[test]
fn a_place_is_found_again_by_what_it_is_on() {
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"src/a.rs".to_vec());
    let open = laid(&view);
    let rows = &open.built.rows;
    let line_of = |want: Row| rows.iter().position(|row| *row == want).unwrap();
    // The second hunk's second line.
    let at = line_of(Row::Line {
        section: Section::Unstaged,
        file: 0,
        hunk: 1,
        line: 1,
    });
    let place = open.built.place(at);
    let same = laid(&view);
    assert_eq!(same.built.line_of(&place), at);
    // With the file closed, its row.
    let closed = laid(&View::default());
    let file_row = closed
        .built
        .rows
        .iter()
        .position(|row| {
            *row == Row::File {
                section: Section::Unstaged,
                file: 0,
            }
        })
        .unwrap();
    assert_eq!(closed.built.line_of(&place), file_row);
    // With the section folded, its heading.
    let mut folded = View::default();
    folded.folded.insert(Section::Unstaged);
    let folded = laid(&folded);
    let heading = folded
        .built
        .rows
        .iter()
        .position(|row| *row == Row::Heading(Section::Unstaged))
        .unwrap();
    assert_eq!(folded.built.line_of(&place), heading);
}

#[test]
fn a_place_whose_file_went_goes_to_the_file_now_at_its_index() {
    let open = laid(&View::default());
    let rows = &open.built.rows;
    let first = rows
        .iter()
        .position(|row| {
            *row == Row::File {
                section: Section::Unstaged,
                file: 0,
            }
        })
        .unwrap();
    let place = open.built.place(first);
    // src/a.rs staged whole: .env is now the first unstaged file.
    let status = status::parse(&record(&[
        format!("# branch.oid {OID}").as_bytes(),
        b"1 M. N... 100644 100644 100644 aaaa bbbb src/a.rs",
        b"1 .M N... 100644 100644 100644 aaaa bbbb .env",
    ]));
    let sections = sections(&status, &counts(), &counts(), &|_| false);
    let after = lay_out(&head(), &[], sections, &HashMap::new(), &View::default());
    let line = after.built.line_of(&place);
    assert_eq!(
        after.built.rows[line],
        Row::File {
            section: Section::Unstaged,
            file: 0
        }
    );
    assert_eq!(after.built.files(Section::Unstaged)[0].path, b".env");
}

#[test]
fn a_line_break_in_a_name_or_a_hunk_never_makes_two_rows() {
    let status = status::parse(&record(&[
        format!("# branch.oid {OID}").as_bytes(),
        b"# branch.head odd\nbranch",
        b"1 .M N... 100644 100644 100644 aaaa bbbb a\nb.rs",
    ]));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let laid = lay_out(&head(), &[], sections, &HashMap::new(), &View::default());
    let lines = laid.text.text().to_string().split('\n').count();
    assert_eq!(lines, laid.built.rows.len() + 1);
}

#[test]
fn tab_folds_a_section_opens_a_file_and_closes_it_from_a_hunk() {
    let mut view = View::default();
    let closed = laid(&view);
    let rows = &closed.built.rows;
    let heading = rows
        .iter()
        .position(|row| *row == Row::Heading(Section::Untracked))
        .unwrap();
    assert!(view.toggle(&closed.built, heading));
    assert!(view.folded.contains(&Section::Untracked));
    assert!(view.toggle(&closed.built, heading));
    assert!(view.folded.is_empty());
    let file = rows
        .iter()
        .position(|row| {
            *row == Row::File {
                section: Section::Unstaged,
                file: 0,
            }
        })
        .unwrap();
    assert!(view.toggle(&closed.built, file));
    assert!(view.is_open(Section::Unstaged, b"src/a.rs"));
    let open = laid(&view);
    let hunk_line = open
        .built
        .rows
        .iter()
        .position(|row| matches!(row, Row::Line { .. }))
        .unwrap();
    assert!(view.toggle(&open.built, hunk_line));
    assert!(!view.is_open(Section::Unstaged, b"src/a.rs"));
    // The head's lines are about nothing.
    assert!(!view.toggle(&open.built, 0));
}

#[test]
fn ret_visits_a_files_row_or_the_line_a_hunks_line_shows() {
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"src/a.rs".to_vec());
    let open = laid(&view);
    let at = |want: Row| open.built.rows.iter().position(|row| *row == want).unwrap();
    let file = at(Row::File {
        section: Section::Unstaged,
        file: 0,
    });
    let visit = open.built.visit(file, 0).unwrap();
    assert_eq!((visit.path, visit.at), (b"src/a.rs".as_slice(), None));
    // `+deux`, the new side's line 2, at the column after its `+`.
    let added = at(Row::Line {
        section: Section::Unstaged,
        file: 0,
        hunk: 0,
        line: 2,
    });
    assert_eq!(open.built.visit(added, 3).unwrap().at, Some((2, 2)));
    let header = at(Row::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 1,
    });
    assert_eq!(open.built.visit(header, 0).unwrap().at, Some((10, 0)));
    assert_eq!(open.built.visit(0, 0), Err("No file on this line"));
}

#[test]
fn files_no_longer_listed_are_forgotten_as_open() {
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"src/a.rs".to_vec());
    view.open_file(Section::Unstaged, b"gone.rs".to_vec());
    let laid = laid(&view);
    view.forget_gone(&laid.built);
    assert!(view.is_open(Section::Unstaged, b"src/a.rs"));
    assert!(!view.is_open(Section::Unstaged, b"gone.rs"));
}

#[test]
fn a_changed_word_of_a_paired_line_is_refined() {
    let patch = b"diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,1 +1,1 @@
-let total = sum(a, b);
+let total = add(a, c);
";
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"src/a.rs".to_vec());
    let sections = sections(&status(), &counts(), &counts(), &|_| false);
    let hunks = HashMap::from([(
        (Section::Unstaged, b"src/a.rs".to_vec()),
        Hunks::Patch(Arc::new({
            let mut patch = patch::parse(patch, false);
            patch.refine();
            patch
        })),
    )]);
    let laid = lay_out(&head(), &[], sections, &hunks, &view);
    let text = laid.text.text().to_string();
    let added = text.find("+let total = add").unwrap();
    let removed = text.find("-let total = sum").unwrap();
    let at = |offset: usize| laid.highlights.at(offset);
    // `+` and `let` keep the line's face; `add` and `c` are refined.
    assert_eq!(at(added), Some(Highlight::DiffAdded));
    assert_eq!(at(added + 13), Some(Highlight::DiffRefineAdded));
    assert_eq!(at(added + 17), Some(Highlight::DiffAdded));
    assert_eq!(at(added + 20), Some(Highlight::DiffRefineAdded));
    assert_eq!(at(removed + 13), Some(Highlight::DiffRefineRemoved));
    assert_eq!(laid.highlights.len(), text.len());
}

/// A commit of the commit sections, its id from `n`.
fn commit(n: u8, subject: &[u8]) -> Commit {
    let id = format!("{n:x}").repeat(40);
    Commit {
        short: id[..7].to_owned(),
        id,
        subject: subject.to_vec(),
    }
}

/// The head with two unpushed commits, one unpulled, the recent ones and a push target.
fn head_with_logs(unpushed: Vec<Commit>) -> Head {
    let mut head = head();
    head.push = Some(b"fork/main".to_vec());
    head.logs = vec![
        (
            Section::Unpushed,
            Log {
                commits: unpushed.clone(),
                more: false,
            },
        ),
        (
            Section::Unpulled,
            Log {
                commits: vec![commit(3, b"theirs")],
                more: true,
            },
        ),
        (
            Section::Recent,
            Log {
                commits: [unpushed, vec![commit(4, b"first")]].concat(),
                more: false,
            },
        ),
    ];
    head
}

#[test]
fn grid_tests_status_commit_sections_follow_the_files() {
    let unpushed = vec![
        commit(1, b"second"),
        commit(2, "a\u{202e}rtl\nsubject".as_bytes()),
    ];
    let head = head_with_logs(unpushed);
    let laid = lay_out(&head, &[], Vec::new(), &HashMap::new(), &View::default());
    insta::assert_snapshot!("git_status_commits", grid(&laid));
    let text = laid.text.text().to_string();
    assert!(text.contains("Push:     fork/main\n"), "{text}");
    assert!(text.contains("Unpushed to origin/main (2)\n"), "{text}");
    assert!(text.contains("Unpulled from origin/main (1+)\n"), "{text}");
    assert!(text.contains("Recent commits (3)\n"), "{text}");
    // A subject never makes a row of its own: each line is one row.
    assert_eq!(text.lines().count(), laid.built.rows.len());
    let line = text
        .lines()
        .position(|line| line.starts_with("2222222"))
        .unwrap();
    let (section, found) = laid.built.commit_at(line).unwrap();
    assert_eq!(section, Section::Unpushed);
    assert_eq!(found.id, "2".repeat(40));
    assert!(laid.built.commit_at(0).is_none());
    // `s` and `k` find nothing to act on in a commit section.
    assert!(laid.built.visit(line, 0).is_err());
}

#[test]
fn a_commit_is_found_again_by_its_id_and_its_section_folds() {
    let before = head_with_logs(vec![commit(1, b"second")]);
    let laid = lay_out(&before, &[], Vec::new(), &HashMap::new(), &View::default());
    let text = laid.text.text().to_string();
    let line = text
        .lines()
        .position(|line| line.starts_with("1111111"))
        .unwrap();
    let place = laid.built.place(line);
    // A new commit above it moves it down a row.
    let after = head_with_logs(vec![commit(5, b"new"), commit(1, b"second")]);
    let again = lay_out(&after, &[], Vec::new(), &HashMap::new(), &View::default());
    let moved = again.built.line_of(&place);
    assert_eq!(again.built.commit_at(moved).unwrap().1.id, "1".repeat(40));
    let mut view = View::default();
    let heading = (again.built.rows.iter())
        .position(|row| *row == Row::Heading(Section::Unpushed))
        .unwrap();
    assert!(view.toggle(&again.built, heading));
    let folded = lay_out(&after, &[], Vec::new(), &HashMap::new(), &view);
    let text = folded.text.text().to_string();
    assert!(text.contains("Unpushed to origin/main (2)…"), "{text}");
    assert!(!text.contains("…\n5555555"), "{text}");
    // TAB on a commit does nothing.
    let row = (again.built.rows.iter())
        .position(|row| matches!(row, Row::Commit { .. }))
        .unwrap();
    assert!(!View::default().toggle(&again.built, row));
}
