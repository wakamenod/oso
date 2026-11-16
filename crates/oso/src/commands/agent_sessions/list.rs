//! The keys of `*Sessions*` (ADR 0121, Q6, step 3): moving between its rows, and acting on the
//! row point is on. Each asks the host, which knows what each line shows.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host for the rest.
//!
//! Moving, by row or by heading, folding, regrouping, the pane's width and explaining are
//! `Look`. Showing a session puts its terminal
//! in front of the person's keys, pinning and archiving change which sessions ask for them,
//! and resuming and stopping run or end a process: those are `Person`.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{RowAction, SessionRequest};
use oso_core::selection::SelectionSet;

/// The marks a row of `*Sessions*` begins with, after its indent: a group's fold, then each
/// state's, a worktree's with no session, then the frames of a working mark that spins
/// (ADR 0131). The list draws them, and never begins another line with one.
pub const ROW_MARKS: &str = "▾▸⚠✗▶◆●○■·⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

/// What `?` says in `*Sessions*` (ADR 0131): the marks, then the keys.
pub const LEGEND: &str = "⚠ needs input  ✗ failed  ▶ working  ◆ ready for review  ● done  ○ idle  ■ stopped · RET show  n/p move  M-n/M-p headings  TAB fold  c new  w worktree  a add repo  / switch  s group  </> width  k stop  e explain";

/// How many columns `<` and `>` take from or give to the pane, once each (ADR 0131).
pub const PANE_STEP: isize = 4;

/// The folds' marks, which begin a space's, a repository's, a group's and a quiet row's line.
const FOLDS: [char; 2] = ['▾', '▸'];

/// The commands of `*Sessions*`.
pub const COMMANDS: &[Command] = &[
    row(
        "sessions-show",
        "Show the session on this row in the window selected last, and select that window.\n\nA session of another space switches to that space first.",
        Reach::Person,
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
    Command {
        name: "sessions-next-heading",
        doc: "Move to the next project, worktree or group row of `*Sessions*`.\n\nIn the list by state, the next group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next_heading,
        state: None,
    },
    Command {
        name: "sessions-previous-heading",
        doc: "Move to the previous project, worktree or group row of `*Sessions*`.\n\nIn the list by state, the previous group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous_heading,
        state: None,
    },
    Command {
        name: "sessions-wider",
        doc: "Make the `*Sessions*` pane 4 columns wider, times the count, up to 80.\n\nFor this oso: `[sessions] pane_width` gives the width it starts at.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: wider,
        state: None,
    },
    Command {
        name: "sessions-narrower",
        doc: "Make the `*Sessions*` pane 4 columns narrower, times the count, down to 24.\n\nFor this oso: `[sessions] pane_width` gives the width it starts at.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: narrower,
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
        Reach::Person,
        pin,
    ),
    row(
        "sessions-archive",
        "Archive the session on this row, or bring an archived one back.\n\nAn archived session is listed last, folded, and is not brought back at the next start.",
        Reach::Person,
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
        "Hang up the terminal of the session on this row of `*Sessions*`, or of the session whose terminal is this buffer.\n\nIt asks first while the session works or needs input.",
        Reach::Person,
        stop,
    ),
    Command {
        name: "sessions-help",
        doc: "Say what the marks of `*Sessions*` mean, and its keys, in the echo area.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: help,
        state: None,
    },
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

/// On a row of `*Sessions*`, or in a session's own terminal (`C-c a K`, ADR 0143).
fn stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.editor.current().1.mode() == Mode::Sessions {
        return on_row(ctx, RowAction::Stop);
    }
    let buffer = super::mode::session_buffer(ctx)
        .map_err(|_| CommandError::new("Not in *Sessions* or a session's buffer"))?;
    ctx.editor
        .request_session(SessionRequest::OnBuffer(buffer, RowAction::Stop));
    Ok(())
}

fn explain(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Explain)
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.request_session(SessionRequest::Refresh);
    Ok(())
}

fn help(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.message(LEGEND);
    Ok(())
}

fn regroup(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.request_session(SessionRequest::Regroup);
    Ok(())
}

fn next(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count(), Stop::Row)
}

fn previous(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count().saturating_neg(), Stop::Row)
}

fn wider(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    pane_width(ctx, args.count())
}

fn narrower(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    pane_width(ctx, args.count().saturating_neg())
}

/// Asks the host for the pane `steps` of [`PANE_STEP`] wider, narrower if negative.
fn pane_width(ctx: &mut Ctx<'_>, steps: isize) -> Result<(), CommandError> {
    in_list(ctx)?;
    let columns = steps.saturating_mul(PANE_STEP);
    ctx.editor
        .request_session(SessionRequest::PaneWidth(columns));
    Ok(())
}

fn next_heading(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count(), Stop::Heading)
}

fn previous_heading(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    step(ctx, args.count().saturating_neg(), Stop::Heading)
}

/// Where a move of point stops.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// At any row: a session's first line, a worktree's, a project's or a group's.
    Row,
    /// At a space's, a repository's, a worktree's or a group's row (ADR 0131, ADR 0136).
    Heading,
}

/// Moves point `count` rows down, up if negative, to the start of a line `stop` stops at.
/// Stays at the last it reaches.
fn step(ctx: &mut Ctx<'_>, count: isize, stop: Stop) -> Result<(), CommandError> {
    in_list(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let text = buffer.text();
    let from = text.offset_to_point(window.selections.primary().cursor).row;
    let last = text.line_count().saturating_sub(1);
    // A line's indent and its first mark, if it begins a row.
    let row_at = |row: usize| {
        let (start, end) = (text.line_start(row), text.line_end(row));
        let mut chars = text.chunks_in_range(start..end).flat_map(str::chars);
        let mut indent = 0;
        let first = chars.find(|c| {
            indent += usize::from(*c == ' ');
            *c != ' '
        });
        first
            .filter(|c| ROW_MARKS.contains(*c))
            .map(|c| (indent, c))
    };
    let stops = |row: usize| match (stop, row_at(row)) {
        (_, None) => false,
        (Stop::Row, Some(_)) => true,
        // The nearest row above with less indent: the fold row a worktree's row is under.
        (Stop::Heading, Some((indent, mark))) => is_heading(indent, mark, || {
            (0..row)
                .rev()
                .find_map(|r| row_at(r).filter(|(above, _)| *above < indent))
        }),
    };
    let mut row = from;
    for _ in 0..count.unsigned_abs() {
        let found = if count > 0 {
            (row + 1..=last).find(|&r| stops(r))
        } else {
            (0..row).rev().find(|&r| stops(r))
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

/// Whether a row `indent` cells in that begins with `mark` is a heading: a fold's mark, or a
/// worktree's row, two cells in from the fold row above it: a space's, or a repository's in a
/// space of several (ADR 0136, step 2). `parent` gives the nearest row above with less indent,
/// its indent and mark; a session is never two cells in from a fold row.
fn is_heading(indent: usize, mark: char, parent: impl FnOnce() -> Option<(usize, char)>) -> bool {
    FOLDS.contains(&mark)
        || parent().is_some_and(|(above, mark)| above + 2 == indent && FOLDS.contains(&mark))
}

#[cfg(test)]
mod tests;
