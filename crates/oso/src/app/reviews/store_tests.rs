//! The tests of the saved reviews' store: what is written is read back for its own project and
//! scope only, in a private directory, and what is not a saved review's file is never read or
//! removed; the sweep and the cap; and the thread's answers.
//!
//! Must not depend on: the editor or the UI thread.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::sync::mpsc;

use oso_core::review::{Anchor, Author, Base, Hunk, Shape, Side};

use super::*;
use crate::review::Scope;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "oso-review-store-{name}-{}-{:?}",
            std::process::id(),
            SystemTime::now()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        Self(dir)
    }

    fn reviews(&self) -> PathBuf {
        self.0.join(DIRECTORY)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn header(root: &str, opened: u64) -> Header {
    Header {
        root: root.into(),
        scope: Scope::Uncommitted,
        opened,
        comments: 1,
        unsent: 1,
    }
}

/// A review with one comment of the person's on `src/a.rs`.
fn snapshot(root: &str, opened: u64) -> Snapshot {
    let mut review = Review::new();
    let shape = Shape::new(
        Base::Unknown,
        vec![Hunk {
            old_start: 1,
            new_start: 1,
            removed: Vec::new(),
            added: vec![b"line\n".as_slice().into()],
        }],
    )
    .unwrap();
    let carried = review
        .carry(b"src/a.rs", std::sync::Arc::new(shape))
        .run(|_, _| None, |_: &[&[u8]], _: &[&[u8]]| Vec::new());
    review.apply(carried);
    let line = Anchor::Line {
        side: Side::New,
        number: 1,
    };
    review
        .add_comment(Author::Person, b"src/a.rs", line, "kept?".into())
        .unwrap();
    Snapshot {
        header: header(root, opened),
        saved: review.save(),
        kept: Kept::default(),
        withheld: HashSet::new(),
    }
}

fn name(root: &str) -> String {
    saved::file_name(Path::new(root), &Scope::Uncommitted)
}

#[test]
fn a_review_written_is_read_back_for_its_own_project_and_scope_only() {
    let scratch = Scratch::new("back");
    let dir = scratch.reviews();
    assert!(
        read(&dir, &name("/p"), &header("/p", 0)).unwrap().is_none(),
        "nothing yet"
    );
    write(&dir, &name("/p"), &snapshot("/p", 5)).unwrap();
    let found = read(&dir, &name("/p"), &header("/p", 0)).unwrap().unwrap();
    assert_eq!(found.review.comments().count(), 1);
    assert_eq!(list(&dir).unwrap()[0].1.opened, 5);
    // Another project's header under this name, as a name that came out the same.
    fs::rename(dir.join(name("/p")), dir.join(name("/q"))).unwrap();
    assert!(read(&dir, &name("/q"), &header("/q", 0)).unwrap().is_none());
    // Private: the directory 0700, the file 0600.
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(name("/q"))), 0o600);
}

#[test]
fn a_file_that_is_not_a_saved_reviews_own_is_refused() {
    let scratch = Scratch::new("refused");
    let dir = scratch.reviews();
    write(&dir, &name("/p"), &snapshot("/p", 5)).unwrap();
    let file = dir.join(name("/p"));
    // A second name.
    let other = scratch.0.join("other");
    fs::hard_link(&file, &other).unwrap();
    assert!(read(&dir, &name("/p"), &header("/p", 0)).is_err());
    fs::remove_file(&other).unwrap();
    // A link to somewhere else.
    fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(&other, &file).unwrap();
    assert!(read(&dir, &name("/p"), &header("/p", 0)).is_err());
    fs::remove_file(&file).unwrap();
    // Too large.
    fs::write(&file, vec![b'x'; saved::MAX_BYTES + 1]).unwrap();
    assert!(read(&dir, &name("/p"), &header("/p", 0)).is_err());
    // A name that is not a saved review's is neither read, written nor removed.
    assert!(read(&dir, "../x.review", &header("/p", 0)).is_err());
    assert!(write(&dir, "x", &snapshot("/p", 5)).is_err());
    assert!(remove(&dir, "../other").is_err());
}

#[test]
fn the_sweep_removes_reviews_not_opened_for_long_and_those_past_the_most() {
    let scratch = Scratch::new("sweep");
    let dir = scratch.reviews();
    let day = 24 * 60 * 60 * 1000;
    let now = 100 * day;
    for (root, opened) in [
        ("/old", now - 31 * day),
        ("/a", now - day),
        ("/b", now - 2 * day),
    ] {
        write(&dir, &name(root), &snapshot(root, opened)).unwrap();
    }
    fs::write(dir.join("not-a-review"), "kept").unwrap();
    sweep(&dir, now).unwrap();
    let roots = |dir: &Path| -> Vec<PathBuf> {
        list(dir)
            .unwrap()
            .into_iter()
            .map(|(_, header)| header.root)
            .collect()
    };
    assert_eq!(roots(&dir), [PathBuf::from("/a"), PathBuf::from("/b")]);
    keep_at_most(&dir, 1).unwrap();
    assert_eq!(roots(&dir), [PathBuf::from("/a")]);
    assert!(dir.join("not-a-review").exists(), "nothing else is touched");
}

#[test]
fn the_thread_reads_writes_lists_and_removes_in_order() {
    let scratch = Scratch::new("thread");
    let (told, heard) = mpsc::channel();
    let store = Store::start(&scratch.0, move |stored| {
        let _ = told.send(stored);
    })
    .unwrap();
    let (mut editor, project) = crate::commands::review::tests::in_project(true);
    let (review, _) = editor.add_review(project, "r".into());
    assert!(store.write(Some(review), name("/p"), snapshot("/p", now())));
    assert!(store.read(review, name("/p"), header("/p", 0)));
    assert!(store.list());
    assert!(store.flush(Duration::from_secs(10)));
    let heard: Vec<Stored> = heard.try_iter().collect();
    assert!(matches!(heard[0], Stored::Written { written: true, .. }));
    assert!(matches!(
        &heard[1],
        Stored::Read {
            found: Ok(Some(_)),
            ..
        }
    ));
    assert!(matches!(&heard[2], Stored::Listed(listed) if listed.len() == 1));
    assert!(store.remove(name("/p")));
    assert!(store.flush(Duration::from_secs(10)));
    assert!(list(&scratch.reviews()).unwrap().is_empty());
}
