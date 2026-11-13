//! Commands for worktree sessions (ADR 0121): starting a session in a new linked worktree from a
//! brief written in `*Brief*`, listing a repository's worktrees in `*Worktrees*`, and removing
//! one whose work is done, with the answers to the questions those ask.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state
//! and asks the host, whose sessions worker runs `git` and `gh`. Each needs a person at the
//! keyboard: they run processes, and `git worktree add` runs the repository's hooks.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::SessionRequest;

/// The worktree commands.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "session-worktree",
        doc: "Start a Claude Code session in a new git worktree of this project, from a brief written in `*Brief*`.\n\nIt reads a branch, completing over the project's local branches; a name that is none of them makes a new branch, which starts at `HEAD`, or with `C-u` at a base read after it. `*Brief*` opens for the brief, sent with `C-c C-c` and dropped with `C-c C-k`. Sending runs `git worktree add` in `.claude/worktrees/` of the repository, copies the files `.worktreeinclude` names that git ignores, and starts a session there with the brief as its first prompt. It needs a trusted project, as git runs the repository's hooks, and refuses a branch checked out elsewhere.",
        args: ArgSpec::BranchAndBaseOnPrefix {
            prompt: "Worktree branch:",
            base_prompt: "Base:",
            history: "worktree-branch",
        },
        reach: Reach::Person,
        run: session_worktree,
        state: None,
    },
    Command {
        name: "brief-send",
        doc: "Send the brief in this `*Brief*` buffer: make its worktree and start its session.\n\nThe brief goes to the session as its first prompt, after a header that says where it runs and what the session that started it left: its transcript, its plans and its uncommitted changes, which did not come along.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: brief_send,
        state: None,
    },
    Command {
        name: "brief-cancel",
        doc: "Drop the brief in this `*Brief*` buffer unsent.\n\nNo worktree is made and no branch is created.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: brief_cancel,
        state: None,
    },
    Command {
        name: "worktree-list",
        doc: "List the linked worktrees of the selected project's repository in `*Worktrees*`.\n\nEach row shows its path, its branch, its uncommitted and untracked files, whether the branch is merged and how that was found, the sessions in it, the buffers with unsaved changes under it, and whether `worktree-remove` would remove it or why not. It runs `git` and `gh`, and needs a trusted project. In `*Worktrees*`, `g` lists them again and `D` removes the one at point.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: worktree_list,
        state: None,
    },
    Command {
        name: "worktree-remove",
        doc: "Remove a linked worktree whose work is done: the one at point in `*Worktrees*`, else the selected buffer's.\n\nIt removes it only when it has no uncommitted or untracked files, no buffer under it has unsaved changes and no brief for it is unsent, no session in it is busy, and its branch is merged. It asks first, naming what git ignores there and will be deleted with it. Then it stops the idle sessions in it and runs `git worktree remove`, never with `--force`. It never deletes the branch. It needs a trusted project.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: worktree_remove,
        state: None,
    },
    Command {
        name: "worktree-remove-confirmed",
        doc: "Remove the worktree `worktree-remove` asked about, as its question's `y` does.\n\nEvery check runs again first. Only the question runs it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: worktree_remove_confirmed,
        state: None,
    },
    Command {
        name: "session-start-shared",
        doc: "Start the session that was asked about in a directory where another session runs, as the question's `y` does.\n\nThe two sessions then change the same files; a worktree (`session-worktree`) keeps them apart. Only the question runs it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: session_start_shared,
        state: None,
    },
];

fn session_worktree(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    let Some((branch, base)) = ctx.branch_and_base() else {
        return Err(CommandError::new(
            "session-worktree reads its branch from the keyboard",
        ));
    };
    let branch = branch.trim();
    if branch.is_empty() {
        return Err(CommandError::new("No branch given"));
    }
    let base = base
        .map(str::trim)
        .filter(|base| !base.is_empty())
        .map(str::to_owned);
    ctx.editor.request_session(SessionRequest::Worktree {
        branch: branch.to_owned(),
        base,
    });
    Ok(())
}

/// The current buffer, if it is a `*Brief*`.
fn brief(ctx: &Ctx<'_>) -> Result<oso_core::buffer::BufferId, CommandError> {
    super::not_in_prompt(ctx)?;
    if ctx.editor.current().1.mode() != Mode::Brief {
        return Err(CommandError::new("Not in a *Brief* buffer"));
    }
    Ok(ctx.editor.window().buffer())
}

fn brief_send(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = brief(ctx)?;
    ctx.editor
        .request_session(SessionRequest::SendBrief(buffer));
    Ok(())
}

fn brief_cancel(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = brief(ctx)?;
    ctx.editor
        .request_session(SessionRequest::CancelBrief(buffer));
    Ok(())
}

fn worktree_list(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor.request_session(SessionRequest::WorktreeList);
    Ok(())
}

