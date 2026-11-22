//! The tests of a saved review's file: what goes in comes back, nothing of a file that may
//! hold secrets goes in but its comments' places and texts, the limit leaves the largest
//! readings out, and what this does not write is left out as it is read.
//!
//! Must not depend on: the file system or threads.

use std::path::Path;
use std::sync::Arc;

use oso_core::editor::ReviewChoice;
use oso_core::review::{Anchor, Author, Base, Decision, Filter, Hunk, Review, Saved, Shape, Side};

use super::*;

const FILE: &[u8] = b"src/a.rs";
const SECRET: &[u8] = b".env";

fn hunk(old: u32, removed: &[&str], new: u32, added: &[&str]) -> Hunk {
    let lines = |texts: &[&str]| texts.iter().map(|text| text.as_bytes().into()).collect();
    Hunk {
        old_start: old,
        new_start: new,
        removed: lines(removed),
        added: lines(added),
    }
}

fn shape(hunks: Vec<Hunk>) -> Arc<Shape> {
    Arc::new(Shape::new(Base::Blob(b"0123abcd".as_slice().into()), hunks).unwrap())
}

/// Reads `path` as `shape` in `review`, carrying what it holds.
fn read(review: &mut Review, path: &[u8], shape: &Arc<Shape>) {
    let carried = review
        .carry(path, Arc::clone(shape))
        .run(|_, _| None, |_: &[&[u8]], _: &[&[u8]]| Vec::new());
    review.apply(carried);
}

fn header() -> Header {
    Header {
        root: "/work/project".into(),
        scope: Scope::Uncommitted,
        opened: 1_700_000_000_000,
        comments: 3,
        unsent: 1,
        pull: None,
        checkpoint: None,
    }
}

/// A review with comments of both authors, a reply, marks, and a file that may hold secrets.
fn review() -> Review {
    let mut review = Review::new();
    read(
        &mut review,
        FILE,
        &shape(vec![hunk(2, &["old line\n"], 2, &["new line\n", "more\n"])]),
    );
    read(
        &mut review,
        SECRET,
        &shape(vec![hunk(1, &["TOKEN=old\n"], 1, &["TOKEN=hunter2\n"])]),
    );
    let line = Anchor::Line {
        side: Side::New,
        number: 3,
    };
    let mine = review
        .add_comment(Author::Person, FILE, line, "why more?".into())
        .unwrap();
    let agent = review
        .add_comment(
            Author::Agent,
            FILE,
            Anchor::Hunk { index: 0 },
            "it is \"needed\"".into(),
        )
        .unwrap();
    review
        .reply(Author::Person, agent, "ok\nthanks".into())
        .unwrap();
    review.mark_sent(&[mine]);
    review
        .add_comment(Author::Person, SECRET, line_of(1), "rotate this".into())
        .unwrap();
    review.set_viewed(FILE, 0, true);
    review.decide(FILE, 0, Some(Decision::Accepted));
    review.set_viewed(SECRET, 0, true);
    review
}

fn line_of(number: u32) -> Anchor {
    Anchor::Line {
        side: Side::New,
        number,
    }
}

fn withheld(path: &[u8]) -> bool {
    path == SECRET
}

fn round_trip(review: &Review, max: usize) -> (Vec<u8>, Decoded) {
    let kept = SavedView {
        choice: ReviewChoice::Panes,
        folded: true,
        place: Some(Place {
            path: FILE.into(),
            line: Some((Side::New, 3)),
        }),
        filter: Filter::new("src todo"),
        files: true,
    };
    let bytes = encode(&header(), &review.save(), &kept, &withheld, max).unwrap();
    let read = decode(&bytes).unwrap();
    assert_eq!(read.view, kept);
    assert_eq!(read.header, header());
    (bytes, read)
}

#[test]
fn what_is_saved_comes_back() {
    let review = review();
    let (_, read) = round_trip(&review, MAX_BYTES);
    assert_eq!(read.left_out, 0);
    let back = Review::restore(read.saved).unwrap();
    let comments = |review: &Review| -> Vec<(u64, String, bool, bool, Anchor)> {
        (review.comments())
            .filter(|comment| comment.path() == FILE)
            .map(|comment| {
                let id = comment.id().get();
                let text = comment.text().to_owned();
                (
                    id,
                    text,
                    comment.is_sent(),
                    comment.is_read(),
                    comment.anchor(),
                )
            })
            .collect()
    };
    assert_eq!(comments(&back), comments(&review));
    assert_eq!(back.decision(FILE, 0), Some(Decision::Accepted));
    assert!(back.viewed(FILE, 0));
    assert!(
        back.comments()
            .all(|comment| comment.path() != FILE || !comment.is_outdated())
    );
}

