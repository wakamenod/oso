//! The review left last (ADR 0184): open against saved, ties, and saved reviews from before.
//!
//! Must not depend on: the store's thread, winit or the renderer.

use std::path::PathBuf;

use oso_core::editor::ReviewId;

use super::{Candidate, Continue, last_left, listed_candidates};
use crate::review::Scope;
use crate::review::saved::Header;

fn header(root: &str, opened: u64, left: Option<u64>) -> Header {
    Header {
        root: root.into(),
        scope: Scope::Uncommitted,
        opened,
        comments: 0,
        unsent: 0,
        pull: None,
        checkpoint: None,
        left,
        space: None,
    }
}

fn listed(rows: &[(&str, Header)]) -> Vec<(String, String, Header)> {
    (rows.iter())
        .map(|(name, header)| ((*name).to_owned(), (*name).to_owned(), header.clone()))
        .collect()
}

fn open(id: ReviewId, left: u64, opened: u64) -> Candidate {
    Candidate {
        left,
        opened,
        space: None,
        what: Continue::Open(id),
    }
}

fn saved_root(found: Option<Candidate>) -> Option<PathBuf> {
    match found?.what {
        Continue::Saved { root, .. } => Some(root),
        Continue::Open(_) => None,
    }
}

#[test]
fn the_review_left_last_is_continued_open_or_saved_and_ties_go_to_the_open_then_the_newer() {
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (a, _) = editor.add_review(project, "a".into());
    let (b, _) = editor.add_review(project, "b".into());
    // Nothing left or saved: none, and the menu opens.
    assert_eq!(last_left(std::iter::empty()), None);
    let saved = listed(&[
        ("old", header("/old", 1, None)),
        ("new", header("/new", 2, Some(500))),
    ]);
    // A saved review left after every open one is continued.
    let found = last_left(
        [open(a, 100, 10), open(b, 300, 20)]
            .into_iter()
            .chain(listed_candidates(&saved, &[])),
    );
    assert_eq!(saved_root(found), Some("/new".into()));
    // One open left later goes first.
    let found = last_left(
        [open(a, 100, 10), open(b, 600, 20)]
            .into_iter()
            .chain(listed_candidates(&saved, &[])),
    );
    assert_eq!(found.map(|found| found.what), Some(Continue::Open(b)));
    // A tie of stamps: opened later wins; a tie of both: the open one, given first.
    let found = last_left([open(a, 500, 1), open(b, 500, 9)].into_iter());
    assert_eq!(found.map(|found| found.what), Some(Continue::Open(b)));
    let found = last_left(std::iter::once(open(a, 500, 2)).chain(listed_candidates(&saved, &[])));
    assert_eq!(found.map(|found| found.what), Some(Continue::Open(a)));
    // A saved review the open one is, or of no stamp, from before: the stamped one, else the
    // one opened last.
    let found = last_left(listed_candidates(&saved, &["new".to_owned()]));
    assert_eq!(saved_root(found), Some("/old".into()));
    let before = listed(&[("x", header("/x", 7, None)), ("y", header("/y", 3, None))]);
    assert_eq!(
        saved_root(last_left(listed_candidates(&before, &[]))),
        Some("/x".into())
    );
}

/// A private directory of the test's own, removed as it ends.
#[cfg(unix)]
struct Scratch(PathBuf);

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The layout's own state as it was left (ADR 0184), a source in its slot.
#[cfg(unix)]
fn layout() -> crate::review::saved::SavedLayout {
    use crate::review::saved::{SavedLayout, SlotShows};
    SavedLayout {
        slot_selected: true,
        slot_permille: 350,
        ask_permille: Some(450),
        slot: SlotShows::Source {
            path: "/q/src/b.rs".into(),
            line: 70,
            column: 4,
            above: 6,
        },
    }
}

