//! Dictation through TypeWhisper (ADR 0175). The commands only ask: the host checks where the
//! text would go, talks to TypeWhisper on a thread of its own and types the text that comes.
//!
//! Must not depend on: processes, the clipboard or `bindings`; a command asks the host through
//! the editor.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::DictationRequest;

/// Dictation's commands. Each needs the person: it turns on their microphone, and starts curl.
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
            editor.take_dictation_requests(),
            [
                DictationRequest::Toggle { prefix: false },
                DictationRequest::Toggle { prefix: true },
                DictationRequest::Cancel,
            ]
        );
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
