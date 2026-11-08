//! Saving the buffer (ADR 0013, ADR 0059).
//!
//! Must not depend on: winit, the GPU, `bindings` or files; the command decides and
//! queues, and the host writes on a thread of its own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx};
use oso_core::editor::SaveRefused;
use oso_core::minibuffer;

/// The most cells of a file's name a question says.
const NAME_CELLS: usize = 40;

/// The save commands.
pub const COMMANDS: &[Command] = &[Command {
    name: "save-buffer",
    doc: "Save the buffer to its file, if it is modified.\n\nEnsures a final newline first, as an edit of its own. The file is written in the background, and replaced at once and whole. If it has changed on disk since it was read, it is kept and the buffer stays modified, and the next save asks whether to save over it. `y` replaces it only if it is still the file that save found. A second save while one runs is refused.",
    args: ArgSpec::None,
    run: save_buffer,
}];

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
            // The name comes from a file, so it is cut first: the question's own words and
            // its `(y or n)` always show.
            let (name, cut) = minibuffer::cut_to(ctx.editor.current().1.name(), NAME_CELLS);
            let question = format!("{name}{cut} changed on disk since it was read; save over it?");
            super::session::ask(ctx, question, "save-buffer")
        }
        Err(refused) => Err(CommandError::new(refused.to_string())),
    }
}
