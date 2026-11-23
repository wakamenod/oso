//! What `space-new` does once its repository is looked at (ADR 0159): the space made, named
//! as the fallback names it, entered with `*scratch*` alone, so that the buffer shown before is
//! in no window of it; named again by the person's `claude` when `[spaces] naming` says so
//! (step 2); and its orchestrator started once its directory is made.
//!
//! Must not depend on: key presses or how a frame is drawn.

use oso_core::buffer::{Buffer, BufferId};
use oso_core::editor::{Editor, SCRATCH};

use std::path::Path;
use std::sync::Arc;

use crate::app::App;
use crate::app::sessions::spaces::printable;
use crate::config::SpacesNaming;
use crate::sessions::oneshot::Running;
use crate::sessions::spaces::edit::Naming;
use crate::sessions::spaces::stored::{fallback_names, now_ms};
use crate::sessions::spaces::summary::Ask;
use crate::sessions::spaces::{Member, SpaceId};
use crate::sessions::worker::{Job, SpaceJob};

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
        // `[spaces] naming` and `root` are the person's (ADR 0173).
        if self.starts_wait() {
            let description = description.to_owned();
            return self.hold_start(move |app| app.make_new_space(&description, member, now));
        }
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
        let Some(id) = self.watch.spaces().stored(at).map(|s| s.id.clone()) else {
            return;
        };
        self.hosting.spaces.placeholders.insert(id.clone(), scratch);
        // Named by the person's `claude` first, when it is to be: nothing that takes the slug,
        // its directory and its orchestrator, comes before its answer (ADR 0159, step 2).
        if self.ask_for_names(&id, description) {
            let name = self.space_label(at);
            return self
                .editor
                .message(format!("Space {name} made; claude names it"));
        }
        self.make_space_directory(at, false, true);
        let name = self.space_label(at);
        self.editor
            .message(format!("Space {name} made; its orchestrator starts"));
    }

    /// Asks the person's `claude` for space `id`'s names from `description`, as `[spaces]
    /// naming` says, on the worker's thread for it. Whether it was asked: not with `local`, nor
    /// with no state directory or worker.
    fn ask_for_names(&mut self, id: &SpaceId, description: &str) -> bool {
        // One run at a time: each is a whole `claude`, and a space made meanwhile takes oso's
        // own names at once.
        if self.configuration.spaces_naming() != SpacesNaming::Claude
            || !self.hosting.spaces.asked_names.is_empty()
        {
            return false;
        }
        let (Some(state), Some(worker)) = (self.hosting.state_directory(), &self.sessions) else {
            return false;
        };
        let running = Running::default();
        let ask = Ask {
            description: description.to_owned(),
            terminal: self.terminal_settings(),
            claude: self.configuration.session_claude().map(Path::to_path_buf),
            session_env: self.configuration.session_env().to_vec(),
            state,
            space: id.as_str().to_owned(),
            running: Arc::clone(&running),
        };
        worker.send(Job::Space(SpaceJob::Summarize {
            id: id.clone(),
            ask: Box::new(ask),
        }));
        let fallback = (self.watch.spaces().find(id))
            .map(|at| self.space_label(at))
            .unwrap_or_default();
        self.hosting.spaces.asked_names.insert(id.clone(), fallback);
        self.hosting.spaces.summary_run = Some(running);
        true
    }

    /// The person's `claude` named space `id`, or said why not: the space takes the names that
    /// hold, its slug only while it has no directory and none is being made, its display name
    /// only if the person did not rename it meanwhile; then its directory is made and its
    /// orchestrator started, unless that is under way already.
    pub(in crate::app::sessions) fn space_named(
        &mut self,
        id: &SpaceId,
        named: Result<crate::sessions::spaces::summary::Named, String>,
    ) {
        let Some(fallback) = self.hosting.spaces.asked_names.remove(id) else {
            return;
        };
        self.hosting.spaces.summary_run = None;
        let Some(at) = self.watch.spaces().find(id) else {
            return;
        };
        let Some(stored) = self.watch.spaces().stored(at).filter(|s| !s.archived) else {
            return;
        };
        let started = stored.directory.is_some() || self.hosting.spaces.making.contains(id);
        let renamed = stored.label() != fallback;
        let why = named.as_ref().err().cloned();
        let named = named.unwrap_or_default();
        let slug = named.slug.as_deref().filter(|_| !started);
        let display = named.display.as_deref().filter(|_| !renamed);
        let taken = self.session_name_taken();
        let names = (self.watch.spaces_mut()).take_names(at, (slug, display), &taken, now_ms());
        let Some((slug, label)) = names else {
            return;
        };
        let said = match why {
            None => format!("Space {} ({slug}) named", printable(&label)),
            Some(why) => format!(
                "claude did not name the space ({why}): it is {} ({slug}); n in the space menu renames it",
                printable(&label)
            ),
        };
        if !started {
            self.make_space_directory(at, false, true);
        }
        self.editor.message(said);
        self.sessions_touched();
        self.request_redraw();
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
