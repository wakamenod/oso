//! Commands that go between buffers (ADR 0030, ADR 0088).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the
//! host-owned state and does not know its keys.

use oso_core::command::{ArgSpec, Args, BufferPurpose, Command, CommandError, Ctx, Reach};
use oso_core::editor::KillRefused;
use oso_core::minibuffer;

/// How many cells of a buffer's name a question shows, so that its own words always do.
const NAME_CELLS: usize = 40;

/// The commands that go between buffers.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "switch-to-buffer",
        doc: "Show a buffer, read with completion, the one shown before first.\n\nA name that matches no buffer makes an empty one with no file; `M-RET` makes it when another matches.",
        args: ArgSpec::Buffer {
            prompt: "Switch to buffer:",
            history: "buffer",
            purpose: BufferPurpose::Switch,
        },
        reach: Reach::Look,
        run: switch_to_buffer,
    },
    Command {
        name: "kill-buffer",
        doc: "Kill a buffer, read with completion, the current one first.\n\nOne with unsaved text asks first. `*Messages*` and the other internal buffers are kept, and a buffer being saved is kept until that ends.",
        args: ArgSpec::Buffer {
            prompt: "Kill buffer:",
            history: "buffer",
            purpose: BufferPurpose::Kill,
        },
        reach: Reach::Person,
        run: kill_buffer,
    },
    Command {
        name: "kill-buffer-anyway",
        doc: "Kill the buffer a question asked about, with its unsaved text.\n\nIt is what `y` runs; it asks again if a save of it has started since.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: kill_buffer_anyway,
    },
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

fn switch_to_buffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let name = ctx.answer().unwrap_or_default().to_owned();
    ctx.editor.switch_to_named(&name);
    ctx.request_redraw();
    Ok(())
}

fn kill_buffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let name = ctx.answer().unwrap_or_default();
    let id = ctx
        .editor
        .buffer_named(name)
        .ok_or_else(|| CommandError::new("No such buffer"))?;
    kill(ctx, id, false)
}

fn kill_buffer_anyway(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let id = ctx
        .editor
        .kill_asked()
        .ok_or_else(|| CommandError::new("No buffer was asked about"))?;
    kill(ctx, id, ctx.confirmed())
}

/// Kills `id` by a person's rules, and asks about unsaved text unless `anyway`.
fn kill(
    ctx: &mut Ctx<'_>,
    id: oso_core::buffer::BufferId,
    anyway: bool,
) -> Result<(), CommandError> {
    let name = ctx
        .editor
        .buffer(id)
        .map(|buffer| buffer.name().to_owned())
        .unwrap_or_default();
    let (cut, more) = minibuffer::cut_to(&name, NAME_CELLS);
    match ctx.editor.kill_by_person(id, anyway) {
        Ok(()) => {
            ctx.request_redraw();
            Ok(())
        }
        Err(KillRefused::Modified) => {
            let question = format!("{cut}{more} is modified; kill it anyway?");
            super::session::ask(ctx, question, "kill-buffer-anyway", ["kill", "keep"])
        }
        Err(KillRefused::Internal) => Err(CommandError::new(format!("{cut}{more} is kept"))),
        Err(KillRefused::Saving) => Err(CommandError::new(format!(
            "{cut}{more} is being saved; kill it when that ends"
        ))),
        Err(KillRefused::Gone) => Err(CommandError::new("No such buffer")),
    }
}

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
