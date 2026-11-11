//! Walking a project's files for `project-find-file` (ADR 0114), on a thread of its own,
//! `oso-walk`, with ripgrep's walker.
//!
//! Must not depend on: `app`, winit or the UI thread; this touches the file system, and the
//! caller supplies the wake-up.
//!
//! The files a walk lists are the ones `rg --files --hidden` lists, less what belongs to no
//! file of this project:
//!
//! - Hidden files are listed. The `.git`, `.jj` and `.hg` of the project are never entered,
//!   and a directory that holds one is another project, so it is not entered either.
//! - Ignore files apply as they do for `rg`: `.gitignore` inside a git repository,
//!   `.git/info/exclude`, the person's global excludes, `.ignore` and `.rgignore`. They are
//!   read as patterns only. Nothing is run.
//! - Links are not followed. A link that leads to a file is listed, as opening it goes through
//!   the loader's full verdict; one that leads to a directory, or nowhere, is not.
//! - A path that is not UTF-8 is left out, as an answer naming it would name another file.
//! - At most [`MAX_WALKED`] files are listed; past that the walk stops and says it was cut.
//!
//! What it finds goes to the editor in batches, one at most every [`BATCH_EVERY`]. A newer
//! listing, `C-g` or closing the prompt moves the request's live generation on, and the walk
//! stops at its next entry. One thread walks every request in turn: a newer one stops the one
//! before first, and a slow or hung file system stalls only this thread.

use std::fmt;
use std::fs;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use ignore::{DirEntry, WalkBuilder};
use oso_core::editor::{WalkEnd, WalkRequest};
use oso_core::project::{MARKERS, MAX_WALKED};

/// How often a walk sends what it found.
pub const BATCH_EVERY: Duration = Duration::from_millis(50);

/// A batch of what a walk found: paths from the project's root, and whether the walk ended.
/// `Debug` gives its generation and size only, as a list can name files the person keeps
/// private.
pub struct Walked {
    pub generation: u64,
    pub paths: Vec<Box<str>>,
    pub end: Option<WalkEnd>,
}

impl fmt::Debug for Walked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Walked")
            .field("generation", &self.generation)
            .field("paths", &self.paths.len())
            .field("end", &self.end)
            .finish()
    }
}

/// The walker's thread, started with the first request.
pub struct Walker {
    requests: Option<Sender<WalkRequest>>,
    send: SendBatch,
}

/// Hands a batch to the event loop; `false` once it is gone.
type SendBatch = std::sync::Arc<dyn Fn(Walked) -> bool + Send + Sync>;

impl Walker {
    /// A walker that hands each batch to `send`.
    pub fn new(send: impl Fn(Walked) -> bool + Send + Sync + 'static) -> Self {
        Self {
            requests: None,
            send: std::sync::Arc::new(send),
        }
    }

    /// Walks `request`'s root on the walker's thread, after any walk before it, which a newer
    /// listing has stopped. If the thread cannot start, the walk ends at once with nothing
    /// found, so the prompt stops waiting.
    pub fn walk(&mut self, request: WalkRequest) {
        if self.requests.is_none() {
            let (requests, queue) = mpsc::channel::<WalkRequest>();
            let send = std::sync::Arc::clone(&self.send);
            let spawned = thread::Builder::new()
                .name("oso-walk".into())
                .spawn(move || serve(&queue, &*send));
            if spawned.is_ok() {
                self.requests = Some(requests);
            } else {
                log::warn!("the walking thread could not start");
            }
        }
        let request = match &self.requests {
            Some(requests) => match requests.send(request) {
                Ok(()) => return,
                Err(failed) => failed.0,
            },
            None => request,
        };
        self.requests = None;
        (self.send)(Walked {
            generation: request.generation,
            paths: Vec::new(),
            end: Some(WalkEnd { cut: false }),
        });
    }
}

/// Walks each request that comes, in turn, until the walker is dropped or the event loop has
/// gone.
fn serve(queue: &mpsc::Receiver<WalkRequest>, send: &(dyn Fn(Walked) -> bool + Send + Sync)) {
    while let Ok(request) = queue.recv() {
        let generation = request.generation;
        let live = || request.live.load(Ordering::Relaxed) == generation;
        if !live() {
            continue;
        }
        let mut gone = false;
        let walked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            walk(&request.root, MAX_WALKED, &|| !live(), &mut |paths, end| {
                gone |= !send(Walked {
                    generation,
                    paths,
                    end,
                });
            });
        }));
        if walked.is_err() {
            // The panic hook writes no message, so nothing of the paths is printed.
            log::warn!("a walk panicked");
            gone |= !send(Walked {
                generation,
                paths: Vec::new(),
                end: Some(WalkEnd { cut: false }),
            });
        }
        if gone {
            return;
        }
    }
}

/// Walks the files under `root` as the module says, listing at most `most`, and hands them to
/// `batch` as paths from `root`, every [`BATCH_EVERY`] and once more as it ends. A walk that
/// `stop` stops ends with no last batch.
pub fn walk(
    root: &Path,
    most: usize,
    stop: &dyn Fn() -> bool,
    batch: &mut dyn FnMut(Vec<Box<str>>, Option<WalkEnd>),
) {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .follow_links(false)
        .add_custom_ignore_filename(".rgignore")
        .filter_entry(enters);
    let mut found = Vec::new();
    let mut count = 0;
    let mut sent = Instant::now();
    for entry in builder.build() {
        if stop() {
            return;
        }
        // An entry that cannot be read, such as a directory this user may not list, is left
        // out, as `rg` leaves it.
        let Ok(entry) = entry else {
            continue;
        };
        if let Some(path) = listed(root, &entry) {
            if count == most {
                batch(found, Some(WalkEnd { cut: true }));
                return;
            }
            found.push(path);
            count += 1;
        }
        if sent.elapsed() >= BATCH_EVERY && !found.is_empty() {
            batch(std::mem::take(&mut found), None);
            sent = Instant::now();
        }
    }
    batch(found, Some(WalkEnd { cut: false }));
}

/// Whether the walk goes on into `entry`, or lists it: never a marker, and never a directory
/// below the root that holds one, as that is another project.
fn enters(entry: &DirEntry) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let name = entry.file_name();
    if MARKERS.iter().any(|marker| name == *marker) {
        return false;
    }
    let directory = entry.file_type().is_some_and(|kind| kind.is_dir());
    // Asked without following a link, as detection asks (ADR 0114).
    !directory
        || !MARKERS
            .iter()
            .any(|marker| fs::symlink_metadata(entry.path().join(marker)).is_ok())
}

/// The path from `root` that `entry` is listed as, if it is a file or a link that leads to
/// one, and its path is UTF-8.
fn listed(root: &Path, entry: &DirEntry) -> Option<Box<str>> {
    let kind = entry.file_type()?;
    let file = kind.is_file()
        || (kind.is_symlink() && fs::metadata(entry.path()).is_ok_and(|meta| meta.is_file()));
    if !file {
        return None;
    }
    let relative = entry.path().strip_prefix(root).ok()?.to_str()?;
    // One spelling on every system: `/` is no name's on Windows either.
    #[cfg(windows)]
    let relative = relative.replace('\\', "/");
    Some(relative.into())
}

#[cfg(test)]
mod tests;
