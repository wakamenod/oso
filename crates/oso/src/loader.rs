//! Loading files on a background thread (ADR 0022, ADR 0040).
//!
//! Must not depend on: `app`, winit or the GPU; the caller supplies the
//! wake-up.
//!
//! The thread reads the file in pieces, decodes them and builds the rope
//! and the display map (`oso_core::load::Builder`), so the UI thread never
//! waits on I/O or does O(n) work. It hands the UI O(1) snapshots: the
//! first right after the first read, so the first screen shows at once,
//! then at most one per [`SEND_INTERVAL`], then the complete one.
//!
//! A snapshot waits in the load's slot, and the thread wakes the UI only
//! when the slot was empty. The UI takes the latest one when woken and
//! before each frame, so a busy UI skips snapshots instead of queueing
//! them, and a window opened after the first read draws it in its first
//! frame.
//!
//! Right after opening the file, before reading it, the thread follows the
//! path to where it really leads, so that the UI can tell whether the file
//! may hold secrets (ADR 0043) without touching the file system. A file
//! swapped in before the open is what gets judged.
//!
//! Loading only reads the file. Nothing in it is run or interpreted
//! (security.md, T1).

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::buffer::BufferId;
use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, LoadSnapshot};

mod resolve;

pub use resolve::Resolved;
use resolve::resolve;

/// Bytes read at a time.
const READ_SIZE: usize = 64 * 1024;

/// The shortest time between two snapshots after the first. Each one can
/// cost the UI a frame, so they are spaced out; the first screen is not
/// delayed by it.
const SEND_INTERVAL: Duration = Duration::from_millis(50);

/// What a loading thread reports.
pub enum LoadUpdate {
    /// The content so far; `done` once the whole file is in.
    Snapshot(Box<LoadSnapshot>),
    /// The file could not be read. Nothing more follows.
    Failed(LoadError),
}

impl std::fmt::Debug for LoadUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Snapshot(snapshot) => f
                .debug_struct("Snapshot")
                .field("progress", &snapshot.progress())
                .field("done", &snapshot.done())
                .finish_non_exhaustive(),
            Self::Failed(e) => f.debug_tuple("Failed").field(e).finish(),
        }
    }
}

/// Why a file could not be loaded.
#[derive(Debug)]
pub enum LoadError {
    /// There is no such file. Emacs visits it as a new, empty file.
    NotFound,
    /// A directory, a device or a pipe: only regular files are opened.
    NotAFile,
    /// Reading failed.
    Io(io::Error),
    /// The loading thread panicked.
    Panicked,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such file"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Panicked => f.write_str("loading failed"),
        }
    }
}

/// What the loading thread and the UI share.
#[derive(Default)]
struct Shared {
    cancel: AtomicBool,
    /// The latest update the UI has not taken.
    latest: Mutex<Option<LoadUpdate>>,
    /// Where the path leads. Set before the first update, and taken once.
    resolved: Mutex<Option<Resolved>>,
}

impl Shared {
    /// Locks the slot. It is held only to swap an `Option`, so this never
    /// waits for long, and a thread that panicked cannot have left it
    /// half-written.
    fn slot(&self) -> std::sync::MutexGuard<'_, Option<LoadUpdate>> {
        self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the resolved path, which is also held only to swap an `Option`.
    fn resolved(&self) -> std::sync::MutexGuard<'_, Option<Resolved>> {
        self.resolved.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A running load. Dropping it cancels the load, as killing its buffer
/// will.
pub struct LoadHandle {
    buffer: BufferId,
    shared: Arc<Shared>,
}

impl LoadHandle {
    /// The buffer being loaded.
    pub fn buffer(&self) -> BufferId {
        self.buffer
    }

    /// The latest update, if one arrived since the last call.
    pub fn take(&self) -> Option<LoadUpdate> {
        self.shared.slot().take()
    }

    /// Where the file's path leads, the first time this is called after it
    /// was resolved. Every update comes after this is available, so a caller
    /// who takes it first never handles content without it.
    pub fn take_resolved(&self) -> Option<Resolved> {
        self.shared.resolved().take()
    }
}

impl Drop for LoadHandle {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts loading `path` into `buffer` on a background thread. `wake`
/// tells the UI thread that an update waits in the handle, and returns
/// false once the UI is gone, which stops the load.
pub fn spawn<W>(buffer: BufferId, path: PathBuf, wake: W) -> io::Result<LoadHandle>
where
    W: Fn(BufferId) -> bool + Send + 'static,
{
    let shared = Arc::new(Shared::default());
    let thread_shared = Arc::clone(&shared);
    thread::Builder::new()
        .name("oso-load".into())
        .spawn(move || {
            let shared = thread_shared;
            let post = |update| {
                let old = shared.slot().replace(update);
                let was_empty = old.is_none();
                // Dropped after the lock is released, so the UI never waits
                // on freeing a snapshot.
                drop(old);
                !was_empty || wake(buffer)
            };
            // A panic must not leave the buffer loading for good. Its
            // message, which may quote file content, is dropped.
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                load(&path, &shared, &post)
            }));
            match loaded {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    post(LoadUpdate::Failed(e));
                }
                Err(_) => {
                    post(LoadUpdate::Failed(LoadError::Panicked));
                }
            }
        })?;
    Ok(LoadHandle { buffer, shared })
}

/// Opens `path` if it is a regular file.
fn open(path: &Path) -> Result<(File, u64), LoadError> {
    let not_found = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => LoadError::NotFound,
        _ => LoadError::Io(e),
    };
    // Checked before opening, which blocks on a pipe.
    if !std::fs::metadata(path).map_err(not_found)?.is_file() {
        return Err(LoadError::NotAFile);
    }
    let file = File::open(path).map_err(not_found)?;
    let metadata = file.metadata().map_err(LoadError::Io)?;
    if !metadata.is_file() {
        return Err(LoadError::NotAFile);
    }
    Ok((file, metadata.len()))
}

