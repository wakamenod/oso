//! What an agent's session tool needs to know before the person is asked (ADR 0121, step 6):
//! whether a worktree session's branch is new and where it would start, and whether a session's
//! directory is the repository's own or one of its linked worktrees. So the question names what
//! will happen, and a call that cannot happen is refused before anyone is asked.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread; it runs git, on a job.

use std::path::{Path, PathBuf};

use super::create::{base_of, checked_branch, refuse_checked_out};
use super::git::{Repo, run};
use super::porcelain::printable;
use super::slug;
use crate::review::git::MAX_SHORT;

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
    let branch = checked_branch(repo, branch)?;
    refuse_checked_out(repo, &branch.name)?;
    let shown = printable(&branch.name, 120);
    let start = match (&branch.tip, base) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "Branch {shown} exists; a base is only for a new branch"
            ));
        }
        (Some(_), None) => None,
        (None, Some(base)) => Some(base_of(repo, Some(base))?),
        (None, None) => Some(caller_head(repo, caller)?),
    };
    let slug = slug::slug(&branch.name).ok_or("That branch makes no directory name")?;
    let place = slug::place(repo.main(), &slug, |p| p.symlink_metadata().is_ok())
        .ok_or("Ten worktrees of that name exist already")?;
    Ok(Prepared::Worktree {
        place,
        branch: branch.name,
        start: start.map(|(name, id)| (id, name)),
    })
}

/// The commit the caller's directory is on, and how the question shows it: its branch, else
/// the commit's first 12 digits.
fn caller_head(repo: &Repo, caller: &Path) -> Result<(String, String), String> {
    let git = if caller == repo.main() {
        Some(repo.git().clone())
    } else {
        repo.in_worktree(caller)
    };
    let git = git.ok_or("Your session's directory is not this repository's")?;
    let text = |args: &[&str]| {
        run(&git, args, MAX_SHORT)
            .ok()
            .and_then(|out| String::from_utf8(out).ok())
            .map(|out| out.trim().to_owned())
            .filter(|out| !out.is_empty())
    };
    let id = text(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .filter(|id| id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("Your session's directory has no commit to start a branch from")?;
    let name = text(&["symbolic-ref", "--quiet", "--short", "HEAD"])
        .unwrap_or_else(|| id.chars().take(12).collect());
    Ok((printable(&name, 120), id))
}
