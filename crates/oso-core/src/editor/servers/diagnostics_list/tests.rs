//! Tests of the list of a project's diagnostics (ADR 0138, step 6b): the request, the rows as
//! they read, the row selected, a stale list dropped, the preview and `RET` of a row of an open
//! buffer and of a file no buffer holds.
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

/// An editor showing `a.rs`, wide enough for the float's preview, with the list open, and its
/// request.
fn listing(text: &str) -> (Editor, DiagnosticsRequest) {
    let mut editor = Editor::with_buffer(visiting(text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor.open_diagnostics_list("Diagnostics").unwrap();
    let request = editor.take_diagnostics_request().unwrap();
    (editor, request)
}

fn row(path: &str, line: usize, at: ListedAt, message: &str) -> DiagnosticRow {
    DiagnosticRow {
        path: path.into(),
        line,
        at,
        severity: Severity::Error,
        message: message.into(),
        source: "rustc".into(),
        code: "E0308".into(),
    }
}

fn file(column: VisitColumn) -> ListedAt {
    ListedAt::File { column }
}

/// The texts of the rows shown, in order.
fn shown(editor: &Editor) -> Vec<String> {
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    (0..candidates.len())
        .map(|place| candidates.get(place).unwrap().text().to_owned())
        .collect()
}

#[test]
fn the_list_opens_waiting_over_the_project_and_asks_for_its_rows_once() {
    let (mut editor, request) = listing("fn main() {}\n");
    assert_eq!(request.root, Path::new("/w/oso"));
    assert_eq!(request.buffer, editor.tiled().0.buffer());
    assert_eq!(request.cursor, 0);
    assert!(!format!("{request:?}").contains("oso"));
    assert!(editor.take_diagnostics_request().is_none(), "once");
    assert_eq!(editor.prompt().unwrap().label(), "Diagnostics in oso:");
    assert!(editor.files_pending());
    assert_eq!(
        editor.open_diagnostics_list("Diagnostics"),
        Err(PromptError::Invalid),
        "one at a time"
    );
}

#[test]
fn rows_read_as_a_path_a_line_a_severity_and_the_first_line_with_blanks_as_one() {
    let (mut editor, request) = listing("fn main() {}\n");
    let mut warning = row(
        "src/b.rs",
        12,
        file(VisitColumn::Utf16(4)),
        "unused  \t variable\nsecond line",
    );
    warning.severity = Severity::Warning;
    warning.code = "".into();
    let rows = vec![
        row("a.rs", 1, file(VisitColumn::Chars(0)), "mismatched types"),
        warning,
    ];
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        rows,
        Some(1),
        false,
    ));
    assert!(!editor.files_pending());
    assert_eq!(
        shown(&editor),
        [
            "a.rs:1: error: mismatched types",
            "src/b.rs:12: warning: unused variable"
        ]
    );
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.get(1).unwrap().annotation(), "rustc");
    assert_eq!(candidates.get(0).unwrap().annotation(), "rustc E0308");
    assert_eq!(candidates.selected(), Some(1), "the row the host picked");
}

#[test]
fn rows_of_an_older_list_or_after_it_closed_are_dropped() {
    let (mut editor, request) = listing("x\n");
    let rows = || vec![row("a.rs", 1, file(VisitColumn::Chars(0)), "m")];
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation + 1,
        rows(),
        None,
        false,
    ));
    assert!(shown(&editor).is_empty() && editor.files_pending());
    editor.quit();
    editor.diagnostics_listed(DiagnosticRows::new(request.generation, rows(), None, false));
    assert!(editor.prompt().is_none());
}

