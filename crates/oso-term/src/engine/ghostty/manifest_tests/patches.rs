//! `build.rs`'s check of oso's patches to the library (`build/patch_check.rs`, ADR 0120): the
//! checked-in patch passes and changes exactly the files the lock hashes, and each kind of
//! patch the check refuses is refused.
//!
//! Must not depend on: anything but `patch_check`.

use super::patch_check::changed_files;

const HUNK: &str = "@@ -1,2 +1,2 @@\n context\n-old\n+new\n";

fn one_file(old: &str, new: &str) -> String {
    format!("--- {old}\n+++ {new}\n{HUNK}")
}

#[test]
fn the_checked_in_patch_changes_what_the_lock_hashes() {
    let patch = include_str!("../../../../patches/0001-row-copies.patch");
    let lock = include_str!("../../../../ghostty-vt.lock");
    let mut changed = changed_files(patch).expect("the patch is a plain diff");
    changed.sort();
    let mut hashed: Vec<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("patched "))
        .filter_map(|rest| rest.split_whitespace().next())
        .collect();
    hashed.sort_unstable();
    assert_eq!(changed, hashed);
}

#[test]
fn a_plain_change_with_free_text_before_it_passes() {
    let patch = format!(
        "What it does.\n\ndiff -ruN a/x.zig b/x.zig\n{}\\ No newline at end of file\n",
        one_file("a/x.zig\t2026-01-01", "b/x.zig")
    );
    assert_eq!(changed_files(&patch), Ok(vec!["x.zig".to_owned()]));
}

#[test]
fn two_names_for_one_file_are_refused() {
    assert!(changed_files(&one_file("a/a.txt", "b/sub/b.txt")).is_err());
}

#[test]
fn creating_or_deleting_a_file_is_refused() {
    assert!(changed_files(&one_file("/dev/null", "b/new.zig")).is_err());
    assert!(changed_files(&one_file("a/old.zig", "/dev/null")).is_err());
}

#[test]
fn a_path_leaving_the_source_is_refused() {
    for path in ["../x.zig", "/etc/x", "src//x.zig", "src/./x.zig", ""] {
        let patch = one_file(&format!("a/{path}"), &format!("b/{path}"));
        assert!(changed_files(&patch).is_err(), "{path:?}");
    }
    assert!(changed_files(&one_file("x.zig", "x.zig")).is_err());
}

#[test]
fn other_diff_formats_and_git_headers_are_refused() {
    let context =
        "*** a/x.zig\n--- b/x.zig\n***************\n*** 1 ****\n! old\n--- 1 ----\n! new\n";
    assert!(changed_files(context).is_err());
    assert!(changed_files(&format!("Index: x.zig\n{}", one_file("a/x.zig", "b/x.zig"))).is_err());
    assert!(changed_files("+++ b/x.zig\n@@ -1 +1 @@\n-a\n+b\n").is_err());
    for head in [
        "rename from x.zig",
        "copy from x.zig",
        "old mode 100644",
        "new mode 100755",
        "new file mode 100644",
        "deleted file mode 100644",
        "similarity index 90%",
        "GIT binary patch",
        "Binary files a/x and b/x differ",
    ] {
        let patch = format!("diff --git a/x.zig b/x.zig\n{head}\n");
        assert!(changed_files(&patch).is_err(), "{head}");
    }
}

#[test]
fn a_hunk_of_the_wrong_length_or_text_after_it_is_refused() {
    assert!(changed_files("--- a/x\n+++ b/x\n@@ -1,3 +1,3 @@\n a\n").is_err());
    assert!(changed_files("--- a/x\n+++ b/x\n@@ -1 +1 @@\n a\n b\n").is_err());
    assert!(changed_files("--- a/x\n+++ b/x\n").is_err());
    assert!(changed_files("just text\n").is_err());
}
