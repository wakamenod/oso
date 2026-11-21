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
    /// Live sessions in the registry that oso does not host.
    pub external: Vec<External>,
    /// Why the registry could not be read in full, or names a live session oso does not host
    /// with no directory, if it does: then such a session may run in any worktree, and check 4
    /// refuses every one.
    pub registry_untold: Option<&'static str>,
    /// The process ids of the `claude`s oso hosts whose registry entry is known live
    /// ([`registry::Live::Yes`]) and whose `status` is `idle`: the registry's half of check 4.
    /// An entry whose liveness is unknown leaves its session busy.
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
            registry::liveness(entry, process::start_time)
        });
        RepoFacts {
            remotes,
            mains,
            pushed,
            github,
            external: found.external.into_iter().map(External::new).collect(),
            registry_untold: untold.or(found.placeless.then_some(REGISTRY_PLACELESS)),
            hosted_idle: found.hosted_idle,
        }
    }

    /// The live sessions oso does not host whose directory is in `path`, by name or process id:
    /// by the paths as given or with links resolved, so that another name for the same
    /// directory is in it too.
    pub fn external_in(&self, path: &Path) -> Vec<String> {
        let real = std::fs::canonicalize(path).ok();
        let paths = [Some(path), real.as_deref()];
        self.external
            .iter()
            .filter(|session| {
                [Some(session.cwd.as_path()), session.real.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|cwd| {
                        paths
                            .into_iter()
                            .flatten()
                            .any(|path| cwd.starts_with(path))
                    })
            })
            .map(|session| session.name.clone())
            .collect()
    }
}

/// A live session in the registry that oso does not host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct External {
    /// Its directory, as the registry gives it.
    pub cwd: PathBuf,
    /// That directory with links resolved, if it can be.
    pub real: Option<PathBuf>,
    /// A name to show.
    pub name: String,
}

impl External {
    /// The session in `cwd` called `name`, resolving `cwd` on the disk.
    fn new((cwd, name): (PathBuf, String)) -> External {
        let real = std::fs::canonicalize(&cwd).ok();
        External { cwd, real, name }
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
    /// The process ids of the sessions oso hosts whose entry is known live and says `idle`.
    hosted_idle: HashSet<u32>,
    /// One oso does not host has no directory oso can use (`registry::Entry::cwd`), so that it
    /// may run anywhere.
    placeless: bool,
}

/// From the registry's `entries`, read once, with `liveness` giving whether each one's process
/// still runs, and `hosted` the process ids of the sessions oso hosts. Each errs towards
/// refusing a removal: an unknown entry counts as another's session, and a hosted session with
/// an entry that is unknown or not idle stays busy, as every entry for it that may still run
/// must be known live and say `idle`.
fn sessions_in_registry(
    entries: Vec<registry::Entry>,
    hosted: &[u32],
    liveness: impl Fn(&registry::Entry) -> Live,
) -> InRegistry {
    let mut found = InRegistry::default();
    let mut busy = HashSet::new();
    for entry in entries {
        let live = liveness(&entry);
        if live == Live::No {
            continue;
        }
        if hosted.contains(&entry.pid) {
            if live == Live::Yes && entry.status.as_deref() == Some("idle") {
                found.hosted_idle.insert(entry.pid);
            } else {
                busy.insert(entry.pid);
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
    found.hosted_idle.retain(|pid| !busy.contains(pid));
    found
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::{
        External, REGISTRY_CUT, REGISTRY_UNLISTABLE, REGISTRY_UNREAD, RepoFacts,
        sessions_in_registry, untold,
    };
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
        let found = sessions_in_registry(entries, &[1, 2], |_| Live::Yes);
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
        let found = sessions_in_registry(vec![hosted, gone], &[1], |e| {
            if e.pid == 2 { Live::No } else { Live::Yes }
        });
        assert!(found.hosted_idle.contains(&1) && !found.placeless);
        let mut external = entry(3, "idle");
        external.cwd = None;
        let found = sessions_in_registry(vec![external], &[1], |_| Live::Unknown);
        assert!(found.external.is_empty() && found.placeless);
    }

    #[test]
    fn an_entry_that_no_longer_runs_says_nothing() {
        let found = sessions_in_registry(vec![entry(1, "idle"), entry(3, "idle")], &[1], |e| {
            if e.pid == 1 { Live::No } else { Live::Yes }
        });
        assert!(found.hosted_idle.is_empty());
        assert_eq!(found.external.len(), 1);
    }

    #[test]
    fn an_idle_entry_not_known_live_leaves_its_hosted_session_busy() {
        // Hosted 1 is unknown, hosted 2 live: only 2 is idle. Unknown 3, not hosted, may still
        // run, so it still counts as another's session.
        let entries = vec![entry(1, "idle"), entry(2, "idle"), entry(3, "idle")];
        let found = sessions_in_registry(entries, &[1, 2], |e| match e.pid {
            2 => Live::Yes,
            _ => Live::Unknown,
        });
        assert_eq!(found.hosted_idle.into_iter().collect::<Vec<_>>(), [2]);
        let external = [(PathBuf::from("/w/3"), "process 3".to_owned())];
        assert_eq!(found.external, external);
    }

    #[test]
    fn an_idle_entry_with_no_start_time_leaves_a_running_hosted_session_busy() {
        // Idle, with neither `procStart` nor `startedAt`, for a process that runs.
        let untimed = entry(7, "idle");
        let runs = |_: u32| Ok(Some(SystemTime::now()));
        assert_eq!(registry::liveness(&untimed, runs), Live::Unknown);
        let found = sessions_in_registry(vec![untimed], &[7], |e| registry::liveness(e, runs));
        assert!(found.hosted_idle.is_empty());
        assert!(found.external.is_empty());
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
            let found = sessions_in_registry(vec![timed.clone()], &[7], |e| {
                registry::liveness(e, |_| {
                    start.ok_or_else(|| std::io::ErrorKind::Unsupported.into())
                })
            });
            found.hosted_idle
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
        let idle = sessions_in_registry(entries, &[7], |_| Live::Yes).hosted_idle;
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
        let idle = sessions_in_registry(entries.clone(), &[7], liveness).hosted_idle;
        assert!(idle.is_empty(), "one is not known live");
        let idle = sessions_in_registry(entries, &[7], |_| Live::Yes).hosted_idle;
        assert_eq!(idle.into_iter().collect::<Vec<_>>(), [7]);
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

    #[cfg(unix)]
    #[test]
    fn a_session_in_another_name_for_the_worktree_is_in_it() {
        let dir = std::env::temp_dir().join(format!(
            "oso-facts-alias-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        let worktree = dir.join("real/wt");
        std::fs::create_dir_all(worktree.join("sub")).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        let facts = |cwd: PathBuf| RepoFacts {
            remotes: Vec::new(),
            mains: Ok(HashSet::new()),
            pushed: HashSet::new(),
            github: None,
            external: vec![External::new((cwd, "other".to_owned()))],
            registry_untold: None,
            hosted_idle: HashSet::new(),
        };
        let other = ["other".to_owned()];
        // The session by a link, the worktree by its own path, and the other way round.
        assert_eq!(
            facts(dir.join("alias/wt/sub")).external_in(&worktree),
            other
        );
        assert_eq!(
            facts(worktree.clone()).external_in(&dir.join("alias/wt")),
            other
        );
        // A directory that is not there is matched by its text.
        assert_eq!(facts(worktree.join("gone")).external_in(&worktree), other);
        assert!(facts(dir.join("real")).external_in(&worktree).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
