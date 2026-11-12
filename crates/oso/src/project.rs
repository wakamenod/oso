//! Finding the project a file is in (ADR 0114), on a thread of its own, after the file's text
//! has come.
//!
//! Must not depend on: `app`, `config` or the UI thread; this touches the file system, and the
//! trusted roots come with each request.
//!
//! From the file's directory up, it asks the metadata of each directory and of `.git`, `.jj`
//! and `.hg` in it, without following a link: a link is no marker and no way up. It stops at
//! the home directory, which is never a project, at a directory another user owns (root
//! aside), and where the file system changes, as git does. Windows has no owner or device to
//! go by, so there only the home directory and a link stop it. It reads a worktree's `.git`,
//! at most [`MAX_GIT_FILE`] bytes, and keeps what that names by its words alone. Nothing is
//! run (security.md, T1).
//!
//! A worktree's claim is checked only when its words name a root the record of trusted
//! projects holds, matched by the words before anything is looked up, so no path a repository
//! writes is looked up unless the person trusted it. Then it reads the main repository's
//! record of the worktree, `<main>/.git/worktrees/<name>/gitdir`, which must name this root's
//! own `.git` back. Neither the worktree's place nor its `.git` may be a link.
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
use oso_core::project::{
    Found, MARKERS, MAX_GIT_FILE, RootIdentity, same_place, worktree_backlink, worktree_claim,
    worktree_entry,
};

/// A request answered: the project found, or none.
pub type Answer = (ProjectRequest, Option<Found>);

/// The roots the record of trusted projects held when a request was made, by path.
pub type Trusted = Arc<[PathBuf]>;

/// The thread that finds projects, started with the first request.
pub struct Detector {
    home: Option<PathBuf>,
    wake: Arc<dyn Fn() -> bool + Send + Sync>,
    requests: Option<Sender<(ProjectRequest, Trusted)>>,
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

    /// Finds the project of `request`'s file, on the thread, checking a worktree's claim
    /// only against the roots `trusted`.
    pub fn find(&mut self, request: ProjectRequest, trusted: Trusted) {
        if self.requests.is_none() {
            self.requests = self.start();
        }
        if let Some(requests) = &self.requests
            && requests.send((request, trusted)).is_err()
        {
            self.requests = None;
        }
    }

    /// The answers that came since the last call.
    pub fn take(&self) -> Vec<Answer> {
        std::mem::take(&mut *lock(&self.answers))
    }

