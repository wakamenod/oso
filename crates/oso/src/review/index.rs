//! Throwaway copies of a repository's index for a review (ADR 0122), so that listing the work
//! tree with its untracked files, and taking a baseline, leave the person's index untouched.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this copies files in a
//! private directory on the review's thread.
//!
//! - A copy keeps the index's modification time, so git's stat cache in it still applies and a
//!   racily clean entry is checked, not missed.
//! - Copies live in `reviews/tmp/` under the state directory, private to the user, as an index
//!   names every path of the repository. A copy is removed when it is dropped, with the lock
//!   git leaves beside it; copies an earlier run left behind are swept when a review's thread
//!   starts.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use oso_platform::privacy::PrivateDir;

/// What every copy's name starts with.
const PREFIX: &str = "index-";
/// How old a copy left behind must be before a sweep takes it: older than any run can last,
/// so that another review's copy in use is never taken.
const STALE: Duration = Duration::from_secs(60 * 60);
/// How many names are tried before giving up on making a copy.
const TRIES: u32 = 1000;

/// The private directory copies are made in.
#[derive(Debug)]
pub(crate) struct Scratch {
    dir: PrivateDir,
}

impl Scratch {
    /// `reviews/tmp/` under `state`, oso's directory in the per-user state directory, made
    /// private if missing, with old copies swept.
    pub(crate) fn open(state: &Path) -> io::Result<Scratch> {
        let dir = PrivateDir::open(state.join("reviews").join("tmp"))?;
        sweep(dir.path());
        Ok(Scratch { dir })
    }

    /// A copy of the index at `index`, keeping its modification time. With no index there
    /// (a repository with nothing added yet), the copy names a file that does not exist, which
    /// git reads as an empty index.
    pub(crate) fn copy(&self, index: &Path) -> io::Result<IndexCopy> {
        let (path, mut copy) = self.create()?;
        let copy_guard = IndexCopy { path };
        let mut real = match File::open(index) {
            Ok(real) => real,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                drop(copy);
                fs::remove_file(&copy_guard.path)?;
                return Ok(copy_guard);
            }
            Err(e) => return Err(e),
        };
        let metadata = real.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("the index is not a file"));
        }
        io::copy(&mut real, &mut copy)?;
        copy.set_modified(metadata.modified()?)?;
        Ok(copy_guard)
    }

    /// A new file of the user's alone, by a name no other copy has.
    fn create(&self) -> io::Result<(PathBuf, File)> {
        let pid = std::process::id();
        for n in 0..TRIES {
            let path = self.dir.path().join(format!("{PREFIX}{pid}-{n}"));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => return Ok((path, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("no free name for an index copy"))
    }
}

/// A copy of an index, removed when dropped.
#[derive(Debug)]
pub(crate) struct IndexCopy {
    path: PathBuf,
}

impl IndexCopy {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IndexCopy {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(lock_of(&self.path));
    }
}

/// The lock git takes beside an index.
fn lock_of(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}

/// Removes copies, and their locks, that were made long enough ago that no run still uses
/// them. A copy keeps the index's modification time, so its age is its status change time
/// where the system has one, else its creation time.
fn sweep(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PREFIX) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let stale = made(&metadata)
            .and_then(|made| now.duration_since(made).ok())
            .is_some_and(|age| age > STALE);
        if metadata.is_file() && stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(unix)]
fn made(metadata: &fs::Metadata) -> Option<SystemTime> {
    use std::os::unix::fs::MetadataExt;
    let seconds = u64::try_from(metadata.ctime()).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

#[cfg(not(unix))]
fn made(metadata: &fs::Metadata) -> Option<SystemTime> {
    metadata.created().ok()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::Scratch;
    use crate::review::tests::scratch_dir;

    #[test]
    fn a_copy_keeps_the_modification_time_and_goes_when_dropped() {
        let dir = scratch_dir("index-copy");
        let index = dir.join("index");
        fs::write(&index, b"DIRC").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(&index)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let scratch = Scratch::open(&dir.join("state")).unwrap();
        let copy = scratch.copy(&index).unwrap();
        let metadata = fs::metadata(copy.path()).unwrap();
        assert_eq!(metadata.modified().unwrap(), old);
        assert_eq!(fs::read(copy.path()).unwrap(), b"DIRC");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        let path = copy.path().to_owned();
        drop(copy);
        assert!(!path.exists());
        // No index yet: the copy names nothing, which git reads as empty.
        let none = scratch.copy(&dir.join("missing")).unwrap();
        assert!(!none.path().exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
