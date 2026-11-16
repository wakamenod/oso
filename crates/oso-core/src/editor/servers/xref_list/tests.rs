//! Tests of the list of a server's places (ADR 0138, step 8b): the rows as they read, a stale
//! list dropped, the preview and `RET` of a row of an open buffer, of a file under the server's
//! root and of one outside it, and a file that may hold secrets.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};

use super::*;
use crate::buffer::{Baseline, Buffer, FileStamp};
use crate::display_map::TabWidth;
use crate::editor::edit_tests::edit;
use crate::editor::{FilePreviewRead, PreviewAt, VisitColumn};
use crate::frame::FrameSize;
use crate::load::decode_all;

/// A buffer visiting `/w/oso/a.rs`, holding `text`, judged ordinary, followed by a server.
fn visiting(text: &str) -> Buffer {
    let path = PathBuf::from("/w/oso/a.rs");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(decode_all(text.as_bytes(), TabWidth::default()));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    buffer.set_baseline(Baseline {
        location: path,
        file: Some(FileStamp {
            modified: std::time::SystemTime::UNIX_EPOCH,
            len: text.len() as u64,
            mode: 0o644,
            identity: None,
        }),
    });
    buffer.set_followed(true);
    buffer
}

/// The host's list number in these tests.
const LIST: u64 = 7;

/// An editor showing `a.rs`, wide enough for the float's preview, with a list of `kind` open.
fn listing(text: &str, kind: XrefKind) -> Editor {
    let mut editor = Editor::with_buffer(visiting(text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
        .open_xref_list(kind, LIST, PathBuf::from("/w/oso"))
        .unwrap();
    editor
}

fn row(shown: &str, path: &str, line: usize, at: DiagnosticRowAt, text: &str) -> XrefRow {
    XrefRow {
        shown: shown.into(),
        path: PathBuf::from(path),
        line,
        at,
        text: text.into(),
    }
}

fn file(column: VisitColumn) -> DiagnosticRowAt {
    DiagnosticRowAt::File { column }
}

/// The texts of the rows shown, in order.
fn shown(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

#[test]
fn the_list_opens_waiting_with_its_kind_s_label_one_at_a_time() {
    let mut editor = listing("x\n", XrefKind::Definitions);
    assert_eq!(editor.prompt().unwrap().label(), "Definitions:");
    assert!(editor.files_pending());
    assert_eq!(
        editor.open_xref_list(XrefKind::References, LIST + 1, PathBuf::from("/w/oso")),
        Err(PromptError::Invalid),
        "one at a time"
    );
    editor.quit();
    let editor = listing("x\n", XrefKind::References);
    assert_eq!(editor.prompt().unwrap().label(), "References:");
}

#[test]
fn rows_read_as_a_path_a_line_and_its_text_in_the_order_given_the_first_selected() {
    let mut editor = listing("x\n", XrefKind::References);
    let rows = vec![
        row(
            "src/b.rs",
            "/w/oso/src/b.rs",
            12,
            file(VisitColumn::Utf16(4)),
            "let x = y;",
        ),
        row(
            "src/b.rs",
            "/w/oso/src/b.rs",
            3,
            file(VisitColumn::Utf16(0)),
            "fn y() {}",
        ),
        row("~/.env", "/h/.env", 1, file(VisitColumn::Utf16(0)), ""),
    ];
    editor.xref_listed(XrefRows::new(LIST, rows, false));
    assert!(!editor.files_pending());
    assert_eq!(
        shown(&editor),
        [
            "src/b.rs:12: let x = y;",
            "src/b.rs:3: fn y() {}",
            "~/.env:1:"
        ]
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.selected(), Some(0));
    assert!(!format!("{:?}", XrefRows::new(LIST, Vec::new(), false)).contains("oso"));
}

#[test]
fn rows_of_another_list_or_after_it_closed_are_dropped() {
    let mut editor = listing("x\n", XrefKind::Definitions);
    let rows = || {
        vec![row(
            "a.rs",
            "/w/oso/a.rs",
            1,
            file(VisitColumn::Chars(0)),
            "x",
        )]
    };
    editor.xref_listed(XrefRows::new(LIST + 1, rows(), false));
    assert!(shown(&editor).is_empty() && editor.files_pending());
    editor.quit();
    editor.xref_listed(XrefRows::new(LIST, rows(), false));
    assert!(editor.prompt().is_none());
}

#[test]
fn a_cut_answer_says_so_and_what_was_typed_filters_the_rows() {
    let mut editor = listing("x\n", XrefKind::References);
    let id = editor.prompt().unwrap().buffer();
    editor.apply_edits(id, &[edit(0..0, "fn")]).unwrap();
    editor.match_completion();
    let rows = vec![
        row(
            "a.rs",
            "/w/oso/a.rs",
            1,
            file(VisitColumn::Chars(0)),
            "let a = 1;",
        ),
        row(
            "a.rs",
            "/w/oso/a.rs",
            2,
            file(VisitColumn::Chars(0)),
            "fn b() {}",
        ),
    ];
    editor.xref_listed(XrefRows::new(LIST, rows, true));
    assert_eq!(shown(&editor), ["a.rs:2: fn b() {}"]);
    assert!(editor.files_cut());
}

#[test]
fn a_row_of_an_open_buffer_previews_and_goes_where_its_edits_since_moved_it() {
    let mut editor = listing("let x = 1;\nlet y = 2;\n", XrefKind::Definitions);
    let buffer = editor.tiled().0.buffer();
    let revision = editor.buffer(buffer).unwrap().revision();
    let at = DiagnosticRowAt::Open {
        buffer,
        offset: 15,
        revision,
    };
    let rows = vec![
        row("a.rs", "/w/oso/a.rs", 2, at, "let y = 2;"),
        row("b.rs", "/w/oso/b.rs", 1, file(VisitColumn::Chars(0)), "y"),
    ];
    editor.xref_listed(XrefRows::new(LIST, rows, false));
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 15, .. })
    ));
    assert!(editor.take_preview_request().is_none(), "shown from memory");
    editor.apply_edits(buffer, &[edit(0..0, "//\n")]).unwrap();
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 18, .. })
    ));
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.take_xref_hit(),
        Some(DiagnosticHit::Buffer {
            id: buffer,
            offset: 18
        })
    );
    assert!(editor.take_xref_hit().is_none(), "once");
}

