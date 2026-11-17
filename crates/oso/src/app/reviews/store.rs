//! Saved reviews on disk (ADR 0122, "Persistence", step 6): one thread, `oso-review-store`,
//! that reads a saved review and builds its model back, writes one as it changes, lists them
//! for `review-resume`, removes one for `review-forget`, and sweeps those not opened for 30
//! days or past the most kept.
//!
//! Must not depend on: the editor, key presses or how a frame is drawn. The UI thread hands it
//! snapshots and takes what it found; it never waits for it, but at an exit or a restart's
//! start, at most the second the recent files and sessions wait too.
//!
//! - Saved reviews are in `reviews/` under the state directory, a private directory (mode
//!   `0700`, as [`PrivateDir`] makes and checks it), each file of mode `0600`, replaced whole
//!   through a `.part`, or left as it was.
//! - A file is read only if it is a regular one of this user's with one name, of at most
//!   [`saved::MAX_BYTES`], and only when its header names the project and scope asked for.
//! - What is read is checked whole by `Review::restore`, here, so the UI thread puts the model
//!   in at O(1).
//! - The sweep runs as the thread starts: a review not opened for [`MAX_AGE`] goes, and past
//!   [`MAX_SAVED`] reviews the ones opened longest ago go. Nothing else in the directory is
//!   touched.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Read as _};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oso_core::editor::ReviewId;
use oso_core::review::{Review, Saved};
use oso_platform::file_meta;
use oso_platform::privacy::{PrivateDir, has_one_name, is_this_users};

use crate::review::saved::{self, Header, Kept, SUFFIX};

/// The directory saved reviews are in, under the state directory.
pub(super) const DIRECTORY: &str = "reviews";
/// Where a write goes first, so that a saved review is whole or not there.
const PART: &str = "review.part";
/// A review not opened for this long goes.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// The most saved reviews kept; past them, those opened longest ago go.
pub(super) const MAX_SAVED: usize = 256;
/// The most bytes of a saved review read for its header.
const HEAD_BYTES: u64 = 64 * 1024;

/// What the UI thread hands over to write: a review's model and view as they are now, with the
/// paths whose lines must not be written.
pub(super) struct Snapshot {
    pub(super) header: Header,
    pub(super) saved: Saved,
    pub(super) kept: Kept,
    pub(super) withheld: HashSet<Box<[u8]>>,
}

enum Job {
    Read {
        review: ReviewId,
        name: String,
        header: Header,
    },
    Write {
        review: Option<ReviewId>,
        name: String,
        snapshot: Box<Snapshot>,
    },
    Remove(String),
    List,
    Flush(SyncSender<()>),
}

/// A saved review found and built back.
pub(crate) struct Found {
    pub(super) review: Review,
    pub(super) kept: Kept,
    /// Lines of the file left out, as not ones oso writes.
    pub(super) left_out: usize,
}

/// What the store found or did, for the UI thread.
pub(crate) enum Stored {
    /// What the read for `review` found: nothing saved, a review, or why the file could not be
    /// read, which a write must then not replace.
    Read {
        review: ReviewId,
        found: Result<Option<Box<Found>>, String>,
    },
    /// Whether the write of `review` went to disk.
    Written { review: ReviewId, written: bool },
    /// The saved reviews, by file name, opened last first.
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
            Self::Written { review, written } => f
                .debug_struct("Written")
                .field("review", &review.get())
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
    /// found. `None` if it cannot start.
    pub(super) fn start(state: &Path, tell: impl Fn(Stored) + Send + 'static) -> io::Result<Store> {
        let directory = state.join(DIRECTORY);
        let (jobs, waiting) = mpsc::channel();
        thread::Builder::new()
            .name("oso-review-store".into())
            .spawn(move || run(&directory, &waiting, &tell))?;
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

    /// Hands `snapshot` over to be written as `name`, saying how it went if `review` is named.
    pub(super) fn write(&self, review: Option<ReviewId>, name: String, snapshot: Snapshot) -> bool {
        self.jobs
            .send(Job::Write {
                review,
                name,
                snapshot: Box::new(snapshot),
            })
            .is_ok()
    }

    pub(super) fn remove(&self, name: String) -> bool {
        self.jobs.send(Job::Remove(name)).is_ok()
    }

    pub(super) fn list(&self) -> bool {
        self.jobs.send(Job::List).is_ok()
    }

    /// Waits at most `wait` until what was handed over before is written; whether it was.
    pub(super) fn flush(&self, wait: Duration) -> bool {
        let (done, answer) = mpsc::sync_channel(1);
        self.jobs.send(Job::Flush(done)).is_ok() && answer.recv_timeout(wait).is_ok()
    }
}

/// The store's thread, until the UI's side is dropped.
fn run(directory: &Path, jobs: &Receiver<Job>, tell: &dyn Fn(Stored)) {
    if let Err(e) = sweep(directory, now()) {
        log::info!("saved reviews were not swept: {e}");
    }
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Read {
                review,
                name,
                header,
            } => tell(Stored::Read {
                review,
                found: read(directory, &name, &header).map(|found| found.map(Box::new)),
            }),
            Job::Write {
                review,
                name,
                snapshot,
            } => {
                let written = write(directory, &name, &snapshot);
                if let Err(e) = &written {
                    log::warn!("a review was not saved: {e}");
                }
                // The thread frees what it was handed.
                drop(snapshot);
                if let Some(review) = review {
                    tell(Stored::Written {
                        review,
                        written: written.is_ok(),
                    });
                }
                if let Err(e) = keep_at_most(directory, MAX_SAVED) {
                    log::info!("saved reviews were not counted: {e}");
                }
            }
            Job::Remove(name) => {
                if let Err(e) = remove(directory, &name) {
                    log::warn!("a saved review was not removed: {e}");
                }
            }
            Job::List => tell(Stored::Listed(list(directory).unwrap_or_else(|e| {
                log::info!("saved reviews were not listed: {e}");
                Vec::new()
            }))),
            Job::Flush(done) => {
                let _ = done.send(());
            }
        }
    }
}

