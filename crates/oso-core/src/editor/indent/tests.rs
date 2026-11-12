//! Tests of indentation at the cursors (ADR 0115): `TAB` reindenting each cursor's line,
//! `RET` with the new line's indentation and between brackets, typed closers, several
//! cursors, undo, tabs, and the buffers with no rules.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::super::edit_tests::{cursors, set_at, text};
use crate::buffer::{Buffer, BufferId, EditError, Mode};
use crate::editor::Editor;
use crate::indent::{Settings, Style};

const LIMIT: usize = 16 << 20;

/// An editor showing `text` in `mode`, with cursors at the `|` marks, which are taken out.
fn editor_in(mode: Mode, marked: &str) -> (Editor, BufferId) {
    let text = marked.replace('|', "");
    let mut at = Vec::new();
    for (index, _) in marked.match_indices('|') {
        at.push(index - at.len());
    }
    let mut buffer = Buffer::from_text("t", &text);
    assert!(buffer.set_mode(mode));
    let mut editor = Editor::with_buffer(buffer);
    editor.windows[0].selections = set_at(&at);
    let id = editor.window().buffer();
    (editor, id)
}

/// The text with `|` where every cursor is.
fn marked(editor: &Editor, id: BufferId) -> String {
    let mut text = text(editor, id);
    for at in cursors(&editor.windows[0]).into_iter().rev() {
        text.insert(at, '|');
    }
    text
}

/// Runs one command's worth of `f`, as the dispatcher does, in an undo group.
fn command<T>(editor: &mut Editor, f: impl FnOnce(&mut Editor) -> T) -> T {
    editor.begin_undo_group();
    let result = f(editor);
    editor.end_undo_group();
    result
}

fn ret(editor: &mut Editor) {
    let done = command(editor, |editor| editor.newline("\n", 1, true, LIMIT));
    assert_eq!(done, Ok(true));
}

#[test]
fn tab_reindents_each_cursors_line_once() {
    let cases = [
        ("fn f() {\n|x\n}\n", "fn f() {\n    |x\n}\n"),
        // A cursor in the text keeps its place in it.
        ("fn f() {\n  ab|c\n}\n", "fn f() {\n    ab|c\n}\n"),
        // One in the indentation goes to its end, also on a line already right.
        ("fn f() {\n |       x\n}\n", "fn f() {\n    |x\n}\n"),
        ("fn f() {\n  |  x\n}\n", "fn f() {\n    |x\n}\n"),
        // Two cursors on one line reindent it once.
        ("fn f() {\n|x|y\n}\n", "fn f() {\n    |x|y\n}\n"),
        ("fn f() {\n|a\n|        }\n", "fn f() {\n    |a\n|}\n"),
    ];
    for (start, expected) in cases {
        let (mut editor, id) = editor_in(Mode::Rust, start);
        command(&mut editor, Editor::indent_for_tab).unwrap();
        assert_eq!(marked(&editor, id), expected, "{start:?}");
    }
}

#[test]
fn tab_in_fundamental_mode_goes_to_the_next_tab_stop() {
    let (mut editor, id) = editor_in(Mode::Fundamental, "  a\n|b");
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), "  a\n        |b");
}

#[test]
fn ret_indents_the_new_line_and_trims_what_it_leaves() {
    let cases = [
        ("fn f() {|", "fn f() {\n    |"),
        (
            "fn f() {\n    let x =   |   y;\n}",
            "fn f() {\n    let x =\n        |y;\n}",
        ),
        // Between brackets: the closer on its own line, the cursor on one between.
        ("fn f() {|}", "fn f() {\n    |\n}"),
        (
            "fn f() {\n    g(|  )\n}",
            "fn f() {\n    g(\n        |\n    )\n}",
        ),
        // Inside a string, nothing around the cursor goes.
        (
            "fn f() {\n    \"a  |  b\"\n}",
            "fn f() {\n    \"a  \n|  b\"\n}",
        ),
        // A line left holding only indentation becomes empty.
        ("fn f() {\n    |\n}", "fn f() {\n\n    |\n}"),
        // Several cursors: each line follows what the ones before leave.
        ("foo(|a, |b)", "foo(\n    |a,\n    |b)"),
    ];
    for (start, expected) in cases {
        let (mut editor, id) = editor_in(Mode::Rust, start);
        ret(&mut editor);
        assert_eq!(marked(&editor, id), expected, "{start:?}");
    }
}

