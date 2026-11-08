//! Loading files on a background thread (ADR 0022, ADR 0040), and reading them again when
//! they change on disk (ADR 0083).
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
//! may hold secrets (ADR 0044) without touching the file system. The verdict is
//! taken from the path of the file that was opened, as the system names it, with
//! the names the walk passed. Where the system cannot name it, the path must still
//! end at the opened file once resolved, or nobody can tell. It also hands over the file's
//! baseline: where the path really leads, and the modification time, size and mode read
//! from the open file, which a save checks before it replaces the file (ADR 0029,
//! ADR 0059).
//!
//! A file that changed on disk is read again the same way, opened, followed, judged and given
//! a baseline, but its text comes whole, compared with the buffer's on this thread
//! ([`spawn_reload`]). [`Watcher`] notices the change (ADR 0083).
//!
//! Loading only reads the file. Nothing in it is run or interpreted
//! (security.md, T1). Nothing logged says how large a file is, as it may hold secrets.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::buffer::{Baseline, BufferId, FileStamp};
use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, LoadSnapshot, Reloaded};
use oso_core::rope::Rope;

mod resolve;
mod watch;

pub use resolve::{Resolved, location, parent};
use resolve::{own_spelling, resolve};
pub use watch::Watcher;

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
    /// The whole file read again, compared with the text the buffer had (ADR 0083). Nothing
    /// more follows.
    Reloaded(Box<Reloaded>),
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
            // Whether it changed, and nothing of its size or where.
            Self::Reloaded(reloaded) => f
                .debug_struct("Reloaded")
                .field("changed", &reloaded.is_changed())
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
    /// The file opened has more than one name, which a load for the agent channel refuses:
    /// its other names, which the secret patterns judge, are not seen (ADR 0085).
    #[cfg_attr(
        not(unix),
        expect(dead_code, reason = "only Unix counts a file's names")
    )]
    SeveralNames,
    /// The file opened is larger than a load for the agent channel takes (ADR 0085).
    TooLargeForTheAgent,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such file"),
            Self::NotAFile => f.write_str("not a regular file"),
            Self::Io(e) => write!(f, "{e}"),
            Self::Panicked => f.write_str("loading failed"),
            Self::SeveralNames => f.write_str("the file has more than one name"),
            Self::TooLargeForTheAgent => f.write_str("the file is larger than the agent may open"),
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
    /// Where the file is and what was opened. Set before the first update, and taken
    /// once.
    baseline: Mutex<Option<Baseline>>,
    /// The file's own spelling. Set before the first update, and taken once.
    spelling: Mutex<Option<PathBuf>>,
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

    /// Locks the file's own spelling, which is also held only to swap an `Option`.
    fn spelling(&self) -> std::sync::MutexGuard<'_, Option<PathBuf>> {
        self.spelling.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Locks the baseline, which is also held only to swap an `Option`.
    fn baseline(&self) -> std::sync::MutexGuard<'_, Option<Baseline>> {
        self.baseline.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// The file's own spelling, if it is there, the first time this is called after it was
    /// opened. Every update comes after this is available.
    pub fn take_own_spelling(&self) -> Option<PathBuf> {
        self.shared.spelling().take()
    }

    /// The file's baseline, the first time this is called after it was opened. Every
    /// update comes after this is available. A path that cannot be resolved has none,
    /// and a buffer without one refuses to save.
    pub fn take_baseline(&self) -> Option<Baseline> {
        self.shared.baseline().take()
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
    spawn_reading(buffer, wake, move |shared, post| {
        load(&path, shared, post, false)
    })
}

/// [`spawn`], for the agent channel: the file opened must have one name, by the open file
/// itself, so that no file swapped in after the channel looked can be a second name of a
/// secret (ADR 0085). One with more fails with [`LoadError::SeveralNames`].
pub fn spawn_one_name<W>(buffer: BufferId, path: PathBuf, wake: W) -> io::Result<LoadHandle>
where
    W: Fn(BufferId) -> bool + Send + 'static,
{
    spawn_reading(buffer, wake, move |shared, post| {
        load(&path, shared, post, true)
    })
}

