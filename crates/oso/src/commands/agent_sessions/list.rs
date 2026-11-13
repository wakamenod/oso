//! The keys of `*Sessions*` (ADR 0121, Q6, step 3): moving between its rows, and acting on the
//! row point is on. Each asks the host, which knows what each line shows.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Showing, folding, regrouping and explaining are `Look`; pinning and archiving change what
//! oso remembers, `Change`; resuming and stopping run or end a process, `Person`.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{RowAction, SessionRequest};
use oso_core::selection::SelectionSet;

/// The commands of `*Sessions*`.
pub const COMMANDS: &[Command] = &[
    row(
        "sessions-show",
        "Show the session on this row in the window selected last, and select that window.\n\nA session of another space switches to that space first.",
        Reach::Look,
        show,
    ),
    Command {
        name: "sessions-next",
        doc: "Move to the next row of `*Sessions*`: a session or a group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next,
        state: None,
    },
    Command {
        name: "sessions-previous",
        doc: "Move to the previous row of `*Sessions*`: a session or a group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous,
        state: None,
    },
    row(
        "sessions-toggle-fold",
        "Fold or open the group on this row, or the sessions nested under the session on it.",
        Reach::Look,
        fold,
    ),
    Command {
        name: "sessions-refresh",
        doc: "Render `*Sessions*` again, its ages as of now.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: refresh,
        state: None,
    },
    Command {
        name: "sessions-regroup",
        doc: "Group `*Sessions*` by state, or by space.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: regroup,
        state: None,
    },
    row(
        "sessions-pin",
        "Pin the session on this row, or unpin it: pinned sessions come first in their group.",
        Reach::Change,
        pin,
    ),
    row(
        "sessions-archive",
        "Archive the session on this row, or bring an archived one back.\n\nAn archived session is listed last, folded, and is not brought back at the next start.",
        Reach::Change,
        archive,
    ),
    row(
        "sessions-resume",
        "Resume the stopped session on this row, as `session-resume` does in its buffer.",
        Reach::Person,
        resume,
    ),
    row(
        "session-stop",
        "Hang up the terminal of the session on this row.\n\nIt asks first while the session works or needs input.",
        Reach::Person,
        stop,
    ),
    row(
        "sessions-explain",
        "Explain the last look at the screen of the session on this row, as `session-explain` does.",
        Reach::Look,
        explain,
    ),
];

/// A command of a row, with no argument.
const fn row(
    name: &'static str,
    doc: &'static str,
    reach: Reach,
    run: fn(&mut Ctx<'_>, Args) -> Result<(), CommandError>,
) -> Command {
    Command {
        name,
        doc,
        args: ArgSpec::None,
        reach,
        run,
        state: None,
    }
}

/// Asks the host to `action` the row point is on.
fn on_row(ctx: &mut Ctx<'_>, action: RowAction) -> Result<(), CommandError> {
    in_list(ctx)?;
    let line = ctx.editor.cursor_line();
    ctx.editor
        .request_session(SessionRequest::Row { line, action });
    Ok(())
}

fn in_list(ctx: &Ctx<'_>) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    if ctx.editor.current().1.mode() != Mode::Sessions {
        return Err(CommandError::new("Not in *Sessions*"));
    }
    Ok(())
}

fn show(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Show)
}

fn fold(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Fold)
}

fn pin(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Pin)
}

fn archive(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Archive)
}

fn resume(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Resume)
}

fn stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Stop)
}

fn explain(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Explain)
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.request_session(SessionRequest::Refresh);
    Ok(())
}

fn regroup(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.request_session(SessionRequest::Regroup);
    Ok(())
}

fn next(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count())
}

fn previous(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count().saturating_neg())
}

/// Moves point `count` rows down, up if negative: to the start of a line that begins a row, a
/// session's first line or a group's header. Stays at the last row it reaches.
fn step(ctx: &mut Ctx<'_>, count: isize) -> Result<(), CommandError> {
    in_list(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let text = buffer.text();
    let from = text.offset_to_point(window.selections.primary().cursor).row;
    let last = text.line_count().saturating_sub(1);
    let starts_row = |row: usize| {
        let (start, end) = (text.line_start(row), text.line_end(row));
        let first = text
            .chunks_in_range(start..end)
            .flat_map(str::chars)
            .find(|c| *c != ' ');
        first.is_some_and(|c| "▾▸⚠✗▶◆●○■".contains(c))
    };
    let mut row = from;
    for _ in 0..count.unsigned_abs() {
        let found = if count > 0 {
            (row + 1..=last).find(|&r| starts_row(r))
        } else {
            (0..row).rev().find(|&r| starts_row(r))
        };
        match found {
            Some(found) => row = found,
            None => break,
        }
    }
    if row == from {
        return Ok(());
    }
    let offset = text.line_start(row);
    ctx.editor.current_mut().0.selections = SelectionSet::new(offset);
    ctx.editor.follow_cursor();
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests;