#[test]
fn ret_in_markdown_and_toml_follows_their_rules() {
    let (mut editor, id) = editor_in(Mode::Markdown, "- item|");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "- item\n  |");
    // Two spaces at a line's end break it in Markdown: they stay.
    let (mut editor, id) = editor_in(Mode::Markdown, "a  |");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "a  \n|");
    let (mut editor, id) = editor_in(Mode::Toml, "x = [|]");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "x = [\n    |\n]");
    // Markdown has no line between brackets.
    let (mut editor, id) = editor_in(Mode::Markdown, "[|]");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "[\n|]");
}

#[test]
fn ret_without_indentation_inserts_the_breaks_alone() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {  |  }");
    let done = command(&mut editor, |editor| {
        editor.newline("\r\n", 2, false, LIMIT)
    });
    assert_eq!(done, Ok(true));
    assert_eq!(marked(&editor, id), "fn f() {  \r\n\r\n|  }");
}

#[test]
fn a_count_indents_only_the_last_new_line() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {|");
    let done = command(&mut editor, |editor| editor.newline("\n", 3, true, LIMIT));
    assert_eq!(done, Ok(true));
    assert_eq!(marked(&editor, id), "fn f() {\n\n\n    |");
}

#[test]
fn ret_with_its_indentation_is_one_undo_step() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {|}");
    ret(&mut editor);
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {|}");
}

#[test]
fn past_the_limit_nothing_changes() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {|");
    let done = command(&mut editor, |editor| editor.newline("\n", 1, true, 3));
    assert_eq!(done, Ok(false));
    assert_eq!(marked(&editor, id), "fn f() {|");
}

#[test]
fn a_shown_region_is_replaced_first() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {xyz|}");
    let mut selection = crate::selection::Selection::at(11);
    selection.mark = Some(8);
    editor.windows[0].selections =
        crate::selection::SelectionSet::from_parts(vec![selection], 0).unwrap();
    editor.windows[0]
        .selections
        .set_mark_active(crate::selection::MarkActive::Active);
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "fn f() {\n    |\n}");
}

#[test]
fn a_typed_closer_first_on_its_line_goes_under_its_opener() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    x\n        }|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n    x\n}|");
    // Not first on its line: nothing moves.
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    a(x)|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n    a(x)|");
    // Off, or in Markdown, nothing moves either.
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n        }|");
    editor.set_electric_indent(false);
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n        }|");
    let (mut editor, id) = editor_in(Mode::Markdown, "- a\n   )|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "- a\n   )|");
}

#[test]
fn tabs_style_indents_with_tabs_then_spaces() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    g(\n|x");
    let settings = Settings::new(4, Style::Tabs, 8).unwrap();
    editor.buffer_mut(id).unwrap().set_indent_settings(settings);
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n    g(\n\t|x");
}

#[test]
fn a_read_only_buffer_is_refused() {
    let mut editor = Editor::with_buffer(Buffer::from_bytes("bad", b"a\xff"));
    assert!(matches!(
        command(&mut editor, |editor| editor.newline("\n", 1, true, LIMIT)),
        Err(EditError::ReadOnly(_))
    ));
    assert!(matches!(
        command(&mut editor, Editor::indent_for_tab),
        Err(EditError::ReadOnly(_))
    ));
}

#[test]
fn each_mode_has_its_configured_indentation_or_the_default() {
    let (mut editor, _) = editor_in(Mode::Rust, "|");
    assert_eq!(editor.mode_indent(Mode::Rust), Settings::default());
    let two = Settings::new(2, Style::Spaces, 8).unwrap();
    let mut indents = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental]
        .map(|mode| (mode, Settings::default()));
    indents[1].1 = two;
    editor.set_mode_indents(indents);
    assert_eq!(editor.mode_indent(Mode::Markdown), two);
    assert_eq!(editor.mode_indent(Mode::Rust), Settings::default());
    assert_eq!(editor.mode_indent(Mode::CopyList), Settings::default());
}

#[test]
fn tab_in_markdown_keeps_the_tab_stops_until_its_cycle_comes() {
    let (mut editor, id) = editor_in(Mode::Markdown, "- a\n|b");
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), "- a\n        |b");
}

