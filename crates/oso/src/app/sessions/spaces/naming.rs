//! How `space-new` names the space it makes and starts (ADR 0159): its slug and display name
//! from the description, unlike every space's and every session's name, entered with
//! `*scratch*` alone, and its orchestrator started once its directory is made.
//!
//! Must not depend on: key presses or how a frame is drawn.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, SCRATCH};

use crate::app::App;
use crate::sessions::identity::same_name;
use crate::sessions::spaces::Member;
use crate::sessions::spaces::edit::Naming;
use crate::sessions::spaces::stored::fallback_names;

impl App {
    /// Whether a name is one a session oso knows answers to, ASCII case aside (ADR 0155's
    /// set), which no new space's slug may be (ADR 0159).
    pub(super) fn session_name_taken(&self) -> impl Fn(&str) -> bool + use<> {
        let names: Vec<String> = (self.session_names(None).into_iter())
            .map(str::to_owned)
            .collect();
        move |name: &str| names.iter().any(|other| same_name(other, name))
    }

    /// Makes the space for `description` of repository `member`, named as the fallback names
    /// it (ADR 0159), enters it with `*scratch*` alone, so that the buffer shown before is in
    /// no window of it, and starts its orchestrator once its directory is made: the person's
    /// `space-new` is the key that starts it.
    pub(super) fn make_new_space(&mut self, description: &str, member: Member, now: u64) {
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
        // Entered for the first time, it shows `*scratch*` alone, until its orchestrator.
        self.watch.spaces_mut().showed(at, None, Some(scratch));
        self.enter_space(at);
        if let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) {
            self.hosting.spaces.start_after.insert(id);
        }
        self.make_space_directory(at, false);
        let name = self.space_name(at);
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
