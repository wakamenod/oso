//! Commands for emergency copies: the list, and a person's answers (ADR 0064). A command has
//! no files or threads, so it says what was asked, and the host does it after the command.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::RecoveryAction;

/// The commands for emergency copies.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "recover-session",
        doc: "Show the emergency copies waiting for an answer, or go back from them.\n\nIn the list, `v` looks at a copy, `=` compares it with its file, `r` recovers it, and `d` deletes it. Nothing is written to a file until it is saved.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: recover_session,
        state: None,
    },
    Command {
        name: "emergency-copy-view",
        doc: "Show the text of the copy on this row, read-only.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: view,
        state: None,
    },
    Command {
        name: "emergency-copy-compare",
        doc: "Show how the copy on this row differs from its file.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: compare,
        state: None,
    },
    Command {
        name: "emergency-copy-recover",
        doc: "Put the copy's text in its file's buffer, once it has been shown.\n\nThe first time, this shows the text. Repeated there, it replaces the buffer's text as one undo step. The buffer is then modified, and the file is untouched until it is saved. The copy goes once that save has reached the disk.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: recover,
        state: None,
    },
    Command {
        name: "emergency-copy-delete",
        doc: "Delete the copy on this row, when repeated.\n\nThe first time, this says what the second does. Any other key cancels it. With several cursors, it takes each cursor's row. A copy deleted cannot be brought back.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: delete,
        state: None,
    },
    Command {
        name: "emergency-copies-refresh",
        doc: "Read the directory of emergency copies again, for what another instance has written.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: refresh,
        state: None,
    },
    Command {
        name: "emergency-copies-back",
        doc: "Go from a copy to the list, or from the list to where the window was.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: back,
        state: None,
    },
];

fn recover_session(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.toggle_copies();
    ctx.editor.queue_recovery_action(RecoveryAction::Toggled);
    ctx.request_redraw();
    Ok(())
}

fn view(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor.queue_recovery_action(RecoveryAction::View(line));
    Ok(())
}

fn compare(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor
        .queue_recovery_action(RecoveryAction::Compare(line));
    Ok(())
}

fn recover(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor
        .queue_recovery_action(RecoveryAction::Recover(line));
    Ok(())
}

/// A second press in a row deletes what the first marked. A third starts over, and what a press
/// is held for arrives only once (`app::input`), so a held key cannot go on through the rows.
fn delete(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let mut lines: Vec<usize> = {
        let (window, buffer) = ctx.editor.current();
        window
            .selections
            .iter()
            .map(|selection| buffer.text().offset_to_point(selection.cursor).row)
            .collect()
    };
    lines.dedup();
    let first = ctx.repeats().is_multiple_of(2);
    if first {
        ctx.editor
            .queue_recovery_action(RecoveryAction::ForgetMarks);
    }
    for line in lines {
        ctx.editor.queue_recovery_action(if first {
            RecoveryAction::MarkForDelete(line)
        } else {
            RecoveryAction::Delete(line)
        });
    }
    Ok(())
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.queue_recovery_action(RecoveryAction::Refresh);
    Ok(())
}

fn back(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.copies_back();
    ctx.editor.queue_recovery_action(RecoveryAction::WentBack);
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Mode;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, RecoveryAction};
    use oso_core::selection::{Selection, SelectionSet};

    /// Runs `name`, as the `repeats`-th press in a row.
    fn press(editor: &mut Editor, name: &str, repeats: usize) {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let invocation = Invocation {
            repeats,
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        registry.execute(id, &mut ctx, Args::NONE).unwrap();
    }

    #[test]
    fn the_list_is_opened_and_closed_and_the_host_is_told_and_it_draws() {
        let mut editor = editor("abc");
        let (result, effects) = run(&mut editor, "recover-session", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!(editor.current().1.mode(), Mode::CopyList);
        assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Toggled]);
        let _ = run(&mut editor, "recover-session", Args::NONE);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Toggled]);
    }

    #[test]
    fn an_answer_says_which_line_it_is_for() {
        let mut editor = editor("a\nb\nc\n");
        editor.current_mut().0.selections = SelectionSet::new(4);
        for (name, action) in [
            ("emergency-copy-view", RecoveryAction::View(2)),
            ("emergency-copy-compare", RecoveryAction::Compare(2)),
            ("emergency-copy-recover", RecoveryAction::Recover(2)),
        ] {
            let _ = run(&mut editor, name, Args::NONE);
            assert_eq!(editor.take_recovery_actions(), [action]);
        }
        let _ = run(&mut editor, "emergency-copies-refresh", Args::NONE);
        assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Refresh]);
    }

    #[test]
    fn the_first_d_marks_the_second_deletes_and_a_third_starts_over() {
        let mut editor = editor("a\nb\n");
        let name = "emergency-copy-delete";
        let marks = [
            RecoveryAction::ForgetMarks,
            RecoveryAction::MarkForDelete(0),
        ];
        press(&mut editor, name, 0);
        assert_eq!(editor.take_recovery_actions(), marks);
        press(&mut editor, name, 1);
        assert_eq!(editor.take_recovery_actions(), [RecoveryAction::Delete(0)]);
        press(&mut editor, name, 2);
        assert_eq!(editor.take_recovery_actions(), marks, "not the next row");
    }

    #[test]
    fn a_delete_takes_each_cursors_row_once() {
        let mut editor = editor("aa\nbb\ncc\n");
        let cursors = vec![Selection::at(0), Selection::at(1), Selection::at(6)];
        let set = SelectionSet::from_parts(cursors, 0).unwrap();
        editor.current_mut().0.selections = set;
        press(&mut editor, "emergency-copy-delete", 1);
        assert_eq!(
            editor.take_recovery_actions(),
            [RecoveryAction::Delete(0), RecoveryAction::Delete(2)]
        );
    }

    #[test]
    fn back_goes_to_where_the_window_was_and_tells_the_host() {
        let mut editor = editor("abc");
        let _ = run(&mut editor, "recover-session", Args::NONE);
        editor.take_recovery_actions();
        let (_, effects) = run(&mut editor, "emergency-copies-back", Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(editor.take_recovery_actions(), [RecoveryAction::WentBack]);
    }
}
