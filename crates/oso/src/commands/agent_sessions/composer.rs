//! A native session's composer and queue, as keys (ADR 0165, step 2; ADR 0170's composer
//! dock): `C-c C-c` sends, or queues while a turn runs, `C-u C-c C-c` sends at once, ahead of
//! the queue, interrupting the turn, `C-c C-k` cancels a queued message's edit or else
//! interrupts the turn, as the transcript's `C-c C-k` does (step 2c), `M-p`/`M-n` walk the messages sent, `C-c C-o` goes back to
//! the transcript and `i` there to the composer, `C-c C-s` in either puts the session's prompt suggestion at the composer's end
//! (part 3f); and on a queued message's row `e` edits it in
//! its place, `d` drops it and `M-<up>`/`M-<down>` move it (`RET` takes it back, as the
//! transcript's `RET` asks). Each asks the host, which keeps the queue. `/` as an empty
//! composer's first character, or `C-c /`, completes over the session's slash commands, which
//! the host lists, and puts the one chosen first in the composer (step 2d). `/model` and
//! `/effort` chosen there, or `C-c /` on one the composer begins with, complete their argument
//! over the models and effort levels the host lists (part 3c).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what the composer holds is the person's message, withheld from
//! agents, and the host sends it or moves it after the command.

use oso_core::buffer::{BufferId, Mode, Replacement};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, NativeAction};
use oso_core::sum_tree::Bias;

use super::native::at_point as ask;

/// The list `native-composer-command` completes over: the slash commands of the session whose
/// composer is selected, as the host last listed them.
pub const SLASH_LIST: &str = "native-slash-commands";

/// The lists `native-composer-model` and `native-composer-effort` complete over: the models
/// the selected composer's session's claude lists, and the effort levels of the one chosen or
/// shown (part 3c).
pub const MODEL_LIST: &str = "native-models";
pub const EFFORT_LIST: &str = "native-efforts";

/// A slash command whose argument completes (part 3c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argument {
    /// What follows the `/`.
    pub command: &'static str,
    /// The editor command that completes its argument, and the list it completes over.
    pub completes: &'static str,
    pub list: &'static str,
}

/// The slash commands whose argument completes.
pub const ARGUMENTS: [Argument; 2] = [
    Argument {
        command: "model",
        completes: "native-composer-model",
        list: MODEL_LIST,
    },
    Argument {
        command: "effort",
        completes: "native-composer-effort",
        list: EFFORT_LIST,
    },
];

/// The most of the composer's start read for the command it may begin with.
const COMMAND_BYTES: usize = 1024;

/// The commands of a native session's composer and of its queued messages.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-composer-command",
        doc: "Complete over the native session's slash commands, and put the one chosen first in its composer, ready for what it takes.\n\nA command the composer begins with is replaced. `/` typed as an empty composer's first character asks too; `C-g` then leaves the `/`. `/model` and `/effort` chosen go on to complete their argument, and on a `/model` or `/effort` the composer begins with, their argument is completed instead.",
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
        name: "native-composer-model",
        doc: "Complete over the models the native session's claude lists, and put `/model` with the one chosen first in its composer, ready to send.\n\nThe argument of a `/model` the composer begins with is replaced; with a prefix argument, no word is, and the model goes right after `/model`. Choosing `/model` in `native-composer-command` asks too, so, as does `C-c /` on a `/model` the composer begins with. The model is noted once claude says it set it, and each later claude of the session starts with it.",
        args: ArgSpec::Listed {
            list: MODEL_LIST,
            prompt: "Model:",
            history: "native-model",
        },
        reach: Reach::Person,
        run: model,
        state: None,
    },
    Command {
        name: "native-composer-effort",
        doc: "Complete over the effort levels of the native session's model, and put `/effort` with the one chosen first in its composer, ready to send.\n\nThe levels are those claude lists for the model chosen with `/model`, else for the model shown, else the five `--effort` takes. The argument of an `/effort` the composer begins with is replaced; with a prefix argument, no word is, and the level goes right after `/effort`, so, as choosing `/effort` in `native-composer-command` asks. The level is noted, and shown after the model in the modeline, once claude says it set it; each later claude of the session starts with it.",
        args: ArgSpec::Listed {
            list: EFFORT_LIST,
            prompt: "Effort:",
            history: "native-effort",
        },
        reach: Reach::Person,
        run: effort,
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
        doc: "Put the message sent before the one shown, in any native session, in this composer, times the count.\n\nThe text there when the walk starts comes back with `native-composer-next` past the newest.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: previous,
        state: None,
    },
    Command {
        name: "native-composer-next",
        doc: "Put the message sent after the one shown, in any native session, in this composer, times the count.\n\nPast the newest, put back the text the walk started from; with no walk, in an empty composer, put in the session's prompt suggestion, escaped as text oso composes, not sent.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: next,
        state: None,
    },
    Command {
        name: "native-composer-past-prompt",
        doc: "Complete over what you sent native sessions, the newest first, and put the message chosen in this composer at point, as it was sent, not sent again.\n\nThe list is shared by every native session and kept over restarts, unless `[sessions] prompt_history` is false. `native-prompt-history-clear` empties it.",
        args: ArgSpec::PastPrompt {
            prompt: "Past prompt:",
        },
        reach: Reach::Person,
        run: past_prompt,
        state: None,
    },
    Command {
        name: "native-prompt-history-clear",
        doc: "Take every message out of the history of what you sent native sessions, in oso's memory and in its file.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: clear_history,
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
        name: "native-session-take-suggestion",
        doc: "Put the native session's prompt suggestion at the end of its composer, and go there.\n\nIt is escaped as text oso composes, and not sent. It goes after the draft there, with a space between, unless the draft is a command.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: take_suggestion,
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

fn past_prompt(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::PastPrompt)
}

