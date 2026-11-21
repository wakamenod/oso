//! What a worktree job reads of a repository once, for every worktree it looks at (ADR 0121):
//! the remotes, the remote branches that count as main lines, the local branches whose work
//! went to a remote, the GitHub repository `gh` is asked about, the live sessions oso did not
//! start, which of the sessions oso hosts Claude Code's registry calls idle, and why the
//! registry cannot tell whether a session oso did not start runs in a worktree, if it cannot.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; this runs on a worktree
//! job's thread.
//!
//! `gh` is asked only about `github.com`, or the host `GH_HOST` names in oso's environment: a
//! remote's address names no other host it is asked about.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use oso_platform::process;

use super::git::{MAX_LISTING, MAX_NAMES, Repo, said};
use super::porcelain;
use crate::git::GhRepo;
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
    /// Why the registry could not be read in full, or names a live session oso does not host
    /// with no directory, if it does: then such a session may run in any worktree, and check 4
    /// refuses every one.
    pub registry_untold: Option<&'static str>,
    /// The process ids of the `claude`s oso hosts whose live registry entry's `status` is
    /// `idle`: the registry's half of check 4.
    pub hosted_idle: HashSet<u32>,
}

impl RepoFacts {
    /// Reads them from `repo`; `gh_host` is `GH_HOST` from oso's environment, `hosted` the
    /// process ids of the `claude`s oso hosts, and `registry` Claude Code's registry directory,
    /// `None` if it is not known (`registry::directory`). A fact that cannot be read is left
    /// empty, but for the registry, which says why in `registry_untold`.
    pub fn gather(
        repo: &Repo,
        gh_host: Option<&str>,
        hosted: &[u32],
        registry: Option<&Path>,
    ) -> RepoFacts {
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
        let github = super::git::github_of(repo, &remotes, gh_host);
        let (listing, untold) = match registry {
            Some(dir) => {
                let listing = registry::list(dir);
                let untold = untold(&listing);
                (listing, untold)
            }
            None => (registry::Listing::default(), Some(REGISTRY_UNKNOWN)),
        };
        let found = sessions_in_registry(listing.entries, hosted, |entry| {
            registry::liveness(entry, process::start_time) != registry::Live::No
        });
        RepoFacts {
            remotes,
            mains,
            pushed,
            github,
            external: found.external,
            registry_untold: untold.or(found.placeless.then_some(REGISTRY_PLACELESS)),
            hosted_idle: found.hosted_idle,
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

/// Why check 4 refuses when Claude Code's registry directory is not known.
pub const REGISTRY_UNKNOWN: &str =
    "Claude Code's session registry is not known, so a session oso did not start may run in it";

/// Why check 4 refuses when the registry holds more entry files than are read.
pub const REGISTRY_CUT: &str = "Claude Code's session registry has more than 256 entry files, \
     so a session oso did not start may run in it";

/// Why check 4 refuses when the registry directory is there but cannot be listed.
pub const REGISTRY_UNLISTABLE: &str = "Claude Code's session registry cannot be listed, so a \
     session oso did not start may run in it";

/// Why check 4 refuses when an entry file in the registry cannot be opened or read.
pub const REGISTRY_UNREAD: &str = "Claude Code's session registry has an entry file that cannot \
     be read, so a session oso did not start may run in it";

/// Why check 4 refuses when a live session oso did not start has no directory oso can use.
pub const REGISTRY_PLACELESS: &str = "Claude Code's session registry has a live session oso did \
     not start whose directory cannot be read, so it may run in it";

/// Why check 4 refuses when the registry `listing` is incomplete, if it is.
fn untold(listing: &registry::Listing) -> Option<&'static str> {
    listing.incomplete().map(|why| match why {
        registry::Incomplete::Cut => REGISTRY_CUT,
        registry::Incomplete::Unlistable => REGISTRY_UNLISTABLE,
        registry::Incomplete::Unread => REGISTRY_UNREAD,
    })
}

/// The remote branches that count as main lines, by full name, from one listing of
/// `refs/remotes/`: all a session's work needs at a turn's end (ADR 0121, "Merged work is not
/// ready for review, as amended").
///
/// # Errors
///
/// What git said.
pub fn main_lines(repo: &Repo) -> Result<HashSet<String>, String> {
    remote_refs(repo).map(|(mains, _)| mains)
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

/// What the registry's live entries say of the sessions.
#[derive(Debug, Default)]
struct InRegistry {
    /// The sessions oso does not host, by directory and name.
    external: Vec<(PathBuf, String)>,
    /// The process ids of the sessions oso hosts whose `status` is `idle`.
    hosted_idle: HashSet<u32>,
    /// One oso does not host has no directory oso can use (`registry::Entry::cwd`), so that it
    /// may run anywhere.
    placeless: bool,
}

/// From the registry's `entries`, read once, those `live` says still run, with `hosted` the
/// process ids of the sessions oso hosts.
fn sessions_in_registry(
    entries: Vec<registry::Entry>,
    hosted: &[u32],
    live: impl Fn(&registry::Entry) -> bool,
) -> InRegistry {
    let mut found = InRegistry::default();
    for entry in entries.into_iter().filter(|entry| live(entry)) {
        if hosted.contains(&entry.pid) {
            if entry.status.as_deref() == Some("idle") {
                found.hosted_idle.insert(entry.pid);
            }
            continue;
        }
        let name = entry.name.as_deref().map_or_else(
            || format!("process {}", entry.pid),
            |name| crate::git::printable(name, 48),
        );
        match entry.cwd {
            Some(cwd) => found.external.push((cwd, name)),
            None => found.placeless = true,
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{REGISTRY_CUT, REGISTRY_UNLISTABLE, REGISTRY_UNREAD, sessions_in_registry, untold};
    use crate::sessions::registry::{Entry, Listing, MAX_FILES};

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
            status_updated_at: None,
        }
    }

    #[test]
    fn hosted_processes_give_their_status_and_others_are_external() {
        let entries = vec![entry(1, "idle"), entry(2, "busy"), entry(3, "idle")];
        let found = sessions_in_registry(entries, &[1, 2], |_| true);
        assert_eq!(found.hosted_idle.into_iter().collect::<Vec<_>>(), [1]);
        let external = [(PathBuf::from("/w/3"), "process 3".to_owned())];
        assert_eq!(found.external, external);
        assert!(!found.placeless);
    }

    #[test]
    fn a_live_external_entry_with_no_directory_may_run_anywhere() {
        let mut hosted = entry(1, "idle");
        hosted.cwd = None;
        let mut gone = entry(2, "idle");
        gone.cwd = None;
        let found = sessions_in_registry(vec![hosted, gone], &[1], |e| e.pid != 2);
        assert!(found.hosted_idle.contains(&1) && !found.placeless);
        let mut external = entry(3, "idle");
        external.cwd = None;
        let found = sessions_in_registry(vec![external], &[1], |_| true);
        assert!(found.external.is_empty() && found.placeless);
    }

    #[test]
    fn an_entry_that_no_longer_runs_says_nothing() {
        let found = sessions_in_registry(vec![entry(1, "idle"), entry(3, "idle")], &[1], |e| {
            e.pid != 1
        });
        assert!(found.hosted_idle.is_empty());
        assert_eq!(found.external.len(), 1);
    }

    #[test]
    fn an_incomplete_registry_read_gives_why() {
        assert_eq!(untold(&Listing::default()), None);
        let cut = Listing {
            cut: true,
            ..Listing::default()
        };
        assert_eq!(untold(&cut), Some(REGISTRY_CUT));
        assert!(REGISTRY_CUT.contains(&MAX_FILES.to_string()));
        let unlistable = Listing {
            unlistable: true,
            ..Listing::default()
        };
        assert_eq!(untold(&unlistable), Some(REGISTRY_UNLISTABLE));
        let unread_file = Listing {
            unread: true,
            ..Listing::default()
        };
        assert_eq!(untold(&unread_file), Some(REGISTRY_UNREAD));
    }
}
