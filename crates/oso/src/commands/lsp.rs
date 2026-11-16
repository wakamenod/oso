//! Commands of the language server that follows the current buffer (ADR 0138, step 3):
//! starting it, starting it again, and stopping it; moving to its diagnostics (step 6), and
//! the list of the project's (step 6b); going to a definition, and back and forward through
//! the window's jumps (step 8); and describing what is at point in a float, which its own keys
//! scroll and close (step 10).
//!
//! Must not depend on: winit, the GPU, `bindings` or the host's servers; a command asks the
//! host through `Editor::request_server`, and the host acts on the server after it.
//!
//! Starting a server spawns a process, and stopping one manages the person's processes, so
//! those are `Person`; the moves only move, and a definition asks a running server about a
//! buffer it knows, so they are `Look`; the list and the description are interactive floats
//! for the keyboard, so they and the description's keys are `Person` (ADR 0138, Q24).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{DiagnosticHit, ServerRequest, VisitColumn};

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
    Command {
        name: "lsp-diagnostics",
        doc: "List the diagnostics of the current buffer's project and go to the one picked, as `consult-flymake`.\n\nThe current buffer's come first, by place, then each other file's by its path and line, from every language server of the project, whether a buffer holds the file or not; hints are left out. Each row reads `path:line: severity: message`, with the source and the code beside it, and what is typed narrows them as any completion does. The first diagnostic at or after point is selected. The preview shows the selected row's place, from its buffer or read from its file, but nothing of a file that may hold secrets. The list holds what the servers had reported when it opened. `RET` goes there, with the place before on the mark ring.",
        args: ArgSpec::Diagnostics {
            prompt: "Diagnostics",
        },
        reach: Reach::Person,
        run: diagnostics,
        state: None,
    },
    Command {
        name: "lsp-describe-at-point",
        doc: "Describe what is at point, as the language server sees it, as Eglot's `eglot-help-at-point`.\n\nThe server's text shows in a float right below the cursor's line, or above it with more room there, at most 80 columns by 20 rows, Markdown highlighted with Rust in its code. `C-v` and `M-v` scroll it, `C-g` closes it, and any other key closes it and does what it always does. It shows only if nothing moved while the server answered. A buffer that may hold secrets is never described.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: describe_at_point,
        state: None,
    },
    Command {
        name: "lsp-describe-scroll-up",
        doc: "Scroll the description at point forward by its rows, less two, as `C-v` scrolls a window.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: describe_scroll_up,
        state: None,
    },
    Command {
        name: "lsp-describe-scroll-down",
        doc: "Scroll the description at point back by its rows, less two, as `M-v` scrolls a window.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: describe_scroll_down,
        state: None,
    },
    Command {
        name: "lsp-describe-close",
        doc: "Close the description at point, and nothing more.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: describe_close,
        state: None,
    },
    Command {
        name: "lsp-find-definition",
        doc: "Go to the definition of what is at point, as the language server finds it, as Eglot's `xref-find-definitions`.\n\nThe server is asked about the primary cursor; one cursor is left at the definition. With several, it goes to the first and says how many there are. The place left is kept in the window's history, which `lsp-go-back` goes back through, and on the mark ring. A file outside any project that it leads to joins the server, until its buffer is killed.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: find_definition,
        state: None,
    },
    Command {
        name: "lsp-go-back",
        doc: "Go back to the place the last jump to a definition left, as Emacs's `xref-go-back`.\n\nEach window keeps its own history of 64 places; `lsp-go-forward` goes forward again.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: go_back,
        state: None,
    },
    Command {
        name: "lsp-go-forward",
        doc: "Go forward again to the place the last `lsp-go-back` left, as Emacs's `xref-go-forward`.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: go_forward,
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

/// Goes to the diagnostic the list's prompt gave: in its open buffer, or in its file once
/// visited, through the loader's verdict, with the place before on the mark ring.
fn diagnostics(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let Some(hit) = ctx.editor.take_diagnostic_hit() else {
        return Err(CommandError::new("No diagnostic was picked"));
    };
    let (path, line, column) = match hit {
        DiagnosticHit::Buffer { id, offset } => {
            let buffer = ctx.editor.buffer(id);
            let Some((buffer, path)) = buffer.and_then(|b| Some((b, b.path()?.to_path_buf())))
            else {
                return Err(CommandError::new("The diagnostic's buffer has gone"));
            };
            let point = buffer.text().offset_to_point(offset);
            (path, point.row + 1, VisitColumn::Utf8(point.column))
        }
        DiagnosticHit::File { path, line, column } => {
            if ctx.editor.large_file(&path).is_some() {
                return super::files::visit(ctx, path);
            }
            (path, line, column)
        }
    };
    ctx.editor
        .request_visit_at_column(path, oso_core::load::UNASKED, line, column);
    ctx.request_redraw();
    Ok(())
}

fn find_definition(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let buffer = ctx.editor.current().1;
    if buffer.is_secret() {
        return Err(CommandError::new(
            "No server for a buffer that may hold secrets",
        ));
    }
    // The host says whether a server follows it.
    let at = ctx.editor.ask_at();
    ctx.editor.request_server(ServerRequest::Definition(at));
    Ok(())
}

/// Asks the server about the primary cursor's place; its answer shows in a float there.
fn describe_at_point(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if ctx.editor.current().1.is_secret() {
        return Err(CommandError::new(
            "No server for a buffer that may hold secrets",
        ));
    }
    // The host says whether a server follows it.
    let at = ctx.editor.ask_at();
    ctx.editor.request_server(ServerRequest::Describe(at));
    Ok(())
}

fn describe_scroll_up(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    scroll_description(ctx, true)
}

fn describe_scroll_down(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    scroll_description(ctx, false)
}

/// Scrolls the description a page, `forward` or back, saying so at either end, as a window's
/// scroll does.
fn scroll_description(ctx: &mut Ctx<'_>, forward: bool) -> Result<(), CommandError> {
    if ctx.editor.hover().is_none() {
        return Err(CommandError::new("No description at point"));
    }
    let page = isize::try_from(ctx.editor.hover_page()).unwrap_or(isize::MAX);
    if !ctx.editor.scroll_hover(if forward { page } else { -page }) {
        let end = if forward {
            "End of the description"
        } else {
            "Start of the description"
        };
        return Err(CommandError::new(end));
    }
    ctx.request_redraw();
    Ok(())
}

fn describe_close(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.close_hover();
    ctx.request_redraw();
    Ok(())
}

fn go_back(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.xref_back().map_err(CommandError::new)?;
    ctx.request_redraw();
    Ok(())
}

fn go_forward(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.xref_forward().map_err(CommandError::new)?;
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
