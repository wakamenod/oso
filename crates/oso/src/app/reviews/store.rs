//! Saved reviews on disk (ADR 0122, "Persistence", step 6): one thread, `oso-review-store`,
//! that reads a saved review and builds its model back, writes one as it changes, lists them
//! for `review-resume`, removes one for `review-forget`, and sweeps those not used for 30 days
//! or past the most kept.
//!
//! Must not depend on: the editor, key presses or how a frame is drawn. The UI thread hands it
//! snapshots and takes what it found; it never waits for it, but at an exit or a restart's
//! start, at most the second the recent files and sessions wait too.
//!
//! - Saved reviews are in `reviews/` under the state directory, a private directory (mode
//!   `0700`, as [`PrivateDir`] makes and checks it), each file of mode `0600`, replaced whole
//!   through a `.part` of its own write's, or left as it was. A part this thread did not make
//!   is never removed but by the sweep, once it is an hour old.
//! - A file is read only if it is a regular one of this user's with one name, of at most
//!   [`saved::MAX_BYTES`], and only when its header names the project and scope asked for:
//!   one that names another is never written over.
//! - What is read is checked whole by `Review::restore`, here, so the UI thread puts the model
//!   in at O(1). Which paths may hold secrets is judged here too, from the buffers the UI
//!   thread saw that may.
//! - The list of saved reviews is read from the directory once, when the thread is first idle
//!   (the sweep), and kept up to date in memory from then on. Its header's time is that
//!   of the last write: a review not written or opened for [`MAX_AGE`] goes, as does one dated
//!   past it in the future, and past [`MAX_SAVED`] those used longest ago go. Nothing else in
//!   the directory is touched.
//! - A write whose bytes are those written last for its name writes nothing.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::hash::{Hash as _, Hasher as _};
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oso_core::editor::ReviewId;
use oso_core::review::{Review, Saved};
use oso_platform::file_meta;
use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};

use crate::review::Entry;
use crate::review::saved::{self, Header, SUFFIX, SavedView};

/// The directory saved reviews are in, under the state directory.
pub(super) const DIRECTORY: &str = "reviews";
/// What a write's part ends with.
const PART_SUFFIX: &str = ".part";
/// A review not written or opened for this long goes.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// A part this old is a write that never ended.
const STALE_PART: Duration = Duration::from_secs(60 * 60);
/// The most saved reviews kept; past them, those used longest ago go.
pub(super) const MAX_SAVED: usize = 256;
/// The most bytes of a saved review read for its header.
const HEAD_BYTES: u64 = 64 * 1024;
/// How long the thread waits with nothing to do before it sweeps, so that the sweep never
/// stands before a read as oso starts.
const IDLE: Duration = Duration::from_secs(2);

/// What the UI thread hands over to write: a review's model and view as they are now, and what
/// the paths whose lines must not be written are judged by.
pub(super) struct Snapshot {
    pub(super) header: Header,
    pub(super) saved: Saved,
    pub(super) view: SavedView,
    /// The review's root, which paths are under.
    pub(super) root: PathBuf,
    /// The files of buffers that may hold secrets or are withheld from agents, as the UI saw
    /// them while the review was open.
    pub(super) held: Arc<HashSet<PathBuf>>,
    /// The review's latest listing, whose renamed files' names before are judged too.
    pub(super) listing: Arc<[Entry]>,
}

/// Who a write's outcome is told to.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Told {
    /// The open review.
    Review(ReviewId),
    /// A review closed, holding this many comments of the person's not sent.
    Closed { unsent: usize },
}

enum Job {
    Read {
        review: ReviewId,
        name: String,
        header: Header,
    },
    Write {
        told: Told,
        name: String,
        snapshot: Box<Snapshot>,
    },
    Remove(String),
    Flush(SyncSender<()>),
}

/// A saved review found and built back.
pub(crate) struct Found {
    pub(super) review: Review,
    pub(super) view: SavedView,
    /// Lines of the file left out, as not ones oso writes.
    pub(super) left_out: usize,
    /// The pull request a review of one commit was stepped to from (ADR 0167).
    pub(super) pull: Option<u64>,
    /// A pull request's review's last review by the person (step 2).
    pub(super) checkpoint: Option<crate::review::commits::Checkpoint>,
}

/// What the store found or did, for the UI thread.
pub(crate) enum Stored {
    /// What the read for `review` found: nothing saved, a review, or why the file could not be
    /// read, which a write must then not replace.
    Read {
        review: ReviewId,
        found: Result<Option<Box<Found>>, String>,
    },
    /// Whether a write went to disk.
    Written { told: Told, written: bool },
    /// The saved reviews, by file name, used last first, each time they changed.
    Listed(Vec<(String, Header)>),
}

impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { review, found } => f
                .debug_struct("Read")
                .field("review", &review.get())
                .field("found", &found.as_ref().map(Option::is_some))
                .finish(),
            Self::Written { told, written } => f
                .debug_struct("Written")
                .field("told", told)
                .field("written", written)
                .finish(),
            Self::Listed(listed) => f.debug_tuple("Listed").field(&listed.len()).finish(),
        }
    }
}

/// The store's side on the UI thread: where jobs go.
pub(super) struct Store {
    jobs: Sender<Job>,
}

impl Store {
    /// Starts the store's thread over `state`, the state directory, telling `tell` what it
    /// found.
    pub(super) fn start(state: &Path, tell: impl Fn(Stored) + Send + 'static) -> io::Result<Store> {
        let directory = state.join(DIRECTORY);
        let (jobs, waiting) = mpsc::channel();
        thread::Builder::new()
            .name("oso-review-store".into())
            .spawn(move || Thread::new(directory).run(&waiting, &tell))?;
        Ok(Store { jobs })
    }

    /// Asks for the review saved as `name`, which `header` must name; false if the thread
    /// ended.
    pub(super) fn read(&self, review: ReviewId, name: String, header: Header) -> bool {
        self.jobs
            .send(Job::Read {
                review,
                name,
                header,
            })
            .is_ok()
    }

    /// Hands `snapshot` over to be written as `name`, its outcome told to `told`; the snapshot
    /// back if the thread ended, for the caller to free off the UI thread.
    pub(super) fn write(
        &self,
        told: Told,
        name: String,
        snapshot: Snapshot,
    ) -> Result<(), Box<Snapshot>> {
        let job = Job::Write {
            told,
            name,
            snapshot: Box::new(snapshot),
        };
        self.jobs
            .send(job)
            .map_err(|mpsc::SendError(job)| match job {
                Job::Write { snapshot, .. } => snapshot,
                _ => unreachable!("the job sent is a write"),
            })
    }

    pub(super) fn remove(&self, name: String) -> bool {
        self.jobs.send(Job::Remove(name)).is_ok()
    }

    /// Waits at most `wait` until what was handed over before is written; whether it was.
    pub(super) fn flush(&self, wait: Duration) -> bool {
        let (done, answer) = mpsc::sync_channel(1);
        self.jobs.send(Job::Flush(done)).is_ok() && answer.recv_timeout(wait).is_ok()
    }
}

/// The store's thread's state.
struct Thread {
    directory: PathBuf,
    /// The saved reviews, once read from the directory, kept up to date since.
    listed: Option<HashMap<String, Header>>,
    /// A hash of the bytes last written for each name.
    written: HashMap<String, u64>,
    /// The parts made, numbered, so that each write has its own.
    parts: u64,
}

impl Thread {
    fn new(directory: PathBuf) -> Thread {
        Thread {
            directory,
            listed: None,
            written: HashMap::new(),
            parts: 0,
        }
    }

