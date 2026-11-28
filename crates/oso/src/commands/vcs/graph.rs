//! Source control's commands for the graph (ADR 0127, step 4): showing a repository's commits
//! as a graph from anywhere in its project (`C-x v l`) or from the log menu (`l`), and the keys
//! of a graph buffer: reviewing the commit on a line, copying its id, showing every ref, and
//! reading it again; from step 4b marking commits, checking out, making a branch or a tag,
//! cherry-picking and reverting, and the prompts and the question those open.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which reads the
//! graph on a thread of its own. Every command here that makes the host run git, open a review
//! or write the clipboard is `Person` (ADR 0085).

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{GitRequest, GraphAct, GraphScope};
use oso_core::project::ProjectId;

/// Why a restricted project gets no graph (ADR 0127, "Restricted projects").
pub(crate) const RESTRICTED: &str =
    "The graph runs git, which this project is not trusted to run: M-x trust-project";

/// The list of refs `l o` completes over, `HEAD` first, which takes any revision typed.
pub const GRAPH_REFS_LIST: &str = "git-graph-refs";
/// The list of branches on a line `c` asks over when there are several (step 4b).
pub const GRAPH_LABELS_LIST: &str = "git-graph-labels";

/// Whether the prompt over `list` takes what is typed, as a revision to show.
pub fn listed_takes_input(list: &str) -> bool {
    list == GRAPH_REFS_LIST
}

/// The keys of a graph buffer and their commands, which `bindings` binds in its keymap.
pub const KEYS: &[(&str, &str)] = &[
    ("RET", "git-graph-review"),
    ("w", "git-graph-copy-id"),
    ("a", "git-graph-toggle-all"),
    ("g", "git-graph-refresh"),
    ("m", "git-graph-mark"),
    ("c", "git-graph-checkout"),
    ("b", "git-graph-branch"),
    ("t", "git-graph-tag"),
    ("C", "git-graph-cherry-pick"),
    ("V", "git-graph-revert"),
    ("n", "next-line"),
    ("p", "previous-line"),
    ("q", "quit-window"),
];

/// The keys of a graph buffer and the built-in menus they open: the log menu, as in a status.
pub const MENUS: &[(&str, &str)] = &[("l", "log")];

macro_rules! command {
    ($name:literal, $doc:literal, $args:expr, $run:ident) => {
        Command {
            name: $name,
            doc: $doc,
            args: $args,
            reach: Reach::Person,
            run: $run,
            state: None,
        }
    };
}

/// The commands of the graph.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-graph",
        "Show the commits of the current project's branch as a graph, `*graph: NAME*` (`C-x v l`, the log menu's `l`).\n\nEach row is the lanes, the short id, the refs, the subject, the author and the date. It runs git, so only in a trusted project. `l` there opens the log menu, `RET` reviews a commit, `w` copies its id, `a` shows every ref and `g` reads it again; `m` marks a commit, `c` checks out, `b` makes a branch, `t` a tag, `C` cherry-picks and `V` reverts.",
        ArgSpec::None,
        graph_current
    ),
    command!(
        "git-graph-all",
        "Show the graph of every branch, remote branch and tag (the log menu's `a`).",
        ArgSpec::None,
        graph_all
    ),
    command!(
        "git-graph-other",
        "Show the graph of a ref read in a prompt over the branches, `HEAD` first, or of any revision typed (the log menu's `o`).",
        ArgSpec::None,
        graph_other
    ),
    command!(
        "git-graph-of",
        "Show the graph of the ref read in a prompt: what `git-graph-other` asks.",
        ArgSpec::Listed {
            list: GRAPH_REFS_LIST,
            prompt: "Graph of: ",
            history: "git-graph",
        },
        graph_of
    ),
    command!(
        "git-graph-review",
        "In a graph, review the commit at the primary cursor against its first parent, read-only (`RET`).",
        ArgSpec::None,
        review
    ),
    command!(
        "git-graph-copy-id",
        "In a graph, copy the full id of the commit at the primary cursor (`w`).",
        ArgSpec::None,
        copy_id
    ),
    command!(
        "git-graph-toggle-all",
        "In a graph, show every branch, remote branch and tag, or the branch checked out again (`a`).",
        ArgSpec::None,
        toggle_all
    ),
    command!(
        "git-graph-refresh",
        "In a graph, read its commits again (`g`).",
        ArgSpec::None,
        refresh
    ),
    command!(
        "git-graph-mark",
        "In a graph, mark or unmark the commit at the primary cursor (`m`).\n\nWith two commits marked, `RET` reviews the range between them; `C` cherry-picks the marked commits, oldest first.",
        ArgSpec::None,
        mark
    ),
    command!(
        "git-graph-checkout",
        "In a graph, check out the branch whose label is at the primary cursor's commit, asking which if there are several; with none, the commit itself, detached, after a question (`c`).\n\nA remote's branch makes a local one that tracks it. It is refused while a merge, rebase, cherry-pick, revert or `git am` is stopped.",
        ArgSpec::None,
        checkout
    ),
    command!(
        "git-graph-branch",
        "In a graph, make a branch at the commit at the primary cursor, its name read in a prompt (`b`). It checks nothing out.",
        ArgSpec::None,
        branch
    ),
    command!(
        "git-graph-tag",
        "In a graph, make a tag at the commit at the primary cursor, its name read in a prompt (`t`); with a prefix argument, `C-u t`, an annotated tag, its message read after.",
        ArgSpec::None,
        tag
    ),
    command!(
        "git-graph-cherry-pick",
        "In a graph, cherry-pick the marked commits, oldest first, or the one at the primary cursor, onto HEAD, after a question that names them (`C`).\n\nOne that stops on a conflict is the status's to go on with: `C-c C-c` there continues, `C-c C-k` aborts.",
        ArgSpec::None,
        cherry_pick
    ),
    command!(
        "git-graph-revert",
        "In a graph, revert the commit at the primary cursor, with git's message, after a question (`V`). A merge's revert is refused: `git revert -m` in a shell does it.",
        ArgSpec::None,
        revert
    ),
    command!(
        "git-graph-confirmed",
        "Run what the graph's question named, after `y` to it.",
        ArgSpec::None,
        confirmed
    ),
    command!(
        "git-graph-label",
        "Check out the branch read in a prompt over those on the commit: what `git-graph-checkout` asks when there are several.",
        ArgSpec::Listed {
            list: GRAPH_LABELS_LIST,
            prompt: "Check out: ",
            history: "git-branch",
        },
        answered
    ),
    command!(
        "git-graph-branch-name",
        "Name the branch made at the graph's commit as read in a prompt: what `git-graph-branch` asks.",
        ArgSpec::Text {
            prompt: "Name of the new branch: ",
            history: "git-branch-name",
        },
        answered
    ),
    command!(
        "git-graph-tag-name",
        "Name the tag made at the graph's commit as read in a prompt: what `git-graph-tag` asks.",
        ArgSpec::Text {
            prompt: "Name of the tag: ",
            history: "git-tag-name",
        },
        answered
    ),
    command!(
        "git-graph-tag-message",
        "Give the annotated tag the message read in a prompt: what `C-u t` asks after the name.",
        ArgSpec::Text {
            prompt: "Tag's message: ",
            history: "git-tag-message",
        },
        answered
    ),
];

