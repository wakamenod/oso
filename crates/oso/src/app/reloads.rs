//! Files changed on disk (ADR 0029, ADR 0083): comparing what the watcher saw with what each
//! buffer read, reading a file again into a buffer with no unsaved edits, and telling about a
//! file that changed under edits or went away.
//!
//! Must not depend on: key presses or how a frame is drawn. Files are looked at and read on
//! the loader's threads, never here; this compares and decides.

use std::path::PathBuf;
use std::time::Instant;

use oso_core::buffer::{Baseline, Buffer, BufferId, ContentId, FileStamp};
use oso_core::editor::ReloadOutcome;

use super::{App, UserEvent};
use crate::loader::{self, LoadError, LoadUpdate, Resolved, Seen, Watcher};

/// What the app knows of open files on disk.
#[derive(Default)]
pub(super) struct Disk {
    /// The watcher's thread, or none if it could not start.
    watcher: Option<Watcher>,
    /// The paths it was last given, by buffer.
    files: Vec<(BufferId, PathBuf)>,
    /// What it last saw of them.
    seen: Vec<(BufferId, Seen)>,
    /// What was done about each file, by buffer.
    watched: Vec<Watched>,
    /// The names of buffers reloaded since that was last said.
    reloaded: Vec<String>,
}

impl Disk {
    /// Starts the watcher, which reports through `waker`.
    pub(super) fn start(waker: super::Waker) -> Self {
        let watcher = Watcher::spawn(move || waker.send(UserEvent::Watch));
        if let Err(e) = &watcher {
            log::warn!("cannot watch files for changes on disk: {e}");
        }
        Self {
            watcher: watcher.ok(),
            ..Self::default()
        }
    }

    fn watched(&mut self, buffer: BufferId) -> &mut Watched {
        let index = match self.watched.iter().position(|w| w.buffer == buffer) {
            Some(index) => index,
            None => {
                self.watched.push(Watched {
                    buffer,
                    acted: None,
                    identity: None,
                });
                self.watched.len() - 1
            }
        };
        &mut self.watched[index]
    }
}

/// What was done about one buffer's file.
struct Watched {
    buffer: BufferId,
    /// What was last seen of the file that differed from what the buffer read, and what was
    /// done then, so that it is done once.
    acted: Option<(Seen, Act)>,
    /// The device and inode of the file as the buffer last read or wrote it, learnt at the first
    /// look that matched that stamp, so that a file swapped for another of the same time and
    /// size still counts as changed.
    identity: Option<(FileStamp, (u64, u64))>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Act {
    /// The file is being read again.
    Reload,
    /// The person was told, as the buffer has edits or the file is gone.
    Told,
}

/// What a reload running in a load keeps until its text arrives.
pub(super) struct Reload {
    /// The state of the buffer the read compares with.
    from: ContentId,
    /// What the read opened, which becomes the buffer's only once its text is in.
    baseline: Option<Baseline>,
    spelling: Option<PathBuf>,
    /// A person asked for it, over unsaved edits too (`revert-buffer`).
    asked: bool,
}

impl Reload {
    pub(super) fn from(&self) -> ContentId {
        self.from
    }
}

/// Whether `seen` is the file a buffer read or wrote, as `read` says, learning its identity
/// in `identity` at the first look.
fn is_as_read(
    seen: Seen,
    read: Option<FileStamp>,
    identity: &mut Option<(FileStamp, (u64, u64))>,
) -> bool {
    match (seen, read) {
        (Seen::Gone, None) => true,
        (
            Seen::File {
                stamp,
                identity: id,
            },
            Some(read),
        ) if stamp.same_content(read) => match (id, *identity) {
            (None, _) => true,
            (Some(id), Some((known_for, known))) if known_for == read => id == known,
            (Some(id), _) => {
                *identity = Some((read, id));
                true
            }
        },
        _ => false,
    }
}

impl App {
    /// The watcher saw something new.
    pub(super) fn watch_reported(&mut self) {
        if let Some(seen) = self.disk.watcher.as_ref().and_then(Watcher::take) {
            self.disk.seen = seen;
        }
    }

