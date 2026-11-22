//! A window docked under a session's transcript (ADR 0170), as the agent channel reaches it:
//! an agent's move into one is undone, and its changes there are refused.
//!
//! Must not depend on: `app` or winit; an editor with a composer docked.

use oso_core::buffer::Buffer;
use oso_core::command::Reach;
use oso_core::editor::Editor;
use oso_core::frame::FrameSize;
use oso_core::window::DockSlot;

use super::Dispatcher;
use super::args::Given;

fn docked() -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\n"));
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 80,
    });
    let host = editor.window().buffer();
    let input = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    editor.set_dock(host, DockSlot::Composer, Some(input));
    editor
}

#[test]
fn an_agents_move_into_a_dock_is_undone_and_its_change_there_refused() {
    let mut editor = docked();
    let host = editor.window().buffer();
    let transcript = editor.window().id();
    let mut d = Dispatcher::new();
    let ran = d.run_for_agent(&mut editor, "other-window", Given::Nothing, Reach::Look);
    assert!(ran.is_ok());
    assert_eq!(
        editor.window().id(),
        transcript,
        "back where the person was"
    );
    // The person went into the composer: the agent may not write there.
    let composer = editor.dock_window(host, DockSlot::Composer).unwrap();
    editor.select_window(composer);
    let refused = d.run_for_agent(&mut editor, "insert", Given::Text("hi"), Reach::Change);
    assert!(refused.is_err());
    assert!(editor.current().1.text().is_empty());
}