/// Starts reading `path` again into `buffer`, whose text is `old`, on a background thread, as
/// its file changed on disk (ADR 0083). The path is followed and judged, and the baseline taken,
/// as for [`spawn`], and the whole text comes in one [`LoadUpdate::Reloaded`], compared with
/// `old` there. `wake` is as for [`spawn`].
pub fn spawn_reload<W>(
    buffer: BufferId,
    path: PathBuf,
    old: Rope,
    for_agent: bool,
    wake: W,
) -> io::Result<LoadHandle>
where
    W: Fn(BufferId) -> bool + Send + 'static,
{
    spawn_reading(buffer, wake, move |shared, post| {
        let (file, metadata) = open_and_record(&path, shared, || {})?;
        // A buffer the agent opened is read again only as it may be opened (ADR 0085).
        if for_agent {
            for_the_agent(&metadata)?;
        }
        let Some((builder, _)) = read_all(file, metadata.len(), shared, |_| true)? else {
            return Ok(());
        };
        let reloaded = builder.snapshot().compared_with(&old);
        // Freed here, not on the UI thread, if the buffer has moved on from it.
        drop(old);
        post(LoadUpdate::Reloaded(Box::new(reloaded)));
        Ok(())
    })
}

/// Starts `read` on a background thread, with what the thread and the UI share.
fn spawn_reading<W, R>(buffer: BufferId, wake: W, read: R) -> io::Result<LoadHandle>
where
    W: Fn(BufferId) -> bool + Send + 'static,
    R: FnOnce(&Shared, &dyn Fn(LoadUpdate) -> bool) -> Result<(), LoadError> + Send + 'static,
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
            let loaded =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read(&shared, &post)));
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

/// Starts filling `buffer` with what `open` gives, no further than `limit`, on a background
/// thread: an emergency copy, read to be looked at (ADR 0064). `open` makes its own checks. This
/// resolves nothing and records no baseline, as the buffer visits no file. `wake` is as for
/// [`spawn`].
pub fn spawn_copy<O, W>(buffer: BufferId, open: O, limit: u64, wake: W) -> io::Result<LoadHandle>
where
    O: FnOnce() -> io::Result<File> + Send + 'static,
    W: Fn(BufferId) -> bool + Send + 'static,
{
    spawn_reading(buffer, wake, move |shared, post| {
        let file = open().map_err(LoadError::Io)?;
        read_into(file, limit, shared, post)
    })
}

/// What a save records of a file, from its metadata, with its device and inode. Loading and saving must agree on
/// it, or every save would report a conflict.
///
/// # Errors
///
/// On a system or file system that keeps no modification time.
pub fn stamp_of(metadata: &Metadata) -> io::Result<FileStamp> {
    #[cfg(unix)]
    let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions());
    #[cfg(not(unix))]
    let mode = 0;
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    // `std` keeps a file's identity unstable here.
    #[cfg(not(unix))]
    let identity = None;
    Ok(FileStamp {
        modified: metadata.modified()?,
        len: metadata.len(),
        mode,
        identity,
    })
}

/// Opens `path` if it is a regular file. Returns it with its metadata, read from the open
/// file, so it describes what is read even if the path now leads elsewhere.
fn open(path: &Path) -> Result<(File, Metadata), LoadError> {
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
    Ok((file, metadata))
}

/// The baseline of `path`, given how opening it went. `None` if it cannot be resolved, or
/// could not be opened for a reason other than not being there.
fn baseline_after_open(
    path: &Path,
    opened: &Result<(File, Metadata), LoadError>,
) -> Option<Baseline> {
    let location = location(path).ok()?;
    match opened {
        Ok((_, metadata)) => baseline_of_opened(location, metadata),
        Err(LoadError::NotFound) => Some(Baseline {
            location,
            file: None,
        }),
        Err(_) => None,
    }
}

/// The baseline of a file opened with `opened` as its metadata, if `location` holds that
/// very file. The location is worked out after the open, so a link switched in between would
/// otherwise pair one file's text with another place, where a save would then write it.
fn baseline_of_opened(location: PathBuf, opened: &Metadata) -> Option<Baseline> {
    holds_opened(&location, opened)?;
    let file = Some(stamp_of(opened).ok()?);
    Some(Baseline { location, file })
}

