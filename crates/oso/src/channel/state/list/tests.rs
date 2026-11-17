//! Tests of a prompt's `list` in the agent's `state` (ADR 0138, step 15): a list of references
//! and of diagnostics as the person sees it, and what is withheld: everything while a buffer
//! that may hold secrets is shown, the text and preview of a row of a file that may hold
//! secrets or of a buffer withheld from agents, and a server's controls.
//!
//! Must not depend on: sockets, threads or `app`; these build editors in memory.

use std::path::PathBuf;

use oso_core::buffer::{Baseline, Buffer};
use oso_core::diagnostic::Severity;
use oso_core::editor::{
    DiagnosticRow, DiagnosticRows, Editor, Excerpt, FilePreviewRead, ListedAt, VisitColumn,
    XrefKind, XrefRow, XrefRows,
};
use oso_core::frame::FrameSize;
use serde_json::{Value, json};

use super::super::{STATE_VERSION, state};

/// Tab-indented lines `x2` to `x60`, as a Go file reads.
fn go_text() -> String {
    let mut text = String::from("package main\n");
    for n in 2..=60 {
        text.push_str(&format!("\t\tx{n} := f{n}()\n"));
    }
    text
}

/// An editor on `text` visiting `/w/a.go`, judged ordinary, wide enough for the float's
/// preview, so that `/w` is the directory its lists are of.
fn editor_on(text: &str) -> Editor {
    let path = PathBuf::from("/w/a.go");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(oso_core::load::decode_all(
        text.as_bytes(),
        oso_core::display_map::TabWidth::default(),
    ));
    buffer.judge_secret(Some(std::slice::from_ref(&path)));
    buffer.set_baseline(Baseline {
        location: path,
        file: None,
    });
    let mut editor = Editor::with_buffer(buffer);
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 160,
    });
    editor
}

/// A row of `/w/a.go`'s open buffer at line `line` of `text`.
fn open_row(editor: &Editor, text: &str, line: usize) -> XrefRow {
    let id = editor.tiled().0.buffer();
    let revision = editor.buffer(id).unwrap().revision();
    let offset: usize = text
        .split_inclusive('\n')
        .take(line - 1)
        .map(str::len)
        .sum();
    XrefRow {
        shown: "a.go".into(),
        path: PathBuf::from("/w/a.go"),
        line,
        at: ListedAt::Open {
            buffer: id,
            offset: offset + 2,
            revision,
        },
        text: format!("x{line} := f{line}()").into(),
    }
}

/// The references `rows` listed as list 3, the row `selected` selected.
fn references(editor: &mut Editor, rows: Vec<XrefRow>, selected: isize) {
    editor
        .open_xref_list(XrefKind::References, 3, PathBuf::from("/w"))
        .unwrap();
    editor.xref_listed(XrefRows::new(3, rows, false));
    editor.move_completion(selected).unwrap();
}

/// The innermost prompt's `list` in the state, with `gopls` the list's server.
fn list(editor: &Editor) -> Value {
    let state = state(editor, None, 0, (&|_| None, Some("gopls")));
    assert_eq!(state["version"], STATE_VERSION);
    state["prompts"][0]["list"].clone()
}

#[test]
fn a_list_of_references_shows_its_rows_the_selected_one_and_its_preview_s_lines() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let rows = vec![open_row(&editor, &text, 10), open_row(&editor, &text, 40)];
    references(&mut editor, rows, 1);
    let list = list(&editor);
    assert_eq!(list["of"], "references");
    assert_eq!(list["server"], "gopls");
    assert_eq!(
        (list["count"].clone(), list["total"].clone()),
        (json!(2), json!(2))
    );
    assert_eq!(
        (list["cut"].clone(), list["pending"].clone()),
        (json!(false), json!(false))
    );
    assert_eq!(list["selected"], 1);
    assert_eq!(
        list["rows"],
        json!([
            { "path": "a.go", "line": 10, "text": "x10 := f10()" },
            { "path": "a.go", "line": 40, "text": "x40 := f40()" },
        ])
    );
    let preview = &list["preview"];
    assert_eq!(preview["kind"], "buffer");
    let at = preview["at"].as_u64().unwrap() as usize;
    assert_eq!(
        preview["rows"][at],
        json!({ "line": 40, "text": "\t\tx40 := f40()" }),
        "the selected row's line, where the person sees it"
    );
    assert_eq!(STATE_VERSION, 4);
}

#[test]
fn nothing_of_a_list_shows_while_a_buffer_that_may_hold_secrets_is_shown() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let path = PathBuf::from("/tmp/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.apply_load(oso_core::load::decode_all(
        b"KEY=1\n",
        oso_core::display_map::TabWidth::default(),
    ));
    secret.judge_secret(Some(&[path]));
    let secret = editor.add_buffer(secret);
    editor.show(secret);
    let rows = vec![open_row(&editor, &text, 10)];
    references(&mut editor, rows, 0);
    assert_eq!(list(&editor), Value::Null);
}

