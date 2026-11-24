//! The keys of a native session's buffer (ADR 0165, step 1; ADR 0130's design): `TAB` folds
//! the row at point, `RET` shows a call's whole result, `w` copies a reply's Markdown, `n`/`p`
//! move between the messages that started turns and `]`/`[` between tool rows, and `C-c C-t`, from the composer too, opens or
//! folds the to-do strip (part 3e). Each asks the host, which knows where each row is.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! `S-TAB` switches the session's permission mode (part 3b), in its composer too.
//!
//! Every key is `Person`: the buffer is withheld from agents, and the host does the work after
//! the command, where the channel's guards no longer see it, so what it folds, where it moves
//! and what it says would tell an agent of the rows; and a result shown puts a buffer withheld
//! from agents in front of the person, as `session-explain`'s report does.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{NativeAction, SessionRequest};

/// `y` to the question before Remote Control is turned on.
pub const REMOTE_CONFIRMED: &str = "native-remote-control-confirmed";

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
        name: "native-session-copy-markdown",
        doc: "Copy the reply at point of a native session's buffer as the Markdown Claude wrote, to the kill ring and the clipboard.\n\nThe buffer shows a reply with its Markdown's marks hidden, and `M-w` copies what is shown.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: copy_markdown,
        state: None,
    },
    Command {
        name: "native-session-next-prompt",
        doc: "Move to the next message of the person's, or the brief an agent started the session with, in a native session's buffer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: next_prompt,
        state: None,
    },
    Command {
        name: "native-session-previous-prompt",
        doc: "Move to the previous message of the person's, or the brief an agent started the session with, in a native session's buffer, times the count.",
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
    Command {
        name: "native-session-cycle-mode",
        doc: "Switch the native session whose transcript or composer this is to its next permission mode: default, acceptEdits, plan, auto, and round, as Claude Code's terminal does.\n\nThe modeline shows the mode once claude says it changed. With the `set_permission_mode` control request where the session's claude takes it; else claude, idle, ends and resumes its conversation in the new mode, and during a turn the mode can change once it is idle. bypassPermissions and dontAsk are never set.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: cycle_mode,
        state: None,
    },
    Command {
        name: "native-session-todo",
        doc: "Open the to-do strip of the native session whose transcript or composer this is to its rows, at most six, or fold it back to one line.\n\nThe strip shows while the session's to-do list has an item: Claude Code's task tools, which its settings turn on (`CLAUDE_CODE_ENABLE_TODO_TOOLS`).",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: todo,
        state: None,
    },
    Command {
        name: "native-remote-control",
        doc: "Turn Claude Code's Remote Control on for the native session whose transcript or composer this is, after a question, or off.\n\nOn, claude.ai follows the session, sends it messages, stops it and changes its permission mode; permission questions are answered in oso alone. What was said and what the tools gave, so far and from now on, is sent to Anthropic's servers and kept there. claude ends and resumes its conversation, idle, to turn it on or off. A session an agent started and an orchestrator are never connected. Off, claude.ai keeps what it has, and may still list the session.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: remote_control,
        state: None,
    },
    Command {
        name: "native-remote-control-address",
        doc: "Copy the address on claude.ai of the native session whose transcript or composer this is, to the kill ring and the clipboard, once Remote Control gave it one. oso never opens it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: remote_address,
        state: None,
    },
    Command {
        name: REMOTE_CONFIRMED,
        doc: "The yes to `native-remote-control`'s question.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: remote_confirmed,
        state: None,
    },
];

/// Asks the host to `action` at point's line of the native session's buffer in front, or of
/// its composer (step 2), its card (step 2e-2), its request's line (ADR 0182) or its plan's edit
/// (part 3d), which the host carries out for their session.
pub(super) fn at_point(ctx: &mut Ctx<'_>, action: NativeAction) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let its = matches!(
        buffer.mode(),
        Mode::Composer | Mode::Card | Mode::Request | Mode::Plan
    );
    if !buffer.is_native_session() && !its {
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

fn copy_markdown(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::CopyMarkdown)
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

fn cycle_mode(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::CycleMode)
}

fn todo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Todo)
}

fn remote_control(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::RemoteControl)
}

fn remote_address(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::RemoteAddress)
}

fn remote_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("Remote Control's question answers this"));
    }
    // What is in front is the question's report, not the session: the host kept which it is.
    let (id, line) = (ctx.editor.tiled().0.buffer(), 0);
    ctx.editor.request_session(SessionRequest::Native {
        buffer: id,
        line,
        action: NativeAction::RemoteConfirmed,
    });
    Ok(())
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
        run(&mut editor, "native-session-cycle-mode", Args::NONE).unwrap();
        run(&mut editor, "native-session-copy-markdown", Args::NONE).unwrap();
        let at = |action| SessionRequest::Native {
            buffer: id,
            line: 1,
            action,
        };
        assert_eq!(
            editor.take_session_requests(),
            [
                at(NativeAction::Tool(-3)),
                at(NativeAction::Toggle),
                at(NativeAction::CycleMode),
                at(NativeAction::CopyMarkdown)
            ]
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