#[test]
fn nothing_of_a_file_that_may_hold_secrets_is_written_but_its_comments() {
    let (bytes, read) = round_trip(&review(), MAX_BYTES);
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("hunter2"), "no line of it");
    assert!(!text.contains("TOKEN"), "nor its old side");
    assert!(
        text.contains("rotate this"),
        "its comment's text is the person's"
    );
    let back = Review::restore(read.saved).unwrap();
    assert!(back.file(SECRET).is_none(), "no reading, so no marks");
    let comment = (back.comments())
        .find(|comment| comment.path() == SECRET)
        .unwrap();
    assert!(
        comment.is_outdated(),
        "back outdated, at the line it was on"
    );
    assert_eq!(comment.anchor(), line_of(1));
}

#[test]
fn past_the_limit_the_largest_readings_go_first() {
    let mut review = Review::new();
    let small = shape(vec![hunk(1, &[], 1, &["s\n"])]);
    let large = shape(vec![hunk(1, &[], 1, &[&"l".repeat(10_000)])]);
    read(&mut review, b"small", &small);
    read(&mut review, b"large", &large);
    for path in [&b"small"[..], b"large"] {
        review
            .add_comment(Author::Person, path, line_of(1), "c".into())
            .unwrap();
    }
    let whole = encode(
        &header(),
        &review.save(),
        &SavedView::default(),
        &|_| false,
        MAX_BYTES,
    )
    .unwrap();
    let bytes = encode(
        &header(),
        &review.save(),
        &SavedView::default(),
        &|_| false,
        5_000,
    )
    .unwrap();
    assert!(bytes.len() <= 5_000 && whole.len() > 10_000);
    let back = Review::restore(decode(&bytes).unwrap().saved).unwrap();
    assert!(back.file(b"small").is_some());
    assert!(back.file(b"large").is_none());
    let outdated = |path: &[u8]| {
        (back.comments())
            .find(|comment| comment.path() == path)
            .unwrap()
            .is_outdated()
    };
    assert!(!outdated(b"small"));
    assert!(outdated(b"large"));
    // Comments alone past the limit: nothing is written.
    assert!(
        encode(
            &header(),
            &review.save(),
            &SavedView::default(),
            &|_| false,
            300
        )
        .is_none()
    );
}

#[test]
fn bytes_that_are_not_utf_8_come_back_as_they_were() {
    let mut review = Review::new();
    let path: &[u8] = b"caf\xe9.txt";
    let odd = Arc::new(
        Shape::new(
            Base::Unknown,
            vec![Hunk {
                old_start: 1,
                new_start: 1,
                removed: vec![b"\xff\xfe\n".as_slice().into()],
                added: vec![b"ok\n".as_slice().into()],
            }],
        )
        .unwrap(),
    );
    read(&mut review, path, &odd);
    review
        .add_comment(
            Author::Person,
            path,
            Anchor::Hunk { index: 0 },
            "odd".into(),
        )
        .unwrap();
    let bytes = encode(
        &header(),
        &review.save(),
        &SavedView::default(),
        &|_| false,
        MAX_BYTES,
    )
    .unwrap();
    let back = Review::restore(decode(&bytes).unwrap().saved).unwrap();
    assert_eq!(back.file(path).map(|shape| &**shape), Some(&*odd));
}

