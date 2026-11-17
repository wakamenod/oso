//! Tests of a server's code actions (ADR 0138, step 13): the list as it reads, the preferred
//! action selected, a disabled one faded and refused, the action `RET` took, and a server's
//! edit applied whole, one undo step a buffer, or not at all.
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

fn part(editor: &Editor, buffer: BufferId, edits: &[(Range<usize>, &str)]) -> BufferEdit {
    BufferEdit {
        buffer,
        revision: editor.buffer(buffer).unwrap().revision(),
        edits: (edits.iter())
            .map(|(range, text)| (range.clone(), (*text).to_owned()))
            .collect(),
    }
}

fn text(editor: &Editor, id: BufferId) -> String {
    editor.buffer(id).unwrap().text().to_string()
}

#[test]
fn an_edit_applies_to_each_buffer_in_one_undo_step() {
    let mut editor = editor("let x = 1;\n");
    let a = editor.tiled().0.buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "fn b() {}\n"));
    let edit = [
        // Two insertions at one place go in the server's order.
        part(
            &editor,
            a,
            &[(0..0, "use std;\n"), (4..5, "y"), (0..0, "use core;\n")],
        ),
        part(&editor, b, &[(3..4, "c")]),
    ];
    let applied = editor.apply_server_edit(&edit).unwrap();
    assert_eq!(
        applied,
        EditApplied {
            edits: 3,
            buffers: 2
        }
    );
    assert_eq!(text(&editor, a), "use std;\nuse core;\nlet y = 1;\n");
    assert_eq!(text(&editor, b), "fn c() {}\n");
    editor.undo().unwrap();
    assert_eq!(text(&editor, a), "let x = 1;\n", "one step");
}

#[test]
fn an_edit_that_is_stale_overlaps_or_meets_a_secret_changes_nothing() {
    let mut editor = editor("let x = 1;\n");
    let a = editor.tiled().0.buffer();
    let b = editor.add_buffer(Buffer::from_text("b.rs", "fn b() {}\n"));
    let fine = part(&editor, a, &[(4..5, "y")]);
    let mut stale = part(&editor, b, &[(3..4, "c")]);
    stale.revision += 1;
    assert_eq!(
        editor.apply_server_edit(&[fine.clone(), stale]),
        Err("A buffer it edits changed since the server answered")
    );
    let overlapping = part(&editor, b, &[(0..5, "x"), (3..6, "y")]);
    assert_eq!(
        editor.apply_server_edit(&[fine.clone(), overlapping]),
        Err("Its edits overlap")
    );
    let gone = BufferEdit {
        buffer: BufferId(999),
        ..fine.clone()
    };
    assert!(editor.apply_server_edit(&[gone]).is_err());
    assert_eq!(text(&editor, a), "let x = 1;\n", "nothing applied");
    assert_eq!(text(&editor, b), "fn b() {}\n");
    let path = std::path::PathBuf::from("/w/.env");
    let mut secret = Buffer::visiting(path.clone());
    secret.judge_secret(Some(std::slice::from_ref(&path)));
    let secret = editor.add_buffer(secret);
    let edit = part(&editor, secret, &[(0..3, "K")]);
    assert_eq!(
        editor.apply_server_edit(&[edit]),
        Err("No server edits a buffer that may hold secrets")
    );
}
