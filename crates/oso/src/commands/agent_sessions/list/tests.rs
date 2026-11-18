//! `*Spaces*`'s commands: each asks the host about the row point is on, only in that buffer
//! and not inside a prompt; moving between rows draws only when point moved.
//!
//! Must not depend on: `app`, winit or the GPU.

use oso_core::buffer::Mode;
use oso_core::command::Args;
use oso_core::editor::{Editor, RowAction, SessionRequest};
use oso_core::selection::SelectionSet;

use crate::commands::tests::{editor, run};

fn listing() -> Editor {
    let mut editor = editor("");
    let list = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
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
        ("session-rename", RowAction::Rename),
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
fn help_says_the_marks_and_keys_in_the_list_only() {
    let mut list = listing();
    assert_eq!(run(&mut list, "sessions-help", Args::NONE).0, Ok(()));
    let id = list.messages_buffer();
    let said = list.buffer(id).unwrap().text().to_string();
    assert!(
        said.contains("⚠ needs input  ✗ failed  ▶ working"),
        "{said}"
    );
    assert!(list.take_session_requests().is_empty());
    let mut elsewhere = editor("text");
    let refused = run(&mut elsewhere, "sessions-help", Args::NONE).0;
    assert_eq!(refused, Err("Not in *Spaces*".to_owned()));
}

#[test]
fn outside_the_list_they_refuse_and_ask_nothing() {
    let mut editor = editor("text");
    for name in [
        "sessions-show",
        "sessions-pin",
        "sessions-refresh",
        "sessions-regroup",
        "sessions-next",
    ] {
        let (result, effects) = run(&mut editor, name, Args::NONE);
        assert_eq!(result, Err("Not in *Spaces*".to_owned()), "{name}");
        assert!(!effects.redraw, "{name}");
    }
    // `session-stop` acts on a session's own terminal too (ADR 0132), and so does
    // `session-rename` (ADR 0155).
    for name in ["session-stop", "session-rename"] {
        let (result, _) = run(&mut editor, name, Args::NONE);
        assert_eq!(
            result,
            Err("Not in *Spaces* or a session's buffer".to_owned()),
            "{name}"
        );
    }
    assert!(editor.take_session_requests().is_empty());
}

/// In a session's terminal, `session-rename` asks about that buffer's session (ADR 0155).
#[test]
fn a_rename_in_a_sessions_buffer_names_the_buffer() {
    let mut editor = editor("");
    let session = editor.add_terminal_buffer("*claude: api*");
    editor.set_hosts_session(session, true);
    editor.show(session);
    assert_eq!(run(&mut editor, "session-rename", Args::NONE).0, Ok(()));
    assert_eq!(
        editor.take_session_requests(),
        [SessionRequest::OnBuffer(session, RowAction::Rename)]
    );
}

/// The prompt's answer goes to the host as typed, trimmed; an empty one asks nothing.
#[test]
fn the_new_name_goes_to_the_host() {
    use oso_core::command::{Ctx, Invocation};
    let registry = crate::commands::registry();
    let id = registry.lookup(super::RENAME_TO).unwrap();
    let answered = |answer| {
        let mut editor = editor("");
        let invocation = Invocation {
            answer: Some(answer),
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(&mut editor, invocation);
        let result = registry.execute(id, &mut ctx, Args::NONE);
        (result.is_ok(), editor.take_session_requests())
    };
    assert_eq!(
        answered(" api v2 "),
        (true, vec![SessionRequest::RenameTo("api v2".into())])
    );
    assert_eq!(answered("  "), (false, vec![]));
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

/// `*Spaces*` showing `text`, point on its first line.
fn showing(text: &str) -> Editor {
    let mut editor = editor("");
    let list = editor.add_internal_buffer("*Spaces*", Mode::Sessions);
    editor.set_internal_text(list, text);
    editor.show(list);
    editor
}

/// The lines `name` stops at, from the first line down, until it moves no more.
fn stops(editor: &mut Editor, name: &str) -> Vec<usize> {
    let mut lines = Vec::new();
    while run(editor, name, Args::NONE).1.redraw {
        lines.push(editor.cursor_line());
    }
    lines
}

#[test]
fn headings_are_spaces_worktrees_and_groups_in_the_tree() {
    let tree = "\
 Spaces                   ▶ 1
 ▾ oso ──────────────── ▶ 1
   ▾ main  primary    ▶  now
     ├ ▶ api              now
     │ └ ○ fork           2m
     │     waits for you
     └ ○ web              1h
   ▸ side  side-branch     3d
   ▾ 2 quiet
     ▸ old                9d
 ▾ Other projects ───────── 1
   ▾ elsewhere (1)
       ○ far              1h
 ▾ Archived ─────────────── 1
     ■ gone  oso          2d
";
    let mut editor = showing(tree);
    assert_eq!(
        stops(&mut editor, "sessions-next-heading"),
        [1, 2, 7, 8, 9, 10, 11, 13]
    );
    let back = stops(&mut editor, "sessions-previous-heading");
    assert_eq!(back, [11, 10, 9, 8, 7, 2, 1]);
    // `M-n` with a count, as `n` takes one.
    let mut editor = showing(tree);
    assert_eq!(
        run(&mut editor, "sessions-next-heading", Args::numeric(3)).0,
        Ok(())
    );
    assert_eq!(editor.cursor_line(), 7);
    // `n` stops at every row past its guides, the title and the reason line excepted.
    let mut editor = showing(tree);
    assert_eq!(
        stops(&mut editor, "sessions-next"),
        [1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14]
    );
}

#[test]
fn left_folds_or_goes_to_the_row_above_and_right_opens_or_goes_in() {
    let tree = "\
 Spaces
 ▾ oso ──────────────── ▶ 1
   ▾ main  primary    ▶  now
     ├ ▶ api              now
     │ └ ○ fork           2m
     └ ○ web              1h
   ▸ side  side-branch     3d
";
    let mut editor = showing(tree);
    let to = |editor: &mut Editor, line: usize| {
        let offset = editor.current().1.text().line_start(line);
        editor.current_mut().0.selections = SelectionSet::new(offset);
    };
    // On a nested session, the session it hangs from; on a session, its worktree.
    to(&mut editor, 4);
    assert_eq!(run(&mut editor, "sessions-close", Args::NONE).0, Ok(()));
    assert_eq!(editor.cursor_line(), 3);
    assert_eq!(run(&mut editor, "sessions-close", Args::NONE).0, Ok(()));
    assert_eq!(editor.cursor_line(), 2);
    // On an open row, a fold; on a folded one, the row above it.
    assert_eq!(run(&mut editor, "sessions-close", Args::NONE).0, Ok(()));
    to(&mut editor, 6);
    assert_eq!(run(&mut editor, "sessions-close", Args::NONE).0, Ok(()));
    assert_eq!(editor.cursor_line(), 1);
    // `<right>` on an open row goes in; on a folded one, it opens it.
    assert_eq!(run(&mut editor, "sessions-open", Args::NONE).0, Ok(()));
    assert_eq!(editor.cursor_line(), 2);
    to(&mut editor, 6);
    assert_eq!(run(&mut editor, "sessions-open", Args::NONE).0, Ok(()));
    assert_eq!(
        run(&mut editor, "sessions-cycle-folds", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        editor.take_session_requests(),
        [
            SessionRequest::Row {
                line: 2,
                action: RowAction::Fold
            },
            SessionRequest::Row {
                line: 6,
                action: RowAction::Fold
            },
            SessionRequest::CycleFolds,
        ]
    );
}

#[test]
fn by_state_the_headings_are_the_groups_not_nested_sessions() {
    let by_state = "\
▾ Working 2
▶ api  oso                now
  ▶ fork  oso             now
▶ web  site               2m
▾ Idle 1
○ docs  oso               1h
";
    let mut list = showing(by_state);
    assert_eq!(stops(&mut list, "sessions-next-heading"), [4]);
    let mut elsewhere = editor("text");
    let refused = run(&mut elsewhere, "sessions-next-heading", Args::NONE).0;
    assert_eq!(refused, Err("Not in *Spaces*".to_owned()));
}

#[test]
fn wider_and_narrower_ask_for_4_columns_a_step() {
    let mut list = listing();
    assert_eq!(run(&mut list, "sessions-wider", Args::NONE).0, Ok(()));
    assert_eq!(
        run(&mut list, "sessions-narrower", Args::numeric(3)).0,
        Ok(())
    );
    assert_eq!(
        run(&mut list, "sessions-wider", Args::numeric(isize::MIN)).0,
        Ok(())
    );
    assert_eq!(
        list.take_session_requests(),
        [
            SessionRequest::PaneWidth(4),
            SessionRequest::PaneWidth(-12),
            SessionRequest::PaneWidth(isize::MIN),
        ]
    );
    let mut elsewhere = editor("text");
    let refused = run(&mut elsewhere, "sessions-wider", Args::NONE).0;
    assert_eq!(refused, Err("Not in *Spaces*".to_owned()));
}

#[test]
fn in_a_space_of_several_repositories_their_rows_and_worktrees_are_headings() {
    let tree = "\
 ▾ fix-login ─────────── ⚠ 1
   ● orchestrator         1m
   ▾ backend/            ▶ 1
     ▾ fix  fix/api    ▶  now
       └ ▶ fix-api        now
   ▾ frontend/           ⚠ 1
     ▾ msg             ⚠  now
       └ ⚠ msg            now
           waits for you
     ▾ 2 quiet
       ▸ old              9d
 ▸ oso ───────────────── ▶ 3
 ▸ Archived spaces ────── 1
   ■ gone  An old issue
";
    let mut editor = showing(tree);
    assert_eq!(
        stops(&mut editor, "sessions-next-heading"),
        [2, 3, 5, 6, 9, 10, 11, 12]
    );
}
