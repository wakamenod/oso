//! Tests of a list's view (ADR 0138, step 15): the rows shown split into path, line and text,
//! the one selected, an open buffer's row named, the preview's lines of a buffer and of a file
//! read for it, a long line cut, and none for any other prompt.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};

use super::*;
use crate::buffer::{Baseline, Buffer, FileStamp};
use crate::diagnostic::Severity;
use crate::display_map::TabWidth;
use crate::editor::{
    DiagnosticRow, DiagnosticRows, Excerpt, FilePreviewRead, ListedAt, VisitColumn, XrefKind,
    XrefRow, XrefRows,
};
use crate::frame::FrameSize;
use crate::load::decode_all;

/// A buffer visiting `/w/oso/a.go`, holding `text`, judged ordinary, followed by a server.
fn visiting(text: &str) -> Buffer {
    let path = PathBuf::from("/w/oso/a.go");
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

/// An editor showing `a.go`, wide enough for the float's preview.
fn editor_on(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(visiting(text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
}

/// Tab-indented lines `x1` to `x60`, as a Go file reads.
fn go_text() -> String {
    let mut text = String::from("package main\n");
    for n in 2..=60 {
        text.push_str(&format!("\t\tx{n} := f{n}()\n"));
    }
    text
}

/// The offset of the line `line`, from 1, of `text`.
fn line_start(text: &str, line: usize) -> usize {
    text.split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum()
}

#[test]
fn a_list_of_references_gives_its_rows_split_and_the_selected_one_s_lines() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let id = editor.tiled().0.buffer();
    let revision = editor.buffer(id).unwrap().revision();
    editor
        .open_xref_list(XrefKind::References, 3, PathBuf::from("/w/oso"))
        .unwrap();
    let pending = editor.listed_view().unwrap();
    assert!(pending.pending && pending.rows.is_empty());
    let open = |line: usize| XrefRow {
        shown: "a.go".into(),
        path: PathBuf::from("/w/oso/a.go"),
        line,
        at: ListedAt::Open {
            buffer: id,
            offset: line_start(&text, line) + 2,
            revision,
        },
        text: format!("x{line} := f{line}()").into(),
    };
    let elsewhere = XrefRow {
        shown: "~/b.go".into(),
        path: PathBuf::from("/h/b.go"),
        line: 9,
        at: ListedAt::File {
            column: VisitColumn::Utf16(1),
        },
        text: "".into(),
    };
    let rows = vec![open(10), open(40), elsewhere];
    editor.xref_listed(XrefRows::new(3, rows, false));
    editor.move_completion(1).unwrap();
    let view = editor.listed_view().unwrap();
    assert_eq!(view.of, ListedOf::References);
    assert_eq!(
        (view.count, view.total, view.cut, view.pending),
        (3, 3, false, false)
    );
    assert_eq!(view.selected, Some(1));
    let read: Vec<_> = (view.rows.iter())
        .map(|row| (row.path, row.line, row.text, row.buffer))
        .collect();
    assert_eq!(
        read,
        [
            ("a.go", 10, "x10 := f10()", Some(id)),
            ("a.go", 40, "x40 := f40()", Some(id)),
            ("~/b.go", 9, "", None),
        ]
    );
    assert_eq!(view.rows[2].file, Path::new("/h/b.go"));
    let ListedPreview::Lines {
        buffer,
        first_line,
        at,
        lines,
    } = view.preview
    else {
        panic!("lines");
    };
    assert_eq!(buffer, Some(id));
    let at = at.expect("the row's line shows");
    assert_eq!(first_line + at, 40, "the selected row's line");
    assert_eq!(lines[at], "\t\tx40 := f40()");
    assert_eq!(lines[0], format!("\t\tx{first_line} := f{first_line}()"));
}

#[test]
fn a_file_s_row_gives_the_lines_read_for_it_numbered_as_in_its_file() {
    let mut editor = editor_on("x\n");
    editor
        .open_xref_list(XrefKind::Definitions, 4, PathBuf::from("/w/oso"))
        .unwrap();
    let row = XrefRow {
        shown: "b.go".into(),
        path: PathBuf::from("/w/oso/b.go"),
        line: 30,
        at: ListedAt::File {
            column: VisitColumn::Utf16(2),
        },
        text: "y30".into(),
    };
    editor.xref_listed(XrefRows::new(4, vec![row], false));
    let read = editor.take_preview_request().expect("a read of b.go");
    // What the reader reads: lines 25 to 35, the row's the sixth.
    let bytes: String = (25..=35).map(|n| format!("\ty{n}\n")).collect();
    let excerpt = Excerpt::new(bytes.as_bytes(), 25, (30, VisitColumn::Utf16(2)), None);
    editor.preview_read(read.generation, FilePreviewRead::Text(excerpt));
    let view = editor.listed_view().unwrap();
    assert_eq!(view.of, ListedOf::Definitions);
    assert_eq!(view.rows[0].buffer, None);
    let ListedPreview::Lines {
        buffer,
        first_line,
        at,
        lines,
    } = view.preview
    else {
        panic!("lines");
    };
    assert_eq!(buffer, None);
    let at = at.expect("the row's line shows");
    assert_eq!(first_line + at, 30);
    assert_eq!(lines[at], "\ty30");
}

#[test]
fn a_list_of_diagnostics_gives_each_row_s_severity_and_message_as_its_text() {
    let text = go_text();
    let mut editor = editor_on(&text);
    editor.open_diagnostics_list("Diagnostics").unwrap();
    let request = editor.take_diagnostics_request().unwrap();
    let row = DiagnosticRow {
        path: "src/c.go".into(),
        line: 7,
        at: ListedAt::File {
            column: VisitColumn::Utf16(0),
        },
        severity: Severity::Error,
        message: "undefined: x".into(),
        source: "compiler".into(),
        code: "".into(),
    };
    let rows = DiagnosticRows::new(request.generation, vec![row], Some(0), true);
    editor.diagnostics_listed(rows);
    let view = editor.listed_view().unwrap();
    assert_eq!(view.of, ListedOf::Diagnostics);
    assert!(view.cut);
    assert_eq!(view.selected, Some(0));
    let row = &view.rows[0];
    assert_eq!(
        (row.path, row.line, row.text),
        ("src/c.go", 7, "error: undefined: x")
    );
    assert_eq!(row.file, Path::new("/w/oso/src/c.go"));
}

#[test]
fn a_long_line_of_the_preview_is_cut_at_a_character() {
    let long = format!(
        "{}é{}\n",
        "a".repeat(LISTED_LINE_BYTES - 1),
        "b".repeat(5000)
    );
    let text = format!("x\n{long}y\n");
    let mut editor = editor_on(&text);
    let id = editor.tiled().0.buffer();
    let revision = editor.buffer(id).unwrap().revision();
    editor
        .open_xref_list(XrefKind::References, 5, PathBuf::from("/w/oso"))
        .unwrap();
    let row = XrefRow {
        shown: "a.go".into(),
        path: PathBuf::from("/w/oso/a.go"),
        line: 2,
        at: ListedAt::Open {
            buffer: id,
            offset: 2,
            revision,
        },
        text: "".into(),
    };
    editor.xref_listed(XrefRows::new(5, vec![row], false));
    let ListedPreview::Lines { lines, at, .. } = editor.listed_view().unwrap().preview else {
        panic!("lines");
    };
    let line = &lines[at.unwrap()];
    assert_eq!(
        line.len(),
        LISTED_LINE_BYTES - 1,
        "the `é` across the bound left out"
    );
    assert!(line.bytes().all(|byte| byte == b'a'));
}

#[test]
fn any_other_prompt_has_no_view() {
    let mut editor = editor_on("x\n");
    assert!(editor.listed_view().is_none());
    editor
        .open_completion_as(
            "Pick:",
            "pick",
            std::sync::Arc::from([]),
            Default::default(),
        )
        .unwrap();
    assert!(editor.listed_view().is_none());
}
