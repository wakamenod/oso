//! Tests of the region commands (ADR 0115): `indent-region` by the rules and to a column,
//! `indent-rigidly` by columns and to a tab stop, the lines a region covers, its bounds, and
//! the layer of `C-x TAB` joining its keys into one undo step.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::super::tests::{LIMIT, command};
use super::{MAX_CHANGED_LINES, REGION_LIMIT, Shift};
use crate::buffer::{Buffer, BufferId, EditError, Mode, ReadOnly};
use crate::editor::{Editor, IndentError};
use crate::indent::{Settings, Style};
use crate::selection::{MarkActive, Selection, SelectionSet};

/// An editor showing `marked` in `mode`, with a region from each `[` to the `|` after it,
/// or from each `|` to the `]` after it, the marks taken out, and the regions shown if `shown`.
fn regions(mode: Mode, marked: &str, shown: bool) -> (Editor, BufferId) {
    let mut text = String::new();
    let mut parts = Vec::new();
    let mut open: Option<(usize, char)> = None;
    for c in marked.chars() {
        match (c, open) {
            ('[' | '|', None) => open = Some((text.len(), c)),
            ('|', Some((mark, '['))) | (']', Some((mark, '|'))) => {
                let (cursor, mark) = if c == '|' {
                    (text.len(), mark)
                } else {
                    (mark, text.len())
                };
                parts.push(Selection {
                    cursor,
                    mark: Some(mark),
                    goal: None,
                });
                open = None;
            }
            _ => text.push(c),
        }
    }
    let mut buffer = Buffer::from_text("t", &text);
    assert!(buffer.set_mode(mode));
    let mut editor = Editor::with_buffer(buffer);
    let mut set = SelectionSet::from_parts(parts, 0).unwrap();
    if shown {
        set.set_mark_active(MarkActive::Active);
    }
    editor.windows[0].selections = set;
    let id = editor.window().buffer();
    (editor, id)
}

fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

fn region(editor: &mut Editor, column: Option<usize>) -> Result<(), IndentError> {
    command(editor, |editor| editor.indent_region(column, LIMIT))
}

fn rigidly(editor: &mut Editor, columns: isize) -> Result<(), IndentError> {
    command(editor, |editor| editor.indent_rigidly(columns, LIMIT))
}

#[test]
fn indent_region_reindents_every_line_and_empties_blank_ones() {
    let (mut editor, id) = regions(
        Mode::Rust,
        "[fn f() {\nlet x = 1;\n   \n        if x {\ny();\n }\n}|\n",
        false,
    );
    region(&mut editor, None).unwrap();
    assert_eq!(
        text(&editor, id),
        "fn f() {\n    let x = 1;\n\n    if x {\n        y();\n    }\n}\n"
    );
    // One undo step brings it all back.
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(
        text(&editor, id),
        "fn f() {\nlet x = 1;\n   \n        if x {\ny();\n }\n}\n"
    );
}

#[test]
fn indent_region_takes_the_lines_that_start_before_the_regions_end() {
    let cases = [
        // A region that ends at a line's start leaves that line.
        ("fn f() {\n[a\n|b\n}\n", "fn f() {\n    a\nb\n}\n"),
        // One that starts inside a line takes it.
        ("fn f() {\na;[\nb;|\n}\n", "fn f() {\n    a;\n    b;\n}\n"),
        // An empty region has none, as in Emacs.
        ("fn f() {\na;[|\nb;\n}\n", "fn f() {\na;\nb;\n}\n"),
        ("fn f() {\n[|a\nb\n}\n", "fn f() {\na\nb\n}\n"),
        // A cursor before its mark.
        ("fn f() {\n|a;\nb;]\n}\n", "fn f() {\n    a;\n    b;\n}\n"),
    ];
    for (start, expected) in cases {
        let (mut editor, id) = regions(Mode::Rust, start, false);
        region(&mut editor, None).unwrap();
        assert_eq!(text(&editor, id), expected, "{start:?}");
    }
}

#[test]
fn a_blank_line_inside_a_string_keeps_its_spaces() {
    let start = "[fn f() {\nlet s = r\"\n   \n\";\n  \n}\n|";
    let (mut editor, id) = regions(Mode::Rust, start, false);
    region(&mut editor, None).unwrap();
    assert_eq!(
        text(&editor, id),
        "fn f() {\n    let s = r\"\n   \n\";\n\n}\n"
    );
}

