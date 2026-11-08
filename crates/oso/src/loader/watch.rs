//! Noticing that open files changed on disk (ADR 0029, ADR 0083): a thread looks at each
//! visited path once a second, and at once when asked, as when the window gains the focus.
//!
//! Must not depend on: `app` or the editor's state; it only reports what it saw, and the UI
//! compares that with what each buffer read.
//!
//! A look is one `stat` per file, which follows links, so a file replaced by a rename, a link
//! pointed elsewhere and a file on a network mount all show the same way. It costs microseconds
//! a file, and needs no dependency and no watch on a directory. The thread wakes the UI only
//! when what it saw differs from its last report, so an idle editor stays idle. A look that
//! blocks, as on a mount that went away, holds up only this thread. It never opens a file.

use std::fs::Metadata;
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use oso_core::buffer::{BufferId, DiskState};

use super::stamp_of;

/// How long the thread waits between two looks.
const INTERVAL: Duration = Duration::from_secs(1);

/// What a look at a visited path found. The device and inode go in the stamp.
fn state_of(look: io::Result<Metadata>) -> DiskState {
    match look {
        Ok(metadata) if !metadata.is_file() => DiskState::NotAFile,
        Ok(metadata) => stamp_of(&metadata).map_or(DiskState::Unknown, DiskState::File),
        Err(e) if e.kind() == io::ErrorKind::NotFound => DiskState::Gone,
        Err(_) => DiskState::Unknown,
    }
}

/// What the thread and the UI share.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

struct State {
    /// The paths to look at, by buffer, shared so that a look copies none.
    files: Arc<[(BufferId, PathBuf)]>,
    /// The UI asked for a look now.
    poked: bool,
    /// The UI forgot what it was told, so the next look reports whatever it finds.
    forget: bool,
    /// The number of the last [`Watcher::look_again`], which each report carries, so that the
    /// UI can tell a report from a look made before it.
    generation: u64,
    /// The watcher is gone.
    stop: bool,
    /// The latest report the UI has not taken.
    report: Option<(u64, Vec<(BufferId, DiskState)>)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            files: Arc::new([]),
            poked: false,
            forget: false,
            generation: 0,
            stop: false,
            report: None,
        }
    }
}

impl Shared {
    /// Locks the state. It is held only to swap what is in it, never during a look.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The thread that looks at open files. Dropping it stops the thread.
pub struct Watcher {
    shared: Arc<Shared>,
}

impl Watcher {
    /// Starts the thread. `wake` tells the UI that a report waits, and returns false once the
    /// UI is gone, which stops the thread.
    pub fn spawn<W>(wake: W) -> io::Result<Self>
    where
        W: Fn() -> bool + Send + 'static,
    {
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        thread::Builder::new()
            .name("oso-watch".into())
            .spawn(move || run(&thread_shared, &wake))?;
        Ok(Self { shared })
    }

    /// Looks at these paths from now on, and at once if they changed.
    pub fn watch(&self, files: Vec<(BufferId, PathBuf)>) {
        let mut state = self.shared.state();
        if *state.files != *files {
            state.files = files.into();
            state.poked = true;
            self.shared.wake.notify_one();
        }
    }

    /// Looks at once, and reports what it finds even if it was reported before, as the UI
    /// forgot it, or a save of a buffer has ended. Returns the generation that look's report
    /// carries: a report of an older one was looked at before this call.
    pub fn look_again(&self) -> u64 {
        let mut state = self.shared.state();
        state.poked = true;
        state.forget = true;
        state.generation += 1;
        self.shared.wake.notify_one();
        state.generation
    }

