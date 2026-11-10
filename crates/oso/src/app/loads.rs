//! Loading files: starting a load, and taking in its snapshots as they arrive.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read on
//! the loader's threads, never here.

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::window;

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
}

impl App {
    /// Visits `path` in the selected window: a buffer named after the file
    /// shows at once, read-only, and fills in as a background thread loads
    /// it. Nothing in the file is run (security.md, T1).
    pub fn open(&mut self, path: PathBuf) {
        if let Some(buffer) = self.visit(path, loader::UNASKED) {
            self.editor.show(buffer);
        }
        self.request_redraw();
    }

    /// Adds a buffer for `path` and starts loading it, if it holds at most `limit` bytes
    /// ([`loader::spawn`]). `None` if the loading thread cannot start.
    pub(super) fn visit(&mut self, path: PathBuf, limit: u64) -> Option<BufferId> {
        self.visit_with(path, limit, false)
    }

    /// [`Self::visit`], for the agent channel if `for_agent`: the file opened must have one
    /// name, or its load fails and the buffer goes, unshown (ADR 0085).
    pub(super) fn visit_with(
        &mut self,
        path: PathBuf,
        limit: u64,
        for_agent: bool,
    ) -> Option<BufferId> {
        // Absolute without touching the file system, so the UI never waits.
        let path = std::path::absolute(&path).unwrap_or(path);
        let buffer = self.editor.add_buffer(Buffer::visiting(path.clone()));
        self.buffers_changed();
        let waker = self.waker.clone();
        let started = Instant::now();
        let wake = move |buffer| waker.send(UserEvent::Load(buffer));
        let spawned = if for_agent {
            loader::spawn_one_name(buffer, path, limit, wake)
        } else {
            loader::spawn(buffer, path, limit, wake)
        };
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
                let name = self.editor.buffer(buffer).map_or("", Buffer::name);
                let message = format!("Cannot open: cannot start loading: {e}: {name:?}");
                self.editor.message(message);
                self.editor.kill_buffer(buffer);
                None
            }
        }
    }

    /// Stops loading `buffer`: its thread ends at its next read, and nothing it read comes in.
    pub(super) fn cancel_load(&mut self, buffer: BufferId) {
        self.loads.retain(|load| load.handle.buffer() != buffer);
    }

    /// Adds a buffer for `path` holding `text`, carried unsaved from another instance, and
    /// starts judging the path, which reads nothing of the file (ADR 0090). Until then it
    /// counts as a file not judged yet. `None`, and nothing added, if the thread cannot
    /// start.
    #[cfg(feature = "dev")]
    pub(super) fn visit_carried(
        &mut self,
        path: PathBuf,
        text: oso_core::load::LoadSnapshot,
        facts: oso_core::buffer::FileFacts,
    ) -> Option<BufferId> {
        let mut buffer = Buffer::visiting(path.clone());
        buffer.apply_carried(text, facts);
        let buffer = self.editor.add_buffer(buffer);
        self.buffers_changed();
        let waker = self.waker.clone();
        let started = Instant::now();
        match loader::spawn_judging(buffer, path, move |buffer| {
            waker.send(UserEvent::Load(buffer))
        }) {
            Ok(handle) => {
                self.loads.push(Load {
                    handle,
                    started,
                    first_screen: None,
                });
                Some(buffer)
            }
            Err(e) => {
                log::warn!("cannot start judging a carried buffer: {e}");
                self.editor.kill_buffer(buffer);
                None
            }
        }
    }

    /// Stops the loads of buffers that are gone, at once, and frees what killed buffers held
    /// on a thread of its own, as a large one's trees take tens of milliseconds a GB to free
    /// (ADR 0088).
    pub(super) fn forget_killed(&mut self) {
        let editor = &self.editor;
        self.loads
            .retain(|load| editor.buffer(load.handle.buffer()).is_some());
        let freed = self.editor.take_freed();
        if freed.is_empty() {
            return;
        }
        let spawned = std::thread::Builder::new()
            .name("oso-free".into())
            .spawn(move || drop(freed));
        if let Err(e) = spawned {
            // What it held was freed here, as the thread could not take it.
            log::warn!("could not free a killed buffer off the UI thread: {e}");
        }
    }

    /// A loading thread has an update for `buffer`: takes it in, and asks for a
    /// frame if it needs one.
    pub(super) fn load_updated(&mut self, buffer: BufferId) {
        let load = self.loads.iter().position(|l| l.handle.buffer() == buffer);
        if load.is_none() {
            // A file read again, which `reloads` runs.
            self.reload_updated(buffer);
            return;
        }
        if let Some(index) = load
            && let Some(update) = self.loads[index].handle.take()
        {
            let redraw = self.apply_load(index, update);
            self.forget_killed();
            #[cfg(feature = "dev")]
            self.reply_to_import();
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
        #[cfg(feature = "dev")]
        self.reply_to_import();
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
            }),
            Err(e) => {
                self.editor.message(format!("Cannot read the copy: {e}"));
                self.free_copy_buffer(buffer);
            }
        }
    }

    /// Where the text of `id` ends, if the tiled window, also beneath a prompt, shows it: what a
    /// load's next part comes after.
    fn shown_end(&self, id: BufferId) -> Option<usize> {
        let (window, buffer) = self.editor.tiled();
        (window.buffer() == id).then(|| buffer.text().len())
    }

    /// Whether the tiled window's view reaches the view row that holds `offset`, or those
    /// after it. Both are its rows as they are now: a load's part starts a wrap over, so a row
    /// counted before it means nothing after (ADR 0103).
    fn reaches_offset(&self, offset: usize) -> bool {
        let (window, buffer) = self.editor.tiled();
        let display = buffer.display();
        let row = match window.wrap() {
            Some(wrap) => wrap.row_of(&display, offset),
            None => display.offset_to_display_point(offset).row,
        };
        window::reaches(window.top_row(&display), window.size(), row)
    }

    /// Applies one update of `self.loads[index]`. Returns whether a frame
    /// is needed: only if the window shows the buffer and reaches the part
    /// that changed, or the buffer was replaced.
    fn apply_load(&mut self, index: usize, update: LoadUpdate) -> bool {
        let id = self.loads[index].handle.buffer();
        let old_end = self.shown_end(id);
        let shown = old_end.is_some();
        if self.take_what_was_found(index, id) {
            return true;
        }
        let Some(buffer) = self.editor.buffer_mut(id) else {
            self.loads.swap_remove(index);
            return false;
        };
        let mut no_baseline = false;
        let redraw = match update {
            LoadUpdate::Snapshot(snapshot) => {
                let first = buffer.text().is_empty();
                let done = snapshot.done();
                buffer.apply_load(*snapshot);
                let load = &mut self.loads[index];
                // How long a file that may hold secrets takes says something of its text, so
                // its load is neither logged nor timed (ADR 0078). It is judged by now.
                let timed = !buffer.is_secret();
                if first && timed {
                    load.first_screen = Some(load.started);
                    #[cfg(feature = "dev")]
                    self.timings.load_first_content(load.started);
                }
                if done {
                    no_baseline = buffer.baseline().is_none() && !buffer.is_internal();
                    self.scan_for_copies |= !buffer.is_internal();
                    if timed {
                        let (name, bytes) = (buffer.name(), buffer.text().len());
                        log::debug!(
                            "{name:?} loaded, {bytes} bytes, in {:?}",
                            load.started.elapsed()
                        );
                    }
                    #[cfg(feature = "dev")]
                    self.timings.load_done(load.started);
                }
                // The windows showing it wrap the text that came, if it wraps (ADR 0103).
                self.editor.loaded(id);
                old_end.is_some_and(|end| self.reaches_offset(end))
            }
            // The buffer holds carried text, and only its verdict was missing.
            #[cfg(feature = "dev")]
            LoadUpdate::Judged => {
                self.loads.swap_remove(index);
                shown
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
            // Only a reload sends one, and `reloads` takes it.
            LoadUpdate::Reloaded(_) => false,
            LoadUpdate::Failed(LoadError::TooLarge(bytes)) => {
                self.stopped_as_large(id, bytes);
                self.loads.swap_remove(index);
                shown
            }
            LoadUpdate::Failed(e) => {
                // The agent hears why its open stopped; the person's echo area stays as it was.
                if !self.agent_load_failed(id, &e) {
                    let name = self.editor.buffer(id).map_or("", Buffer::name);
                    self.editor.message(format!("Cannot open: {e}: {name:?}"));
                }
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

    /// Takes in what the load at `index` found of `id`'s file before its content: the verdict,
    /// the baseline and the file's own spelling. Returns whether the buffer went, as another one
    /// visits that very file.
    fn take_what_was_found(&mut self, index: usize, id: BufferId) -> bool {
        let handle = &self.loads[index].handle;
        let (resolved, baseline, spelling) = (
            handle.take_resolved(),
            handle.take_baseline(),
            handle.take_own_spelling(),
        );
        let Some(buffer) = self.editor.buffer_mut(id) else {
            return false;
        };
        if let Some(resolved) = resolved {
            resolved.judge(buffer);
        }
        if let Some(spelling) = spelling {
            buffer.set_own_spelling(spelling);
        }
        let Some(baseline) = baseline else {
            return false;
        };
        buffer.set_baseline(baseline);
        self.shows_the_one_open(index, id)
    }

    /// If another buffer visits the very file the load of `id` opened, by its device and inode,
    /// drops this one and shows that one, so one file is never in two buffers (ADR 0087).
    /// Returns whether it did.
    fn shows_the_one_open(&mut self, index: usize, id: BufferId) -> bool {
        let Some(other) = self.editor.visiting_same_file(id) else {
            return false;
        };
        let name = |editor: &oso_core::editor::Editor, id| {
            editor
                .buffer(id)
                .map_or_else(String::new, |buffer| buffer.name().to_owned())
        };
        let message = format!(
            "{} is already open as {}",
            name(&self.editor, id),
            name(&self.editor, other)
        );
        self.editor.show(other);
        self.editor.kill_buffer(id);
        self.editor.message(message);
        self.loads.swap_remove(index);
        true
    }

    /// A load of `id` stopped before reading, as its file holds `bytes`, 2 GiB or more: the
    /// buffer goes, and the next `C-x C-f` of that file asks (ADR 0087).
    fn stopped_as_large(&mut self, id: BufferId, bytes: u64) {
        let Some(buffer) = self.editor.buffer(id) else {
            return;
        };
        // The channel's `open` refuses it, as ADR 0087 words it, and leaves nothing of it with
        // the person: no message, and no question for their next `C-x C-f`.
        if buffer.agent_pending() {
            self.refuse_agent_open(id, super::agent::TOO_LARGE_FOR_THE_AGENT);
            self.editor.kill_buffer(id);
            return;
        }
        let path = buffer.path().map(std::path::Path::to_path_buf);
        let message = format!(
            "{} is {}; C-x C-f asks to open it",
            buffer.name(),
            loader::gib(bytes)
        );
        if let Some(path) = path {
            self.editor.remember_large(path, bytes);
        }
        self.editor.message(message);
        self.editor.kill_buffer(id);
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
            // A carried buffer has all its text, and waits only for its verdict.
            editor.buffer(load.handle.buffer()).is_some_and(|buffer| {
                buffer.loading().is_some() || buffer.verdict() == Verdict::Unjudged
            })
        });
    }
}