#[test]
fn a_shift_or_a_column_leaves_the_line_a_region_starts_inside() {
    let (mut editor, id) = regions(Mode::Fundamental, "a[b\nc\nd|\n", false);
    rigidly(&mut editor, 1).unwrap();
    assert_eq!(text(&editor, id), "ab\n c\n d\n");
    let (mut editor, id) = regions(Mode::Fundamental, "a[b\nc|\n", false);
    region(&mut editor, Some(2)).unwrap();
    assert_eq!(text(&editor, id), "ab\n  c\n");
}

#[test]
fn tab_indentation_already_right_is_left_and_mixed_is_redone() {
    // A level is a tab.
    let tabs = Settings::new(8, Style::Tabs, 8).unwrap();
    let cases = [
        ("[fn f() {\n\tif x {\n\t\ty();\n\t}\n}\n|", None),
        (
            "[fn f() {\n        if x {\n\t\ty();\n\t}\n}\n|",
            Some("fn f() {\n\tif x {\n\t\ty();\n\t}\n}\n"),
        ),
        (
            "[fn f() {\n  \t  if x {\n\t\ty();\n\t}\n}\n|",
            Some("fn f() {\n\tif x {\n\t\ty();\n\t}\n}\n"),
        ),
    ];
    for (start, expected) in cases {
        let (mut editor, id) = regions(Mode::Rust, start, false);
        editor.buffer_mut(id).unwrap().set_indent_settings(tabs);
        let revision = editor.buffer(id).unwrap().revision();
        let before = text(&editor, id);
        region(&mut editor, None).unwrap();
        match expected {
            None => assert_eq!(editor.buffer(id).unwrap().revision(), revision, "{start:?}"),
            Some(expected) => assert_eq!(text(&editor, id), expected, "{before:?}"),
        }
    }
}

#[test]
fn a_blank_a_combining_mark_is_on_stays_its_base() {
    let (mut editor, id) = regions(Mode::Fundamental, "[  \u{301}a\n\t \u{301}b|", false);
    region(&mut editor, Some(0)).unwrap();
    assert_eq!(text(&editor, id), " \u{301}a\n \u{301}b");
    let (mut editor, id) = regions(Mode::Fundamental, "[   \u{301}a|", false);
    rigidly(&mut editor, 2).unwrap();
    assert_eq!(text(&editor, id), "     \u{301}a");
}

#[test]
fn several_cursors_regions_share_one_pass_and_one_edit() {
    let (mut editor, id) = regions(Mode::Rust, "fn f() {\n[a;\n|b;\n[c;|\nd;\n|e;]\n}\n", false);
    let revision = editor.buffer(id).unwrap().revision();
    region(&mut editor, None).unwrap();
    assert_eq!(
        text(&editor, id),
        "fn f() {\n    a;\nb;\n    c;\nd;\n    e;\n}\n"
    );
    assert_eq!(editor.buffer(id).unwrap().revision(), revision + 1);
}

#[test]
fn overlapping_regions_reindent_their_lines_once() {
    let (mut editor, id) = regions(Mode::Rust, "fn f() {\n  a;\n  b;\n}\n[|", false);
    let overlapping = |cursor, mark| Selection {
        cursor,
        mark: Some(mark),
        goal: None,
    };
    let set = SelectionSet::from_parts(vec![overlapping(11, 9), overlapping(16, 10)], 0);
    editor.windows[0].selections = set.unwrap();
    region(&mut editor, None).unwrap();
    assert_eq!(text(&editor, id), "fn f() {\n    a;\n    b;\n}\n");
}

#[test]
fn lines_already_right_are_not_touched() {
    let start = "[fn f() {\n    a\n}\n|";
    let (mut editor, id) = regions(Mode::Rust, start, false);
    let revision = editor.buffer(id).unwrap().revision();
    region(&mut editor, None).unwrap();
    assert_eq!(editor.buffer(id).unwrap().revision(), revision);
}

