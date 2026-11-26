//! The tests of the saved reviews' store: what is written is read back for its own project and
//! scope only, in a private directory, another review's file is never written over, and what
//! is not a saved review's file is never read or removed; what may hold secrets is not written;
//! the sweep, the cap and parts; and the thread's answers.
//!
//! Must not depend on: the editor or the UI thread.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::mpsc;

use oso_core::review::{Anchor, Author, Base, Hunk, Shape, Side};

use super::*;
use crate::git::Id;
use crate::review::{Scope, Status};

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

    fn thread(&self) -> Thread {
        Thread::new(self.reviews())
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
        pull: None,
        checkpoint: None,
        left: None,
        space: None,
    }
}

/// A review with one comment of the person's on each of `paths`, whose added line reads `line`.
fn snapshot_of(root: &str, opened: u64, paths: &[&[u8]], line: &str) -> Snapshot {
    let mut review = Review::new();
    for path in paths {
        let shape = Shape::new(
            Base::Unknown,
            vec![Hunk {
                old_start: 1,
                new_start: 1,
                removed: Vec::new(),
                added: vec![line.as_bytes().into()],
            }],
        )
        .unwrap();
        let carried = review
            .carry(path, Arc::new(shape))
            .run(|_, _| None, |_: &[&[u8]], _: &[&[u8]]| Vec::new());
        review.apply(carried);
        let at = Anchor::Line {
            side: Side::New,
            number: 1,
        };
        review
            .add_comment(Author::Person, path, at, "kept?".into())
            .unwrap();
    }
    Snapshot {
        header: header(root, opened),
        saved: review.save(),
        view: SavedView::default(),
        root: root.into(),
        held: Arc::default(),
        listing: Arc::from(Vec::new()),
    }
}

fn snapshot(root: &str, opened: u64) -> Snapshot {
    snapshot_of(root, opened, &[b"src/a.rs"], "line\n")
}

fn name(root: &str) -> String {
    saved::file_name(Path::new(root), &Scope::Uncommitted)
}

/// The bytes of the review saved for `root`.
fn on_disk(scratch: &Scratch, root: &str) -> String {
    fs::read_to_string(scratch.reviews().join(name(root))).unwrap()
}

#[test]
fn a_review_written_is_read_back_for_its_own_project_and_scope_only() {
    let scratch = Scratch::new("back");
    let dir = scratch.reviews();
    let mut thread = scratch.thread();
    assert!(
        read(&dir, &name("/p"), &header("/p", 0)).unwrap().is_none(),
        "nothing yet"
    );
    assert!(thread.write(&name("/p"), snapshot("/p", 5)).unwrap());
    let found = read(&dir, &name("/p"), &header("/p", 0)).unwrap().unwrap();
    assert_eq!(found.review.comments().count(), 1);
    assert!(
        list(&dir).unwrap()[0].1.opened >= now() - 60_000,
        "dated as written"
    );
    // Another project's file under this name, as a name that came out the same: an error, so
    // that it is never written over.
    fs::rename(dir.join(name("/p")), dir.join(name("/q"))).unwrap();
    assert!(read(&dir, &name("/q"), &header("/q", 0)).is_err());
    // Private: the directory 0700, the file 0600, and no part left.
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&dir.join(name("/q"))), 0o600);
    let parts = fs::read_dir(&dir).unwrap().filter(|entry| {
        let name = entry.as_ref().unwrap().file_name();
        name.to_str()
            .is_some_and(|name| name.ends_with(PART_SUFFIX))
    });
    assert_eq!(parts.count(), 0);
}

#[test]
fn a_write_of_the_same_bytes_writes_nothing_and_a_change_writes() {
    let scratch = Scratch::new("same");
    let mut thread = scratch.thread();
    assert!(thread.write(&name("/p"), snapshot("/p", 5)).unwrap());
    let file = scratch.reviews().join(name("/p"));
    fs::write(&file, "moved by hand").unwrap();
    assert!(
        !thread.write(&name("/p"), snapshot("/p", 5)).unwrap(),
        "the same review"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "moved by hand");
    let changed = snapshot_of("/p", 5, &[b"src/a.rs"], "new\n");
    assert!(thread.write(&name("/p"), changed).unwrap());
}

#[test]
fn no_line_of_a_file_that_may_hold_secrets_is_written() {
    let scratch = Scratch::new("secrets");
    let mut thread = scratch.thread();
    let paths: [&[u8]; 4] = [b".env", b"notes.txt", b"moved.txt", b"plain.txt"];
    let mut snapshot = snapshot_of("/p", 5, &paths, "HIDDEN\n");
    // An open buffer of `notes.txt` was seen to hold secrets.
    snapshot.held = Arc::new(HashSet::from([PathBuf::from("/p/notes.txt")]));
    // `moved.txt` was renamed from a file whose name may hold secrets.
    let zero = Id::parse(&[b'0'; 40]).unwrap();
    snapshot.listing = Arc::from(vec![Entry {
        path: b"moved.txt".as_slice().into(),
        old_path: Some(b"id_rsa".as_slice().into()),
        status: Status::Renamed,
        old_mode: 0o100_644,
        new_mode: 0o100_644,
        old_id: zero.clone(),
        new_id: zero,
    }]);
    thread.write(&name("/p"), snapshot).unwrap();
    let text = on_disk(&scratch, "/p");
    assert_eq!(text.matches("HIDDEN").count(), 1, "plain.txt's alone");
    assert_eq!(text.matches("kept?").count(), 4, "every comment's text");
}

