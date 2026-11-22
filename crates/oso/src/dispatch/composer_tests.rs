//! The keys of a native session's composer and queue (ADR 0165, step 2): in the composer,
//! typing writes and `C-c` keys ask the host; in the transcript, `i` goes to the composer and
//! `e`, `d` and `M-<up>` act on the queued message at point.
//!
//! Must not depend on: `app` or winit; these drive the default dispatcher.

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::{Editor, NativeAction, SessionRequest};

use super::Dispatcher;
use super::motion_tests::send;

fn actions(editor: &mut Editor) -> Vec<NativeAction> {
    (editor.take_session_requests().into_iter())
        .filter_map(|request| match request {
            SessionRequest::Native { action, .. } => Some(action),
            _ => None,
        })
        .collect()
}

#[test]
fn the_composer_takes_typing_and_its_own_keys() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let composer = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    assert!(
        editor
            .buffer_mut(composer)
            .unwrap()
            .set_mode(Mode::Composer)
    );
    editor.show(composer);
    send(
        &mut d,
        &mut editor,
        "h i C-c C-c C-c C-k M-p M-n C-c C-o C-c C-q",
    );
    assert_eq!(editor.current().1.text().to_string(), "hi");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Send,
            NativeAction::Cancel,
            NativeAction::History(1),
            NativeAction::History(-1),
            NativeAction::Transcript,
            NativeAction::Queue,
        ]
    );
}

#[test]
fn the_transcripts_letters_go_to_the_composer_and_act_on_a_queued_message() {
    let (mut d, mut editor) = (Dispatcher::new(), Editor::default());
    let transcript = editor.add_native_session_buffer("*claude: s*");
    editor.show(transcript);
    send(&mut d, &mut editor, "i e d M-<up> M-<down> C-c C-q");
    assert_eq!(
        actions(&mut editor),
        [
            NativeAction::Compose,
            NativeAction::EditQueued,
            NativeAction::DropQueued,
            NativeAction::MoveQueued(-1),
            NativeAction::MoveQueued(1),
            NativeAction::Queue,
        ]
    );
}
