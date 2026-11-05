//! Loading files: starting a load, and taking in its snapshots as they arrive.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read on
//! the loader's threads, never here.

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId};
use oso_core::window;

use super::{App, UserEvent};
use crate::loader::{self, LoadError, LoadHandle, LoadUpdate};

/// A file loading into a buffer.
pub(super) struct Load {
    /// Holds the latest update; cancels the load when dropped.
    handle: LoadHandle,
    started: Instant,
    /// When its first content arrived, until that frame is drawn.
    first_screen: Option<Instant>,
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
        let proxy = self.proxy.clone();
        let started = Instant::now();
        let spawned = loader::spawn(buffer, path, move |buffer| {
            proxy.send_event(UserEvent::Load(buffer)).is_ok()
        });
        match spawned {
            Ok(handle) => {
                self.loads.push(Load {
                    handle,
                    started,
                    first_screen: None,
                });
                Some(buffer)
            }
            Err(e) => {
                log::warn!("cannot start loading: {e}");
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
            && self.apply_load(index, update)
        {
            self.request_redraw();
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

    /// Applies one update of `self.loads[index]`. Returns whether a frame
    /// is needed: only if the window shows the buffer and reaches the part
    /// that changed, or the buffer was replaced.
    fn apply_load(&mut self, index: usize, update: LoadUpdate) -> bool {
        let id = self.loads[index].handle.buffer();
        let shown = self.editor.window().buffer() == id;
        let Some(buffer) = self.editor.buffer_mut(id) else {
            self.loads.swap_remove(index);
            return false;
        };
        if let Some(resolved) = self.loads[index].handle.take_resolved() {
            buffer.judge_secret(resolved.paths());
        }
        if let Some(baseline) = self.loads[index].handle.take_baseline() {
            buffer.set_baseline(baseline);
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
                    no_baseline = buffer.baseline().is_none();
                    log::debug!("{:?} loaded in {:?}", buffer.name(), load.started.elapsed());
                    #[cfg(feature = "dev")]
                    self.timings.load_done(load.started);
                }
                let window = self.editor.window();
                shown && window::reaches(window.scroll().top, window.size(), old_rows - 1)
            }
            LoadUpdate::Failed(LoadError::NotFound) => {
                // As Emacs does: a new file, empty until saved.
                log::info!("(New file) {:?}", buffer.name());
                no_baseline = buffer.baseline().is_none();
                buffer.finish_empty();
                self.loads.swap_remove(index);
                shown
            }
            LoadUpdate::Failed(e) => {
                let message = format!("cannot open {:?}: {e}", buffer.name());
                self.editor.message(message);
                self.editor.kill_buffer(id);
                self.loads.swap_remove(index);
                shown
            }
        };
        if no_baseline {
            self.editor.message(
                "Where this file leads could not be told as it was read, so it cannot be saved",
            );
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
            editor
                .buffer(load.handle.buffer())
                .is_some_and(|buffer| buffer.loading().is_some())
        });
    }
}
