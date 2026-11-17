//! Tests of a list's view (ADR 0138, step 15): the rows shown split into path, line and text,
//! the one selected, an open buffer's row named, the preview's lines of a buffer and of a file
//! read for it, a long line cut, and none for any other prompt.
//!
//! Must not depend on: threads or the file system.

use std::path::{Path, PathBuf};

use super::*;
use crate::buffer::{Baseline, Buffer, BufferId, FileStamp};
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
        .map(|row| (row.shown, row.line, row.text, row.buffer))
        .collect();
    assert_eq!(
        read,
        [
            ("a.go", 10, "x10 := f10()", Some(id)),
            ("a.go", 40, "x40 := f40()", Some(id)),
            ("~/b.go", 9, "", None),
        ]
    );
    assert_eq!(view.rows[2].path, Path::new("/h/b.go"));
    let ListedPreview::Rows { buffer, at, rows } = view.preview else {
        panic!("rows");
    };
    assert_eq!(buffer, Some(id));
    let at = at.expect("the row's line shows");
    assert_eq!(
        rows[at],
        (40, "\t\tx40 := f40()".to_owned()),
        "the selected row's line"
    );
    let (first, _) = rows[0];
    assert_eq!(rows[0].1, format!("\t\tx{first} := f{first}()"));
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
    let ListedPreview::Rows { buffer, at, rows } = view.preview else {
        panic!("rows");
    };
    assert_eq!(buffer, None);
    let at = at.expect("the row's line shows");
    assert_eq!(
        rows[at],
        (30, "\ty30".to_owned()),
        "numbered as in its file"
    );
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
        (row.shown, row.line, row.text),
        ("src/c.go", 7, "error: undefined: x")
    );
    assert_eq!(row.path, Path::new("/w/oso/src/c.go"));
}

/// An editor on `text` in a frame `columns` wide, with a list of references to the place at
/// `offset` on its line `line` open.
fn listing_at(text: &str, columns: usize, (line, offset): (usize, usize)) -> (Editor, BufferId) {
    let mut editor = editor_on(text);
    editor.set_frame_size(FrameSize { rows: 40, columns });
    let id = editor.tiled().0.buffer();
    let revision = editor.buffer(id).unwrap().revision();
    editor
        .open_xref_list(XrefKind::References, 5, PathBuf::from("/w/oso"))
        .unwrap();
    let row = XrefRow {
        shown: "a.go".into(),
        path: PathBuf::from("/w/oso/a.go"),
        line,
        at: ListedAt::Open {
            buffer: id,
            offset,
            revision,
        },
        text: "".into(),
    };
    editor.xref_listed(XrefRows::new(5, vec![row], false));
    (editor, id)
}

#[test]
fn a_long_row_of_the_preview_is_cut_at_a_character() {
    let long = format!(
        "{}é{}\n",
        "a".repeat(LISTED_LINE_BYTES - 1),
        "b".repeat(5000)
    );
    let text = format!("x\n{long}y\n");
    // Wide enough that the preview draws more than a row keeps.
    let (editor, _) = listing_at(&text, 1200, (2, 2));
    let ListedPreview::Rows { rows, at, .. } = editor.listed_view().unwrap().preview else {
        panic!("rows");
    };
    let (line, shown) = &rows[at.unwrap()];
    assert_eq!(*line, 2);
    assert_eq!(
        shown.len(),
        LISTED_LINE_BYTES - 1,
        "the `é` across the bound left out"
    );
    assert!(shown.bytes().all(|byte| byte == b'a'));
}

#[test]
fn a_place_deep_in_a_long_line_shows_as_the_preview_scrolls_to_it() {
    let long = format!("{}PLACE{}\n", "a".repeat(2000), "b".repeat(2000));
    let text = format!("x\n{long}y\n");
    let (editor, _) = listing_at(&text, 160, (2, 2 + 2000));
    let ListedPreview::Rows { rows, at, .. } = editor.listed_view().unwrap().preview else {
        panic!("rows");
    };
    let (line, shown) = &rows[at.unwrap()];
    assert_eq!(*line, 2);
    assert!(
        shown.contains("PLACE"),
        "the columns the preview draws: {shown}"
    );
    assert!(shown.len() < 200, "only those");
}

#[test]
fn a_line_the_preview_wraps_is_one_row_for_each_part_it_draws() {
    let long = format!("{}\n", "w".repeat(300));
    let text = format!("x\n{long}y\n");
    let (mut editor, id) = listing_at(&text, 160, (2, 2 + 250));
    editor.set_wraps(id, true);
    crate::editor::wrap_tests::settle(&mut editor);
    // As after each command, the preview's wrap at its own width.
    editor.fit_preview();
    let ListedPreview::Rows { rows, at, .. } = editor.listed_view().unwrap().preview else {
        panic!("rows");
    };
    let twos: Vec<_> = rows.iter().filter(|(line, _)| *line == 2).collect();
    assert!(twos.len() > 1, "line 2 wraps over several rows: {rows:?}");
    let joined: String = twos.iter().map(|(_, shown)| shown.as_str()).collect();
    // A float with one candidate has few rows: the parts it draws, each once.
    assert!(joined.len() <= 300 && joined.bytes().all(|byte| byte == b'w'));
    assert!(
        twos.iter().all(|(_, shown)| shown.len() < 300),
        "each one part"
    );
    assert_eq!(rows[at.unwrap()].0, 2);
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

#[test]
fn a_row_of_a_buffer_edited_since_still_names_it_without_mapping_its_place() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let id = editor.tiled().0.buffer();
    let revision = editor.buffer(id).unwrap().revision();
    editor
        .open_xref_list(XrefKind::References, 6, PathBuf::from("/w/oso"))
        .unwrap();
    let row = |line: usize| XrefRow {
        shown: "a.go".into(),
        path: PathBuf::from("/w/oso/a.go"),
        line,
        at: ListedAt::Open {
            buffer: id,
            offset: line_start(&text, line),
            revision,
        },
        text: "".into(),
    };
    editor.xref_listed(XrefRows::new(6, vec![row(10), row(20)], false));
    let replacement = crate::buffer::Replacement {
        range: 0..0,
        text: "// new\n",
    };
    editor.apply_edits(id, &[replacement]).unwrap();
    let view = editor.listed_view().unwrap();
    assert!(view.rows.iter().all(|row| row.buffer == Some(id)));
    // The selected row's preview still follows it through the edit, a line down.
    let ListedPreview::Rows { rows, at, .. } = view.preview else {
        panic!("rows");
    };
    assert_eq!(rows[at.unwrap()].0, 11);
}
