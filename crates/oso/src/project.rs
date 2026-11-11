//! Finding the project a file is in (ADR 0114), on a thread of its own, after the file's text
//! has come.
//!
//! Must not depend on: `app` or the UI thread; this touches the file system.
//!
//! From the file's directory up, it asks the metadata of each directory and of `.git`, `.jj`
//! and `.hg` in it, without following a link: a link is no marker and no way up. It stops at
//! the home directory, which is never a project, at a directory another user owns (root
//! aside), and where the file system changes, as git does. It reads one file, a worktree's
//! `.git`, at most [`MAX_GIT_FILE`] bytes, and keeps what that names by its words alone. Nothing
//! is run, and no path a repository writes is looked up (security.md, T1).
//!
//! One thread answers every request in turn, so a slow or hung file system delays other
//! projects, never a file's text or the UI.

use std::fs::{self, Metadata};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use oso_core::editor::ProjectRequest;
use oso_core::project::{Found, MAX_GIT_FILE, RootIdentity, worktree_claim};

/// A request answered: the project found, or none.
pub type Answer = (ProjectRequest, Option<Found>);

/// The thread that finds projects, started with the first request.
pub struct Detector {
    home: Option<PathBuf>,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
    requests: Option<Sender<ProjectRequest>>,
    answers: Arc<Mutex<Vec<Answer>>>,
}

impl Detector {
    /// A detector that never takes `home` for a project, and calls `wake` when answers wait
    /// after none did. `wake` returns false once the UI is gone, which ends the thread.
    pub fn new(home: Option<PathBuf>, wake: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            home,
            wake: Arc::new(wake),
            requests: None,
            answers: Arc::default(),
        }
    }

    /// Finds the project of `request`'s file, on the thread.
    pub fn find(&mut self, request: ProjectRequest) {
        if self.requests.is_none() {
            self.requests = self.start();
        }
        if let Some(requests) = &self.requests
            && requests.send(request).is_err()
        {
            self.requests = None;
        }
    }

    /// The answers that came since the last call.
    pub fn take(&self) -> Vec<Answer> {
        std::mem::take(&mut *lock(&self.answers))
    }

    fn start(&self) -> Option<Sender<ProjectRequest>> {
        let (requests, received) = mpsc::channel::<ProjectRequest>();
        let answers = Arc::clone(&self.answers);
        let wake = Arc::clone(&self.wake);
        let home = self.home.clone();
        let started = thread::Builder::new()
            .name("oso-project".into())
            .spawn(move || {
                // The home directory as the person's environment names it, not a repository.
                let home = home.and_then(|home| fs::canonicalize(home).ok());
                for request in received {
                    // A panic leaves the file in no project, and the thread goes on.
                    let found =
                        std::panic::catch_unwind(|| detect(&request.location, home.as_deref()))
                            .ok()
                            .flatten();
                    let was_empty = {
                        let mut answers = lock(&answers);
                        answers.push((request, found));
                        answers.len() == 1
                    };
                    if was_empty && !wake() {
                        return;
                    }
                }
            });
        match started {
            Ok(_) => Some(requests),
            Err(e) => {
                log::warn!("cannot start finding projects: {e}");
                None
            }
        }
    }
}

fn lock(answers: &Mutex<Vec<Answer>>) -> std::sync::MutexGuard<'_, Vec<Answer>> {
    answers.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The project of the file at `location`, the real place a path led, whether or not a file is
/// there yet. `None` if no directory on the way up holds a marker before the walk stops.
pub fn detect(location: &Path, home: Option<&Path>) -> Option<Found> {
    let mut device = None;
    for dir in location.parent()?.ancestors() {
        if Some(dir) == home {
            return None;
        }
        let metadata = match fs::symlink_metadata(dir) {
            Ok(metadata) => metadata,
            // The directories of a new file may not exist yet.
            Err(e) if e.kind() == io::ErrorKind::NotFound && device.is_none() => continue,
            Err(_) => return None,
        };
        if !metadata.is_dir() || !ours(&metadata) {
            return None;
        }
        let here = device_of(&metadata);
        if *device.get_or_insert(here) != here {
            return None;
        }
        match markers_in(dir) {
            Marker::None => {}
            Marker::Plain => return Some(found(dir, &metadata, None)),
            Marker::GitFile => {
                let claim = read_git_file(&dir.join(".git"))
                    .ok()
                    .and_then(|bytes| worktree_claim(dir, &bytes));
                return Some(found(dir, &metadata, claim));
            }
        }
    }
    None
}

/// What makes a directory a root.
enum Marker {
    None,
    /// A `.git`, `.jj` or `.hg` directory.
    Plain,
    /// A `.git` file, as a worktree or a submodule has.
    GitFile,
}

/// Which marker `dir` holds, by metadata that follows no link, and only one this user or root
/// owns.
fn markers_in(dir: &Path) -> Marker {
    let entry = |name: &str| fs::symlink_metadata(dir.join(name)).ok().filter(ours);
    match entry(".git") {
        Some(git) if git.is_dir() => return Marker::Plain,
        Some(git) if git.is_file() => return Marker::GitFile,
        _ => {}
    }
    if [".jj", ".hg"]
        .into_iter()
        .any(|name| entry(name).is_some_and(|marker| marker.is_dir()))
    {
        return Marker::Plain;
    }
    Marker::None
}

/// At most one byte more than [`MAX_GIT_FILE`] of a `.git` file, so that a longer one is
/// refused by its parser, never waiting on a pipe or a device.
fn read_git_file(path: &Path) -> io::Result<Vec<u8>> {
    let Some((file, _)) = oso_platform::file_meta::open_for_reading(path)? else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    };
    let mut bytes = Vec::new();
    Read::take(file, MAX_GIT_FILE + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// Whether this user or root owns what `metadata` describes, as git's `safe.directory` asks.
fn ours(metadata: &Metadata) -> bool {
    #[cfg(unix)]
    let by_root = std::os::unix::fs::MetadataExt::uid(metadata) == 0;
    #[cfg(not(unix))]
    let by_root = false;
    by_root || oso_platform::privacy::is_this_users(metadata)
}

/// The device a directory is on, where the system says.
fn device_of(metadata: &Metadata) -> Option<u64> {
    #[cfg(unix)]
    return Some(std::os::unix::fs::MetadataExt::dev(metadata));
    #[cfg(not(unix))]
    {
        let _ = metadata;
        None
    }
}

fn found(root: &Path, metadata: &Metadata, worktree_of: Option<PathBuf>) -> Found {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((metadata.dev(), metadata.ino()))
    };
    // `std` keeps a file's identity unstable here, so the path alone is recorded (ADR 0114).
    #[cfg(not(unix))]
    let identity = {
        let _ = metadata;
        None
    };
    Found {
        root: RootIdentity {
            path: root.to_path_buf(),
            identity,
        },
        worktree_of,
    }
}

#[cfg(test)]
mod tests;
