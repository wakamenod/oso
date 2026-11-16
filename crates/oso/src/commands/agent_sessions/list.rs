//! The keys of `*Spaces*` (ADR 0121, Q6, step 3; ADR 0140): moving between its rows, and acting on the
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

/// The marks a row of `*Spaces*` begins with, after its indent and guides: a fold, which every
/// space, repository, worktree and group has (ADR 0140), then each state's, then the frames of
/// a working mark that spins (ADR 0131). The list draws them, and never begins another line
/// with one.
pub const ROW_MARKS: &str = "▾▸⚠✗▶◆●○■⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";

/// The guides that hang a session's row from its worktree's (ADR 0140), before its mark.
pub const GUIDES: &str = "│├└";

/// What `?` says in `*Spaces*` (ADR 0131, ADR 0140): the marks, then the keys.
pub const LEGEND: &str = "⚠ needs input  ✗ failed  ▶ working  ◆ ready for review  ● done  ○ idle  ■ stopped · RET show  n/p move  M-n/M-p headings  TAB fold  S-TAB fold all  ←/→ close/open  q back  c new  w worktree  a add repo  / switch  s group  </> width  k stop  e explain";

/// How many columns `<` and `>` take from or give to the pane, once each (ADR 0131).
pub const PANE_STEP: isize = 4;

/// The folds' marks, which begin a space's, a repository's, a worktree's, a group's and a
/// quiet row's line (ADR 0140).
pub const FOLDS: [char; 2] = ['▾', '▸'];

/// The row `line`, a line of the list's text, begins, if it begins one: its indent in cells
/// past its guides, and its mark (ADR 0140). The one rule the keys and the mouse read rows by.
pub fn row_mark(line: impl IntoIterator<Item = char>) -> Option<(usize, char)> {
    let mut indent = 0;
    let first = line.into_iter().find(|c| {
        let lead = *c == ' ' || GUIDES.contains(*c);
        indent += usize::from(lead);
        !lead
    });
    first
        .filter(|c| ROW_MARKS.contains(*c))
        .map(|c| (indent, c))
}

