//! Source control's commands (ADR 0127), step 1a: a repository's status buffer, as magit's,
//! moving between its sections, showing a file's hunks, and staging and unstaging by section,
//! file and hunk. Committing and the log of git's runs are [`commit`]'s (step 1b), discarding
//! and the list of what it kept [`discard`]'s (step 1c), remotes [`remote`]'s (step 2), and
//! going on with what is stopped [`sequence`]'s (step 3a), and stashes [`stash`]'s and branches
//! [`branch`]'s (step 3b), and the graph [`graph`]'s (step 4).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host, which runs git on the status's thread. What a line names is the host's: a
//! command says which lines it means, and the host reads its table, never the text. Every
//! command that makes the host run git, or opens a file, is `Person` (ADR 0085); moving and the
//! keys' list are `Look`.

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{Editor, GitHeading, GitLines, GitRequest};
use oso_core::project::ProjectId;
use oso_core::selection::Selection;

pub mod branch;
pub mod commit;
pub mod discard;
pub mod graph;
pub mod pr;
pub mod remote;
pub mod sequence;
pub mod stash;

/// Why a restricted project gets no status (ADR 0127, "Restricted projects").
pub(crate) const RESTRICTED: &str =
    "The status runs git, which this project is not trusted to run: M-x trust-project";

/// What the buffer `git-status-keys` shows is named.
const KEYS_BUFFER: &str = "*Git keys*";

macro_rules! command {
    ($name:literal, $doc:literal, $reach:ident, $args:expr, $run:ident) => {
        Command {
            name: $name,
            doc: $doc,
            args: $args,
            reach: Reach::$reach,
            run: $run,
            state: None,
        }
    };
}

/// Source control's commands.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-status",
        "Show the git status of the selected window's project, as magit's status buffer: its head, then its unmerged, untracked, unstaged and staged files, each section folding.\n\nIn it, `s` stages and `u` unstages what point is on: a section, a file or a hunk; `TAB` shows a file's hunks or folds a section; `g` reads it again. It runs git, so only in a trusted project.",
        Person,
        ArgSpec::None,
        git_status
    ),
    command!(
        "git-refresh",
        "In a git status, read the status again (`g`), keeping point on what it was on.",
        Person,
        ArgSpec::None,
        refresh
    ),
    command!(
        "git-toggle",
        "In a git status, show or hide the hunks of the file at point, or fold or unfold the section at point (`TAB`).\n\nOn a hunk, it hides its file's hunks. A file that may hold secrets shows no hunks: it stages as a whole. It acts at the primary cursor.",
        Person,
        ArgSpec::None,
        toggle
    ),
    command!(
        "git-stage",
        "In a git status, stage what each cursor is on: a section's files, a file, or a hunk (`s`).\n\nA region stages each section, file and hunk whose heading it holds.",
        Person,
        ArgSpec::None,
        stage
    ),
    command!(
        "git-unstage",
        "In a git status, unstage what each cursor is on among the staged changes: the section, a file, or a hunk (`u`).",
        Person,
        ArgSpec::None,
        unstage
    ),
    command!(
        "git-stage-all",
        "In a git status, stage every unstaged change of tracked files (`S`). Untracked files stay untracked.",
        Person,
        ArgSpec::None,
        stage_all
    ),
    command!(
        "git-unstage-all",
        "In a git status, unstage every staged change (`U`).",
        Person,
        ArgSpec::None,
        unstage_all
    ),
    command!(
        "git-visit",
        "In a git status, visit the file at the primary cursor, at the line a hunk's line shows (`RET`).",
        Person,
        ArgSpec::None,
        visit
    ),
    command!(
        "git-next-section",
        "In a git status, move to the next heading: a section's, a file's or a hunk's (`n`).",
        Look,
        ArgSpec::Count,
        next_section
    ),
    command!(
        "git-previous-section",
        "In a git status, move to the previous heading (`p`).",
        Look,
        ArgSpec::Count,
        previous_section
    ),
    command!(
        "git-next-sibling",
        "In a git status, move to the next heading of the same depth in the same section (`M-n`).",
        Look,
        ArgSpec::Count,
        next_sibling
    ),
    command!(
        "git-previous-sibling",
        "In a git status, move to the previous heading of the same depth in the same section (`M-p`).",
        Look,
        ArgSpec::Count,
        previous_sibling
    ),
    command!(
        "git-up-section",
        "In a git status, move to the heading of the section that holds point's (`^`).",
        Look,
        ArgSpec::None,
        up_section
    ),
    command!(
        "git-status-keys",
        "Show the keys of a git status, each with what its command does, in `*Git keys*`.",
        Look,
        ArgSpec::None,
        status_keys
    ),
];