    /// The window has gained the focus: the files are looked at now, as another program may
    /// have changed them meanwhile.
    pub(super) fn window_focused(&self) {
        if let Some(watcher) = &self.disk.watcher {
            watcher.look_now();
        }
    }

    /// A save of `id` has ended: what was seen of its file before is no longer news, as the
    /// save may have written it, and the file is looked at again.
    pub(super) fn look_again_after_save(&mut self, id: BufferId) {
        self.disk.seen.retain(|(buffer, _)| *buffer != id);
        self.window_focused();
    }

    /// Compares what the watcher last saw with what each buffer read, and acts on what
    /// changed. Before the loop waits, so after every batch of events. O(buffers), with no
    /// I/O and nothing allocated while nothing changes.
    pub(super) fn check_disk(&mut self) {
        self.sync_watched_files();
        self.drop_stale_reloads();
        if !self.editor.reload_waits() {
            for index in 0..self.disk.seen.len() {
                let (buffer, seen) = self.disk.seen[index];
                self.check_file(buffer, seen);
            }
        }
        self.tell_reloaded();
    }

    /// Gives the watcher the paths of the buffers visiting files, when they changed.
    fn sync_watched_files(&mut self) {
        let same = {
            let mut now = self
                .editor
                .buffers()
                .filter_map(|(id, buffer)| buffer.path().map(|path| (id, path)));
            let mut known = self.disk.files.iter();
            loop {
                match (now.next(), known.next()) {
                    (None, None) => break true,
                    (Some((id, path)), Some((known_id, known_path)))
                        if id == *known_id && path == known_path => {}
                    _ => break false,
                }
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
            .watched
            .retain(|watched| files.iter().any(|(id, _)| *id == watched.buffer));
        self.disk
            .seen
            .retain(|(id, _)| files.iter().any(|(file, _)| file == id));
        if let Some(watcher) = &self.disk.watcher {
            watcher.watch(self.disk.files.clone());
        }
    }

    /// Stops reading a file again into a buffer that has changed since, as its text could not
    /// go in. What the buffer then needs is decided again.
    fn drop_stale_reloads(&mut self) {
        let editor = &self.editor;
        let mut dropped = Vec::new();
        self.loads.retain(|load| {
            let Some(from) = load.reload_from() else {
                return true;
            };
            let id = load.buffer();
            let keep = editor
                .buffer(id)
                .is_some_and(|buffer| buffer.content_id() == from);
            if !keep {
                dropped.push(id);
            }
            keep
        });
        for id in dropped {
            self.disk.watched(id).acted = None;
        }
    }

    /// Acts on what the watcher saw of the file of `id`, once for each thing seen.
    fn check_file(&mut self, id: BufferId, seen: Seen) {
        let Some(buffer) = self.editor.buffer(id) else {
            return;
        };
        let reloading = self.loads.iter().any(|load| load.buffer() == id);
        if buffer.loading().is_some() || buffer.is_saving() || reloading {
            return;
        }
        // A buffer whose load could not tell where its file is has nothing to compare with.
        let Some(baseline) = buffer.baseline() else {
            return;
        };
        let read = baseline.file;
        let modified = buffer.is_modified();
        let watched = self.disk.watched(id);
        if is_as_read(seen, read, &mut watched.identity) {
            watched.acted = None;
            self.take_mode(id, seen);
            return;
        }
        let act = match seen {
            Seen::Unknown => return,
            Seen::File { .. } if !modified => Act::Reload,
            _ => Act::Told,
        };
        if watched.acted == Some((seen, act)) {
            return;
        }
        watched.acted = Some((seen, act));
        let name = self.editor.buffer(id).map_or("", Buffer::name).to_owned();
        match seen {
            Seen::File { .. } if act == Act::Reload => self.start_reload(id, false),
            Seen::File { stamp, .. } => {
                self.editor.file_changed_on_disk(id);
                // A save asks before it goes over this file, and replaces only this one.
                self.editor.may_save_over(id, stamp);
                self.editor
                    .message(format!("{name} changed on disk; your edits are kept"));
            }
            _ => {
                self.editor.file_gone_from_disk(id);
                self.editor
                    .message(format!("{name} is gone from disk; its text is kept"));
            }
        }
    }

    /// Takes the mode of a file whose text is as read, as `chmod` changes no text.
    fn take_mode(&mut self, id: BufferId, seen: Seen) {
        let Seen::File { stamp, .. } = seen else {
            return;
        };
        if let Some(buffer) = self.editor.buffer_mut(id)
            && let Some(baseline) = buffer.baseline()
            && baseline.file.is_some_and(|read| read.mode != stamp.mode)
        {
            let location = baseline.location.clone();
            let file = baseline.file.map(|read| FileStamp {
                mode: stamp.mode,
                ..read
            });
            buffer.set_baseline(Baseline { location, file });
        }
    }

    /// Starts reading again the files of the buffers `revert-buffer` asked for, after a
    /// command. A read already running for one starts over.
    pub(super) fn start_reverts(&mut self) {
        for id in self.editor.take_revert_requests() {
            self.loads
                .retain(|load| load.buffer() != id || load.reload_from().is_none());
            self.start_reload(id, true);
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
        let from = buffer.content_id();
        let old = buffer.text().clone();
        let waker = self.waker.clone();
        let spawned = loader::spawn_reload(id, path, old, move |buffer| {
            waker.send(UserEvent::Load(buffer))
        });
        match spawned {
            Ok(handle) => self.loads.push(super::loads::Load::reloading(
                handle,
                Instant::now(),
                Reload {
                    from,
                    baseline: None,
                    spelling: None,
                    asked,
                },
            )),
            Err(e) => {
                let name = buffer.name();
                let message = format!("Cannot reload: cannot start reading: {e}: {name}");
                self.editor.message(message);
            }
        }
    }

    /// Takes in what reading a file again reported. Returns whether a frame is needed: only if
    /// the window shows the buffer and its text changed.
    pub(super) fn apply_reload(&mut self, index: usize, update: LoadUpdate) -> bool {
        let load = &mut self.loads[index];
        let id = load.buffer();
        if let Some(resolved) = load.handle().take_resolved() {
            match &resolved {
                Resolved::Paths(paths) => self.editor.judge_reload(id, Ok(paths)),
                Resolved::Unknown(seen) => self.editor.judge_reload(id, Err(seen)),
            }
        }
        let load = &mut self.loads[index];
        let baseline = load.handle().take_baseline();
        let spelling = load.handle().take_own_spelling();
        if let Some(reload) = load.reload_mut() {
            reload.baseline = baseline.or(reload.baseline.take());
            reload.spelling = spelling.or(reload.spelling.take());
        }
        let shown = self.editor.tiled().0.buffer() == id;
        let name = self.editor.buffer(id).map_or("", Buffer::name).to_owned();
        match update {
            // A reload sends its text whole.
            LoadUpdate::Snapshot(_) => false,
            LoadUpdate::Reloaded(reloaded) => {
                let load = self.loads.swap_remove(index);
                let Some(reload) = load.into_reload() else {
                    return false;
                };
                let outcome = if reload.asked {
                    self.editor.revert(id, *reloaded, reload.from)
                } else {
                    self.editor.reload(id, *reloaded, reload.from)
                };
                match outcome {
                    outcome @ (ReloadOutcome::Reloaded | ReloadOutcome::Same) => {
                        if let Some(buffer) = self.editor.buffer_mut(id) {
                            if let Some(baseline) = reload.baseline {
                                buffer.set_baseline(baseline);
                            }
                            if let Some(spelling) = reload.spelling {
                                buffer.set_own_spelling(spelling);
                            }
                        }
                        // One asked for is said even if nothing changed.
                        if outcome == ReloadOutcome::Reloaded || reload.asked {
                            self.disk.reloaded.push(name);
                        }
                        shown && outcome == ReloadOutcome::Reloaded
                    }
                    // What the buffer needs now is decided again, as it has edits, or something
                    // stood in the way.
                    ReloadOutcome::Edited | ReloadOutcome::Later => {
                        if reload.asked {
                            self.editor
                                .message(format!("Not reloaded, as {name} changed meanwhile"));
                        }
                        self.disk.watched(id).acted = None;
                        false
                    }
                    ReloadOutcome::NoSuchBuffer => false,
                }
            }
            // Gone between the look and the read: the next look says so, unless a person asked.
            LoadUpdate::Failed(e @ (LoadError::NotFound | LoadError::NotAFile)) => {
                let load = self.loads.swap_remove(index);
                if load.into_reload().is_some_and(|reload| reload.asked) {
                    self.editor.message(format!("Cannot reload: {e}: {name}"));
                }
                false
            }
            LoadUpdate::Failed(e) => {
                self.loads.swap_remove(index);
                self.editor.message(format!("Cannot reload: {e}: {name}"));
                false
            }
        }
    }

    /// Says which buffers were reloaded, once no other reload is running, so that a checkout
    /// that changed many files says so once.
    fn tell_reloaded(&mut self) {
        if self.disk.reloaded.is_empty()
            || self.loads.iter().any(|load| load.reload_from().is_some())
        {
            return;
        }
        let names = std::mem::take(&mut self.disk.reloaded);
        for name in &names {
            self.editor.message(format!("Reloaded {name}"));
        }
        if names.len() > 1 {
            self.editor
                .message(format!("Reloaded {} files", names.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn stamp(len: u64) -> FileStamp {
        FileStamp {
            modified: SystemTime::UNIX_EPOCH + Duration::from_secs(9),
            len,
            mode: 0o644,
        }
    }

    fn file(len: u64, identity: Option<(u64, u64)>) -> Seen {
        Seen::File {
            stamp: stamp(len),
            identity,
        }
    }

    #[test]
    fn a_file_is_as_read_by_its_time_size_and_identity() {
        let mut identity = None;
        assert!(is_as_read(
            file(3, Some((1, 2))),
            Some(stamp(3)),
            &mut identity
        ));
        assert_eq!(
            identity,
            Some((stamp(3), (1, 2))),
            "learnt at the first look"
        );
        assert!(is_as_read(
            file(3, Some((1, 2))),
            Some(stamp(3)),
            &mut identity
        ));
        assert!(
            !is_as_read(file(3, Some((1, 9))), Some(stamp(3)), &mut identity),
            "another file of the same time and size"
        );
        assert!(!is_as_read(
            file(4, Some((1, 2))),
            Some(stamp(3)),
            &mut identity
        ));
        assert!(!is_as_read(Seen::Gone, Some(stamp(3)), &mut identity));
        assert!(!is_as_read(Seen::NotAFile, Some(stamp(3)), &mut identity));
    }

    #[test]
    fn a_new_identity_is_learnt_after_a_save() {
        let mut identity = Some((stamp(3), (1, 2)));
        // A save renamed a new file in, and the buffer's stamp is the new one.
        assert!(is_as_read(
            file(5, Some((1, 7))),
            Some(stamp(5)),
            &mut identity
        ));
        assert_eq!(identity, Some((stamp(5), (1, 7))));
    }

    #[test]
    fn a_file_that_was_not_there_is_as_read_while_it_is_not() {
        let mut identity = None;
        assert!(is_as_read(Seen::Gone, None, &mut identity));
        assert!(
            !is_as_read(file(1, None), None, &mut identity),
            "it appeared"
        );
    }
}