/// The commands of `*Spaces*`.
pub const COMMANDS: &[Command] = &[
    row(
        "sessions-show",
        "Show the session on this row in the window selected last, and select that window.\n\nA session of another space switches to that space first.",
        Reach::Person,
        show,
    ),
    Command {
        name: "sessions-next",
        doc: "Move to the next row of `*Spaces*`: a session, a space, a repository, a worktree or a group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next,
        state: None,
    },
    Command {
        name: "sessions-previous",
        doc: "Move to the previous row of `*Spaces*`: a session, a space, a repository, a worktree or a group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous,
        state: None,
    },
    Command {
        name: "sessions-next-heading",
        doc: "Move to the next space, repository, worktree or group row of `*Spaces*`.\n\nIn the list by state, the next group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: next_heading,
        state: None,
    },
    Command {
        name: "sessions-previous-heading",
        doc: "Move to the previous space, repository, worktree or group row of `*Spaces*`.\n\nIn the list by state, the previous group.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: previous_heading,
        state: None,
    },
    Command {
        name: "sessions-wider",
        doc: "Make the `*Spaces*` pane 4 columns wider, times the count, up to 80.\n\nFor this oso: `[sessions] pane_width` gives the width it starts at.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: wider,
        state: None,
    },
    Command {
        name: "sessions-narrower",
        doc: "Make the `*Spaces*` pane 4 columns narrower, times the count, down to 24.\n\nFor this oso: `[sessions] pane_width` gives the width it starts at.",
        args: ArgSpec::Count,
        reach: Reach::Look,
        run: narrower,
        state: None,
    },
    row(
        "sessions-toggle-fold",
        "Fold or open the space, repository, worktree or group on this row, or the sessions nested under the session on it.",
        Reach::Look,
        fold,
    ),
    row(
        "sessions-cycle-folds",
        "Fold `*Spaces*` to its spaces, then open it down to its worktrees, then everything, in turn.\n\nIt starts from the deepest level an open row shows.",
        Reach::Look,
        cycle_folds,
    ),
    row(
        "sessions-open",
        "Open the folded row at point; on an open one, move to the first row under it.",
        Reach::Look,
        open,
    ),
    row(
        "sessions-close",
        "Fold the open row at point; on a session or a folded row, move to the row it is under.",
        Reach::Look,
        close,
    ),
    Command {
        name: "sessions-refresh",
        doc: "Render `*Spaces*` again, its ages as of now.",
        args: ArgSpec::None,
        reach: Reach::Look,
        run: refresh,
        state: None,
    },
    Command {
        name: "sessions-regroup",
        doc: "Group `*Spaces*` by state, or by space.",
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
        "Hang up the terminal of the session on this row of `*Spaces*`, or of the session whose terminal is this buffer.\n\nIt asks first while the session works or needs input.",
        Reach::Person,
        stop,
    ),
    Command {
        name: "sessions-help",
        doc: "Say what the marks of `*Spaces*` mean, and its keys, in the echo area.",
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
        return Err(CommandError::new("Not in *Spaces*"));
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

/// On a row of `*Spaces*`, or in a session's own terminal (`C-c K`, ADR 0132).
fn stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if ctx.editor.current().1.mode() == Mode::Sessions {
        return on_row(ctx, RowAction::Stop);
    }
    let buffer = super::mode::session_buffer(ctx)
        .map_err(|_| CommandError::new("Not in *Spaces* or a session's buffer"))?;
    ctx.editor
        .request_session(SessionRequest::OnBuffer(buffer, RowAction::Stop));
    Ok(())
}

fn explain(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    on_row(ctx, RowAction::Explain)
}

fn cycle_folds(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    ctx.editor.request_session(SessionRequest::CycleFolds);
    Ok(())
}

fn open(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    match row_at(ctx, ctx.editor.cursor_line()) {
        Some((_, '▸')) => on_row(ctx, RowAction::Fold),
        Some((_, '▾')) => step(ctx, 1, Stop::Row),
        _ => Ok(()),
    }
}

fn close(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    in_list(ctx)?;
    let line = ctx.editor.cursor_line();
    match row_at(ctx, line) {
        Some((_, '▾')) => on_row(ctx, RowAction::Fold),
        Some((indent, _)) => {
            // The nearest row above that is further out: the one this one is under.
            let parent = (0..line)
                .rev()
                .find(|&above| row_at(ctx, above).is_some_and(|(outer, _)| outer < indent));
            if let Some(parent) = parent {
                go_to(ctx, parent);
            }
            Ok(())
        }
        None => Ok(()),
    }
}

/// Line `line` of the selected buffer's indent, in cells past its guides, and its first mark,
/// if it begins a row.
fn row_at(ctx: &Ctx<'_>, line: usize) -> Option<(usize, char)> {
    let text = ctx.editor.current().1.text();
    if line >= text.line_count() {
        return None;
    }
    let (start, end) = (text.line_start(line), text.line_end(line));
    row_mark(text.chunks_in_range(start..end).flat_map(str::chars))
}

/// Puts point at the start of line `line` of the selected buffer, in view.
fn go_to(ctx: &mut Ctx<'_>, line: usize) {
    let offset = ctx.editor.current().1.text().line_start(line);
    ctx.editor.current_mut().0.selections = SelectionSet::new(offset);
    ctx.editor.follow_cursor();
    ctx.request_redraw();
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
    let stops = |row: usize| match (stop, row_at(ctx, row)) {
        (_, None) => false,
        (Stop::Row, Some(_)) => true,
        (Stop::Heading, Some((_, mark))) => is_heading(mark),
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
    if row != from {
        go_to(ctx, row);
    }
    Ok(())
}

/// Whether a row that begins with `mark` is a heading: a fold's mark, which a space, a
/// repository, a worktree and a group have and a session never does (ADR 0140).
fn is_heading(mark: char) -> bool {
    FOLDS.contains(&mark)
}

#[cfg(test)]
mod tests;
