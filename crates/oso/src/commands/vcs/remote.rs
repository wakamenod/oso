//! Source control's commands for remotes (ADR 0127, step 2): the push menu's heads, which read
//! its switches, the prompts a push opens when it has no target, the question before a forced
//! one, pulling, with the pull menu's `-r` and `-m` from step 3a, and fetching.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which runs git on
//! the repository's own thread. Every command here is `Person`: each makes the host run git,
//! which talks to a remote (ADR 0085).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};

use super::request;
use oso_core::editor::{FetchFrom, GitRequest, PullHow, PushSwitches, PushTarget};

/// The push menu's switches, by name, as its heads flip them.
pub const FORCE_WITH_LEASE: &str = "push-force-with-lease";
pub const PUSH_NO_VERIFY: &str = "push-no-verify";
/// The pull menu's switches (step 3a).
pub const PULL_REBASE: &str = "pull-rebase";
pub const PULL_MERGE: &str = "pull-merge";

/// The list of a repository's remotes the host gives a push with no target to pick from.
pub const REMOTES_LIST: &str = "git-remotes";
/// The list of `REMOTE/BRANCH`es the host gives `P e` to pick from, which takes any typed.
pub const TARGETS_LIST: &str = "git-push-targets";

/// Whether the prompt over `list` takes what is typed, as a new branch on a remote.
pub fn listed_takes_input(list: &str) -> bool {
    list == TARGETS_LIST
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

/// The commands of remotes.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-push",
        "In a git status, push the branch checked out to where git pushes it, `@{push}` (the push menu's `p`).\n\nWith no push target set, it asks for a remote and pushes to the branch of the same name there, writing nothing to the configuration. The repository's pre-push hook runs, but with the menu's `-n`. With `-f` it pushes with `--force-with-lease`, after a question that names the commits it would drop there; never a plain `--force`. It runs with no time limit: `$` shows what git writes, `C-g` stops it.",
        ArgSpec::None,
        push
    ),
    command!(
        "git-push-upstream",
        "In a git status, push the branch checked out to its upstream (the push menu's `u`).\n\nWith no upstream, it asks for a remote, pushes to the branch of the same name there and sets it as the upstream (`--set-upstream`).",
        ArgSpec::None,
        push_upstream
    ),
    command!(
        "git-push-elsewhere",
        "In a git status, push the branch checked out to a remote's branch read in a prompt as REMOTE/BRANCH (the push menu's `e`).\n\nIt completes over the remotes' branches the repository tracks, the branch's own name on each remote first, and takes a new name as typed.",
        ArgSpec::None,
        push_elsewhere
    ),
    command!(
        "git-push-to-remote",
        "Push the branch checked out to the remote read in a prompt, to the branch of the same name there: what `git-push` and `git-push-upstream` ask with no target set.",
        ArgSpec::Listed {
            list: REMOTES_LIST,
            prompt: "Push to remote: ",
            history: "git-remote",
        },
        push_named
    ),
    command!(
        "git-push-to",
        "Push the branch checked out to the REMOTE/BRANCH read in a prompt: what `git-push-elsewhere` asks.",
        ArgSpec::Listed {
            list: TARGETS_LIST,
            prompt: "Push to (REMOTE/BRANCH): ",
            history: "git-push-target",
        },
        push_named
    ),
    command!(
        "git-push-confirmed",
        "Run the forced push the question before it named, after `y` to it.",
        ArgSpec::None,
        push_confirmed
    ),
    command!(
        "git-pull",
        "In a git status, pull the upstream into the branch checked out (the pull menu's `u`): fast-forward only, unless the menu's `-r` rebases or `-m` merges.\n\nIt runs `git pull --ff-only`, with the repository's hooks and no time limit; a branch that has gone apart from its upstream is refused by git, and stays as it was. With `-r` it runs `git pull --rebase`, and with `-m` `git pull --no-rebase --ff --no-edit`, a merge commit with git's message when it cannot fast-forward; either may stop on a conflict, which the status shows, with `C-c C-c` to continue and `C-c C-k` to abort. It is refused while something is stopped.",
        ArgSpec::None,
        pull
    ),
    command!(
        "git-fetch",
        "In a git status, fetch from the upstream's remote, pruning the branches it deleted (the fetch menu's `u`).",
        ArgSpec::None,
        fetch
    ),
    command!(
        "git-fetch-all",
        "In a git status, fetch from every remote, pruning the branches each deleted (the fetch menu's `a`).",
        ArgSpec::None,
        fetch_all
    ),
];

/// The switches on in the menu whose head ran the command.
fn switches(ctx: &Ctx<'_>) -> PushSwitches {
    PushSwitches {
        force_with_lease: ctx.switch(FORCE_WITH_LEASE),
        no_verify: ctx.switch(PUSH_NO_VERIFY),
    }
}

fn push_to(ctx: &mut Ctx<'_>, target: PushTarget) -> Result<(), CommandError> {
    let switches = switches(ctx);
    request(ctx, |buffer| GitRequest::Push {
        buffer,
        target,
        switches,
    })
}

fn push(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    push_to(ctx, PushTarget::PushRemote)
}

fn push_upstream(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    push_to(ctx, PushTarget::Upstream)
}

fn push_elsewhere(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    push_to(ctx, PushTarget::Elsewhere)
}

fn push_named(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::refuse_secret_answer(
        ctx,
        "A remote or branch holding what may be secret is not given to git",
    )?;
    let answer = ctx.answer().unwrap_or_default().trim().to_owned();
    if answer.is_empty() {
        return Err(CommandError::new("No remote was named"));
    }
    push_to(ctx, PushTarget::Named(answer))
}

fn push_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new(
            "No forced push was asked about: P -f p asks",
        ));
    }
    push_to(ctx, PushTarget::Asked)
}

fn pull(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let how = match (ctx.switch(PULL_REBASE), ctx.switch(PULL_MERGE)) {
        (true, true) => {
            return Err(CommandError::new(
                "Both -r and -m are on: a pull rebases or merges, not both",
            ));
        }
        (true, false) => PullHow::Rebase,
        (false, true) => PullHow::Merge,
        (false, false) => PullHow::FfOnly,
    };
    request(ctx, |buffer| GitRequest::Pull { buffer, how })
}

fn fetch(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Fetch {
        buffer,
        from: FetchFrom::Upstream,
    })
}

fn fetch_all(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Fetch {
        buffer,
        from: FetchFrom::All,
    })
}

#[cfg(test)]
mod tests;