fn clear_history(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    (ctx.editor).request_session(oso_core::editor::SessionRequest::ClearPromptHistory);
    Ok(())
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

fn take_suggestion(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Suggestion)
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
    let name = chosen(ctx, "No command")?;
    let (id, start) = composer_start(ctx)?;
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

fn model(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    with_argument(ctx, args, "model", "No model")
}

fn effort(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    with_argument(ctx, args, "effort", "No effort level")
}

/// Puts `/command` with the chosen argument first in the composer: in place of a `/command`
/// it begins with and its argument, or with a prefix argument that command alone, else before
/// its text (part 3c).
fn with_argument(
    ctx: &mut Ctx<'_>,
    args: Args,
    command: &str,
    none: &'static str,
) -> Result<(), CommandError> {
    let chosen = chosen(ctx, none)?;
    let (id, start) = composer_start(ctx)?;
    // With a prefix, as when chosen after the command was put in, no word is the argument.
    let replaced = argument_end(&start, command, args.raw().is_some());
    let mut inserted = format!("/{command} {chosen}");
    if start[replaced..].starts_with(|c: char| !ends_word(c)) {
        inserted.push(' ');
    }
    let edit = Replacement {
        range: 0..replaced,
        text: &inserted,
    };
    ctx.editor
        .apply_edits(id, &[edit])
        .map_err(|error| CommandError::new(error.to_string()))
}

/// The prompt's answer, or `none` as the error when it gave nothing.
fn chosen(ctx: &Ctx<'_>, none: &'static str) -> Result<String, CommandError> {
    match ctx.answer() {
        Some(answer) if !answer.is_empty() => Ok(answer.to_owned()),
        _ => Err(CommandError::new(none)),
    }
}

/// The selected composer and its start, at most [`COMMAND_BYTES`] of it.
fn composer_start(ctx: &Ctx<'_>) -> Result<(BufferId, String), CommandError> {
    selected_start(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is no native session's composer"))
}

/// The selected buffer, if it is a composer, and its start, at most [`COMMAND_BYTES`] of it.
fn selected_start(editor: &Editor) -> Option<(BufferId, String)> {
    let (window, buffer) = editor.current();
    if buffer.mode() != Mode::Composer {
        return None;
    }
    let text = buffer.text();
    let end = text.clip_offset(text.len().min(COMMAND_BYTES), Bias::Left);
    Some((window.buffer(), text.chunks_in_range(0..end).collect()))
}

/// Which of [`ARGUMENTS`] `start`, a composer's start, begins with: `/model` or `/effort` as a
/// word of its own (part 3c).
pub fn begun_argument(start: &str) -> Option<Argument> {
    ARGUMENTS.into_iter().find(|argument| {
        let rest = start
            .strip_prefix('/')
            .and_then(|rest| rest.strip_prefix(argument.command));
        rest.is_some_and(|rest| rest.is_empty() || rest.starts_with(ends_word))
    })
}

/// Which of [`ARGUMENTS`] the selected composer begins with, as the dispatcher asks (part 3c).
pub fn composer_argument(editor: &Editor) -> Option<Argument> {
    begun_argument(&selected_start(editor)?.1)
}

/// How much of `start` a `/command` it begins with and its argument take: the command, the
/// blanks after it on its line, and the word after them, which `none` leaves out but for one
/// space; 0 when it begins with none.
fn argument_end(start: &str, command: &str, none: bool) -> usize {
    if begun_argument(start).is_none_or(|begun| begun.command != command) {
        return 0;
    }
    let after = 1 + command.len();
    let rest = &start[after..];
    if none {
        return after + usize::from(rest.starts_with(' '));
    }
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    let word = rest[blanks..]
        .find(ends_word)
        .unwrap_or(rest.len() - blanks);
    after + blanks + word
}

/// Whether `c` ends a command's word, as the CLI reads it (step 2d): the twin of
/// `sessions::native::slash::ends_word`, which `commands` may not use.
fn ends_word(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// How much of `start`, the composer's start, a command it begins with takes: its `/`, a word
/// that is, or begins, one of `names`, and one space after it; 0 when it begins with none, as
/// a path does.
fn begun_command<'a>(start: &str, mut names: impl Iterator<Item = &'a str>) -> usize {
    let Some(rest) = start.strip_prefix('/') else {
        return 0;
    };
    let word = &rest[..rest.find(ends_word).unwrap_or(rest.len())];
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
    fn the_suggestion_is_asked_for_in_the_transcript_and_the_composer() {
        let mut editor = Editor::default();
        let transcript = editor.add_native_session_buffer("*claude: x*");
        let mut composer = Buffer::from_text("*compose: x*", "draft");
        composer.set_mode(Mode::Composer);
        let composer = editor.add_buffer(composer);
        let name = "native-session-take-suggestion";
        for buffer in [transcript, composer] {
            editor.show(buffer);
            run(&mut editor, name, Args::NONE).unwrap();
            let request = SessionRequest::Native {
                buffer,
                line: 0,
                action: NativeAction::Suggestion,
            };
            assert_eq!(editor.take_session_requests(), [request]);
        }
        let plain = editor.add_buffer(Buffer::from_text("plain", ""));
        editor.show(plain);
        assert!(run(&mut editor, name, Args::NONE).is_err());
    }

    #[test]
    fn a_chosen_argument_takes_the_place_of_the_one_typed() {
        use super::argument_end;
        // `/model`, the blanks after it on its line, and one word.
        let end = |start, command| argument_end(start, command, false);
        assert_eq!(end("/model", "model"), 6);
        assert_eq!(end("/model ", "model"), 7);
        assert_eq!(end("/model  son more", "model"), 11);
        assert_eq!(end("/model\nmore", "model"), 6);
        assert_eq!(end("/effort\u{feff}x", "effort"), 7);
        // Another command, or a word that only begins so.
        assert_eq!(end("/models x", "model"), 0);
        assert_eq!(end("/effort x", "model"), 0);
        assert_eq!(end(" /model x", "model"), 0);
        // With a prefix, the command and one space alone: the word after is the person's.
        assert_eq!(argument_end("/model fix this", "model", true), 7);
        assert_eq!(super::begun_argument("/effort"), Some(super::ARGUMENTS[1]));
    }

    #[test]
    fn choosing_a_model_puts_it_after_model_in_the_composer() {
        let mut editor = Editor::default();
        let mut buffer = Buffer::from_text("*compose: x*", "/model opus and more");
        buffer.set_mode(Mode::Composer);
        let id = editor.add_buffer(buffer);
        editor.show(id);
        let text = |editor: &Editor| editor.buffer(id).unwrap().text().to_string();
        let answered = |editor: &mut Editor, name: &str, answer: &str| {
            let registry = crate::commands::registry();
            let command = registry.lookup(name).unwrap();
            let invocation = Invocation {
                answer: Some(answer),
                ..Invocation::default()
            };
            let mut ctx = Ctx::invoked(editor, invocation);
            registry.execute(command, &mut ctx, Args::NONE).unwrap();
        };
        answered(&mut editor, "native-composer-model", "sonnet");
        assert_eq!(text(&editor), "/model sonnet and more");
        answered(&mut editor, "native-composer-effort", "high");
        assert_eq!(text(&editor), "/effort high /model sonnet and more");
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