#[test]
fn what_was_typed_while_the_rows_came_filters_them() {
    let (mut editor, request) = listing("x\n");
    let id = editor.prompt().unwrap().buffer();
    editor.apply_edits(id, &[edit(0..0, "unused")]).unwrap();
    editor.match_completion();
    let rows = vec![
        row("a.rs", 1, file(VisitColumn::Chars(0)), "mismatched types"),
        row("a.rs", 2, file(VisitColumn::Chars(0)), "unused import"),
    ];
    editor.diagnostics_listed(DiagnosticRows::new(request.generation, rows, Some(0), true));
    assert_eq!(shown(&editor), ["a.rs:2: error: unused import"]);
    assert!(editor.files_cut());
}

#[test]
fn a_row_of_an_open_buffer_previews_and_goes_where_its_edits_since_moved_it() {
    let (mut editor, request) = listing("let x = 1;\nlet y = 2;\n");
    let buffer = request.buffer;
    let revision = editor.buffer(buffer).unwrap().revision();
    let at = ListedAt::Open {
        buffer,
        offset: 15,
        revision,
    };
    let rows = vec![row("a.rs", 2, at, "m")];
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        rows,
        Some(0),
        false,
    ));
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 15, .. })
    ));
    assert!(editor.take_preview_request().is_none(), "shown from memory");
    // Text typed before it, as an agent may type while the list is open.
    editor.apply_edits(buffer, &[edit(0..0, "//\n")]).unwrap();
    assert!(matches!(
        editor.preview(),
        Some(Preview::Buffer { cursor: 18, .. })
    ));
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.take_diagnostic_hit(),
        Some(ListedHit::Buffer {
            id: buffer,
            offset: 18
        })
    );
    assert!(editor.take_diagnostic_hit().is_none(), "once");
}

#[test]
fn a_row_of_a_file_no_buffer_holds_is_read_by_its_line_and_goes_there() {
    let (mut editor, request) = listing("x\n");
    let rows = vec![row("src/b.rs", 7, file(VisitColumn::Utf16(3)), "m")];
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        rows,
        Some(0),
        false,
    ));
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
    assert!(editor.take_preview_request().is_none(), "once per row");
    editor.preview_read(read.generation, FilePreviewRead::Unreadable);
    assert!(
        editor.preview().is_none(),
        "nothing shows of what could not be read"
    );
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.take_diagnostic_hit(),
        Some(ListedHit::File {
            path: PathBuf::from("/w/oso/src/b.rs"),
            line: 7,
            column: VisitColumn::Utf16(3)
        })
    );
}

#[test]
fn a_padded_source_and_code_read_with_their_blanks_as_one() {
    let (mut editor, request) = listing("x\n");
    let mut padded = row("a.rs", 1, file(VisitColumn::Chars(0)), "m");
    padded.source = "  rust\tc   ".into();
    padded.code = " E0308 \n".into();
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        vec![padded],
        None,
        false,
    ));
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    assert_eq!(candidates.get(0).unwrap().annotation(), "rust c E0308");
}

#[test]
fn an_open_buffer_s_row_goes_by_its_line_once_its_log_no_longer_reaches_the_list() {
    let (mut editor, request) = listing("let x = 1;\nlet y = 2;\n");
    let buffer = request.buffer;
    let revision = editor.buffer(buffer).unwrap().revision();
    let at = ListedAt::Open {
        buffer,
        offset: 15,
        revision,
    };
    let rows = vec![row("a.rs", 2, at, "m")];
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        rows,
        Some(0),
        false,
    ));
    // More edits than its log keeps, so the list's revision is out of its reach.
    for _ in 0..70 {
        editor.apply_edits(buffer, &[edit(0..0, "/")]).unwrap();
    }
    assert!(
        matches!(editor.preview(), Some(Preview::Buffer { cursor: 81, .. })),
        "line 2's start, by its line: {:?}",
        editor.preview()
    );
    editor.submit_prompt().unwrap();
    assert_eq!(
        editor.take_diagnostic_hit(),
        Some(ListedHit::File {
            path: PathBuf::from("/w/oso/a.rs"),
            line: 2,
            column: VisitColumn::Chars(0)
        })
    );
}
