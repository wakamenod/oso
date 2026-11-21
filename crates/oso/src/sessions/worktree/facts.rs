//! What a worktree job reads of a repository once, for every worktree it looks at (ADR 0121):
//! the remotes, the remote branches that count as main lines, the local branches whose work
//! went to a remote, the GitHub repository `gh` is asked about, the live sessions oso did not
//! start, which of the sessions oso hosts Claude Code's registry calls idle, and whether the
//! registry was listed in full.
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
use crate::sessions::registry::{self, Live};

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
    /// Why the registry could not be listed in full, if it could not: then a session oso does
    /// not host may run in any worktree, and check 4 refuses every one.
    pub registry_unread: Option<&'static str>,
    /// The process ids of the `claude`s oso hosts whose registry entry is known live
    /// ([`registry::Live::Yes`]) and whose `status` is `idle`: the registry's half of check 4.
    /// An entry whose liveness is unknown leaves its session busy.
    pub hosted_idle: HashSet<u32>,
}

impl RepoFacts {
    /// Reads them from `repo`; `gh_host` is `GH_HOST` from oso's environment, `hosted` the
    /// process ids of the `claude`s oso hosts, and `registry` Claude Code's registry directory,
    /// `None` if it is not known (`registry::directory`). A fact that cannot be read is left
    /// empty, but for the registry, which says why in `registry_unread`.
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
        let (listing, registry_unread) = match registry {
            Some(dir) => {
                let listing = registry::list(dir);
                let unread = unread(&listing);
                (listing, unread)
            }
            None => (registry::Listing::default(), Some(REGISTRY_UNKNOWN)),
        };
        let (external, hosted_idle) = sessions_in_registry(listing.entries, hosted, |entry| {
            registry::liveness(entry, process::start_time)
        });
        RepoFacts {
            remotes,
            mains,
            pushed,
            github,
            external,
            registry_unread,
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

/// Why check 4 refuses when Claude Code's registry directory is not known.
pub const REGISTRY_UNKNOWN: &str =
    "Claude Code's session registry is not known, so a session oso did not start may run in it";

/// Why check 4 refuses when the registry holds more entry files than are read.
pub const REGISTRY_CUT: &str = "Claude Code's session registry has more than 256 entry files, \
     so a session oso did not start may run in it";

/// Why check 4 refuses when the registry directory is there but cannot be listed.
pub const REGISTRY_UNLISTABLE: &str = "Claude Code's session registry cannot be listed, so a \
     session oso did not start may run in it";

/// Why check 4 refuses when the registry `listing` is incomplete, if it is.
fn unread(listing: &registry::Listing) -> Option<&'static str> {
    if listing.complete() {
        None
    } else if listing.unlistable {
        Some(REGISTRY_UNLISTABLE)
    } else {
        Some(REGISTRY_CUT)
    }
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

/// From the registry's `entries`, read once, with `liveness` giving whether each one's process
/// still runs: the sessions oso does not host that may still run (live or unknown), by
/// directory and name, and the process ids of those it hosts (`hosted`) whose entry is known
/// live and says `idle`, as every entry for it that may still run does. Each errs towards
/// refusing a removal: an unknown entry counts as another's session, and a hosted session with
/// an entry that is unknown or not idle stays busy.
fn sessions_in_registry(
    entries: Vec<registry::Entry>,
    hosted: &[u32],
    liveness: impl Fn(&registry::Entry) -> Live,
) -> (Vec<(PathBuf, String)>, HashSet<u32>) {
    let mut external = Vec::new();
    let mut idle = HashSet::new();
    let mut busy = HashSet::new();
    for entry in entries {
        let live = liveness(&entry);
        if live == Live::No {
            continue;
        }
        if hosted.contains(&entry.pid) {
            if live == Live::Yes && entry.status.as_deref() == Some("idle") {
                idle.insert(entry.pid);
            } else {
                busy.insert(entry.pid);
            }
            continue;
        }
        let name = entry.name.as_deref().map_or_else(
            || format!("process {}", entry.pid),
            |name| crate::git::printable(name, 48),
        );
        if let Some(cwd) = entry.cwd {
            external.push((cwd, name));
        }
    }
    idle.retain(|pid| !busy.contains(pid));
    (external, idle)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{REGISTRY_CUT, REGISTRY_UNLISTABLE, sessions_in_registry, unread};
    use crate::sessions::registry::{self, Entry, Listing, Live, MAX_FILES};

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
        let (external, idle) = sessions_in_registry(entries, &[1, 2], |_| Live::Yes);
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [1]);
        assert_eq!(external, [(PathBuf::from("/w/3"), "process 3".to_owned())]);
    }

