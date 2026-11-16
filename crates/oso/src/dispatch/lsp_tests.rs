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
