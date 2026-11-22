//! A native session's composer and queue, as keys (ADR 0165, step 2; ADR 0170's composer
//! dock): `C-c C-c` sends, or queues while a turn runs, `C-u C-c C-c` sends at once, ahead of
//! the queue, interrupting the turn, `C-c C-k` cancels a queued message's edit or else
//! interrupts the turn, as the transcript's `C-c C-k` does (step 2c), `M-p`/`M-n` walk the messages sent, `C-c C-o` goes back to
//! the transcript and `i` there to the composer; and on a queued message's row `e` edits it in
//! its place, `d` drops it and `M-<up>`/`M-<down>` move it (`RET` takes it back, as the
//! transcript's `RET` asks). Each asks the host, which keeps the queue. `/` as an empty
//! composer's first character, or `C-c /`, completes over the session's slash commands, which
//! the host lists, and puts the one chosen first in the composer (step 2d).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what the composer holds is the person's message, withheld from
//! agents, and the host sends it or moves it after the command.

use oso_core::buffer::{Mode, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::NativeAction;
use oso_core::sum_tree::Bias;

use super::native::at_point as ask;

/// The list `native-composer-command` completes over: the slash commands of the session whose
/// composer is selected, as the host last listed them.
pub const SLASH_LIST: &str = "native-slash-commands";

/// The most of the composer's start read for the command it may begin with.
const COMMAND_BYTES: usize = 1024;

/// The commands of a native session's composer and of its queued messages.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-composer-command",
        doc: "Complete over the native session's slash commands, and put the one chosen first in its composer, ready for what it takes.\n\nA command the composer begins with is replaced. `/` typed as an empty composer's first character asks too; `C-g` then leaves the `/`.",
        args: ArgSpec::Listed {
            list: SLASH_LIST,
            prompt: "Command:",
            history: "native-slash-command",
        },
        reach: Reach::Person,
        run: slash_command,
        state: None,
    },
    Command {
        name: "native-composer-send",
        doc: "Send the composer's text to its native session, or queue it while a turn runs.\n\nWhile a queued message is edited in the composer, put it back in its place. With a prefix argument, interrupt the turn under way and send it first, ahead of the queue, once that turn ends; with no turn to interrupt, send it as without one.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: send,
        state: None,
    },
    Command {
        name: "native-composer-cancel",
        doc: "Give up the edit of a queued message in a native session's composer: it stays as it was.\n\nWith no edit, interrupt the session's turn under way: nothing queued goes until the next send.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: cancel,
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
        name: "native-session-interrupt",
        doc: "Interrupt the turn under way of the native session whose transcript this is: nothing queued goes until the next send.\n\nThe `interrupt` control request where the session's claude takes it; else claude is interrupted with a signal and resumes its conversation.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: interrupt,
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

fn send(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let action = if args.raw().is_some() {
        NativeAction::SendNow
    } else {
        NativeAction::Send
    };
    ask(ctx, action)
}

fn cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Cancel)
}

fn interrupt(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Cancel)
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

/// Puts the chosen slash command first in the composer: in place of the one it begins with, if
/// any, else before its text, where point stays.
fn slash_command(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = match ctx.answer() {
        Some(name) if !name.is_empty() => name.to_owned(),
        _ => return Err(CommandError::new("No command")),
    };
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::Composer {
        return Err(CommandError::new(
            "This buffer is no native session's composer",
        ));
    }
    let id = window.buffer();
    let text = buffer.text();
    let end = text.clip_offset(text.len().min(COMMAND_BYTES), Bias::Left);
    let start: String = text.chunks_in_range(0..end).collect();
    let listed = ctx.editor.listed_candidates(SLASH_LIST);
    let names = listed.iter().map(|candidate| candidate.text());
    let replaced = begun_command(&start, names);
    let inserted = format!("/{name} ");
    let edit = Replacement {
        range: 0..replaced,
        text: &inserted,
    };
    ctx.editor
        .apply_edits(id, &[edit])
        .map_err(|error| CommandError::new(error.to_string()))
}

/// How much of `start`, the composer's start, a command it begins with takes: its `/`, a word
/// that is, or begins, one of `names`, and one space after it; 0 when it begins with none, as
/// a path does.
fn begun_command<'a>(start: &str, mut names: impl Iterator<Item = &'a str>) -> usize {
    let Some(rest) = start.strip_prefix('/') else {
        return 0;
    };
    let ends = |c: char| c.is_whitespace() || c == '\u{feff}';
    let word = &rest[..rest.find(ends).unwrap_or(rest.len())];
    if !names.any(|name| name.starts_with(word)) {
        return 0;
    }
    let after = 1 + word.len();
    after + usize::from(start[after..].starts_with(' '))
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::{Buffer, Mode};
    use oso_core::command::{Args, Ctx, Invocation, Prefix};
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
    fn the_composers_keys_ask_the_host_and_a_prefix_sends_at_once() {
        let mut editor = Editor::default();
        let mut buffer = Buffer::from_text("*compose: x*", "hi");
        buffer.set_mode(Mode::Composer);
        let id = editor.add_buffer(buffer);
        editor.show(id);
        run(&mut editor, "native-composer-send", Args::NONE).unwrap();
        let prefix = Args::with_prefix(Some(Prefix::universal(None)));
        run(&mut editor, "native-composer-send", prefix).unwrap();
        run(&mut editor, "native-composer-previous", Args::numeric(2)).unwrap();
        let at = |action| SessionRequest::Native {
            buffer: id,
            line: 0,
            action,
        };
        assert_eq!(
            editor.take_session_requests(),
            [
                at(NativeAction::Send),
                at(NativeAction::SendNow),
                at(NativeAction::History(2)),
            ]
        );
    }

    #[test]
    fn elsewhere_they_say_why_not() {
        let mut editor = Editor::default();
        let id = editor.add_internal_buffer("*x*", Mode::Fundamental);
        editor.show(id);
        let refused = run(&mut editor, "native-composer-send", Args::NONE);
        assert!(refused.unwrap_err().contains("no native session"));
        assert!(editor.take_session_requests().is_empty());
    }
}