#[test]
fn a_count_puts_every_line_at_its_column_in_any_mode() {
    for mode in [Mode::Rust, Mode::Fundamental, Mode::Markdown] {
        let (mut editor, id) = regions(mode, "[a\n      b\n  \n\tc|\n", false);
        region(&mut editor, Some(2)).unwrap();
        assert_eq!(text(&editor, id), "  a\n  b\n\n  c\n", "{mode:?}");
    }
    let (mut editor, id) = regions(Mode::Fundamental, "[a|\n", false);
    region(&mut editor, Some(usize::MAX)).unwrap();
    assert_eq!(text(&editor, id).len(), crate::indent::MAX_COLUMN + 2);
}

#[test]
fn without_rules_or_a_mark_nothing_changes() {
    for mode in [Mode::Fundamental, Mode::Markdown] {
        let (mut editor, _) = regions(mode, "[a\n b|\n", false);
        assert_eq!(region(&mut editor, None), Err(IndentError::NoRules));
    }
    let (mut editor, _) = regions(Mode::Rust, "[a\n b|\n", false);
    editor.windows[0].selections = SelectionSet::new(0);
    assert_eq!(region(&mut editor, None), Err(IndentError::NoMark));
    assert_eq!(rigidly(&mut editor, 1), Err(IndentError::NoMark));
    let mut editor = Editor::with_buffer(Buffer::from_bytes("bad", b"a\xff"));
    let region = Selection {
        cursor: 0,
        mark: Some(2),
        goal: None,
    };
    let set = SelectionSet::from_parts(vec![region], 0);
    editor.windows[0].selections = set.unwrap();
    assert_eq!(
        rigidly(&mut editor, 1),
        Err(IndentError::Edit(EditError::ReadOnly(
            ReadOnly::InvalidUtf8
        )))
    );
}

#[test]
fn a_region_past_the_limit_is_refused_whole() {
    let line = "x\n";
    let lines = REGION_LIMIT / (line.len() + crate::indent::LINE_COST) + 1;
    let start = format!("[{}|", line.repeat(lines));
    let (mut editor, id) = regions(Mode::Fundamental, &start, false);
    let revision = editor.buffer(id).unwrap().revision();
    assert_eq!(region(&mut editor, Some(1)), Err(IndentError::TooLarge));
    assert_eq!(rigidly(&mut editor, 1), Err(IndentError::TooLarge));
    assert_eq!(editor.buffer(id).unwrap().revision(), revision);
}

#[test]
fn more_lines_to_change_than_one_edit_takes_is_refused_whole() {
    for (lines, done) in [
        (MAX_CHANGED_LINES, Ok(())),
        (MAX_CHANGED_LINES + 1, Err(IndentError::TooLarge)),
    ] {
        // Lines already right do not count.
        let start = format!("[fn f() {{\n    a;\n{}}}\n|", "x;\n".repeat(lines));
        let (mut editor, id) = regions(Mode::Rust, &start, false);
        let revision = editor.buffer(id).unwrap().revision();
        assert_eq!(region(&mut editor, None), done, "{lines}");
        let changed = editor.buffer(id).unwrap().revision() != revision;
        assert_eq!(changed, done.is_ok(), "{lines}");
    }
}

#[test]
fn more_than_the_limit_to_insert_changes_nothing() {
    let (mut editor, id) = regions(Mode::Fundamental, "[a\nb\nc|", false);
    let done = command(&mut editor, |editor| editor.indent_rigidly(4, 11));
    assert_eq!(done, Err(IndentError::TooMuchText));
    assert_eq!(text(&editor, id), "a\nb\nc");
    let done = command(&mut editor, |editor| editor.indent_rigidly(4, 12));
    assert_eq!(done, Ok(()));
}

#[test]
fn indent_rigidly_shifts_each_line_never_past_column_zero() {
    let start = "[    a\n  b\n   \n\tc|\n";
    let cases = [
        (2, "      a\n    b\n\n          c\n"),
        (-3, " a\nb\n\n     c\n"),
        (-100, "a\nb\n\nc\n"),
    ];
    for (columns, expected) in cases {
        let (mut editor, id) = regions(Mode::Fundamental, start, false);
        rigidly(&mut editor, columns).unwrap();
        assert_eq!(text(&editor, id), expected, "{columns}");
    }
}

#[test]
fn shifted_lines_take_the_buffers_style() {
    let (mut editor, id) = regions(Mode::Fundamental, "[      a\n  b|\n", false);
    let tabs = Settings::new(4, Style::Tabs, 8).unwrap();
    editor.buffer_mut(id).unwrap().set_indent_settings(tabs);
    rigidly(&mut editor, 4).unwrap();
    assert_eq!(text(&editor, id), "\t  a\n      b\n");
}

