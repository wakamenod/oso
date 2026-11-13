//! query-replace (ADR 0113): the command that reads its strings and begins it, and the
//! answers at each match, which its keymap binds. All are `Person`: they read keys.
//!
//! Must not depend on: winit, the GPU or `bindings`.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ReplaceAnswer;

/// An answer's command: what it does, at a match of a query-replace under way.
macro_rules! answer {
    ($name:literal, $doc:literal, $run:ident, $answer:expr) => {
        Command {
            name: $name,
            doc: $doc,
            args: ArgSpec::None,
            reach: Reach::Person,
            run: |ctx, _| $run(ctx, $answer),
            state: None,
        }
    };
}

/// The query-replace commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "query-replace",
        doc: "Replace a string, asking at each match.\n\nReads the string and its replacement, and an empty string takes the last pair. It searches from the cursor to the end of the buffer, or within the regions shown, with smart case: case-sensitive once the string holds an uppercase letter. While case folds, the replacement takes the match's case. At each match, `y` or `SPC` replaces it, `n` or `DEL` skips it, `!` replaces the rest, `.` replaces it and ends, `,` replaces it and stays, `^` goes back, `u` undoes the last replacement, `q` or `RET` ends, and `?` shows the keys. Any other key ends it and runs. The whole session undoes as one step.",
        args: ArgSpec::Replace { regexp: false },
        reach: Reach::Person,
        run: query_replace,
        state: None,
    },
    Command {
        name: "query-replace-regexp",
        doc: "Replace a regexp's matches, asking at each.\n\nAs `query-replace`, with a regexp in the `regex` crate's syntax, matched within one line at a time: `^` and `$` are a line's start and end, and no match crosses a line break. The replacement takes `$1`, `${name}` and `$0`, and `$$` for a dollar. The search is case-sensitive once the regexp holds an uppercase letter outside an escape, and a `(?i)` in it wins. A regexp that does not compile keeps the prompt open, saying why. A line over 64 MiB ends the search at it.",
        args: ArgSpec::Replace { regexp: true },
        reach: Reach::Person,
        run: query_replace_regexp,
        state: None,
    },
    answer!(
        "replace-this",
        "Replace this match, and go to the next.",
        reply,
        ReplaceAnswer::Replace
    ),
    answer!(
        "replace-skip",
        "Skip this match, and go to the next.",
        reply,
        ReplaceAnswer::Skip
    ),
    answer!(
        "replace-all",
        "Replace this match and every one left, without asking.",
        reply,
        ReplaceAnswer::All
    ),
    answer!(
        "replace-this-and-end",
        "Replace this match, and end.",
        reply,
        ReplaceAnswer::ReplaceAndExit
    ),
    answer!(
        "replace-this-and-stay",
        "Replace this match, and stay at it.",
        reply,
        ReplaceAnswer::ReplaceAndStay
    ),
    answer!(
        "replace-back",
        "Go back to the match before, without undoing it.",
        reply,
        ReplaceAnswer::Back
    ),
    answer!(
        "replace-undo",
        "Undo the last replacement, and go back to its match.",
        reply,
        ReplaceAnswer::Undo
    ),
    answer!(
        "replace-end",
        "End query-replace here.",
        reply,
        ReplaceAnswer::Exit
    ),
    answer!(
        "replace-help",
        "Show query-replace's keys.",
        reply,
        ReplaceAnswer::Help
    ),
];

fn query_replace(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (from, to) = ctx
        .replace_pair()
        .ok_or_else(|| CommandError::new("No string to replace"))?;
    ctx.editor
        .replace_begin(from, to, false)
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn query_replace_regexp(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (from, to) = ctx
        .replace_pair()
        .ok_or_else(|| CommandError::new("No regexp to replace"))?;
    ctx.editor
        .replace_begin(from, to, true)
        .map_err(|refused| CommandError::new(refused.to_string()))?;
    ctx.request_redraw();
    Ok(())
}

fn reply(ctx: &mut Ctx<'_>, answer: ReplaceAnswer) -> Result<(), CommandError> {
    if ctx.editor.replace().is_none() {
        return Err(CommandError::new("No query-replace under way"));
    }
    ctx.editor.replace_answer(answer);
    ctx.request_redraw();
    Ok(())
}
