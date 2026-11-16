//! The worktrees the tree of `*Spaces*` shows (ADR 0131, step 2): one `git worktree list`
//! of a trusted repository on the pool, each worktree's path resolved, and when it was made
//! and last changed, read from the repository's own files without running anything more.
//!
//! Must not depend on: `app` or `dispatch`; the report goes back as the other jobs' do.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Jobs, Report, Trust};
use crate::sessions::list::Worktree;
use crate::sessions::worktree::porcelain::Listed;

impl Jobs {
    /// Lists the worktrees of the repository at `root`, under `trust`, for the tree.
    pub fn tree_worktrees(&self, trust: Trust, root: PathBuf) {
        let failed = root.clone();
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
                send(Report::Tree { root, result });
            },
            move |why| Report::Tree {
                root: failed,
                result: Err(why),
            },
        );
    }
}

/// One listed worktree of the repository at `main`, the main checkout if `primary`: its path
/// resolved where it can be, when it was made (its `.git` file's change) and when git last
/// wrote its index.
fn worktree(main: &Path, listed: Listed, primary: bool) -> Worktree {
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
    }
}
