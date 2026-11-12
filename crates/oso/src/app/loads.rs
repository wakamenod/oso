//! Loading files: starting a load, and taking in its snapshots as they arrive.
//!
//! Must not depend on: key presses or how a frame is drawn. The file is read on
//! the loader's threads, never here.

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::editor::Editor;
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
    /// What EditorConfig gave its file, until the buffer is shown with the tab width the
    /// display map was built with (ADR 0115).
    editorconfig: Option<loader::EditorConfigFound>,
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
        let visiting = Buffer::visiting(path.clone());
        // The mode's indentation, under which the loader reads EditorConfig before it builds
        // the display map; adding the buffer gives it the mode's indent size and style until
        // then (ADR 0115).
        let visit = loader::Visit {
            configured: self.editor.mode_indent(visiting.mode()),
            editorconfig: true,
            home: self.home.clone(),
        };
        let buffer = self.editor.add_buffer(visiting);
        self.buffers_changed();
        let waker = self.waker.clone();
        let started = Instant::now();
        let wake = move |buffer| waker.send(UserEvent::Load(buffer));
        let spawned = if for_agent {
            loader::spawn_one_name(buffer, path, limit, visit, wake)
        } else {
            loader::spawn(buffer, path, limit, visit, wake)
        };
        match spawned {
            Ok(handle) => {
                self.loads.push(Load {
                    handle,
                    started,
                    first_screen: None,
                    editorconfig: None,
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
        self.editor.request_project(buffer);
        // Its EditorConfig, as after `C-x C-w`: the tab width stays the default (ADR 0115).
        self.editor.request_editorconfig(buffer);
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
                    editorconfig: None,
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
        let wake = wake_for(load.is_some(), self.disk.busy(buffer));
        if wake.reload {
            // A file read again, which `reloads` runs.
            self.reload_updated(buffer);
        }
        if !wake.load {
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
                editorconfig: None,
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
        let id = self.loads[index].handle.buffer();
        let old_end = shown_end(&self.editor, id);
        let shown = old_end.is_some();
        if self.take_what_was_found(index, id) {
            return true;
        }
        let editorconfig = self.editorconfig_for(index, &update);
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
                old_end.is_some_and(|end| reaches_offset(&self.editor, id, end))
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
        self.give_editorconfig(id, editorconfig);
        if no_baseline {
            self.cannot_be_saved(id);
        }
        #[cfg(feature = "dev")]
        let redraw = self.try_restore_positions() || redraw;
        redraw
    }

    /// Says that `id` cannot be saved, as where its file leads could not be told.
    fn cannot_be_saved(&mut self, id: BufferId) {
        let name = self.editor.buffer(id).map_or("", Buffer::name);
        let message = format!(
            "This cannot be saved, as where its file leads could not be told as it was read: {name}"
        );
        self.editor.message(message);
    }

    /// What the update of the load at `index` gives its buffer of its indentation, if it shows
    /// the file with the tab width the display map was built with, or makes it a new file
    /// (ADR 0115).
    fn editorconfig_for(&mut self, index: usize, update: &LoadUpdate) -> Option<Indentation> {
        match update {
            LoadUpdate::Snapshot(_) => self.loads[index]
                .editorconfig
                .take()
                .map(Indentation::Shown),
            LoadUpdate::Failed(LoadError::NotFound) => {
                Some(Indentation::NewFile(self.loads[index].editorconfig.take()))
            }
            _ => None,
        }
    }

    /// Gives `id` its indentation, once the update that shows it is in.
    fn give_editorconfig(&mut self, id: BufferId, given: Option<Indentation>) {
        match given {
            Some(Indentation::Shown(found)) => self.editor.set_editorconfig(id, found.applied),
            Some(Indentation::NewFile(found)) => show_new_file(&mut self.editor, id, found),
            None => {}
        }
    }

    /// Takes in what the load at `index` found of `id`'s file before its content: the verdict,
    /// the baseline and the file's own spelling, and asks for its project, which a thread of its
    /// own finds while the text comes (ADR 0114). Returns whether the buffer went, as another
    /// one visits that very file.
    fn take_what_was_found(&mut self, index: usize, id: BufferId) -> bool {
        let handle = &self.loads[index].handle;
        let (resolved, baseline, spelling, editorconfig) = (
            handle.take_resolved(),
            handle.take_baseline(),
            handle.take_own_spelling(),
            handle.take_editorconfig(),
        );
        if let Some(found) = editorconfig {
            for note in &found.notes {
                self.editor.note(note.as_str());
            }
            self.loads[index].editorconfig = Some(found);
        }
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
        self.editor.request_project(id);
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

/// What a load's update gives its buffer of its indentation (ADR 0115).
enum Indentation {
    /// What EditorConfig gave the file, which is shown with its tab width now.
    Shown(loader::EditorConfigFound),
    /// A new file, with what EditorConfig gave where it will be, if it was read.
    NewFile(Option<loader::EditorConfigFound>),
}

/// Shows `id`, a new file with no text, with the tab width a visit builds its display map with,
/// EditorConfig's or its mode's configured one, and gives it what EditorConfig gave (ADR 0115).
/// An empty display map costs nothing to build.
fn show_new_file(editor: &mut Editor, id: BufferId, found: Option<loader::EditorConfigFound>) {
    let Some(buffer) = editor.buffer(id) else {
        return;
    };
    let configured = editor.mode_indent(buffer.mode());
    let tab_width = found
        .as_ref()
        .map_or(configured.tab_width(), |found| found.tab_width(configured));
    if let Some(buffer) = editor.buffer_mut(id)
        && buffer.text().is_empty()
        && buffer.tab_width() != tab_width
    {
        buffer.apply_load(oso_core::load::decode_all(b"", tab_width));
        editor.loaded(id);
    }
    let applied = found.and_then(|found| found.applied);
    editor.set_editorconfig(id, applied);
}

/// Which of a buffer's reads a wake from the loading threads may be for: its load, its read
/// again, or both. A load stays listed until a frame shows its file (`note_first_screens`),
/// while a read again can already have started, as a revert or a file changed on disk before
/// the window drew: its wake must reach it, or its text is never taken in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Wake {
    load: bool,
    reload: bool,
}

/// [`Wake`] for a buffer with a load listed or not, and a read again running or not.
fn wake_for(load_listed: bool, reload_running: bool) -> Wake {
    Wake {
        load: load_listed,
        // With no load listed, the read again is the only reader the wake can be from.
        reload: reload_running || !load_listed,
    }
}

/// The tiled windows that show `id`, also beneath a prompt, selected or not (ADR 0112).
fn windows_showing(editor: &Editor, id: BufferId) -> impl Iterator<Item = &window::Window> {
    editor
        .frame()
        .windows()
        .iter()
        .filter_map(|(window, _)| editor.window_by_id(*window))
        .filter(move |window| window.buffer() == id)
}

/// Whether a tiled window shows `id`, selected or not (ADR 0112).
pub(super) fn shown_in_a_window(editor: &Editor, id: BufferId) -> bool {
    windows_showing(editor, id).next().is_some()
}

/// Where the text of `id` ends, if a tiled window shows it: what a load's next part comes
/// after.
fn shown_end(editor: &Editor, id: BufferId) -> Option<usize> {
    windows_showing(editor, id).next()?;
    editor.buffer(id).map(|buffer| buffer.text().len())
}

/// Whether the view of a tiled window showing `id` reaches the view row that holds `offset`,
/// or those after it. Both are its rows as they are now: a load's part starts a wrap over, so
/// a row counted before it means nothing after (ADR 0103).
fn reaches_offset(editor: &Editor, id: BufferId, offset: usize) -> bool {
    let Some(buffer) = editor.buffer(id) else {
        return false;
    };
    let display = buffer.display();
    windows_showing(editor, id).any(|window| {
        let row = match window.wrap_of(&display) {
            Some(wrap) => wrap.row_of(&display, offset),
            None => display.offset_to_display_point(offset).row,
        };
        window::reaches(window.top_row(&display), window.size(), row)
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Wake, reaches_offset, show_new_file, shown_end, shown_in_a_window, wake_for,
        windows_showing,
    };
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::frame::{FrameSize, Way};

    #[test]
    fn a_buffer_in_a_window_not_selected_counts_as_shown_and_reached() {
        let lines: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let mut editor = Editor::with_buffer(Buffer::from_text("log", &lines));
        editor.set_frame_size(FrameSize {
            rows: 24,
            columns: 80,
        });
        let log = editor.window().buffer();
        editor.split_window(Way::Down, true).unwrap();
        let notes = editor.add_buffer(Buffer::from_text("notes", "agenda\n"));
        editor.show(notes);
        assert_ne!(
            editor.window().buffer(),
            log,
            "the log is in the other window"
        );
        assert_eq!(windows_showing(&editor, log).count(), 1);
        assert!(shown_in_a_window(&editor, log));
        assert_eq!(shown_end(&editor, log), Some(lines.len()));
        assert!(reaches_offset(&editor, log, 0), "its top is in view");
        assert!(!reaches_offset(&editor, log, lines.len()), "its end is not");
        let gone = editor.add_buffer(Buffer::from_text("gone", ""));
        assert_eq!(shown_end(&editor, gone), None, "shown nowhere");
    }

    #[test]
    fn a_reload_running_before_the_loads_first_frame_still_gets_its_wake() {
        let both = Wake {
            load: true,
            reload: true,
        };
        assert_eq!(wake_for(true, true), both);
    }

    #[test]
    fn a_wake_goes_to_whichever_read_there_is() {
        let load = Wake {
            load: true,
            reload: false,
        };
        let reload = Wake {
            load: false,
            reload: true,
        };
        assert_eq!(wake_for(true, false), load);
        assert_eq!(wake_for(false, true), reload);
        assert_eq!(
            wake_for(false, false),
            reload,
            "a read that has ended takes nothing"
        );
    }

    /// A new `/p/src/new.rs`, as a visit leaves it before its file is found missing.
    fn new_file() -> (Editor, oso_core::buffer::BufferId) {
        let buffer = Buffer::visiting(std::path::PathBuf::from("/p/src/new.rs"));
        let editor = Editor::with_buffer(buffer);
        let id = editor.window().buffer();
        (editor, id)
    }

    #[test]
    fn a_new_file_is_shown_with_editorconfigs_tab_width_and_indents_by_it() {
        use oso_core::editorconfig::{Applied, Budget, File};
        use oso_core::indent::{Origin, Style};

        let (mut editor, id) = new_file();
        let file = File::parse(
            "[*]\nindent_style = tab\nindent_size = 4\n",
            &mut Budget::default(),
        );
        let files = vec![(std::path::PathBuf::from("/p/.editorconfig"), file)];
        let applied = Applied::from_walk(std::path::Path::new("/p/src/new.rs"), files);
        let found = crate::loader::EditorConfigFound {
            applied: Some(applied),
            notes: Vec::new(),
        };
        show_new_file(&mut editor, id, Some(found));
        let buffer = editor.buffer(id).unwrap();
        assert_eq!(
            buffer.tab_width().get(),
            4,
            "indent_size = 4 makes the tab width 4"
        );
        let settings = buffer.indent_settings();
        assert_eq!((settings.style(), settings.indent_size()), (Style::Tabs, 4));
        assert_eq!(buffer.indent_origins().tab_width, Origin::EditorConfig);
        assert!(buffer.loading().is_none());
    }

    #[test]
    fn a_new_file_with_no_editorconfig_takes_the_configured_tab_width() {
        use oso_core::buffer::Mode;
        use oso_core::indent::{Origin, Origins, Settings, Style};

        let (mut editor, id) = new_file();
        let three = Settings::new(3, Style::Tabs, 3).unwrap();
        let origins = Origins {
            indent_size: Origin::Configuration,
            style: Origin::Configuration,
            tab_width: Origin::Configuration,
        };
        let modes = [Mode::Rust, Mode::Markdown, Mode::Toml, Mode::Fundamental];
        editor.set_mode_indents(modes.map(|mode| (mode, three, origins)));
        show_new_file(&mut editor, id, None);
        let buffer = editor.buffer(id).unwrap();
        assert_eq!(buffer.tab_width().get(), 3);
        assert_eq!(
            buffer.indent_origins(),
            origins,
            "nothing kept from the visit"
        );
    }
}
