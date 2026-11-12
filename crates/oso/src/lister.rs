//! Listing directories for `C-x C-f` and `C-x C-w` on worker threads (ADR 0087).
//!
//! Must not depend on: `app`, winit or the GPU; the caller supplies the wake-up.
//!
//! The UI thread never touches the file system. A prompt asks for a directory's names, or for
//! a look at the rows it shows, and a worker answers with the request's generation, which the
//! prompt keeps only if it is still its newest. The app holds one [`Lister`], so its limits hold
//! for the process. A thread serves one request at a time, at most [`MAX_THREADS`] are alive,
//! and at most one request per directory is in flight: a directory on a file system that hangs
//! stalls only its own thread, and its next requests wait without holding others up. A newer
//! request for a directory replaces one still waiting, and a prompt that closes drops what it
//! left waiting. Nothing listed is opened.
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
    /// For a link: what is where it leads.
    pub leads_to: Option<LeadsTo>,
}

/// What is where a link leads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeadsTo {
    Directory,
    /// A file, or anything else that is not a directory.
    Other,
    /// Nothing is there.
    Missing,
    /// It could not be told: a loop of links, or no right to look.
    Unknown,
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
        /// Some names could not be read, and are left out.
        partial: bool,
    },
    /// What a look at some of a directory's names found; a name that could not be looked at is
    /// left out of `looks` and named in `missed`, so that nothing waits for it.
    Looked {
        directory: PathBuf,
        generation: u64,
        looks: Vec<Look>,
        missed: Vec<OsString>,
    },
}

/// What a worker is asked.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    List(PathBuf, u64),
    Look(PathBuf, Vec<OsString>, u64),
}

impl Request {
    fn directory(&self) -> &Path {
        match self {
            Self::List(directory, _) | Self::Look(directory, ..) => directory,
        }
    }

    /// Whether `self`, asked later, makes `other` of no use.
    fn replaces(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::List(..), Self::List(..)) | (Self::Look(..), Self::Look(..))
        ) && self.directory() == other.directory()
    }
}

/// The requests waiting, and the directories a worker is busy with.
#[derive(Debug, Default)]
struct Queue {
    waiting: VecDeque<Request>,
    busy: Vec<PathBuf>,
    /// Threads alive.
    alive: usize,
}

impl Queue {
    /// Queues `request` in place of one it replaces.
    fn push(&mut self, request: Request) {
        self.waiting.retain(|other| !request.replaces(other));
        self.waiting.push_back(request);
    }

    /// The first request whose directory no worker is busy with, which this worker now is.
    fn next(&mut self) -> Option<Request> {
        let index = self
            .waiting
            .iter()
            .position(|request| !self.busy.iter().any(|busy| busy == request.directory()))?;
        let request = self.waiting.remove(index)?;
        self.busy.push(request.directory().to_path_buf());
        Some(request)
    }

    /// A worker is done with `directory`.
    fn done(&mut self, directory: &Path) {
        if let Some(index) = self.busy.iter().position(|busy| busy == directory) {
            self.busy.swap_remove(index);
        }
    }

    /// Whether another thread would find work that no thread alive is free to take: a request
    /// for a directory no worker is busy with, beyond those the threads not busy will take.
    fn wants_a_thread(&self) -> bool {
        let free = self
            .waiting
            .iter()
            .filter(|request| !self.busy.iter().any(|busy| busy == request.directory()))
            .count();
        self.alive < MAX_THREADS && free > self.alive.saturating_sub(self.busy.len())
    }
}

/// What the workers and the prompt share.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    answers: Mutex<Vec<Answer>>,
}

/// Locks `mutex`. Each is held only to move a value in or out, so whatever a panic left is
/// whole.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The listing workers. The app holds one, so that its limits hold for the process. Dropping it
/// lets the threads end once their request is done, and drops what they answer.
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

    /// Drops every request still waiting, and every answer not taken, as a prompt that asked
    /// closes. A request a worker has begun runs to its end, and its answer is dropped later.
    pub fn forget(&self) {
        lock(&self.shared.queue).waiting.clear();
        lock(&self.shared.answers).clear();
    }

    /// Queues `request`, and starts a thread for it if one would find it.
    fn ask(&self, request: Request) {
        let mut queue = lock(&self.shared.queue);
        queue.push(request);
        if !queue.wants_a_thread() {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let wake = Arc::clone(&self.wake);
        let spawned = thread::Builder::new()
            .name("oso-list".into())
            .spawn(move || serve(&shared, wake.as_ref()));
        match spawned {
            Ok(_) => queue.alive += 1,
            Err(e) => log::warn!("could not start listing a directory: {e}"),
        }
    }
}