/// What is at `path`, if `path` itself holds the file opened with `opened` as its metadata, and no
/// link there leads to it.
fn holds_opened(path: &Path, opened: &Metadata) -> Option<Metadata> {
    let there = std::fs::symlink_metadata(path).ok()?;
    (!there.is_symlink() && same_file(opened, &there)).then_some(there)
}

/// What decides whether the opened `file` may hold secrets: the path the system gives for the open
/// file, with the names the walk of `resolved` passed, as the verdict is on the file that is read.
/// A walk that could not tell still counts so. Where the system gives no path, the one the walk ended at must surely hold the opened file, or
/// nobody can tell. With nothing opened, nothing was read.
fn verdict_of_opened(resolved: Resolved, file: Option<(&File, &Metadata)>) -> Resolved {
    let Some((file, opened)) = file else {
        return resolved;
    };
    if let Ok(Some(real)) = oso_platform::file_meta::path_of_opened(file) {
        // A walk that could not tell stays so, with the file's own path among its names.
        return match resolved {
            Resolved::Paths(mut names) => {
                names.push(real);
                Resolved::Paths(names)
            }
            Resolved::Unknown(mut names) => {
                names.push(real);
                Resolved::Unknown(names)
            }
        };
    }
    match resolved {
        Resolved::Paths(paths)
            if !paths.last().is_some_and(|last| {
                holds_opened(last, opened).is_some_and(|there| surely_same_file(opened, &there))
            }) =>
        {
            Resolved::Unknown(paths)
        }
        resolved => resolved,
    }
}

/// Whether two sets of metadata are of one file: its device and inode.
#[cfg(unix)]
pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether two sets of metadata are known to be of one file. Unlike [`same_file`], this does
/// not guess: where the system cannot say, two files are two.
#[cfg(unix)]
pub(crate) fn surely_same_file(a: &Metadata, b: &Metadata) -> bool {
    same_file(a, b)
}

/// `std` keeps a file's identity unstable here, so nothing can be sure.
#[cfg(not(unix))]
pub(crate) fn surely_same_file(_: &Metadata, _: &Metadata) -> bool {
    false
}

/// `std` keeps a file's identity unstable here, so this goes by what a save compares.
#[cfg(not(unix))]
pub(crate) fn same_file(a: &Metadata, b: &Metadata) -> bool {
    same_stamp(a, b)
}

/// Whether two sets of metadata show one modification time and size.
#[cfg(any(not(unix), test))]
fn same_stamp(a: &Metadata, b: &Metadata) -> bool {
    match (stamp_of(a), stamp_of(b)) {
        (Ok(a), Ok(b)) => a.same_content(b),
        _ => false,
    }
}

/// Reads, decodes and sends the file. Returns early, without an error,
/// when cancelled or when the UI is gone.
///
/// Only the size seen at open is read, so a file another process keeps
/// appending to (a log) cannot be read forever.
fn load(
    path: &Path,
    shared: &Shared,
    send: &dyn Fn(LoadUpdate) -> bool,
    one_name: bool,
) -> Result<(), LoadError> {
    load_around(path, shared, send, || {}, one_name)
}

/// [`load`], running `after_open` right after the open, for a test.
fn load_around(
    path: &Path,
    shared: &Shared,
    send: &dyn Fn(LoadUpdate) -> bool,
    after_open: impl FnOnce(),
    one_name: bool,
) -> Result<(), LoadError> {
    let (file, metadata) = open_and_record(path, shared, after_open)?;
    if one_name {
        for_the_agent(&metadata)?;
    }
    read_into(file, metadata.len(), shared, send)
}

/// What a load for the agent channel checks on the file it opened, so that no file swapped
/// in after the channel looked gets past: one name only, and no more than the channel opens
/// (ADR 0085).
fn for_the_agent(metadata: &Metadata) -> Result<(), LoadError> {
    #[cfg(unix)]
    if std::os::unix::fs::MetadataExt::nlink(metadata) > 1 {
        return Err(LoadError::SeveralNames);
    }
    if metadata.len() > crate::channel::server::MAX_OPEN_BYTES {
        return Err(LoadError::TooLargeForTheAgent);
    }
    Ok(())
}

