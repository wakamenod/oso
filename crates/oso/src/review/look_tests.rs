//! A review's looks (ADR 0122, step 4): nothing said while nothing moved, a reviewed file's
//! edit found at once, and a new file found by the listing on the second look.
//!
//! Must not depend on: the app or the person's git configuration.

use super::tests::Repo;
use super::work::Reader;
use super::{Report, Request, Scope};

/// Whether a look says the review is stale.
fn looks_stale(reader: &mut Reader) -> bool {
    let mut stale = false;
    reader.answer(Request::Look, &|| false, &mut || None, &mut |report| {
        stale |= matches!(report, Report::Stale);
        true
    });
    stale
}

#[test]
fn a_look_finds_a_file_edited_at_once_and_a_new_one_by_the_listing() {
    let repo = Repo::new("look");
    repo.write("a.rs", b"one\ntwo\n");
    repo.commit("base");
    repo.write("a.rs", b"one\n2\n");
    let mut reader = Reader::new(&repo.root, Some(&repo.dir.join("state")));
    // Nothing opened: nothing to look at.
    assert!(!looks_stale(&mut reader));
    reader.answer(
        Request::Open(Scope::Uncommitted),
        &|| false,
        &mut || None,
        &mut |_| true,
    );
    assert!(!looks_stale(&mut reader));
    assert!(!looks_stale(&mut reader), "the listing is the same too");
    // An edit of a reviewed file: its stamp moved.
    repo.write("a.rs", b"one\n2\nthree\n");
    assert!(looks_stale(&mut reader));
    reader.answer(
        Request::Refresh { first: Vec::new() },
        &|| false,
        &mut || None,
        &mut |_| true,
    );
    // A new file is no reviewed file: the listing on the second look finds it.
    repo.write("b.rs", b"fn b() {}\n");
    let first = looks_stale(&mut reader);
    let second = looks_stale(&mut reader);
    assert!(first || second, "one of two looks compares the listing");
}
