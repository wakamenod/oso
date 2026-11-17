//! Source control's commands for pull requests (ADR 0127, step 5a): the pull request menu's
//! heads, `N v` to show the branch's, `N k` its checks, `N l` the repository's open ones and
//! `N o` to open the branch's in the browser, and the keys of `*pr: #N*` and of the list:
//! reading it again, visiting what a line names, opening and copying an address.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which runs `gh`
//! on the repository's thread for it. Every command here runs `gh`, opens a program or writes
//! the clipboard, so each is `Person` (ADR 0085).

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{GitRequest, PrAct};
use oso_core::project::ProjectId;

/// The keys of a pull request's buffer, and of a list of them, and their commands, which
/// `bindings` binds in its keymap.
pub const KEYS: &[(&str, &str)] = &[
    ("RET", "git-pr-visit"),
    ("o", "git-pr-open"),
    ("w", "git-pr-copy-link"),
    ("g", "git-pr-refresh"),
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

/// The commands of pull requests.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-pr-view",
        "Show the pull request of the current project's branch, `*pr: #N*`, as `gh` gives it (the pull request menu's `v`).\n\nIt shows the state, the checks, the review threads, unresolved first, and the conversation. It runs `gh` outside the repository, about the GitHub repository the branch's remote names, so only in a trusted project. There, `RET` visits a thread's file or shows a check's address, `o` opens the pull request in the browser, `w` copies an address and `g` reads it again.",
        view
    ),
    command!(
        "git-pr-checks",
        "Show the pull request of the current project's branch at its checks (the pull request menu's `k`).",
        checks
    ),
    command!(
        "git-pr-list",
        "Show the open pull requests of the current project's GitHub repository, `*pull requests: NAME*` (the pull request menu's `l`). `RET` there shows one.",
        list
    ),
    command!(
        "git-pr-browse",
        "Open the pull request of the current project's branch in the browser, once the echo area shows its address (the pull request menu's `o`).",
        browse
    ),
    command!(
        "git-pr-visit",
        "In a pull request's buffer, visit the file of the review thread at the primary cursor at its line, or show the address of the check there; in a list, show the pull request there (`RET`).",
        visit
    ),
    command!(
        "git-pr-open",
        "In a pull request's buffer, or at a pull request in a list, open it in the browser, once the echo area shows its address (`o`).",
        open
    ),
    command!(
        "git-pr-copy-link",
        "In a pull request's buffer, copy the address of the check at the primary cursor, or of the pull request; in a list, of the pull request there (`w`).",
        copy_link
    ),
    command!(
        "git-pr-refresh",
        "In a pull request's buffer, or a list of them, read it again (`g`).",
        refresh
    ),
];

/// The project the selected window's buffer is of.
fn project(ctx: &Ctx<'_>) -> Result<ProjectId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    super::status_project(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is of no project"))
}

/// The pull request's buffer, or list, the selected window shows.
fn shown(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    let mode = buffer.mode();
    if !matches!(mode, Mode::GitPr | Mode::GitPrList)
        || ctx.editor.git_pr_of(window.buffer()).is_none()
    {
        return Err(CommandError::new("Not in a pull request's buffer"));
    }
    Ok(window.buffer())
}

fn ask(ctx: &mut Ctx<'_>, request: GitRequest) -> Result<(), CommandError> {
    ctx.editor.request_git(request);
    ctx.request_redraw();
    Ok(())
}

fn view(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    let request = GitRequest::PrView {
        project,
        number: None,
        checks: false,
    };
    ask(ctx, request)
}

fn checks(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    let request = GitRequest::PrView {
        project,
        number: None,
        checks: true,
    };
    ask(ctx, request)
}

fn list(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    ask(ctx, GitRequest::PrList { project })
}

fn browse(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    ask(ctx, GitRequest::PrOpen { project })
}

fn act(ctx: &mut Ctx<'_>, act: PrAct) -> Result<(), CommandError> {
    let buffer = shown(ctx)?;
    let line = super::point_line(ctx);
    ask(ctx, GitRequest::PrAct { buffer, act, line })
}

fn visit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, PrAct::Visit)
}

fn open(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, PrAct::Open)
}

fn copy_link(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, PrAct::CopyLink)
}

fn refresh(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    act(ctx, PrAct::Refresh)
}

pub mod create;
pub mod issue;

#[cfg(test)]
mod tests;
