//! Sending a review's comments to an agent session (ADR 0122, step 12): `C-c C-c` in a review
//! buffer gathers the person's comments that were not sent into `*review message*`, and there
//! `C-c C-c` pastes it into the review's session and `C-c C-k` drops it. A message made from a
//! pull request's checks or threads (ADR 0127, step 6b) is sent and dropped the same way, and
//! `C-c C-t` takes in what it left out.
//!
//! Must not depend on: winit, the GPU or `bindings`; a command sees only the host-owned state.
//! The host gathers, composes and pastes; these only ask it.
//!
//! - Every command here is `Person`: what goes to a session is the person's typing, and the
//!   message is theirs to change (Q7), so no command of the agent channel sends or drops one.
//! - `Q` (`review-ask`, ADR 0163) asks a question about the place at point, sent at once to
//!   the space's orchestrator: the question is the person's, typed in the prompt and confirmed
//!   with `RET` there, which stands for the Enter a paste otherwise waits for.

use oso_core::buffer::Mode;
use oso_core::command::{ArgSpec, Args, Command, CommandError, Ctx, Reach};
use oso_core::editor::ReviewRequest;

use super::current_review;
use crate::commands::agent_sessions::SEND_LIST;

/// The keys of a review's message and their commands, which `bindings` binds in its keymap.
pub const MESSAGE_KEYS: &[(&str, &str)] = &[
    ("C-c C-c", "review-message-send"),
    ("C-c C-k", "review-message-drop"),
    ("C-c C-t", "review-message-take"),
];

/// The commands that send a review's comments.
pub const COMMANDS: &[Command] = &[
    person_command!(
        "review-send",
        "Gather your comments of this review that were not sent into a message, shown in `*review message*`.\n\nEach goes with its file, its line or hunk and the hunk's lines, cut past 60 lines; an outdated one with where it was; one in a file that may hold secrets with its place alone. Threads you resolved are left out. Nothing is sent yet: read and change the message, then `C-c C-c` there pastes it.",
        review_send
    ),
    Command {
        name: "review-message-send",
        doc: "Paste this message into its agent session, not submitted, and mark the comments a review's message holds sent.\n\nThe session is the one picked for the review, or the pull request, with `C-u` while it runs, else in a space with an orchestrator that orchestrator, which relays the comments to the review's worktree, else the one that runs in the review's worktree, the one shown last if several do, or for a pull request's checks or threads the one in the worktree its branch is checked out in; with `C-u`, one read with completion, which is then kept. With none, the message stays and nothing is sent. The paste is the person's: read it in the session's input box and press Enter. A session asking for permission or another answer is refused, and the message stays. A comment changed in the review since the message was made stays unsent.",
        args: ArgSpec::ListedOnPrefix {
            list: SEND_LIST,
            prompt: "Send the review message to session:",
            history: "session",
        },
        reach: Reach::Person,
        run: review_message_send,
        state: None,
    },
    person_command!(
        "review-message-drop",
        "Drop this message and show what it was made from again, its review or pull request: nothing is sent, and a review's comments stay unsent.",
        review_message_drop
    ),
    person_command!(
        "review-message-take",
        "In a message made from a pull request's checks or threads, take in what it left out: the comments of others than you and the repository's owners, members and collaborators, and a fork's logs, quoted, above the end of what GitHub said (ADR 0127, step 6b). What you wrote stays.",
        review_message_take
    ),
    Command {
        name: "review-ask",
        doc: "Ask a question about the hunk or line at point, or the lines the region spans, sent at once to the space's orchestrator.\n\nThe question goes with the review's worktree, the file, the line numbers and the lines, as a comment's place and hunk go with `C-c C-c`, and is submitted: the orchestrator answers in its terminal, shown in the review layout's slot while you stay in the review. Outside a space with an orchestrator it goes to the review's session, as `C-c C-c` sends. A file that may hold secrets sends its place alone. A session asking for permission or another answer is refused; `M-p` in the prompt brings the question back.",
        args: ArgSpec::Text {
            prompt: "Ask about this: ",
            history: "review-ask",
        },
        reach: Reach::Person,
        run: review_ask,
        state: None,
    },
];

fn review_ask(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    // A yank of a secret entry turns the prompt secret: its text stays here.
    if ctx.answer_secret() {
        return Err(CommandError::new("Secret text is not sent"));
    }
    let question = ctx.answer().unwrap_or_default().trim();
    if question.is_empty() {
        return Err(CommandError::new("No question was asked"));
    }
    if question.len() > crate::commands::agent_sessions::MAX_SEND_BYTES {
        return Err(CommandError::new(
            "The question is past the 256 KiB a session takes",
        ));
    }
    let question = question.to_owned();
    ctx.editor
        .request_review(ReviewRequest::Ask { review, question });
    ctx.request_redraw();
    Ok(())
}

fn review_send(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let review = current_review(ctx)?;
    ctx.editor.request_review(ReviewRequest::Send(review));
    ctx.request_redraw();
    Ok(())
}

/// The review message the selected window shows, refused elsewhere.
fn message(ctx: &Ctx<'_>) -> Result<oso_core::buffer::BufferId, CommandError> {
    super::super::not_in_prompt(ctx)?;
    let (window, buffer) = ctx.editor.current();
    if buffer.mode() != Mode::ReviewMessage {
        return Err(CommandError::new("Not in a review's message"));
    }
    Ok(window.buffer())
}

fn review_message_send(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    let to = ctx
        .answer()
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    ctx.editor
        .request_review(ReviewRequest::MessageSend { buffer, to });
    ctx.request_redraw();
    Ok(())
}

fn review_message_drop(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor
        .request_review(ReviewRequest::MessageDrop { buffer });
    ctx.request_redraw();
    Ok(())
}

fn review_message_take(ctx: &mut Ctx<'_>, _: Args) -> Result<(), CommandError> {
    let buffer = message(ctx)?;
    ctx.editor
        .request_review(ReviewRequest::MessageTake { buffer });
    ctx.request_redraw();
    Ok(())
}

#[cfg(test)]
mod tests;