impl Drop for Lister {
    fn drop(&mut self) {
        self.forget();
    }
}

/// Answers requests until none waits that it can take, then ends.
fn serve(shared: &Shared, wake: &(dyn Fn() -> bool + Send + Sync)) {
    loop {
        let request = {
            let mut queue = lock(&shared.queue);
            let request = queue.next();
            if request.is_none() {
                // Under the queue's lock, so that a request queued now finds this one gone
                // and starts another.
                queue.alive -= 1;
            }
            request
        };
        let Some(request) = request else {
            return;
        };
        let directory = request.directory().to_path_buf();
        let answer = std::panic::catch_unwind(|| answer(request));
        lock(&shared.queue).done(&directory);
        let Ok(answer) = answer else {
            continue;
        };
        lock(&shared.answers).push(answer);
        if !wake() {
            lock(&shared.queue).waiting.clear();
        }
    }
}

fn answer(request: Request) -> Answer {
    match request {
        Request::List(directory, generation) => {
            let (entries, truncated, partial) = match list(&directory, MAX_NAMES) {
                Ok(listed) => (Ok(listed.entries.into()), listed.truncated, listed.partial),
                Err(e) => (Err(e.to_string()), false, false),
            };
            Answer::Listed {
                directory,
                generation,
                entries,
                truncated,
                partial,
            }
        }
        Request::Look(directory, names, generation) => {
            let mut missed = Vec::new();
            let looks = names
                .into_iter()
                .filter_map(|name| {
                    let found = look(&directory, &name);
                    if found.is_none() {
                        missed.push(name);
                    }
                    found
                })
                .collect();
            Answer::Looked {
                directory,
                generation,
                looks,
                missed,
            }
        }
    }
}

/// What a listing found.
struct Listed {
    entries: Vec<Entry>,
    /// There were more than the limit.
    truncated: bool,
    /// Some names could not be read.
    partial: bool,
}

/// The names in `directory`, at most `limit`. `read_dir` gives each one's type without a look at
/// the file on most file systems.
fn list(directory: &Path, limit: usize) -> io::Result<Listed> {
    let mut listed = Listed {
        entries: Vec::new(),
        truncated: false,
        partial: false,
    };
    for entry in fs::read_dir(directory)? {
        let Ok(entry) = entry else {
            listed.partial = true;
            continue;
        };
        if listed.entries.len() == limit {
            listed.truncated = true;
            return Ok(listed);
        }
        let kind = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => Kind::Link,
            Ok(kind) if kind.is_dir() => Kind::Directory,
            Ok(kind) if kind.is_file() => Kind::File,
            _ => Kind::Other,
        };
        listed.entries.push(Entry {
            name: entry.file_name(),
            kind,
        });
    }
    Ok(listed)
}

/// A look at `name` in `directory`, which opens nothing.
fn look(directory: &Path, name: &OsString) -> Option<Look> {
    let path = directory.join(name);
    let own = fs::symlink_metadata(&path).ok()?;
    let link = own.file_type().is_symlink();
    // Following a link can reach another file system, which may be slow to answer; this thread
    // waits for it, and only this one.
    let leads_to = link.then(|| match fs::metadata(&path) {
        Ok(metadata) if metadata.is_dir() => LeadsTo::Directory,
        Ok(_) => LeadsTo::Other,
        Err(e) if e.kind() == io::ErrorKind::NotFound => LeadsTo::Missing,
        Err(_) => LeadsTo::Unknown,
    });
    Some(Look {
        mode: mode(&own),
        len: own.len(),
        modified: own.modified().ok(),
        others: !oso_platform::privacy::is_this_users(&own),
        target: link.then(|| fs::read_link(&path).ok()).flatten(),
        leads_to,
        name: name.clone(),
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