#[test]
fn a_row_of_a_file_that_may_hold_secrets_or_of_a_withheld_buffer_gives_its_place_only() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let env = XrefRow {
        shown: ".env".into(),
        path: PathBuf::from("/w/.env"),
        line: 2,
        at: ListedAt::File {
            column: VisitColumn::Utf16(0),
        },
        text: "".into(),
    };
    let rows = vec![open_row(&editor, &text, 10), env];
    references(&mut editor, rows, 1);
    let list_now = list(&editor);
    assert_eq!(
        list_now["rows"][1],
        json!({ "path": ".env", "line": 2, "text": null })
    );
    assert_eq!(list_now["preview"], json!({ "kind": "withheld" }));
    // The open buffer, withheld from agents once the list is open.
    let id = editor.tiled().0.buffer();
    editor.buffer_mut(id).unwrap().withhold_from_agents();
    editor.move_completion(-1).unwrap();
    let list_now = list(&editor);
    assert_eq!(
        list_now["rows"][0],
        json!({ "path": "a.go", "line": 10, "text": null })
    );
    assert_eq!(list_now["preview"], json!({ "kind": "withheld" }));
}

#[test]
fn a_list_of_diagnostics_names_no_server_and_writes_a_message_s_controls_out() {
    let mut editor = editor_on(&go_text());
    editor.open_diagnostics_list("Diagnostics").unwrap();
    let request = editor.take_diagnostics_request().unwrap();
    let row = DiagnosticRow {
        path: "a.go".into(),
        line: 7,
        at: ListedAt::File {
            column: VisitColumn::Utf16(0),
        },
        severity: Severity::Error,
        message: "undefined: x\u{202e}y".into(),
        source: "compiler".into(),
        code: "".into(),
    };
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        vec![row],
        Some(0),
        false,
    ));
    let list = list(&editor);
    assert_eq!(list["of"], "diagnostics");
    assert_eq!(list["server"], Value::Null);
    assert_eq!(list["rows"][0]["text"], "error: undefined: x\\u{202e}y");
}

/// A row of `/w/b.go`, which no list opened, at line `line`.
fn file_row(line: usize) -> XrefRow {
    XrefRow {
        shown: "b.go".into(),
        path: PathBuf::from("/w/b.go"),
        line,
        at: ListedAt::File {
            column: VisitColumn::Utf16(1),
        },
        text: format!("y{line}").into(),
    }
}

#[test]
fn a_file_s_row_shows_the_rows_read_for_it_numbered_as_in_its_file() {
    let mut editor = editor_on(&go_text());
    references(&mut editor, vec![file_row(30)], 0);
    let read = editor.take_preview_request().expect("a read of b.go");
    let bytes: String = (25..=35).map(|n| format!("\ty{n}\n")).collect();
    let excerpt = Excerpt::new(bytes.as_bytes(), 25, (30, VisitColumn::Utf16(1)), None);
    editor.preview_read(read.generation, FilePreviewRead::Text(excerpt));
    let list = list(&editor);
    assert_eq!(
        list["rows"][0],
        json!({ "path": "b.go", "line": 30, "text": "y30" })
    );
    let preview = &list["preview"];
    assert_eq!(preview["kind"], "file");
    let at = preview["at"].as_u64().unwrap() as usize;
    assert_eq!(preview["rows"][at], json!({ "line": 30, "text": "\ty30" }));
}

#[test]
fn a_file_s_row_whose_file_a_withheld_buffer_holds_gives_its_place_only() {
    let mut editor = editor_on(&go_text());
    let path = PathBuf::from("/w/b.go");
    let mut held = Buffer::visiting(path.clone());
    held.apply_load(oso_core::load::decode_all(
        b"y1\n",
        oso_core::display_map::TabWidth::default(),
    ));
    held.judge_secret(Some(std::slice::from_ref(&path)));
    let held = editor.add_buffer(held);
    editor.buffer_mut(held).unwrap().withhold_from_agents();
    references(&mut editor, vec![file_row(1)], 0);
    let list = list(&editor);
    assert_eq!(
        list["rows"][0],
        json!({ "path": "b.go", "line": 1, "text": null })
    );
    assert_eq!(list["preview"], json!({ "kind": "withheld" }));
}

#[test]
fn what_the_person_typed_is_in_no_field_of_the_list() {
    let text = go_text();
    let mut editor = editor_on(&text);
    let rows = vec![open_row(&editor, &text, 10), open_row(&editor, &text, 40)];
    references(&mut editor, rows, 0);
    // Typed in the list's input, which is matched again after each command.
    let input = editor.prompt().unwrap().buffer();
    let typed = oso_core::buffer::Replacement {
        range: 0..0,
        text: "zq4",
    };
    editor.apply_edits(input, &[typed]).unwrap();
    editor.match_completion();
    let state = state(&editor, None, 0, (&|_| None, Some("gopls")));
    let list = state["prompts"][0]["list"].to_string();
    assert!(!list.contains("zq4"), "{list}");
    assert_eq!(
        state["prompts"][0]["list"]["count"], 0,
        "the rows follow it"
    );
}