    /// Answers jobs until the UI's side is dropped, sweeping once the first time it is idle.
    fn run(mut self, jobs: &Receiver<Job>, tell: &dyn Fn(Stored)) {
        loop {
            let job = if self.listed.is_some() {
                match jobs.recv() {
                    Ok(job) => job,
                    Err(_) => return,
                }
            } else {
                match jobs.recv_timeout(IDLE) {
                    Ok(job) => job,
                    Err(RecvTimeoutError::Timeout) => {
                        self.sweep(now());
                        tell(Stored::Listed(self.list()));
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            };
            self.answer(job, tell);
        }
    }

    fn answer(&mut self, job: Job, tell: &dyn Fn(Stored)) {
        match job {
            Job::Read {
                review,
                name,
                header,
            } => tell(Stored::Read {
                review,
                found: read(&self.directory, &name, &header).map(|found| found.map(Box::new)),
            }),
            Job::Write {
                told,
                name,
                snapshot,
            } => {
                let written = self.write(&name, *snapshot);
                if let Err(e) = &written {
                    log::warn!("a review was not saved: {e}");
                }
                let changed = written.as_ref().is_ok_and(|changed| *changed);
                tell(Stored::Written {
                    told,
                    written: written.is_ok(),
                });
                if changed && self.listed.is_some() {
                    tell(Stored::Listed(self.list()));
                }
            }
            Job::Remove(name) => {
                if let Err(e) = remove(&self.directory, &name) {
                    log::warn!("a saved review was not removed: {e}");
                }
                self.written.remove(&name);
                if let Some(listed) = &mut self.listed {
                    listed.remove(&name);
                }
                tell(Stored::Listed(self.list()));
            }
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }

    /// The saved reviews, used last first.
    fn list(&self) -> Vec<(String, Header)> {
        let mut listed: Vec<(String, Header)> = (self.listed.iter())
            .flatten()
            .map(|(name, header)| (name.clone(), header.clone()))
            .collect();
        listed.sort_by_key(|(_, header)| std::cmp::Reverse(header.opened));
        listed
    }

    /// Reads the list from the directory, then removes the reviews not used since [`MAX_AGE`]
    /// before `now` or dated past it after, those past the [`MAX_SAVED`] used last, and parts
    /// an hour old.
    fn sweep(&mut self, now: u64) {
        let listed = match list(&self.directory) {
            Ok(listed) => listed,
            Err(e) => {
                log::info!("saved reviews were not swept: {e}");
                self.listed = Some(HashMap::new());
                return;
            }
        };
        self.listed = Some(listed.into_iter().collect());
        let age = u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX);
        let gone: Vec<String> = (self.listed.iter().flatten())
            .filter(|(_, header)| {
                now.saturating_sub(header.opened) > age || header.opened > now.saturating_add(age)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for name in gone {
            self.forget(&name);
        }
        self.keep_at_most(MAX_SAVED);
        if let Err(e) = remove_stale_parts(&self.directory) {
            log::info!("old parts of saved reviews were not removed: {e}");
        }
    }

    fn forget(&mut self, name: &str) {
        match remove(&self.directory, name) {
            Ok(()) => {
                if let Some(listed) = &mut self.listed {
                    listed.remove(name);
                }
            }
            Err(e) => log::warn!("a saved review was not removed: {e}"),
        }
    }

    /// Removes the saved reviews past the `most` used last, by the list in memory.
    fn keep_at_most(&mut self, most: usize) {
        if self
            .listed
            .as_ref()
            .is_none_or(|listed| listed.len() <= most)
        {
            return;
        }
        let past: Vec<String> = (self.list().into_iter().skip(most))
            .map(|(name, _)| name)
            .collect();
        for name in past {
            self.forget(&name);
        }
    }

    /// Writes `snapshot` as `name`, dated now, unless its bytes are those written last for
    /// it, its date aside; whether it wrote.
    fn write(&mut self, name: &str, mut snapshot: Snapshot) -> io::Result<bool> {
        if !is_saved_name(name) {
            return Err(io::Error::other("not a saved review's name"));
        }
        snapshot.header.opened = snapshot.header.opened.max(now());
        let withheld = withheld_paths(&snapshot);
        let withheld = |path: &[u8]| withheld.contains(path);
        let bytes = saved::encode(
            &snapshot.header,
            &snapshot.saved,
            &snapshot.view,
            &withheld,
            saved::MAX_BYTES,
        )
        .ok_or_else(|| io::Error::other("its comments alone are past the limit"))?;
        // The header aside, as its date changes with every write.
        let mut hasher = DefaultHasher::new();
        let body = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        bytes[body..].hash(&mut hasher);
        let hash = hasher.finish();
        if self.written.get(name) == Some(&hash) {
            return Ok(false);
        }
        self.parts += 1;
        let part = format!("{name}.{}.{}{PART_SUFFIX}", std::process::id(), self.parts);
        replace(&self.directory, name, &part, &bytes)?;
        self.written.insert(name.to_owned(), hash);
        if let Some(listed) = &mut self.listed {
            listed.insert(name.to_owned(), snapshot.header);
        }
        self.keep_at_most(MAX_SAVED);
        Ok(true)
    }
}

/// Milliseconds since the epoch.
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX))
}

/// The paths of `snapshot` whose lines must not be written: those whose name may hold secrets,
/// or the name they had before a rename, and those of a buffer the UI saw that may. Each path
/// once, however many comments it has.
fn withheld_paths(snapshot: &Snapshot) -> HashSet<Box<[u8]>> {
    let paths: HashSet<&[u8]> = (snapshot.saved.files.iter().map(|file| &*file.path))
        .chain(snapshot.saved.comments.iter().map(|comment| &*comment.path))
        .collect();
    let before: HashMap<&[u8], &[u8]> = (snapshot.listing.iter())
        .filter_map(|entry| Some((&*entry.path, &**entry.old_path.as_ref()?)))
        .filter(|(path, _)| paths.contains(path))
        .collect();
    let judge = |path: &[u8]| super::secrets::withheld(&snapshot.root, &snapshot.held, path);
    paths
        .into_iter()
        .filter(|path| judge(path) || before.get(path).is_some_and(|old| judge(old)))
        .map(Box::from)
        .collect()
}

/// Whether `name` is one a saved review has: 32 hex digits and [`SUFFIX`], so that no other
/// file in the directory, nor a path, is ever read or removed.
fn is_saved_name(name: &str) -> bool {
    name.strip_suffix(SUFFIX)
        .is_some_and(|stem| stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Whether `name` is a write's part: a saved review's name, then two numbers, or the one part
/// an earlier build used.
fn is_part_name(name: &str) -> bool {
    if name == "review.part" {
        return true;
    }
    let Some(rest) = name.strip_suffix(PART_SUFFIX) else {
        return false;
    };
    let mut pieces = rest.rsplitn(3, '.');
    let digits = |piece: Option<&str>| {
        piece.is_some_and(|piece| !piece.is_empty() && piece.bytes().all(|b| b.is_ascii_digit()))
    };
    digits(pieces.next()) && digits(pieces.next()) && pieces.next().is_some_and(is_saved_name)
}

/// The review saved as `name` in `directory`, if its header names `asked`'s root and scope:
/// a file of another review's is an error, so that it is never written over.
fn read(directory: &Path, name: &str, asked: &Header) -> Result<Option<Found>, String> {
    if !is_saved_name(name) {
        return Err("not a saved review's name".into());
    }
    let max = u64::try_from(saved::MAX_BYTES).unwrap_or(u64::MAX);
    let bytes = match crate::sessions::record::read_file(directory, name, max) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let read = saved::decode(&bytes).map_err(str::to_owned)?;
    let theirs = saved::file_name(&read.header.root, &read.header.scope);
    let ours = saved::file_name(&asked.root, &asked.scope);
    if read.header.root != asked.root || theirs != ours {
        return Err("its file is another review's, whose name came out the same".into());
    }
    let review = Review::restore(read.saved).map_err(|e| format!("{e:?}"))?;
    Ok(Some(Found {
        review,
        view: read.view,
        left_out: read.left_out,
        pull: read.header.pull,
        checkpoint: read.header.checkpoint,
    }))
}

/// Replaces `name` in `directory`, made private if it is missing, with `bytes` whole, through
/// `part`, a name of this write's own, and syncs the directory; or leaves it as it was.
fn replace(directory: &Path, name: &str, part: &str, bytes: &[u8]) -> io::Result<()> {
    let directory = PrivateDir::open(directory)?;
    let part = directory.path().join(part);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&part).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| fs::rename(&part, directory.path().join(name)));
    if let Err(e) = renamed {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    #[cfg(unix)]
    if let Ok(entries) = fs::File::open(directory.path()) {
        let _ = entries.sync_all();
    }
    Ok(())
}

fn remove(directory: &Path, name: &str) -> io::Result<()> {
    if !is_saved_name(name) {
        return Err(io::Error::other("not a saved review's name"));
    }
    let Some(directory) = PrivateDir::existing(directory)? else {
        return Ok(());
    };
    match fs::remove_file(directory.path().join(name)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Removes the parts in `directory` an hour old: writes that never ended, any instance's.
fn remove_stale_parts(directory: &Path) -> io::Result<()> {
    let Some(directory) = PrivateDir::existing(directory)? else {
        return Ok(());
    };
    for entry in fs::read_dir(directory.path())? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let old = (entry.metadata())
            .and_then(|meta| meta.modified())
            .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > STALE_PART));
        if is_part_name(&name) && old {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

/// The header of the review saved as `name` in `directory`, read from its first line alone,
/// if it is a regular file of this user's with one name.
fn head(directory: &PrivateDir, name: &str) -> Option<Header> {
    let (file, meta) = file_meta::open_regular_here(&directory.path().join(name)).ok()??;
    if !is_this_users(&meta) || !has_one_name(&meta) {
        return None;
    }
    let mut first = Vec::new();
    BufReader::new(file.take(HEAD_BYTES))
        .read_until(b'\n', &mut first)
        .ok()?;
    saved::header(&first)
}

/// The saved reviews in `directory`, by file name, used last first.
fn list(directory: &Path) -> io::Result<Vec<(String, Header)>> {
    let Some(directory) = PrivateDir::existing(directory)? else {
        return Ok(Vec::new());
    };
    let mut found: Vec<(String, Header)> = Vec::new();
    for entry in fs::read_dir(directory.path())? {
        let Ok(name) = entry?.file_name().into_string() else {
            continue;
        };
        if !is_saved_name(&name) {
            continue;
        }
        if let Some(header) = head(&directory, &name) {
            found.push((name, header));
        }
    }
    found.sort_by_key(|(_, header)| std::cmp::Reverse(header.opened));
    Ok(found)
}

#[cfg(all(test, unix))]
#[path = "store_tests.rs"]
mod tests;
