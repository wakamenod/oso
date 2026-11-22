//! Source control's commands for committing (ADR 0127, step 1b): the commit menu's heads, which
//! read its switches, the keys of a commit's message, and the log of git's runs; and asking the
//! worktree's agent session for a draft or a fix (step 6a); and taking away a stale index lock
//! the person's `y` lets go (ADR 0168).
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which runs git on
//! the repository's own thread. Every command here is `Person`: each makes the host run git,
//! stop it, or change what the person writes (ADR 0085).

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{CommitKind, CommitSwitches, GitRequest};

/// The commit menu's switches, by name, as its heads flip them.
pub const NO_VERIFY: &str = "no-verify";
pub const SIGNOFF: &str = "signoff";
pub const ALLOW_EMPTY: &str = "allow-empty";

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

/// The commands of committing and of the log.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-commit",
        "In a git status, write a commit's message for what is staged, in `*commit: NAME*` (the commit menu's `c`).\n\n`C-c C-c` there commits, running the repository's hooks, and `C-c C-k` cancels. With the menu's `-n` the commit runs no pre-commit or commit-msg hook, with `-s` it adds a sign-off, and with `-e` it may commit nothing.",
        commit_new
    ),
    command!(
        "git-commit-amend",
        "In a git status, make the last commit again with what is staged, its message edited from its own (the commit menu's `a`).",
        commit_amend
    ),
    command!(
        "git-commit-extend",
        "In a git status, make the last commit again with what is staged, keeping its message (the commit menu's `e`).\n\nIt runs at once, with the repository's hooks.",
        commit_extend
    ),
    command!(
        "git-commit-reword",
        "In a git status, edit the last commit's message alone, leaving what is staged out (the commit menu's `w`).",
        commit_reword
    ),
    command!(
        "git-commit-fixup",
        "In a git status, make a `fixup!` commit of what is staged for the commit at the primary cursor, in the unpushed or recent commits (the commit menu's `f`).\n\nIt runs at once, with the repository's hooks. oso does not rebase: `git rebase -i --autosquash` in a shell, or a squash merge, folds it in.",
        commit_fixup
    ),
    command!(
        "git-commit-squash",
        "In a git status, write the message of a `squash!` commit of what is staged for the commit at the primary cursor, in the unpushed or recent commits (the commit menu's `s`).\n\nWhat you write follows git's `squash!` line; `C-c C-c` commits it, and `git rebase -i --autosquash` in a shell folds it in.",
        commit_squash
    ),
    command!(
        "git-commit-finish",
        "In a commit's message, commit with it (`C-c C-c`).\n\nLines starting with `#` are left out. git runs the repository's hooks, with no time limit; the echo area says how long it has run, and `$` in the status shows what it wrote. A commit that fails keeps the message here.",
        commit_finish
    ),
    command!(
        "git-commit-cancel",
        "In a commit's message, drop it without committing (`C-c C-k`); `M-p` in the next one brings its text back.",
        commit_cancel
    ),
    command!(
        "git-commit-previous-message",
        "In a commit's message, put the message kept before the one shown in its place (`M-p`): the last 20 committed or cancelled.",
        previous_message
    ),
    command!(
        "git-commit-next-message",
        "In a commit's message, put the message kept after the one shown in its place (`M-n`).",
        next_message
    ),
    command!(
        "git-draft-commit-message",
        "In a commit's message, ask the agent session of the repository's worktree for a draft of it (`C-c C-d`).\n\nThe request is pasted into the session, not submitted: it names the worktree, what the commit holds and the last five subjects, and you press Enter there. The session gives the draft back with its tool `draft_set`; it replaces the text above the `#` lines, and what you had written there is kept below as `#` lines. Nothing is committed: `C-c C-c` does that. Only the newest request may be answered, within 15 minutes, once.",
        draft_message
    ),
    command!(
        "git-fix-with-agent",
        "Ask the agent session of the repository's worktree to fix what made the last commit fail (`C-c C-f` in its message, `F` in the log).\n\nThe request is pasted into the session, not submitted: it names the worktree and the hooks, and holds the failed run's output, its last 200 lines, which you read before you press Enter there. The session is told not to commit, skip hooks or amend; you commit again with `C-c C-c` once it is done. It works while the failed commit's message is open.",
        fix_with_agent
    ),
    command!(
        "git-show-log",
        "In a git status, show what git's long runs in its repository wrote, in `*git log: NAME*` (`$`), as magit's process buffer.\n\nIt keeps the last 4 MiB. `k` or `C-g` there stops a run under way.",
        show_log
    ),
    command!(
        "git-stop",
        "In a git status or its log, stop what git runs in the repository (`k` in the log; `C-g` does it too).\n\ngit and what it started get `SIGTERM`, then `SIGKILL` after 5 s.",
        stop
    ),
    command!(
        "git-remove-stale-lock",
        "Take away the index's lock, `index.lock`, that a stopped run left in the project's repository, or that a run failed on, after a question.\n\nOnly a lock oso noted, and only if it is still the same file: a git started elsewhere may own a newer one. While it is there, git writes no index.",
        remove_stale_lock
    ),
];

