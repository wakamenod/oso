//! Saving the buffer (ADR 0013, ADR 0059), and reading it again from its file (ADR 0083).
//!
//! Must not depend on: winit, the GPU, `bindings` or files; the command decides and
//! queues, and the host writes on a thread of its own.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
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
        reach: Reach::Change,
        run: save_buffer,
    },
    Command {
        name: "save-buffer-the-agent-opened",
        doc: "Save a buffer the agent channel opened, as the person's `y` to the agent's question does.\n\nRun on its own, it says that the question answers this (ADR 0085).",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: save_buffer_the_agent_opened,
    },
    Command {
        name: "revert-buffer",
        doc: "Read the buffer again from its file.\n\nA buffer with unsaved edits asks first, and `y` discards them. The text read goes in as one undo step, so undo brings the edits back. Cursors keep their place around what changed. The file is read in the background.",
        args: ArgSpec::None,
        // It discards edits after a question, which stays the person's.
        reach: Reach::Person,
        run: revert_buffer,
    },
];

/// The agent channel saves without asking only a buffer the person visited: any other asks
/// the person first, as a question that is the agent's (ADR 0085).
fn save_buffer_the_agent_opened(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new(
            "The agent's question about saving answers this",
        ));
    }
    start_save(ctx, false)
}

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
    let buffer = ctx.editor.current().1;
    if ctx.editor.runs_for_agent() && buffer.opened_by_agent() && !ctx.confirmed() {
        // Where the save writes, which a link may have put far from the name: the end of it
        // shows, home as `~`.
        let place = buffer
            .baseline()
            .map(|baseline| baseline.location.clone())
            .or_else(|| buffer.path().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        let question = format!("Save {}, which the agent opened?", shown_place(&place));
        return super::session::ask(
            ctx,
            question,
            "save-buffer-the-agent-opened",
            ["save", "keep"],
        );
    }
    let confirmed = ctx.confirmed();
    start_save(ctx, confirmed)
}

/// The cells of a place the agent's save question shows: its end, which names the file.
const PLACE_CELLS: usize = 48;

/// `place` as the person reads it: home as `~`, and only its last [`PLACE_CELLS`] cells,
/// after `…`, when it is longer.
fn shown_place(place: &std::path::Path) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let shown = match home
        .as_deref()
        .and_then(|home| place.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => place.display().to_string(),
    };
    let chars: Vec<char> = shown.chars().collect();
    if chars.len() <= PLACE_CELLS {
        return shown;
    }
    let tail: String = chars[chars.len() - PLACE_CELLS..].iter().collect();
    format!("…{tail}")
}

/// Starts saving the selected buffer, over a file changed on disk if `over`.
fn start_save(ctx: &mut Ctx<'_>, over: bool) -> Result<(), CommandError> {
    let started = if over {
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

#[cfg(test)]
mod tests {
    use super::super::tests::{editor, run};
    use oso_core::command::Args;

    #[test]
    fn the_agent_saves_a_buffer_it_opened_only_once_the_person_agrees() {
        let mut editor = editor("text");
        let (window, _) = editor.current_mut();
        let id = window.buffer();
        editor.buffer_mut(id).unwrap().mark_opened_by_agent();
        editor
            .for_agent(|editor| run(editor, "save-buffer", Args::NONE).0)
            .unwrap();
        let prompt = editor.prompt().unwrap();
        assert!(prompt.from_agent());
        assert!(
            prompt.label().contains("which the agent opened?"),
            "{}",
            prompt.label()
        );
        editor.quit();
        // The person's own save of it asks nothing.
        let _ = run(&mut editor, "save-buffer", Args::NONE);
        assert!(editor.prompt().is_none());
        // Nor does the agent's save of a buffer the person visited.
        let mut visited = super::super::tests::editor("text");
        let _ = visited.for_agent(|editor| run(editor, "save-buffer", Args::NONE));
        assert!(visited.prompt().is_none());
    }

    #[test]
    fn the_place_shows_its_end_and_home_as_a_tilde() {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap();
        assert_eq!(super::shown_place(&home.join("notes.md")), "~/notes.md");
        let long = std::path::PathBuf::from(format!("/{}/settings.json", "d".repeat(80)));
        let shown = super::shown_place(&long);
        assert!(
            shown.starts_with('…') && shown.ends_with("settings.json"),
            "{shown}"
        );
    }

    #[test]
    fn the_answer_alone_runs_nothing() {
        let mut editor = editor("text");
        assert!(
            run(&mut editor, "save-buffer-the-agent-opened", Args::NONE)
                .0
                .is_err()
        );
    }
}
