//! Tests of directory listings in the editor (ADR 0134).
//!
//! Must not depend on: the host's threads or files.

use std::path::PathBuf;

use super::{Listing, ListingShown};
use crate::buffer::Mode;
use crate::editor::Editor;
use crate::load::decode_all;
use crate::rope::Rope;
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
            order: super::super::ListingOrder::Name,
            marked: 0,
        },
        highlights,
        resolved: Some(vec![PathBuf::from(directory)]),
        line,
        lines,
    }
}

#[test]
fn a_listing_goes_in_with_point_on_a_name() {
    let mut editor = Editor::default();
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
    editor.show(id);
    let given = editor.set_listing_text(
        id,
        decode_all(TEXT.as_bytes(), Default::default()),
        shown("/tmp/d", 3, Vec::new()),
    );
    assert!(given.is_none());
    let buffer = editor.buffer(id).unwrap();
    let highlights = buffer.highlights().expect("highlights");
    assert_eq!(highlights.at(3), Some(Highlight::DiredHeader));
    assert_eq!(buffer.text().to_string(), TEXT);
    assert_eq!(buffer.mode(), Mode::Directory);
    assert_eq!(buffer.name(), "d/");
    assert_eq!(buffer.verdict(), Verdict::Ordinary);
    assert!(buffer.read_only().is_some());
    let cursor = editor.window().selections.primary().cursor;
    assert_eq!(cursor, TEXT.find("notes").unwrap());
    assert_eq!(
        editor.listing(id).unwrap().order,
        super::super::ListingOrder::Name
    );
    assert_eq!(editor.listing_of(&PathBuf::from("/tmp/d")), Some(id));
    assert_eq!(editor.listing_points(id), vec![(editor.window().id(), 3)]);
}

#[test]
fn a_window_keeps_the_line_the_host_found() {
    let mut editor = Editor::default();
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
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
    let id = editor.add_listing(PathBuf::from("/home/me/.ssh"));
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
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
    let mut wrong = shown("/tmp/d", 1, Vec::new());
    wrong.highlights = Highlights::new(3);
    let given = editor.set_listing_text(id, decode_all(TEXT.as_bytes(), Default::default()), wrong);
    assert_eq!(given.map(|given| given.len()), Some(3));
}

#[test]
fn a_person_goes_to_a_listing_and_kills_it() {
    let mut editor = Editor::default();
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
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
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
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

#[test]
fn listings_of_one_name_are_told_apart_by_the_directories_above() {
    let mut editor = Editor::default();
    let oso = editor.add_listing(PathBuf::from("/src/oso/crates"));
    assert_eq!(editor.buffer(oso).unwrap().name(), "crates/");
    let zed = editor.add_listing(PathBuf::from("/src/zed/crates"));
    assert_eq!(editor.buffer(oso).unwrap().name(), "crates/<oso>");
    assert_eq!(editor.buffer(zed).unwrap().name(), "crates/<zed>");
    let root = editor.add_listing(PathBuf::from("/"));
    assert_eq!(editor.buffer(root).unwrap().name(), "/");
    editor.show(zed);
    editor.kill_by_person(zed, false).unwrap();
    assert_eq!(editor.buffer(oso).unwrap().name(), "crates/");
}

#[test]
fn a_mark_changes_one_character_and_its_faces_and_moves_no_cursor() {
    let mut editor = Editor::default();
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
    editor.show(id);
    let text = decode_all(TEXT.as_bytes(), Default::default());
    editor.set_listing_text(id, text, shown("/tmp/d", 3, Vec::new()));
    let cursor = editor.window().selections.primary().cursor;
    let row = super::RowMark {
        line: 3,
        marked: true,
        face: None,
        name_len: "notes.md".len(),
    };
    editor.set_listing_marks(id, &[row], 1);
    let buffer = editor.buffer(id).unwrap();
    let marked = TEXT.replacen("  -rw-  notes", "* -rw-  notes", 1);
    assert_eq!(buffer.text().to_string(), marked);
    assert_eq!(editor.window().selections.primary().cursor, cursor);
    let highlights = buffer.highlights().unwrap();
    let at = marked.find("* -rw").unwrap();
    assert_eq!(highlights.at(at), Some(Highlight::DiredMark));
    assert_eq!(
        highlights.at(marked.find("notes").unwrap()),
        Some(Highlight::DiredMarked)
    );
    assert_eq!(buffer.mode_note(), "by name  1 marked");
    // A header line is no row, and stays as it is.
    let header = super::RowMark { line: 0, ..row };
    editor.set_listing_marks(
        id,
        &[
            header,
            super::RowMark {
                marked: false,
                ..row
            },
        ],
        0,
    );
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.text().to_string(), TEXT);
    assert_eq!(
        buffer.highlights().unwrap().at(TEXT.find("notes").unwrap()),
        None
    );
    assert_eq!(buffer.mode_note(), "by name");
}

#[test]
fn a_name_starts_at_its_column_or_at_a_short_lines_end() {
    let record = Listing {
        name_column: 8,
        ..Listing::default()
    };
    let text = Rope::from(TEXT);
    assert_eq!(record.name_start(&text, 3), TEXT.find("notes").unwrap());
    assert_eq!(record.name_start(&text, 99), TEXT.len());
}
