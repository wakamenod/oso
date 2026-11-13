//! What a worktree job reads of a repository once, for every worktree it looks at (ADR 0121):
//! the remotes, the remote branches that count as main lines, the local branches whose work
//! went to a remote, the GitHub repository `gh` is asked about, the live sessions oso did not
//! start, and which of the sessions oso hosts Claude Code's registry calls idle.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread; this runs on a worktree
//! job's thread.
//!
//! `gh` is asked only about `github.com`, or the host `GH_HOST` names in oso's environment: a
//! remote's address names no other host it is asked about.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oso_platform::process;

use super::git::{GhRepo, MAX_LISTING, MAX_NAMES, Repo, said};
use super::porcelain;
use crate::sessions::registry;

/// The branch names a remote's main line goes by, besides the one its `HEAD` points to.
pub const MAIN_BRANCHES: [&str; 3] = ["main", "master", "develop"];

/// What a job reads of the repository once.
#[derive(Debug)]
pub struct RepoFacts {
    /// The remotes' names, `origin` first.
    pub remotes: Vec<String>,
    /// The remote branches that count as main lines, by full name, or why they cannot be read.
    pub mains: Result<HashSet<String>, String>,
    /// Local branches with a remote-tracking branch of the same name on some remote, or an
    /// upstream: their work went to a remote, so a pull request may hold it.
    pub pushed: HashSet<String>,
    /// The first remote whose address names a GitHub repository `gh` is asked about, `origin`
    /// first, and that repository.
    pub github: Option<(String, GhRepo)>,
    /// Live sessions in the registry that oso does not host, by their directory and a name to
    /// show.
    pub external: Vec<(PathBuf, String)>,
    /// The process ids of the `claude`s oso hosts whose live registry entry's `status` is
    /// `idle`: the registry's half of check 4.
    pub hosted_idle: HashSet<u32>,
}

impl RepoFacts {
    /// Reads them from `repo`; `gh_host` is `GH_HOST` from oso's environment, and `hosted` the
    /// process ids of the `claude`s oso hosts. A fact that cannot be read is left empty.
    pub fn gather(repo: &Repo, gh_host: Option<&str>, hosted: &[u32]) -> RepoFacts {
        let remotes = repo.remotes();
        let (mains, mut pushed) = if remotes.is_empty() {
            (Ok(HashSet::new()), HashSet::new())
        } else {
            match remote_refs(repo) {
                Ok((mains, names)) => (Ok(mains), names),
                Err(why) => (Err(why), HashSet::new()),
            }
        };
        if !remotes.is_empty() {
            pushed.extend(with_upstream(repo));
        }
        let github = remotes.iter().find_map(|remote| {
            let url = repo.remote_url(remote)?;
            Some((remote.clone(), GhRepo::from_url(&url, gh_host)?))
        });
        let (external, hosted_idle) = registry::directory().map_or_else(Default::default, |dir| {
            sessions_in_registry(registry::read(&dir), hosted, |entry| {
                registry::liveness(entry, process::start_time) != registry::Live::No
            })
        });
        RepoFacts {
            remotes,
            mains,
            pushed,
            github,
            external,
            hosted_idle,
        }
    }

    /// The live sessions oso does not host whose directory is in `path`, by name or process id.
    pub fn external_in(&self, path: &Path) -> Vec<String> {
        self.external
            .iter()
            .filter(|(cwd, _)| cwd.starts_with(path))
            .map(|(_, name)| name.clone())
            .collect()
    }
}

/// From one listing of `refs/remotes/`: the remote branches that count as main lines
/// (`refs/remotes/R/{main,master,develop}` and what each `refs/remotes/R/HEAD` points to), by
/// full name, and the names of every remote branch, without their remote.
fn remote_refs(repo: &Repo) -> Result<(HashSet<String>, HashSet<String>), String> {
    let args = [
        "for-each-ref",
        "--format=%(refname)%00%(symref)%00",
        "refs/remotes/",
    ];
    let out = repo.run(&args, MAX_LISTING).map_err(|e| said(&e))?;
    let mut mains = HashSet::new();
    let mut names = HashSet::new();
    for (name, target) in porcelain::ref_pairs(&out, MAX_NAMES) {
        let Some((remote, branch)) = name
            .strip_prefix("refs/remotes/")
            .and_then(|rest| rest.split_once('/'))
        else {
            continue;
        };
        if branch == "HEAD" {
            mains.extend(target.filter(|t| t.starts_with(&format!("refs/remotes/{remote}/"))));
            continue;
        }
        if MAIN_BRANCHES.contains(&branch) {
            mains.insert(name.clone());
        }
        names.insert(branch.to_owned());
    }
    Ok((mains, names))
}

/// The local branches that have an upstream, by short name.
fn with_upstream(repo: &Repo) -> Vec<String> {
    let args = [
        "for-each-ref",
        "--format=%(refname)%00%(upstream)%00",
        "refs/heads/",
    ];
    let Ok(out) = repo.run(&args, MAX_LISTING) else {
        return Vec::new();
    };
    porcelain::ref_pairs(&out, MAX_NAMES)
        .into_iter()
        .filter(|(_, upstream)| upstream.is_some())
        .filter_map(|(name, _)| name.strip_prefix("refs/heads/").map(str::to_owned))
        .collect()
}

/// From the registry's `entries`, read once, those `live` says still run: the sessions oso does
/// not host, by directory and name, and the process ids of those it hosts (`hosted`) whose
/// `status` is `idle`.
fn sessions_in_registry(
    entries: Vec<registry::Entry>,
    hosted: &[u32],
    live: impl Fn(&registry::Entry) -> bool,
) -> (Vec<(PathBuf, String)>, HashSet<u32>) {
    let mut external = Vec::new();
    let mut idle = HashSet::new();
    for entry in entries.into_iter().filter(|entry| live(entry)) {
        if hosted.contains(&entry.pid) {
            if entry.status.as_deref() == Some("idle") {
                idle.insert(entry.pid);
            }
            continue;
        }
        let name = entry.name.as_deref().map_or_else(
            || format!("process {}", entry.pid),
            |name| porcelain::printable(name, 48),
        );
        if let Some(cwd) = entry.cwd {
            external.push((cwd, name));
        }
    }
    (external, idle)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::sessions_in_registry;
    use crate::sessions::registry::Entry;

    fn entry(pid: u32, status: &str) -> Entry {
        Entry {
            pid,
            session_id: None,
            cwd: Some(PathBuf::from(format!("/w/{pid}"))),
            proc_start: None,
            started_at: None,
            name: None,
            kind: None,
            status: Some(status.to_owned()),
        }
    }

    #[test]
    fn hosted_processes_give_their_status_and_others_are_external() {
        let entries = vec![entry(1, "idle"), entry(2, "busy"), entry(3, "idle")];
        let (external, idle) = sessions_in_registry(entries, &[1, 2], |_| true);
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [1]);
        assert_eq!(external, [(PathBuf::from("/w/3"), "process 3".to_owned())]);
    }

    #[test]
    fn an_entry_that_no_longer_runs_says_nothing() {
        let (external, idle) =
            sessions_in_registry(vec![entry(1, "idle"), entry(3, "idle")], &[1], |e| {
                e.pid != 1
            });
        assert!(idle.is_empty());
        assert_eq!(external.len(), 1);
    }
}