    fn start(&self) -> Option<Sender<(ProjectRequest, Trusted)>> {
        let (requests, received) = mpsc::channel::<(ProjectRequest, Trusted)>();
        let answers = Arc::clone(&self.answers);
        let wake = Arc::clone(&self.wake);
        let home = self.home.clone();
        let started = thread::Builder::new()
            .name("oso-project".into())
            .spawn(move || {
                // The home directory as the person's environment names it, not a repository.
                let home = home.and_then(|home| fs::canonicalize(home).ok());
                for (request, trusted) in received {
                    // A panic leaves the file in no project, and the thread goes on.
                    let found = std::panic::catch_unwind(|| {
                        detect(&request.location, home.as_deref(), &trusted)
                    })
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
/// there yet, with a worktree's claim checked if it names a root of `trusted`. `None` if no
/// directory on the way up holds a marker before the walk stops.
pub fn detect(location: &Path, home: Option<&Path>, trusted: &[PathBuf]) -> Option<Found> {
    for Above {
        dir,
        metadata,
        home,
    } in upward(location, home)
    {
        // The home directory is never a project.
        if home {
            return None;
        }
        match markers_in(dir) {
            Marker::None => {}
            Marker::Plain => return Some(found(dir, &metadata, None)),
            Marker::GitFile => {
                let git_file = read_short(&dir.join(".git")).ok();
                let git_file = git_file.as_deref().unwrap_or_default();
                let claim = worktree_claim(dir, git_file);
                let checked = claim
                    .as_deref()
                    .and_then(|claim| check_main(dir, git_file, claim, trusted));
                let mut found = found(dir, &metadata, claim);
                found.checked_main = checked;
                return Some(found);
            }
        }
    }
    None
}

/// A directory on the way up from a file, as [`upward`] finds it.
pub struct Above<'a> {
    pub dir: &'a Path,
    /// Its metadata, not following a link.
    pub metadata: Metadata,
    /// Whether it is the home directory, after which the way up ends.
    pub home: bool,
}

/// The directories above the file at `location`, the real place a path led, from its own up,
/// as project detection and EditorConfig both walk them (ADR 0114, ADR 0115). The way ends at
/// a link or anything but a directory, at a directory another user owns (root aside), where
/// the file system changes, and after the home directory, `home`. Directories missing below
/// the first one there, as a new file's may be, are passed over. Windows has no owner or
/// device to go by, so there only the home directory and a link end it.
pub fn upward<'a>(location: &'a Path, home: Option<&'a Path>) -> impl Iterator<Item = Above<'a>> {
    let mut device = None;
    let mut ended = false;
    location
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .map_while(move |dir| {
            if ended {
                return None;
            }
            let metadata = match fs::symlink_metadata(dir) {
                Ok(metadata) => metadata,
                // The directories of a new file may not exist yet.
                Err(e) if e.kind() == io::ErrorKind::NotFound && device.is_none() => {
                    return Some(None);
                }
                Err(_) => return None,
            };
            if !metadata.is_dir() || !ours(&metadata) {
                return None;
            }
            let here = device_of(&metadata);
            if *device.get_or_insert(here) != here {
                return None;
            }
            ended = Some(dir) == home;
            Some(Some(Above {
                dir,
                metadata,
                home: ended,
            }))
        })
        .flatten()
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
    // `.git` is the first marker, and the only one that may be a file.
    let [git, others @ ..] = MARKERS;
    match entry(git) {
        Some(git) if git.is_dir() => return Marker::Plain,
        Some(git) if git.is_file() => return Marker::GitFile,
        _ => {}
    }
    if others
        .into_iter()
        .any(|name| entry(name).is_some_and(|marker| marker.is_dir()))
    {
        return Marker::Plain;
    }
    Marker::None
}

/// The main repository the worktree at `root`, whose `.git` file holds `git_file` and claims
/// `claim`, belongs with: the claim names a root of `trusted`, and that repository records this
/// worktree back, with neither `root` nor its `.git` a link. Nothing is looked up before the
/// words match, so a path a repository wrote is looked up only under a root the person trusted,
/// and then by the record's spelling of it, never the claim's.
fn check_main(
    root: &Path,
    git_file: &[u8],
    claim: &Path,
    trusted: &[PathBuf],
) -> Option<RootIdentity> {
    // The root as the record spells it, which on Windows is verbatim where git's words are not.
    let main = trusted.iter().find(|known| same_place(known, claim))?;
    let name = worktree_entry(root, git_file)?.file_name()?.to_owned();
    let entry = main.join(".git").join("worktrees").join(name);
    // The main repository is looked at before and after its record is read, so that one put
    // in its place for the read, and moved away again, is no record of it.
    let before = fs::symlink_metadata(main).ok()?;
    let backlink = read_short(&entry.join("gitdir")).ok()?;
    let own = root.join(".git");
    if !same_place(&worktree_backlink(&entry, &backlink)?, &own) {
        return None;
    }
    // Looked at again after the reads, which follow links: the place and its `.git` must still
    // be what the walk found, a directory and a file, neither a link.
    let place = fs::symlink_metadata(root).ok()?;
    let git = fs::symlink_metadata(&own).ok()?;
    if !place.is_dir() || !git.is_file() {
        return None;
    }
    let after = fs::symlink_metadata(main).ok()?;
    if !after.is_dir() || !same_node(&before, &after) {
        return None;
    }
    Some(found(main, &after, None).root)
}

/// At most one byte more than [`MAX_GIT_FILE`] of a short file git writes, so that a longer one
/// is refused by its parser, never waiting on a pipe or a device. The file read must be the
/// one at `path` after the read, and no link.
fn read_short(path: &Path) -> io::Result<Vec<u8>> {
    let not_plain = || io::Error::new(io::ErrorKind::InvalidInput, "not a plain file");
    let Some((file, opened)) = oso_platform::file_meta::open_for_reading(path)? else {
        return Err(not_plain());
    };
    let mut bytes = Vec::new();
    Read::take(file, MAX_GIT_FILE + 1).read_to_end(&mut bytes)?;
    let now = fs::symlink_metadata(path)?;
    if !now.is_file() || !same_node(&now, &opened) {
        return Err(not_plain());
    }
    Ok(bytes)
}

/// Whether two sets of metadata are of one file: its device and inode, and where the system
/// keeps those unstable, its size and modification time.
fn same_node(a: &Metadata, b: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev() && a.ino() == b.ino()
    }
    #[cfg(not(unix))]
    {
        a.len() == b.len() && a.modified().ok() == b.modified().ok()
    }
}

/// Whether this user or root owns what `metadata` describes, as git's `safe.directory` asks.
pub(crate) fn ours(metadata: &Metadata) -> bool {
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
        checked_main: None,
    }
}

#[cfg(test)]
mod tests;
