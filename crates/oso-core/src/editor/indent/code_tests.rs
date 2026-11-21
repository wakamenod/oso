//! Tests of the keys in ADR 0164's modes: `TAB` reindenting CSS, HTML, JavaScript and Go
//! lines, typed closers and end tags, `RET` between brackets and between a start tag and its
//! end tag, YAML's `RET` and `TAB`, and each mode's default indentation.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

use super::tests::{LIMIT, command, editor_in, marked};
use crate::buffer::{BufferId, Mode};
use crate::editor::Editor;
use crate::editor::indent::IndentError;
use crate::indent::{Origins, Settings, Style};

fn two() -> Settings {
    Settings::new(2, Style::Spaces, 8).unwrap()
}

/// An editor showing `marked` in `mode`, indented by `settings`.
fn editor_with(mode: Mode, settings: Settings, marked: &str) -> (Editor, BufferId) {
    let (mut editor, id) = editor_in(mode, marked);
    editor.buffer_mut(id).unwrap().set_indent_settings(settings);
    (editor, id)
}

fn ret(editor: &mut Editor) {
    let done = command(editor, |editor| editor.newline("\n", 1, true, LIMIT));
    assert_eq!(done, Ok(true));
}

#[test]
fn tab_reindents_lines_in_the_languages_of_brackets_and_tags() {
    let cases = [
        (
            Mode::Css,
            "a {\n|color: red;\n}\n",
            "a {\n  |color: red;\n}\n",
        ),
        (Mode::Json, "{\n      |\"a\": 1\n}\n", "{\n  |\"a\": 1\n}\n"),
        (
            Mode::Html,
            "<ul>\n|<li>a</li>\n</ul>\n",
            "<ul>\n  |<li>a</li>\n</ul>\n",
        ),
        (
            Mode::Tsx,
            "const a = (\n  <div>\n|<p />\n",
            "const a = (\n  <div>\n    |<p />\n",
        ),
        (
            Mode::TypeScript,
            "if (a) {\n|b();\n}\n",
            "if (a) {\n  |b();\n}\n",
        ),
    ];
    for (mode, before, after) in cases {
        let (mut editor, id) = editor_with(mode, two(), before);
        command(&mut editor, Editor::indent_for_tab).unwrap();
        assert_eq!(marked(&editor, id), after, "{mode:?}");
    }
    let tabs = Settings::new(8, Style::Tabs, 8).unwrap();
    let (mut editor, id) = editor_with(Mode::Go, tabs, "func f() {\n|x()\n}\n");
    command(&mut editor, Editor::indent_for_tab).unwrap();
    assert_eq!(marked(&editor, id), "func f() {\n\t|x()\n}\n");
}

#[test]
fn a_typed_closer_or_end_tag_first_on_its_line_goes_under_its_opener() {
    let (mut editor, id) = editor_with(Mode::Css, two(), "a {\n  b: c;\n    }|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "a {\n  b: c;\n}|");
    let (mut editor, id) = editor_with(Mode::Html, two(), "<div>\n  <p>x</p>\n    </|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "<div>\n  <p>x</p>\n</|");
    let (mut editor, id) = editor_with(Mode::Tsx, two(), "(\n  <div>\n    <p />\n      </|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "(\n  <div>\n    <p />\n  </|");
    // A `/` that is no end tag's, or not first on its line, moves nothing; nor does `</` in
    // TypeScript, which has no elements.
    let (mut editor, id) = editor_with(Mode::Html, two(), "<div>\n    a </|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "<div>\n    a </|");
    let (mut editor, id) = editor_with(Mode::TypeScript, two(), "{\n    </|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "{\n    </|");
}

#[test]
fn ret_between_brackets_and_between_a_start_tag_and_its_end_tag() {
    let cases = [
        (Mode::Css, "a {|}", "a {\n  |\n}"),
        (Mode::Json, "[|]", "[\n  |\n]"),
        (Mode::Go, "f(|)", "f(\n  |\n)"),
        (
            Mode::Html,
            "<ul>\n  <li>|</li>",
            "<ul>\n  <li>\n    |\n  </li>",
        ),
        (
            Mode::JavaScript,
            "x = <div>|</div>",
            "x = <div>\n  |\n</div>",
        ),
    ];
    for (mode, before, after) in cases {
        let (mut editor, id) = editor_with(mode, two(), before);
        ret(&mut editor);
        assert_eq!(marked(&editor, id), after, "{mode:?}");
    }
    // Another element's end tag, or a void element, opens no line between.
    let (mut editor, id) = editor_with(Mode::Html, two(), "<p>|</b>");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "<p>\n|</b>");
    // JSON's parentheses mean nothing.
    let (mut editor, id) = editor_with(Mode::Json, two(), "(|)");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "(\n|)");
}

#[test]
fn yaml_s_ret_goes_in_after_a_block_and_tab_cycles_out() {
    let (mut editor, id) = editor_with(Mode::Yaml, two(), "jobs:\n  test:|");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "jobs:\n  test:\n    |");
    let (mut editor, id) = editor_with(Mode::Yaml, two(), "steps:\n  - name: a|");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "steps:\n  - name: a\n  |");
    // `TAB` cycles: the item's content column, then out to 0, then back.
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
    // YAML's lines are never reindented by rules: a region is refused, as in Fundamental.
    let (mut editor, _) = editor_with(Mode::Yaml, two(), "a:\n|b: 1");
    let reindented = command(&mut editor, |editor| editor.indent_region(None, LIMIT));
    assert_eq!(reindented, Err(IndentError::NoRules));
}

#[test]
fn each_new_mode_has_its_default_and_yaml_only_spaces() {
    let (mut editor, id) = editor_in(Mode::Yaml, "|");
    assert_eq!(editor.mode_indent(Mode::Html), two());
    assert_eq!(editor.mode_indent(Mode::Tsx), two());
    assert_eq!(
        editor.mode_indent(Mode::Go),
        Settings::new(8, Style::Tabs, 8).unwrap()
    );
    assert_eq!(editor.mode_indent(Mode::Rust), Settings::default());
    // A configuration that asks YAML for tabs gets spaces.
    let tabs = Settings::new(2, Style::Tabs, 8).unwrap();
    let modes = crate::editor::indent::compiled_modes();
    editor.set_mode_indents(modes.map(|mode| (mode, tabs, Origins::default())));
    assert_eq!(editor.buffer(id).unwrap().indent_settings(), two());
}
