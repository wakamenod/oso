//! What the cursors' lines name, and the git commands staging and unstaging them runs.
//!
//! Must not depend on: `app`, `dispatch` or winit; these are pure functions' tests.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::editor::GitLines;

use super::{Op, Target, ops, targets};
use crate::vcs::layout::{Built, Head, Hunks, Row, Section, View, lay_out, sections};
use crate::vcs::{patch, status};

const PATCH: &[u8] = b"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-one
+un
 two
@@ -9,2 +9,2 @@
 nine
-ten
+dix
";

fn record(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(part);
        out.push(0);
    }
    out
}

/// A status of `a.rs` changed in both, `b.rs` staged, `c.txt` untracked, with `a.rs`'s unstaged
/// hunks open.
fn built() -> Arc<Built> {
    let status = status::parse(&record(&[
        b"# branch.oid 1111111111111111111111111111111111111111",
        b"1 MM N... 100644 100644 100644 aaaa bbbb a.rs",
        b"1 M. N... 100644 100644 100644 aaaa bbbb b.rs",
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 d.rs",
        b"old-d.rs",
        b"? c.txt",
    ]));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"a.rs".to_vec());
    let hunks = HashMap::from([(
        (Section::Unstaged, b"a.rs".to_vec()),
        Hunks::Patch(Arc::new(patch::parse(PATCH, false))),
    )]);
    lay_out(&Head::default(), &[], sections, &hunks, &view).built
}

fn line_of(built: &Built, want: Row) -> usize {
    built.rows.iter().position(|row| *row == want).unwrap()
}

fn cursor(line: usize) -> GitLines {
    GitLines {
        first: line,
        last: line,
        region: false,
    }
}

fn region(first: usize, last: usize) -> GitLines {
    GitLines {
        first,
        last,
        region: true,
    }
}

const UNSTAGED_A: Row = Row::File {
    section: Section::Unstaged,
    file: 0,
};

fn hunk(hunk: usize) -> Row {
    Row::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk,
    }
}

fn line(hunk: usize, line: usize) -> Row {
    Row::Line {
        section: Section::Unstaged,
        file: 0,
        hunk,
        line,
    }
}

#[test]
fn a_cursor_means_what_its_line_is_on() {
    let built = built();
    let heading = line_of(&built, Row::Heading(Section::Untracked));
    let file = line_of(&built, UNSTAGED_A);
    let header = line_of(&built, hunk(1));
    let body = line_of(&built, line(0, 1));
    let found = targets(
        &built,
        &[cursor(heading), cursor(file), cursor(header), cursor(body)],
    )
    .unwrap();
    // The file holds its hunks, so they fold into it.
    assert_eq!(
        found,
        [
            Target::Section(Section::Untracked),
            Target::File {
                section: Section::Unstaged,
                file: 0
            },
        ]
    );
    let found = targets(&built, &[cursor(body)]).unwrap();
    assert_eq!(
        found,
        [Target::Hunk {
            section: Section::Unstaged,
            file: 0,
            hunk: 0
        }]
    );
    assert_eq!(targets(&built, &[cursor(0)]).unwrap(), []);
}

#[test]
fn a_region_means_the_headings_it_holds_and_the_lines_of_a_hunk_it_holds_without_its_header() {
    let built = built();
    let first = line_of(&built, hunk(0));
    let last = line_of(&built, line(1, 2));
    let found = targets(&built, &[region(first, last)]).unwrap();
    assert_eq!(found.len(), 2);
    let inside = (line_of(&built, line(0, 0)), line_of(&built, line(0, 1)));
    assert_eq!(
        targets(&built, &[region(inside.0, inside.1)]).unwrap(),
        [Target::Lines {
            section: Section::Unstaged,
            file: 0,
            hunk: 0,
            first: 0,
            last: 1
        }]
    );
}

#[test]
fn lines_stage_as_a_cut_patch_and_unstage_in_reverse() {
    let built = built();
    let lines = |first, last| Target::Lines {
        section: Section::Unstaged,
        file: 0,
        hunk: 1,
        first,
        last,
    };
    // `-ten` alone of the second hunk: `+dix` goes.
    let (found, said) = ops(&built, &[lines(1, 1)], true).unwrap();
    assert_eq!(said, "Staged a hunk's lines");
    let [Op::Apply { patch, reverse }] = found.as_slice() else {
        panic!("{found:?}");
    };
    assert!(!reverse);
    let patch = String::from_utf8(patch.clone()).unwrap();
    assert!(patch.ends_with("@@ -9,2 +9,1 @@\n nine\n-ten\n"), "{patch}");
    // Lines and their hunk whole: the hunk.
    let hunk = Target::Hunk {
        section: Section::Unstaged,
        file: 0,
        hunk: 1,
    };
    let (_, said) = ops(&built, &[lines(1, 1), hunk], true).unwrap();
    assert_eq!(said, "Staged a hunk");
    // Only context chosen: nothing to stage.
    assert!(ops(&built, &[lines(0, 0)], true).is_err());
}

