//! Commands that tell the person which keys oso received and what a key runs: `view-lossage`
//! (`C-h l`) and `describe-key` (`C-h k`) (ADR 0150). They only check where they run; the
//! dispatcher, which holds the keys and the keymaps, answers them after they succeed, and shows
//! their text with [`show_internal`].
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and does not know its keys.

use oso_core::buffer::{Buffer, BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::Editor;

/// The name of the command that shows the keys received last.
pub(crate) const VIEW_LOSSAGE: &str = "view-lossage";
/// The name of the command that reads a key sequence and says what it runs.
pub(crate) const DESCRIBE_KEY: &str = "describe-key";

/// What `describe-key` says where secrets may be, where it reads no key.
pub(crate) const SECRET: &str = "No key is described where secrets may be";

/// The commands for help.
pub const COMMANDS: &[Command] = &[
    Command {
        name: VIEW_LOSSAGE,
        doc: "Show the last 300 keys oso received in `*Lossage*`, each with the command it ran.\n\nThe newest is at the end. A key oso has no name for shows as `<no Emacs name>`. Keys typed in a buffer that may hold secrets or at a passphrase prompt show as `<hidden>`, as do characters typed into a terminal. The keys are kept only in memory, and the agent channel cannot read `*Lossage*`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: view_lossage,
        state: None,
    },
    Command {
        name: DESCRIBE_KEY,
        doc: "Read a key sequence and say what it runs, without running it.\n\nA bound sequence shows its command and that command's documentation in `*Help*`; an unbound one says so in the echo area, and `C-g` under a prefix quits.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: describe_key,
        state: None,
    },
];

fn view_lossage(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)
}

fn describe_key(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    // The keys it reads would be the person's typing there (ADR 0150).
    if ctx.editor.current().1.is_secret() || ctx.editor.around_secret() {
        return Err(CommandError::new(SECRET));
    }
    ctx.editor.echo("Describe the following key: ");
    Ok(())
}

/// Puts `text` in the internal buffer `kept` names, or in a new one named `name` if it is
/// gone, and shows it, withheld from the agent channel if `withhold` says so. Never a buffer
/// that is not internal: one of that name anyone made stays theirs. Whether it showed the text.
pub(crate) fn show_internal(
    editor: &mut Editor,
    kept: &mut Option<BufferId>,
    name: &str,
    text: &str,
    withhold: bool,
) -> bool {
    let id = match kept.filter(|id| editor.buffer(*id).is_some_and(Buffer::is_internal)) {
        Some(id) => id,
        None => editor.add_internal_buffer(name, Mode::Fundamental),
    };
    *kept = Some(id);
    let Some(buffer) = editor.buffer_mut(id) else {
        return false;
    };
    if withhold {
        buffer.withhold_from_agents();
    }
    editor.set_internal_text(id, text);
    // A report of some 20 KiB at most, compared once.
    let set = editor.buffer(id).is_some_and(|buffer| {
        buffer.text().len() == text.len() && buffer.text().to_string() == text
    });
    if set {
        editor.show(id);
    }
    set
}
