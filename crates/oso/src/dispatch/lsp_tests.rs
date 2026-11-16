//! Scenario tests of the `lsp` minor mode's layer through the [`Dispatcher`] (ADR 0138, Keys in
//! buffers a server follows): on while a server follows the buffer, kept while it waits to
//! start again, gone once it stopped or no server follows, and under a prefix argument.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, ServerMark};
use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
use oso_core::editor::Editor;
use oso_core::key::KeySequence;

use super::Dispatcher;

/// A dispatcher and an editor whose buffer has errors at 4 and 10, and the mark `mark`.
fn followed(mark: Option<ServerMark>) -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", &"x".repeat(20)));
    let id = editor.window().buffer();
    let buffer = editor.buffer_mut(id).unwrap();
    buffer.set_followed(true);
    buffer.set_server(mark);
    let new = |at: usize| New {
        range: at..at + 1,
        severity: Severity::Error,
        tags: Tags::default(),
        source: "",
        code: "",
        message: "m",
    };
    let revision = buffer.revision();
    assert!(buffer.take_diagnostics(revision, Diagnostics::new("fake", [new(4), new(10)], 20)));
    (Dispatcher::new(), editor)
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

fn cursor(editor: &Editor) -> usize {
    editor.window().selections.primary().cursor
}

#[test]
fn c_m_n_and_c_m_p_move_while_a_server_follows_the_buffer() {
    for mark in [
        ServerMark::Ready,
        ServerMark::Starting,
        ServerMark::Working(Some(40)),
    ] {
        let (mut d, mut editor) = followed(Some(mark));
        send(&mut d, &mut editor, "C-M-n");
        assert_eq!(cursor(&editor), 4, "{mark:?}");
        send(&mut d, &mut editor, "C-M-n");
        assert_eq!(cursor(&editor), 10);
        send(&mut d, &mut editor, "C-M-p");
        assert_eq!(cursor(&editor), 4);
    }
}

#[test]
fn the_layer_goes_once_the_server_stopped_or_none_follows() {
    for mark in [Some(ServerMark::Stopped), None] {
        let (mut d, mut editor) = followed(mark);
        send(&mut d, &mut editor, "C-M-n");
        assert_eq!(cursor(&editor), 0, "{mark:?}");
    }
}

#[test]
fn a_prefix_argument_keeps_the_layer() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    send(&mut d, &mut editor, "C-u C-M-n");
    assert_eq!(cursor(&editor), 4);
}

#[test]
fn the_layer_of_c_x_tab_comes_before_the_lsp_layer_while_it_is_open() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    send(&mut d, &mut editor, "C-SPC C-e C-x TAB");
    assert!(editor.indent_rigidly_layer());
    let reading = d.keymaps.reading(&editor);
    let layers = d.keymaps.active(&reading, None, &editor);
    assert_eq!(layers.minor.len(), 2);
    assert!(
        std::ptr::eq(layers.minor[0], &d.keymaps.rigid),
        "C-x TAB's first"
    );
    assert!(std::ptr::eq(layers.minor[1], &d.keymaps.lsp));
}

#[test]
fn m_comma_goes_back_through_the_jumps_while_a_server_follows_and_walks_buffers_elsewhere() {
    use oso_core::editor::ServerRequest;
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    let followed_id = editor.window().buffer();
    // `M-.` asks the host, at the primary cursor.
    send(&mut d, &mut editor, "C-f C-f M-.");
    match editor.take_server_requests().as_slice() {
        [ServerRequest::Definition(at)] => {
            assert_eq!((at.buffer, at.offset, at.agent), (followed_id, 2, false));
        }
        other => panic!("{other:?}"),
    }
    // The host jumps; `M-,` and `C-M-,` walk the window's history.
    editor.xref_to_place(followed_id, 15);
    assert_eq!(cursor(&editor), 15);
    send(&mut d, &mut editor, "M-,");
    assert_eq!(cursor(&editor), 2);
    send(&mut d, &mut editor, "C-M-,");
    assert_eq!(cursor(&editor), 15);
    send(&mut d, &mut editor, "C-M-,");
    assert!(
        echo(&editor).contains("No later place"),
        "{}",
        echo(&editor)
    );
    // In a buffer no server follows, `M-,` is `previous-buffer`.
    let (mut d, mut editor) = followed(None);
    let first = editor.window().buffer();
    let second = editor.add_buffer(Buffer::from_text("u.rs", "y"));
    editor.show(second);
    send(&mut d, &mut editor, "M-,");
    assert_eq!(editor.window().buffer(), first);
}

