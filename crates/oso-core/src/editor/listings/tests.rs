//! Tests of directory listings in the editor (ADR 0134).
//!
//! Must not depend on: the host's threads or files.

use std::path::PathBuf;

use super::{Listing, ListingShown};
use crate::buffer::Mode;
use crate::editor::Editor;
use crate::load::decode_all;
use crate::secret::Verdict;
use crate::syntax::{Highlight, Highlights};

const TEXT: &str = "  /tmp/d:\n  drwx  .\n  drwx  ..\n  -rw-  notes.md\n";

fn shown(
    directory: &str,
    line: usize,
    lines: Vec<(crate::window::WindowId, usize)>,
) -> ListingShown {
    let mut highlights = Highlights::new(TEXT.len());
    highlights.replace(0..9, [(9, Some(Highlight::DiredHeader))]);
    ListingShown {
        record: Listing {
            directory: PathBuf::from(directory),
            header_lines: 1,
            name_column: 8,
            rows: 3,
            details: true,
            note: "by name".to_owned(),
        },
        highlights,
        name: "d/".to_owned(),
        resolved: Some(vec![PathBuf::from(directory)]),
        line,
        lines,
    }
}

#[test]
fn a_listing_goes_in_with_point_on_a_name() {
    let mut editor = Editor::default();
    let id = editor.add_listing("d/", PathBuf::from("/tmp/d"));
    editor.show(id);
    let given = editor.set_listing_text(
        id,
        decode_all(TEXT.as_bytes(), Default::default()),
        shown("/tmp/d", 3, Vec::new()),
    );
    assert!(given.is_none());
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.text().to_string(), TEXT);
    assert_eq!(buffer.mode(), Mode::Directory);
    assert_eq!(buffer.name(), "d/");
    assert_eq!(buffer.verdict(), Verdict::Ordinary);
    assert!(buffer.read_only().is_some());
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.find("notes").unwrap());
    assert_eq!(editor.listing(id).unwrap().note, "by name");
    assert_eq!(editor.listing_of(&PathBuf::from("/tmp/d")), Some(id));
    assert_eq!(editor.listing_points(id), vec![(editor.window().id(), 3)]);
}

#[test]
fn a_window_keeps_the_line_the_host_found() {
    let mut editor = Editor::default();
    let id = editor.add_listing("d/", PathBuf::from("/tmp/d"));
    editor.show(id);
    let window = editor.window().id();
    let text = decode_all(TEXT.as_bytes(), Default::default());
    editor.set_listing_text(id, text, shown("/tmp/d", 1, vec![(window, 2)]));
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.find("..").unwrap());
}

#[test]
fn a_listing_inside_ssh_is_withheld() {
    let mut editor = Editor::default();
    let id = editor.add_listing(".ssh/", PathBuf::from("/home/me/.ssh"));
    // Before its text comes, by its path's words.
    assert_eq!(editor.buffer(id).unwrap().verdict(), Verdict::SecretName);
    let text = decode_all(TEXT.as_bytes(), Default::default());
    editor.set_listing_text(id, text, shown("/home/me/.ssh", 1, Vec::new()));
    assert_eq!(editor.buffer(id).unwrap().verdict(), Verdict::SecretName);
    // One whose place could not be found counts as one that may hold secrets.
    let mut unresolved = shown("/tmp/d", 1, Vec::new());
    unresolved.resolved = None;
    editor.set_listing_text(
        id,
        decode_all(TEXT.as_bytes(), Default::default()),
        unresolved,
    );
    assert!(editor.buffer(id).unwrap().is_secret());
}

#[test]
fn highlights_that_do_not_cover_the_text_come_back() {
    let mut editor = Editor::default();
    let id = editor.add_listing("d/", PathBuf::from("/tmp/d"));
    let mut wrong = shown("/tmp/d", 1, Vec::new());
    wrong.highlights = Highlights::new(3);
    let given = editor.set_listing_text(id, decode_all(TEXT.as_bytes(), Default::default()), wrong);
    assert_eq!(given.map(|given| given.len()), Some(3));
}

#[test]
fn a_person_goes_to_a_listing_and_kills_it() {
    let mut editor = Editor::default();
    let id = editor.add_listing("d/", PathBuf::from("/tmp/d"));
    editor.set_listing_text(
        id,
        decode_all(TEXT.as_bytes(), Default::default()),
        shown("/tmp/d", 1, Vec::new()),
    );
    assert!(editor.buffers_by_recency().contains(&id));
    assert_eq!(editor.buffer_named("d/"), Some(id));
    editor.show(id);
    editor.kill_by_person(id, false).unwrap();
    assert!(editor.buffer(id).is_none());
    assert!(editor.listing(id).is_none());
    assert!(editor.listings().next().is_none());
}

#[test]
fn requests_go_to_the_host_in_order() {
    let mut editor = Editor::default();
    let id = editor.add_listing("d/", PathBuf::from("/tmp/d"));
    editor.request_listing(super::ListingRequest::Revert { buffer: id });
    editor.request_listing(super::ListingRequest::Up { buffer: id });
    assert_eq!(
        editor.take_listing_requests(),
        [
            super::ListingRequest::Revert { buffer: id },
            super::ListingRequest::Up { buffer: id }
        ]
    );
    assert!(editor.take_listing_requests().is_empty());
}
