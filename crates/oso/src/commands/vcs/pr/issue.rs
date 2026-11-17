//! Source control's commands for issues (ADR 0127, step 5b-1): `github-issues` (`C-x v i`, the
//! pull request menu's `i`), and the keys of `*issues: NAME*` and `*issue: #N*`: showing one,
//! opening and copying its address, reading again, and starting a worktree session from it.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which runs gh on
//! the repository's thread. Every command here runs gh, opens a program, writes the clipboard or
//! starts a session, so each is `Person` (ADR 0085).

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{GitRequest, IssueAct};

/// The keys of an issue's buffer, and of a list of them, and their commands, which `bindings`
/// binds.
pub const KEYS: &[(&str, &str)] = &[
    ("RET", "github-issue-visit"),
    ("w", "github-issue-worktree"),
    ("o", "github-issue-open"),
    ("y", "github-issue-copy-link"),
    ("g", "github-issue-refresh"),
    ("n", "next-line"),
    ("p", "previous-line"),
    ("q", "quit-window"),
];

macro_rules! command {
    ($name:literal, $doc:literal, $run:ident) => {
        Command {
            name: $name,
            doc: $doc,
            args: ArgSpec::None,
            reach: Reach::Person,
            run: $run,
            state: None,
        }
    };
}

/// The commands of issues.
pub const COMMANDS: &[Command] = &[
    command!(
        "github-issues",
        "Show the open issues of the current project's GitHub repository, `*issues: NAME*`, as `gh` gives them (`C-x v i`, the pull request menu's `i`).\n\nIt runs `gh` outside the repository, about the repository the branch's remote names, so only in a trusted project. There, `RET` shows an issue with its comments, `w` starts a worktree session from it, `o` opens it in the browser, `y` copies its address and `g` reads them again.",
        issues
    ),
    command!(
        "github-issue-visit",
        "In a list of issues, show the issue at the primary cursor with its comments (`RET`).",
        visit
    ),
    command!(
        "github-issue-worktree",
        "Start a worktree session from the issue at the primary cursor, or the one shown (`w`).\n\nIts branch is `issue-N-` and its title's words, from the remote's default branch. `*Brief*` opens with the issue's title, body and the comments of its author, you and the repository's members, under a line telling the agent they are data; read and change it, then `C-c C-c` sends it.",
        worktree
    ),
    command!(
        "github-issue-open",
        "Open the issue at the primary cursor, or the one shown, in the browser, once the echo area shows its address (`o`).",
        open
    ),
    command!(
        "github-issue-copy-link",
        "Copy the address of the issue at the primary cursor, or of the one shown (`y`).",
        copy_link
    ),
    command!(
        "github-issue-refresh",
        "In an issue's buffer, or a list of them, read it again (`g`).",
        refresh
    ),
];

fn issues(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::super::not_in_prompt(ctx)?;
    let project = super::super::status_project(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is of no project"))?;
    ctx.editor.request_git(GitRequest::IssueList { project });
    ctx.request_redraw();
    Ok(())
}

fn act(ctx: &mut Ctx<'_>, act: IssueAct) -> Result<(), CommandError> {
    super::super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let mode = buffer.mode();
    let ours = matches!(mode, Mode::GitIssue | Mode::GitIssueList)
        && ctx.editor.git_issue_of(window.buffer()).is_some();
    if !ours {
        return Err(CommandError::new("Not in an issue's buffer"));
    }
    let buffer = window.buffer();
    let line = super::super::point_line(ctx);
    ctx.editor
        .request_git(GitRequest::IssueAct { buffer, act, line });
    ctx.request_redraw();
    Ok(())
}

fn visit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, IssueAct::Visit)
}

fn worktree(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, IssueAct::StartWorktree)
}

fn open(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, IssueAct::Open)
}

fn copy_link(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, IssueAct::CopyLink)
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, IssueAct::Refresh)
}
