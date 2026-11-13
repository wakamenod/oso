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
use super::motion_tests::drain;

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
            oso_core::indent::Origins::default(),
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
    drain(&mut editor);
    assert!(editor.isearch().is_some(), "the search goes on");
    assert_eq!(marked(&editor), "a\n|b", "it found a line break after a");
}

#[test]
fn a_line_break_in_a_search_string_goes_in_its_history_and_comes_back() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\nb\na\nc");
    send(&mut dispatcher, &mut editor, "C-s a C-j");
    drain(&mut editor);
    send(&mut dispatcher, &mut editor, "RET");
    assert!(editor.isearch().is_none());
    assert_eq!(marked(&editor), "a\n|b\na\nc");
    send(&mut dispatcher, &mut editor, "C-s M-p");
    assert_eq!(editor.isearch().map(|search| search.string()), Some("a\n"));
    send(&mut dispatcher, &mut editor, "C-s");
    drain(&mut editor);
    assert_eq!(
        marked(&editor),
        "a\nb\na\n|c",
        "the string recalled finds the next one"
    );
}

fn echo(editor: &Editor) -> String {
    let mut status = oso_core::editor::Status::default();
    editor.status_into(editor.tiled().0.id(), &mut status);
    status.echo
}

fn region_shown(editor: &Editor) -> bool {
    editor.window().selections.mark_active().is_active()
}

#[test]
fn c_m_backslash_reindents_the_region_as_one_step() {
    let start = "|fn f() {\nx;\n      y;\n   \n}";
    let (mut dispatcher, mut editor) = at(Mode::Rust, start);
    send(&mut dispatcher, &mut editor, "C-SPC M-> C-M-\\");
    assert_eq!(marked(&editor), "fn f() {\n    x;\n    y;\n\n}|");
    assert!(!region_shown(&editor));
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(
        editor.current().1.text().to_string(),
        start.replace('|', "")
    );
    // With a count, every line goes to that column.
    send(&mut dispatcher, &mut editor, "M-< C-SPC M-> C-u 2 C-M-\\");
    assert_eq!(
        editor.current().1.text().to_string(),
        "  fn f() {\n  x;\n  y;\n\n  }"
    );
}

#[test]
fn indent_region_signals_with_no_mark_or_no_rules() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "|a");
    send(&mut dispatcher, &mut editor, "C-M-\\");
    assert_eq!(echo(&editor), "The mark is not set now");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\n b");
    send(&mut dispatcher, &mut editor, "C-SPC M-> C-M-\\");
    assert_eq!(echo(&editor), "No indentation rules in this mode");
}

#[test]
fn tab_with_a_region_shown_indents_it_in_a_language_mode_only() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "|fn f() {\nx;\ny;\n}");
    send(&mut dispatcher, &mut editor, "C-SPC M-> TAB");
    assert_eq!(marked(&editor), "fn f() {\n    x;\n    y;\n}|");
    // Markdown cycles the cursor's line; Fundamental inserts spaces at it.
    let (mut dispatcher, mut editor) = at(Mode::Markdown, "- a\n|b");
    send(&mut dispatcher, &mut editor, "C-SPC C-e TAB");
    assert_eq!(marked(&editor), "- a\n  b|");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|ab");
    send(&mut dispatcher, &mut editor, "C-SPC C-f TAB");
    assert_eq!(marked(&editor), "a       |b");
}

#[test]
fn c_x_tab_opens_a_layer_whose_arrows_shift_the_region_as_one_step() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "x\n|a\n b\n");
    send(&mut dispatcher, &mut editor, "C-SPC M-> C-x TAB");
    assert!(editor.indent_rigidly_layer());
    assert_eq!(
        echo(&editor),
        "Indent region with <left>, <right>, S-<left>, or S-<right>."
    );
    send(
        &mut dispatcher,
        &mut editor,
        "<right> <right> <left> S-<right>",
    );
    assert_eq!(editor.current().1.text().to_string(), "x\n    a\n     b\n");
    assert!(region_shown(&editor), "the region stays shown");
    send(&mut dispatcher, &mut editor, "S-<left> S-<left>");
    assert_eq!(editor.current().1.text().to_string(), "x\na\n b\n");
    send(&mut dispatcher, &mut editor, "<right>");
    // Another key closes the layer and does what it always does.
    send(&mut dispatcher, &mut editor, "C-g");
    assert!(!editor.indent_rigidly_layer());
    assert!(!region_shown(&editor));
    send(&mut dispatcher, &mut editor, "<right>");
    assert_eq!(editor.current().1.text().to_string(), "x\n a\n  b\n");
    send(&mut dispatcher, &mut editor, "C-z");
    assert_eq!(
        editor.current().1.text().to_string(),
        "x\na\n b\n",
        "the layer's keys undo as one step"
    );
}

