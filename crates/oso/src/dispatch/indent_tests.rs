//! Scenario tests of the indentation commands with their keys, through the [`Dispatcher`]
//! (ADR 0037, ADR 0115): `TAB`, `RET`, `C-j`, a typed closer, `electric-indent-mode`, and
//! what each undoes as.
//!
//! Must not depend on: `app`, winit, the renderer or the GPU; these run on every OS with no
//! window.

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::{Args, Invocation};
use oso_core::editor::Editor;
use oso_core::key::KeySequence;
use oso_core::selection::{Selection, SelectionSet};
use oso_core::window::{Scroll, ViewSize};

use super::Dispatcher;

/// A dispatcher and an editor showing `marked` in `mode`, with a cursor at each `|`, which is
/// taken out.
fn at(mode: Mode, marked: &str) -> (Dispatcher, Editor) {
    let text = marked.replace('|', "");
    let mut buffer = Buffer::from_bytes("test", text.as_bytes());
    assert!(buffer.set_mode(mode));
    let mut editor = Editor::with_buffer(buffer);
    editor.set_view_size(ViewSize::new(10, 40));
    let mut cursors = Vec::new();
    for (index, _) in marked.match_indices('|') {
        cursors.push(Selection::at(index - cursors.len()));
    }
    let set = SelectionSet::from_parts(cursors, 0).unwrap();
    editor.restore_positions(set, Scroll::default());
    (Dispatcher::new(), editor)
}

fn send(dispatcher: &mut Dispatcher, editor: &mut Editor, keys: &str) {
    let keys: KeySequence = keys.parse().unwrap();
    dispatcher.handle_keys(editor, keys.0);
}

/// Runs the command `name`, with `count` as a prefix argument if any.
fn run(dispatcher: &mut Dispatcher, editor: &mut Editor, name: &str, count: Option<isize>) {
    let id = dispatcher.registry.lookup(name).unwrap();
    let args = count.map_or(Args::with_prefix(None), Args::numeric);
    let step = super::Step::MayJoin;
    let (_, result) = dispatcher.execute(editor, id, args, Invocation::default(), step);
    result.unwrap();
}

/// The text with `|` at each cursor.
fn marked(editor: &Editor) -> String {
    let mut text = editor.current().1.text().to_string();
    let selections: Vec<_> = editor
        .window()
        .selections
        .iter()
        .map(|s| s.cursor)
        .collect();
    for at in selections.into_iter().rev() {
        text.insert(at, '|');
    }
    text
}

#[test]
fn tab_indents_by_mode_and_fundamental_keeps_tab_stops() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "fn f() {\n|x\n}");
    send(&mut dispatcher, &mut editor, "TAB");
    assert_eq!(marked(&editor), "fn f() {\n    |x\n}");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "a|");
    send(&mut dispatcher, &mut editor, "TAB");
    assert_eq!(marked(&editor), "a       |");
}

#[test]
fn ret_indents_and_c_j_does_not_while_electric_indent_is_on() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "fn f() {|}");
    send(&mut dispatcher, &mut editor, "RET");
    assert_eq!(marked(&editor), "fn f() {\n    |\n}");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(
        marked(&editor),
        "fn f() {|}",
        "RET with its indentation undoes as one step"
    );
    send(&mut dispatcher, &mut editor, "C-j");
    assert_eq!(marked(&editor), "fn f() {\n|}");
}

#[test]
fn electric_indent_mode_swaps_ret_and_c_j() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "fn f() {|");
    run(&mut dispatcher, &mut editor, "electric-indent-mode", None);
    assert!(!editor.electric_indent());
    send(&mut dispatcher, &mut editor, "RET");
    assert_eq!(marked(&editor), "fn f() {\n|");
    send(&mut dispatcher, &mut editor, "C-j");
    assert_eq!(marked(&editor), "fn f() {\n\n    |");
    // A positive argument turns it on, as a minor mode's does.
    run(
        &mut dispatcher,
        &mut editor,
        "electric-indent-mode",
        Some(1),
    );
    assert!(editor.electric_indent());
    run(
        &mut dispatcher,
        &mut editor,
        "electric-indent-mode",
        Some(1),
    );
    assert!(editor.electric_indent());
}

#[test]
fn a_typed_closer_goes_under_its_opener_and_undoes_with_the_typing() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "fn f() {\n    x\n    |");
    send(&mut dispatcher, &mut editor, "}");
    assert_eq!(marked(&editor), "fn f() {\n    x\n}|");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(marked(&editor), "fn f() {\n    x\n    |");
}

#[test]
fn ret_at_several_cursors_is_one_edit() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "foo(|a, |b)");
    send(&mut dispatcher, &mut editor, "RET");
    assert_eq!(marked(&editor), "foo(\n    |a,\n    |b)");
}

#[test]
fn a_mode_command_gives_the_buffer_its_modes_indentation() {
    use oso_core::indent::{Settings, Style};

    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "fn f() {\n|x\n}");
    let two = Settings::new(2, Style::Spaces, 8).unwrap();
    let modes = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental];
    editor.set_mode_indents(modes.map(|mode| {
        (
            mode,
            if mode == Mode::Rust {
                two
            } else {
                Settings::default()
            },
        )
    }));
    run(&mut dispatcher, &mut editor, "rust-mode", None);
    assert_eq!(editor.current().1.indent_settings(), two);
    send(&mut dispatcher, &mut editor, "TAB");
    assert_eq!(marked(&editor), "fn f() {\n  |x\n}");
}

#[test]
fn newline_and_indent_indents_whether_electric_indent_is_on_or_not() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "fn f() {|");
    editor.set_electric_indent(false);
    run(&mut dispatcher, &mut editor, "newline-and-indent", None);
    assert_eq!(marked(&editor), "fn f() {\n    |");
    editor.set_electric_indent(true);
    run(&mut dispatcher, &mut editor, "newline-and-indent", Some(2));
    assert_eq!(marked(&editor), "fn f() {\n\n\n    |");
}

#[test]
fn c_j_answers_a_prompt_and_adds_a_line_break_to_a_search() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\nb\nc");
    send(&mut dispatcher, &mut editor, "M-g g 3 C-j");
    assert_eq!(marked(&editor), "a\nb\n|c", "C-j answered the prompt");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\nb");
    send(&mut dispatcher, &mut editor, "C-s a C-j");
    assert!(editor.isearch().is_some(), "the search goes on");
    assert_eq!(marked(&editor), "a\n|b", "it found a line break after a");
}