fn echo(editor: &Editor) -> String {
    let mut status = oso_core::editor::Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

#[test]
fn m_g_d_lists_the_project_s_diagnostics_and_ret_goes_to_the_one_picked() {
    use oso_core::buffer::Baseline;
    use oso_core::editor::{DiagnosticRow, DiagnosticRowAt, DiagnosticRows};
    let path = std::path::PathBuf::from("/w/oso/t.rs");
    let mut buffer = Buffer::visiting(path.clone());
    buffer.apply_load(oso_core::load::decode_all(
        b"let x = 1;\nlet y = 2;\n",
        oso_core::display_map::TabWidth::default(),
    ));
    buffer.set_baseline(Baseline {
        location: path,
        file: None,
    });
    buffer.set_server(Some(ServerMark::Ready));
    let mut editor = Editor::with_buffer(buffer);
    let id = editor.window().buffer();
    let revision = editor.buffer(id).unwrap().revision();
    let mut d = Dispatcher::new();
    send(&mut d, &mut editor, "M-g d");
    assert_eq!(editor.prompt().unwrap().label(), "Diagnostics in oso:");
    let request = editor.take_diagnostics_request().unwrap();
    let row = DiagnosticRow {
        path: "t.rs".into(),
        line: 2,
        at: DiagnosticRowAt::Open {
            buffer: id,
            offset: 15,
            revision,
        },
        severity: Severity::Error,
        message: "m".into(),
        source: "".into(),
        code: "".into(),
    };
    editor.diagnostics_listed(DiagnosticRows::new(
        request.generation,
        vec![row],
        Some(0),
        false,
    ));
    send(&mut d, &mut editor, "RET");
    assert!(editor.prompt().is_none());
    assert_eq!(cursor(&editor), 15);
    // With no server, `M-g d` is not bound.
    let (mut d, mut editor) = followed(None);
    send(&mut d, &mut editor, "M-g d");
    assert!(editor.prompt().is_none());
}

#[test]
fn m_question_asks_for_references_and_ret_in_the_list_of_places_goes_to_the_row() {
    use oso_core::editor::{DiagnosticHit, DiagnosticRowAt, ServerRequest, XrefKind, XrefRow};
    use oso_core::editor::{VisitColumn, XrefRows};
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    let id = editor.window().buffer();
    send(&mut d, &mut editor, "C-f M-?");
    match editor.take_server_requests().as_slice() {
        [ServerRequest::References(at)] => {
            assert_eq!((at.buffer, at.offset, at.agent), (id, 1, false));
        }
        other => panic!("{other:?}"),
    }
    // The host opens the list once an answer has several places.
    let root = std::path::PathBuf::from("/w");
    assert!(d.list_xrefs(&mut editor, XrefKind::References, 3, root.clone()));
    assert!(
        !d.list_xrefs(&mut editor, XrefKind::References, 4, root),
        "not over a prompt"
    );
    let row = |line| XrefRow {
        shown: "b.rs".into(),
        path: "/w/b.rs".into(),
        line,
        at: DiagnosticRowAt::File {
            column: VisitColumn::Utf16(2),
        },
        text: "x".into(),
    };
    editor.xref_listed(XrefRows::new(3, vec![row(4), row(9)], false));
    send(&mut d, &mut editor, "C-n RET");
    assert!(editor.prompt().is_none());
    assert_eq!(
        editor.take_server_requests(),
        [ServerRequest::GoToXref],
        "lsp-go-to-xref asks the host"
    );
    assert_eq!(
        editor.take_xref_hit(),
        Some(DiagnosticHit::File {
            path: "/w/b.rs".into(),
            line: 9,
            column: VisitColumn::Utf16(2)
        })
    );
}

#[test]
fn m_x_lsp_go_to_xref_opens_nothing() {
    let (mut d, mut editor) = followed(Some(ServerMark::Ready));
    let name: Vec<String> = "lsp-go-to-xref".chars().map(String::from).collect();
    send(&mut d, &mut editor, &format!("M-x {} RET", name.join(" ")));
    assert!(editor.prompt().is_none());
    assert!(editor.take_server_requests().is_empty());
    assert!(echo(&editor).contains("M-. and M-?"), "{}", echo(&editor));
}
