//! Dictation through TypeWhisper (ADR 0175). The commands only ask: the host checks where the
//! text would go, talks to TypeWhisper on a thread of its own and types the text that comes.
//!
//! Must not depend on: processes, the clipboard or `bindings`; a command asks the host through
//! the editor.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::DictationRequest;

/// Dictation's commands. Each needs the person: their microphone, curl, or what was learned of
/// their words.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "dictation-toggle",
        doc: "Start dictating through TypeWhisper, or stop and type what it heard.\n\nTypeWhisper, running on this Mac with its API server on, records from the microphone while the modeline says `● REC`, and recognises the speech once you stop. The text goes where you were when you started, at each cursor, in one undo step: a buffer, the minibuffer or a session's composer. If that place is gone or its text changed meanwhile, the text goes to the kill ring instead, for `C-y`. It waits while an input method composes there. TypeWhisper's own paste of the same text, a Command-V that oso reads as `M-v`, is dropped while the text is due. Not in a terminal, a passphrase, a question or a read-only buffer, nor in a keyboard macro. `[dictation]` in the configuration names TypeWhisper's discovery file, curl, a workflow and the longest recording.",
        // A prefix argument still reaches it, kept for the fix that comes with it later.
        args: ArgSpec::None,
        reach: Reach::Person,
        run: dictation_toggle,
        state: None,
    },
    Command {
        name: "dictation-cancel",
        doc: "Drop the dictation under way and what it would type.\n\nOne that records is stopped, and TypeWhisper's text is thrown away when it comes; its own paste of it is still dropped.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: dictation_cancel,
        state: None,
    },
    Command {
        name: "dictation-forget-learned",
        doc: "Forget everything dictation has learned, after asking once.\n\nFixes by Claude, and your own rewrites of dictated text soon after, teach dictation the words it mishears: the pairs go to each later fix as history, and those learned often enough to TypeWhisper's dictionary. This empties that record and its log, in oso's state directory, stops watching what was dictated just now, and removes from TypeWhisper's dictionary every entry oso added there, but none you changed or added yourself. With TypeWhisper not running, its entries go the next time you dictate.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: dictation_forget_learned,
        state: None,
    },
    Command {
        name: "dictation-learning-log",
        doc: "Show what dictation learned and when, newest first, in `*Dictation log*`.\n\nEach fix kept or undone, each rewrite of yours, each history sent with a fix, as counts, and each entry added to TypeWhisper's dictionary, left to you, removed or not added, over the last 30 days. Agents cannot read it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: dictation_learning_log,
        state: None,
    },
];

/// Dictation starts the person's microphone and types what they said, which a keyboard macro
/// cannot replay.
fn not_in_a_macro(ctx: &Ctx<'_>) -> Result<(), CommandError> {
    if ctx.editor.macro_recording() || ctx.editor.macro_replaying() {
        return Err(CommandError::new(
            "Dictation does not run in a keyboard macro",
        ));
    }
    Ok(())
}

fn dictation_toggle(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    not_in_a_macro(ctx)?;
    let prefix = args.raw().is_some();
    ctx.editor
        .request_dictation(DictationRequest::Toggle { prefix });
    ctx.request_redraw();
    Ok(())
}

fn dictation_cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_a_macro(ctx)?;
    ctx.editor.request_dictation(DictationRequest::Cancel);
    ctx.request_redraw();
    Ok(())
}

fn dictation_forget_learned(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_a_macro(ctx)?;
    if !ctx.confirmed() {
        ctx.editor
            .ask(
                "Forget everything dictation has learned?",
                "dictation-forget-learned",
            )
            .map_err(|e| CommandError::new(e.to_string()))?;
        return Ok(());
    }
    ctx.editor.request_dictation(DictationRequest::Forget);
    ctx.request_redraw();
    Ok(())
}

fn dictation_learning_log(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    not_in_a_macro(ctx)?;
    ctx.editor.request_dictation(DictationRequest::ShowLog);
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::commands::tests::{editor, run};
    use oso_core::command::{Args, Prefix};
    use oso_core::editor::DictationRequest;

    #[test]
    fn the_commands_ask_the_host_with_the_prefix_kept() {
        let mut editor = editor("text");
        assert_eq!(run(&mut editor, "dictation-toggle", Args::NONE).0, Ok(()));
        assert_eq!(
            run(
                &mut editor,
                "dictation-toggle",
                Args::with_prefix(Some(Prefix::Universal(4)))
            )
            .0,
            Ok(())
        );
        assert_eq!(run(&mut editor, "dictation-cancel", Args::NONE).0, Ok(()));
        assert_eq!(
            run(&mut editor, "dictation-learning-log", Args::NONE).0,
            Ok(())
        );
        assert_eq!(
            editor.take_dictation_requests(),
            [
                DictationRequest::Toggle { prefix: false },
                DictationRequest::Toggle { prefix: true },
                DictationRequest::Cancel,
                DictationRequest::ShowLog,
            ]
        );
    }

    #[test]
    fn forgetting_asks_once_and_then_asks_the_host() {
        let mut editor = editor("text");
        assert_eq!(
            run(&mut editor, "dictation-forget-learned", Args::NONE).0,
            Ok(())
        );
        assert!(editor.take_dictation_requests().is_empty());
        let prompt = editor.prompt().expect("a question");
        assert_eq!(
            prompt.label(),
            "Forget everything dictation has learned? (y or n)"
        );
        assert!(matches!(
            prompt.kind(),
            oso_core::minibuffer::Kind::Question {
                yes: "dictation-forget-learned",
                ..
            }
        ));
        let registry = crate::commands::registry();
        let id = registry.lookup("dictation-forget-learned").unwrap();
        let invocation = oso_core::command::Invocation {
            confirmed: true,
            ..oso_core::command::Invocation::default()
        };
        let mut ctx = oso_core::command::Ctx::invoked(&mut editor, invocation);
        assert!(registry.execute(id, &mut ctx, Args::NONE).is_ok());
        assert_eq!(editor.take_dictation_requests(), [DictationRequest::Forget]);
    }

    #[test]
    fn a_keyboard_macro_never_dictates() {
        let mut editor = editor("text");
        editor.macro_start().unwrap();
        assert_eq!(
            run(&mut editor, "dictation-toggle", Args::NONE).0,
            Err("Dictation does not run in a keyboard macro".to_owned())
        );
        assert!(editor.take_dictation_requests().is_empty());
    }
}
