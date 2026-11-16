//! Source control's commands for what is stopped (ADR 0127, step 3a): continuing a merge,
//! rebase, cherry-pick, revert or `git am` that stopped, and the questions before skipping the
//! commit it stopped at and aborting it: a sequence, as magit calls it, in the code.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which reads what
//! is stopped from the status and runs git on the repository's own thread. Every command here
//! is `Person`: each makes the host run git with the repository's hooks (ADR 0085).

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::GitRequest;

use super::request;

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

/// The commands of what is stopped.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-continue",
        "In a git status, go on with the merge, rebase, cherry-pick, revert or `git am` that stopped (`C-c C-c`).\n\nIt runs `git rebase --continue` and the like with the repository's hooks, keeping the message git prepared; no editor opens. It is refused while unmerged paths are left: resolve each, and `s` stages it. A rebase that stops again shows where.",
        continue_stopped
    ),
    command!(
        "git-skip",
        "In a git status, skip the commit the rebase, cherry-pick, revert or `git am` stopped at (`C-c C-s`), after a question that names it.\n\nIts changes, and what you resolved in it, go; the rest goes on.",
        skip
    ),
    command!(
        "git-skip-confirmed",
        "Skip the commit the question before it named, after `y` to it.",
        skip_confirmed
    ),
    command!(
        "git-abort",
        "In a git status, abort the merge, rebase, cherry-pick, revert or `git am` that stopped (`C-c C-k`), after a question that says what goes.\n\nWhat it made and what you resolved go, and `HEAD` goes back to where it began.",
        abort
    ),
    command!(
        "git-abort-confirmed",
        "Abort what the question before it named, after `y` to it.",
        abort_confirmed
    ),
];

fn continue_stopped(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Continue { buffer })
}

fn skip(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Skip {
        buffer,
        confirmed: false,
    })
}

fn skip_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("No skip was asked about: C-c C-s asks"));
    }
    request(ctx, |buffer| GitRequest::Skip {
        buffer,
        confirmed: true,
    })
}

fn abort(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    request(ctx, |buffer| GitRequest::Abort {
        buffer,
        confirmed: false,
    })
}

fn abort_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("No abort was asked about: C-c C-k asks"));
    }
    request(ctx, |buffer| GitRequest::Abort {
        buffer,
        confirmed: true,
    })
}

#[cfg(test)]
mod tests;
