//! Saving the buffer (ADR 0013, ADR 0059), and reading it again from its file (ADR 0083).
//!
//! Must not depend on: winit, the GPU, `bindings` or files; the command decides and
//! queues, and the host writes on a thread of its own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::SaveRefused;
use oso_core::minibuffer;

/// The most cells of a file's name a question says.
const NAME_CELLS: usize = 40;

/// The save commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "save-buffer",
        doc: "Save the buffer to its file, if it is modified.\n\nEnsures a final newline first, as an edit of its own. The file is written in the background, and replaced at once and whole. If it has changed on disk since it was read, it is kept and the buffer stays modified, and the next save asks whether to save over it. `y` replaces it only if it is still the file that save found. A second save while one runs is refused.",
        args: ArgSpec::None,
        run: save_buffer,
    },
    Command {
        name: "revert-buffer",
        doc: "Read the buffer again from its file.\n\nA buffer with unsaved edits asks first, and `y` discards them. The text read goes in as one undo step, so undo brings the edits back. Cursors keep their place around what changed. The file is read in the background.",
        args: ArgSpec::None,
        run: revert_buffer,
    },
];

fn revert_buffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = ctx.editor.current().1;
    if buffer.path().is_none() || buffer.is_internal() {
        return Err(CommandError::new(
            "Buffer does not seem to be associated with any file",
        ));
    }
    if buffer.loading().is_some() {
        return Err(CommandError::new("The file is still loading"));
    }
    if buffer.is_modified() && !ctx.confirmed() {
        let (name, cut) = minibuffer::cut_to(buffer.name(), NAME_CELLS);
        let question = format!("Discard your edits and reload {name}{cut}?");
        return super::session::ask(ctx, question, "revert-buffer", ["reload", "keep"]);
    }
    ctx.editor.request_revert();
    Ok(())
}

fn save_buffer(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let started = if ctx.confirmed() {
        ctx.editor.start_save_over()
    } else {
        ctx.editor.start_save()
    };
    match started {
        Ok(edited) => {
            if edited {
                ctx.request_redraw();
            }
            Ok(())
        }
        // Not a failure: there is just nothing to do, as in Emacs.
        Err(SaveRefused::Unmodified) => {
            ctx.editor.message(SaveRefused::Unmodified.to_string());
            Ok(())
        }
        Err(SaveRefused::ChangedOnDisk) => {
            // The name comes from a file, so it is cut to 40 cells. It follows the question's
            // opening, which a float too small for the whole label keeps first with as much of
            // the name as its row holds.
            let (name, cut) = minibuffer::cut_to(ctx.editor.current().1.name(), NAME_CELLS);
            let question = format!("Save over {name}{cut}? It changed on disk since it was read.");
            super::session::ask(ctx, question, "save-buffer", ["save over", "keep"])
        }
        Err(refused) => Err(CommandError::new(refused.to_string())),
    }
}
