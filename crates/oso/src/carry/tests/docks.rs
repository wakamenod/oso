//! The state handoff with a window docked under a session (ADR 0170).
//!
//! Must not depend on: a window or the file system.

use oso_core::buffer::Buffer;
use oso_core::editor::Editor;

/// A window docked under a session (ADR 0170) is not one of the tree's windows a restart
/// carries: its host docks it again.
#[test]
fn a_docked_window_is_not_carried() {
    use oso_core::frame::FrameSize;
    use oso_core::window::DockSlot;
    let mut editor = Editor::with_buffer(Buffer::from_text("*claude: s*", "one\n"));
    editor.set_frame_size(FrameSize {
        rows: 30,
        columns: 80,
    });
    let host = editor.window().buffer();
    let input = editor.add_buffer(Buffer::from_text("*compose: s*", ""));
    editor.set_dock(host, DockSlot::Composer, Some(input));
    let carried = crate::carry::tiled_windows(&editor);
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].buffer(), host);
}