#[cfg(unix)]
#[test]
fn a_review_left_and_saved_as_oso_goes_is_the_one_a_new_run_continues_with_its_place() {
    use std::os::unix::fs::PermissionsExt as _;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    use oso_core::review::Side;

    use super::super::store::{Snapshot, Store, Stored, Told};
    use crate::review::saved::{self, Place, SavedView};

    let dir = std::env::temp_dir().join(format!(
        "oso-review-left-{}-{:?}",
        std::process::id(),
        std::time::SystemTime::now()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let scratch = Scratch(dir);
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (review, _) = editor.add_review(project, "r".into());
    let space = "0123456789abcdef0123456789abcdef";
    let place = Place {
        path: b"src/a.rs".as_slice().into(),
        line: Some((Side::New, 40)),
        above: Some(9),
    };
    let layout = layout();
    // Two reviews saved, the second left later, as `hand_review_saves` hands them at a quit.
    let (told, _heard) = mpsc::channel();
    let store = Store::start(&scratch.0, move |stored| {
        let _ = told.send(stored);
    })
    .unwrap();
    for (root, left) in [("/p", 2_000), ("/q", 3_000)] {
        let snapshot = Snapshot {
            header: Header {
                left: Some(left),
                space: Some(space.into()),
                ..header(root, 1_000, None)
            },
            saved: oso_core::review::Review::new().save(),
            view: SavedView {
                place: Some(place.clone()),
                layout: Some(layout.clone()),
                ..SavedView::default()
            },
            root: root.into(),
            held: Arc::default(),
            listing: Arc::from(Vec::new()),
        };
        let name = saved::file_name(std::path::Path::new(root), &Scope::Uncommitted);
        assert!(store.write(Told::Review(review), name, snapshot).is_ok());
    }
    assert!(store.flush(Duration::from_secs(10)));
    drop(store);
    // A new run on the same state: its list, once idle, names the one left last.
    let (told, heard) = mpsc::channel();
    let store = Store::start(&scratch.0, move |stored| {
        let _ = told.send(stored);
    })
    .unwrap();
    let listed = loop {
        if let Stored::Listed(listed) = heard.recv_timeout(Duration::from_secs(20)).unwrap() {
            break listed;
        }
    };
    let listed: Vec<(String, String, Header)> = (listed.into_iter())
        .map(|(name, header)| (name.clone(), name, header))
        .collect();
    let found = last_left(listed_candidates(&listed, &[])).unwrap();
    assert_eq!(found.space.as_deref(), Some(space));
    let Continue::Saved { root, .. } = found.what else {
        panic!("a saved review");
    };
    assert_eq!(root, PathBuf::from("/q"));
    // Read back, its cursor and the rows above it are as they were left.
    let (_, name, header) = listed.iter().find(|(_, _, h)| h.root == root).unwrap();
    assert!(store.read(review, name.clone(), header.clone()));
    let found = loop {
        if let Stored::Read { found, .. } = heard.recv_timeout(Duration::from_secs(20)).unwrap() {
            break found.unwrap().unwrap();
        }
    };
    assert_eq!(found.view.place, Some(place));
    assert_eq!(found.view.layout, Some(layout));
}

#[test]
fn a_saved_review_continues_only_in_a_project_oso_knows_and_trusts_and_else_says_why() {
    use super::super::list::resumable;
    use crate::commands::review::RESTRICTED;

    let root = std::path::Path::new("/home/me/src/oso");
    let (editor, project) = crate::commands::review::tests::in_project(true);
    assert_eq!(resumable(&editor, Some(project), root), Ok(project));
    // Not trusted: refused before any thread starts, so nothing runs git.
    let (editor, project) = crate::commands::review::tests::in_project(false);
    assert_eq!(
        resumable(&editor, Some(project), root),
        Err(RESTRICTED.to_owned())
    );
    assert!(editor.reviews().next().is_none());
    // Not known: the person opens a file of it first.
    let said = resumable(&editor, None, root).unwrap_err();
    assert!(
        said.contains("open a file of /home/me/src/oso first"),
        "{said}"
    );
}