    /// What the last look saw, with its generation, if it differs from the report taken before.
    pub fn take(&self) -> Option<(u64, Vec<(BufferId, DiskState)>)> {
        self.shared.state().report.take()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.shared.state().stop = true;
        self.shared.wake.notify_one();
    }
}

fn run(shared: &Shared, wake: &dyn Fn() -> bool) {
    let mut last: Vec<(BufferId, DiskState)> = Vec::new();
    loop {
        let files = {
            let mut state = shared.state();
            while !state.stop && !state.poked {
                if state.files.is_empty() {
                    // Nothing to look at until the UI says.
                    state = shared
                        .wake
                        .wait(state)
                        .unwrap_or_else(PoisonError::into_inner);
                    continue;
                }
                let (next, waited) = shared
                    .wake
                    .wait_timeout(state, INTERVAL)
                    .unwrap_or_else(PoisonError::into_inner);
                state = next;
                if waited.timed_out() {
                    break;
                }
            }
            if state.stop {
                return;
            }
            state.poked = false;
            if std::mem::take(&mut state.forget) {
                last.clear();
            }
            (state.generation, Arc::clone(&state.files))
        };
        let (generation, files) = files;
        let seen: Vec<(BufferId, DiskState)> = files
            .iter()
            .map(|(buffer, path)| (*buffer, state_of(std::fs::metadata(path))))
            .collect();
        if seen == last {
            continue;
        }
        last.clone_from(&seen);
        let was_empty = shared.state().report.replace((generation, seen)).is_none();
        if was_empty && !wake() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::editor::Editor;
    use std::sync::mpsc;

    fn id() -> BufferId {
        Editor::default().buffers().next().unwrap().0
    }

    fn scratch(name: &str) -> PathBuf {
        let name = format!("oso-watch-test-{}-{name}", std::process::id());
        std::env::temp_dir().join(name)
    }

    /// The next report, within a few seconds.
    fn next(watcher: &Watcher, rx: &mpsc::Receiver<()>) -> Vec<(BufferId, DiskState)> {
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        watcher.take().unwrap().1
    }

    #[test]
    fn a_change_is_reported_once() {
        let path = scratch("change.txt");
        std::fs::write(&path, "one").unwrap();
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::spawn(move || tx.send(()).is_ok()).unwrap();
        watcher.watch(vec![(id(), path.clone())]);
        let first = next(&watcher, &rx);
        let [(_, DiskState::File(stamp))] = first[..] else {
            panic!("{first:?}");
        };
        assert_eq!(stamp.len, 3);

        std::fs::write(&path, "three").unwrap();
        let [(_, DiskState::File(stamp))] = next(&watcher, &rx)[..] else {
            panic!("a regular file");
        };
        assert_eq!(stamp.len, 5);
        // Nothing changed, so the next look, a second later, wakes nothing.
        assert!(
            rx.recv_timeout(INTERVAL + Duration::from_millis(300))
                .is_err()
        );
        // Unless the UI forgot it, and then the report says which look it is.
        let generation = watcher.look_again();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (reported, seen) = watcher.take().unwrap();
        assert_eq!(reported, generation);
        assert_eq!(seen[0].1, DiskState::File(stamp));

        std::fs::remove_file(&path).unwrap();
        assert_eq!(next(&watcher, &rx)[0].1, DiskState::Gone);
    }

    #[test]
    fn a_directory_is_not_a_file() {
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::spawn(move || tx.send(()).is_ok()).unwrap();
        watcher.watch(vec![(id(), std::env::temp_dir())]);
        assert_eq!(next(&watcher, &rx)[0].1, DiskState::NotAFile);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_replaced_by_a_rename_is_another_file() {
        let path = scratch("renamed.txt");
        let other = scratch("renamed.txt.new");
        std::fs::write(&path, "same").unwrap();
        let (tx, rx) = mpsc::channel();
        let watcher = Watcher::spawn(move || tx.send(()).is_ok()).unwrap();
        watcher.watch(vec![(id(), path.clone())]);
        let DiskState::File(before) = next(&watcher, &rx)[0].1 else {
            panic!("a regular file");
        };
        std::fs::write(&other, "same").unwrap();
        std::fs::rename(&other, &path).unwrap();
        let DiskState::File(after) = next(&watcher, &rx)[0].1 else {
            panic!("a regular file");
        };
        assert_eq!(before.len, after.len);
        assert_ne!(before.identity, after.identity);
        assert!(!before.same_file(after));
        std::fs::remove_file(&path).unwrap();
    }
}