#[test]
fn to_a_tab_stop_moves_the_least_indented_line_of_each_region() {
    let start = "[  a\n      b|\nc\n[     d\n         e|\n";
    let (mut editor, id) = regions(Mode::Fundamental, start, false);
    to_stop(&mut editor, Shift::Right);
    assert_eq!(
        text(&editor, id),
        "    a\n        b\nc\n        d\n            e\n"
    );
    to_stop(&mut editor, Shift::Left);
    assert_eq!(text(&editor, id), "a\n    b\nc\n    d\n        e\n");
    // A region already at column 0 stays.
    to_stop(&mut editor, Shift::Left);
    assert_eq!(text(&editor, id), "a\n    b\nc\nd\n    e\n");
}

#[test]
fn regions_that_only_touch_go_to_their_own_tab_stops() {
    let (mut editor, id) = regions(Mode::Fundamental, "[a\nb\n|[  c\n  d\n|", false);
    to_stop(&mut editor, Shift::Right);
    assert_eq!(text(&editor, id), "    a\n    b\n    c\n    d\n");
}

#[test]
fn the_layers_keys_keep_the_region_and_undo_as_one_step() {
    let (mut editor, id) = regions(Mode::Fundamental, "x\n[a\nb|\n", true);
    command(&mut editor, |e| e.indent_rigidly(1, LIMIT)).unwrap();
    // A shift with no layer hides the region, as any edit does.
    assert_eq!(text(&editor, id), "x\n a\n b\n");
    assert!(!editor.window().selections.mark_active().is_active());

    let (mut editor, id) = regions(Mode::Fundamental, "x\n[a\nb|\n", true);
    let before = editor.window().selections.clone();
    command(&mut editor, |e| e.set_indent_rigidly_layer(true));
    assert!(editor.indent_rigidly_layer());
    for _ in 0..30 {
        rigidly(&mut editor, 1).unwrap();
        assert!(editor.window().selections.mark_active().is_active());
    }
    to_stop(&mut editor, Shift::Left);
    editor.set_indent_rigidly_layer(false);
    assert!(!editor.indent_rigidly_layer());
    assert!(
        !editor.window().selections.mark_active().is_active(),
        "closing hides the region"
    );
    let shifted = format!("x\n{0}a\n{0}b\n", " ".repeat(28));
    assert_eq!(text(&editor, id), shifted);
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), "x\na\nb\n");
    assert_eq!(cursors_of(&editor), cursors_in(&before));
}

fn to_stop(editor: &mut Editor, shift: Shift) {
    let done = command(editor, |editor| {
        editor.indent_rigidly_to_tab_stop(shift, LIMIT)
    });
    assert_eq!(done, Ok(()));
}

fn cursors_of(editor: &Editor) -> Vec<usize> {
    cursors_in(&editor.window().selections)
}

fn cursors_in(set: &SelectionSet) -> Vec<usize> {
    set.iter().map(|selection| selection.cursor).collect()
}

/// `text` with every space, tab and, if `breaks`, line break taken out.
fn without_blanks(text: &str, breaks: bool) -> String {
    text.chars()
        .filter(|&c| !(matches!(c, ' ' | '\t') || breaks && matches!(c, '\n' | '\r')))
        .collect()
}

