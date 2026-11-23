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

/// A native session's composer (ADR 0165, step 2) is made again with its session: an empty
/// one lets the restart go, carrying nothing of it, and one that holds a message refuses it,
/// naming it, as the restart would lose the message.
#[test]
fn an_empty_composer_is_left_behind_and_one_holding_a_message_refuses_the_restart() {
    use oso_core::buffer::{Mode, Replacement};
    let mut editor = Editor::with_buffer(Buffer::from_text(oso_core::editor::SCRATCH, ""));
    let mut composer = Buffer::from_text("*compose: s*", "");
    composer.set_mode(Mode::Composer);
    composer.withhold_from_agents();
    let input = editor.add_buffer(composer);
    let state = crate::carry::export_value(&editor, &[], None, None).unwrap();
    assert!(!state.to_string().contains("*compose: s*"), "{state}");
    let edit = Replacement {
        range: 0..0,
        text: "push it",
    };
    editor.apply_edits(input, &[edit]).unwrap();
    let refused = crate::carry::export_value(&editor, &[], None, None).unwrap_err();
    assert!(refused.contains("*compose: s*"), "{refused}");
    assert!(!refused.contains("push it"), "{refused}");
}

/// A native session's plan's edit (ADR 0165, part 3d) goes with its call, which a restart
/// denies: one as Claude wrote it is left behind, and one the person changed refuses the
/// restart, naming it and quoting none of it.
#[test]
fn a_plans_edit_is_left_behind_unless_the_person_changed_it() {
    use oso_core::buffer::{Mode, Replacement};
    let mut editor = Editor::with_buffer(Buffer::from_text(oso_core::editor::SCRATCH, ""));
    let mut plan = Buffer::from_text("*plan: s*", "Write hello.txt\n");
    plan.set_mode(Mode::Plan);
    plan.withhold_from_agents();
    let id = editor.add_buffer(plan);
    let state = crate::carry::export_value(&editor, &[], None, None).unwrap();
    assert!(!state.to_string().contains("*plan: s*"), "{state}");
    assert!(!state.to_string().contains("hello.txt"), "{state}");
    let edit = Replacement {
        range: 6..11,
        text: "pelican",
    };
    editor.apply_edits(id, &[edit]).unwrap();
    let refused = crate::carry::export_value(&editor, &[], None, None).unwrap_err();
    assert!(refused.contains("*plan: s*"), "{refused}");
    assert!(!refused.contains("pelican"), "{refused}");
}

/// A composer wraps (ADR 0165), and its entry stands for `*scratch*`, which it must not wrap.
#[test]
fn a_composer_that_wraps_leaves_the_scratch_buffer_as_it_is() {
    use oso_core::buffer::Mode;
    let export = |editor: &Editor| crate::carry::export_value(editor, &[], None, None).unwrap();
    let mut editor = Editor::default();
    let scratch = editor.window().buffer();
    let mut composer = Buffer::from_text("*compose: s*", "");
    composer.set_mode(Mode::Composer);
    let composer = editor.add_buffer(composer);
    editor.set_wraps(composer, true);
    let state = crate::carry::parse(&export(&editor)).unwrap();
    assert!(state.buffers.iter().all(|entry| !entry.wraps));
    // Its own setting still goes.
    editor.set_wraps(scratch, true);
    let exported = export(&editor);
    assert_eq!(exported["buffers"][0]["wraps"], true);
    assert!(exported["buffers"][1].get("wraps").is_none());
}
