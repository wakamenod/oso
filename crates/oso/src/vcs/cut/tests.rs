//! Cutting a patch to the lines chosen: magit's rules each way, a missing line break at the end,
//! the headers counted again with the other side's starts following, and, over any hunk and any
//! choice, that the cut leaves git's side as it was and moves only the chosen changes.
//!
//! Must not depend on: `app`, `dispatch`, git or winit; these are pure functions' tests.

use proptest::prelude::*;

use super::{cut, file_patch};
use crate::vcs::patch::{Hunk, parse};

const PATCH: &[u8] = b"diff --git a/a b/a
--- a/a
+++ b/a
@@ -1,4 +1,4 @@ fn top() {
 one
-two
-three
+deux
+trois
 four
@@ -10,2 +10,3 @@
 ten
+dix
 eleven
";

fn text(lines: &[Vec<u8>]) -> String {
    lines
        .iter()
        .map(|line| String::from_utf8_lossy(line))
        .collect()
}

#[test]
fn forward_an_unchosen_removal_stays_and_an_unchosen_addition_goes() {
    let patch = parse(PATCH, false);
    // `-two` and `+deux` chosen.
    let lines = cut(&patch.hunks[0], &[1, 3], false).unwrap();
    assert_eq!(text(&lines), " one\n-two\n three\n+deux\n four\n");
    let lines = cut(&patch.hunks[0], &[1, 3], true).unwrap();
    assert_eq!(text(&lines), " one\n-two\n+deux\n trois\n four\n");
    // Only context chosen: nothing to do.
    assert_eq!(cut(&patch.hunks[0], &[0, 5], false), None);
}

#[test]
fn a_files_patch_counts_its_hunks_again_and_moves_the_other_sides_start() {
    let patch = parse(PATCH, false);
    let out = file_patch(&patch, &[(0, Some(vec![1])), (1, None)], false).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(out.starts_with("diff --git a/a b/a\n--- a/a\n+++ b/a\n"));
    // `-two` alone: the old side keeps 4 lines, the new one has 3, and the second hunk's new
    // start moves up by one.
    assert!(
        out.contains("@@ -1,4 +1,3 @@ fn top() {\n one\n-two\n three\n four\n"),
        "{out}"
    );
    assert!(
        out.contains("@@ -10,2 +9,3 @@\n ten\n+dix\n eleven\n"),
        "{out}"
    );
    // In reverse the new side keeps its start, and the old one follows.
    let out = file_patch(&patch, &[(0, Some(vec![3])), (1, None)], true).unwrap();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.contains("@@ -1,3 +1,4 @@ fn top() {\n one\n+deux\n trois\n four\n"),
        "{out}"
    );
    assert!(out.contains("@@ -9,2 +10,3 @@\n"), "{out}");
    // A hunk with no chosen change is left out; with none at all, nothing is.
    assert_eq!(file_patch(&patch, &[(0, Some(vec![0]))], false), None);
}

#[test]
fn a_missing_line_break_goes_with_its_line() {
    let patch = parse(
        b"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n one\n-two\n\\ No newline at end of file\n+deux\n\\ No newline at end of file\n",
        false,
    );
    let lines = cut(&patch.hunks[0], &[1, 2], false).unwrap();
    assert_eq!(text(&lines), " one\n-two\n\\ No newline at end of file\n");
    let lines = cut(&patch.hunks[0], &[3, 4], false).unwrap();
    assert_eq!(
        text(&lines),
        " one\n two\n\\ No newline at end of file\n+deux\n\\ No newline at end of file\n"
    );
}

/// The old and new sides of `lines`, as their text.
fn sides(lines: &[Vec<u8>]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let (mut old, mut new) = (Vec::new(), Vec::new());
    for line in lines {
        match line.first() {
            Some(b' ') => {
                old.push(line[1..].to_vec());
                new.push(line[1..].to_vec());
            }
            Some(b'-') => old.push(line[1..].to_vec()),
            Some(b'+') => new.push(line[1..].to_vec()),
            _ => {}
        }
    }
    (old, new)
}

fn hunk_of(kinds: &[u8]) -> Hunk {
    let lines: Vec<Vec<u8>> = (kinds.iter().enumerate())
        .map(|(n, kind)| {
            let mark = [b' ', b'-', b'+'][usize::from(*kind % 3)];
            let mut line = vec![mark];
            line.extend_from_slice(format!("line {n}\n").as_bytes());
            line
        })
        .collect();
    Hunk {
        header: b"@@ -1 +1 @@\n".to_vec(),
        new_start: 1,
        lines,
        whole: true,
    }
}

proptest! {
    #[test]
    fn a_cut_keeps_gits_side_and_moves_only_the_chosen_changes(
        kinds in proptest::collection::vec(0u8..3, 1..40),
        picked in proptest::collection::vec(any::<bool>(), 40),
        reverse in any::<bool>(),
    ) {
        let hunk = hunk_of(&kinds);
        let chosen: Vec<usize> = (0..hunk.lines.len()).filter(|&at| picked[at]).collect();
        let (old, new) = sides(&hunk.lines);
        let Some(lines) = cut(&hunk, &chosen, reverse) else {
            let any = chosen.iter().any(|&at| hunk.lines[at][0] != b' ');
            prop_assert!(!any);
            return Ok(());
        };
        let (cut_old, cut_new) = sides(&lines);
        let pick = |at: usize| chosen.contains(&at);
        if reverse {
            prop_assert_eq!(&cut_new, &new);
            // The old side is the new one with the chosen changes taken back.
            let want: Vec<Vec<u8>> = (hunk.lines.iter().enumerate())
                .filter(|(at, line)| match line[0] {
                    b' ' => true,
                    b'-' => pick(*at),
                    _ => !pick(*at),
                })
                .map(|(_, line)| line[1..].to_vec())
                .collect();
            prop_assert_eq!(cut_old, want);
        } else {
            prop_assert_eq!(&cut_old, &old);
            let want: Vec<Vec<u8>> = (hunk.lines.iter().enumerate())
                .filter(|(at, line)| match line[0] {
                    b' ' => true,
                    b'+' => pick(*at),
                    _ => !pick(*at),
                })
                .map(|(_, line)| line[1..].to_vec())
                .collect();
            prop_assert_eq!(cut_new, want);
        }
        // Everything chosen is the hunk itself.
        let all: Vec<usize> = (0..hunk.lines.len()).collect();
        if let Some(whole) = cut(&hunk, &all, reverse) {
            prop_assert_eq!(whole, hunk.lines.clone());
        }
    }
}
