//! What the rings of commit and pull request messages keep for `M-p`: a message's text, unless
//! it is too long or a secret was yanked into it, as a text that a secret was put in stays
//! marked.
//!
//! Must not depend on: winit or the window; these are the check's own tests.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

use super::{cancelled_says, ring_text};

#[test]
fn a_message_is_kept_unless_too_long_or_a_secret_was_yanked_into_it() {
    let mut editor = Editor::with_buffer(Buffer::from_text("*commit: p*", "Subject\n"));
    let id = editor.window().buffer();
    let shown = editor.buffer(id).unwrap();
    assert_eq!(ring_text(shown, 64).as_deref(), Some("Subject\n"));
    assert_eq!(ring_text(shown, 4), None, "longer than a message");
    assert_eq!(
        cancelled_says("Commit cancelled", Some(shown)),
        "Commit cancelled; M-p in the next message brings it back"
    );
    editor.kill_new("hunter2", true);
    editor.yank(0, false).unwrap();
    let shown = editor.buffer(id).unwrap();
    assert!(shown.is_secret());
    assert_eq!(ring_text(shown, 64), None);
    assert_eq!(
        cancelled_says("Pull request cancelled", Some(shown)),
        "Pull request cancelled; its message held secret text, which M-p does not bring back"
    );
}
