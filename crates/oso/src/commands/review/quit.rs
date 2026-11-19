//! Leaving a review and the review layout's key (ADR 0122, ADR 0160): `q` buries the review
//! and, for the person, asks the host to put back the windows from before the layout;
//! `review-layout` opens the review menu or asks the host for the layout.
//!
//! Must not depend on: `app`, `dispatch` or winit; these only ask the host, which keeps the
//! layout with the spaces' trees.

use oso_core::command::{Args, CommandError, Ctx};
use oso_core::editor::ReviewRequest;

use super::current_review;

pub(super) fn review_quit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    current_review(ctx)?;
    // A pane buries its pair: the review's one window goes back, as the review's buffer would.
    let window = ctx.editor.window().id();
    ctx.editor.show_review_unified(window);
    ctx.editor.walk_buffers(1).map_err(CommandError::new)?;
    // The windows from before the layout come back for the person's `q` only (ADR 0160).
    if !ctx.editor.runs_for_agent() {
        ctx.editor.request_review(ReviewRequest::Quit);
    }
    ctx.request_redraw();
    Ok(())
}

/// `review-layout` (ADR 0160): the review menu, which the dispatcher opens as it ends, with a
/// prefix or no review open; else the host, which keeps the layout, says what it does.
pub(super) fn review_layout(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let menu = args.raw().is_some() || ctx.editor.reviews().next().is_none();
    ctx.editor.request_review(ReviewRequest::Layout { menu });
    ctx.request_redraw();
    Ok(())
}