fn worktree_remove(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    ctx.editor
        .request_session(SessionRequest::WorktreeRemove(None));
    Ok(())
}

fn worktree_remove_confirmed(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if !ctx.confirmed() {
        return Err(CommandError::new("worktree-remove asks this itself"));
    }
    ctx.editor
        .request_session(SessionRequest::WorktreeRemoveConfirmed);
    Ok(())
}

fn session_start_shared(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::not_in_prompt(ctx)?;
    if !ctx.confirmed() {
        return Err(CommandError::new(
            "Starting a session asks this itself when another runs there",
        ));
    }
    ctx.editor.request_session(SessionRequest::StartShared);
    Ok(())
}

#[cfg(test)]
mod tests {
    use oso_core::buffer::Mode;
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, SessionRequest};

    use super::super::tests::editor;

    /// Runs `name` as `invocation` says, and what it said if it failed.
    fn run_as(editor: &mut Editor, name: &str, invocation: Invocation<'_>) -> Result<(), String> {
        let registry = crate::commands::registry();
        let id = registry.lookup(name).unwrap();
        let mut ctx = Ctx::invoked(editor, invocation);
        registry
            .execute(id, &mut ctx, Args::NONE)
            .map_err(|error| error.to_string())
    }

    fn read(editor: &mut Editor, branch: &str, base: Option<&str>) -> Result<(), String> {
        let invocation = Invocation {
            answer: Some(branch),
            second: base,
            ..Invocation::default()
        };
        run_as(editor, "session-worktree", invocation)
    }

    fn confirmed() -> Invocation<'static> {
        Invocation {
            confirmed: true,
            ..Invocation::default()
        }
    }

    #[test]
    fn session_worktree_asks_for_the_branch_and_base_read() {
        let mut editor = editor("");
        assert_eq!(read(&mut editor, "feature/api", None), Ok(()));
        assert_eq!(read(&mut editor, " fix ", Some(" main ")), Ok(()));
        assert_eq!(read(&mut editor, "fix", Some("")), Ok(()));
        let worktree = |branch: &str, base: Option<&str>| SessionRequest::Worktree {
            branch: branch.to_owned(),
            base: base.map(str::to_owned),
        };
        assert_eq!(
            editor.take_session_requests(),
            [
                worktree("feature/api", None),
                worktree("fix", Some("main")),
                worktree("fix", None),
            ]
        );
    }

    #[test]
    fn session_worktree_refuses_no_branch() {
        let mut editor = editor("");
        assert!(
            read(&mut editor, " ", None)
                .unwrap_err()
                .contains("No branch")
        );
        let nothing = run_as(&mut editor, "session-worktree", Invocation::default());
        assert!(nothing.unwrap_err().contains("from the keyboard"));
        assert!(editor.take_session_requests().is_empty());
    }

    #[test]
    fn a_brief_is_sent_or_dropped_only_from_its_buffer() {
        let mut editor = editor("");
        let buffer = editor.window().buffer();
        for name in ["brief-send", "brief-cancel"] {
            let refused = run_as(&mut editor, name, Invocation::default()).unwrap_err();
            assert!(refused.contains("*Brief*"), "{name}: {refused}");
        }
        assert!(editor.take_session_requests().is_empty());
        assert!(editor.buffer_mut(buffer).unwrap().set_mode(Mode::Brief));
        assert_eq!(
            run_as(&mut editor, "brief-send", Invocation::default()),
            Ok(())
        );
        assert_eq!(
            run_as(&mut editor, "brief-cancel", Invocation::default()),
            Ok(())
        );
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::SendBrief(buffer),
                SessionRequest::CancelBrief(buffer)
            ]
        );
    }

    #[test]
    fn listing_and_removing_ask_the_host() {
        let mut editor = editor("");
        assert_eq!(
            run_as(&mut editor, "worktree-list", Invocation::default()),
            Ok(())
        );
        assert_eq!(
            run_as(&mut editor, "worktree-remove", Invocation::default()),
            Ok(())
        );
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::WorktreeList,
                SessionRequest::WorktreeRemove(None)
            ]
        );
    }

    #[test]
    fn the_answers_run_only_from_their_questions() {
        let mut editor = editor("");
        for name in ["worktree-remove-confirmed", "session-start-shared"] {
            assert!(
                run_as(&mut editor, name, Invocation::default()).is_err(),
                "{name}"
            );
        }
        assert!(editor.take_session_requests().is_empty());
        assert_eq!(
            run_as(&mut editor, "worktree-remove-confirmed", confirmed()),
            Ok(())
        );
        assert_eq!(
            run_as(&mut editor, "session-start-shared", confirmed()),
            Ok(())
        );
        assert_eq!(
            editor.take_session_requests(),
            [
                SessionRequest::WorktreeRemoveConfirmed,
                SessionRequest::StartShared
            ]
        );
    }
}
