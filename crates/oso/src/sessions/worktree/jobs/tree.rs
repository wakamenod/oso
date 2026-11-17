//! The worktrees the tree of `*Spaces*` shows (ADR 0131, step 2): one `git worktree list`
//! of a trusted repository on the pool, each worktree's path resolved, and when it was made
//! and last changed, read from the repository's own files without running anything more;
//! then, when the person asked, the repository's open pull requests through `gh` (ADR 0127,
//! step 5b-2).
//!
//! Must not depend on: `app` or `dispatch`; the report goes back as the other jobs' do.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Jobs, Report, Trust};
use crate::git::ListedWorktree;
use crate::sessions::list::Worktree;
use crate::sessions::worktree::git;

impl Jobs {
    /// Lists the worktrees of the repository at `root`, under `trust`, for the tree; then, if
    /// `with_pulls`, its open pull requests (ADR 0127, step 5b-2), so that `gh` never holds the
    /// rows back.
    pub fn tree_worktrees(&self, trust: Trust, root: PathBuf, with_pulls: bool) {
        let failed = root.clone();
        let gh_host = self.gh_host.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let result = repo.worktrees().map(|listed| {
                    listed
                        .into_iter()
                        .filter(|w| !w.prunable && !w.bare)
                        .enumerate()
                        .map(|(at, listed)| worktree(repo.main(), listed, at == 0))
                        .collect()
                });
                let listed = result.is_ok();
                send(Report::Tree {
                    root: root.clone(),
                    result,
                });
                if with_pulls
                    && listed
                    && let Some(pulls) = open_pulls(repo, gh_host.as_deref())
                {
                    send(Report::TreePulls { root, pulls });
                }
            },
            move |why| Report::Tree {
                root: failed,
                result: Err(why),
            },
        );
    }
}

/// The open pull requests of `repo`'s GitHub repository, if `gh` is found and answers.
fn open_pulls(repo: &git::Repo, gh_host: Option<&str>) -> Option<Vec<git::OpenPull>> {
    let gh = crate::git::Gh::find(repo.main())?;
    let (_, github) = git::github_of(repo, &repo.remotes(), gh_host)?;
    git::open_pulls(&gh, &github, repo.hooks()).ok()
}

/// One listed worktree of the repository at `main`, the main checkout if `primary`: its path
/// resolved where it can be, when it was made (its `.git` file's change) and when git last
/// wrote its index.
fn worktree(main: &Path, listed: ListedWorktree, primary: bool) -> Worktree {
    let path = std::fs::canonicalize(&listed.path).unwrap_or(listed.path);
    let changed = |file: &Path| -> Option<SystemTime> {
        std::fs::symlink_metadata(file)
            .and_then(|m| m.modified())
            .ok()
    };
    let (made, active) = if primary {
        (None, changed(&main.join(".git").join("index")))
    } else {
        let admin = path
            .file_name()
            .map(|name| main.join(".git").join("worktrees").join(name));
        (
            changed(&path.join(".git")),
            admin.and_then(|admin| changed(&admin.join("index"))),
        )
    };
    Worktree {
        path,
        branch: listed.branch,
        primary,
        made,
        active,
        pull: None,
    }
}
