//! What an agent's session tool needs to know before the person is asked (ADR 0121, step 6):
//! whether a worktree session's branch is new and where it would start, and whether a session's
//! directory is the repository's own or one of its linked worktrees. So the question names what
//! will happen, and a call that cannot happen is refused before anyone is asked.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; it runs git, on a job.

use std::path::{Path, PathBuf};

use super::create::{Resolved, head_of, resolve};
use super::git::Repo;
use super::porcelain::printable;

/// What to look at.
#[derive(Clone, Debug)]
pub enum Prepare {
    /// A worktree session on `branch`, a new one starting at `base`, else at the commit the
    /// caller's directory `caller` is on (the author's choice, step 6).
    Worktree {
        branch: String,
        base: Option<String>,
        caller: PathBuf,
    },
    /// A session in `directory`, which must be the repository's main checkout or one of its
    /// linked worktrees.
    Directory(PathBuf),
}

/// What it found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prepared {
    Worktree {
        /// Where the worktree would go, as it is now: the job that makes it looks again.
        place: PathBuf,
        branch: String,
        /// For a new branch: the commit it starts at, and that commit as the question shows it.
        start: Option<(String, String)>,
    },
    Directory(PathBuf),
}

/// Looks at what `prepare` names in `repo`.
///
/// # Errors
///
/// Why the call cannot happen, in words for the agent.
pub fn prepare(repo: &Repo, prepare: &Prepare) -> Result<Prepared, String> {
    match prepare {
        Prepare::Worktree {
            branch,
            base,
            caller,
        } => worktree(repo, branch, base.as_deref(), caller),
        Prepare::Directory(directory) => {
            if directory == repo.main() || repo.in_worktree(directory).is_some() {
                Ok(Prepared::Directory(directory.clone()))
            } else {
                Err(format!(
                    "{} is neither this repository's root nor one of its linked worktrees",
                    printable(&directory.display().to_string(), 200)
                ))
            }
        }
    }
}

fn worktree(
    repo: &Repo,
    branch: &str,
    base: Option<&str>,
    caller: &Path,
) -> Result<Prepared, String> {
    let Resolved { branch, base, path } = resolve(repo, branch, base, &|| head_of(repo, caller))?;
    Ok(Prepared::Worktree {
        place: path,
        branch: branch.name,
        start: base.map(|(name, id)| (id, name)),
    })
}
