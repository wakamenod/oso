//! Source control's commands for the graph (ADR 0127, step 4): showing a repository's commits
//! as a graph from anywhere in its project (`C-x v l`) or from the log menu (`l`), and the keys
//! of a graph buffer: reviewing the commit on a line, copying its id, showing every ref, and
//! reading it again.
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
        "Show the commits of the current project's branch as a graph, `*graph: NAME*` (`C-x v l`, the log menu's `l`).\n\nEach row is the lanes, the short id, the refs, the subject, the author and the date. It runs git, so only in a trusted project. `l` there opens the log menu, `RET` reviews a commit, `w` copies its id, `a` shows every ref and `g` reads it again.",
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

fn toggle_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = graph(ctx)?;
    ctx.editor.request_git(GitRequest::GraphToggle { buffer });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests;
