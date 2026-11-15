//! What the directory editor does to files (ADR 0134, step 1b-2): copying, renaming or moving,
//! deleting and making directories, each run on the `oso-files` thread from a plan the UI makes.
//!
//! Must not depend on: `app`, `dispatch`, `commands`, `lister` or winit. It runs on its own
//! thread; the UI thread never touches the file system.
//!
//! - Every operation acts from the directory as listed: it is opened again by its path and must
//!   be, by device and inode, the one listed, or nothing is done. Each source is reached from it
//!   by one name, following no link: a link is copied or moved as a link.
//! - A target is the path the person typed, taken as typed: a link they named is followed.
//! - What exists at a target is found first, and nothing is written over it unless the person
//!   said yes to exactly those names. A directory is never written over a file, or a file over a
//!   directory; a directory copied onto one of its name merges into it, as `cp -R`.
//! - No operation goes into another file system: a directory on another device stops it, named.
//! - Each row is acted on as the kind it was listed as: one that turned from a file into a
//!   directory, or back, since it was listed stops the operation.
//! - A move across file systems copies, then removes only what it copied, each directory last
//!   and only once empty, and nothing if any entry could not be copied.
//! - It stops at the first error, or once asked to between two files, and says how far it got.
//!   Names in what it says are escaped as a listing's rows are.
//! - On Windows, where a directory is reached by its path, only `+` runs: copying, moving and
//!   deleting wait until handles that cannot be redirected are used there.

pub(crate) mod worker;

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};

use oso_core::editor::row_text;
use oso_platform::file_meta::{Directory, EntryKind, MAX_DEPTH};

/// What an operation does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Copy,
    /// Rename or move: across file systems, a copy, then what was copied removed.
    Rename,
    Delete,
    /// Make the directory `target`, with its parents.
    Mkdir,
}

/// An operation as the UI asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) kind: Kind,
    /// The directory listed, and its device and inode when listed.
    pub(crate) directory: PathBuf,
    pub(crate) identity: Option<(u64, u64)>,
    /// The names it acts on, each one entry of `directory`, and whether each was listed as a
    /// directory.
    pub(crate) names: Vec<OsString>,
    pub(crate) directories: Vec<bool>,
    /// Where they go, as typed: a directory, or for one name a new path. For `Mkdir`, the
    /// directory to make.
    pub(crate) target: PathBuf,
    /// The targets the person agreed may be written over, which must be those found again.
    pub(crate) overwrite: Option<Vec<PathBuf>>,
}

/// A target that is there already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Existing {
    pub(crate) path: PathBuf,
    /// A directory, which a copy merges into: files of the same names in it are written over.
    pub(crate) directory: bool,
}

/// What an operation came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Outcome {
    /// Entries copied, moved, deleted or made.
    pub(crate) done: usize,
    /// Why it stopped early, if it did, its names escaped.
    pub(crate) error: Option<String>,
    /// Targets that exist, which nothing was written over: the person is asked about them.
    pub(crate) existing: Vec<Existing>,
    /// Each source moved, and its real place now, for the buffers visiting files there. A link
    /// moved is not among them: what visits through it did not move.
    pub(crate) moved: Vec<(PathBuf, PathBuf)>,
    /// The directories whose listings changed.
    pub(crate) touched: Vec<PathBuf>,
    /// Pipes, sockets and devices left as they were.
    pub(crate) skipped: Vec<OsString>,
}

/// What an operation tells as it goes: each entry done, and whether to stop.
pub(crate) struct Progress<'a> {
    pub(crate) stop: &'a dyn Fn() -> bool,
    pub(crate) each: &'a mut dyn FnMut(usize),
}

impl Progress<'_> {
    fn check(&self) -> io::Result<()> {
        if (self.stop)() {
            Err(io::Error::new(io::ErrorKind::Interrupted, "stopped"))
        } else {
            Ok(())
        }
    }
}

/// `name` as an error says it.
fn named(name: &OsStr) -> String {
    row_text(&name.to_string_lossy())
}

