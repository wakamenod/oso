//! Source control's commands for stashes (ADR 0127, step 3b): the stash menu's heads, which read
//! its switch, the prompts they open for a message or a stash, and the question before a drop.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which runs git on
//! the repository's own thread. Every command here is `Person`: each makes the host run git
//! with the repository's hooks, or answers a prompt or a question that does (ADR 0085).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{GitRequest, StashAction};

use super::request;

/// The stash menu's switch, by name, as its head flips it.
pub const STASH_UNTRACKED: &str = "stash-include-untracked";

/// The list of a repository's stashes the host gives a head with point on none to pick from.
pub const STASHES_LIST: &str = "git-stashes";

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

/// The commands of stashes.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-stash",
        "In a git status, stash the changes, staged and not (the stash menu's `z`).\n\nIt asks for a message first; an empty one keeps git's own, `WIP on BRANCH`. With the menu's `-u` untracked files go too. Stashes are the repository's, shared by every worktree.",
        ArgSpec::None,
        push
    ),
    command!(
        "git-stash-message",
        "Stash the changes with the message read in a prompt: what `git-stash` asks.",
        ArgSpec::Text {
            prompt: "Stash message: ",
            history: "git-stash-message",
        },
        message
    ),
    command!(
        "git-stash-apply",
        "In a git status, apply the stash at the primary cursor, keeping it (the stash menu's `a`).\n\nWith point on no stash it asks which. A stash that does not apply cleanly leaves its files to resolve. It is refused while a merge, rebase, cherry-pick, revert or `git am` is stopped.",
        ArgSpec::None,
        apply
    ),
    command!(
        "git-stash-pop",
        "In a git status, apply the stash at the primary cursor and drop it (the stash menu's `p`).\n\nWith point on no stash it asks which. A stash that does not apply cleanly is kept. It is refused while a merge, rebase, cherry-pick, revert or `git am` is stopped.",
        ArgSpec::None,
        pop
    ),
    command!(
        "git-stash-drop",
        "In a git status, drop the stash at the primary cursor, after a question that names it (the stash menu's `k`).\n\nWith point on no stash it asks which.",
        ArgSpec::None,
        drop_stash
    ),
    command!(
        "git-stash-pick",
        "Act on the stash read in a prompt, as `stash@{N}`: what the stash menu's heads ask with point on none.",
        ArgSpec::Listed {
            list: STASHES_LIST,
            prompt: "Stash: ",
            history: "git-stash",
        },
        pick
    ),
    command!(
        "git-stash-drop-confirmed",
        "Drop the stash the question before it named, after `y` to it.",
        ArgSpec::None,
        drop_confirmed
    ),
];

fn ask(ctx: &mut Ctx<'_>, action: StashAction) -> Result<(), CommandError> {
    let line = super::point_line(ctx);
    request(ctx, |buffer| GitRequest::Stash {
        buffer,
        action,
        line,
    })
}

fn push(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let untracked = ctx.switch(STASH_UNTRACKED);
    ask(ctx, StashAction::Push { untracked })
}

fn message(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::refuse_secret_answer(
        ctx,
        "A stash message holding what may be secret is not kept",
    )?;
    let text = ctx.answer().unwrap_or_default().to_owned();
    ask(ctx, StashAction::Message(text))
}

fn apply(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, StashAction::Apply)
}

fn pop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, StashAction::Pop)
}

fn drop_stash(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, StashAction::Drop)
}

fn pick(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let answer = ctx.answer().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(CommandError::new("No stash was named"));
    }
    ask(ctx, StashAction::Picked(answer))
}

fn drop_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("No drop was asked about: z k asks"));
    }
    ask(ctx, StashAction::Confirmed)
}

#[cfg(test)]
mod tests;
