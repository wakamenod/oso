//! The commands that give a name, a title or a message read in a prompt to git, `gh`, a
//! language server or a session's record refuse one that a secret was yanked into, as a text
//! that a secret was put in stays marked, and ask nothing of the host.
//!
//! Must not depend on: `app`, `dispatch` or winit; these run commands through the registry.

use oso_core::command::{Args, Ctx, ExecuteError, Invocation};
use oso_core::editor::Editor;

use crate::commands::registry;

/// Each command, and what it says when its answer may hold a secret.
const REFUSED: &[(&str, &str)] = &[
    (
        "claude",
        "A directory or name holding what may be secret is not used",
    ),
    (
        "session-rename-to",
        "A name holding what may be secret is not kept",
    ),
    (
        "session-worktree",
        "A branch or base holding what may be secret is not used",
    ),
    (
        "git-stash-message",
        "A stash message holding what may be secret is not kept",
    ),
    (
        "git-graph-label",
        "A name or message holding what may be secret is not given to git",
    ),
    (
        "git-graph-branch-name",
        "A name or message holding what may be secret is not given to git",
    ),
    (
        "git-graph-tag-name",
        "A name or message holding what may be secret is not given to git",
    ),
    (
        "git-graph-tag-message",
        "A name or message holding what may be secret is not given to git",
    ),
    (
        "git-branch-pick",
        "A branch or name holding what may be secret is not given to git",
    ),
    (
        "git-branch-start",
        "A branch or name holding what may be secret is not given to git",
    ),
    (
        "git-branch-name",
        "A branch or name holding what may be secret is not given to git",
    ),
    (
        "git-branch-new-name",
        "A branch or name holding what may be secret is not given to git",
    ),
    (
        "git-push-to",
        "A remote or branch holding what may be secret is not given to git",
    ),
    (
        "git-push-to-remote",
        "A remote or branch holding what may be secret is not given to git",
    ),
    (
        "git-graph-of",
        "A ref holding what may be secret is not given to git",
    ),
    (
        "git-pr-create-base",
        "A base holding what may be secret is not given to gh",
    ),
    (
        "review-commit",
        "A revision holding what may be secret is not given to git",
    ),
    (
        "review-range",
        "A revision holding what may be secret is not given to git",
    ),
    (
        "lsp-rename-to",
        "A name holding what may be secret is not sent to the server",
    ),
];

/// Runs `name` with `answer` as both its prompts' answers, which may hold a secret if
/// `secret`, and what it said if it failed.
fn run(editor: &mut Editor, name: &str, answer: &str, secret: bool) -> Result<(), String> {
    let registry = registry();
    let id = registry.lookup(name).unwrap();
    let invocation = Invocation {
        answer: Some(answer),
        second: Some(answer),
        answer_secret: secret,
        ..Invocation::default()
    };
    let mut ctx = Ctx::invoked(editor, invocation);
    (registry.execute(id, &mut ctx, Args::NONE)).map_err(|e| match e {
        ExecuteError::Failed { error, .. } => error.message().to_owned(),
        other => panic!("{other}"),
    })
}

/// Whether the command asked anything of the host.
fn asked(editor: &mut Editor) -> bool {
    !editor.take_session_requests().is_empty()
        || !editor.take_git_requests().is_empty()
        || !editor.take_server_requests().is_empty()
}

#[test]
fn an_answer_that_a_secret_was_yanked_into_is_refused_and_asks_nothing() {
    for (name, says) in REFUSED {
        let (mut editor, _) = crate::commands::review::tests::in_project(true);
        let refused = run(&mut editor, name, "a..b", true);
        assert_eq!(refused, Err((*says).to_owned()), "{name}");
        assert!(!asked(&mut editor), "{name}");
        // Not refused so without one, whatever else it says here.
        let plain = run(&mut editor, name, "a..b", false);
        assert_ne!(plain, Err((*says).to_owned()), "{name}");
    }
}
