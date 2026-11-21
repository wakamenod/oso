//! Starting a review in a space whose orchestrator is recorded (ADR 0163): `review-layout`'s
//! menu there asks for the space's repository, then what of it to review, `review-start`'s
//! menu: a pull request (`review-pull-request`, for the repository the host set as the
//! target), a worktree (`review-worktree`) or this checkout (`review-checkout`). Each only
//! names its answer to the host, which holds what each root is.
//!
//! Must not depend on: `app`, `dispatch` or winit; the host finds the projects off the UI
//! thread and opens each next step.

use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::{ReviewRequest, ReviewStart};

/// The list `review-repository` completes over: the space in front's trusted repositories, by
/// their names in it.
pub const REVIEW_REPOSITORIES_LIST: &str = "review-repositories";
/// The built-in menu of what of a space's repository to review.
pub const REVIEW_START_MENU: &str = "review-start";
/// The list `review-worktree` completes over: the chosen repository's worktrees, by path.
pub const REVIEW_WORKTREES_LIST: &str = "review-worktrees";

/// The commands of a start in a space.
pub const COMMANDS: &[Command] = &[
    Command {
        name: "review-repository",
        doc: "Review a repository of the space in front, read with completion, as `review-layout` asks in a space whose orchestrator is recorded.\n\nThen `review-start`'s menu asks what of it to review: a pull request, a worktree or this checkout. Only a repository oso trusts is listed.",
        args: ArgSpec::Listed {
            list: REVIEW_REPOSITORIES_LIST,
            prompt: "Review repository:",
            history: "review-repository",
        },
        reach: Reach::Person,
        run: review_repository,
        state: None,
    },
    Command {
        name: "review-worktree",
        doc: "Review a worktree of the repository `review-repository` chose, read with completion: the review menu opens for it.",
        args: ArgSpec::Listed {
            list: REVIEW_WORKTREES_LIST,
            prompt: "Review worktree:",
            history: "review-worktree",
        },
        reach: Reach::Person,
        run: review_worktree,
        state: None,
    },
    Command {
        name: "review-checkout",
        doc: "Review this checkout of the repository `review-repository` chose: the selected buffer's worktree of it, else its main checkout. The review menu opens for it.",
        args: ArgSpec::None,
        reach: Reach::Person,
        run: review_checkout,
        state: None,
    },
];

/// The prompt's answer, which must not be empty.
fn answer(ctx: &Ctx<'_>, none: &str) -> Result<String, CommandError> {
    match ctx.answer().map(str::trim) {
        Some(answer) if !answer.is_empty() => Ok(answer.to_owned()),
        _ => Err(CommandError::new(none.to_owned())),
    }
}

fn start(ctx: &mut Ctx<'_>, step: ReviewStart) -> Result<(), CommandError> {
    ctx.editor.request_review(ReviewRequest::Start(step));
    ctx.request_redraw();
    Ok(())
}

fn review_repository(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let name = answer(ctx, "No repository")?;
    start(ctx, ReviewStart::Repository(name))
}

fn review_worktree(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let path = answer(ctx, "No worktree")?;
    start(ctx, ReviewStart::Worktree(path))
}

fn review_checkout(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    super::super::not_in_prompt(ctx)?;
    start(ctx, ReviewStart::Checkout)
}

#[cfg(test)]
mod tests {
    use oso_core::command::{Args, Ctx, Invocation};
    use oso_core::editor::{Editor, ReviewRequest, ReviewStart};

    use crate::commands::registry;

    /// Runs `name` with `answer` as its prompt's.
    fn run(editor: &mut Editor, name: &str, answer: Option<&str>) -> bool {
        let registry = registry();
        let id = registry.lookup(name).unwrap();
        let invocation = Invocation {
            answer,
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(editor, invocation);
        registry.execute(id, &mut ctx, Args::NONE).is_ok()
    }

    #[test]
    fn each_step_names_its_answer_to_the_host() {
        let mut editor = Editor::default();
        assert!(run(&mut editor, "review-repository", Some("front")));
        assert!(run(&mut editor, "review-worktree", Some(" /w/front-wt ")));
        assert!(run(&mut editor, "review-checkout", None));
        assert_eq!(
            editor.take_review_requests(),
            [
                ReviewRequest::Start(ReviewStart::Repository("front".into())),
                ReviewRequest::Start(ReviewStart::Worktree("/w/front-wt".into())),
                ReviewRequest::Start(ReviewStart::Checkout),
            ]
        );
    }

    #[test]
    fn an_empty_answer_asks_nothing() {
        let mut editor = Editor::default();
        assert!(!run(&mut editor, "review-repository", Some("  ")));
        assert!(!run(&mut editor, "review-worktree", None));
        assert!(editor.take_review_requests().is_empty());
    }
}
