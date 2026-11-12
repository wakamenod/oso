//! Files changed on disk (ADR 0029, ADR 0083): handing what the watcher saw to the editor,
//! which decides what each buffer needs, and running the reads it asks for.
//!
//! Must not depend on: key presses or how a frame is drawn. Files are looked at and read on
//! the loader's threads, never here, and what a look means is the editor's to decide.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use oso_core::buffer::{Baseline, Buffer, BufferId, ContentId, DiskState};
use oso_core::editor::ReloadOutcome;

use super::{App, UserEvent};
use crate::loader::{self, LoadError, LoadHandle, LoadUpdate, Resolved, Watcher};

/// What the app knows of open files on disk.
#[derive(Default)]
pub(super) struct Disk {
    /// The watcher's thread, or none if it could not start.
    watcher: Option<Watcher>,
    /// The paths it was last given, by buffer.
    files: Vec<(BufferId, PathBuf)>,
    /// What it last saw of them, sorted by buffer.
    seen: Vec<(BufferId, DiskState)>,
    /// The files being read again. A read whose buffer has changed since runs to its end, and
    /// its text is refused then: so a read that hangs, as on a mount gone away, keeps its
    /// buffer from starting another.
    running: Vec<Running>,
    /// The names of buffers reloaded since that was last said.
    reloaded: Vec<String>,
    /// A report from a look older than this was looked at before a save ended, and says
    /// nothing true of that save's file.
    min_generation: u64,
    /// A report came since the editor last saw one.
    fresh: bool,
    /// The buffers or their paths may have changed since the watcher was given them.
    files_changed: bool,
}

/// How long a read may run before the message about the reloads done waits no longer for it:
/// one that hangs, as on a mount gone away, must not hold back the rest.
const READ_PATIENCE: Duration = Duration::from_secs(1);

impl Disk {
    /// Starts the watcher, which reports through `waker`.
    pub(super) fn start(waker: super::Waker) -> Self {
        let watcher = Watcher::spawn(move || waker.send(UserEvent::Watch));
        if let Err(e) = &watcher {
            log::warn!("cannot watch files for changes on disk: {e}");
        }
        Self {
            watcher: watcher.ok(),
            files_changed: true,
            ..Self::default()
        }
    }

    /// Whether a read of `buffer`'s file again is running.
    pub(super) fn busy(&self, buffer: BufferId) -> bool {
        self.running
            .iter()
            .any(|running| running.handle.buffer() == buffer)
    }
}

/// A file being read again.
struct Running {
    /// Holds the latest update; cancels the read when dropped.
    handle: LoadHandle,
    /// The state of the buffer the read compares with.
    read_from: ContentId,
    /// What the read opened, which becomes the buffer's only once its text is in.
    baseline: Option<Baseline>,
    spelling: Option<PathBuf>,
    /// A person asked for it, over unsaved edits too (`revert-buffer`).
    asked: bool,
    started: Instant,
}

impl App {
    /// The watcher saw something new.
    pub(super) fn watch_reported(&mut self) {
        let Some((generation, mut seen)) = self.disk.watcher.as_ref().and_then(Watcher::take)
        else {
            return;
        };
        if generation < self.disk.min_generation {
            return;
        }
        seen.sort_unstable_by_key(|(buffer, _)| *buffer);
        self.disk.seen = seen;
        self.disk.fresh = true;
        self.disk.files_changed = true;
    }

    /// A buffer may have been added, or its path changed, so the watcher's paths are checked
    /// at the next look.
    pub(super) fn buffers_changed(&mut self) {
        self.disk.files_changed = true;
    }

    /// The window has gained the focus: the files are looked at now, as another program may
    /// have changed them meanwhile.
    pub(super) fn window_focused(&mut self) {
        self.disk.files_changed = true;
        if let Some(watcher) = &self.disk.watcher {
            // Reported whatever it finds, so that a read that failed is tried again.
            watcher.look_again();
        }
    }

    /// A save of `id` has ended: what was seen of its file before is no longer news, as the
    /// save may have written it, and the file is looked at again and reported whatever it is.
    pub(super) fn look_again_after_save(&mut self, id: BufferId) {
        self.disk.seen.retain(|(buffer, _)| *buffer != id);
        self.disk.files_changed = true;
        if let Some(watcher) = &self.disk.watcher {
            self.disk.min_generation = watcher.look_again();
        }
    }

