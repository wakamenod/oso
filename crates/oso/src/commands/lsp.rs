//! Commands of the language server that follows the current buffer (ADR 0138, step 3):
//! starting it, starting it again, and stopping it; and moving to its diagnostics (step 6).
//!
//! Must not depend on: winit, the GPU, `bindings` or the host's servers; a command asks the
//! host through `Editor::request_server`, and the host acts on the server after it.
//!
//! Starting a server spawns a process, and stopping one manages the person's processes, so
//! those are `Person`; the moves only move, so they are `Look` (ADR 0138, Q24).

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
    Command {
        name: "lsp-next-diagnostic",
        doc: "Move to the start of the next diagnostic the language server reported, as Flycheck's `flycheck-next-error`.\n\nPast the last it wraps to the first, and says `Wrapped`. Hints are skipped. One cursor is left, from the primary, and no mark is pushed.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: next_diagnostic,
        state: None,
    },
    Command {
        name: "lsp-previous-diagnostic",
        doc: "Move to the start of the previous diagnostic the language server reported, as Flycheck's `flycheck-previous-error`.\n\nBefore the first it wraps to the last, and says `Wrapped`. Hints are skipped. One cursor is left, from the primary, and no mark is pushed.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: previous_diagnostic,
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

fn next_diagnostic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    to_diagnostic(ctx, true)
}

fn previous_diagnostic(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    to_diagnostic(ctx, false)
}

/// Moves the primary cursor, alone, to the next diagnostic or the previous one, wrapping past
/// the end (ADR 0138, Q15); the editor works out where.
fn to_diagnostic(ctx: &mut Ctx<'_>, forward: bool) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let wrapped = ctx
        .editor
        .to_diagnostic(forward)
        .map_err(CommandError::new)?;
    if wrapped {
        ctx.editor.echo("Wrapped");
    }
    ctx.request_redraw();
    Ok(())
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

    /// An editor with one followed buffer of `text`, with errors at 4, 10 and 20, a warning at
    /// 10 and a hint at 15.
    fn diagnosed(text: &str) -> Editor {
        use oso_core::diagnostic::{Diagnostics, New, Severity, Tags};
        let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", text));
        let id = editor.window().buffer();
        let buffer = editor.buffer_mut(id).unwrap();
        buffer.set_followed(true);
        let new = |at: usize, severity| New {
            range: at..at + 1,
            severity,
            tags: Tags::default(),
            source: "",
            code: "",
            message: "m",
        };
        let set = Diagnostics::new(
            "fake",
            [
                new(4, Severity::Error),
                new(10, Severity::Warning),
                new(10, Severity::Error),
                new(15, Severity::Hint),
                new(20, Severity::Error),
            ],
            text.len(),
        );
        let revision = buffer.revision();
        assert!(buffer.take_diagnostics(revision, set));
        editor
    }

    fn cursors(editor: &Editor) -> Vec<usize> {
        editor
            .window()
            .selections
            .iter()
            .map(|selection| selection.cursor)
            .collect()
    }

    #[test]
    fn the_moves_go_to_each_start_skip_hints_and_wrap() {
        let mut editor = diagnosed(&"x".repeat(30));
        for expected in [4, 10, 20] {
            run(&mut editor, "lsp-next-diagnostic").unwrap();
            assert_eq!(cursors(&editor), [expected]);
        }
        run(&mut editor, "lsp-next-diagnostic").unwrap();
        assert_eq!(cursors(&editor), [4]);
        let mut status = oso_core::editor::Status::default();
        editor.status_into(editor.tiled().0.id(), &mut status);
        assert_eq!(status.echo, "Wrapped");
        run(&mut editor, "lsp-previous-diagnostic").unwrap();
        assert_eq!(cursors(&editor), [20], "wrapped back");
        run(&mut editor, "lsp-previous-diagnostic").unwrap();
        assert_eq!(cursors(&editor), [10]);
    }

    #[test]
    fn a_move_leaves_one_cursor_and_pushes_no_mark() {
        use oso_core::selection::{Selection, SelectionSet};
        let mut editor = diagnosed(&"x".repeat(30));
        editor.current_mut().0.selections = SelectionSet::from_parts(
            vec![Selection::at(1), Selection::at(12), Selection::at(25)],
            1,
        )
        .unwrap();
        run(&mut editor, "lsp-next-diagnostic").unwrap();
        assert_eq!(cursors(&editor), [20], "from the primary");
        assert!(
            editor
                .window()
                .selections
                .mark_ring()
                .iter()
                .next()
                .is_none()
        );
        assert_eq!(editor.window().selections.primary().mark, None);
    }

    #[test]
    fn with_no_diagnostics_or_in_a_secret_buffer_nothing_moves() {
        let mut editor = Editor::with_buffer(Buffer::from_text("t.rs", "abc"));
        assert_eq!(
            run(&mut editor, "lsp-next-diagnostic"),
            Err("lsp-next-diagnostic: No diagnostics".to_owned())
        );
        let path = std::path::PathBuf::from("/p/.env");
        let mut secret = Buffer::visiting(path.clone());
        secret.judge_secret(Some(&[path]));
        let mut editor = Editor::with_buffer(secret);
        assert_eq!(
            run(&mut editor, "lsp-previous-diagnostic"),
            Err("lsp-previous-diagnostic: No server for a buffer that may hold secrets".to_owned())
        );
        assert_eq!(cursors(&editor), [0]);
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
