//! Saved reviews' commands (ADR 0122, step 6): `review-resume` and `review-forget` over the
//! host's list of saved reviews, and what closing a review, or leaving oso, would lose of the
//! person's comments once the host keeps the review across a restart.
//!
//! Must not depend on: the host's store or its threads; the commands ask the host, which
//! reads and writes the saved reviews.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, ReviewId, ReviewRequest};

use super::unsent;

/// The host's list of saved reviews, for `review-resume` and `review-forget` (step 6).
pub(crate) const SAVED_LIST: &str = "review-saved";

/// The commands of saved reviews.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "review-resume",
        doc: "Open a saved review again, read with completion over the saved reviews, opened last first, each with its project, its comments and when it was last opened.\n\nIts comments come back carried to the changes as they are now; one whose line went is kept as outdated. The project must be trusted, as opening a review runs git, and known to oso: open a file of it first. A review not opened for 30 days is removed.",
        args: ArgSpec::Listed {
            list: SAVED_LIST,
            prompt: "Resume review:",
            history: "review-resume",
        },
        reach: Reach::Person,
        run: review_resume,
        state: None,
    },
    Command {
        name: "review-forget",
        doc: "Remove a saved review now, read with completion over the saved reviews: its comments, decisions and viewed marks.\n\nAn open review of it is saved again as it changes; close it first to forget it for good.",
        args: ArgSpec::Listed {
            list: SAVED_LIST,
            prompt: "Forget saved review:",
            history: "review-resume",
        },
        reach: Reach::Person,
        run: review_forget,
        state: None,
    },
];

/// The saved review a `Listed` prompt read, or an error saying there is none.
fn saved_answer(ctx: &Ctx<'_>) -> Result<String, CommandError> {
    super::super::not_in_prompt(ctx)?;
    match ctx.answer() {
        Some(answer) if !answer.is_empty() => Ok(answer.to_owned()),
        _ => Err(CommandError::new("No saved review")),
    }
}

fn review_resume(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let pick = saved_answer(ctx)?;
    ctx.editor.request_review(ReviewRequest::Resume(pick));
    ctx.request_redraw();
    Ok(())
}

fn review_forget(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let pick = saved_answer(ctx)?;
    ctx.editor.request_review(ReviewRequest::Forget(pick));
    Ok(())
}

/// How many comments of the person's the review `id` would lose if it closed now: an open
/// draft for it, and, unless the host keeps the review across a restart (step 6), those not
/// sent ([`unsent`]).
pub(crate) fn lost(editor: &Editor, id: ReviewId) -> usize {
    if !editor.review_kept(id) {
        return unsent(editor, id);
    }
    usize::from(editor.draft().is_some_and(|(drafted, _)| drafted == id))
}

#[cfg(test)]
#[path = "saved_tests.rs"]
mod tests;
