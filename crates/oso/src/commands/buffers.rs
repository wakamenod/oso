//! Commands that go between buffers (ADR 0030, ADR 0088).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The commands that go between buffers.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "previous-buffer",
        doc: "Show the buffer this window showed before this one, where it was there.\n\nRepeated, go further back, through every buffer but internal ones such as `*Messages*`. With a count, go that many; negative goes the other way.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous_buffer,
    },
    Command {
        name: "next-buffer",
        doc: "Go the other way from `previous-buffer`, so this after it comes back.\n\nWith a count, go that many; negative goes the other way.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next_buffer,
    },
];

fn previous_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    walk(ctx, args.count())
}

fn next_buffer(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    walk(ctx, args.count().saturating_neg())
}

fn walk(ctx: &mut Ctx<'_>, count: isize) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.walk_buffers(count).map_err(CommandError::new)?;
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::buffer::Buffer;
    use oso_core::command::Args;

    #[test]
    fn it_walks_back_and_forward_and_says_when_there_is_nothing_else() {
        let mut editor = editor("abc");
        let (result, _) = run(&mut editor, "previous-buffer", Args::NONE);
        assert_eq!(
            result.map_err(|e| e.to_string()),
            Err("No other buffer".to_owned())
        );
        let other = editor.add_buffer(Buffer::from_text("other", "x"));
        editor.show(other);
        let (result, effects) = run(&mut editor, "previous-buffer", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(effects.redraw);
        assert_eq!(editor.current().1.name(), "test");
        run(&mut editor, "next-buffer", Args::NONE).0.unwrap();
        assert_eq!(editor.current().1.name(), "other");
    }
}
