//! Turning the agent channel on and off, letting a connection in, allowing what an agent's
//! session tool asks, and whether agents' session starts ask (ADR 0085, ADR 0121, ADR 0135). Each is `Person`: only someone at the keyboard
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
    Command {
        name: "agent-starts-ask",
        doc: "Make every session an agent starts ask you first, until oso restarts.\n\nWith `[sessions] agents_start_without_asking` in `config.toml`, a session you started may start sessions, and stop an idle one to archive it, without asking; this takes that back for now. `agent-starts-allow` gives it again.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: agent_starts_ask,
        state: None,
    },
    Command {
        name: "agent-starts-allow",
        doc: "Let sessions you started start sessions without asking again, as `config.toml` says, after `agent-starts-ask`.\n\nThe same goes for an agent's archive that stops an idle session. It changes nothing unless `[sessions] agents_start_without_asking` was set when oso started.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: agent_starts_allow,
        state: None,
    },
];

fn agent_starts_ask(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    agent_starts(ctx, false)
}

fn agent_starts_allow(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    agent_starts(ctx, true)
}

fn agent_starts(ctx: &mut Ctx<'_>, allow: bool) -> Result<(), CommandError> {
    ctx.editor
        .request_session(oso_core::editor::SessionRequest::AgentStarts(allow));
    ctx.request_redraw();
    Ok(())
}

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
    use oso_core::editor::SessionRequest;

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
    fn question_details_need_an_agents_question_open() {
        let mut editor = editor("");
        let (result, _) = run(&mut editor, "agent-question-details", Args::NONE);
        assert!(result.is_err(), "no question is open");
        assert!(editor.take_session_requests().is_empty());
        editor
            .ask_for_agent(
                "let session \"a\" start a session?",
                "agent-channel-allow",
                ["allow", "refuse"],
            )
            .unwrap();
        assert_eq!(
            run(&mut editor, "agent-question-details", Args::NONE).0,
            Ok(())
        );
        assert_eq!(
            editor.take_session_requests(),
            [SessionRequest::QuestionDetails]
        );
        assert!(editor.prompt().is_some(), "the question stays open");
    }

    #[test]
    fn agent_starts_ask_and_allow_ask_the_host() {
        let mut editor = editor("");
        assert_eq!(run(&mut editor, "agent-starts-ask", Args::NONE).0, Ok(()));
        assert_eq!(run(&mut editor, "agent-starts-allow", Args::NONE).0, Ok(()));
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::AgentStarts(false),
                SessionRequest::AgentStarts(true)
            ]
        );
    }

    #[test]
    fn only_a_question_lets_an_agent_in() {
        let mut editor = editor("");
        let (result, _) = run(&mut editor, "agent-channel-allow", Args::NONE);
        assert!(result.is_err());
        assert!(!editor.take_agent_allowed());
    }
}
