//! Listing directories for `C-x C-f` and `C-x C-w` on worker threads (ADR 0087).
//!
//! Must not depend on: `app`, winit or the GPU; the caller supplies the wake-up.
//!
//! The UI thread never touches the file system. A prompt asks for a directory's names, or for
//! a look at the rows it shows, and a worker answers with the request's generation, which the
//! prompt keeps only if it is still its newest. A thread serves one request at a time, and at
//! most [`MAX_THREADS`] are alive: a directory on a file system that hangs stalls only its own
//! thread, and a request made while all are busy waits for one. Nothing listed is opened.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "C-x C-f and C-x C-w read paths once M2-17's prompt merges"
    )
)]

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::SystemTime;

/// The most names a listing keeps of one directory. The rest are left out, and the listing says
/// so.
pub const MAX_NAMES: usize = 100_000;

/// The most listing threads alive at once.
pub const MAX_THREADS: usize = 4;

/// The most rows one look covers: the rows a prompt shows (ADR 0076).
pub const MAX_LOOKS: usize = 18;

/// What a name in a directory is, as listing it says, with no look at the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Directory,
    /// A symbolic link. Where it leads is for a look to say.
    Link,
    /// A pipe, a device or a socket: listed, and refused if opened.
    Other,
}

/// A name in a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
}

/// What a look at one name found, for its annotation (ADR 0087).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Look {
    pub name: OsString,
    /// The mode bits, as `ls -l` writes them, of the name itself, a link's own for a link.
    pub mode: u32,
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// Someone other than this user owns it.
    pub others: bool,
    /// For a link: where it leads, as it is written.
    pub target: Option<PathBuf>,
    /// For a link: what it leads to is a directory.
    pub to_directory: bool,
    /// For a link: nothing is where it leads.
    pub missing: bool,
}

/// A worker's answer.
#[derive(Debug)]
pub enum Answer {
    /// The names in a directory, in no order, or why they could not be read.
    Listed {
        directory: PathBuf,
        generation: u64,
        entries: Result<Arc<[Entry]>, String>,
        /// More than [`MAX_NAMES`] were there.
        truncated: bool,
    },
    /// What a look at some of a directory's names found; a name that could not be looked at is
    /// left out.
    Looked {
        directory: PathBuf,
        generation: u64,
        looks: Vec<Look>,
    },
}

/// What a worker is asked.
enum Request {
    List(PathBuf, u64),
    Look(PathBuf, Vec<OsString>, u64),
}

/// What the workers and the prompt share.
#[derive(Default)]
struct Shared {
    waiting: Mutex<VecDeque<Request>>,
    answers: Mutex<Vec<Answer>>,
    alive: Mutex<usize>,
}

/// Locks `mutex`. Each is held only to move a value in or out, so whatever a panic left is
/// whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The listing workers of one prompt. Dropping it lets the threads end once their request is
/// done, and drops what they answer.
pub struct Lister {
    shared: Arc<Shared>,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Lister {
    /// Workers that call `wake` after each answer, which says false once the UI is gone.
    pub fn new(wake: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            shared: Arc::default(),
            wake: Arc::new(wake),
        }
    }

    /// Asks for the names in `directory`, answered with `generation`.
    pub fn list(&self, directory: PathBuf, generation: u64) {
        self.ask(Request::List(directory, generation));
    }

    /// Asks for a look at `names` in `directory`, at most [`MAX_LOOKS`] of them.
    pub fn look(&self, directory: PathBuf, mut names: Vec<OsString>, generation: u64) {
        names.truncate(MAX_LOOKS);
        self.ask(Request::Look(directory, names, generation));
    }

    /// The answers that came since the last call, oldest first.
    pub fn take(&self) -> Vec<Answer> {
        std::mem::take(&mut *lock(&self.shared.answers))
    }

    /// Queues `request`, and starts a thread for it if fewer than [`MAX_THREADS`] are alive.
    fn ask(&self, request: Request) {
        lock(&self.shared.waiting).push_back(request);
        let mut alive = lock(&self.shared.alive);
        if *alive >= MAX_THREADS {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let wake = Arc::clone(&self.wake);
        let spawned = thread::Builder::new()
            .name("oso-list".into())
            .spawn(move || serve(&shared, wake.as_ref()));
        match spawned {
            Ok(_) => *alive += 1,
            Err(e) => log::warn!("could not start listing a directory: {e}"),
        }
    }
}

/// Answers requests until none waits, then ends.
fn serve(shared: &Shared, wake: &(dyn Fn() -> bool + Send + Sync)) {
    loop {
        let request = {
            let mut waiting = lock(&shared.waiting);
            let request = waiting.pop_front();
            if request.is_none() {
                // Under the queue's lock, so that a request queued now finds this one gone
                // and starts another.
                *lock(&shared.alive) -= 1;
            }
            request
        };
        let Some(request) = request else {
            return;
        };
        let answer = std::panic::catch_unwind(|| answer(request));
        let Ok(answer) = answer else {
            continue;
        };
        lock(&shared.answers).push(answer);
        if !wake() {
            lock(&shared.waiting).clear();
        }
    }
}

fn answer(request: Request) -> Answer {
    match request {
        Request::List(directory, generation) => {
            let (entries, truncated) = match list(&directory, MAX_NAMES) {
                Ok((entries, truncated)) => (Ok(entries.into()), truncated),
                Err(e) => (Err(e.to_string()), false),
            };
            Answer::Listed {
                directory,
                generation,
                entries,
                truncated,
            }
        }
        Request::Look(directory, names, generation) => {
            let looks = names
                .into_iter()
                .filter_map(|name| look(&directory, name))
                .collect();
            Answer::Looked {
                directory,
                generation,
                looks,
            }
        }
    }
}

/// The names in `directory`, at most `limit`, and whether there were more. `read_dir` gives
/// each one's type without a look at the file on most file systems.
fn list(directory: &Path, limit: usize) -> io::Result<(Vec<Entry>, bool)> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory)? {
        let Ok(entry) = entry else {
            continue;
        };
        if entries.len() == limit {
            return Ok((entries, true));
        }
        let kind = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => Kind::Link,
            Ok(kind) if kind.is_dir() => Kind::Directory,
            Ok(kind) if kind.is_file() => Kind::File,
            _ => Kind::Other,
        };
        entries.push(Entry {
            name: entry.file_name(),
            kind,
        });
    }
    Ok((entries, false))
}

/// A look at `name` in `directory`, which opens nothing.
fn look(directory: &Path, name: OsString) -> Option<Look> {
    let path = directory.join(&name);
    let own = fs::symlink_metadata(&path).ok()?;
    let link = own.file_type().is_symlink();
    let led = link.then(|| fs::metadata(&path));
    Some(Look {
        mode: mode(&own),
        len: own.len(),
        modified: own.modified().ok(),
        others: !oso_platform::privacy::is_this_users(&own),
        target: link.then(|| fs::read_link(&path).ok()).flatten(),
        to_directory: matches!(&led, Some(Ok(metadata)) if metadata.is_dir()),
        missing: matches!(&led, Some(Err(_))),
        name,
    })
}

#[cfg(unix)]
fn mode(metadata: &fs::Metadata) -> u32 {
    std::os::unix::fs::MetadataExt::mode(metadata)
}

#[cfg(not(unix))]
fn mode(_: &fs::Metadata) -> u32 {
    0
}

#[cfg(test)]
mod tests;
