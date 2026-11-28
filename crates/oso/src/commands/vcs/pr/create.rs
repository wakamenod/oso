//! Source control's commands for making and merging pull requests (ADR 0127, step 5a-2): the
//! pull request menu's `c` with its switches, the prompt `- b` opens for a base, the keys of a
//! pull request's message, `C-c C-c`, `C-c C-k`, `M-p` and `M-n`, the merge menu's heads, and
//! the `y` to their questions.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command asks the host, which reads and
//! runs `gh` on the repository's threads. Every command here runs gh or git, so each is
//! `Person` (ADR 0085).

use oso_core::buffer::{BufferId, Mode};
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{GitRequest, MergeMethod};

/// The pull request menu's switch that makes a draft.
pub const DRAFT: &str = "pr-draft";
/// Its switch that reads the base in a prompt first.
pub const BASE: &str = "pr-base";
/// The merge menu's switch that deletes the branch on the remote.
pub const DELETE_BRANCH: &str = "pr-delete-branch";

/// Why a message that a secret was yanked into is not sent, by the command and the host.
pub(crate) const SECRET_MESSAGE: &str =
    "The message holds secret text, which is not sent: C-c C-k drops it";

/// The keys of a pull request's message and their commands, which `bindings` binds.
pub const MESSAGE_KEYS: &[(&str, &str)] = &[
    ("C-c C-c", "git-pr-create-finish"),
    ("C-c C-k", "git-pr-create-cancel"),
    ("M-p", "git-pr-previous-message"),
    ("M-n", "git-pr-next-message"),
];

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

/// The commands of making and merging pull requests.
pub const COMMANDS: &[Command] = &[
    command!(
        "git-pr-create",
        "Write a pull request for the current project's branch, `*pull request: BRANCH*` (the pull request menu's `c`).\n\nIt starts from the branch's single commit's message, or the subjects of what its base lacks, above the repository's template; the first line is the title. `- d` makes a draft, `- b` reads the base in a prompt, else the remote's default branch. `C-c C-c` makes it through `gh`, asking first to push a branch with no upstream or ahead of it; `C-c C-k` cancels.",
        ArgSpec::None,
        create
    ),
    command!(
        "git-pr-create-base",
        "Name the base the pull request goes into, as read in a prompt: what `git-pr-create` with `- b` asks.",
        ArgSpec::Text {
            prompt: "Into the base: ",
            history: "git-pr-base",
        },
        create_base
    ),
    command!(
        "git-pr-create-finish",
        "In a pull request's message, make it through `gh` (`C-c C-c`).\n\nA message that a secret was yanked into is not sent.",
        ArgSpec::None,
        finish
    ),
    command!(
        "git-pr-create-cancel",
        "In a pull request's message, drop it, keeping its text for `M-p` (`C-c C-k`), unless a secret was yanked into it.",
        ArgSpec::None,
        cancel
    ),
    command!(
        "git-pr-previous-message",
        "In a pull request's message, show the message kept before the one shown (`M-p`).",
        ArgSpec::None,
        previous
    ),
    command!(
        "git-pr-next-message",
        "In a pull request's message, show the message kept after the one shown (`M-n`).",
        ArgSpec::None,
        next
    ),
    command!(
        "git-pr-merge-squash",
        "Merge the pull request of the current project's branch, squashed, after a question naming it, its base and its head (the merge menu's `s`).\n\n`- d` deletes its branch on the remote. `gh` refuses it if the pull request's head moved since the question.",
        ArgSpec::None,
        merge_squash
    ),
    command!(
        "git-pr-merge-commit",
        "Merge the pull request of the current project's branch with a merge commit, after a question (the merge menu's `m`).",
        ArgSpec::None,
        merge_commit
    ),
    command!(
        "git-pr-merge-rebase",
        "Merge the pull request of the current project's branch, rebased, after a question (the merge menu's `r`).",
        ArgSpec::None,
        merge_rebase
    ),
    command!(
        "git-pr-confirmed",
        "Go on with what the pull request's question asked about, after the person's `y`: push first, or merge.",
        ArgSpec::None,
        confirmed
    ),
];

fn project(ctx: &Ctx<'_>) -> Result<oso_core::project::ProjectId, CommandError> {
    super::super::super::not_in_prompt(ctx)?;
    super::super::status_project(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is of no project"))
}

/// The pull request's message the selected window shows.
fn message(ctx: &Ctx<'_>) -> Result<BufferId, CommandError> {
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::GitPrMessage {
        return Err(CommandError::new("Not in a pull request's message"));
    }
    Ok(window.buffer())
}

fn ask(ctx: &mut Ctx<'_>, request: GitRequest) -> Result<(), CommandError> {
    ctx.editor.request_git(request);
    ctx.request_redraw();
    Ok(())
}

fn create(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let project = project(ctx)?;
    let draft = ctx.switch(DRAFT);
    let ask_base = ctx.switch(BASE);
    let request = GitRequest::PrCreate {
        project,
        draft,
        ask_base,
    };
    ask(ctx, request)
}

fn create_base(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::super::refuse_secret_answer(
        ctx,
        "A base holding what may be secret is not given to gh",
    )?;
    let base = ctx.answer().unwrap_or_default().trim().to_owned();
    if base.is_empty() {
        return Err(CommandError::new("No base was named"));
    }
    let project = super::super::status_project(ctx.editor)
        .ok_or_else(|| CommandError::new("This buffer is of no project"))?;
    ask(ctx, GitRequest::PrCreateBase { project, base })
}

fn finish(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    // A yank of a secret entry turns it secret: a text that a secret was put in stays marked.
    if ctx.editor.current().1.is_secret() {
        return Err(CommandError::new(SECRET_MESSAGE));
    }
    ask(ctx, GitRequest::PrCreateFinish { buffer })
}

fn cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ask(ctx, GitRequest::PrCreateCancel { buffer })
}

fn previous(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ask(
        ctx,
        GitRequest::PrRing {
            buffer,
            older: true,
        },
    )
}

fn next(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    let request = GitRequest::PrRing {
        buffer,
        older: false,
    };
    ask(ctx, request)
}

fn merge(ctx: &mut Ctx<'_>, method: MergeMethod) -> Result<(), CommandError> {
    let project = project(ctx)?;
    let delete_branch = ctx.switch(DELETE_BRANCH);
    let request = GitRequest::PrMerge {
        project,
        method,
        delete_branch,
    };
    ask(ctx, request)
}

fn merge_squash(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    merge(ctx, MergeMethod::Squash)
}

fn merge_commit(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    merge(ctx, MergeMethod::Merge)
}

fn merge_rebase(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    merge(ctx, MergeMethod::Rebase)
}

fn confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new(
            "No pull request was asked about: N c or N m asks",
        ));
    }
    ask(ctx, GitRequest::PrConfirmed)
}