/// `path` as an error says it.
fn path_named(path: &Path) -> String {
    row_text(&path.to_string_lossy())
}

/// Runs `plan`.
pub(crate) fn run(plan: &Plan, progress: &mut Progress<'_>) -> Outcome {
    let mut outcome = Outcome::default();
    let ran = match plan.kind {
        Kind::Mkdir => make_directories(&plan.target, &mut outcome),
        _ if cfg!(windows) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "copying, moving and deleting from a listing do not run on Windows yet",
        )),
        _ => listed(plan).and_then(|from| match plan.kind {
            Kind::Delete => delete(plan, &from, progress, &mut outcome),
            _ => copy_or_move(plan, &from, progress, &mut outcome),
        }),
    };
    if let Err(error) = ran {
        outcome.error = Some(error.to_string());
    }
    outcome
}

/// The directory listed, opened again, if it is still the one listed.
fn listed(plan: &Plan) -> io::Result<Directory> {
    let directory = Directory::open(&plan.directory)?;
    match (plan.identity, directory.identity()?) {
        (Some(listed), Some(now)) if listed != now => Err(io::Error::other(
            "this directory changed since it was listed: g lists it again",
        )),
        _ => Ok(directory),
    }
}

/// What `name` is now, if it is the kind it was listed as.
fn as_listed(from: &Directory, name: &OsStr, directory: bool) -> io::Result<EntryKind> {
    let kind = from.kind_of(name)?;
    if (kind == EntryKind::Directory) != directory {
        return Err(io::Error::other(format!(
            "{} changed since it was listed: g lists it again",
            named(name)
        )));
    }
    Ok(kind)
}

/// Where each name goes: into the target if it is a directory, else, for one name alone, to the
/// target's own name in its parent.
fn destinations(plan: &Plan) -> io::Result<(Directory, PathBuf, Vec<OsString>)> {
    match Directory::open(&plan.target) {
        Ok(into) => Ok((into, plan.target.clone(), plan.names.clone())),
        Err(_) if plan.names.len() == 1 => {
            let (Some(parent), Some(name)) = (plan.target.parent(), plan.target.file_name()) else {
                return Err(io::Error::other("no directory to put it in"));
            };
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            Ok((
                Directory::open(parent)?,
                parent.to_path_buf(),
                vec![name.to_os_string()],
            ))
        }
        Err(error) => Err(io::Error::other(format!(
            "{} is no directory to put several files in: {error}",
            path_named(&plan.target)
        ))),
    }
}

/// Refuses to put a directory inside itself, which would copy its copies until the depth ran
/// out: by their real places.
fn not_into_itself(source: &Path, into: &Path) -> io::Result<()> {
    let (Ok(source), Ok(into)) = (std::fs::canonicalize(source), std::fs::canonicalize(into))
    else {
        return Ok(());
    };
    if into.starts_with(&source) {
        return Err(io::Error::other(format!(
            "{} would go inside itself",
            path_named(&source)
        )));
    }
    Ok(())
}

