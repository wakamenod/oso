//! Tests of the directory editor's commands: what they ask of the host, and moving by rows.
//!
//! Must not depend on: `app` or winit.

use std::path::PathBuf;

use oso_core::command::Args;
use oso_core::editor::{Editor, Listing, ListingRequest, ListingShown};
use oso_core::load::decode_all;
use oso_core::syntax::Highlights;

use crate::commands::tests::{cursor, editor, run};

const TEXT: &str = "  ~/d/:\n  drwx  .\n  drwx  ..\n  -rw-  a.md\n  -rw-  b.md\n";

/// An editor showing a listing of `/tmp/d` with four rows, point on `a.md`.
fn listed() -> (Editor, oso_core::buffer::BufferId) {
    let mut editor = editor("");
    let id = editor.add_listing(PathBuf::from("/tmp/d"));
    editor.show(id);
    let shown = ListingShown {
        record: Listing {
            directory: PathBuf::from("/tmp/d"),
            header_lines: 1,
            name_column: 8,
            rows: 4,
            details: true,
            order: oso_core::editor::ListingOrder::Name,
            marked: 0,
        },
        highlights: Highlights::new(TEXT.len()),
        resolved: Some(vec![PathBuf::from("/tmp/d")]),
        line: 3,
        lines: Vec::new(),
    };
    editor.set_listing_text(id, decode_all(TEXT.as_bytes(), Default::default()), shown);
    (editor, id)
}

#[test]
fn n_and_p_move_by_rows_to_the_names_and_stay_on_them() {
    let (mut editor, _) = listed();
    let (result, _) = run(&mut editor, "dired-next-line", Args::NONE);
    assert!(result.is_ok());
    assert_eq!(cursor(&editor), TEXT.find("b.md").unwrap());
    let _ = run(&mut editor, "dired-next-line", Args::NONE);
    assert_eq!(cursor(&editor), TEXT.find("b.md").unwrap());
    let _ = run(
        &mut editor,
        "dired-previous-line",
        Args::with_prefix(Some(oso_core::command::Prefix::Number(9))),
    );
    assert_eq!(cursor(&editor), TEXT.find(".\n").unwrap());
}

#[test]
fn ret_asks_for_the_row_at_point() {
    let (mut editor, id) = listed();
    let _ = run(&mut editor, "dired-find-file", Args::NONE);
    let _ = run(&mut editor, "dired-find-file-other-window", Args::NONE);
    assert_eq!(
        editor.take_listing_requests(),
        [
            ListingRequest::Visit {
                buffer: id,
                line: 3,
                other_window: false
            },
            ListingRequest::Visit {
                buffer: id,
                line: 3,
                other_window: true
            },
        ]
    );
}

#[test]
fn the_listing_commands_ask_the_host() {
    let (mut editor, id) = listed();
    for name in [
        "dired-up-directory",
        "dired-revert",
        "dired-hide-details-mode",
        "dired-sort-toggle",
        "dired-jump",
    ] {
        assert!(run(&mut editor, name, Args::NONE).0.is_ok(), "{name}");
    }
    let _ = run(
        &mut editor,
        "dired-copy-filename-as-kill",
        Args::with_prefix(Some(oso_core::command::Prefix::Number(0))),
    );
    assert_eq!(
        editor.take_listing_requests(),
        [
            ListingRequest::Up { buffer: id },
            ListingRequest::Revert { buffer: id },
            ListingRequest::ToggleDetails { buffer: id },
            ListingRequest::ToggleSort { buffer: id },
            ListingRequest::Up { buffer: id },
            ListingRequest::CopyNames {
                buffer: id,
                lines: vec![3],
                absolute: true
            },
        ]
    );
}

#[test]
fn outside_a_listing_they_say_so() {
    let mut editor = editor("text");
    let (result, _) = run(&mut editor, "dired-next-line", Args::NONE);
    assert_eq!(result.unwrap_err(), "Not in a directory listing");
    assert!(editor.take_listing_requests().is_empty());
}