#[test]
fn a_file_under_the_root_is_read_from_it_and_one_outside_from_its_own_directory() {
    let mut editor = listing("x\n", XrefKind::Definitions);
    let rows = vec![
        row(
            "src/b.rs",
            "/w/oso/src/b.rs",
            7,
            file(VisitColumn::Utf16(3)),
            "b",
        ),
        row(
            "~/.cargo/registry/c.rs",
            "/h/.cargo/registry/c.rs",
            2,
            file(VisitColumn::Utf8(1)),
            "c",
        ),
    ];
    editor.xref_listed(XrefRows::new(LIST, rows, false));
    let read = editor.take_preview_request().expect("a read of src/b.rs");
    assert_eq!(
        (read.root.as_path(), &*read.path),
        (Path::new("/w/oso"), "src/b.rs")
    );
    assert_eq!(
        read.at,
        PreviewAt::Line {
            line: 7,
            column: VisitColumn::Utf16(3)
        }
    );
    editor.preview_read(read.generation, FilePreviewRead::Unreadable);
    editor.move_completion(1).unwrap();
    let read = editor.take_preview_request().expect("a read of c.rs");
    assert_eq!(
        (read.root.as_path(), &*read.path),
        (Path::new("/h/.cargo/registry"), "c.rs")
    );
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.take_xref_hit(),
        Some(DiagnosticHit::File {
            path: PathBuf::from("/h/.cargo/registry/c.rs"),
            line: 2,
            column: VisitColumn::Utf8(1)
        })
    );
}

#[test]
fn nothing_of_a_file_that_may_hold_secrets_is_asked_for() {
    let mut editor = listing("x\n", XrefKind::References);
    let rows = vec![row("~/.env", "/h/.env", 1, file(VisitColumn::Chars(0)), "")];
    editor.xref_listed(XrefRows::new(LIST, rows, false));
    assert!(editor.take_preview_request().is_none());
    assert!(matches!(editor.preview(), Some(Preview::Withheld)));
}