/// Reads, decodes and sends the file. Returns early, without an error,
/// when cancelled or when the UI is gone.
///
/// Only the size seen at open is read, so a file another process keeps
/// appending to (a log) cannot be read forever.
fn load(path: &Path, shared: &Shared, send: &dyn Fn(LoadUpdate) -> bool) -> Result<(), LoadError> {
    let started = Instant::now();
    let opened = open(path);
    *shared.resolved() = Some(resolve(path));
    let (file, total) = opened?;
    let mut file = file.take(total);
    let mut builder = Builder::new(Some(total), TabWidth::default());
    let mut piece = vec![0; READ_SIZE];
    let mut sent: Option<Instant> = None;
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let read = match file.read(&mut piece) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(LoadError::Io(e)),
        };
        builder.push(&piece[..read]);
        if sent.is_none_or(|sent| sent.elapsed() >= SEND_INTERVAL) {
            if sent.is_none() {
                log::debug!("first {read} bytes decoded in {:?}", started.elapsed());
            }
            if !send(LoadUpdate::Snapshot(Box::new(builder.snapshot()))) {
                return Ok(());
            }
            sent = Some(Instant::now());
        }
    }
    builder.finish();
    if file
        .into_inner()
        .read(&mut piece[..1])
        .is_ok_and(|read| read > 0)
    {
        log::info!("the file grew while loading; the rest is not shown");
    }
    log::debug!("loaded {} bytes in {:?}", builder.len(), started.elapsed());
    send(LoadUpdate::Snapshot(Box::new(builder.snapshot())));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oso_core::editor::Editor;
    use std::sync::mpsc;
    use std::time::Duration;

    /// A file in the temporary directory, which the test removes.
    fn scratch_file(name: &str, content: &[u8]) -> PathBuf {
        let name = format!("oso-loader-test-{}-{name}", std::process::id());
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn id() -> BufferId {
        Editor::default().buffers().next().unwrap().0
    }

    /// Loads `path` and collects the updates the UI would take, until the
    /// last one.
    fn events(path: PathBuf) -> Vec<LoadUpdate> {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(id(), path, move |_| tx.send(()).is_ok()).unwrap();
        let mut updates = Vec::new();
        while rx.recv_timeout(Duration::from_secs(10)).is_ok() {
            // Slow down, as a busy UI would, so updates pile up.
            std::thread::sleep(Duration::from_millis(1));
            let Some(update) = handle.take() else {
                continue;
            };
            let last = matches!(&update, LoadUpdate::Failed(_))
                || matches!(&update, LoadUpdate::Snapshot(s) if s.done());
            updates.push(update);
            if last {
                break;
            }
        }
        updates
    }

    #[test]
    fn a_file_arrives_in_snapshots_ending_with_the_whole() {
        let content = "日本語 🍣\n".repeat(30_000);
        let path = scratch_file("big.txt", content.as_bytes());
        let events = events(path.clone());
        let LoadUpdate::Snapshot(first) = &events[0] else {
            panic!("{events:?}");
        };
        assert!(first.text().len() >= READ_SIZE - 4);
        let Some(LoadUpdate::Snapshot(last)) = events.last() else {
            panic!("{events:?}");
        };
        assert!(last.done());
        assert_eq!(last.text().to_string(), content);
        assert_eq!(last.progress().total, Some(content.len() as u64));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_missing_file_and_a_directory_fail() {
        let missing = events(std::env::temp_dir().join("oso-no-such-file-here"));
        assert!(matches!(
            missing[..],
            [LoadUpdate::Failed(LoadError::NotFound)]
        ));
        let directory = events(std::env::temp_dir());
        assert!(matches!(
            directory[..],
            [LoadUpdate::Failed(LoadError::NotAFile)]
        ));
        #[cfg(unix)]
        {
            // A device is refused before it is opened.
            let device = events(PathBuf::from("/dev/null"));
            assert!(matches!(
                device[..],
                [LoadUpdate::Failed(LoadError::NotAFile)]
            ));
        }
    }

    #[test]
    fn the_resolved_path_is_there_before_the_first_update() {
        let path = scratch_file("resolved.txt", b"abc");
        let (tx, rx) = mpsc::channel();
        let handle = spawn(id(), path.clone(), move |_| tx.send(()).is_ok()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let resolved = handle.take_resolved().unwrap();
        assert_eq!(
            resolved.paths().unwrap().last(),
            Some(&std::fs::canonicalize(&path).unwrap())
        );
        assert_eq!(handle.take_resolved(), None, "taken once");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_missing_file_is_resolved_too() {
        let (tx, rx) = mpsc::channel();
        let path = std::env::temp_dir().join("oso-no-such-file-here");
        let handle = spawn(id(), path, move |_| tx.send(()).is_ok()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(
            handle.take_resolved(),
            Some(Resolved::Paths(paths)) if !paths.is_empty()
        ));
    }

    #[test]
    fn a_closed_ui_stops_the_load() {
        let path = scratch_file("closed.txt", &vec![b'a'; READ_SIZE * 4]);
        let (tx, rx) = mpsc::channel();
        let handle = spawn(id(), path.clone(), move |_| {
            let _ = tx.send(());
            false
        })
        .unwrap();
        assert_eq!(rx.iter().count(), 1, "one wake, then it stopped");
        let Some(LoadUpdate::Snapshot(snapshot)) = handle.take() else {
            panic!("no update");
        };
        assert!(!snapshot.done());
        std::fs::remove_file(path).unwrap();
    }
}
