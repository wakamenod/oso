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

/// A space's directory made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Made {
    pub path: PathBuf,
    pub identity: Option<(u64, u64)>,
    /// The name it was made under: the space's, or one with `-2` and on when that was taken
    /// on the disk.
    pub name: String,
}

/// Makes the directory of a space called `name` under `root`, for `members`: under `name`,
/// else the first of `name-2`, `name-3` and on that nothing is at, and that `taken`, the
/// other spaces' names, does not name (ADR 0136, step 1b, P3).
///
/// # Errors
///
/// Why not, for the echo area.
pub fn make_directory(
    root: &Path,
    name: &str,
    members: &[Member],
    taken: &dyn Fn(&str) -> bool,
) -> Result<Made, String> {
    let mut tried: Vec<String> = Vec::new();
    for _ in 0..TRIES {
        let next = unique(name, &|candidate| {
            taken(candidate) || tried.iter().any(|t| t == candidate)
        });
        match dir::make(root, &next, members) {
            Ok((path, identity)) => {
                return Ok(Made {
                    path,
                    identity,
                    name: next,
                });
            }
            Err(Refusal::Exists(_)) => tried.push(next),
            Err(why) => return Err(why.to_string()),
        }
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

/// The file of the instructions of the space with id `id`, in `state`, oso's state directory,
/// whose directory for them is made private if missing.
///
/// # Errors
///
/// Why the directory may not be used, for the echo area.
pub fn instructions_file(state: &Path, id: &str) -> Result<PathBuf, String> {
    let directory = PrivateDir::open(state.join(INSTRUCTIONS)).map_err(|e| e.to_string())?;
    Ok(directory.path().join(format!("{id}.md")))
}

#[cfg(all(test, unix))]
mod tests;
