//! The agent channel never saves into oso's configuration directory (ADR 0108): a save made
//! for the channel asks, on its saving thread right before it writes, whether its target is in
//! that directory, by the target's canonical path and by device and inode, so that a symbolic
//! link or a hard link to a file there is caught as well. It runs the channel's saves, and
//! says what the channel hears of one that failed.
//!
//! Must not depend on: the editor or `app`; this asks the environment and the file system only.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;

use oso_core::editor::SaveJob;

use super::{SaveError, SaveReport, run_with, write};

/// Runs `job`, a save made for the agent channel, as [`super::spawn`] does, refusing it right before
/// it would write if its target is in oso's configuration directory (ADR 0108). The check
/// asks the disk, so it is made here, on the saving thread, fresh each time.
///
/// # Errors
///
/// If the thread cannot start. The job is gone, and nothing was written.
pub fn spawn_for_agent<R>(job: SaveJob, report: R) -> io::Result<()>
where
    R: FnOnce(SaveReport) + Send + 'static,
{
    thread::Builder::new()
        .name("oso-save".into())
        .spawn(move || report(run_for_agent(job, config_home().as_deref())))
        .map(drop)
}

/// Runs `job` for the agent channel here and now, with `directory` as oso's configuration
/// directory.
fn run_for_agent(job: SaveJob, directory: Option<&Path>) -> SaveReport {
    run_with(job, |job| {
        if refuse(job, directory)? {
            return Err(SaveError::IntoConfiguration);
        }
        write(job)
    })
}

impl SaveReport {
    /// What the agent channel hears of a save made for it that failed, if it did: never the
    /// path, which the person's echo area names.
    pub fn agent_failure(&self) -> Option<&'static str> {
        Some(match self.outcome.as_ref().err()? {
            SaveError::IntoConfiguration => {
                "refused: the file is in oso's configuration directory, where the agent never saves"
            }
            SaveError::Conflict(_) => "not saved: the file changed on disk since it was read",
            SaveError::Elsewhere => "not saved: the path now leads somewhere else",
            _ => "not saved; the person's echo area says why",
        })
    }
}

/// The most entries of the configuration directory looked through for another name of a file.
#[cfg(unix)]
const MAX_ENTRIES: usize = 1000;

/// How deep into the configuration directory that look goes.
#[cfg(unix)]
const MAX_DEPTH: usize = 4;

/// oso's configuration directory, asking the environment and not the disk:
/// `$XDG_CONFIG_HOME/oso` when that is set and absolute, else `~/.config/oso`, and
/// `%APPDATA%\oso` on Windows (ADR 0108). `None` with no absolute base to go by.
///
/// For now this is the tip crate's own: ADR 0108's step 3 adds
/// `oso_platform::privacy::config_home()`, which replaces it.
pub fn config_home() -> Option<PathBuf> {
    config_home_from(&|name| std::env::var_os(name))
}

fn config_home_from(var: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |name: &str| {
        var(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    if cfg!(windows) {
        return Some(absolute("APPDATA")?.join("oso"));
    }
    match absolute("XDG_CONFIG_HOME") {
        Some(base) => Some(base.join("oso")),
        None => Some(absolute("HOME")?.join(".config").join("oso")),
    }
}

/// Why `job`, a save for the agent channel, must not be written, if it must not: its target
/// is in `directory`, oso's configuration directory. A save of the buffer's file goes where
/// that file was found to be; one to a new place goes to the path given.
///
/// # Errors
///
/// When what the check needs could not be read: the save is then not made either.
pub fn refuse(job: &SaveJob, directory: Option<&Path>) -> io::Result<bool> {
    let Some(directory) = directory else {
        return Ok(false);
    };
    let target = match (&job.elsewhere, &job.baseline) {
        (None, Some(baseline)) => baseline.location.as_path(),
        _ => job.path.as_path(),
    };
    is_inside(target, directory)
}

/// Whether `target`, a file that may not exist yet, is in `directory` or below it: by its
/// canonical path, by the device and inode of what holds it, or, for a file with more than
/// one name, as another name of a file there. A directory that does not exist holds nothing.
///
/// # Errors
///
/// What the file system said, where it could not tell.
pub fn is_inside(target: &Path, directory: &Path) -> io::Result<bool> {
    let directory = match fs::canonicalize(directory) {
        Ok(directory) => directory,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let target = resolved(target)?;
    if target.starts_with(&directory) {
        return Ok(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let held = fs::metadata(&directory)?;
        let identity = (held.dev(), held.ino());
        // Another spelling of the directory, as through a bind mount, holds the same inode.
        for ancestor in target.ancestors().skip(1) {
            let meta = fs::metadata(ancestor)?;
            if (meta.dev(), meta.ino()) == identity {
                return Ok(true);
            }
        }
        // A hard link elsewhere to a file in the directory is that file by another name.
        match fs::metadata(&target) {
            Ok(meta) if meta.nlink() > 1 => {
                return holds_file(&directory, (meta.dev(), meta.ino()));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// `target` with every link on its way followed, its own name kept if it does not exist.
fn resolved(target: &Path) -> io::Result<PathBuf> {
    match fs::canonicalize(target) {
        Ok(resolved) => Ok(resolved),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let (Some(parent), Some(name)) = (target.parent(), target.file_name()) else {
                return Err(e);
            };
            Ok(fs::canonicalize(parent)?.join(name))
        }
        Err(e) => Err(e),
    }
}

/// Whether `directory` holds, at most [`MAX_DEPTH`] down and among its first [`MAX_ENTRIES`]
/// entries, a file whose device and inode are `identity`. Links are not followed: what one
/// leads to is judged where it is.
#[cfg(unix)]
fn holds_file(directory: &Path, identity: (u64, u64)) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let mut pending = vec![(directory.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            seen += 1;
            if seen > MAX_ENTRIES {
                // Too many to tell: refused, as what it might hold cannot be ruled out.
                return Ok(true);
            }
            let entry = entry?;
            let meta = fs::symlink_metadata(entry.path())?;
            if meta.is_dir() && depth < MAX_DEPTH {
                pending.push((entry.path(), depth + 1));
            } else if meta.is_file() && (meta.dev(), meta.ino()) == identity {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
