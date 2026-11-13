//! Turning the agent channel on and off, letting a connection in, and allowing what an agent's
//! session tool asks (ADR 0085, ADR 0121). Each is `Person`: only someone at the keyboard
//! decides who may drive the editor, and what an agent may start.
//!
//! Must not depend on: the channel's socket or threads, or `bindings`; a command asks the
//! host through the editor, and the host starts or stops the channel.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

/// The agent channel's commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "agent-channel-start",
        doc: "Let an agent, such as Claude Code through `oso mcp`, drive this editor.\n\nTurns the agent channel on for this session. Each agent that connects asks you first, and the modeline shows `Agent` while it is on. An agent runs registered commands only, never one that needs you at the keyboard, and nothing while a buffer that may hold secrets is shown. `*Agent*` lists what it asked.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: start,
        state: None,
    },
    Command {
        name: "agent-channel-stop",
        doc: "Turn the agent channel off, and close the agents' connections.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: stop,
        state: None,
    },
    Command {
        name: "agent-channel-allow",
        doc: "Let in the agent whose connection waits, or allow what an agent's session tool asks, as `y` to its question does.\n\nRun on its own, it says that the question answers this.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: allow,
        state: None,
    },
    Command {
        name: "agent-question-details",
        doc: "Show what the agent's open question would run, as `?` in it does: the session that asks, and the whole brief it would start a session with.\n\nThe question stays open; answer it with `y` or `n`. `*Agent request*` is read-only, and no agent reads it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: question_details,
        state: None,
    },
];

fn question_details(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx
        .editor
        .prompt()
        .is_some_and(oso_core::minibuffer::Prompt::from_agent)
    {
        return Err(CommandError::new("No question of an agent's is open"));
    }
    ctx.editor
        .request_session(oso_core::editor::SessionRequest::QuestionDetails);
    ctx.request_redraw();
    Ok(())
}

fn start(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.request_agent_channel(true);
    ctx.request_redraw();
    Ok(())
}

fn stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ctx.editor.request_agent_channel(false);
    ctx.request_redraw();
    Ok(())
}

fn allow(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new(
            "An agent's connection asks you when it comes",
        ));
    }
    ctx.editor.allow_agent();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{editor, run};
    use oso_core::command::Args;

    #[test]
    fn start_and_stop_ask_the_host() {
        let mut editor = editor("");
        assert_eq!(
            run(&mut editor, "agent-channel-start", Args::NONE).0,
            Ok(())
        );
        assert_eq!(editor.take_agent_channel_request(), Some(true));
        assert_eq!(run(&mut editor, "agent-channel-stop", Args::NONE).0, Ok(()));
        assert_eq!(editor.take_agent_channel_request(), Some(false));
    }

    #[test]
    fn only_a_question_lets_an_agent_in() {
        let mut editor = editor("");
        let (result, _) = run(&mut editor, "agent-channel-allow", Args::NONE);
        assert!(result.is_err());
        assert!(!editor.take_agent_allowed());
    }
}