#[test]
fn dired_jump_lists_the_files_directory_with_point_on_it() {
    let mut editor = editor("");
    let file = editor.add_buffer(oso_core::buffer::Buffer::visiting(PathBuf::from(
        "/src/oso/notes.md",
    )));
    editor.show(file);
    let _ = run(&mut editor, "dired-jump", Args::NONE);
    assert_eq!(
        editor.take_listing_requests(),
        [ListingRequest::Open {
            directory: PathBuf::from("/src/oso"),
            point: Some("notes.md".into())
        }]
    );
}

#[test]
fn quit_window_shows_the_buffer_before() {
    let (mut editor, id) = listed();
    let _ = run(&mut editor, "quit-window", Args::NONE);
    assert_ne!(editor.window().buffer(), id);
    assert!(editor.buffer(id).is_some());
}

#[test]
fn m_marks_the_row_and_moves_down_and_del_moves_up_and_unmarks() {
    let (mut editor, id) = listed();
    let _ = run(&mut editor, "dired-mark", Args::NONE);
    assert_eq!(cursor(&editor), TEXT.find("b.md").unwrap());
    let _ = run(&mut editor, "dired-unmark-backward", Args::NONE);
    assert_eq!(cursor(&editor), TEXT.find("a.md").unwrap());
    let _ = run(
        &mut editor,
        "dired-mark",
        Args::with_prefix(Some(oso_core::command::Prefix::Number(2))),
    );
    assert_eq!(
        editor.take_listing_requests(),
        [
            ListingRequest::Mark {
                buffer: id,
                lines: vec![3],
                on: true
            },
            ListingRequest::Mark {
                buffer: id,
                lines: vec![3],
                on: false
            },
            ListingRequest::Mark {
                buffer: id,
                lines: vec![3, 4],
                on: true
            },
        ]
    );
}

#[test]
fn a_shown_region_marks_the_rows_it_covers_and_point_stays() {
    let (mut editor, id) = listed();
    let _ = run(&mut editor, "set-mark-command", Args::NONE);
    let _ = run(&mut editor, "dired-next-line", Args::NONE);
    let at = cursor(&editor);
    let _ = run(&mut editor, "dired-mark", Args::NONE);
    assert_eq!(cursor(&editor), at);
    let requests = editor.take_listing_requests();
    assert_eq!(
        requests.last(),
        Some(&ListingRequest::Mark {
            buffer: id,
            lines: vec![3, 4],
            on: true
        })
    );
}

#[test]
fn u_and_t_ask_the_host() {
    let (mut editor, id) = listed();
    let _ = run(&mut editor, "dired-unmark-all-marks", Args::NONE);
    let _ = run(&mut editor, "dired-toggle-marks", Args::NONE);
    assert_eq!(
        editor.take_listing_requests(),
        [
            ListingRequest::UnmarkAll { buffer: id },
            ListingRequest::ToggleMarks { buffer: id }
        ]
    );
}

#[test]
fn d_asks_the_host_for_the_rows_and_the_answers_confirm() {
    let (mut editor, id) = listed();
    let _ = run(
        &mut editor,
        "dired-do-delete",
        Args::with_prefix(Some(oso_core::command::Prefix::Number(2))),
    );
    let _ = run(&mut editor, "dired-do-delete-confirmed", Args::NONE);
    let _ = run(&mut editor, "dired-do-overwrite-confirmed", Args::NONE);
    assert_eq!(
        editor.take_listing_requests(),
        [
            ListingRequest::Delete {
                buffer: id,
                lines: vec![3, 4]
            },
            ListingRequest::DeleteConfirmed,
            ListingRequest::OverwriteConfirmed,
        ]
    );
}

#[test]
fn the_operations_are_the_persons() {
    let registry = crate::commands::registry();
    for name in [
        "dired-do-copy",
        "dired-do-rename",
        "dired-do-delete",
        "dired-create-directory",
        "dired-do-delete-confirmed",
        "dired-do-overwrite-confirmed",
    ] {
        let id = registry.lookup(name).unwrap();
        assert_eq!(
            registry.get(id).unwrap().reach,
            oso_core::command::Reach::Person,
            "{name}"
        );
    }
}
