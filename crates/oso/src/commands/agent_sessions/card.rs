//! A native session's permission card, as keys (ADR 0165, step 2e-2; ADR 0170's card dock): `y`
//! allows the call or approves the plan, `n` denies it, declines the questions or keeps
//! planning, `N` denies with a note, a digit chooses a question's option, `o` answers it in the
//! person's own words, and `RET` shows the whole call or plan. Each asks the host, which holds
//! the call and answers its bridge; a note and an own answer are read in the minibuffer, from
//! the host, which opens [`CARD_NOTE`]'s or [`CARD_OWN`]'s prompt.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what a card answers lets a call of a session run, which no agent may
//! do (ADR 0165, Q2).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{NativeAction, SessionRequest};

use super::native::at_point as ask;

/// The command whose prompt reads a denial's note.
pub const CARD_NOTE: &str = "native-card-note-text";
/// The command whose prompt reads a question's answer in the person's own words.
pub const CARD_OWN: &str = "native-card-own-text";

/// The commands of a native session's permission card.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "native-card-allow",
        doc: "Allow the call a native session's permission card asks about, once, or approve the plan it shows.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: allow,
        state: None,
    },
    Command {
        name: "native-card-deny",
        doc: "Deny the call a native session's permission card asks about, decline its questions, or keep planning.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: deny,
        state: None,
    },
    Command {
        name: "native-card-note",
        doc: "Deny the call a native session's permission card asks about with a note, read in the minibuffer, which the session's claude is told.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: note,
        state: None,
    },
    Command {
        name: "native-card-choose",
        doc: "Choose the option of the digit typed in a native session's question card; in a question that takes several, choose or unchoose it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: choose,
        state: None,
    },
    Command {
        name: "native-card-other",
        doc: "Answer the question of a native session's question card in your own words, read in the minibuffer.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: other,
        state: None,
    },
    Command {
        name: CARD_NOTE,
        doc: "Give the permission card the host asked about the note read in its prompt, and deny its call with it.",
        args: ArgSpec::Text {
            prompt: "Deny with a note: ",
            history: "native-card-note",
        },
        reach: Reach::Person,
        run: text,
        state: None,
    },
    Command {
        name: CARD_OWN,
        doc: "Give the question card the host asked about the answer read in its prompt.",
        args: ArgSpec::Text {
            prompt: "Your answer: ",
            history: "native-card-own",
        },
        reach: Reach::Person,
        run: text,
        state: None,
    },
];

fn allow(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Allow)
}

fn deny(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Deny)
}

fn note(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Note)
}

fn other(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, NativeAction::Other)
}

fn choose(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let digit = ctx.key_char().and_then(|c| c.to_digit(10));
    let Some(digit) = digit
        .filter(|&digit| digit > 0)
        .and_then(|d| u8::try_from(d).ok())
    else {
        return Err(CommandError::new("Type an option's number, 1 to 9"));
    };
    ask(ctx, NativeAction::Choose(digit))
}

/// The answer of [`CARD_NOTE`]'s or [`CARD_OWN`]'s prompt, for the card the host asked about.
fn text(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let text = match ctx.answer().map(str::trim) {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => return Err(CommandError::new("Nothing written")),
    };
    ctx.editor.request_session(SessionRequest::CardText(text));
    Ok(())
}