/// The keys of a git status and their commands, as magit's, which `bindings` binds in the
/// status's keymap and `git-status-keys` lists.
pub const KEYS: &[(&str, &str)] = &[
    ("TAB", "git-toggle"),
    ("s", "git-stage"),
    ("u", "git-unstage"),
    ("S", "git-stage-all"),
    ("U", "git-unstage-all"),
    ("k", "git-discard"),
    ("$", "git-show-log"),
    ("C-c C-c", "git-continue"),
    ("C-c C-s", "git-skip"),
    ("C-c C-k", "git-abort"),
    ("RET", "git-visit"),
    ("g", "git-refresh"),
    ("n", "git-next-section"),
    ("p", "git-previous-section"),
    ("M-n", "git-next-sibling"),
    ("M-p", "git-previous-sibling"),
    ("^", "git-up-section"),
    ("q", "quit-window"),
    ("?", "git-status-keys"),
];

/// The project `C-x g` and `C-x v l` show: the status's or the graph's when the selected window
/// shows one, else the project of the buffer it shows.
pub(crate) fn status_project(editor: &Editor) -> Option<ProjectId> {
    let buffer = editor.tiled().0.buffer();
    (editor.git_status_of(buffer))
        .or_else(|| editor.git_graph_of(buffer))
        .or_else(|| editor.git_pr_of(buffer))
        .or_else(|| editor.buffer(buffer)?.project())
}

/// Whether the prompt over `list`, one of source control's, keeps the host's order.
pub fn listed_in_given_order(list: &str) -> bool {
    branch::listed_in_given_order(list)
        || list == graph::GRAPH_REFS_LIST
        || list == graph::GRAPH_LABELS_LIST
}

/// Whether the prompt over `list`, one of source control's, takes what is typed.
pub fn listed_takes_input(list: &str) -> bool {
    remote::listed_takes_input(list)
        || branch::listed_takes_input(list)
        || graph::listed_takes_input(list)
}

/// The status buffer the selected window shows, or why the command means nothing here.
pub(super) fn status(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitStatus || ctx.editor.git_status_of(window.buffer()).is_none() {
        return Err(CommandError::new("Not in a git status"));
    }
    Ok(window.buffer())
}

/// Asks the host for what `make` makes of the status the selected window shows, outside a
/// prompt: what the commands of remotes and of what is stopped share.
fn request(
    ctx: &mut Ctx<'_>,
    make: impl FnOnce(BufferId) -> GitRequest,
) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let buffer = status(ctx)?;
    ctx.editor.request_git(make(buffer));
    ctx.request_redraw();
    Ok(())
}

/// The line of the selected window's primary cursor.
fn point_line(ctx: &Ctx<'_>) -> usize {
    let (window, buffer) = ctx.editor.current();
    buffer
        .text()
        .offset_to_point(window.selections.primary().cursor)
        .row
}

/// The lines each cursor of the selected window covers, in order: its region's while the mark
/// is active, else its own line.
pub(super) fn cursor_lines(ctx: &Ctx<'_>) -> Vec<GitLines> {
    let (window, buffer) = ctx.editor.current();
    let text = buffer.text();
    let region = window.selections.mark_active().is_active();
    let mut lines: Vec<GitLines> = window
        .selections
        .iter()
        .map(|selection| {
            let range = selection.range();
            let first = text.offset_to_point(range.start).row;
            let mut last = text.offset_to_point(range.end).row;
            // A region that ends at a line's start does not hold that line, as Emacs's
            // region-based commands read it.
            if region && last > first && text.line_start(last) == range.end {
                last -= 1;
            }
            GitLines {
                first,
                last,
                region: region && !range.is_empty(),
            }
        })
        .collect();
    lines.sort_unstable_by_key(|lines| (lines.first, lines.last));
    lines.dedup();
    lines
}

