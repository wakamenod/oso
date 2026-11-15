//! Moving this instance to the binary installed in its place, with its terminals, sessions and
//! unsaved text (ADR 0133). The command only asks: the host checks the binary, starts it and
//! hands everything over, and says in the echo area if it stays.
//!
//! Must not depend on: processes, sockets or `bindings`; a command asks the host through the
//! editor.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The restart's command.
pub const COMMANDS: &[Command] = &[Command {
    name: "oso-restart",
    doc: "Move to the oso installed in this one's place, keeping terminals, sessions and unsaved text.\n\nStarts the binary this instance came from, once it is owned by you or root and no one else can change it or a directory above it, and hands it the buffers, the windows, the kill ring, the histories and every running terminal and session. Then this instance exits. If the new one does not start or take over, everything stays here and the terminals read again. A full-screen program's other screen is redrawn by it.",
    args: ArgSpec::None,
    reach: Reach::Person,
    run: restart,
    state: None,
}];

fn restart(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor
        .request_terminal(oso_core::editor::TerminalRequest::Restart);
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::command::Args;

    #[test]
    fn it_asks_the_host_once() {
        let mut editor = editor("text");
        let (result, _) = run(&mut editor, "oso-restart", Args::NONE);
        assert_eq!(result, Ok(()));
        assert_eq!(
            editor.take_terminal_requests(),
            [oso_core::editor::TerminalRequest::Restart]
        );
    }
}
