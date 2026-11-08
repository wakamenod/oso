//! Loading files: starting a load, and taking in its snapshots as they arrive.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read on
//! the loader's threads, never here.

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::window;

use super::reloads::Reload;
use super::{App, UserEvent};
use crate::emergency;
use crate::loader::{self, LoadError, LoadHandle, LoadUpdate};

/// A file loading into a buffer.
pub(super) struct Load {
    /// Holds the latest update; cancels the load when dropped.
    handle: LoadHandle,
    started: Instant,
    /// When its first content arrived, until that frame is drawn.
    first_screen: Option<Instant>,
    /// It reads a file again into a buffer that has its text (ADR 0083).
    reload: Option<Reload>,
}

impl Load {
    /// A load that reads a file again, as `reload` says.
    pub(super) fn reloading(handle: LoadHandle, started: Instant, reload: Reload) -> Self {
        Self {
            handle,
            started,
            first_screen: None,
            reload: Some(reload),
        }
    }

    pub(super) fn buffer(&self) -> BufferId {
        self.handle.buffer()
    }

    pub(super) fn handle(&self) -> &LoadHandle {
        &self.handle
    }

    /// The state of the buffer a reload compares with, if this is one.
    pub(super) fn reload_from(&self) -> Option<oso_core::buffer::ContentId> {
        self.reload.as_ref().map(Reload::from)
    }

    pub(super) fn reload_mut(&mut self) -> Option<&mut Reload> {
        self.reload.as_mut()
    }

    pub(super) fn into_reload(self) -> Option<Reload> {
        self.reload
    }
}

impl App {
    /// Visits `path` in the selected window: a buffer named after the file
    /// shows at once, read-only, and fills in as a background thread loads
    /// it. Nothing in the file is run (security.md, T1).
    pub fn open(&mut self, path: PathBuf) {
        if let Some(buffer) = self.visit(path) {
            self.editor.show(buffer);
        }
        self.request_redraw();
    }

    /// Adds a buffer for `path` and starts loading it. `None` if the loading
    /// thread cannot start.
    pub(super) fn visit(&mut self, path: PathBuf) -> Option<BufferId> {
        // Absolute without touching the file system, so the UI never waits.
        let path = std::path::absolute(&path).unwrap_or(path);
        let buffer = self.editor.add_buffer(Buffer::visiting(path.clone()));
        let waker = self.waker.clone();
        let started = Instant::now();
        let spawned = loader::spawn(buffer, path, move |buffer| {
            waker.send(UserEvent::Load(buffer))
        });
        match spawned {
            Ok(handle) => {
                self.loads.push(Load {
                    handle,
                    started,
                    first_screen: None,
                    reload: None,
                });
                Some(buffer)
            }
            Err(e) => {
                let name = self.editor.buffer(buffer).map_or("", Buffer::name);
                let message = format!("Cannot open: cannot start loading: {e}: {name:?}");
                self.editor.message(message);
                self.editor.kill_buffer(buffer);
                None
            }
        }
    }

    /// A loading thread has an update for `buffer`: takes it in, and asks for a
    /// frame if it needs one.
    pub(super) fn load_updated(&mut self, buffer: BufferId) {
        let load = self.loads.iter().position(|l| l.handle.buffer() == buffer);
        if let Some(index) = load
            && let Some(update) = self.loads[index].handle.take()
        {
            let redraw = self.apply_load(index, update);
            // Not in `apply_load`, as `poll_loads` runs on the frame path. A buffer still
            // loading is not at risk, so nothing is lost by waiting for this event.
            self.dispatcher.publish_for_emergency(&self.editor);
            if redraw {
                self.request_redraw();
            }
        }
    }

    /// Takes in the latest update of every load, before a frame, so that
    /// the frame shows what has loaded by then.
    pub(super) fn poll_loads(&mut self) {
        for index in (0..self.loads.len()).rev() {
            if let Some(update) = self.loads[index].handle.take() {
                self.apply_load(index, update);
            }
        }
    }

