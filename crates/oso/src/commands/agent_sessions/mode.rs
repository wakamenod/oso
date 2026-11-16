//! Session mode's commands on the session whose terminal is the current buffer (ADR 0132):
//! pinning and archiving it, as `P` and `A` do on its row of `*Sessions*`, and saying session
//! mode's keys. `session-stop` is `list`'s, which acts on a session's buffer too.
//!
//! Must not depend on: winit, the GPU or `bindings`; the dispatcher writes the keys
//! `session-help` says, as only it holds the keymaps.
//!
//! Pinning and archiving change which sessions ask for the person: `Person`, as their rows'
//! are. Saying the keys only looks.

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{RowAction, SessionRequest};

/// The name of the command that says session mode's keys, which the dispatcher answers.
pub const SESSION_HELP: &str = "session-help";

/// What a command on the session in front says in a buffer that hosts none, and what the Agents
/// menu's heads for them say there (ADR 0143).
pub const NO_SESSION: &str = "This buffer holds no session";

/// Session mode's commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "session-pin",
        doc: "Pin the session whose terminal is this buffer, or unpin it: pinned sessions come first in their group of `*Sessions*`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: pin,
        state: None,
    },
    Command {
        name: "session-archive",
        doc: "Archive the session whose terminal is this buffer, or bring an archived one back.\n\nAn archived session is listed last, folded, and is not brought back at the next start.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: archive,
        state: None,
    },
    Command {
        name: SESSION_HELP,
        doc: "Say session mode's keys in the echo area: `C-c` and a key, in the terminal of a session oso hosts.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: help,
        state: None,
    },
];

/// The current buffer, if its terminal hosts a session.
pub(super) fn session_buffer(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    if !buffer.hosts_session() {
        return Err(CommandError::new(NO_SESSION));
    }
    Ok(window.buffer())
}

fn on_buffer(ctx: &mut Ctx<'_>, action: RowAction) -> Result<(), CommandError> {
    let buffer = session_buffer(ctx)?;
    ctx.editor
        .request_session(SessionRequest::OnBuffer(buffer, action));
    Ok(())
}

fn pin(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_buffer(ctx, RowAction::Pin)
}

fn archive(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_buffer(ctx, RowAction::Archive)
}

/// Only checks where it runs: the dispatcher says the keys once it succeeded.
fn help(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    session_buffer(ctx).map(|_| ())
}
