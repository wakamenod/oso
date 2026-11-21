//! Tests of YAML's keys (ADR 0164): `RET` after a block, a block scalar, a bare item and a
//! comment, `TAB`'s cycle and a line it keeps past the reach, no region, and spaces whatever
//! the setting says.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::super::IndentError;
use super::super::languages_tests::{LIMIT, command, editor_in, editor_with, marked, ret, two};
use crate::buffer::Mode;
use crate::editor::Editor;
use crate::indent::{Origin, Origins, Settings, Style};

#[test]
fn ret_goes_in_after_a_block_and_follows_any_other_line() {
    let cases = [
        ("jobs:\n  test:|", "jobs:\n  test:\n    |"),
        ("steps:\n  - name: a|", "steps:\n  - name: a\n  |"),
        ("steps:\n  - with:|", "steps:\n  - with:\n      |"),
        // `|` marks a cursor here; `|` scalars are `indent::yaml`'s to test.
        ("run: >-|", "run: >-\n  |"),
        ("text: > # folded|", "text: > # folded\n  |"),
        ("list:\n  -|", "list:\n  -\n    |"),
        // A comment is no code line: the line before it is followed.
        ("a:\n  b: 1\n# note|", "a:\n  b: 1\n# note\n  |"),
    ];
    for (before, after) in cases {
        let (mut editor, id) = editor_with(Mode::Yaml, two(), before);
        ret(&mut editor);
        assert_eq!(marked(&editor, id), after, "{before:?}");
    }
}

#[test]
fn tab_cycles_out_and_back() {
    let (mut editor, id) = editor_with(Mode::Yaml, two(), "steps:\n  - name: a\n|x");
    let mut seen = Vec::new();
    for _ in 0..4 {
        command(&mut editor, Editor::indent_for_tab).unwrap();
        seen.push(marked(&editor, id));
    }
    assert_eq!(
        seen,
        [
            "steps:\n  - name: a\n    |x",
            "steps:\n  - name: a\n  |x",
            "steps:\n  - name: a\n|x",
            "steps:\n  - name: a\n    |x",
        ]
    );
}

/// Past the reach, the line before is not known: `TAB` leaves the line, whose indentation is
/// what it means, where it is.
#[test]
fn tab_keeps_a_line_past_the_reach() {
    let text = format!("a:\n{}\n      |x", "  # c\n".repeat(20_000));
    let (mut editor, id) = editor_with(Mode::Yaml, two(), &text);
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), text);
}

#[test]
fn no_region_is_reindented_by_rules() {
    let (mut editor, _) = editor_with(Mode::Yaml, two(), "a:\n|b: 1");
    let reindented = command(&mut editor, |editor| editor.indent_region(None, LIMIT));
    assert_eq!(reindented, Err(IndentError::NoRules));
}

#[test]
fn yaml_is_indented_with_spaces_whatever_the_setting_says() {
    let (mut editor, id) = editor_in(Mode::Yaml, "|");
    let tabs = Settings::new(2, Style::Tabs, 8).unwrap();
    let modes = crate::editor::indent::compiled_modes();
    editor.set_mode_indents(modes.map(|mode| {
        let origins = Origins {
            style: Origin::Configuration,
            ..Origins::default()
        };
        (mode, tabs, origins)
    }));
    let buffer = editor.buffer(id).unwrap();
    assert_eq!(buffer.indent_settings(), two());
    // The style is the mode's own, not the configuration's that asked for tabs.
    assert_eq!(buffer.indent_origins().style, Origin::Default);
}