fn git_status(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let project =
        status_project(ctx.editor).ok_or_else(|| CommandError::new("Not in a project"))?;
    if !ctx.editor.project_trusted(project) {
        return Err(CommandError::new(RESTRICTED));
    }
    ctx.editor.request_git(GitRequest::Status { project });
    ctx.request_redraw();
    Ok(())
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    ctx.editor.request_git(GitRequest::Refresh { buffer });
    Ok(())
}

fn toggle(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    let line = point_line(ctx);
    ctx.editor.request_git(GitRequest::Toggle { buffer, line });
    Ok(())
}

fn stage(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    let lines = cursor_lines(ctx);
    ctx.editor.request_git(GitRequest::Stage { buffer, lines });
    deactivate_mark(ctx);
    Ok(())
}

fn unstage(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    let lines = cursor_lines(ctx);
    ctx.editor
        .request_git(GitRequest::Unstage { buffer, lines });
    deactivate_mark(ctx);
    Ok(())
}

/// Hides the region a command used, as Emacs does after one.
pub(super) fn deactivate_mark(ctx: &mut Ctx<'_>) {
    let (window, _) = ctx.editor.current_mut();
    if window.selections.mark_active().is_active() {
        window
            .selections
            .set_mark_active(oso_core::selection::MarkActive::Inactive);
        ctx.request_redraw();
    }
}

fn stage_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    ctx.editor.request_git(GitRequest::StageAll { buffer });
    Ok(())
}

fn unstage_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    ctx.editor.request_git(GitRequest::UnstageAll { buffer });
    Ok(())
}

fn visit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    let (window, shown) = ctx.editor.current();
    let point = shown
        .text()
        .offset_to_point(window.selections.primary().cursor);
    ctx.editor.request_git(GitRequest::Visit {
        buffer,
        line: point.row,
        column: point.column,
    });
    ctx.request_redraw();
    Ok(())
}

/// Where a cursor on `row` goes, given the outline `heads`.
type Step = fn(&[GitHeading], usize) -> Option<usize>;

/// Moves every cursor by `step`, `count` times, to the start of the heading's line; one that
/// has nowhere to go stays. The outline is read in place, each cursor's place found under it
/// first and then put in.
fn move_by(ctx: &mut Ctx<'_>, count: isize, forward: Step, back: Step) -> Result<(), CommandError> {
    let buffer = status(ctx)?;
    let step = if count < 0 { back } else { forward };
    let times = count.unsigned_abs().max(1);
    let (window, shown) = ctx.editor.current();
    let heads = ctx.editor.git_outline(buffer);
    if heads.is_empty() {
        return Ok(());
    }
    let text = shown.text();
    let mut moves: Vec<(usize, usize)> = window
        .selections
        .iter()
        .map(|selection| {
            let mut row = text.offset_to_point(selection.cursor).row;
            for _ in 0..times {
                match step(heads, row) {
                    Some(next) => row = next,
                    None => break,
                }
            }
            (
                selection.cursor,
                text.line_start(row.min(text.max_point().row)),
            )
        })
        .collect();
    moves.sort_unstable();
    let (window, _) = ctx.editor.current_mut();
    let update = window.selections.update(|selection: &mut Selection| {
        if let Ok(at) = moves.binary_search_by_key(&selection.cursor, |(from, _)| *from) {
            selection.cursor = moves[at].1;
            selection.goal = None;
        }
        Ok::<(), ()>(())
    });
    if update.changed {
        ctx.request_redraw();
    }
    Ok(())
}

/// The next heading below `row`. O(log n): the outline is in the text's order.
pub(crate) fn next_head(heads: &[GitHeading], row: usize) -> Option<usize> {
    let at = heads.partition_point(|head| head.line <= row);
    heads.get(at).map(|head| head.line)
}

/// The previous heading above `row`.
pub(crate) fn previous_head(heads: &[GitHeading], row: usize) -> Option<usize> {
    let at = heads.partition_point(|head| head.line < row);
    at.checked_sub(1).map(|at| heads[at].line)
}