    /// Hands what the watcher last saw to the editor and starts the reads it asks for, and
    /// those `revert-buffer` asked for. Before the loop waits, so after every batch of events.
    /// Nothing is allocated and no file is touched while nothing changes, and the paths are
    /// compared only after a report, a save or a new buffer.
    pub(super) fn check_disk(&mut self) {
        if std::mem::take(&mut self.disk.files_changed) {
            self.sync_watched_files();
        }
        let fresh = std::mem::take(&mut self.disk.fresh);
        let disk = &self.disk;
        let reads = self
            .editor
            .files_seen(&disk.seen, fresh, |id| disk.busy(id));
        for id in reads {
            self.start_reload(id, false);
        }
        self.start_reverts();
        self.tell_reloaded();
    }

    /// Gives the watcher the paths of the buffers visiting files, when they changed.
    fn sync_watched_files(&mut self) {
        let mut now = self
            .editor
            .buffers()
            .filter_map(|(id, buffer)| buffer.path().map(|path| (id, path)));
        let mut known = self.disk.files.iter();
        let same = loop {
            match (now.next(), known.next()) {
                (None, None) => break true,
                (Some((id, path)), Some((known_id, known_path)))
                    if id == *known_id && path == known_path => {}
                _ => break false,
            }
        };
        if same {
            return;
        }
        self.disk.files = self
            .editor
            .buffers()
            .filter_map(|(id, buffer)| buffer.path().map(|path| (id, path.to_path_buf())))
            .collect();
        let files = &self.disk.files;
        self.disk
            .seen
            .retain(|(id, _)| files.iter().any(|(file, _)| file == id));
        if let Some(watcher) = &self.disk.watcher {
            watcher.watch(self.disk.files.clone());
        }
    }

    /// Starts reading again the files of the buffers `revert-buffer` asked for, once nothing
    /// makes them wait. One whose file is being read already takes that read, so a read that
    /// hangs, as on a mount gone away, is never joined by another.
    pub(super) fn start_reverts(&mut self) {
        for (id, content) in self.editor.take_revert_requests() {
            let running = self
                .disk
                .running
                .iter_mut()
                .find(|running| running.handle.buffer() == id);
            match running {
                Some(running) if running.read_from == content => running.asked = true,
                Some(_) => {
                    self.editor
                        .message("Not reloaded, as a read of the file is still running");
                }
                None => self.start_reload(id, true),
            }
        }
    }

    /// Starts reading the file of `id` again, as a person `asked` or as it changed.
    fn start_reload(&mut self, id: BufferId, asked: bool) {
        let Some(buffer) = self.editor.buffer(id) else {
            return;
        };
        let Some(path) = buffer.path().map(PathBuf::from) else {
            return;
        };
        let read_from = buffer.content_id();
        let old = buffer.text().clone();
        let for_agent = buffer.opened_by_agent();
        // Read again with the tab width it has, which a reload keeps (ADR 0115).
        let tab_width = buffer.tab_width();
        let waker = self.waker.clone();
        let spawned = loader::spawn_reload(id, path, old, for_agent, tab_width, move |buffer| {
            waker.send(UserEvent::Load(buffer))
        });
        match spawned {
            Ok(handle) => self.disk.running.push(Running {
                handle,
                read_from,
                baseline: None,
                spelling: None,
                asked,
                started: Instant::now(),
            }),
            Err(e) => {
                let name = buffer.name();
                let message = format!("Cannot reload: cannot start reading: {e}: {name}");
                self.editor.message(message);
            }
        }
    }

    /// A thread reading `id`'s file again has an update: takes it in, and asks for a frame if
    /// the window shows the buffer and its text changed.
    pub(super) fn reload_updated(&mut self, id: BufferId) {
        let Some(index) = self
            .disk
            .running
            .iter()
            .position(|running| running.handle.buffer() == id)
        else {
            return;
        };
        // The verdict comes first, before any text.
        if let Some(resolved) = self.disk.running[index].handle.take_resolved() {
            match &resolved {
                Resolved::Paths(paths) => self.editor.judge_reload(id, Ok(paths)),
                Resolved::Unknown(seen) => self.editor.judge_reload(id, Err(seen)),
            }
        }
        let running = &mut self.disk.running[index];
        if let Some(baseline) = running.handle.take_baseline() {
            running.baseline = Some(baseline);
        }
        if let Some(spelling) = running.handle.take_own_spelling() {
            running.spelling = Some(spelling);
        }
        let Some(update) = running.handle.take() else {
            return;
        };
        let running = self.disk.running.swap_remove(index);
        let redraw = self.apply_reload(id, running, update);
        self.dispatcher.publish_for_emergency(&self.editor);
        if redraw {
            self.request_redraw();
        }
    }