#[test]
fn hunks_of_one_file_go_as_one_patch() {
    let built = built();
    let targets = [
        Target::Hunk {
            section: Section::Unstaged,
            file: 0,
            hunk: 1,
        },
        Target::Hunk {
            section: Section::Unstaged,
            file: 0,
            hunk: 0,
        },
    ];
    let (ops, said) = ops(&built, &targets, true).unwrap();
    assert_eq!(said, "Staged 2 hunks");
    let [Op::Apply { patch, reverse }] = ops.as_slice() else {
        panic!("one patch: {ops:?}");
    };
    assert!(!reverse);
    assert_eq!(patch.as_slice(), PATCH);
}

#[test]
fn files_go_as_paths_and_a_rename_unstages_both_its_paths() {
    let built = built();
    let (ops, said) = ops(&built, &[Target::Section(Section::Untracked)], true).unwrap();
    assert_eq!(ops, [Op::Add(vec![b"c.txt".to_vec()])]);
    assert_eq!(said, "Staged a file");
    let (ops, said) = ops_of(&built, Target::Section(Section::Staged), false);
    assert_eq!(
        ops,
        [Op::Unstage(vec![
            b"a.rs".to_vec(),
            b"b.rs".to_vec(),
            b"d.rs".to_vec(),
            b"old-d.rs".to_vec()
        ])]
    );
    assert_eq!(said, "Unstaged 3 files");
}

fn ops_of(built: &Built, target: Target, stage: bool) -> (Vec<Op>, String) {
    ops(built, &[target], stage).unwrap()
}

#[test]
fn what_is_in_the_other_section_is_refused() {
    let built = built();
    let staged = Target::Section(Section::Staged);
    assert_eq!(ops(&built, &[staged], true), Err("Already staged".into()));
    let unstaged = Target::File {
        section: Section::Unstaged,
        file: 0,
    };
    assert_eq!(ops(&built, &[unstaged], false), Err("Not staged".into()));
}

#[test]
fn a_hunk_of_a_file_that_stages_whole_is_refused() {
    let status = status::parse(&record(&[b"1 .M S.M. 160000 160000 160000 a b vendor"]));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let built = lay_out(
        &Head::default(),
        &[],
        sections,
        &HashMap::new(),
        &View::default(),
    )
    .built;
    let why = ops(
        &built,
        &[Target::Hunk {
            section: Section::Unstaged,
            file: 0,
            hunk: 0,
        }],
        true,
    )
    .unwrap_err();
    assert!(why.contains("a submodule"), "{why}");
}

#[test]
fn a_note_of_what_was_not_shown_names_nothing() {
    let status = status::parse(&record(&[
        b"# branch.oid 1111111111111111111111111111111111111111",
        b"1 .M N... 100644 100644 100644 aaaa bbbb a.rs",
    ]));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"a.rs".to_vec());
    let cut = &PATCH[..PATCH.len() - 2];
    let hunks = HashMap::from([(
        (Section::Unstaged, b"a.rs".to_vec()),
        Hunks::Patch(Arc::new(patch::parse(cut, true))),
    )]);
    let built = lay_out(&Head::default(), &[], sections, &hunks, &view).built;
    let note = (built.rows.iter())
        .position(|row| matches!(row, Row::Detail { whole: false, .. }))
        .expect("a note of what was cut");
    assert_eq!(targets(&built, &[cursor(note)]).unwrap(), []);
}

#[test]
fn a_region_over_ten_thousand_rows_is_told_apart_quickly() {
    let mut records = vec![b"# branch.oid 1111111111111111111111111111111111111111".to_vec()];
    for index in 0..10_000 {
        records.push(format!("1 .M N... 100644 100644 100644 a b f{index:05}").into_bytes());
    }
    let parts: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let status = status::parse(&record(&parts));
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &|_| false);
    let built = lay_out(
        &Head::default(),
        &[],
        sections,
        &HashMap::new(),
        &View::default(),
    )
    .built;
    let first = line_of(&built, UNSTAGED_A);
    let started = std::time::Instant::now();
    let found = targets(&built, &[region(first, built.rows.len() - 1)]).unwrap();
    // Linear: some milliseconds in a debug build, where the quadratic one took seconds.
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    assert_eq!(found.len(), 10_000);
}
