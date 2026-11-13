//! `*Sessions*`'s commands: each asks the host about the row point is on, only in that buffer
//! and not inside a prompt; moving between rows draws only when point moved.
//!
//! Must not depend on: `app`, winit or the GPU.

use oso_core::buffer::Mode;
use oso_core::command::Args;
use oso_core::editor::{Editor, RowAction, SessionRequest};

use crate::commands::tests::{editor, run};

fn listing() -> Editor {
    let mut editor = editor("");
    let list = editor.add_internal_buffer("*Sessions*", Mode::Sessions);
    editor.set_internal_text(list, "▾ Idle 1\n○ a  1m\n  oso\n  recap\n");
    editor.show(list);
    editor
}

#[test]
fn each_row_command_names_the_line_and_its_action() {
    let mut editor = listing();
    let cases = [
        ("sessions-show", RowAction::Show),
        ("sessions-toggle-fold", RowAction::Fold),
        ("sessions-pin", RowAction::Pin),
        ("sessions-archive", RowAction::Archive),
        ("sessions-resume", RowAction::Resume),
        ("session-stop", RowAction::Stop),
        ("sessions-explain", RowAction::Explain),
    ];
    for (name, action) in cases {
        assert_eq!(run(&mut editor, name, Args::NONE).0, Ok(()), "{name}");
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::Row { line: 0, action }],
            "{name}"
        );
    }
}

#[test]
fn outside_the_list_they_refuse_and_ask_nothing() {
    let mut editor = editor("text");
    for name in [
        "sessions-show",
        "sessions-pin",
        "session-stop",
        "sessions-refresh",
        "sessions-regroup",
        "sessions-next",
    ] {
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Err("Not in *Sessions*".to_owned()), "{name}");
        assert!(!effects.redraw, "{name}");
    }
    assert!(editor.take_session_requests().is_empty());
}

#[test]
fn moving_draws_only_when_point_moves() {
    let mut editor = listing();
    let (result, effects) = run(&mut editor, "sessions-previous", Args::NONE);
    assert_eq!(
        (result, effects.redraw),
        (Ok(()), false),
        "already at the top"
    );
    let (result, effects) = run(&mut editor, "sessions-next", Args::NONE);
    assert_eq!((result, effects.redraw), (Ok(()), true));
    assert_eq!(editor.cursor_line(), 1);
    let (_, effects) = run(&mut editor, "sessions-next", Args::NONE);
    assert!(!effects.redraw, "the last row");
    let back = Args::numeric(-1);
    assert_eq!(run(&mut editor, "sessions-next", back).0, Ok(()));
    assert_eq!(editor.cursor_line(), 0, "a negative count goes up");
    assert_eq!(
        run(&mut editor, "sessions-next", Args::numeric(isize::MIN)).0,
        Ok(())
    );
    assert_eq!(editor.cursor_line(), 0);
}
