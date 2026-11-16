//! The issue space commands (ADR 0136, step 1b): making a space for an issue, adding and
//! taking out its repositories, closing and reopening it, renaming it, and its instructions
//! and directory. Each but `space-new` and `space-reopen` acts on the space in front; the host
//! does the work, its disk work on the sessions worker.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host.
//!
//! Every one is `Person`: a space's repositories become an orchestrator's `--add-dir` (step 3),
//! its title and name are the person's words that orchestrator is given, closing a space stops
//! its idle sessions, and showing its directory makes it and puts a listing in front of the
//! person's keys after the command, where the agent channel's guards no longer see it.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{SessionRequest, SpaceRequest};

/// The list `space-add-repo` completes over: the repositories oso knows, by path.
pub const REPOSITORIES_LIST: &str = "space-repositories";

/// The list `space-remove-repo` completes over: the repositories of the space in front.
pub const MEMBERS_LIST: &str = "space-members";

/// The list `space-reopen` completes over: the archived spaces.
pub const ARCHIVED_LIST: &str = "spaces-archived";

/// Whether the prompt of a command over `list` takes any text as its answer, not only a
/// candidate's: a repository's path need not be one oso knows yet.
pub fn listed_takes_input(list: &str) -> bool {
    list == REPOSITORIES_LIST
}

/// The space commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "space-new",
        doc: "Make a space for an issue, read as its title, and enter it.

A space is the workplace of one issue: its repositories, their worktrees and sessions, and its own windows and directory. The space's name is made from the title. Its first repository is the selected buffer's; with none, its path is read. `space-add-repo` adds more.",
        args: ArgSpec::Text {
            prompt: "New space for issue:",
            history: "space-title",
        },
        reach: Reach::Person,
        run: space_new,
        state: None,
    },
    Command {
        name: "space-add-repo",
        doc: "Add a repository to the space in front, read with completion over the repositories oso knows, or as a path.

It must be a repository's main checkout, not a worktree. A repository oso does not trust is added, but its worktrees and agents' starts wait for trust.",
        args: ArgSpec::Listed {
            list: REPOSITORIES_LIST,
            prompt: "Add repository:",
            history: "space-repository",
        },
        reach: Reach::Person,
        run: space_add_repo,
        state: None,
    },
    Command {
        name: "space-remove-repo",
        doc: "Take a repository out of the space in front, read with completion.

It is refused while a session of the space runs in it. Its worktrees stay.",
        args: ArgSpec::Listed {
            list: MEMBERS_LIST,
            prompt: "Remove repository:",
            history: "space-member",
        },
        reach: Reach::Person,
        run: space_remove_repo,
        state: None,
    },
    Command {
        name: "space-use-repo",
        doc: "Choose the repository of the space in front that commands needing a project use from its orchestrator's buffer, read with completion.

The orchestrator runs in the space's own directory, in no repository: with one trusted repository the space needs no choice; with several, `session-worktree`, `worktree-list` and `worktree-remove` there use the one chosen here, which must be trusted. A buffer of that repository must be open, as for its row in `*Spaces*`.",
        args: ArgSpec::Listed {
            list: MEMBERS_LIST,
            prompt: "Use repository:",
            history: "space-member",
        },
        reach: Reach::Person,
        run: space_use_repo,
        state: None,
    },
    Command {
        name: "space-close",
        doc: "Archive the space in front.

It is refused while a session of the space is working or needs input. Its idle sessions are stopped; its worktrees, branches and directory are kept. `space-reopen` opens it again.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_close,
        state: None,
    },
    Command {
        name: "space-reopen",
        doc: "Open an archived space again, read with completion, and enter it. Its sessions stay stopped.",
        args: ArgSpec::Listed {
            list: ARCHIVED_LIST,
            prompt: "Reopen space:",
            history: "space",
        },
        reach: Reach::Person,
        run: space_reopen,
        state: None,
    },
    Command {
        name: "space-rename",
        doc: "Give the space in front a new title, and a name made from it. Its directory stays where it is.",
        args: ArgSpec::Text {
            prompt: "Space title:",
            history: "space-title",
        },
        reach: Reach::Person,
        run: space_rename,
        state: None,
    },
    Command {
        name: "space-instructions",
        doc: "Edit the instructions of the space in front: the goal, in your words, that its orchestrator is given.

They are kept in oso's state directory, never in the space's own directory or a repository.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_instructions,
        state: None,
    },
    Command {
        name: "space-orchestrator-start",
        doc: "Start the orchestrator of the space in front, in the space's own directory: the session you talk to about the issue, which works through sessions in worktrees of the space's repositories.

It is told the space's repositories, title and instructions, and given each trusted repository as a working directory. With a prefix argument, a space directory that is gone is made again, empty, at its place.",
        args: ArgSpec::Count,
        reach: Reach::Person,
        run: space_orchestrator_start,
        state: None,
    },
    Command {
        name: "space-orchestrator-replace",
        doc: "Replace the orchestrator of the space in front by a new one, which takes over from it.

It is refused while the orchestrator works or needs input. The old one is stopped and kept, stopped, as `NAME-was-N`; once it ended, the new one starts under the space's name, told to read `handoff.md` first, where the old one's transcript is, and the space's sessions. With no `handoff.md` in the space's directory, it asks first.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_orchestrator_replace,
        state: None,
    },
    Command {
        name: "space-confirm",
        doc: "Do what the space question just answered with y offers: paste `/add-dir` into the space's orchestrator, not sent, or replace its orchestrator.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_confirm,
        state: None,
    },
    Command {
        name: "space-directory",
        doc: "List the directory of the space in front, where its orchestrator works, making it first if it has none.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: space_directory,
        state: None,
    },
];

/// Asks the host for `request`.
fn ask(ctx: &mut Ctx<'_>, request: SpaceRequest) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::Spaces(request));
    Ok(())
}

/// The prompt's answer, which must not be empty.
fn answer(ctx: &Ctx<'_>, none: &str) -> Result<String, CommandError> {
    super::super::not_in_prompt(ctx)?;
    match ctx.answer().map(str::trim) {
        Some(answer) if !answer.is_empty() => Ok(answer.to_owned()),
        _ => Err(CommandError::new(none.to_owned())),
    }
}

fn space_new(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let title = answer(ctx, "No title")?;
    ask(ctx, SpaceRequest::New(title))
}

fn space_add_repo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let repository = answer(ctx, "No repository")?;
    ask(ctx, SpaceRequest::AddRepo(repository))
}

fn space_remove_repo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No repository")?;
    ask(ctx, SpaceRequest::RemoveRepo(name))
}

fn space_use_repo(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No repository")?;
    ask(ctx, SpaceRequest::UseRepo(name))
}

fn space_close(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, SpaceRequest::Close)
}

fn space_reopen(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No space")?;
    ask(ctx, SpaceRequest::Reopen(name))
}

fn space_rename(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let title = answer(ctx, "No title")?;
    ask(ctx, SpaceRequest::Rename(title))
}

fn space_instructions(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, SpaceRequest::Instructions)
}

fn space_orchestrator_start(ctx: &mut Ctx<'_>, args: Args) -> Result<(), CommandError> {
    let remake_directory = args.raw().is_some();
    ask(ctx, SpaceRequest::StartOrchestrator { remake_directory })
}

fn space_orchestrator_replace(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, SpaceRequest::ReplaceOrchestrator)
}

fn space_confirm(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    if !ctx.confirmed() {
        return Err(CommandError::new("The space's question answers this"));
    }
    ask(ctx, SpaceRequest::Confirmed)
}

fn space_directory(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    ask(ctx, SpaceRequest::Directory)
}