    #[test]
    fn an_entry_that_no_longer_runs_says_nothing() {
        let (external, idle) =
            sessions_in_registry(vec![entry(1, "idle"), entry(3, "idle")], &[1], |e| {
                if e.pid == 1 { Live::No } else { Live::Yes }
            });
        assert!(idle.is_empty());
        assert_eq!(external.len(), 1);
    }

    #[test]
    fn an_idle_entry_not_known_live_leaves_its_hosted_session_busy() {
        // Hosted 1 is unknown, hosted 2 live: only 2 is idle. Unknown 3, not hosted, may still
        // run, so it still counts as another's session.
        let entries = vec![entry(1, "idle"), entry(2, "idle"), entry(3, "idle")];
        let (external, idle) = sessions_in_registry(entries, &[1, 2], |e| match e.pid {
            2 => Live::Yes,
            _ => Live::Unknown,
        });
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [2]);
        assert_eq!(external, [(PathBuf::from("/w/3"), "process 3".to_owned())]);
    }

    #[test]
    fn an_idle_entry_with_no_start_time_leaves_a_running_hosted_session_busy() {
        // Idle, with neither `procStart` nor `startedAt`, for a process that runs.
        let untimed = entry(7, "idle");
        let runs = |_: u32| Ok(Some(SystemTime::now()));
        assert_eq!(registry::liveness(&untimed, runs), Live::Unknown);
        let (external, idle) =
            sessions_in_registry(vec![untimed], &[7], |e| registry::liveness(e, runs));
        assert!(idle.is_empty());
        assert!(external.is_empty());
    }

    #[test]
    fn a_hosted_entry_is_idle_only_when_its_start_matches_its_process() {
        let started = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let timed = Entry {
            started_at: Some(1_800_000_000_000),
            ..entry(7, "idle")
        };
        // `None`: the system does not say when the process started.
        let idle_with = |start: Option<Option<SystemTime>>| {
            let (_, idle) = sessions_in_registry(vec![timed.clone()], &[7], |e| {
                registry::liveness(e, |_| {
                    start.ok_or_else(|| std::io::ErrorKind::Unsupported.into())
                })
            });
            idle
        };
        let idle = idle_with(Some(Some(started)));
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [7]);
        let later = started + Duration::from_secs(60);
        assert!(
            idle_with(Some(Some(later))).is_empty(),
            "another process's start"
        );
        assert!(idle_with(Some(None)).is_empty(), "no such process");
        assert!(idle_with(None).is_empty(), "the system does not say");
    }

    #[test]
    fn a_hosted_process_with_any_entry_not_idle_is_not_idle() {
        // Two files can give one process id; each must say idle and be known live.
        let entries = vec![entry(7, "busy"), entry(7, "idle")];
        let (_, idle) = sessions_in_registry(entries, &[7], |_| Live::Yes);
        assert!(idle.is_empty(), "one says busy");
        let unknown = Entry {
            name: Some("unknown".to_owned()),
            ..entry(7, "idle")
        };
        let entries = vec![entry(7, "idle"), unknown];
        let liveness = |e: &Entry| {
            if e.name.is_some() {
                Live::Unknown
            } else {
                Live::Yes
            }
        };
        let (_, idle) = sessions_in_registry(entries.clone(), &[7], liveness);
        assert!(idle.is_empty(), "one is not known live");
        let (_, idle) = sessions_in_registry(entries, &[7], |_| Live::Yes);
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [7]);
    }

    #[test]
    fn an_incomplete_registry_read_gives_why() {
        assert_eq!(unread(&Listing::default()), None);
        let cut = Listing {
            cut: true,
            ..Listing::default()
        };
        assert_eq!(unread(&cut), Some(REGISTRY_CUT));
        assert!(REGISTRY_CUT.contains(&MAX_FILES.to_string()));
        let unlistable = Listing {
            unlistable: true,
            ..Listing::default()
        };
        assert_eq!(unread(&unlistable), Some(REGISTRY_UNLISTABLE));
    }
}
