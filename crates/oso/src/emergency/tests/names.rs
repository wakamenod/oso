//! What an entry says of a buffer's name and path once they change (ADR 0062, ADR 0087).
//!
//! Must not depend on: `app` or winit.

use oso_core::editor::Editor;

use super::{Scratch, file_buffer};
use crate::emergency::Emergency;

#[test]
fn a_buffer_renamed_or_moved_is_published_under_its_new_name() {
    let scratch = Scratch::new();
    let mut editor = Editor::default();
    let first = file_buffer(&mut editor, "/a/notes.md", Some(&[]), "first");
    let emergency = Emergency::new(Some(scratch.copies()));
    let publisher = emergency.publisher();
    publisher.publish(&editor);
    let name = |emergency: &Emergency| {
        let state = emergency.shared.state.lock().unwrap();
        let entry = state.entries.iter().find(|entry| entry.buffer == first);
        entry.map(|entry| entry.identity.name.clone())
    };
    assert_eq!(name(&emergency).as_deref(), Some("notes.md"));
    // A second file with that name renames the first, with no change to its text.
    file_buffer(&mut editor, "/b/notes.md", Some(&[]), "second");
    publisher.publish(&editor);
    assert_eq!(name(&emergency).as_deref(), Some("notes.md<a>"));
}
