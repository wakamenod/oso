//! What discarding runs and asks: each section's steps, hunks and lines as a reverse patch,
//! staged ones from the index too, copies but of a secret file, and the refusals.
//!
//! Must not depend on: `app`, `dispatch` or winit; these are pure functions' tests.

use std::collections::HashMap;
use std::sync::Arc;

use super::{Step, plan, question};
use crate::vcs::layout::{Built, Head, Hunks, Section, View, lay_out, sections};
use crate::vcs::stage::Target;
use crate::vcs::{patch, status};

const PATCH: &[u8] = b"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
@@ -1,2 +1,2 @@
-one
+un
 two
";

fn record(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(part);
        out.push(0);
    }
    out
}

/// `a.rs` changed in both, `b.rs` staged, `n.rs` staged new, `d.rs` a staged rename, `.env`
/// changed, `c.txt` untracked, `u.rs` unmerged; `a.rs`'s unstaged hunk open.
fn built() -> Arc<Built> {
    let status = status::parse(&record(&[
        b"# branch.oid 1111111111111111111111111111111111111111",
        b"1 MM N... 100644 100644 100644 aaaa bbbb a.rs",
        b"1 M. N... 100644 100644 100644 aaaa bbbb b.rs",
        b"1 A. N... 000000 100644 100644 0000 bbbb n.rs",
        b"2 R. N... 100644 100644 100644 aaaa aaaa R100 d.rs",
        b"old-d.rs",
        b"1 .M N... 100644 100644 100644 aaaa aaaa .env",
        b"u UU N... 100644 100644 100644 100644 aaaa bbbb cccc u.rs",
        b"? c.txt",
    ]));
    let secret = |path: &[u8]| path == b".env";
    let sections = sections(&status, &HashMap::new(), &HashMap::new(), &secret);
    let mut view = View::default();
    view.open_file(Section::Unstaged, b"a.rs".to_vec());
    let hunks = HashMap::from([(
        (Section::Unstaged, b"a.rs".to_vec()),
        Hunks::Patch(Arc::new(patch::parse(PATCH, false))),
    )]);
    lay_out(&Head::default(), &[], sections, &hunks, &view).built
}

fn index_of(built: &Built, section: Section, path: &[u8]) -> usize {
    (built.files(section).iter())
        .position(|file| file.path == path)
        .unwrap()
}

fn file(built: &Built, section: Section, path: &[u8]) -> Target {
    Target::File {
        section,
        file: index_of(built, section, path),
    }
}

#[test]
fn each_kind_of_file_has_its_own_step_and_is_copied_first() {
    let built = built();
    let targets = [
        file(&built, Section::Unstaged, b"a.rs"),
        file(&built, Section::Staged, b"b.rs"),
        file(&built, Section::Staged, b"n.rs"),
        file(&built, Section::Untracked, b"c.txt"),
    ];
    let plan = plan(&built, &targets).unwrap();
    assert_eq!(
        plan.steps,
        [
            Step::Restore(vec![b"a.rs".to_vec()]),
            Step::RestoreHead(vec![b"b.rs".to_vec()]),
            Step::Unadd(vec![b"n.rs".to_vec()]),
            Step::Delete(vec![b"c.txt".to_vec(), b"n.rs".to_vec()]),
        ]
    );
    let copied: Vec<&[u8]> = plan
        .copies
        .iter()
        .map(|(path, _)| path.as_slice())
        .collect();
    assert_eq!(copied, [&b"a.rs"[..], b"b.rs", b"c.txt", b"n.rs"]);
    assert_eq!(
        plan.said,
        "Discarded 4 files: M-x git-discarded brings it back"
    );
}

#[test]
fn a_hunk_and_lines_go_back_as_a_reverse_patch() {
    let built = built();
    let at = index_of(&built, Section::Unstaged, b"a.rs");
    let hunk = Target::Hunk {
        section: Section::Unstaged,
        file: at,
        hunk: 0,
    };
    let plan_of = |targets: &[Target]| plan(&built, targets).unwrap();
    let whole = plan_of(&[hunk]);
    let [Step::Reverse { patch, index }] = whole.steps.as_slice() else {
        panic!("{:?}", whole.steps);
    };
    assert!(!index);
    assert!(String::from_utf8_lossy(patch).ends_with("-one\n+un\n two\n"));
    assert_eq!(whole.copies, [(b"a.rs".to_vec(), false)]);
    // `+un` alone: in reverse, the unchosen `-one` goes.
    let lines = Target::Lines {
        section: Section::Unstaged,
        file: at,
        hunk: 0,
        first: 1,
        last: 1,
    };
    let part = plan_of(&[lines]);
    let [Step::Reverse { patch, .. }] = part.steps.as_slice() else {
        panic!("{:?}", part.steps);
    };
    assert!(String::from_utf8_lossy(patch).ends_with("@@ -1,1 +1,2 @@\n+un\n two\n"));
}

#[test]
fn a_secret_file_gets_no_copy_and_the_question_says_so() {
    let built = built();
    let targets = [file(&built, Section::Unstaged, b".env")];
    let plan = plan(&built, &targets).unwrap();
    assert_eq!(plan.copies, [(b".env".to_vec(), true)]);
    assert_eq!(plan.said, "Discarded a file");
    assert_eq!(
        question(&built, &targets),
        "Discard the file's changes in .env, with no copy of what may hold secrets?"
    );
}

#[test]
fn unmerged_paths_submodules_and_staged_renames_are_refused() {
    let built = built();
    let unmerged = file(&built, Section::Unmerged, b"u.rs");
    assert!(
        plan(&built, &[unmerged])
            .unwrap_err()
            .contains("resolve it first")
    );
    let renamed = file(&built, Section::Staged, b"d.rs");
    assert!(
        plan(&built, &[renamed])
            .unwrap_err()
            .contains("u unstages it first")
    );
}

#[test]
fn the_question_names_what_goes() {
    let built = built();
    assert_eq!(
        question(&built, &[file(&built, Section::Untracked, b"c.txt")]),
        "Delete untracked c.txt?"
    );
    let at = index_of(&built, Section::Unstaged, b"a.rs");
    let hunk = Target::Hunk {
        section: Section::Unstaged,
        file: at,
        hunk: 0,
    };
    assert_eq!(question(&built, &[hunk]), "Discard a hunk in a.rs?");
    let both = [hunk, file(&built, Section::Staged, b"b.rs")];
    assert_eq!(
        question(&built, &both),
        "Discard the file's changes and a hunk?"
    );
}