proptest::proptest! {
    #[test]
    fn region_commands_change_only_blanks_and_never_panic(
        pieces in proptest::collection::vec(
            proptest::sample::select(&[
                "{", "}", "(", ")", "\"", " ", "\t", "\n", "\r\n", "\u{3000}", "\u{301}", "x",
                "- ", "1. ", "```", "//",
            ][..]),
            0..80,
        ),
        ends in proptest::collection::vec((0..300usize, 0..300usize), 1..5),
        mode in proptest::sample::select(&[Mode::Rust, Mode::Toml, Mode::Markdown, Mode::Fundamental][..]),
        shown in proptest::bool::ANY,
        shift in -9..9isize,
    ) {
        let text: String = pieces.concat();
        let boundary = |at: usize| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        };
        let parts: Vec<Selection> = ends
            .iter()
            .map(|&(cursor, mark)| Selection {
                cursor: boundary(cursor),
                mark: Some(boundary(mark)),
                goal: None,
            })
            .collect();
        let run = |f: &dyn Fn(&mut Editor) -> bool, breaks: bool| {
            let mut buffer = Buffer::from_text("t", &text);
            buffer.set_mode(mode);
            // A cursor is where one can be: never inside a cluster, such as a CRLF.
            let display = buffer.display();
            let clip = |at| crate::motion::cluster_start(&display, at);
            let parts: Vec<Selection> = parts
                .iter()
                .map(|part| Selection {
                    cursor: clip(part.cursor),
                    mark: part.mark.map(clip),
                    goal: None,
                })
                .collect();
            let mut editor = Editor::with_buffer(buffer);
            let mut set = SelectionSet::from_parts(parts, 0).unwrap();
            if shown {
                set.set_mark_active(MarkActive::Active);
            }
            editor.windows[0].selections = set;
            let id = editor.window().buffer();
            editor.begin_undo_group();
            f(&mut editor);
            editor.end_undo_group();
            let after = editor.buffer(id).unwrap().text().to_string();
            assert_eq!(without_blanks(&after, breaks), without_blanks(&text, breaks));
            let len = after.len();
            assert!(editor.window().selections.iter().all(|s| s.cursor <= len));
        };
        run(&|e| e.indent_region(None, LIMIT).is_ok(), false);
        run(&|e| e.indent_region(Some(3), LIMIT).is_ok(), false);
        run(&|e| e.indent_rigidly(shift, LIMIT).is_ok(), false);
        run(&|e| e.indent_rigidly_to_tab_stop(Shift::Right, LIMIT).is_ok(), false);
        run(&|e| e.indent_for_tab().is_ok(), false);
        run(&|e| e.delete_indentation(shift > 0).is_ok(), true);
        run(&|e| { e.back_to_indentation(); true }, false);
    }
}

#[test]
fn a_reload_waits_while_the_layer_is_open() {
    let (mut editor, _) = regions(Mode::Fundamental, "[a|", true);
    assert!(!editor.reload_waits());
    editor.set_indent_rigidly_layer(true);
    assert!(editor.reload_waits());
    editor.set_indent_rigidly_layer(false);
    assert!(!editor.reload_waits());
}

#[test]
fn an_edit_from_elsewhere_while_the_layer_is_open_joins_nothing() {
    let (mut editor, id) = regions(Mode::Fundamental, "[a\nb|", true);
    editor.set_indent_rigidly_layer(true);
    rigidly(&mut editor, 1).unwrap();
    rigidly(&mut editor, 1).unwrap();
    // An edit no key of the layer made, outside any command.
    let end = text(&editor, id).len();
    let typed = [crate::buffer::Replacement {
        range: end..end,
        text: "!",
    }];
    command(&mut editor, |e| e.apply_edits(id, &typed)).unwrap();
    editor.set_indent_rigidly_layer(false);
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), "  a\n  b", "only the other edit undoes");
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), " a\n b", "and the shifts one by one");
}

#[test]
fn a_region_command_inserts_at_most_its_own_limit() {
    let lines = super::REGION_INSERT / 4096 + 1;
    let start = format!("[{}|", "x\n".repeat(lines));
    let (mut editor, id) = regions(Mode::Fundamental, &start, false);
    assert_eq!(
        region(&mut editor, Some(4096)),
        Err(IndentError::TooMuchText)
    );
    assert_eq!(text(&editor, id), "x\n".repeat(lines));
    assert_eq!(region(&mut editor, Some(2048)), Ok(()));
}

#[test]
fn a_long_layer_session_starts_a_new_undo_step_past_the_changed_lines() {
    let lines = MAX_CHANGED_LINES * 2 / 3;
    let start = format!("[{}|", "x\n".repeat(lines));
    let (mut editor, id) = regions(Mode::Fundamental, &start, true);
    editor.set_indent_rigidly_layer(true);
    for _ in 0..3 {
        rigidly(&mut editor, 1).unwrap();
    }
    editor.set_indent_rigidly_layer(false);
    let shifted = |columns: usize| format!("{}x\n", " ".repeat(columns)).repeat(lines);
    assert_eq!(text(&editor, id), shifted(3));
    // Each shift but the first passes the bound with the ones before it, so each is a step.
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), shifted(2));
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), shifted(1));
    command(&mut editor, Editor::undo).unwrap();
    assert_eq!(text(&editor, id), shifted(0));
}