#[test]
fn lines_this_does_not_write_are_left_out_with_the_replies_to_them() {
    let review = review();
    let bytes = encode(
        &header(),
        &review.save(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    // The agent's comment is spoilt: its reply goes with it.
    let spoilt: Vec<String> = text
        .lines()
        .map(|line| {
            if line.contains("\"needed\\\"") {
                line.replace("\"author\":\"agent\"", "\"author\":\"someone\"")
            } else {
                line.to_owned()
            }
        })
        .chain(["not json".to_owned(), "{\"what\":1}".to_owned()])
        .collect();
    let read = decode(spoilt.join("\n").as_bytes()).unwrap();
    assert_eq!(
        read.left_out, 4,
        "the comment, its reply and two strange lines"
    );
    let back = Review::restore(read.saved).unwrap();
    assert_eq!(back.comments().count(), 2);
    assert!(decode(b"{\"oso-review\":2}\n").is_err());
    assert!(decode(b"").is_err());
}

#[test]
fn a_header_says_what_the_review_is_from_its_first_line_alone() {
    let bytes = encode(
        &header(),
        &review().save(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    let first = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    assert_eq!(super::header(&bytes[..first]), Some(header()));
    let session = Header {
        scope: Scope::Session {
            baseline: Id::parse(&[b'a'; 40]).unwrap(),
        },
        ..header()
    };
    let bytes = encode(
        &session,
        &Saved::default(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    assert_eq!(super::header(&bytes), Some(session));
    // A commit's review stepped to from a pull request names it (ADR 0167); one of none, as
    // every file before, writes no such field.
    let stepped = Header {
        scope: Scope::Commit {
            revision: "a".repeat(40),
        },
        pull: Some(12),
        ..header()
    };
    let bytes = encode(
        &stepped,
        &Saved::default(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    assert_eq!(super::header(&bytes), Some(stepped));
    let plain = encode(
        &header(),
        &Saved::default(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    assert!(!String::from_utf8_lossy(&plain).contains("\"pull\""));
    // A pull request's review names the person's last review of it (step 2).
    let pulled = Header {
        scope: Scope::PullRequest { number: 12 },
        checkpoint: Some(crate::review::commits::Checkpoint {
            base: Id::parse(&[b'b'; 40]).unwrap(),
            head: Id::parse(&[b'c'; 40]).unwrap(),
        }),
        ..header()
    };
    let read = encode(
        &pulled,
        &Saved::default(),
        &SavedView::default(),
        &withheld,
        MAX_BYTES,
    )
    .unwrap();
    assert_eq!(super::header(&read), Some(pulled));
    let forged = String::from_utf8_lossy(&bytes).replace("\"pull\":12", "\"pull\":0");
    assert_eq!(super::header(forged.as_bytes()).map(|h| h.pull), Some(None));
}

#[test]
fn a_file_name_tells_projects_and_scopes_apart_but_not_baselines() {
    let root = Path::new("/work/project");
    let baseline = |digit: u8| Scope::Session {
        baseline: Id::parse(&[digit; 40]).unwrap(),
    };
    let name = file_name(root, &Scope::Uncommitted);
    assert_eq!(name.len(), 32 + SUFFIX.len());
    assert!(name.ends_with(SUFFIX));
    assert!(name[..32].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        file_name(root, &baseline(b'a')),
        file_name(root, &baseline(b'b'))
    );
    let names: HashSet<String> = [
        file_name(root, &Scope::Uncommitted),
        file_name(Path::new("/work/other"), &Scope::Uncommitted),
        file_name(root, &Scope::Staged),
        file_name(root, &baseline(b'a')),
        file_name(
            root,
            &Scope::Branch {
                base: "main".into(),
            },
        ),
        file_name(
            root,
            &Scope::Commit {
                revision: "main".into(),
            },
        ),
        file_name(
            root,
            &Scope::Range {
                from: "a".into(),
                to: "b".into(),
            },
        ),
        file_name(root, &Scope::PullRequest { number: 7 }),
    ]
    .into_iter()
    .collect();
    assert_eq!(names.len(), 8);
    // A pull request's scope comes back by its number.
    let pr = Scope::PullRequest { number: 7 };
    assert_eq!(super::scope_from_value(&super::scope_value(&pr)), Some(pr));
}

#[test]
fn a_view_kept_before_the_files_pane_reads_with_no_filter_and_no_pane() {
    let kept = serde_json::json!({ "choice": "auto", "folded": false, "place": null });
    let view = super::view_of(&kept).unwrap();
    assert_eq!(view.filter, None);
    assert!(!view.files);
    // A filter read back is cut and trimmed as one typed is; one of no word is none.
    let long = "x".repeat(4096);
    let kept = serde_json::json!({
        "choice": "auto", "folded": false, "place": null, "filter": long, "files": true,
    });
    let view = super::view_of(&kept).unwrap();
    assert_eq!(view.filter.unwrap().text().len(), 1024);
    assert!(view.files);
    let blank = serde_json::json!({
        "choice": "auto", "folded": false, "place": null, "filter": "  ",
    });
    assert_eq!(super::view_of(&blank).unwrap().filter, None);
    let wrong = serde_json::json!({
        "choice": "auto", "folded": false, "place": null, "filter": 3,
    });
    assert!(super::view_of(&wrong).is_none());
}
