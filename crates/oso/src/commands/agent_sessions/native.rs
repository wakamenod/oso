//! The keys of a native session's buffer (ADR 0165, step 1; ADR 0130's design): `TAB` folds
//! the row at point, `RET` shows a call's whole result, `n`/`p` move between the person's
//! messages and `]`/`[` between tool rows. Each asks the host, which knows where each row is.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: the buffer is withheld from agents, and the host does the work after
//! the command, where the channel's guards no longer see it, so what it folds, where it moves
//! and what it says would tell an agent of the rows; and a result shown puts a buffer withheld
//! from agents in front of the person, as `session-explain`'s report does.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{NativeAction, SessionRequest};

/// The commands of a native session's buffer.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-session-toggle",
        doc: "Fold or unfold the row at point of a native session's buffer: thinking, or a tool call and its result.\n\nIn an open row of several calls, on a call's lines, that call.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: toggle,
        state: None,
    },
    Command {
        name: "native-session-result",
        doc: "Show the whole result of the tool call at point of a native session's buffer, read-only, in `*result: NAME*`.\n\nA long result shows only its first and last lines in the session's buffer.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: result,
        state: None,
    },
    Command {
        name: "native-session-next-prompt",
        doc: "Move to the next message of the person's in a native session's buffer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: next_prompt,
        state: None,
    },
    Command {
        name: "native-session-previous-prompt",
        doc: "Move to the previous message of the person's in a native session's buffer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: previous_prompt,
        state: None,
    },
    Command {
        name: "native-session-next-tool",
        doc: "Move to the next row of tool calls in a native session's buffer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: next_tool,
        state: None,
    },
    Command {
        name: "native-session-previous-tool",
        doc: "Move to the previous row of tool calls in a native session's buffer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: previous_tool,
        state: None,
    },
];

/// Asks the host to `action` at point's line of the native session's buffer in front, or of
/// its composer (step 2), which the host carries out for the session it is docked under.
pub(super) fn at_point(ctx: &mut Ctx<'_>, action: NativeAction) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    if !buffer.is_native_session() && buffer.mode() != Mode::Composer {
        return Err(CommandError::new("This buffer is no native session"));
    }
    let id = window.buffer();
    let line = ctx.editor.cursor_line();
    ctx.editor.request_session(SessionRequest::Native {
        buffer: id,
        line,
        action,
    });
    Ok(())
}

fn toggle(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Toggle)
}

fn result(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Open)
}

fn next_prompt(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Prompt(args.count()))
}

fn previous_prompt(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Prompt(-args.count()))
}

fn next_tool(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Tool(args.count()))
}

fn previous_tool(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Tool(-args.count()))
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, NativeAction, SessionRequest};

    fn run(editor: &mut Editor, name: &str, args: Args) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::invoked(editor, Invocation::default());
        registry
            .execute(id, &mut ctx, args)
            .map_err(|error| error.to_string())
    }

    #[test]
    fn a_key_asks_the_host_at_points_line() {
        let mut editor = Editor::default();
        let id = editor.add_native_session_buffer("*claude: x*");
        editor.replace_native_rows(id, 0..0, "› one\n● two\n", &[]);
        editor.show(id);
        editor.current_mut().0.selections.move_primary(8);
        run(
            &mut editor,
            "native-session-previous-tool",
            Args::numeric(3),
        )
        .unwrap();
        run(&mut editor, "native-session-toggle", Args::NONE).unwrap();
        let at = |action| SessionRequest::Native {
            buffer: id,
            line: 1,
            action,
        };
        assert_eq!(
            editor.take_session_requests(),
            [at(NativeAction::Tool(-3)), at(NativeAction::Toggle)]
        );
    }

    #[test]
    fn elsewhere_a_key_says_why_not() {
        let mut editor = Editor::default();
        let id = editor.add_internal_buffer("*x*", Mode::Fundamental);
        editor.show(id);
        let refused = run(&mut editor, "native-session-result", Args::NONE);
        assert_eq!(
            refused,
            Err("native-session-result: This buffer is no native session".to_owned())
        );
        assert!(editor.take_session_requests().is_empty());
    }
}