/// Opens `path`, runs `after_open`, and records where the path leads, the verdict, the file's own
/// spelling and its baseline for the UI, before anything is read.
fn open_and_record(
    path: &Path,
    shared: &Shared,
    after_open: impl FnOnce(),
) -> Result<(File, Metadata), LoadError> {
    let opened = open(path);
    after_open();
    let resolved = resolve(path);
    let file = opened
        .as_ref()
        .ok()
        .map(|(file, metadata)| (file, metadata));
    *shared.resolved() = Some(verdict_of_opened(resolved, file));
    let baseline = baseline_after_open(path, &opened);
    *shared.spelling() = baseline
        .as_ref()
        .and_then(|baseline| own_spelling(&baseline.location));
    *shared.baseline() = baseline;
    opened
}

/// Reads `total` bytes of `file`, decodes them and sends them.
fn read_into(
    file: File,
    total: u64,
    shared: &Shared,
    send: &dyn Fn(LoadUpdate) -> bool,
) -> Result<(), LoadError> {
    let mut sent: Option<Instant> = None;
    let read = read_all(file, total, shared, |builder| {
        if sent.is_none_or(|sent| sent.elapsed() >= SEND_INTERVAL) {
            if !send(LoadUpdate::Snapshot(Box::new(builder.snapshot()))) {
                return false;
            }
            sent = Some(Instant::now());
        }
        true
    })?;
    let Some((builder, grew)) = read else {
        return Ok(());
    };
    if grew {
        log::info!("the file grew while loading; the rest is not shown");
    }
    send(LoadUpdate::Snapshot(Box::new(builder.snapshot())));
    Ok(())
}

