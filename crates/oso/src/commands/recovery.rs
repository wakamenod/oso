//! Commands for emergency copies: the list, and a person's answers (ADR 0064). A command has
//! no files or threads, so it says what was asked, and the host does it after the command.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::CopyAction;

/// The commands for emergency copies.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "recover-session",
        doc: "Show the emergency copies waiting for an answer, or go back from them.\n\nIn the list, `v` looks at a copy, `=` compares it with its file, `r` recovers it, and `d` deletes it. Nothing is written to a file until it is saved.",
        args: ArgSpec::None,
        run: recover_session,
    },
    Command {
        name: "emergency-copy-view",
        doc: "Show the text of the copy on this row, read-only.",
        args: ArgSpec::None,
        run: view,
    },
    Command {
        name: "emergency-copy-compare",
        doc: "Show how the copy on this row differs from its file.",
        args: ArgSpec::None,
        run: compare,
    },
    Command {
        name: "emergency-copy-recover",
        doc: "Put the copy's text in its file's buffer, once it has been shown.\n\nThe first time, this shows the text. Repeated there, it replaces the buffer's text as one undo step. The buffer is then modified, and the file is untouched until it is saved. The copy goes once that save has reached the disk.",
        args: ArgSpec::None,
        run: recover,
    },
    Command {
        name: "emergency-copy-delete",
        doc: "Delete the copy on this row, when repeated.\n\nThe first time, this says what the second does. Any other key cancels it. With several cursors, it takes each cursor's row. A copy deleted cannot be brought back.",
        args: ArgSpec::None,
        run: delete,
    },
    Command {
        name: "emergency-copies-refresh",
        doc: "Read the directory of emergency copies again, for what another instance has written.",
        args: ArgSpec::None,
        run: refresh,
    },
    Command {
        name: "emergency-copies-back",
        doc: "Go from a copy to the list, or from the list to where the window was.",
        args: ArgSpec::None,
        run: back,
    },
];

fn recover_session(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.toggle_copies();
    ctx.editor.queue_copy_action(CopyAction::Toggled);
    ctx.request_redraw();
    Ok(())
}

fn view(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor.queue_copy_action(CopyAction::View(line));
    ctx.request_redraw();
    Ok(())
}

fn compare(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor.queue_copy_action(CopyAction::Compare(line));
    Ok(())
}

fn recover(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let line = ctx.editor.cursor_line();
    ctx.editor.queue_copy_action(CopyAction::Recover(line));
    ctx.request_redraw();
    Ok(())
}

/// A second press in a row deletes. A third starts over, so that holding the key down, with
/// rows moving up as they go, cannot delete one nobody looked at.
fn delete(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.repeats().is_multiple_of(2) {
        ctx.editor
            .echo("Press d again to delete this emergency copy for good");
        return Ok(());
    }
    let mut lines: Vec<usize> = {
        let (window, buffer) = ctx.editor.current();
        window
            .selections
            .iter()
            .map(|selection| buffer.text().offset_to_point(selection.cursor).row)
            .collect()
    };
    lines.dedup();
    for line in lines {
        ctx.editor.queue_copy_action(CopyAction::Delete(line));
    }
    Ok(())
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.queue_copy_action(CopyAction::Refresh);
    Ok(())
}

fn back(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.copies_back();
    ctx.editor.queue_copy_action(CopyAction::WentBack);
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Mode;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{CopyAction, Editor};
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
        assert_eq!(editor.current().1.mode(), Mode::EmergencyCopies);
        assert_eq!(editor.take_copy_actions(), [CopyAction::Toggled]);
        let _ = run(&mut editor, "recover-session", Args::NONE);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(editor.take_copy_actions(), [CopyAction::Toggled]);
    }

    #[test]
    fn an_answer_says_which_line_it_is_for() {
        let mut editor = editor("a\nb\nc\n");
        editor.current_mut().0.selections = SelectionSet::new(4);
        for (name, action) in [
            ("emergency-copy-view", CopyAction::View(2)),
            ("emergency-copy-compare", CopyAction::Compare(2)),
            ("emergency-copy-recover", CopyAction::Recover(2)),
        ] {
            let _ = run(&mut editor, name, Args::NONE);
            assert_eq!(editor.take_copy_actions(), [action]);
        }
        let _ = run(&mut editor, "emergency-copies-refresh", Args::NONE);
        assert_eq!(editor.take_copy_actions(), [CopyAction::Refresh]);
    }

    #[test]
    fn the_first_d_only_says_what_the_second_does_and_a_third_starts_over() {
        let mut editor = editor("a\nb\n");
        let name = "emergency-copy-delete";
        press(&mut editor, name, 0);
        assert_eq!(editor.take_copy_actions(), []);
        press(&mut editor, name, 1);
        assert_eq!(editor.take_copy_actions(), [CopyAction::Delete(0)]);
        press(&mut editor, name, 2);
        assert_eq!(editor.take_copy_actions(), [], "the next row is not taken");
        press(&mut editor, name, 3);
        assert_eq!(editor.take_copy_actions(), [CopyAction::Delete(0)]);
    }

    #[test]
    fn a_delete_takes_each_cursors_row_once() {
        let mut editor = editor("aa\nbb\ncc\n");
        let cursors = vec![Selection::at(0), Selection::at(1), Selection::at(6)];
        let set = SelectionSet::from_parts(cursors, 0).unwrap();
        editor.current_mut().0.selections = set;
        press(&mut editor, "emergency-copy-delete", 1);
        assert_eq!(
            editor.take_copy_actions(),
            [CopyAction::Delete(0), CopyAction::Delete(2)]
        );
    }

    #[test]
    fn back_goes_to_where_the_window_was_and_tells_the_host() {
        let mut editor = editor("abc");
        let _ = run(&mut editor, "recover-session", Args::NONE);
        editor.take_copy_actions();
        let (_, effects) = run(&mut editor, "emergency-copies-back", Args::NONE);
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "test");
        assert_eq!(editor.take_copy_actions(), [CopyAction::WentBack]);
    }
}
