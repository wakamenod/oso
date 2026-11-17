//! The decision keys of a review buffer (ADR 0122, "Decisions", step 5): `A` accepts, `x`
//! rejects, `v` marks viewed and `u` undoes the last decision. Each takes the hunk at point, or
//! every hunk of the file on its header, and asks the host, which runs git or edits the file.
//!
//! Must not depend on: the host's threads or git; this finds what is at point and asks.
//!
//! - Every one is `Person`: they stage, revert and save at the person's word, and viewed is the
//!   person's record of what they read. No agent tool decides.
//! - `x` asks first when the file's buffer holds unsaved edits, as the revert saves the whole
//!   buffer (Q-C).

use std::path::{Component, Path, PathBuf};

use oso_core::buffer::BufferId;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, ReviewId, ReviewRequest};
use oso_core::review::RowKind;

use super::current_review;

/// The most cells of a buffer's name the question shows.
const NAME_CELLS: usize = 40;

/// The commands that decide on hunks.
pub const COMMANDS: &[Command] = &[
    person_command!(
        "review-accept",
        "Accept the hunk at the primary cursor, or every hunk of the file on its header: stage it, as the review shows it.\n\nOnly a modified file's hunks in a review of the work tree are accepted; in the staged scope they are staged already. `u` undoes it.",
        review_accept
    ),
    person_command!(
        "review-reject",
        "Reject the hunk at the primary cursor, or every hunk of the file on its header: put its old lines back in the file, through the file's buffer as one undo step, and save it. In the staged scope, unstage it.\n\nIt refuses if the hunk's lines are no longer where the review read them, and asks first if the file's buffer holds unsaved edits, as it saves the whole buffer. `u` undoes it.",
        review_reject
    ),
    person_command!(
        "review-viewed",
        "Mark the hunk at the primary cursor, or every hunk of the file on its header, viewed, or clear the mark if each has it. `u` undoes it.",
        review_viewed
    ),
    person_command!(
        "review-undo",
        "Undo the review's last decision: unstage what was staged, put back what was reverted, or clear a viewed mark.",
        review_undo
    ),
];

/// What a decision is on: the review, the file, and its hunks by their index in the model's
/// reading.
type Target = (ReviewId, Box<[u8]>, Vec<usize>);

/// What the line at the primary cursor decides on.
fn target(ctx: &Ctx<'_>) -> Result<Target, CommandError> {
    let review = current_review(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let rows = (ctx.editor.review_rows_of(window.buffer()))
        .ok_or_else(|| CommandError::new("Not in a review"))?;
    let line = (buffer.text())
        .offset_to_point(window.selections.primary().cursor)
        .row;
    let row = rows
        .at(line)
        .ok_or_else(|| CommandError::new("Not on a hunk"))?;
    let hunks = match row.kind {
        RowKind::Hunk { hunk } | RowKind::Lines { hunk, .. } => vec![hunk],
        RowKind::File => {
            let shape = (ctx.editor.review(review)).and_then(|model| model.file(row.path));
            (0..shape.map_or(0, |shape| shape.hunks().len())).collect()
        }
        RowKind::Filler => return Err(CommandError::new("Nothing to decide here")),
        _ => return Err(CommandError::new("Not on a hunk")),
    };
    if hunks.is_empty() {
        return Err(CommandError::new("No hunk to decide on"));
    }
    if ctx.editor.review_file_behind(review, row.path) {
        return Err(CommandError::new(
            "This file is being read again: try in a moment",
        ));
    }
    Ok((review, row.path.into(), hunks))
}

/// `path`, as git names it, under `root`, if it is plain names and nothing of git's own: where
/// a decision's file is, the same for the command that asks and the host that reverts.
pub(crate) fn place_of(root: &Path, path: &[u8]) -> Option<PathBuf> {
    let relative = Path::new(crate::git::os_path(path).ok()?);
    let plain = |part: Component<'_>| match part {
        Component::Normal(name) => {
            let name = name.to_string_lossy().to_ascii_lowercase();
            let name = name.trim_end_matches(['.', ' ']);
            name != ".git" && name != "git~1"
        }
        _ => false,
    };
    let fine = !relative.as_os_str().is_empty() && relative.components().all(plain);
    fine.then(|| root.join(relative))
}

/// The open buffer of the file at `place`, but one the agent opened and the person has not
/// seen.
pub(crate) fn open_buffer_of(editor: &Editor, place: &Path) -> Option<BufferId> {
    (editor.buffers())
        .find(|(_, buffer)| buffer.path() == Some(place) && !buffer.agent_pending())
        .map(|(id, _)| id)
}

/// The open buffer of the file at `path` of the review `review`'s project, if any.
fn file_buffer(editor: &Editor, review: ReviewId, path: &[u8]) -> Option<BufferId> {
    let project = editor
        .reviews()
        .find_map(|(id, project)| (id == review).then_some(project))?;
    let root = editor.project(project)?.root();
    open_buffer_of(editor, &place_of(root, path)?)
}

fn review_accept(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, path, hunks) = target(ctx)?;
    ctx.editor.request_review(ReviewRequest::Accept {
        review,
        path,
        hunks,
    });
    ctx.request_redraw();
    Ok(())
}

fn review_reject(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, path, hunks) = target(ctx)?;
    let edited = file_buffer(ctx.editor, review, &path)
        .and_then(|buffer| ctx.editor.buffer(buffer))
        .filter(|buffer| buffer.is_modified());
    if let Some(buffer) = edited
        && !ctx.confirmed()
    {
        let (name, cut) = oso_core::minibuffer::cut_to(buffer.name(), NAME_CELLS);
        let name = format!("{name}{cut}");
        let question = format!("{name} has unsaved edits; revert the hunk and save it?");
        return super::super::session::ask(ctx, question, "review-reject", ["revert", "keep"]);
    }
    ctx.editor.request_review(ReviewRequest::Reject {
        review,
        path,
        hunks,
    });
    ctx.request_redraw();
    Ok(())
}

fn review_viewed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (review, path, hunks) = target(ctx)?;
    ctx.editor.request_review(ReviewRequest::Viewed {
        review,
        path,
        hunks,
    });
    ctx.request_redraw();
    Ok(())
}

fn review_undo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    ctx.editor.request_review(ReviewRequest::Undo(review));
    ctx.request_redraw();
    Ok(())
}