#[test]
fn a_key_outside_the_layer_runs_as_usual() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|ab\ncd");
    send(
        &mut dispatcher,
        &mut editor,
        "C-SPC C-n C-x TAB <right> C-a",
    );
    assert!(!editor.indent_rigidly_layer());
    // Leaving the layer hides the region, so typing then replaces nothing.
    assert!(!region_shown(&editor));
    // The region ends at the second line's start, so only the first shifted.
    assert_eq!(marked(&editor), " ab\n|cd");
    send(&mut dispatcher, &mut editor, "<right>");
    assert_eq!(marked(&editor), " ab\nc|d");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|ab\ncd\nef");
    send(
        &mut dispatcher,
        &mut editor,
        "C-SPC C-n C-n C-x TAB <right> C-e x",
    );
    assert_eq!(marked(&editor), " ab\n cd\nefx|");
}

#[test]
fn c_x_tab_with_a_count_shifts_by_it() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|  a\n    b");
    send(&mut dispatcher, &mut editor, "C-SPC M-> C-u 3 C-x TAB");
    assert!(!editor.indent_rigidly_layer());
    assert_eq!(editor.current().1.text().to_string(), "     a\n       b");
    send(&mut dispatcher, &mut editor, "C-x C-x M-- 6 C-x TAB");
    assert_eq!(editor.current().1.text().to_string(), "a\n b");
}

#[test]
fn the_agent_channels_indent_rigidly_shifts_by_the_indent_size() {
    use super::args::Given;
    use oso_core::command::Reach;

    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\nb");
    send(&mut dispatcher, &mut editor, "C-SPC M->");
    let ran =
        dispatcher.run_for_agent(&mut editor, "indent-rigidly", Given::Nothing, Reach::Change);
    assert_eq!(ran.unwrap().1, Ok(()));
    assert!(!editor.indent_rigidly_layer());
    assert_eq!(editor.current().1.text().to_string(), "    a\n    b");
}

#[test]
fn the_named_shifts_run_as_commands() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|  a\n b");
    send(&mut dispatcher, &mut editor, "C-SPC M->");
    let steps = [
        ("indent-rigidly-right", "   a\n  b"),
        ("indent-rigidly-left", "  a\n b"),
        ("indent-rigidly-right-to-tab-stop", "     a\n    b"),
        ("indent-rigidly-left-to-tab-stop", " a\nb"),
    ];
    for (name, expected) in steps {
        run(&mut dispatcher, &mut editor, name, None);
        assert_eq!(editor.current().1.text().to_string(), expected, "{name}");
    }
}

#[test]
fn m_m_m_i_and_m_caret() {
    let (mut dispatcher, mut editor) = at(Mode::Rust, "  \tab|c");
    send(&mut dispatcher, &mut editor, "M-m");
    assert_eq!(marked(&editor), "  \t|abc");
    send(&mut dispatcher, &mut editor, "M-i");
    assert_eq!(marked(&editor), "  \t        |abc");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "a\n  |b\nc");
    send(&mut dispatcher, &mut editor, "M-^");
    assert_eq!(marked(&editor), "a| b\nc");
    send(&mut dispatcher, &mut editor, "C-u M-^");
    assert_eq!(marked(&editor), "a b| c");
}

#[test]
fn m_ret_starts_a_list_item_in_markdown_only() {
    let (mut dispatcher, mut editor) = at(Mode::Markdown, "1. a|");
    send(&mut dispatcher, &mut editor, "M-RET");
    assert_eq!(marked(&editor), "1. a\n2. |");
    let (mut dispatcher, mut editor) = at(Mode::Markdown, "a|");
    send(&mut dispatcher, &mut editor, "M-RET");
    assert_eq!(echo(&editor), "Not in a list");
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "- a|");
    send(&mut dispatcher, &mut editor, "M-RET");
    assert_eq!(echo(&editor), "M-RET is undefined");
}

#[test]
fn a_prefix_argument_before_an_arrow_still_moves() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|abcdef\nx");
    send(&mut dispatcher, &mut editor, "C-SPC C-n C-p C-u <right>");
    assert_eq!(marked(&editor), "abcd|ef\nx");
    send(&mut dispatcher, &mut editor, "M-- <right> C-u S-<left>");
    assert_eq!(marked(&editor), "|abcdef\nx");
    // With the layer open too: the prefix argument closes it, and the arrow moves.
    send(
        &mut dispatcher,
        &mut editor,
        "C-SPC C-n C-x TAB C-u <right>",
    );
    assert!(!editor.indent_rigidly_layer());
    assert_eq!(marked(&editor), "abcdef\nx|");
    assert_eq!(editor.current().1.text().to_string(), "abcdef\nx");
}

#[test]
fn a_layer_left_in_another_buffer_shifts_nothing_there() {
    let (mut dispatcher, mut editor) = at(Mode::Fundamental, "|a\nb");
    let other = editor.add_buffer(Buffer::from_text("other", "c\nd"));
    send(&mut dispatcher, &mut editor, "C-SPC M-> C-x TAB <right>");
    assert_eq!(editor.current().1.text().to_string(), " a\n b");
    // Another buffer is shown, as the agent's `switch-to-buffer` or a load does, outside any
    // command, while the person's layer is open.
    editor.show(other);
    assert!(!editor.indent_rigidly_layer());
    send(&mut dispatcher, &mut editor, "<right>");
    assert_eq!(editor.current().1.text().to_string(), "c\nd");
}
