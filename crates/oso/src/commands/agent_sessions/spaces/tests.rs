//! The space commands' requests (ADR 0159): `space-new` asks for a space for its description,
//! `space-close` for the close of the one chosen,
//! `space-rename` asks the host for its prompt, `space-rename-to` gives the name it read, and an
//! answer from a prompt that may have held secrets asks for nothing.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use oso_core::buffer::Buffer;
use oso_core::command::{Args, Ctx, ExecuteError, Invocation};
use oso_core::editor::{Editor, SessionRequest, SpaceRequest};

use crate::commands::registry;

/// Runs `name` with `answer` as its prompt's, which may have held secrets when `secret`.
fn run_answered(
    editor: &mut Editor,
    name: &str,
    answer: Option<&str>,
    secret: bool,
) -> Result<(), String> {
    let registry = registry();
    let id = registry.lookup(name).unwrap();
    let invocation = Invocation {
        answer,
        answer_secret: secret,
        ..Invocation::default()
    };
    let mut ctx = Ctx::invoked(editor, invocation);
    (registry.execute(id, &mut ctx, Args::NONE)).map_err(|e| match e {
        ExecuteError::Failed { error, .. } => error.message().to_owned(),
        other => panic!("{other}"),
    })
}

fn spaces_asked(editor: &mut Editor) -> Vec<SpaceRequest> {
    (editor.take_session_requests().into_iter())
        .filter_map(|request| match request {
            SessionRequest::Spaces(space) => Some(space),
            _ => None,
        })
        .collect()
}

#[test]
fn space_new_asks_for_its_description_unless_it_may_hold_secrets() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "a\n"));
    let typed = Some(" Fix the login ");
    assert_eq!(run_answered(&mut editor, "space-new", typed, false), Ok(()));
    assert_eq!(
        spaces_asked(&mut editor),
        [SpaceRequest::New("Fix the login".to_owned())]
    );
    let refused = run_answered(&mut editor, "space-new", typed, true);
    assert!(refused.is_err_and(|why| why.contains("secret")));
    assert!(spaces_asked(&mut editor).is_empty(), "nothing asked");
}

#[test]
fn space_close_asks_for_the_space_its_prompt_chose() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "a\n"));
    let chosen = Some("ログイン 🔧");
    assert_eq!(
        run_answered(&mut editor, "space-close", chosen, false),
        Ok(())
    );
    assert_eq!(
        spaces_asked(&mut editor),
        [SpaceRequest::Close("ログイン 🔧".to_owned())]
    );
    for none in [None, Some(""), Some("  ")] {
        let refused = run_answered(&mut editor, "space-close", none, false);
        assert_eq!(refused, Err("No space".to_owned()));
    }
    assert!(spaces_asked(&mut editor).is_empty(), "nothing asked");
}

#[test]
fn space_rename_asks_the_host_for_its_prompt_and_its_answer_gives_the_name() {
    let mut editor = Editor::with_buffer(Buffer::from_text("a.rs", "a\n"));
    assert_eq!(
        run_answered(&mut editor, "space-rename", None, false),
        Ok(())
    );
    assert_eq!(spaces_asked(&mut editor), [SpaceRequest::AskRename]);
    let name = Some("ログイン 🔧");
    assert_eq!(
        run_answered(&mut editor, "space-rename-to", name, false),
        Ok(())
    );
    assert_eq!(
        spaces_asked(&mut editor),
        [SpaceRequest::Rename("ログイン 🔧".to_owned())]
    );
    let refused = run_answered(&mut editor, "space-rename-to", name, true);
    assert!(refused.is_err_and(|why| why.contains("secret")));
    assert!(spaces_asked(&mut editor).is_empty(), "nothing asked");
}
