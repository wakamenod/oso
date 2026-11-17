//! The commands that send a review's comments: what each asks of the host, where each refuses,
//! and the session `C-u` reads for the message.
//!
//! Must not depend on: `bindings`, `dispatch`, `app` or winit; these run commands through the
//! registry.

use oso_core::buffer::{Buffer, Mode};
use oso_core::command::Args;
use oso_core::editor::ReviewRequest;

use super::super::super::tests::run;
use super::super::tests::{in_project, with_review};

#[test]
fn c_c_c_c_in_a_review_asks_for_its_message() {
    let (mut editor, project) = in_project(true);
    let review = with_review(&mut editor, project);
    assert_eq!(run(&mut editor, "review-send", Args::NONE).0, Ok(()));
    assert_eq!(editor.take_review_requests(), [ReviewRequest::Send(review)]);
}

#[test]
fn sending_refuses_outside_a_review_and_its_message() {
    let (mut editor, _) = in_project(true);
    assert!(run(&mut editor, "review-send", Args::NONE).0.is_err());
    assert!(
        run(&mut editor, "review-message-send", Args::NONE)
            .0
            .is_err()
    );
    assert!(
        run(&mut editor, "review-message-drop", Args::NONE)
            .0
            .is_err()
    );
    assert!(
        run(&mut editor, "review-message-take", Args::NONE)
            .0
            .is_err()
    );
    assert!(editor.take_review_requests().is_empty());
}

#[test]
fn the_message_asks_to_send_or_drop_itself() {
    let (mut editor, _) = in_project(true);
    let mut message = Buffer::from_text("*review message*", "Review comments");
    message.set_mode(Mode::ReviewMessage);
    let buffer = editor.add_buffer(message);
    editor.show(buffer);
    assert_eq!(
        run(&mut editor, "review-message-send", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        run(&mut editor, "review-message-take", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        run(&mut editor, "review-message-drop", Args::NONE).0,
        Ok(())
    );
    assert_eq!(
        editor.take_review_requests(),
        [
            ReviewRequest::MessageSend { buffer, to: None },
            ReviewRequest::MessageTake { buffer },
            ReviewRequest::MessageDrop { buffer },
        ]
    );
}
