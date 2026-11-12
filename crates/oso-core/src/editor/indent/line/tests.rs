//! Tests of `back-to-indentation` and `delete-indentation` (ADR 0115): spaces and tabs only,
//! the space a join leaves or not, a count, regions, blank lines between, and several cursors.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::super::tests::{command, editor_in, marked};
use crate::buffer::Mode;
use crate::editor::IndentError;
use crate::selection::{MarkActive, Selection, SelectionSet};

#[test]
fn back_to_indentation_skips_spaces_and_tabs_only() {
    let cases = [
        ("  \t ab|c", "  \t |abc"),
        ("|  abc", "  |abc"),
        ("abc|", "|abc"),
        // A full-width space is text.
        ("\u{3000}あ|", "|\u{3000}あ"),
        // A blank line's end.
        ("x\n  |  \ny", "x\n    |\ny"),
        ("|a\n  b|", "|a\n  |b"),
    ];
    for (start, expected) in cases {
        let (mut editor, id) = editor_in(Mode::Fundamental, start);
        editor.back_to_indentation();
        assert_eq!(marked(&editor, id), expected, "{start:?}");
    }
    let (mut editor, _) = editor_in(Mode::Fundamental, "  |a");
    assert!(!editor.back_to_indentation());
}

fn join(start: &str, next: bool) -> Result<String, IndentError> {
    let (mut editor, id) = editor_in(Mode::Fundamental, start);
    command(&mut editor, |editor| editor.delete_indentation(next))?;
    Ok(marked(&editor, id))
}

#[test]
fn delete_indentation_joins_with_one_space_or_none() {
    let cases = [
        ("foo  \n    b|ar", "foo| bar"),
        ("foo\n|bar", "foo| bar"),
        // None after an opener or before a closer, nor at a line's start or end.
        ("f(\n    |a", "f(|a"),
        ("a\n    |)", "a|)"),
        ("\n  |a", "|a"),
        ("a  \n  |", "a|"),
        // A full-width space is text, kept.
        ("a\n\u{3000}|b", "a| \u{3000}b"),
        // A blank a combining mark is on stays as its base.
        ("f(\n   \u{301}|x", "f(| \u{301}x"),
        // The first line has none before it.
        ("|a\nb", "|a\nb"),
        ("a\r\n  |b", "a| b"),
    ];
    for (start, expected) in cases {
        assert_eq!(join(start, false), Ok(expected.to_string()), "{start:?}");
    }
    // With a count, the line after joins this one; the last has none after it.
    assert_eq!(
        join("f|oo\n  bar\nbaz", true),
        Ok("foo| bar\nbaz".to_string())
    );
    assert_eq!(join("foo\nba|r", true), Ok("foo\nba|r".to_string()));
}

#[test]
fn several_cursors_join_their_lines_in_one_edit() {
    assert_eq!(join("a\n|b\nc\n|d", false), Ok("a| b\nc| d".to_string()));
    // Two cursors on adjacent lines join all three, each to its own join.
    assert_eq!(join("a\n|b\n|c", false), Ok("a| b| c".to_string()));
    // Two on one line join it once.
    assert_eq!(join("a\n|b|c", false), Ok("a| bc".to_string()));
}

#[test]
fn a_shown_region_joins_all_its_lines() {
    let (mut editor, id) = editor_in(Mode::Fundamental, "|x\na\n  b\n\n  c\nd");
    let region = Selection {
        cursor: 11,
        mark: Some(2),
        goal: None,
    };
    let mut set = SelectionSet::from_parts(vec![region], 0).unwrap();
    set.set_mark_active(MarkActive::Active);
    editor.windows[0].selections = set;
    command(&mut editor, |editor| editor.delete_indentation(false)).unwrap();
    // A blank line between goes, and the cursor goes to the first join.
    assert_eq!(marked(&editor, id), "x\na| b c\nd");
    assert!(!editor.window().selections.mark_active().is_active());
}

#[test]
fn a_region_on_one_line_or_hidden_joins_as_without_one() {
    let (mut editor, id) = editor_in(Mode::Fundamental, "|x\nab");
    let region = Selection {
        cursor: 3,
        mark: Some(2),
        goal: None,
    };
    let mut set = SelectionSet::from_parts(vec![region], 0).unwrap();
    set.set_mark_active(MarkActive::Active);
    editor.windows[0].selections = set.clone();
    command(&mut editor, |editor| editor.delete_indentation(false)).unwrap();
    assert_eq!(marked(&editor, id), "x\na|b");
    set.set_mark_active(MarkActive::Inactive);
    editor.windows[0].selections = set;
    command(&mut editor, |editor| editor.delete_indentation(false)).unwrap();
    assert_eq!(marked(&editor, id), "x| ab");
}

#[test]
fn too_many_lines_to_join_change_nothing() {
    let text = "a\n".repeat(super::MAX_CHANGED_LINES + 2);
    let start = format!("|{text}");
    let (mut editor, id) = editor_in(Mode::Fundamental, &start);
    let region = Selection {
        cursor: text.len(),
        mark: Some(0),
        goal: None,
    };
    let mut set = SelectionSet::from_parts(vec![region], 0).unwrap();
    set.set_mark_active(MarkActive::Active);
    editor.windows[0].selections = set;
    let done = command(&mut editor, |editor| editor.delete_indentation(false));
    assert_eq!(done, Err(IndentError::TooLarge));
    assert_eq!(editor.buffer(id).unwrap().text().to_string(), text);
}

#[test]
fn a_single_join_beside_a_huge_line_reads_only_its_edges() {
    let long = "x".repeat(super::REGION_LIMIT + 1);
    let (mut editor, id) = editor_in(Mode::Fundamental, &format!("{long}\n  |b"));
    command(&mut editor, |editor| editor.delete_indentation(false)).unwrap();
    assert_eq!(marked(&editor, id), format!("{long}| b"));
}
