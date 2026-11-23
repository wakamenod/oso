//! A native session's permission card, as keys (ADR 0165, step 2e-2; ADR 0170's card dock): `y`
//! allows the call or approves the plan, `n` denies it, declines the questions or keeps
//! planning, `N` denies with a note, a digit chooses a question's option, `o` answers it in the
//! person's own words, and `RET` shows the whole call or plan. A question or a plan longer than
//! its card pages with `SPC` and `DEL` (ADR 0180). A plan's card (part 3d) also
//! approves with accept edits (`a`) or auto (`A`, after `y` to a question) after it, or edits
//! it (`e`) in `*plan: NAME*`, where `C-c C-c` approves the edit, `C-c C-a` and `C-c C-A` with
//! those modes, and `C-c C-k` gives it up. Each asks the host,
//! which holds the call and answers its bridge; a note and an own answer are read in the
//! minibuffer, from the host, which opens [`CARD_NOTE`]'s or [`CARD_OWN`]'s prompt.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Every key is `Person`: what a card answers lets a call of a session run, which no agent may
//! do (ADR 0165, Q2).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Approval, NativeAction, SessionRequest};

use super::native::at_point;

/// The command a question's `y` runs to approve a plan with auto (part 3d).
pub const PLAN_CONFIRMED: &str = "native-plan-confirmed";
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
        name: "native-card-whole",
        doc: "Show the whole call or plan in a native session's permission card, or go back to the card; in a question that takes several options, answer with those chosen.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: whole,
        state: None,
    },
    Command {
        name: "native-card-page-down",
        doc: "Show the next page of a native session's question or plan card; with a prefix argument, as many pages on.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: page_down,
        state: None,
    },
    Command {
        name: "native-card-page-up",
        doc: "Show the page before of a native session's question or plan card; with a prefix argument, as many pages back.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: page_up,
        state: None,
    },
    Command {
        name: "native-card-leave",
        doc: "Leave a native session's permission card for its transcript: the call still waits on the card.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: leave,
        state: None,
    },
    Command {
        name: "native-card-approve-accept-edits",
        doc: "Approve the plan a native session's card shows, or as edited in *plan: NAME*, and let the session's edits run unasked after it (accept edits): `a` on the card, `C-c C-a` in the edit.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: approve_accept_edits,
        state: None,
    },
    Command {
        name: "native-card-approve-auto",
        doc: "Approve the plan a native session's card shows, or as edited in *plan: NAME*, and leave the session in auto mode after it, whose classifier lets what it judges safe run unasked, after `y` to a question: `A` on the card, `C-c C-A` in the edit.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: approve_auto,
        state: None,
    },
    Command {
        name: "native-card-edit-plan",
        doc: "Edit the plan a native session's card shows, in *plan: NAME*, in place of the session's transcript: C-c C-c there approves the edit, C-c C-k gives it up.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: edit_plan,
        state: None,
    },
    Command {
        name: "native-plan-approve",
        doc: "Approve the plan of a native session as edited in *plan: NAME*: the session's claude is given the edit.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: plan_approve,
        state: None,
    },
    Command {
        name: "native-plan-cancel",
        doc: "Give up the edit of a native session's plan in *plan: NAME*, and go back to its card, which still waits.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: plan_cancel,
        state: None,
    },
    Command {
        name: PLAN_CONFIRMED,
        doc: "Approve the plan a native session's card or *plan: NAME* asked about with auto, after `y` to the question `A` or `C-c C-A` asks.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: confirmed,
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
    at_point(ctx, NativeAction::Allow)
}

fn deny(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Deny)
}

fn note(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Note)
}

fn other(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Other)
}

fn whole(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Open)
}

fn page_down(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Page(args.count()))
}

fn page_up(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Page(args.count().saturating_neg()))
}

fn leave(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Transcript)
}

fn approve_accept_edits(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Approve(Approval::AcceptEdits))
}

fn approve_auto(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Approve(Approval::Auto))
}

fn edit_plan(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::EditPlan)
}

fn plan_approve(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Approve(Approval::Manual))
}

fn plan_cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    at_point(ctx, NativeAction::Cancel)
}

fn confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("The plan's question answers this"));
    }
    at_point(ctx, NativeAction::Confirmed)
}

fn choose(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let digit = ctx.key_char().and_then(|c| c.to_digit(10));
    let Some(digit) = digit
        .filter(|&digit| digit > 0)
        .and_then(|d| u8::try_from(d).ok())
    else {
        return Err(CommandError::new("Type an option's number, 1 to 9"));
    };
    at_point(ctx, NativeAction::Choose(digit))
}

/// The answer of [`CARD_NOTE`]'s or [`CARD_OWN`]'s prompt, for the card the host asked about.
fn text(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    // A yank of a secret entry turns it secret: it never goes to the session.
    if ctx.answer_secret() {
        return Err(CommandError::new(
            "The text holds secret text, which is not sent: write it again without it",
        ));
    }
    let text = match ctx.answer().map(str::trim) {
        Some(text) if !text.is_empty() => text.to_owned(),
        _ => return Err(CommandError::new("Nothing written")),
    };
    ctx.editor.request_session(SessionRequest::CardText(text));
    Ok(())
}