    /// Takes in what reading `id`'s file again ended with. Returns whether a frame is needed.
    fn apply_reload(&mut self, id: BufferId, running: Running, update: LoadUpdate) -> bool {
        let shown = super::loads::shown_in_a_window(&self.editor, id);
        let name = self.editor.buffer(id).map_or("", Buffer::name).to_owned();
        let Running {
            handle,
            read_from,
            baseline,
            spelling,
            asked,
            started: _,
        } = running;
        drop(handle);
        let reloaded = match update {
            LoadUpdate::Reloaded(reloaded) => reloaded,
            // Gone between the look and the read: the next look says so, unless a person asked.
            LoadUpdate::Failed(e @ (LoadError::NotFound | LoadError::NotAFile)) => {
                if asked {
                    self.editor.message(format!("Cannot reload: {e}: {name}"));
                }
                return false;
            }
            LoadUpdate::Failed(e) => {
                self.editor.message(format!("Cannot reload: {e}: {name}"));
                self.editor.reload_failed(id);
                return false;
            }
            // Only a carried buffer's path is judged alone (ADR 0090).
            #[cfg(feature = "dev")]
            LoadUpdate::Judged => return false,
            // A reload sends its text whole.
            LoadUpdate::Snapshot(snapshot) => {
                self.freer.free(snapshot);
                return false;
            }
        };
        // Where the opened file is could not be told, so its text would pair with a place a
        // save could not trust. It stays out, as a first load's would be refused a save.
        let Some(baseline) = baseline else {
            self.freer.free(reloaded);
            let message = format!(
                "Not reloaded, as where its file leads could not be told as it was read: {name}"
            );
            self.editor.message(message);
            self.editor.reload_failed(id);
            return false;
        };
        let (outcome, leftover) = if asked {
            self.editor.revert(id, *reloaded, read_from)
        } else {
            self.editor.reload(id, *reloaded, read_from)
        };
        self.freer.free(leftover);
        match outcome {
            ReloadOutcome::Reloaded | ReloadOutcome::Same => {
                if let Some(buffer) = self.editor.buffer_mut(id) {
                    // A file found elsewhere now, as through a link pointed anew, is no longer
                    // the one the person visited: the agent's save of it asks (ADR 0085).
                    let moved = buffer
                        .baseline()
                        .is_some_and(|old| old.location != baseline.location);
                    if moved {
                        buffer.mark_opened_by_agent();
                    }
                    buffer.set_baseline(baseline);
                    if let Some(spelling) = spelling {
                        buffer.set_own_spelling(spelling);
                    }
                }
                // One asked for is said even if nothing changed.
                if outcome == ReloadOutcome::Reloaded || asked {
                    self.disk.reloaded.push(name);
                }
                shown && outcome == ReloadOutcome::Reloaded
            }
            ReloadOutcome::NotEditable => {
                self.editor.message(format!(
                    "Not reloaded, as the file of {name} is now binary or not valid UTF-8, and \
                     your edits would be lost"
                ));
                false
            }
            ReloadOutcome::Edited if asked => {
                self.editor
                    .message(format!("Not reloaded, as {name} changed meanwhile"));
                false
            }
            // The editor decides again at the next look, and a revert that must wait is asked
            // for again.
            ReloadOutcome::Edited | ReloadOutcome::Later | ReloadOutcome::NoSuchBuffer => false,
        }
    }

    /// When the message that names the buffers reloaded is due, if it is held for a read
    /// still running: once the last of them to start has run its patience out. One already
    /// past wakes the loop once, and [`App::tell_reloaded`] then says it, so it cannot spin.
    pub(super) fn reload_deadline(&self) -> Option<Instant> {
        if self.disk.reloaded.is_empty() {
            return None;
        }
        self.disk
            .running
            .iter()
            .map(|running| running.started + READ_PATIENCE)
            .max()
    }

    /// Says in one message which buffers were reloaded, once no other read has started in
    /// the last [`READ_PATIENCE`], so that a checkout that changed many files says so once, and
    /// a read that hangs holds back nothing.
    fn tell_reloaded(&mut self) {
        let waiting = |running: &Running| running.started.elapsed() < READ_PATIENCE;
        if self.disk.reloaded.is_empty() || self.disk.running.iter().any(waiting) {
            return;
        }
        let names = std::mem::take(&mut self.disk.reloaded);
        let message = match &names[..] {
            [name] => format!("Reloaded {name}"),
            names => format!("Reloaded {} files: {}", names.len(), names.join(", ")),
        };
        self.editor.message(message);
    }
}
