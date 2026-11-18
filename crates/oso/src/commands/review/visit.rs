//! Jumping to the source from a review (ADR 0122, "Jump to the source", step 3, part three):
//! `RET` on a row opens the file at the line it shows, `C-u RET` the old side read-only. The
//! command finds what the row is and asks the host, which carries the line to the file's text
//! now on the review's thread and picks the window.
//!
//! Must not depend on: the host's threads, git or the file system; this reads the rows and the
//! model.
//!
//! - It is `Person`: it opens files and makes a buffer of a side, which may reach a file the
//!   agent channel should not open for itself (ADR 0085).
//! - Every row but a filler and the title's goes somewhere: a line to itself, of whichever side; a hunk's
//!   header to its first line; a file's header or note to its first line; a comment to the line
//!   or hunk it is on, an outdated one to the file's first line. In a pane, a row is of that
//!   pane's side (Q3).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ReviewRequest;
use oso_core::review::{Anchor, RowKind, Side};

use super::current_review;

/// The commands that jump to the source.
pub const COMMANDS: &[Command] = &[Command {
    name: "review-visit",
    doc: "Open the file of the review's line at the primary cursor, at that line, in another window (`RET`).\n\nThe line is carried through the edits made to the file since the review read it, and one changed since goes to the start of what replaced it. A removed line goes to where it was removed; a hunk's header to its first line; a file's header to the file's first line; a comment to its line. A review whose new side is a commit, as of a commit, a range or a pull request not checked out here, shows that side read-only instead. With a prefix argument, it shows the old side read-only, at the line.",
    args: ArgSpec::None,
    reach: Reach::Person,
    run: review_visit,
    state: None,
}];

fn review_visit(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    let old_side = args.raw().is_some();
    let (window, buffer) = ctx.editor.current();
    let rows = (ctx.editor.review_rows_of(window.buffer()))
        .ok_or_else(|| CommandError::new("Not in a review"))?;
    let line = (buffer.text())
        .offset_to_point(window.selections.primary().cursor)
        .row;
    // The title's rows are of no file.
    let row = (rows.at(line))
        .filter(|row| !row.path.is_empty())
        .ok_or_else(|| CommandError::new("Nothing to visit here"))?;
    let model = ctx.editor.review(review);
    let shape = model.and_then(|model| model.file(row.path));
    let hunk_start = |index: usize| {
        let hunk = shape.and_then(|shape| shape.hunks().get(index))?;
        // A hunk that adds nothing goes to where it removed its lines.
        Some(if old_side {
            (Side::Old, hunk.old_start)
        } else {
            (Side::New, hunk.new_start)
        })
    };
    let first = (if old_side { Side::Old } else { Side::New }, 1);
    let (side, number) = match row.kind {
        RowKind::Lines { .. } => row
            .line()
            .ok_or_else(|| CommandError::new("Nothing to visit here"))?,
        RowKind::Hunk { hunk } => hunk_start(hunk).unwrap_or(first),
        RowKind::File | RowKind::Note => first,
        RowKind::Label { comment } | RowKind::Card { comment } => {
            let found = (model.into_iter())
                .flat_map(|model| model.comments_on(row.path))
                .find(|found| found.id() == comment);
            match found {
                Some(found) if !found.is_outdated() => match found.anchor() {
                    Anchor::Line { side, number } => (side, number),
                    Anchor::Hunk { index } => hunk_start(index).unwrap_or(first),
                },
                _ => first,
            }
        }
        RowKind::Filler => return Err(CommandError::new("Nothing to visit here")),
    };
    ctx.editor.request_review(ReviewRequest::Visit {
        review,
        path: row.path.into(),
        side,
        number,
        old_side,
    });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
#[path = "visit_tests.rs"]
mod tests;