/// Milliseconds since the epoch.
pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |now| u64::try_from(now.as_millis()).unwrap_or(u64::MAX))
}

/// Whether `name` is one a saved review has: 32 hex digits and [`SUFFIX`], so that no other
/// file in the directory, nor a path, is ever read or removed.
fn is_saved_name(name: &str) -> bool {
    name.strip_suffix(SUFFIX)
        .is_some_and(|stem| stem.len() == 32 && stem.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The review saved as `name` in `directory`, if its header names `asked`'s root and scope.
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
        // Another review's, by a name that came out the same: nothing of it is used.
        return Ok(None);
    }
    let review = Review::restore(read.saved).map_err(|e| format!("{e:?}"))?;
    Ok(Some(Found {
        review,
        kept: read.kept,
        left_out: read.left_out,
    }))
}

/// Writes `snapshot` as `name` in `directory`, made private if it is missing.
fn write(directory: &Path, name: &str, snapshot: &Snapshot) -> io::Result<()> {
    if !is_saved_name(name) {
        return Err(io::Error::other("not a saved review's name"));
    }
    let withheld = |path: &[u8]| snapshot.withheld.contains(path);
    let bytes = saved::encode(
        &snapshot.header,
        &snapshot.saved,
        &snapshot.kept,
        &withheld,
        saved::MAX_BYTES,
    )
    .ok_or_else(|| io::Error::other("its comments alone are past the limit"))?;
    let text = String::from_utf8(bytes).map_err(io::Error::other)?;
    let directory = PrivateDir::open(directory)?;
    crate::sessions::record::replace_file(&directory, name, PART, &text)
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

/// The header of the review saved as `name` in `directory`, read from its start alone, if it
/// is a regular file of this user's with one name.
fn head(directory: &PrivateDir, name: &str) -> Option<Header> {
    let (file, meta) = file_meta::open_regular_here(&directory.path().join(name)).ok()??;
    if !is_this_users(&meta) || !has_one_name(&meta) {
        return None;
    }
    let mut start = Vec::new();
    file.take(HEAD_BYTES).read_to_end(&mut start).ok()?;
    saved::header(&start)
}

/// The saved reviews in `directory`, by file name, opened last first.
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

/// Removes the saved reviews in `directory` not opened since [`MAX_AGE`] before `now`, and
/// past [`MAX_SAVED`] those opened longest ago.
fn sweep(directory: &Path, now: u64) -> io::Result<()> {
    let age = u64::try_from(MAX_AGE.as_millis()).unwrap_or(u64::MAX);
    let listed = list(directory)?;
    for (name, header) in &listed {
        if now.saturating_sub(header.opened) > age {
            remove(directory, name)?;
        }
    }
    keep_at_most(directory, MAX_SAVED)
}

/// Removes the saved reviews past the `most` opened last. Reads their headers only when there
/// are more than `most`, so that a write costs a look at the directory's names.
fn keep_at_most(directory: &Path, most: usize) -> io::Result<()> {
    let Some(private) = PrivateDir::existing(directory)? else {
        return Ok(());
    };
    let mut count = 0;
    for entry in fs::read_dir(private.path())? {
        let name = entry?.file_name();
        count += usize::from(name.to_str().is_some_and(is_saved_name));
    }
    if count <= most {
        return Ok(());
    }
    for (name, _) in list(directory)?.iter().skip(most) {
        remove(directory, name)?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "store_tests.rs"]
mod tests;
