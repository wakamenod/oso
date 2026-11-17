//! A review's looks (ADR 0122, step 4): nothing said while nothing moved, a reviewed file's
//! edit found at once, and a new file found by the listing on the second look; and a pull
//! request's looks and refreshes of its marks reading the sides `gh` last gave (step 12b).
//!
//! Must not depend on: the app or the person's git configuration.

use super::scope::{New, Sides};
use super::tests::Repo;
use super::work::Reader;
use super::{Id, Report, Request, Scope};

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
        Request::Refresh {
            first: Vec::new(),
            person: true,
        },
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

#[test]
fn a_pull_request_s_looks_and_marked_refreshes_ask_gh_nothing() {
    let repo = Repo::new("look-pr");
    repo.write("a.rs", b"one\n");
    repo.commit("base");
    let base = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    repo.write("a.rs", b"one\ntwo\n");
    repo.commit("head");
    let head = String::from_utf8(repo.git(&["rev-parse", "HEAD"])).unwrap();
    let id = |text: &str| Id::parse(text.trim().as_bytes()).unwrap();
    let mut reader = Reader::new(&repo.root, Some(&repo.dir.join("state")));
    let sides = Sides {
        old: Some(id(&base)),
        new: New::Tree(id(&head)),
    };
    reader.pulled_for_tests(7, sides);
    // A refresh step 4's marks start lists the sides kept: no `gh`, no failure.
    let mut said = Vec::new();
    let refresh = Request::Refresh {
        first: Vec::new(),
        person: false,
    };
    reader.answer(refresh, &|| false, &mut || None, &mut |report| {
        said.push(format!("{report:?}"));
        true
    });
    assert!(
        said.iter().any(|said| said.starts_with("Listed")),
        "{said:?}"
    );
    assert!(
        !said.iter().any(|said| said.starts_with("Failed")),
        "{said:?}"
    );
    assert!(
        !said.iter().any(|said| said.starts_with("PrFound")),
        "{said:?}"
    );
    // Two looks, the second comparing the listing: nothing moved, nothing failed.
    for _ in 0..2 {
        let mut failed = false;
        reader.answer(Request::Look, &|| false, &mut || None, &mut |report| {
            failed |= matches!(report, Report::Failed { .. });
            true
        });
        assert!(!failed);
    }
    assert!(!looks_stale(&mut reader));
}
