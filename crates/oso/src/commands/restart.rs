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
    doc: "Move to the oso installed in this one's place, keeping terminals, sessions and unsaved text.\n\nStarts the binary this instance came from, once it is owned by you or root and no one else can change it or a directory above it, and hands it the buffers, the windows, the kill ring, the histories and every running terminal and session. Then this instance exits. If the new one does not start or take over, everything stays here and the terminals read again. A full-screen program's other screen is redrawn by it. While git commits, it asks first, as the commit ends with this instance. While a native session has a turn under way, asks you something, is starting or is being interrupted, or while git or a file operation runs, it waits until nothing has for 2 seconds, sending nothing queued meanwhile, and the echo area names what it waits for: `C-g` where oso has the keys, `C-x C-c` or `quit-when-idle` cancels the wait.",
    args: ArgSpec::None,
    reach: Reach::Person,
    run: restart,
    state: None,
}];

fn restart(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    // git's runs end with this instance, as with `C-x C-c`, which asks first (ADR 0127, Q7).
    let runs: Vec<_> = ctx.editor.git_runs().collect();
    if !runs.is_empty() && !ctx.confirmed() {
        let question = format!(
            "git runs in {}; restart anyway?",
            super::session::names(ctx.editor, &runs)
        );
        return super::session::ask(ctx, question, "oso-restart", ["restart", "stay"]);
    }
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
    fn a_restart_while_git_runs_asks_first() {
        let mut editor = editor("text");
        let log = editor.add_internal_buffer("*git log: p*", oso_core::buffer::Mode::GitLog);
        editor.set_git_running(log, true);
        let (result, _) = run(&mut editor, "oso-restart", Args::NONE);
        assert_eq!(result, Ok(()));
        assert!(editor.take_terminal_requests().is_empty());
        assert_eq!(
            editor.prompt().expect("a question").label(),
            "git runs in *git log: p*; restart anyway? (y or n)"
        );
    }

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
