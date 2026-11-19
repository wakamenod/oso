//! What `space-new` does once its repository is looked at (ADR 0159): the space made, named
//! as the fallback names it, entered with `*scratch*` alone, so that the buffer shown before is
//! in no window of it, and its orchestrator started once its directory is made.
//!
//! Must not depend on: key presses or how a frame is drawn.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, SCRATCH};

use crate::app::App;
use crate::sessions::spaces::Member;
use crate::sessions::spaces::edit::Naming;
use crate::sessions::spaces::stored::fallback_names;

impl App {
    /// Makes the space for `description` of repository `member`, named as the fallback names
    /// it (ADR 0159), enters it with `*scratch*` alone, so that the buffer shown before is in
    /// no window of it, and starts its orchestrator once its directory is made: the person's
    /// `space-new` is the key that starts it.
    pub(in crate::app::sessions) fn make_new_space(
        &mut self,
        description: &str,
        member: Member,
        now: u64,
    ) {
        let (slug, display) = fallback_names(description, &member.name());
        let naming = Naming {
            description: description.to_owned(),
            display,
            slug,
        };
        let taken = self.session_name_taken();
        let spaces = self.watch.spaces_mut();
        let Some(at) = spaces.new_space(&naming, member, now, &taken) else {
            return self
                .editor
                .message("No id for a new space yet; try again".to_owned());
        };
        let scratch = scratch(&mut self.editor);
        // Entered for the first time, it shows `*scratch*` alone, until its orchestrator. With
        // no space in front, the frame's tree is no space's and the new one would take it
        // whole: `*scratch*` alone takes its place first.
        self.watch.spaces_mut().showed(at, None, Some(scratch));
        if self.watch.spaces().current().is_none() {
            let tree = self.editor.window_tree_showing(&[scratch]);
            let _ = self.editor.swap_window_tree(tree);
        }
        self.enter_space(at);
        if let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) {
            self.hosting.spaces.placeholders.insert(id, scratch);
        }
        self.make_space_directory(at, false, true);
        let name = self.space_label(at);
        self.editor
            .message(format!("Space {name} made; its orchestrator starts"));
    }
}

/// The `*scratch*` buffer, made again if it was killed.
fn scratch(editor: &mut Editor) -> BufferId {
    let found = editor
        .buffers()
        .find(|(_, buffer)| buffer.name() == SCRATCH && buffer.path().is_none());
    match found {
        Some((id, _)) => id,
        None => editor.add_buffer(Buffer::new(SCRATCH)),
    }
}
