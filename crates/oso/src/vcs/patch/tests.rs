//! One file's patch, cut into hunks: what stages alone and what only whole, the new side's
//! lines, cut patches, and any bytes.
//!
//! Must not depend on: `app`, `dispatch`, git or winit; these are pure functions' tests.

use proptest::prelude::*;

use super::parse;
use crate::vcs::cut::file_patch;

const PATCH: &[u8] = b"diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,3 +1,4 @@ fn main() {
 one
-two
+deux
+trois
 three
@@ -10,2 +11,2 @@
 ten
-eleven
+onze
";

#[test]
fn a_patch_is_cut_into_its_header_and_hunks() {
    let patch = parse(PATCH, false);
    assert!(patch.header.starts_with(b"diff --git"));
    assert!(patch.header.ends_with(b"+++ b/src/a.rs\n"));
    assert_eq!(patch.hunks.len(), 2);
    assert_eq!(patch.hunks[0].header, b"@@ -1,3 +1,4 @@ fn main() {\n");
    assert_eq!(patch.hunks[0].lines.len(), 5);
    assert!(patch.hunks.iter().all(|hunk| hunk.whole));
    assert_eq!(patch.whole_only, None);
    assert!(!patch.cut);
}

#[test]
fn a_hunks_patch_is_the_header_and_that_hunk_as_git_wrote_them() {
    let patch = parse(PATCH, false);
    let second = file_patch(&patch, &[(1, None)], false).unwrap();
    let text = String::from_utf8(second).unwrap();
    assert!(text.starts_with("diff --git a/src/a.rs b/src/a.rs\n"));
    assert!(text.ends_with("@@ -10,2 +11,2 @@\n ten\n-eleven\n+onze\n"));
    assert!(!text.contains("deux"));
    assert_eq!(file_patch(&patch, &[(2, None)], false), None);
}

#[test]
fn a_line_of_a_hunk_finds_its_line_on_the_new_side() {
    let patch = parse(PATCH, false);
    let hunk = &patch.hunks[0];
    assert_eq!(hunk.new_line(0), 1); // " one"
    assert_eq!(hunk.new_line(1), 2); // "-two" was removed before line 2
    assert_eq!(hunk.new_line(2), 2); // "+deux"
    assert_eq!(hunk.new_line(4), 4); // " three"
    assert_eq!(patch.hunks[1].new_line(0), 11);
}

#[test]
fn a_hunk_without_a_count_or_with_no_final_newline_marker_reads() {
    let patch = parse(
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n",
        false,
    );
    assert_eq!(patch.hunks.len(), 1);
    assert!(patch.hunks[0].whole);
    assert!(file_patch(&patch, &[(0, None)], false).is_some());
}

#[test]
fn a_hunk_that_does_not_add_up_shows_but_does_not_stage() {
    let patch = parse(
        b"diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-a\n+b\n",
        false,
    );
    assert_eq!(patch.hunks.len(), 1);
    assert!(!patch.hunks[0].whole);
    assert_eq!(file_patch(&patch, &[(0, None)], false), None);
}

#[test]
fn a_cut_patch_drops_its_last_part_and_stages_whole() {
    let cut = &PATCH[..PATCH.len() - 4];
    let patch = parse(cut, true);
    assert_eq!(patch.hunks.len(), 1);
    assert!(patch.cut);
    assert!(patch.whole_only.is_some());
    assert_eq!(file_patch(&patch, &[(0, None)], false), None);
}

#[test]
fn new_deleted_renamed_binary_and_submodule_patches_stage_whole() {
    let cases: [&[u8]; 5] = [
        b"diff --git a/n b/n\nnew file mode 100644\n--- /dev/null\n+++ b/n\n@@ -0,0 +1 @@\n+x\n",
        b"diff --git a/d b/d\ndeleted file mode 100644\n--- a/d\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n",
        b"diff --git a/o b/n\nsimilarity index 90%\nrename from o\nrename to n\n",
        b"diff --git a/i b/i\nindex 1..2 100644\nBinary files a/i and b/i differ\n",
        b"diff --git a/s b/s\nindex 1..2 160000\n--- a/s\n+++ b/s\n@@ -1 +1 @@\n-Subproject commit 1\n+Subproject commit 2\n",
    ];
    for case in cases {
        let patch = parse(case, false);
        assert!(
            patch.whole_only.is_some(),
            "{}",
            String::from_utf8_lossy(case)
        );
        assert!(file_patch(&patch, &[(0, None)], false).is_none());
    }
}

#[test]
fn a_combined_diff_is_not_cut_into_hunks() {
    let patch = parse(
        b"diff --cc x\nindex 1,2..3\n--- a/x\n+++ b/x\n@@@ -1,1 -1,1 +1,5 @@@\n++<<<<<<< HEAD\n",
        false,
    );
    assert!(patch.hunks.is_empty());
    assert!(patch.whole_only.is_some());
}

proptest! {
    #[test]
    fn any_bytes_parse_without_growing(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let patch = parse(&bytes, false);
        let held: usize = patch.header.len()
            + patch.hunks.iter().map(|hunk| {
                hunk.header.len() + hunk.lines.iter().map(Vec::len).sum::<usize>()
            }).sum::<usize>();
        prop_assert!(held <= bytes.len());
        for index in 0..patch.hunks.len() {
            if let Some(one) = file_patch(&patch, &[(index, None)], false) {
                prop_assert!(one.len() <= bytes.len());
            }
        }
    }

    // Whatever hunks a patch holds, staging each one alone applies its own lines only.
    #[test]
    fn a_hunks_patch_holds_only_its_own_lines(
        bodies in proptest::collection::vec(
            proptest::collection::vec(proptest::sample::select(vec![" k\n", "-r\n", "+a\n"]), 1..6),
            1..4,
        )
    ) {
        let mut bytes = b"diff --git a/x b/x\n--- a/x\n+++ b/x\n".to_vec();
        let mut start = 1;
        for body in &bodies {
            let old = body.iter().filter(|line| !line.starts_with('+')).count();
            let new = body.iter().filter(|line| !line.starts_with('-')).count();
            bytes.extend_from_slice(format!("@@ -{start},{old} +{start},{new} @@\n").as_bytes());
            for line in body {
                bytes.extend_from_slice(line.as_bytes());
            }
            start += 100;
        }
        let patch = parse(&bytes, false);
        prop_assert_eq!(patch.hunks.len(), bodies.len());
        for (index, body) in bodies.iter().enumerate() {
            let one = file_patch(&patch, &[(index, None)], false).expect("a hunk that adds up stages");
            let tail = body.concat();
            prop_assert!(one.ends_with(tail.as_bytes()));
            let hunks = one.windows(3).filter(|w| *w == b"@@ ").count();
            prop_assert_eq!(hunks, 1);
        }
    }
}
