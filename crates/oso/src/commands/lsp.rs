//! Commands of the language server that follows the current buffer (ADR 0138, step 3):
//! starting it, starting it again, and stopping it.
//!
//! Must not depend on: winit, the GPU, `bindings` or the host's servers; a command asks the
//! host through `Editor::request_server`, and the host acts on the server after it.
//!
//! Each is `Person`: starting a server spawns a process, and stopping one manages the
//! person's processes (ADR 0138, Q24).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ServerRequest;

/// The commands of language servers.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "lsp-start",
        doc: "Start the language server of the current buffer's mode for its project.\n\nThe project must be trusted. The server comes from oso's own table, with the changes `[lsp.servers]` in `config.toml` makes. With `[lsp] start = \"manual\"`, this is the only way a server starts; it then runs for the rest of the session. After `lsp-shutdown`, it starts the server again.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: start,
        state: None,
    },
    Command {
        name: "lsp-restart",
        doc: "Stop the language server of the current buffer and start it again, with `config.toml` as it is now.\n\nIt also starts one that stopped after crashing three times, or that could not start. `*LSP*` says what the servers did.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: restart,
        state: None,
    },
    Command {
        name: "lsp-shutdown",
        doc: "Stop the language server of the current buffer.\n\nIt is asked to shut down, then told to exit, then sent `SIGTERM` and at last `SIGKILL`, a few seconds apart, with what it started. It stays stopped for the project until `lsp-start` or `lsp-restart`.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: shut_down,
        state: None,
    },
];

fn start(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, ServerRequest::Start)
}

fn restart(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, ServerRequest::Restart)
}

fn shut_down(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, ServerRequest::ShutDown)
}

/// Asks the host for `request` about the current buffer, which must be a file's.
fn ask(
    ctx: &mut Ctx<'_>,
    request: fn(oso_core::buffer::BufferId) -> ServerRequest,
) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let id = ctx.editor.window().buffer();
    if ctx.editor.current().1.path().is_none() {
        return Err(CommandError::new(
            "No language server for a buffer with no file",
        ));
    }
    ctx.editor.request_server(request(id));
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Buffer;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, ServerRequest};
    use oso_core::minibuffer::Kind;

    fn run(editor: &mut Editor, name: &str) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::invoked(editor, Invocation::default());
        registry
            .execute(id, &mut ctx, Args::NONE)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn each_asks_the_host_about_the_current_files_buffer() {
        let mut editor = Editor::with_buffer(Buffer::visiting("/w/p/src/main.rs".into()));
        let id = editor.window().buffer();
        for name in ["lsp-start", "lsp-restart", "lsp-shutdown"] {
            run(&mut editor, name).unwrap();
        }
        assert_eq!(
            editor.take_server_requests(),
            [
                ServerRequest::Start(id),
                ServerRequest::Restart(id),
                ServerRequest::ShutDown(id)
            ]
        );
    }

    #[test]
    fn a_buffer_with_no_file_or_a_prompt_asks_nothing() {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes", "x"));
        assert_eq!(
            run(&mut editor, "lsp-start"),
            Err("lsp-start: No language server for a buffer with no file".to_owned())
        );
        let mut editor = Editor::with_buffer(Buffer::visiting("/w/p/src/main.rs".into()));
        editor
            .open_prompt("Go to line: ", Kind::Number { history: None })
            .unwrap();
        assert!(run(&mut editor, "lsp-shutdown").is_err());
        assert!(editor.take_server_requests().is_empty());
    }
}