fn copy_or_move(
    plan: &Plan,
    from: &Directory,
    progress: &mut Progress<'_>,
    outcome: &mut Outcome,
) -> io::Result<()> {
    let (into, into_path, targets) = destinations(plan)?;
    // What is there already, and whether each source can go there at all.
    let mut existing = Vec::new();
    for ((name, target), directory) in plan.names.iter().zip(&targets).zip(&plan.directories) {
        let source = as_listed(from, name, *directory)?;
        let source_is_directory = source == EntryKind::Directory;
        if source_is_directory {
            not_into_itself(&plan.directory.join(name), &into_path)?;
        }
        match into.kind_of(target) {
            Ok(there) => {
                if source_is_directory != (there == EntryKind::Directory) {
                    return Err(io::Error::other(format!(
                        "{} is there, and one is a directory and the other not",
                        path_named(&into_path.join(target))
                    )));
                }
                if source_is_directory && plan.kind == Kind::Rename {
                    return Err(io::Error::other(format!(
                        "{} is a directory there already: not moved over it",
                        path_named(&into_path.join(target))
                    )));
                }
                existing.push(Existing {
                    path: into_path.join(target),
                    directory: source_is_directory,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let paths: Vec<PathBuf> = existing.iter().map(|there| there.path.clone()).collect();
    if !existing.is_empty() && plan.overwrite.as_ref() != Some(&paths) {
        outcome.existing = existing;
        return Ok(());
    }
    outcome.touched.push(into_path.clone());
    if plan.kind == Kind::Rename {
        outcome.touched.push(plan.directory.clone());
    }
    let device = from.identity()?.map(|(device, _)| device);
    for ((name, target), directory) in plan.names.iter().zip(&targets).zip(&plan.directories) {
        progress.check()?;
        let kind = as_listed(from, name, *directory)?;
        // Only what the person agreed to is written over: a target that appeared since is not.
        let replace = paths.contains(&into_path.join(target));
        match plan.kind {
            Kind::Rename => {
                move_one(from, name, (&into, target), replace, progress, outcome)?;
                if kind != EntryKind::Link {
                    let moved = into_path.join(target);
                    let real = std::fs::canonicalize(&moved).unwrap_or(moved);
                    outcome.moved.push((plan.directory.join(name), real));
                }
            }
            _ => {
                let mut copied = Vec::new();
                let copying = (replace, device, MAX_DEPTH);
                copy_one(
                    from,
                    name,
                    (&into, target),
                    copying,
                    progress,
                    (outcome, &mut copied),
                )?;
            }
        }
    }
    Ok(())
}

/// Moves `name` by one rename, or across file systems by a copy, then what was copied removed.
fn move_one(
    from: &Directory,
    name: &OsStr,
    (into, target): (&Directory, &OsStr),
    replace: bool,
    progress: &mut Progress<'_>,
    outcome: &mut Outcome,
) -> io::Result<()> {
    match from.rename_to(name, into, target, replace) {
        Ok(()) => {
            outcome.done += 1;
            (progress.each)(outcome.done);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            let device = from.identity()?.map(|(device, _)| device);
            let skipped = outcome.skipped.len();
            let mut copied = Vec::new();
            let copying = (replace, device, MAX_DEPTH);
            copy_one(
                from,
                name,
                (into, target),
                copying,
                progress,
                (outcome, &mut copied),
            )?;
            if outcome.skipped.len() > skipped {
                return Err(io::Error::other(format!(
                    "{} holds what cannot be copied: copied, and left where it was",
                    named(name)
                )));
            }
            remove_copied(from, &copied)
        }
        Err(error) => Err(error),
    }
}

/// Removes what a copy copied, each entry by its names from `from`, children before the
/// directory that held them, which goes only once empty: what appeared in it since stays.
fn remove_copied(from: &Directory, copied: &[Vec<OsString>]) -> io::Result<()> {
    for names in copied {
        let Some((last, above)) = names.split_last() else {
            continue;
        };
        let mut opened: Option<Directory> = None;
        for name in above {
            let next = opened.as_ref().unwrap_or(from).open_directory(name)?;
            let Some(next) = next else {
                return Err(io::Error::other(format!(
                    "{} changed while it was moved",
                    named(name)
                )));
            };
            opened = Some(next);
        }
        opened.as_ref().unwrap_or(from).remove(last)?;
    }
    Ok(())
}

/// Copies `name` as it is, never following it: a file, a link as a link, or a directory with
/// all it holds, into one there of its name if there is one. Pipes, sockets and devices are
/// left, and named. Each entry copied goes into `copied` by its names from where the copy
/// started, after what it holds.
fn copy_one(
    from: &Directory,
    name: &OsStr,
    (into, target): (&Directory, &OsStr),
    (replace, device, depth): (bool, Option<u64>, usize),
    progress: &mut Progress<'_>,
    (outcome, copied): (&mut Outcome, &mut Vec<Vec<OsString>>),
) -> io::Result<()> {
    copy_under(
        &mut Vec::new(),
        from,
        name,
        (into, target),
        (replace, device, depth),
        progress,
        (outcome, copied),
    )
}

fn copy_under(
    above: &mut Vec<OsString>,
    from: &Directory,
    name: &OsStr,
    (into, target): (&Directory, &OsStr),
    (replace, device, depth): (bool, Option<u64>, usize),
    progress: &mut Progress<'_>,
    (outcome, copied): (&mut Outcome, &mut Vec<Vec<OsString>>),
) -> io::Result<()> {
    progress.check()?;
    match from.kind_of(name)? {
        EntryKind::File => from.copy_file_to(name, into, target, replace)?,
        EntryKind::Link => from.copy_link_to(name, into, target, replace)?,
        EntryKind::Other => {
            outcome.skipped.push(name.to_os_string());
            return Ok(());
        }
        EntryKind::Directory => {
            let Some(source) = from.open_directory(name)? else {
                return Err(io::Error::other("a directory turned into something else"));
            };
            if device.is_some() && source.identity()?.map(|(device, _)| device) != device {
                return Err(io::Error::other(format!(
                    "{} is on another file system: not gone into",
                    named(name)
                )));
            }
            if depth == 0 {
                return Err(io::Error::other("directories go deeper than may be copied"));
            }
            let mode = mode_of(&source);
            let Some(made) = into.make_directory(target, mode)? else {
                return Err(io::Error::other(format!(
                    "{} is there and is no directory",
                    named(target)
                )));
            };
            let names: Vec<OsString> = source
                .entries()?
                .map(|entry| entry.map(|entry| entry.name))
                .collect::<io::Result<_>>()?;
            above.push(name.to_os_string());
            for entry in names {
                let copying = (replace, device, depth - 1);
                copy_under(
                    above,
                    &source,
                    &entry,
                    (&made, &entry),
                    copying,
                    progress,
                    (&mut *outcome, &mut *copied),
                )?;
            }
            above.pop();
        }
    }
    let mut names = above.clone();
    names.push(name.to_os_string());
    copied.push(names);
    outcome.done += 1;
    (progress.each)(outcome.done);
    Ok(())
}

/// A directory's permission bits, which its copy is made with, less the umask.
fn mode_of(directory: &Directory) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        directory
            .metadata()
            .map_or(0o700, |metadata| metadata.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        0o777
    }
}

fn delete(
    plan: &Plan,
    from: &Directory,
    progress: &mut Progress<'_>,
    outcome: &mut Outcome,
) -> io::Result<()> {
    outcome.touched.push(plan.directory.clone());
    for (name, directory) in plan.names.iter().zip(&plan.directories) {
        progress.check()?;
        // A file the question named is never taken as a directory and all it holds.
        as_listed(from, name, *directory)?;
        let done = &mut outcome.done;
        let each = &mut *progress.each;
        from.remove_tree(name, progress.stop, &mut || {
            *done += 1;
            each(*done);
        })?;
    }
    Ok(())
}

/// Makes `target` and the directories above it that are missing, from the nearest one there,
/// each with the umask's mode.
fn make_directories(target: &Path, outcome: &mut Outcome) -> io::Result<()> {
    let mut missing = Vec::new();
    let mut at = target;
    let mut base = loop {
        match Directory::open(at) {
            Ok(directory) => break directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (Some(parent), Some(name)) = (at.parent(), at.file_name()) else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                at = parent;
            }
            Err(error) => return Err(error),
        }
    };
    if missing.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is there already", path_named(target)),
        ));
    }
    outcome.touched.push(at.to_path_buf());
    for name in missing.iter().rev() {
        let Some(made) = base.make_directory(name, 0o777)? else {
            return Err(io::Error::other(format!(
                "{} is there and is no directory",
                named(name)
            )));
        };
        base = made;
        outcome.done += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