/// The project the selected window's buffer is of: a status's, a graph's, or a file's.
fn project(ctx: &Ctx<'_>) -> Result<ProjectId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    super::status_project(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is of no project"))
}

/// The graph the selected window shows, or why the command means nothing here.
fn graph(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitGraph || ctx.editor.git_graph_of(window.buffer()).is_none() {
        return Err(CommandError::new("Not in a graph"));
    }
    Ok(window.buffer())
}

fn show(ctx: &mut Ctx<'_>, scope: GraphScope) -> Result<(), CommandError> {
    let project = project(ctx)?;
    ctx.editor.request_git(GitRequest::Graph { project, scope });
    ctx.request_redraw();
    Ok(())
}

fn graph_current(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show(ctx, GraphScope::Current)
}

fn graph_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show(ctx, GraphScope::All)
}

fn graph_other(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    show(ctx, GraphScope::Pick)
}

fn graph_of(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::refuse_secret_answer(
        ctx,
        "A ref holding what may be secret is not given to git",
    )?;
    let answer = ctx.answer().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(CommandError::new("No ref was named"));
    }
    show(ctx, GraphScope::Named(answer))
}

fn act(ctx: &mut Ctx<'_>, act: GraphAct) -> Result<(), CommandError> {
    let buffer = graph(ctx)?;
    let line = super::point_line(ctx);
    ctx.editor
        .request_git(GitRequest::GraphAct { buffer, act, line });
    ctx.request_redraw();
    Ok(())
}

fn review(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Review)
}

fn copy_id(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::CopyId)
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Refresh)
}

fn mark(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Mark)
}

fn checkout(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Checkout)
}

fn branch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Branch)
}

fn tag(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let annotated = args.prefix().is_some();
    act(ctx, GraphAct::Tag { annotated })
}

fn cherry_pick(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::CherryPick)
}

fn revert(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, GraphAct::Revert)
}

fn confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("Nothing was asked about in a graph"));
    }
    act(ctx, GraphAct::Confirmed)
}

/// A graph's prompt's answer, which goes to the host as read.
fn answered(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::refuse_secret_answer(
        ctx,
        "A name or message holding what may be secret is not given to git",
    )?;
    let text = ctx.answer().unwrap_or_default().to_owned();
    let buffer = graph(ctx)?;
    ctx.editor
        .request_git(GitRequest::GraphAnswer { buffer, text });
    ctx.request_redraw();
    Ok(())
}

fn toggle_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = graph(ctx)?;
    ctx.editor.request_git(GitRequest::GraphToggle { buffer });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests;
