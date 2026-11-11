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
    /// Whether the save wrote the file: not one that failed, nor one with nothing to write.
    pub fn wrote(&self) -> bool {
        matches!(self.outcome, Ok(Some(_)))
    }

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

/// The most entries of the configuration directory looked through, all levels together.
const MAX_ENTRIES: usize = 1000;

/// How deep into the configuration directory that look goes.
const MAX_DEPTH: usize = 4;

/// The most links followed to resolve one path.
const MAX_LINKS: usize = 40;

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

/// Why `job`, a save for the agent channel, must not be written, if it must not: a name it
/// writes through, or what that name leads to, is in `directory`, oso's configuration
/// directory, or is what an entry of it leads to. A save of the buffer's file writes through
/// the name it was opened by to where that was found to be; one to a new place, to the path
/// given, or the buffer's file's name in it if it is a directory.
///
/// # Errors
///
/// When what the check needs could not be read: the save is then not made either.
pub fn refuse(job: &SaveJob, directory: Option<&Path>) -> io::Result<bool> {
    let Some(directory) = directory else {
        return Ok(false);
    };
    let mut targets = Vec::new();
    match (&job.elsewhere, &job.baseline) {
        (Some(elsewhere), _) => {
            // As `elsewhere::write` works it out.
            let into_directory = fs::metadata(&job.path).is_ok_and(|meta| meta.is_dir());
            targets.push(if into_directory {
                job.path.join(&elsewhere.name)
            } else {
                job.path.clone()
            });
        }
        (None, Some(baseline)) => {
            targets.push(job.path.clone());
            targets.push(baseline.location.clone());
        }
        (None, None) => targets.push(job.path.clone()),
    }
    for target in &targets {
        if is_inside(target, directory)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a save to `target`, a path that may not exist yet, would write into `directory`:
/// the path as given or what it leads to is in it or below it, by canonical path or by the
/// device and inode of what holds it; or it is what an entry of the directory leads to, as a
/// configuration file kept elsewhere and linked into it, by path or by device and inode. A
/// directory that does not exist holds nothing.
///
/// Past [`MAX_ENTRIES`] entries the directory cannot be told apart from what it might lead to,
/// so the answer is yes. A subdirectory that cannot be read is passed over, as nothing in it
/// could be read as configuration either.
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
    // The name as given, with only its directory resolved, and where it finally leads.
    let named = named(target)?;
    let resolved = resolved(target)?;
    if named.starts_with(&directory) || resolved.starts_with(&directory) {
        return Ok(true);
    }
    #[cfg(unix)]
    if held_by(&named, &directory)? || held_by(&resolved, &directory)? {
        return Ok(true);
    }
    leads_there(&directory, &resolved)
}

/// Whether a directory holding `path`, under another spelling, as through a bind mount, is
/// `directory`, by device and inode.
#[cfg(unix)]
fn held_by(path: &Path, directory: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let held = fs::metadata(directory)?;
    for ancestor in path.ancestors().skip(1) {
        match fs::metadata(ancestor) {
            Ok(meta) if (meta.dev(), meta.ino()) == (held.dev(), held.ino()) => return Ok(true),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// Whether an entry of `directory`, at most [`MAX_DEPTH`] down, leads to `target`, a resolved
/// path: by path, by holding it, or, on Unix, by device and inode, as a hard link would.
fn leads_there(directory: &Path, target: &Path) -> io::Result<bool> {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        match fs::metadata(target) {
            Ok(meta) => Some((meta.dev(), meta.ino())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        }
    };
    let mut pending = vec![(directory.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            if depth == 0 {
                // The directory itself: what it holds cannot be ruled out.
                return Ok(true);
            }
            continue;
        };
        for entry in entries {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Ok(true);
            }
            let path = entry?.path();
            let Ok(led) = resolved(&path) else {
                continue;
            };
            if led == target || target.starts_with(&led) {
                return Ok(true);
            }
            #[cfg(unix)]
            if let Some(identity) = identity
                && let Ok(meta) = fs::metadata(&led)
            {
                use std::os::unix::fs::MetadataExt;
                if (meta.dev(), meta.ino()) == identity {
                    return Ok(true);
                }
            }
            if depth < MAX_DEPTH && fs::metadata(&led).is_ok_and(|meta| meta.is_dir()) {
                pending.push((path, depth + 1));
            }
        }
    }
    Ok(false)
}

/// `path` with its directory resolved and its own name kept, whatever that name is.
fn named(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return fs::canonicalize(&path);
    };
    Ok(resolved(parent)?.join(name))
}

/// Where `path` leads, every link on its way followed, a link that leads nowhere yet included;
/// a final name that is not there yet is kept as it is.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    let mut path = std::path::absolute(path)?;
    for _ in 0..MAX_LINKS {
        match fs::canonicalize(&path) {
            Ok(resolved) => return Ok(resolved),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
                    return Err(e);
                };
                let (parent, name) = (parent.to_path_buf(), name.to_os_string());
                match fs::read_link(&path) {
                    // A link to what is not there yet leads there all the same.
                    Ok(to) => path = parent.join(to),
                    Err(_) => return Ok(resolved(&parent)?.join(name)),
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("too many links on the way"))
}

#[cfg(test)]
mod tests;