/// Reads `total` bytes of `file` and decodes them, showing `each` the builder after each read.
/// Returns the finished builder and whether the file had more, or `None` if cancelled or once
/// `each` returns false.
fn read_all(
    file: File,
    total: u64,
    shared: &Shared,
    mut each: impl FnMut(&Builder) -> bool,
) -> Result<Option<(Builder, bool)>, LoadError> {
    let mut file = file.take(total);
    let mut builder = Builder::new(Some(total), TabWidth::default());
    let mut piece = vec![0; READ_SIZE];
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = match file.read(&mut piece) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(LoadError::Io(e)),
        };
        builder.push(&piece[..read]);
        if !each(&builder) {
            return Ok(None);
        }
    }
    builder.finish();
    let grew = file
        .into_inner()
        .read(&mut piece[..1])
        .is_ok_and(|read| read > 0);
    Ok(Some((builder, grew)))
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
        // Where the system cannot name an open file, nobody can tell.
        #[cfg(unix)]
        assert_eq!(
            resolved.paths().unwrap().last(),
            Some(&std::fs::canonicalize(&path).unwrap())
        );
        #[cfg(not(unix))]
        assert!(matches!(resolved, Resolved::Unknown(_)), "{resolved:?}");
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
    fn the_baseline_is_the_opened_file_and_is_there_before_the_first_snapshot() {
        let path = scratch_file("baseline.txt", b"abc");
        let metadata = std::fs::metadata(&path).unwrap();
        let (tx, rx) = mpsc::channel();
        let handle = spawn(id(), path.clone(), move |_| tx.send(()).is_ok()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            handle.take_baseline(),
            Some(Baseline {
                location: std::fs::canonicalize(&path).unwrap(),
                file: Some(stamp_of(&metadata).unwrap()),
            })
        );
        assert_eq!(handle.take_baseline(), None, "taken once");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_location_holding_another_file_gives_no_baseline() {
        let opened = scratch_file("opened.txt", b"abc");
        let other = scratch_file("other.txt", b"abc");
        let metadata = std::fs::metadata(&opened).unwrap();
        let location = std::fs::canonicalize(&opened).unwrap();
        assert!(baseline_of_opened(location.clone(), &metadata).is_some());
        // A link switched to another file between the open and the look.
        #[cfg(unix)]
        assert_eq!(baseline_of_opened(other.clone(), &metadata), None);
        // To nowhere: the text must not count as a file deleted from there.
        let missing = location.with_file_name("oso-no-such-file-after-the-open");
        assert_eq!(baseline_of_opened(missing, &metadata), None);
        std::fs::remove_file(opened).unwrap();
        std::fs::remove_file(other).unwrap();
    }

    /// The verdict a buffer visiting `path` gets from `resolved`.
    #[cfg(unix)]
    fn secret_by(path: &Path, resolved: &Resolved) -> bool {
        let mut buffer = oso_core::buffer::Buffer::visiting(path.to_path_buf());
        resolved.judge(&mut buffer);
        buffer.is_secret()
    }

    #[cfg(unix)]
    #[test]
    fn the_verdict_is_on_the_file_that_was_opened() {
        let directory = scratch_file("judged-dir", b"");
        std::fs::remove_file(&directory).unwrap();
        std::fs::create_dir(&directory).unwrap();
        let secret = directory.join(".env");
        let plain = directory.join("plain.txt");
        std::fs::write(&secret, b"KEY=1\n").unwrap();
        std::fs::write(&plain, b"text\n").unwrap();
        let link = directory.join("notes.txt");
        let point_at = |target: &Path| {
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(target, &link).unwrap();
        };
        let judged = |after_open: &dyn Fn()| {
            let shared = Shared::default();
            load_around(&link, &shared, &|_| true, after_open, false).unwrap();
            let resolved = shared.resolved().take().unwrap();
            secret_by(&link, &resolved)
        };
        point_at(&plain);
        assert!(!judged(&|| {}), "an ordinary file through a link");
        point_at(&secret);
        assert!(judged(&|| {}), "a secret one through a link");
        point_at(&secret);
        assert!(judged(&|| point_at(&plain)));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_path_holds_the_opened_file_only_if_it_is_that_file_and_no_link() {
        let opened = scratch_file("held.txt", b"abc");
        let other = scratch_file("held-other.txt", b"abcd");
        let metadata = std::fs::metadata(&opened).unwrap();
        assert!(holds_opened(&opened, &metadata).is_some());
        assert!(holds_opened(&other, &metadata).is_none());
        let missing = opened.with_file_name("oso-no-such-file-held");
        assert!(holds_opened(&missing, &metadata).is_none());
        #[cfg(unix)]
        {
            let link =
                opened.with_file_name(format!("oso-loader-test-{}-held-link", std::process::id()));
            let _ = std::fs::remove_file(&link);
            std::os::unix::fs::symlink(&opened, &link).unwrap();
            assert!(holds_opened(&link, &metadata).is_none(), "a link to it");
            std::fs::remove_file(link).unwrap();
        }
        std::fs::remove_file(opened).unwrap();
        std::fs::remove_file(other).unwrap();
    }

    #[test]
    fn with_nothing_opened_the_walk_decides() {
        let walked = Resolved::Paths(vec![PathBuf::from("/p/notes.txt")]);
        assert_eq!(verdict_of_opened(walked.clone(), None), walked);
        let unknown = Resolved::Unknown(vec![PathBuf::from("/p/notes.txt")]);
        assert_eq!(verdict_of_opened(unknown.clone(), None), unknown);
    }

    #[test]
    fn a_walk_that_could_not_tell_stays_so_with_the_file_opened() {
        let path = scratch_file("untold.txt", b"abc");
        let file = File::open(&path).unwrap();
        let metadata = file.metadata().unwrap();
        let seen = vec![PathBuf::from("/p/notes.txt")];
        let verdict = verdict_of_opened(Resolved::Unknown(seen.clone()), Some((&file, &metadata)));
        let Resolved::Unknown(names) = verdict else {
            panic!("{verdict:?}");
        };
        assert_eq!(names[0], seen[0]);
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        assert_eq!(
            std::fs::canonicalize(names.last().unwrap()).unwrap(),
            std::fs::canonicalize(&path).unwrap(),
            "with its own path among the names"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn the_system_names_the_opened_file() {
        let path = scratch_file("named.txt", b"abc");
        let file = File::open(&path).unwrap();
        let named = oso_platform::file_meta::path_of_opened(&file)
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::canonicalize(&named).unwrap(),
            std::fs::canonicalize(&path).unwrap()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_location_holding_a_file_of_another_size_gives_no_baseline_on_any_system() {
        let opened = scratch_file("sized.txt", b"abc");
        let other = scratch_file("other-size.txt", b"abcd");
        let metadata = std::fs::metadata(&opened).unwrap();
        assert_eq!(baseline_of_opened(other.clone(), &metadata), None);
        std::fs::remove_file(opened).unwrap();
        std::fs::remove_file(other).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_load_for_the_agent_refuses_a_file_with_another_name() {
        let path = scratch_file("one-name.txt", b"abc");
        let shared = Shared::default();
        assert!(load(&path, &shared, &|_| true, true).is_ok(), "one name");
        let second = path.with_extension("second");
        let _ = std::fs::remove_file(&second);
        std::fs::hard_link(&path, &second).unwrap();
        let shared = Shared::default();
        let refused = load(&second, &shared, &|_| true, true);
        assert!(
            matches!(refused, Err(LoadError::SeveralNames)),
            "{refused:?}"
        );
        let shared = Shared::default();
        assert!(
            load(&second, &shared, &|_| true, false).is_ok(),
            "a person's load reads it"
        );
        std::fs::remove_file(&second).unwrap();
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn stamps_tell_another_file_by_its_time_or_size() {
        let path = scratch_file("stamp.txt", b"abc");
        let longer = scratch_file("stamp-longer.txt", b"abcd");
        let older = scratch_file("stamp-older.txt", b"abc");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&older)
            .unwrap();
        file.set_modified(std::time::SystemTime::UNIX_EPOCH)
            .unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert!(same_stamp(&metadata, &std::fs::metadata(&path).unwrap()));
        assert!(!same_stamp(&metadata, &std::fs::metadata(&longer).unwrap()));
        assert!(!same_stamp(&metadata, &std::fs::metadata(&older).unwrap()));
        for path in [path, longer, older] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn a_missing_file_has_a_baseline_with_no_file() {
        let (tx, rx) = mpsc::channel();
        let path = std::env::temp_dir().join("oso-no-such-file-here");
        let handle = spawn(id(), path.clone(), move |_| tx.send(()).is_ok()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let directory = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert_eq!(
            handle.take_baseline(),
            Some(Baseline {
                location: directory.join("oso-no-such-file-here"),
                file: None,
            })
        );
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

    #[test]
    fn a_reload_sends_the_whole_text_compared_with_the_old_one() {
        let content = "head\n".to_owned() + &"日本語 🍣\n".repeat(30_000);
        let path = scratch_file("reload.txt", content.as_bytes());
        let old = Rope::from("head\nother\n");
        let (tx, rx) = mpsc::channel();
        let handle =
            spawn_reload(id(), path.clone(), old, false, move |_| tx.send(()).is_ok()).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(handle.take_resolved().is_some());
        assert!(handle.take_baseline().unwrap().file.is_some());
        let update = handle.take().unwrap();
        let printed = format!("{update:?}");
        assert_eq!(
            printed, "Reloaded { changed: true, .. }",
            "nothing of its size"
        );
        let LoadUpdate::Reloaded(reloaded) = update else {
            panic!("{update:?}");
        };
        assert!(reloaded.is_changed());
        assert!(handle.take().is_none(), "one update only");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_load_for_the_agent_refuses_a_file_grown_past_its_limit() {
        let path = scratch_file("agent-large.txt", b"");
        // Sparse: it takes no room, but says it is that large.
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(crate::channel::server::MAX_OPEN_BYTES + 1)
            .unwrap();
        let shared = Shared::default();
        let refused = load(&path, &shared, &|_| true, true);
        assert!(
            matches!(refused, Err(LoadError::TooLargeForTheAgent)),
            "{refused:?}"
        );
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_reload_for_the_agent_refuses_a_file_given_a_second_name() {
        let path = scratch_file("reload-agent.txt", b"plain\n");
        let second = path.with_extension("key");
        let _ = std::fs::remove_file(&second);
        std::fs::hard_link(&path, &second).unwrap();
        let (tx, rx) = mpsc::channel();
        let wake = move |_| tx.send(()).is_ok();
        let handle = spawn_reload(id(), path.clone(), Rope::from(""), true, wake).unwrap();
        rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let update = handle.take().unwrap();
        assert!(
            matches!(update, LoadUpdate::Failed(LoadError::SeveralNames)),
            "no text comes: {update:?}"
        );
        std::fs::remove_file(&second).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
