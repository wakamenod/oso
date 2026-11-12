//! Tests of Markdown's keys (ADR 0115): `TAB` cycling a line through its list's columns and
//! fenced code blocks, and `M-RET` starting items, numbered or not, nested, before an item, on
//! a blank line, and nowhere outside a list.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::super::tests::{LIMIT, command, editor_in, marked};
use crate::buffer::Mode;
use crate::editor::{Editor, IndentError};

fn tab(start: &str, times: usize) -> String {
    let (mut editor, id) = editor_in(Mode::Markdown, start);
    for _ in 0..times {
        command(&mut editor, Editor::indent_for_tab).unwrap();
    }
    marked(&editor, id)
}

#[test]
fn tab_cycles_through_the_items_then_the_margin() {
    let list = "- a\n  - b\n";
    let cases = [(1, "    |c"), (2, "  |c"), (3, "|c"), (4, "    |c")];
    for (times, expected) in cases {
        assert_eq!(
            tab(&format!("{list}|c"), times),
            format!("{list}{expected}")
        );
    }
    // A line at no stop goes to the innermost; one at the innermost, out.
    assert_eq!(tab("- a\n |c", 1), "- a\n  |c");
    assert_eq!(tab(&format!("{list}    |c"), 1), format!("{list}  |c"));
    // A cursor in the text keeps its place in it; the numbered item's text is further in.
    assert_eq!(tab(&format!("{list}c|d"), 1), format!("{list}    c|d"));
    assert_eq!(tab("10. a\n|b", 1), "10. a\n    |b");
    // Outside a list only the margin is a stop.
    assert_eq!(tab("a\n   |b", 1), "a\n|b");
    assert_eq!(tab("a\n|b", 1), "a\n|b");
}

#[test]
fn tab_in_a_fenced_code_block_goes_to_the_next_tab_stop() {
    assert_eq!(tab("- a\n```\n|x\n```\n", 1), "- a\n```\n        |x\n```\n");
    assert_eq!(tab("```\nx|y|\n```\n", 1), "```\nx       |y       |\n```\n");
    // After the fence, the cycle again.
    assert_eq!(tab("- a\n```\nx\n```\n|b", 1), "- a\n```\nx\n```\n  |b");
}

#[test]
fn tab_reindents_each_line_once_for_several_cursors() {
    let start = "- a\n|b|c\n  - d\n|e";
    assert_eq!(tab(start, 1), "- a\n  |b|c\n  - d\n    |e");
}

fn item(start: &str) -> Result<String, IndentError> {
    let (mut editor, id) = editor_in(Mode::Markdown, start);
    command(&mut editor, |editor| {
        editor.markdown_insert_list_item("\n", LIMIT)
    })?;
    Ok(marked(&editor, id))
}

#[test]
fn m_ret_starts_the_next_item() {
    let cases = [
        ("- a|", "- a\n- |"),
        ("* a|\n", "* a\n* |\n"),
        ("1. a|\n2. b", "1. a\n2. |\n2. b"),
        ("1) a|", "1) a\n2) |"),
        // The text stays where it was when the number grows, if a space can go.
        ("9.  a|", "9.  a\n10. |"),
        ("9. a|", "9. a\n10. |"),
        // Past nine digits a number is no marker: the last one takes no tenth.
        ("999999999. a|", "999999999. a\n999999999. |"),
        ("- a\n  - b|", "- a\n  - b\n  - |"),
        ("- a\n  1. b\n  more|", "- a\n  1. b\n  more\n  2. |"),
        // It breaks the line at the cursor, the blanks around it gone.
        ("- ab  |  cd", "- ab\n- |cd"),
        // On a blank line, the item goes on it.
        ("- a\n  |", "- a\n- |"),
        ("- a\n\n|", "- a\n\n- |"),
        // Before an item's marker, a new one before it, with its number.
        ("|- a", "- |\n- a"),
        ("x\n  |1. a", "x\n  1. |\n  1. a"),
    ];
    for (start, expected) in cases {
        assert_eq!(item(start), Ok(expected.to_string()), "{start:?}");
    }
}

#[test]
fn m_ret_at_several_cursors_is_one_edit() {
    assert_eq!(
        item("- a|\n- b|\n1. c|"),
        Ok("- a\n- |\n- b\n- |\n1. c\n2. |".to_string())
    );
}

#[test]
fn m_ret_outside_a_list_changes_nothing() {
    for start in ["a|", "- a\n\nb|", "- a\n```\n|x\n```", "# h\n- a\n\n# |"] {
        assert_eq!(item(start), Err(IndentError::NotInList), "{start:?}");
    }
    // One cursor outside a list stops them all.
    assert_eq!(item("- a|\nb\n\nc|"), Err(IndentError::NotInList));
    let (mut editor, _) = editor_in(Mode::Fundamental, "- a|");
    let done = command(&mut editor, |e| e.markdown_insert_list_item("\n", LIMIT));
    assert_eq!(done, Err(IndentError::NotInList));
    let (mut editor, id) = editor_in(Mode::Markdown, "- a|");
    let done = command(&mut editor, |e| e.markdown_insert_list_item("\n", 2));
    assert_eq!(done, Err(IndentError::TooMuchText));
    assert_eq!(marked(&editor, id), "- a|");
}

#[test]
fn m_ret_takes_the_files_line_ending() {
    let (mut editor, id) = editor_in(Mode::Markdown, "- a|\r\n");
    command(&mut editor, |e| e.markdown_insert_list_item("\r\n", LIMIT)).unwrap();
    assert_eq!(marked(&editor, id), "- a\r\n- |\r\n");
}

proptest::proptest! {
    #[test]
    fn m_ret_and_tab_never_panic_and_m_ret_adds_only_items(
        pieces in proptest::collection::vec(
            proptest::sample::select(&[
                "- ", "* ", "1. ", "999999999. ", "000000001) ", "  ", "\t", "\n", "\r\n",
                "\u{301}", "\u{3000}", "x", "```", "#",
            ][..]),
            0..60,
        ),
        at in proptest::collection::vec(0..300usize, 1..6),
    ) {
        let text: String = pieces.concat();
        let mut marked = text.clone();
        let mut at: Vec<usize> = at.into_iter().map(|at| {
            let mut at = at.min(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            at
        }).collect();
        at.sort_unstable();
        at.dedup();
        for &at in at.iter().rev() {
            marked.insert(at, '|');
        }
        for item in [true, false] {
            let (mut editor, id) = editor_in(Mode::Markdown, &marked);
            // A cursor is where one can be: never inside a cluster.
            let display = editor.current().1.display();
            let clipped: Vec<_> = editor
                .window()
                .selections
                .iter()
                .map(|s| crate::selection::Selection::at(crate::motion::cluster_start(&display, s.cursor)))
                .collect();
            editor.windows[0].selections = crate::selection::SelectionSet::from_parts(clipped, 0).unwrap();
            let done = command(&mut editor, |editor| {
                if item {
                    editor.markdown_insert_list_item("\n", LIMIT).is_ok()
                } else {
                    editor.indent_for_tab().is_ok()
                }
            });
            let after = editor.buffer(id).unwrap().text().to_string();
            let len = after.len();
            proptest::prop_assert!(editor.window().selections.iter().all(|s| s.cursor <= len));
            if item && done {
                // What was there stays, in order; only blanks around the cursors may go.
                let kept: String = text.chars().filter(|c| !matches!(c, ' ' | '\t')).collect();
                let mut rest = after.chars().filter(|c| !matches!(c, ' ' | '\t'));
                proptest::prop_assert!(kept.chars().all(|c| rest.any(|d| d == c)));
            }
        }
    }
}