#[test]
fn a_slot_s_file_that_may_hold_secrets_is_written_as_nothing() {
    // ADR 0184: judged again on the store's thread, as the review's files are.
    let scratch = Scratch::new("slot");
    let dir = scratch.reviews();
    let mut thread = scratch.thread();
    let layout = |slot| saved::SavedLayout {
        slot_selected: false,
        slot_permille: 400,
        ask_permille: None,
        slot,
    };
    let source = |path: &str| saved::SlotShows::Source {
        path: path.into(),
        line: 3,
        column: 0,
        above: 1,
    };
    let side = |path: &[u8]| saved::SlotShows::Side {
        path: path.into(),
        side: Side::New,
        line: 3,
        above: 1,
    };
    let cases = [
        (source("/p/.env"), saved::SlotShows::Nothing),
        (source("/p/notes.txt"), saved::SlotShows::Nothing),
        (side(b"notes.txt"), saved::SlotShows::Nothing),
        (side(b"id_rsa"), saved::SlotShows::Nothing),
        (source("/p/plain.txt"), source("/p/plain.txt")),
        (side(b"plain.txt"), side(b"plain.txt")),
    ];
    for (shows, written) in cases {
        let mut snapshot = snapshot("/p", 5);
        // An open buffer of `notes.txt` was seen to hold secrets.
        snapshot.held = Arc::new(HashSet::from([PathBuf::from("/p/notes.txt")]));
        snapshot.view.layout = Some(layout(shows));
        thread.write(&name("/p"), snapshot).unwrap();
        let found = read(&dir, &name("/p"), &header("/p", 0)).unwrap().unwrap();
        assert_eq!(found.view.layout, Some(layout(written)));
    }
}

#[test]
fn a_file_that_is_not_a_saved_reviews_own_is_refused() {
    let scratch = Scratch::new("refused");
    let dir = scratch.reviews();
    let mut thread = scratch.thread();
    thread.write(&name("/p"), snapshot("/p", 5)).unwrap();
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
    assert!(thread.write("x", snapshot("/p", 5)).is_err());
    assert!(remove(&dir, "../other").is_err());
}

/// Dates the review saved for `root` in `dir` as last used at `opened`, by hand.
fn date(dir: &Path, root: &str, opened: u64) {
    let path = dir.join(name(root));
    let text = fs::read_to_string(&path).unwrap();
    let first = text.find('\n').unwrap();
    let mut head: serde_json::Value = serde_json::from_str(&text[..first]).unwrap();
    head["opened"] = opened.into();
    fs::write(&path, format!("{head}{}", &text[first..])).unwrap();
}

#[test]
fn the_sweep_removes_reviews_not_used_for_long_dated_ahead_or_past_the_most() {
    let scratch = Scratch::new("sweep");
    let dir = scratch.reviews();
    let day = 24 * 60 * 60 * 1000;
    let now = now();
    let mut thread = scratch.thread();
    for root in ["/old", "/a", "/b", "/ahead"] {
        thread.write(&name(root), snapshot(root, 0)).unwrap();
    }
    date(&dir, "/old", now - 31 * day);
    date(&dir, "/b", now - 2 * day);
    date(&dir, "/ahead", now + 40 * day);
    fs::write(dir.join("not-a-review"), "kept").unwrap();
    let mut thread = scratch.thread();
    thread.sweep(now);
    let roots = |thread: &Thread| -> Vec<PathBuf> {
        (thread.list().into_iter())
            .map(|(_, header)| header.root)
            .collect()
    };
    assert_eq!(roots(&thread), [PathBuf::from("/a"), PathBuf::from("/b")]);
    thread.keep_at_most(1);
    assert_eq!(roots(&thread), [PathBuf::from("/a")]);
    assert_eq!(list(&dir).unwrap().len(), 1, "the disk as the list says");
    assert!(dir.join("not-a-review").exists(), "nothing else is touched");
}

#[test]
fn only_parts_an_hour_old_are_swept() {
    let scratch = Scratch::new("parts");
    let dir = scratch.reviews();
    scratch
        .thread()
        .write(&name("/p"), snapshot("/p", 5))
        .unwrap();
    let part = format!("{}.123.4{PART_SUFFIX}", name("/p"));
    let names = [part.as_str(), "review.part", "x.part"];
    for name in names {
        fs::write(dir.join(name), "part").unwrap();
    }
    assert!(is_part_name(&part) && is_part_name("review.part") && !is_part_name("x.part"));
    remove_stale_parts(&dir).unwrap();
    assert!(dir.join(&part).exists(), "a write of now may be running");
    let old = SystemTime::now() - Duration::from_secs(2 * 60 * 60);
    for name in names {
        let file = fs::File::options()
            .write(true)
            .open(dir.join(name))
            .unwrap();
        file.set_modified(old).unwrap();
    }
    remove_stale_parts(&dir).unwrap();
    assert!(!dir.join(&part).exists() && !dir.join("review.part").exists());
    assert!(
        dir.join("x.part").exists(),
        "not a part a review's write makes"
    );
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
    let wrote = store.write(Told::Review(review), name("/p"), snapshot("/p", now()));
    assert!(wrote.is_ok());
    assert!(store.read(review, name("/p"), header("/p", 0)));
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
    assert!(store.remove(name("/p")));
    assert!(store.flush(Duration::from_secs(10)));
    assert!(list(&scratch.reviews()).unwrap().is_empty());
}
