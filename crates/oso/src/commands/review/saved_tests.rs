//! The tests of saved reviews' commands: what closing a review the host keeps says and asks,
//! and which requests resuming and forgetting make.
//!
//! Must not depend on: the host or its threads.

use oso_core::command::{Args, Ctx, Invocation};
use oso_core::editor::ReviewRequest;

use crate::commands::review::tests::{comment_in, in_project, with_review};
use crate::commands::tests::run;

#[test]
fn a_review_the_host_keeps_closes_with_no_question_and_says_its_comments_are_kept() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    comment_in(&mut editor, review);
    editor.set_review_kept(review, true);
    assert_eq!(
        super::super::unsent_everywhere(&editor),
        0,
        "nothing lost on leaving"
    );
    assert_eq!(run(&mut editor, "review-kill", Args::NONE).0, Ok(()));
    assert!(editor.prompt().is_none());
    assert!(editor.review(review).is_none());
    let messages = editor.messages_buffer();
    let logged = editor.buffer(messages).unwrap().text().to_string();
    assert!(logged.contains("1 unsent comment kept; review-resume brings them back"));
    // One the host could not save asks as before.
    let review = with_review(&mut editor, project);
    comment_in(&mut editor, review);
    editor.set_review_kept(review, false);
    assert_eq!(super::super::unsent_everywhere(&editor), 1);
}

#[test]
fn resuming_and_forgetting_ask_the_host_for_the_review_picked() {
    let (mut editor, _) = in_project(true);
    let registry = crate::commands::registry();
    for (name, request) in [
        (
            "review-resume",
            ReviewRequest::Resume("uncommitted /p".into()),
        ),
        (
            "review-forget",
            ReviewRequest::Forget("uncommitted /p".into()),
        ),
    ] {
        let command = registry.lookup(name).unwrap();
        let answered = Invocation {
            answer: Some("uncommitted /p"),
            ..Invocation::default()
        };
        let mut ctx = Ctx::invoked(&mut editor, answered);
        assert!(registry.execute(command, &mut ctx, Args::NONE).is_ok());
        assert_eq!(editor.take_review_requests(), [request]);
        let mut ctx = Ctx::invoked(&mut editor, Invocation::default());
        assert!(
            registry.execute(command, &mut ctx, Args::NONE).is_err(),
            "none picked"
        );
    }
}