    /// Starts filling an internal buffer with a copy's text.
    pub(super) fn load_copy(&mut self, load: emergency::CopyLoad) {
        let emergency::CopyLoad {
            buffer,
            path,
            limit,
        } = load;
        let waker = self.waker.clone();
        let started = Instant::now();
        let open = move || emergency::open_copy(&path);
        let spawned = loader::spawn_copy(buffer, open, limit, move |buffer| {
            waker.send(UserEvent::Load(buffer))
        });
        match spawned {
            Ok(handle) => self.loads.push(Load {
                handle,
                started,
                first_screen: None,
                reload: None,
            }),
            Err(e) => {
                self.editor.message(format!("Cannot read the copy: {e}"));
                self.free_copy_buffer(buffer);
            }
        }
    }

    /// Applies one update of `self.loads[index]`. Returns whether a frame
    /// is needed: only if the window shows the buffer and reaches the part
    /// that changed, or the buffer was replaced.
    fn apply_load(&mut self, index: usize, update: LoadUpdate) -> bool {
        if self.loads[index].reload.is_some() {
            return self.apply_reload(index, update);
        }
        let id = self.loads[index].handle.buffer();
        // The tiled window, also beneath a prompt.
        let shown = self.editor.tiled().0.buffer() == id;
        let Some(buffer) = self.editor.buffer_mut(id) else {
            self.loads.swap_remove(index);
            return false;
        };
        if let Some(resolved) = self.loads[index].handle.take_resolved() {
            resolved.judge(buffer);
        }
        if let Some(baseline) = self.loads[index].handle.take_baseline() {
            buffer.set_baseline(baseline);
        }
        if let Some(spelling) = self.loads[index].handle.take_own_spelling() {
            buffer.set_own_spelling(spelling);
        }
        let mut no_baseline = false;
        let redraw = match update {
            LoadUpdate::Snapshot(snapshot) => {
                let old_rows = buffer.display().row_count();
                let first = buffer.text().is_empty();
                let done = snapshot.done();
                buffer.apply_load(*snapshot);
                let load = &mut self.loads[index];
                if first {
                    load.first_screen = Some(load.started);
                    #[cfg(feature = "dev")]
                    self.timings.load_first_content(load.started);
                }
                if done {
                    no_baseline = buffer.baseline().is_none() && !buffer.is_internal();
                    self.scan_for_copies |= !buffer.is_internal();
                    log::debug!("{:?} loaded in {:?}", buffer.name(), load.started.elapsed());
                    #[cfg(feature = "dev")]
                    self.timings.load_done(load.started);
                }
                let window = self.editor.tiled().0;
                shown && window::reaches(window.scroll().top, window.size(), old_rows - 1)
            }
            LoadUpdate::Failed(LoadError::NotFound) => {
                // As Emacs does: a new file, empty until saved.
                log::info!("(New file) {:?}", buffer.name());
                no_baseline = buffer.baseline().is_none();
                buffer.finish_empty();
                self.scan_for_copies = true;
                self.loads.swap_remove(index);
                shown
            }
            // Only a reload sends one.
            LoadUpdate::Reloaded(_) => false,
            LoadUpdate::Failed(e) => {
                let message = format!("Cannot open: {e}: {:?}", buffer.name());
                self.editor.message(message);
                self.editor.kill_buffer(id);
                // What a notice waited on is over.
                self.scan_for_copies = true;
                // An internal buffer stays through that.
                self.free_copy_buffer(id);
                self.loads.swap_remove(index);
                shown
            }
        };
        if no_baseline {
            let name = self.editor.buffer(id).map_or("", Buffer::name);
            let message = format!(
                "This cannot be saved, as where its file leads could not be told as it was read: {name}"
            );
            self.editor.message(message);
        }
        #[cfg(feature = "dev")]
        let redraw = self.try_restore_positions() || redraw;
        redraw
    }

    /// Logs how long the first screen of each new file took, once drawn,
    /// and forgets loads that finished.
    pub(super) fn note_first_screens(&mut self) {
        let editor = &self.editor;
        self.loads.retain_mut(|load| {
            if let Some(started) = load.first_screen.take() {
                log::debug!(
                    "first frame of the file drawn {:?} after loading started",
                    started.elapsed()
                );
            }
            // A reload ends where its text is taken in.
            load.reload.is_some()
                || editor
                    .buffer(load.handle.buffer())
                    .is_some_and(|buffer| buffer.loading().is_some())
        });
    }
}
