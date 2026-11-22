//! A native session's composer and queue, as keys (ADR 0165, step 2; ADR 0170's composer
//! dock): `C-c C-c` sends, or queues while a turn runs, `C-u C-c C-c` sends at once, `C-c C-k`
//! gives up a queued message's edit, `M-p`/`M-n` walk the messages sent, `C-c C-o` goes back to
//! the transcript and `i` there to the composer; and on a queued message's row `e` edits it in
//! its place, `d` drops it and `M-<up>`/`M-<down>` move it (`RET` takes it back, as the
//! transcript's `RET` asks). Each asks the host, which keeps the queue.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what the composer holds is the person's message, withheld from
//! agents, and the host sends it or moves it after the command.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{NativeAction, SessionRequest};

/// The commands of a native session's composer and of its queued messages.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-composer-send",
        doc: "Send the composer's text to its native session, or queue it while a turn runs.\n\nWhile a queued message is edited in the composer, put it back in its place. With a prefix argument, send it at once: the turn under way is interrupted.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: send,
        state: None,
    },
    Command {
        name: "native-composer-give-up",
        doc: "Give up the edit of a queued message in a native session's composer: it stays as it was.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: give_up,
        state: None,
    },
    Command {
        name: "native-composer-previous",
        doc: "Put the message sent before the one shown in a native session's composer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: previous,
        state: None,
    },
    Command {
        name: "native-composer-next",
        doc: "Put the message sent after the one shown in a native session's composer, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: next,
        state: None,
    },
    Command {
        name: "native-composer-transcript",
        doc: "Go from a native session's composer back to its transcript.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: transcript,
        state: None,
    },
    Command {
        name: "native-session-compose",
        doc: "Go from a native session's transcript to its composer.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: compose,
        state: None,
    },
    Command {
        name: "native-session-queue",
        doc: "Go to the first message queued for a native session, in its transcript.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: queue,
        state: None,
    },
    Command {
        name: "native-queued-edit",
        doc: "Edit the queued message at point of a native session's transcript in its composer, where it keeps its place: `C-c C-c` puts it back, `C-c C-k` gives the edit up.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: edit,
        state: None,
    },
    Command {
        name: "native-queued-drop",
        doc: "Drop the queued message at point of a native session's transcript.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: drop_queued,
        state: None,
    },
    Command {
        name: "native-queued-up",
        doc: "Move the queued message at point of a native session's transcript up, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: up,
        state: None,
    },
    Command {
        name: "native-queued-down",
        doc: "Move the queued message at point of a native session's transcript down, times the count.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: down,
        state: None,
    },
];

/// Asks the host to `action` at point's line of the native session's transcript or composer
/// in front.
fn ask(ctx: &mut Ctx<'_>, action: NativeAction) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    if !buffer.is_native_session() && buffer.mode() != Mode::Composer {
        return Err(CommandError::new("This buffer is no native session's"));
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

fn send(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let action = if args.raw().is_some() {
        NativeAction::SendNow
    } else {
        NativeAction::Send
    };
    ask(ctx, action)
}

fn give_up(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::GiveUp)
}

fn previous(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::History(args.count()))
}

fn next(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::History(-args.count()))
}

fn transcript(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Transcript)
}

fn compose(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Compose)
}

fn queue(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Queue)
}

fn edit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::EditQueued)
}

fn drop_queued(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::DropQueued)
}

fn up(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::MoveQueued(-args.count()))
}

fn down(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::MoveQueued(args.count()))
}
