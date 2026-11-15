//! What the space commands look up on the disk, on the sessions worker (ADR 0136, step 1b): a
//! space's directory made under the next free name, its repositories' devices and inodes,
//! whether a path is a repository's main checkout, and the private directory of the spaces'
//! instructions.
//!
//! Must not depend on: threads, the UI or `app`; it touches the disk, so it runs on a worker.

use std::fs;
use std::path::{Path, PathBuf};

use oso_platform::privacy::PrivateDir;

use super::dir::{self, Refusal};
use super::stored::{Member, unique};

/// How many names a space's directory is tried under: its own, then `-2` and on.
const TRIES: usize = 32;

/// The directory under `root` in which spaces' instructions are kept, in oso's state
/// directory, never in a space's own.
const INSTRUCTIONS: &str = "space-instructions";

/// A look at whether a path is a repository, by a count of this instance's, so that its answer
/// finds what asked for it (`space-new`, `space-add-repo`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RepoCheck(pub u64);

/// A space's directory made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub path: PathBuf,
    pub identity: Option<(u64, u64)>,
    /// The name it was made under: the space's, or one with `-2` and on when that was taken
    /// on the disk.
    pub name: String,
    /// The name it was asked for under, so that a space renamed meanwhile keeps its new name.
    pub asked: String,
}

/// Makes the directory of a space called `name` under `root`, for `members`: under `name`,
/// else, unless `exact`, the first of `name-2`, `name-3` and on that nothing is at, and that
/// `taken`, the other spaces' names, does not name (ADR 0136, step 1b, P3).
///
/// # Errors
///
/// Why not, for the echo area.
pub fn make_directory(
    root: &Path,
    name: &str,
    members: &[Member],
    taken: &dyn Fn(&str) -> bool,
    exact: bool,
) -> Result<Made, String> {
    let mut tried: Vec<String> = Vec::new();
    let tries = if exact { 1 } else { TRIES };
    for _ in 0..tries {
        let next = unique(name, &|candidate| {
            taken(candidate) || tried.iter().any(|t| t == candidate)
        });
        // A name taken on the disk is passed over with one look, before the checks.
        if fs::symlink_metadata(dir::place(root, &next)).is_ok() {
            tried.push(next);
            continue;
        }
        match dir::make(root, &next, members) {
            Ok((path, identity)) => {
                return Ok(Made {
                    path,
                    identity,
                    name: next,
                    asked: name.to_owned(),
                });
            }
            Err(Refusal::Exists(_)) => tried.push(next),
            Err(why) => return Err(why.to_string()),
        }
    }
    if exact {
        return Err(format!(
            "space directory refused: {} is there already",
            dir::place(root, name).display()
        ));
    }
    Err(format!(
        "space directory refused: {TRIES} names under {} are taken",
        root.display()
    ))
}

/// The device and inode of each of `members`' roots, where the system has them.
pub fn identities(members: &[Member]) -> Vec<(PathBuf, Option<(u64, u64)>)> {
    members
        .iter()
        .map(|member| {
            let identity = fs::symlink_metadata(&member.root)
                .ok()
                .and_then(|meta| dir::identity(&meta));
            (member.root.clone(), identity)
        })
        .collect()
}

/// The repository whose main checkout is at `path`, by its real path: a directory holding a
/// `.git` directory the person or root owns. A worktree, whose `.git` is a file, is refused
/// with a word on its repository. Nothing is run and nothing in the repository is read.
///
/// # Errors
///
/// Why it is not one, for the echo area.
pub fn main_checkout(path: &Path) -> Result<Member, String> {
    let shown = path.display();
    let real = fs::canonicalize(path).map_err(|e| format!("{shown}: {e}"))?;
    let meta = fs::symlink_metadata(&real).map_err(|e| format!("{shown}: {e}"))?;
    if !meta.is_dir() {
        return Err(format!("{shown} is no directory"));
    }
    // The home directory, one above it, and the root are no project's (ADR 0114): a space's
    // orchestrator would reach all of them through `--add-dir`.
    if holds_home(&real) {
        return Err(format!(
            "{shown} holds the home directory; add a repository inside it"
        ));
    }
    match fs::symlink_metadata(real.join(".git")) {
        Ok(git) if git.is_dir() && crate::project::ours(&git) => Ok(Member {
            identity: dir::identity(&meta),
            root: real,
        }),
        Ok(git) if git.is_file() => Err(format!(
            "{shown} is a worktree; add its repository's main checkout"
        )),
        _ => Err(format!("{shown} is no repository's main checkout")),
    }
}

/// Whether the directory at `path`, by its real path, is the root or holds the home directory,
/// which are no project's (ADR 0114): a space's orchestrator would reach all of them through
/// `--add-dir`. One whose real path is unknown is taken as holding it.
pub fn holds_home(path: &Path) -> bool {
    let Ok(real) = fs::canonicalize(path) else {
        return true;
    };
    let home = oso_platform::privacy::home().and_then(|home| fs::canonicalize(home).ok());
    real.parent().is_none() || home.is_some_and(|home| home.starts_with(&real))
}

/// Where the instructions of the space with id `id` are, in `state`, oso's state directory,
/// made or not.
pub fn instructions_path(state: &Path, id: &str) -> PathBuf {
    state.join(INSTRUCTIONS).join(format!("{id}.md"))
}

/// The file of the instructions of the space with id `id`, in `state`, oso's state directory,
/// whose directory for them is made private if missing.
///
/// # Errors
///
/// Why the directory may not be used, for the echo area.
pub fn instructions_file(state: &Path, id: &str) -> Result<PathBuf, String> {
    let directory = PrivateDir::open(state.join(INSTRUCTIONS)).map_err(|e| e.to_string())?;
    let file = directory.path().join(format!("{id}.md"));
    // A link or a second name would show and save another file's text as the instructions.
    match fs::symlink_metadata(&file) {
        Ok(meta) if !meta.is_file() || !oso_platform::privacy::has_one_name(&meta) => {
            Err(format!("{} is not a plain file of its own", file.display()))
        }
        _ => Ok(file),
    }
}

#[cfg(all(test, unix))]
mod tests;