/// The keys of a commit's message and their commands, above the global ones there.
pub const MESSAGE_KEYS: &[(&str, &str)] = &[
    ("C-c C-c", "git-commit-finish"),
    ("C-c C-k", "git-commit-cancel"),
    ("M-p", "git-commit-previous-message"),
    ("M-n", "git-commit-next-message"),
    ("C-c C-d", "git-draft-commit-message"),
    ("C-c C-f", "git-fix-with-agent"),
];

/// The keys of the log of git's runs and their commands.
pub const LOG_KEYS: &[(&str, &str)] = &[
    ("k", "git-stop"),
    ("F", "git-fix-with-agent"),
    ("q", "quit-window"),
];

/// The status buffer the selected window shows, or why the command means nothing here.
fn status(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    super::status(ctx)
}

/// The commit's message the selected window shows.
fn message(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitCommit {
        return Err(CommandError::new("Not in a commit's message"));
    }
    Ok(window.buffer())
}

/// The switches on in the menu whose head ran the command.
fn switches(ctx: &Ctx<'_>) -> CommitSwitches {
    CommitSwitches {
        no_verify: ctx.switch(NO_VERIFY),
        signoff: ctx.switch(SIGNOFF),
        allow_empty: ctx.switch(ALLOW_EMPTY),
    }
}

fn commit_of(ctx: &mut Ctx<'_>, kind: CommitKind) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let buffer = status(ctx)?;
    let switches = switches(ctx);
    let line = super::point_line(ctx);
    ctx.editor.request_git(GitRequest::Commit {
        buffer,
        kind,
        switches,
        line,
    });
    ctx.request_redraw();
    Ok(())
}

fn commit_new(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::New)
}

fn commit_amend(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::Amend)
}

fn commit_extend(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::Extend)
}

fn commit_reword(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::Reword)
}

fn commit_fixup(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::Fixup)
}

fn commit_squash(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    commit_of(ctx, CommitKind::Squash)
}

fn commit_finish(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor.request_git(GitRequest::CommitFinish { buffer });
    ctx.request_redraw();
    Ok(())
}

fn commit_cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor.request_git(GitRequest::CommitCancel { buffer });
    ctx.request_redraw();
    Ok(())
}

fn previous_message(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor.request_git(GitRequest::CommitRing {
        buffer,
        older: true,
    });
    ctx.request_redraw();
    Ok(())
}

fn next_message(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor.request_git(GitRequest::CommitRing {
        buffer,
        older: false,
    });
    ctx.request_redraw();
    Ok(())
}

fn draft_message(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor.request_git(GitRequest::DraftMessage { buffer });
    Ok(())
}

fn fix_with_agent(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if !matches!(buffer.mode(), Mode::GitCommit | Mode::GitLog) {
        return Err(CommandError::new(
            "Not in a commit's message or the log of git's runs",
        ));
    }
    let buffer = window.buffer();
    ctx.editor.request_git(GitRequest::FixWithAgent { buffer });
    Ok(())
}

fn show_log(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    let buffer = status(ctx)?;
    ctx.editor.request_git(GitRequest::ShowLog { buffer });
    ctx.request_redraw();
    Ok(())
}

fn stop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let (window, buffer) = ctx.editor.current();
    if !matches!(buffer.mode(), Mode::GitStatus | Mode::GitLog) {
        return Err(CommandError::new("Not in a git status or its log"));
    }
    let buffer = window.buffer();
    ctx.editor.request_git(GitRequest::Stop { buffer });
    ctx.request_redraw();
    Ok(())
}

fn remove_stale_lock(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    // The tiled window's, as `M-x` runs it with the minibuffer selected.
    let buffer = ctx.editor.tiled().0.buffer();
    let in_log = (ctx.editor.buffer(buffer)).is_some_and(|shown| shown.mode() == Mode::GitLog);
    if !in_log && super::status_project(ctx.editor).is_none() {
        return Err(CommandError::new("Not in a project"));
    }
    let confirmed = ctx.confirmed();
    ctx.editor
        .request_git(GitRequest::RemoveLock { buffer, confirmed });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
pub(super) mod tests;
