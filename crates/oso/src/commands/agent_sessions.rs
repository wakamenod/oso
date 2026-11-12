//! Commands for agent sessions (ADR 0121): so far, explaining a saved screen with the rules in
//! force, for writing a rule. `session.rs` beside it is the editing session's: cancelling and
//! exiting.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host, whose sessions worker reads the file.

use std::path::PathBuf;

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SessionRequest;

/// The sessions commands.
pub const COMMANDS: &[Command] = &[Command {
    name: "session-explain-file",
    doc: "Explain a saved screen of a hosted session, its path read with completion, with the rules in force.\n\n`*Session explain*` shows the state the rules give, the rule that matched and whether it is bundled or local, the region's text, or why no rule matched. It reads at most 256 KiB, the last 40 non-empty lines, for writing a rule in `agent-rules/claude.toml` beside `config.toml`. Only the person runs it: the screen may hold what an agent must not read.",
    args: ArgSpec::File {
        prompt: "Explain screen file:",
        history: "file",
    },
    reach: Reach::Person,
    run: session_explain_file,
}];

fn session_explain_file(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let path = ctx.answer().unwrap_or_default();
    if path.is_empty() {
        return Err(CommandError::new("No file to explain"));
    }
    ctx.editor
        .request_session(SessionRequest::ExplainFile(PathBuf::from(path)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, SessionRequest};

    use super::super::tests::editor;

    fn explain(editor: &mut Editor, answer: &str) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup("session-explain-file").unwrap();
        let invocation = Invocation {
            answer: Some(answer),
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        registry
            .execute(id, &mut ctx, Args::NONE)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn the_path_read_goes_to_the_host() {
        let mut editor = editor("");
        assert_eq!(explain(&mut editor, "/tmp/screen.txt"), Ok(()));
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::ExplainFile(PathBuf::from(
                "/tmp/screen.txt"
            ))]
        );
    }

    #[test]
    fn no_path_asks_nothing() {
        let mut editor = editor("");
        assert!(explain(&mut editor, "").unwrap_err().contains("No file"));
        assert!(editor.take_session_requests().is_empty());
    }
}
