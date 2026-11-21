//! Tests of the keys in ADR 0164's languages of brackets and tags: `TAB` reindenting CSS,
//! JSON, HTML, JavaScript, TypeScript and Go lines, typed closers and end tags, `RET` between
//! brackets and between a start tag and its end tag, and each mode's default indentation.
//! YAML's are `yaml/tests.rs`'s.
//!
//! Must not depend on: `command` or `keymap`; the dispatcher's calls are written out here.

pub(super) use super::tests::{LIMIT, command, editor_in, marked};
use crate::buffer::{BufferId, Mode};
use crate::editor::Editor;
use crate::indent::{Settings, Style};

pub(super) fn two() -> Settings {
    Settings::new(2, Style::Spaces, 8).unwrap()
}

/// An editor showing `marked` in `mode`, indented by `settings`.
pub(super) fn editor_with(mode: Mode, settings: Settings, marked: &str) -> (Editor, BufferId) {
    let (mut editor, id) = editor_in(mode, marked);
    editor.buffer_mut(id).unwrap().set_indent_settings(settings);
    (editor, id)
}

pub(super) fn ret(editor: &mut Editor) {
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
fn an_end_tag_compares_names_as_html_does() {
    // HTML ignores case, and a void element has no end tag to open a line before.
    let (mut editor, id) = editor_with(Mode::Html, two(), "<LI>|</li>");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "<LI>\n  |\n</li>");
    let (mut editor, id) = editor_with(Mode::Html, two(), "<br>|</br>");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "<br>\n|</br>");
    // JSX does not ignore case.
    let (mut editor, id) = editor_with(Mode::JavaScript, two(), "x = <Li>|</li>");
    ret(&mut editor);
    assert_eq!(marked(&editor, id), "x = <Li>\n|</li>");
    // A typed `</` in JavaScript goes under its element's line too.
    let (mut editor, id) = editor_with(Mode::JavaScript, two(), "x = (\n  <a>\n      </|");
    command(&mut editor, Editor::indent_typed_closers).unwrap();
    assert_eq!(marked(&editor, id), "x = (\n  <a>\n  </|");
}

#[test]
fn each_mode_has_its_default() {
    let (editor, _) = editor_in(Mode::Rust, "|");
    let tab = Settings::new(8, Style::Tabs, 8).unwrap();
    for mode in [
        Mode::Html,
        Mode::Css,
        Mode::Json,
        Mode::Yaml,
        Mode::JavaScript,
        Mode::TypeScript,
        Mode::Tsx,
    ] {
        assert_eq!(editor.mode_indent(mode), two(), "{mode:?}");
        assert_eq!(mode.default_indent(), two(), "{mode:?}");
    }
    assert_eq!(editor.mode_indent(Mode::Go), tab);
    for mode in [
        Mode::Rust,
        Mode::Toml,
        Mode::Markdown,
        Mode::Latex,
        Mode::Fundamental,
    ] {
        assert_eq!(editor.mode_indent(mode), Settings::default(), "{mode:?}");
    }
}