/// The index of the heading `row` is under, if any.
fn current(heads: &[GitHeading], row: usize) -> Option<usize> {
    heads
        .partition_point(|head| head.line <= row)
        .checked_sub(1)
}

/// The next heading of the same depth as the one `row` is under, before its parent ends; before
/// the first heading, the first one.
pub(crate) fn next_sibling_head(heads: &[GitHeading], row: usize) -> Option<usize> {
    let Some(at) = current(heads, row) else {
        return heads.first().map(|head| head.line);
    };
    let depth = heads[at].depth;
    heads[at + 1..]
        .iter()
        .take_while(|head| head.depth >= depth)
        .find(|head| head.depth == depth)
        .map(|head| head.line)
}

/// The previous heading of the same depth as the one `row` is under, after its parent's.
pub(crate) fn previous_sibling_head(heads: &[GitHeading], row: usize) -> Option<usize> {
    let at = current(heads, row)?;
    let depth = heads[at].depth;
    heads[..at]
        .iter()
        .rev()
        .take_while(|head| head.depth >= depth)
        .find(|head| head.depth == depth)
        .map(|head| head.line)
}

/// The heading of the section that holds the one `row` is under.
pub(crate) fn parent_head(heads: &[GitHeading], row: usize) -> Option<usize> {
    let at = current(heads, row)?;
    let depth = heads[at].depth;
    (heads[..at].iter().rev())
        .find(|head| head.depth < depth)
        .map(|head| head.line)
}

fn next_section(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count(), next_head, previous_head)
}

fn previous_section(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count().saturating_neg(), next_head, previous_head)
}

fn next_sibling(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(ctx, args.count(), next_sibling_head, previous_sibling_head)
}

fn previous_sibling(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    move_by(
        ctx,
        args.count().saturating_neg(),
        next_sibling_head,
        previous_sibling_head,
    )
}

fn up_section(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    move_by(ctx, 1, parent_head, parent_head)
}

fn status_keys(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let text = keys_text();
    let shown = ctx
        .editor
        .buffer_named(KEYS_BUFFER)
        .filter(|id| ctx.editor.buffer(*id).is_some_and(|b| b.is_internal()));
    let id = match shown {
        Some(id) => id,
        None => ctx
            .editor
            .add_internal_buffer(KEYS_BUFFER, Mode::Fundamental),
    };
    ctx.editor.set_internal_text(id, &text);
    let window = ctx.editor.window().id();
    ctx.editor.show_in(window, id);
    ctx.request_redraw();
    Ok(())
}

/// The menus a git status's keys open, as `(keys, menu)`.
pub const MENUS: &[(&str, &str)] = &[
    ("b", "branch"),
    ("c", "commit"),
    ("P", "push"),
    ("F", "pull"),
    ("f", "fetch"),
    ("l", "log"),
    ("N", "pull-request"),
    ("z", "stash"),
];

/// The first line of the doc of the source-control command `name`, if it is one.
fn summary_of(name: &str) -> Option<&'static str> {
    let all = COMMANDS.iter().chain(commit::COMMANDS);
    let all = all.chain(discard::COMMANDS).chain(remote::COMMANDS);
    let all = all.chain(sequence::COMMANDS).chain(stash::COMMANDS);
    let all = all.chain(branch::COMMANDS).chain(graph::COMMANDS);
    let mut all = all.chain(pr::COMMANDS);
    let found = all.find(|command| command.name == name)?;
    found.doc.lines().next()
}

/// What `*Git keys*` says: each key, and its command's name and summary.
fn keys_text() -> String {
    let mut text = String::from("Keys in a git status\n\n");
    for (keys, name) in KEYS {
        let summary = summary_of(name);
        // `q` is the buffers' own `quit-window`.
        let summary = match (*name, summary) {
            ("quit-window", _) => "Leave the status, showing the buffer shown before it.",
            (_, summary) => summary.unwrap_or_default(),
        };
        text.push_str(&format!("{keys:<6}{name:<24}{summary}\n"));
    }
    for (keys, menu) in MENUS {
        text.push_str(&format!("{keys:<6}{menu} menu\n"));
    }
    text.push_str("\nC-x g shows the status from any buffer of the project; C-x k kills it.\n");
    text
}

#[cfg(test)]
mod tests;
