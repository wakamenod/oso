//! Source control's commands for branches (ADR 0127, step 3b): the branch menu's heads, the
//! prompts they open in turn, and the question before a forced delete.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which lists the
//! branches and runs git on the repository's own thread. Every command here is `Person`: each
//! makes the host run git with the repository's hooks, or answers a prompt or a question that
//! does (ADR 0085).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{BranchAction, GitRequest};

use super::request;

/// The list of branches the host gives a head that checks out, renames or deletes one.
pub const BRANCHES_LIST: &str = "git-branches";
/// The list of where a branch may be made: `HEAD`, the local and the remote branches, and any
/// revision typed.
pub const STARTS_LIST: &str = "git-branch-starts";

/// Whether the prompt over `list` takes what is typed, as a revision to make a branch at.
pub fn listed_takes_input(list: &str) -> bool {
    list == STARTS_LIST
}

/// Whether the prompt over `list` keeps the host's order: `HEAD` or the branch checked out
/// first, the local branches before the remote ones.
pub fn listed_in_given_order(list: &str) -> bool {
    list == BRANCHES_LIST || list == STARTS_LIST
}

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

/// The commands of branches.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-branch-checkout",
        "In a git status, check out a branch read in a prompt (the branch menu's `b`).\n\nIt completes over the local branches, then the remotes' ones; a remote's makes a local branch that tracks it. The repository's post-checkout hook runs, and its filters. It is refused while a merge, rebase, cherry-pick, revert or `git am` is stopped.",
        ArgSpec::None,
        checkout
    ),
    command!(
        "git-branch-create-checkout",
        "In a git status, make a branch and check it out (the branch menu's `c`): where to make it, `HEAD` first, then its name, read in prompts.\n\nIt is refused while a merge, rebase, cherry-pick, revert or `git am` is stopped.",
        ArgSpec::None,
        create_checkout
    ),
    command!(
        "git-branch-create",
        "In a git status, make a branch without checking it out (the branch menu's `n`): where to make it, `HEAD` first, then its name, read in prompts.",
        ArgSpec::None,
        create
    ),
    command!(
        "git-branch-rename",
        "In a git status, rename a branch read in a prompt, the one checked out first (the branch menu's `m`).",
        ArgSpec::None,
        rename
    ),
    command!(
        "git-branch-delete",
        "In a git status, delete a branch read in a prompt (the branch menu's `k`).\n\nOne merged into its upstream, or into `HEAD` with none, goes with `git branch -d`; one that is not asks first, naming the commits only it holds, then goes with `-D`. A branch checked out in a worktree is refused: `worktree-remove` removes the worktree.",
        ArgSpec::None,
        delete
    ),
    command!(
        "git-branch-pick",
        "Act on the branch read in a prompt: what the branch menu's heads ask.",
        ArgSpec::Listed {
            list: BRANCHES_LIST,
            prompt: "Branch: ",
            history: "git-branch",
        },
        picked
    ),
    command!(
        "git-branch-start",
        "Make the new branch at the revision read in a prompt, `HEAD` first: what `git-branch-create` asks.",
        ArgSpec::Listed {
            list: STARTS_LIST,
            prompt: "Make the branch at: ",
            history: "git-branch-start",
        },
        picked
    ),
    command!(
        "git-branch-name",
        "Name the new branch as read in a prompt: what `git-branch-create` asks after where.",
        ArgSpec::Text {
            prompt: "Name of the new branch: ",
            history: "git-branch-name",
        },
        named
    ),
    command!(
        "git-branch-new-name",
        "Rename the branch to the name read in a prompt: what `git-branch-rename` asks.",
        ArgSpec::Text {
            prompt: "Rename it to: ",
            history: "git-branch-name",
        },
        named
    ),
    command!(
        "git-branch-delete-confirmed",
        "Delete the branch the question before it named, after `y` to it.",
        ArgSpec::None,
        delete_confirmed
    ),
];

fn ask(ctx: &mut Ctx<'_>, action: BranchAction) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Branch { buffer, action })
}

fn checkout(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, BranchAction::Checkout)
}

fn create_checkout(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, BranchAction::Create { checkout: true })
}

fn create(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, BranchAction::Create { checkout: false })
}

fn rename(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, BranchAction::Rename)
}

fn delete(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, BranchAction::Delete)
}

/// The answer read, trimmed, or why there is none.
fn answer(ctx: &Ctx<'_>) -> Result<String, CommandError> {
    let answer = ctx.answer().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(CommandError::new("No branch was named"));
    }
    Ok(answer)
}

fn picked(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let answer = answer(ctx)?;
    ask(ctx, BranchAction::Picked(answer))
}

fn named(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let answer = answer(ctx)?;
    ask(ctx, BranchAction::Named(answer))
}

fn delete_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("No delete was asked about: b k asks"));
    }
    ask(ctx, BranchAction::Confirmed)
}

#[cfg(test)]
mod tests;
