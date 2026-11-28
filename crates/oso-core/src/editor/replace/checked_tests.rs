//! A query-replace regexp's prompt keeps the regexp it compiled, so that the session compiles
//! it once, but not one that a secret was yanked into, which the command may refuse.
//!
//! Must not depend on: `command` or `keymap`; these run on the editor alone.

use crate::buffer::Buffer;
use crate::editor::Editor;

fn editor(text: &str) -> Editor {
    Editor::with_buffer(Buffer::from_text("notes.txt", text))
}

/// Opens a regexp's prompt, as query-replace-regexp's first is, yanks `pattern` into it from a
/// kill that is secret if `secret`, and answers it.
fn regexp_answered(editor: &mut Editor, pattern: &str, secret: bool) {
    let kind = crate::minibuffer::Kind::Text { history: None };
    editor.open_prompt("Query replace regexp: ", kind).unwrap();
    editor.prompt_takes_regexp();
    editor.kill_new(pattern, secret);
    editor.yank(0, false).unwrap();
    editor.submit_prompt().unwrap();
}

#[test]
fn a_regexp_that_a_secret_was_yanked_into_is_not_kept_compiled() {
    let mut editor = editor("foo\n");
    regexp_answered(&mut editor, "f.o", false);
    assert!(editor.replacing.checked.is_some(), "kept to compile once");
    let mut editor = self::editor("foo\n");
    regexp_answered(&mut editor, "f.o|hunter2", true);
    assert!(editor.replacing.checked.is_none());
    // The replace still compiles it when it begins.
    editor.replace_begin("f.o|hunter2", "x", true).unwrap();
    assert!(editor.replace().is_some());
}
