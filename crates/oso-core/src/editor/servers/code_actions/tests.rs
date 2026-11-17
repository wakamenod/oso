//! Tests of a server's code actions (ADR 0138, step 13): the list as it reads, the preferred
//! action selected, a disabled one faded and refused, and the action `RET` took.
//!
//! Must not depend on: threads or the file system.

use super::*;
use crate::buffer::Buffer;
use crate::frame::FrameSize;

fn editor(text: &str) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", text));
    editor.set_frame_size(FrameSize {
        rows: 40,
        columns: 120,
    });
    editor
}

fn item(
    title: &str,
    kind: Option<&str>,
    preferred: bool,
    disabled: Option<&str>,
) -> CodeActionItem {
    CodeActionItem {
        title: title.into(),
        kind: kind.map(str::to_owned),
        preferred,
        disabled: disabled.map(str::to_owned),
    }
}

fn items() -> Vec<CodeActionItem> {
    vec![
        item(
            "Extract into function",
            Some("refactor.extract"),
            false,
            None,
        ),
        item(
            "Inline",
            Some("refactor.inline"),
            false,
            Some("nothing to inline"),
        ),
        item("Import `HashMap`", Some("quickfix"), true, None),
    ]
}

#[test]
fn the_list_shows_each_action_with_its_kind_the_preferred_selected() {
    let mut editor = editor("let x = 1;\n");
    editor.open_code_actions(&items()).unwrap();
    let candidates = editor.prompt().unwrap().candidates().unwrap();
    let rows: Vec<(&str, &str)> = (0..candidates.len())
        .map(|place| {
            let candidate = candidates.get(place).unwrap();
            (candidate.text(), candidate.annotation())
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("Extract into function", "refactor.extract"),
            ("Inline", "refactor.inline (disabled: nothing to inline)"),
            ("Import `HashMap`", "quickfix"),
        ],
        "in the server's order"
    );
    assert_eq!(candidates.selected(), Some(2), "the preferred one");
    assert_eq!(
        (0..3)
            .map(|place| editor.code_action_disabled(place))
            .collect::<Vec<_>>(),
        [Some(false), Some(true), Some(false)]
    );
    assert!(
        editor.open_code_actions(&items()).is_err(),
        "one list at a time"
    );
    assert!(editor.open_code_actions(&[]).is_err() || editor.prompt().is_some());
}

#[test]
fn ret_takes_the_selected_action_once_and_refuses_a_disabled_one() {
    let mut editor = editor("let x = 1;\n");
    editor.open_code_actions(&items()).unwrap();
    editor.move_completion(-1).unwrap();
    assert!(editor.submit_prompt().is_err(), "the disabled one");
    assert!(editor.prompt().is_some(), "the list stays open");
    assert_eq!(
        editor.prompt().unwrap().message(),
        Some("This code action is disabled: nothing to inline")
    );
    assert!(editor.take_code_action_hit().is_none());
    editor.move_completion(-1).unwrap();
    editor.submit_prompt().unwrap();
    assert!(editor.prompt().is_none());
    assert_eq!(editor.take_code_action_hit(), Some(0));
    assert_eq!(editor.take_code_action_hit(), None, "once");
    assert_eq!(editor.code_action_disabled(0), None, "no list is open");
}