#[test]
fn a_new_mode_table_reaches_the_open_buffers_of_the_person() {
    let (mut editor, id) = editor_in(Mode::Rust, "|");
    let two = Settings::new(2, Style::Tabs, 8).unwrap();
    let modes = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental];
    editor.set_mode_indents(modes.map(|mode| (mode, two)));
    assert_eq!(editor.buffer(id).unwrap().indent_settings(), two);
}

#[test]
fn cursors_in_one_run_of_blanks_each_trim_their_own_side() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    a; |  |b\n}");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "fn f() {\n    a;\n    |\n    |b\n}");
    // Two cursors between the same brackets.
    let (mut editor, id) = editor_in(Mode::Rust, "g(|)\nh(|)");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "g(\n    |\n)\nh(\n    |\n)");
}

#[test]
fn ret_between_brackets_with_cr_lf_and_after_wide_characters() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {|}");
    let done = command(&mut editor, |editor| editor.newline("\r\n", 1, true, LIMIT));
    assert_eq!(done, Ok(true));
    assert_eq!(marked(&editor, id), "fn f() {\r\n    |\r\n}");
    let (mut editor, id) = editor_in(Mode::Rust, "f(\"é\", é|)");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "f(\"é\", é\n|)");
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    é}|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n    é}|");
}

#[test]
fn a_blank_a_combining_mark_is_on_stays() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    a;| \u{301}x\n}");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "fn f() {\n    a;\n    | \u{301}x\n}");
}

#[test]
fn past_the_limit_a_shown_region_stays_too() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {xyz|}");
    let mut selection = crate::selection::Selection::at(11);
    selection.mark = Some(8);
    editor.windows[0].selections =
        crate::selection::SelectionSet::from_parts(vec![selection], 0).unwrap();
    editor.windows[0]
        .selections
        .set_mark_active(crate::selection::MarkActive::Active);
    let done = command(&mut editor, |editor| editor.newline("\n", 1, true, 100));
    assert_eq!(done, Ok(false));
    assert_eq!(text(&editor, id), "fn f() {xyz}");
}

#[test]
fn lines_with_more_blanks_than_are_read_are_left_as_they_are() {
    let blanks = " ".repeat(5000);
    let start = format!("fn f() {{\n{blanks}|x\n}}");
    let (mut editor, id) = editor_in(Mode::Rust, &start);
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(text(&editor, id), start.replace('|', ""));
    let (mut editor, _) = editor_in(Mode::Rust, &format!("fn f() {{{blanks}|{blanks}}}"));
    ret(&mut editor);
}

#[test]
fn tab_in_a_string_leaves_the_cursor_where_it_is() {
    let (mut editor, id) = editor_in(Mode::Rust, "fn f() {\n    let s = \"\n  | x\";\n}");
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), "fn f() {\n    let s = \"\n  | x\";\n}");
}

proptest::proptest! {
    #[test]
    fn random_text_and_cursors_never_panic_and_cursors_stay_in_the_text(
        pieces in proptest::collection::vec(
            proptest::sample::select(&["{", "}", "(", ")", "\"", " ", "\t", "\n", "\r\n", "é", "\u{301}", "x", "//", "/*"][..]),
            0..120,
        ),
        at in proptest::collection::vec(0..400usize, 1..8),
        count in 1..3usize,
    ) {
        let text: String = pieces.concat();
        let mut at: Vec<usize> = at.into_iter().map(|at| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        }).collect();
        at.sort_unstable();
        at.dedup();
        for mode in [Mode::Rust, Mode::Toml, Mode::Markdown, Mode::Fundamental] {
            let mut buffer = Buffer::from_text("t", &text);
            assert!(buffer.set_mode(mode));
            let mut editor = Editor::with_buffer(buffer);
            editor.windows[0].selections = set_at(&at);
            let _ = command(&mut editor, |editor| editor.newline("\n", count, true, LIMIT));
            let _ = command(&mut editor, Editor::indent_for_tab);
            let _ = command(&mut editor, Editor::indent_typed_closers);
            let id = editor.window().buffer();
            let len = editor.buffer(id).unwrap().text().len();
            for cursor in cursors(&editor.windows[0]) {
                proptest::prop_assert!(cursor <= len);
            }
        }
    }
}
