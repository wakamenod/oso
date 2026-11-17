//! Worktree jobs as the app asks for them (ADR 0121): listing branches for `session-worktree`,
//! making a worktree, listing a repository's worktrees row by row, checking one before its
//! removal and removing it. Each runs on the [`Pool`], only for a trusted root whose main
//! repository is trusted too, and tells the app what it found through the function the app
//! gives.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread.
//!
//! - Each job checks its root again before it runs anything: a project no longer trusted, or
//!   whose root is no longer the directory that was trusted, runs no git.
//! - A listing looks at every row without `gh` first, then asks `gh` only for the rows that
//!   need it, and for the person's listing the repository's open pull requests (ADR 0127, step
//!   5b-2). A newer listing stops an older one between rows.
//! - A removal runs every check again, in the same job as `git worktree remove`. Once they hold,
//!   it asks the app to clear the worktree (stop its idle sessions) and waits for the app's
//!   word before git removes it. git then runs with no time limit, on a thread of its own
//!   (`remove`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::SyncSender;

use super::checks::{self, Check, Depth, Held, HostedSession, Looked};
use super::create::{self, Created, Order, Step};
use super::facts::RepoFacts;
use super::git::{Repo, said};
use super::merged::{self, Merged};
use super::pool::Pool;
use super::prepare::{self, Prepare, Prepared};
use crate::git::Gh;
use crate::git::MAX_SHORT;
use crate::git::TrustedRoot;
use crate::project::{Trusted, linked_worktree};
use crate::sessions::spaces::Member;

/// Why a job refuses a repository whose main checkout is not trusted.
pub const MAIN_UNTRUSTED: &str =
    "Worktree sessions need the main repository trusted: trust-project it first";

/// What the UI thread holds when a job is asked for, for every worktree it may meet: buffers
/// with unsaved changes, unsent briefs and hosted sessions, by path.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// The files of buffers with unsaved changes.
    pub unsaved: Vec<PathBuf>,
    /// The worktree paths unsent briefs are for.
    pub briefs: Vec<PathBuf>,
    /// The hosted sessions that run, neither failed nor stopped, with their screen's half of
    /// check 4.
    pub sessions: Vec<HostedSession>,
    /// The process ids of the `claude`s oso hosts.
    pub hosted: Vec<u32>,
}

impl Snapshot {
    /// What it holds about the worktree at `path`, with `idle` the process ids the registry
    /// calls idle: a hosted session is busy unless both halves say it is not.
    pub fn held(&self, path: &Path, idle: &HashSet<u32>) -> Held {
        Held {
            unsaved: self.unsaved.iter().filter(|f| f.starts_with(path)).count(),
            unsent: self.briefs.iter().any(|b| b.starts_with(path)),
            busy: self
                .sessions
                .iter()
                .filter(|session| session.directory.starts_with(path))
                .filter(|session| {
                    let registry_idle = session.pid.is_some_and(|pid| idle.contains(&pid));
                    checks::busy(session, registry_idle)
                })
                .map(|session| session.name.clone())
                .collect(),
        }
    }
}

/// The app's word that a removal whose checks hold may go on, once it has cleared the
/// worktree. Dropping it calls the removal off.
#[derive(Debug)]
pub struct Go(SyncSender<()>);

impl Go {
    /// The removal goes on.
    pub fn proceed(self) {
        let _ = self.0.send(());
    }
}

/// The trust a job is asked under: the project's root, and the roots the record of trusted
/// projects holds, which must hold the repository's main checkout too.
#[derive(Clone, Debug)]
pub struct Trust {
    pub root: TrustedRoot,
    pub trusted: Trusted,
}

/// What a job found.
#[derive(Debug)]
pub enum Report {
    /// The worktrees of the repository at `root`, for the tree of `*Spaces*` (ADR 0131).
    Tree {
        root: PathBuf,
        result: Result<Vec<crate::sessions::list::Worktree>, String>,
    },
    /// The local branches, for `session-worktree`'s completion.
    Branches {
        token: u64,
        branches: Result<Vec<String>, String>,
    },
    /// A create job reached a step.
    Step { token: u64, step: Step },
    /// A create job ended: what it made, and the trust the new worktree's session takes.
    Created {
        token: u64,
        result: Result<(Created, TrustedRoot), String>,
    },
    /// A listing found this many linked worktrees; their rows follow.
    Listed {
        token: u64,
        result: Result<usize, String>,
    },
    /// One row of a listing.
    Row { token: u64, looked: Box<Looked> },
    /// The repository's open pull requests, each its head branch and its row's line (ADR 0127,
    /// step 5b-2), after every row.
    Pulls {
        token: u64,
        pulls: Vec<super::git::OpenPull>,
    },
    /// The open pull requests of the tree's repository at `root`, after its worktrees.
    TreePulls {
        root: PathBuf,
        pulls: Vec<super::git::OpenPull>,
    },
    /// What `gh` said of a listed row's branch, after every row.
    Merged {
        token: u64,
        path: PathBuf,
        merged: Merged,
    },
    /// A removal's checks held again: the app clears the worktree, then says [`Go`].
    Clear { token: u64, path: PathBuf, go: Go },
    /// A removal's checks: what git ignores there, which the removal deletes, or the first
    /// check that refuses.
    Checked {
        token: u64,
        path: PathBuf,
        result: Result<Vec<PathBuf>, Check>,
    },
    /// A removal ended, with what git ignored there, which went with it; the branch is kept
    /// either way.
    Removed {
        token: u64,
        path: PathBuf,
        result: Result<Vec<PathBuf>, String>,
    },
    /// A listing ended: every row and every answer of `gh` came. It follows every listing
    /// job that reached its repository, a stopped one too; one that could not open the
    /// repository sends [`Report::Listed`] with an error instead.
    ListEnded { token: u64 },
    /// What an agent's session tool needs to know before the person is asked (ADR 0121,
    /// step 6).
    ///
    /// A directory found is given with its trust, which a session started there takes.
    ///
    /// With it, the directories the new session also reads, each checked with its identity
    /// (ADR 0136, step 4b).
    Prepared {
        token: u64,
        result: Result<(Prepared, Option<TrustedRoot>, Vec<Member>), String>,
    },
}

/// How a removal was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asked {
    /// The person's question named this many of what git ignores there: a removal that would
    /// delete more is refused.
    Named(usize),
    /// An agent's `worktree_remove`, which asks no question once every check holds (Q14).
    Unasked,
}

/// The jobs, run on a pool of their own.
#[derive(Clone)]
pub struct Jobs {
    pool: Pool,
    /// oso's state directory, where the empty directory for git's hooks is.
    state: Option<PathBuf>,
    /// `GH_HOST` in oso's environment, read once: the one host besides `github.com` that `gh`
    /// is asked about.
    gh_host: Option<String>,
    /// The newest listing's number: an older listing stops.
    listing: Arc<AtomicU64>,
    /// The removals asked for and not yet ended.
    removals: remove::Removals,
    send: Arc<dyn Fn(Report) + Send + Sync>,
}

impl std::fmt::Debug for Jobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Jobs").field("state", &self.state).finish()
    }
}

impl Jobs {
    /// Jobs whose reports go to `send`, keeping their private directory under `state`.
    pub fn new(state: Option<PathBuf>, send: impl Fn(Report) + Send + Sync + 'static) -> Jobs {
        let gh_host = std::env::var("GH_HOST")
            .ok()
            .map(|host| host.trim().to_owned())
            .filter(|host| !host.is_empty());
        Jobs {
            pool: Pool::default(),
            state,
            gh_host,
            listing: Arc::default(),
            removals: remove::Removals::default(),
            send: Arc::new(send),
        }
    }

    /// Runs `work` with the repository of `trust` on the pool, ahead of the jobs waiting if
    /// `first`; `failed` makes the report when the repository cannot be run in.
    fn on_repo(
        &self,
        trust: Trust,
        first: bool,
        work: impl FnOnce(&TrustedRoot, &Repo, &dyn Fn(Report)) + Send + 'static,
        failed: impl FnOnce(String) -> Report + Send + 'static,
    ) {
        let send = Arc::clone(&self.send);
        let state = self.state.clone();
        let job = move || {
            let repo = trust
                .root
                .check()
                .and_then(|()| repo(&trust, state.as_deref()));
            match repo {
                Ok(repo) => work(&trust.root, &repo, &*send),
                Err(why) => send(failed(why)),
            }
        };
        let started = if first {
            self.pool.run_first(job)
        } else {
            self.pool.run(job)
        };
        if !started {
            log::warn!("a worktree job was dropped: no thread could start");
        }
    }

    /// Lists the local branches of the repository, ahead of other jobs: a prompt waits.
    pub fn branches(&self, token: u64, trust: Trust) {
        self.on_repo(
            trust,
            true,
            move |_, repo, send| {
                send(Report::Branches {
                    token,
                    branches: repo.branches(),
                });
            },
            move |why| Report::Branches {
                token,
                branches: Err(why),
            },
        );
    }

    /// Makes the worktree `order` asks for.
    pub fn create(&self, token: u64, trust: Trust, order: Order) {
        self.on_repo(
            trust,
            false,
            move |root, repo, send| {
                let progress = |step| send(Report::Step { token, step });
                let result =
                    create::create(repo, &order, &progress, &|| root.check()).map(|created| {
                        let trusted = root.for_worktree(created.path.clone());
                        (created, trusted)
                    });
                send(Report::Created { token, result });
            },
            move |why| Report::Created {
                token,
                result: Err(why),
            },
        );
    }

    /// Lists the linked worktrees of the repository, a row each as it is looked at, then what
    /// `gh` says of the rows that need it. A newer listing stops this one.
    pub fn list(&self, token: u64, trust: Trust, snapshot: Snapshot) {
        let number = self.listing.fetch_add(1, Ordering::SeqCst) + 1;
        let newest = Arc::clone(&self.listing);
        self.list_while(token, (trust, true), snapshot, move || {
            newest.load(Ordering::SeqCst) == number
        });
    }

    /// Lists as [`Self::list`] does, for an agent's `worktree_list`, until `stop` is set: no
    /// listing of the person's stops it, and it stops none of theirs.
    pub fn list_for_agent(
        &self,
        token: u64,
        trust: Trust,
        snapshot: Snapshot,
        stop: Arc<AtomicBool>,
    ) {
        // An agent's listing asks `gh` for no open pull request (ADR 0127, step 5b-2).
        self.list_while(token, (trust, false), snapshot, move || {
            !stop.load(Ordering::SeqCst)
        });
    }

    /// A listing that goes on while `live` says so, and says when it ended.
    fn list_while(
        &self,
        token: u64,
        (trust, pulls): (Trust, bool),
        snapshot: Snapshot,
        live: impl Fn() -> bool + Send + 'static,
    ) {
        let gh_host = self.gh_host.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                // One stopped while it waited for a thread runs nothing.
                if live() {
                    let asks = (token, pulls);
                    list(repo, &snapshot, gh_host.as_deref(), &live, asks, send);
                }
                send(Report::ListEnded { token });
            },
            move |why| Report::Listed {
                token,
                result: Err(why),
            },
        );
    }

    /// Looks at what an agent's session tool would do, before the person is asked.
    ///
    /// `with` are the directories the new session also reads, each with its repository's
    /// trust: each is checked as that repository's root or one of its linked worktrees.
    pub fn prepare(&self, token: u64, trust: Trust, what: Prepare, with: Vec<(Trust, PathBuf)>) {
        let state = self.state.clone();
        let removals = self.removals.clone();
        self.on_repo(
            trust,
            true,
            move |root, repo, send| {
                let readable = (with.iter())
                    .map(|(trust, dir)| readable_in(trust, dir, state.as_deref()))
                    .collect::<Result<Vec<Member>, String>>();
                let result = prepare::prepare(repo, &what).and_then(|prepared| {
                    // git may be deleting it: a session started there would lose its files.
                    if let Prepared::Directory(directory) = &prepared
                        && let Some(why) = removals.running_under(directory)
                    {
                        return Err(why);
                    }
                    let trusted = match &prepared {
                        // The root itself keeps the identity its check just held.
                        Prepared::Directory(directory) if directory == root.path() => {
                            Some(root.clone())
                        }
                        Prepared::Directory(directory) => {
                            Some(root.for_worktree(directory.clone()))
                        }
                        Prepared::Worktree { .. } => None,
                    };
                    Ok((prepared, trusted, readable?))
                });
                send(Report::Prepared { token, result });
            },
            move |why| Report::Prepared {
                token,
                result: Err(why),
            },
        );
    }

    /// Runs the checks of a removal of the worktree at `path`, in order.
    pub fn check(&self, token: u64, trust: Trust, path: PathBuf, snapshot: Snapshot) {
        let failed_path = path.clone();
        let gh_host = self.gh_host.clone();
        let removals = self.removals.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let result = match removals.running_at(&path) {
                    Some(why) => Err(Check::Unknown(why)),
                    None => check(repo, gh_host.as_deref(), &path, &snapshot),
                };
                send(Report::Checked {
                    token,
                    path,
                    result,
                });
            },
            move |why| Report::Checked {
                token,
                path: failed_path,
                result: Err(Check::Unknown(why)),
            },
        );
    }
}

/// `dir`, a directory a new session also reads, checked in the repository `trust` names, with
/// `state` oso's state directory: the root still the one trusted, and `dir` that root or one
/// of its linked worktrees itself (ADR 0136, step 4b). It runs git, so it belongs off the UI
/// thread.
///
/// # Errors
///
/// Why not, in words for the agent or the echo area.
pub fn readable_in(trust: &Trust, dir: &Path, state: Option<&Path>) -> Result<Member, String> {
    trust.root.check()?;
    prepare::readable(&repo(trust, state)?, dir)
}

/// git in the main checkout of the repository of `trust`, with the empty private directory
/// for hooks under `state`, if that main checkout is trusted too.
fn repo(trust: &Trust, state: Option<&Path>) -> Result<Repo, String> {
    let state = state.ok_or("oso has no state directory, so it runs no git for worktrees")?;
    let hooks = crate::git::empty_hooks(state, "worktrees")?;
    let main = main_of(trust.root.path(), &hooks)?;
    // Worktrees take their trust from the main repository (ADR 0114).
    let main_trusted = trust
        .trusted
        .iter()
        .any(|known| oso_core::project::same_place(known, &main));
    if !main_trusted {
        return Err(MAIN_UNTRUSTED.to_owned());
    }
    Repo::open(&main, &hooks)
}

/// The main checkout of the repository `project` is in: `project` itself, or the main
/// repository a linked worktree's claim names and which records it back.
fn main_of(project: &Path, hooks: &Path) -> Result<PathBuf, String> {
    let probe = Repo::open(project, hooks)?;
    let args = ["rev-parse", "--path-format=absolute", "--git-common-dir"];
    let out = probe.run(&args, MAX_SHORT).map_err(|e| said(&e))?;
    let common = PathBuf::from(String::from_utf8_lossy(&out).trim());
    let main = match (common.file_name(), common.parent()) {
        (Some(name), Some(main)) if name == ".git" => main.to_owned(),
        _ => return Err("Worktree sessions need a repository with a .git directory".to_owned()),
    };
    if oso_core::project::same_place(&main, project) || linked_worktree(&main, project).is_some() {
        Ok(main)
    } else {
        Err("This worktree's repository does not record it".to_owned())
    }
}

/// A listing's work: every row looked at without `gh`, then `gh` asked for the rows that need
/// it, while `live` says no newer listing was asked for.
fn list(
    repo: &Repo,
    snapshot: &Snapshot,
    gh_host: Option<&str>,
    live: &dyn Fn() -> bool,
    (token, pulls): (u64, bool),
    send: &dyn Fn(Report),
) {
    let listed = match repo.worktrees() {
        Ok(listed) => listed,
        Err(why) => {
            return send(Report::Listed {
                token,
                result: Err(why),
            });
        }
    };
    // The first is the main worktree.
    let linked: Vec<_> = listed.into_iter().skip(1).collect();
    send(Report::Listed {
        token,
        result: Ok(linked.len()),
    });
    let gh = Gh::find(repo.main());
    let facts = RepoFacts::gather(repo, gh_host, &snapshot.hosted);
    let mut asks = Vec::new();
    for listed in linked {
        if !live() {
            return;
        }
        let held = snapshot.held(&listed.path, &facts.hosted_idle);
        let looked = checks::look(repo, gh.as_ref(), &facts, listed, &held, Depth::Local);
        if let (Some(tip), Some(branch)) = (&looked.gh_pending, &looked.listed.branch) {
            asks.push((looked.listed.path.clone(), branch.clone(), tip.clone()));
        }
        send(Report::Row {
            token,
            looked: Box::new(looked),
        });
    }
    let Some(gh) = gh else {
        return;
    };
    if pulls
        && live()
        && let Some((_, github)) = &facts.github
        && let Ok(pulls) = super::git::open_pulls(&gh, github, repo.hooks())
    {
        send(Report::Pulls { token, pulls });
    }
    for (path, branch, tip) in asks {
        if !live() {
            return;
        }
        let merged = merged::by_pull_request(repo, &gh, &facts, &branch, &tip);
        send(Report::Merged {
            token,
            path,
            merged,
        });
    }
}

/// The checks of a removal of the worktree at `path`: what git ignores there, or the first
/// check that refuses.
fn check(
    repo: &Repo,
    gh_host: Option<&str>,
    path: &Path,
    snapshot: &Snapshot,
) -> Result<Vec<PathBuf>, Check> {
    let listed = repo.worktrees().map_err(Check::Unknown)?;
    let Some(listed) = listed
        .into_iter()
        .skip(1)
        .find(|w| oso_core::project::same_place(&w.path, path))
    else {
        return Err(Check::NotLinked(
            "it is not a linked worktree of this repository".to_owned(),
        ));
    };
    let gh = Gh::find(repo.main());
    let facts = RepoFacts::gather(repo, gh_host, &snapshot.hosted);
    let held = snapshot.held(&listed.path, &facts.hosted_idle);
    let looked = checks::look(
        repo,
        gh.as_ref(),
        &facts,
        listed,
        &held,
        Depth::UntilRefused,
    );
    checks::removable(&looked, &held).map(|()| looked.ignored)
}

mod remove;
mod tree;

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    use super::Snapshot;
    use crate::sessions::worktree::checks::HostedSession;

    fn session(name: &str, pid: u32, settled: bool) -> HostedSession {
        HostedSession {
            directory: PathBuf::from(format!("/r/.claude/worktrees/w/{name}")),
            name: name.to_owned(),
            pid: Some(pid),
            settled,
        }
    }

    #[test]
    fn a_hosted_session_is_busy_unless_its_screen_and_the_registry_say_idle() {
        let snapshot = Snapshot {
            sessions: vec![
                session("working", 1, false),
                session("idle-but-registry-busy", 2, true),
                session("idle", 3, true),
                session("not-in-the-registry", 4, true),
            ],
            ..Snapshot::default()
        };
        let idle: HashSet<u32> = [1, 3].into();
        let held = snapshot.held(&PathBuf::from("/r/.claude/worktrees/w"), &idle);
        assert_eq!(
            held.busy,
            ["working", "idle-but-registry-busy", "not-in-the-registry"]
        );
        let elsewhere = snapshot.held(&PathBuf::from("/r/.claude/worktrees/v"), &idle);
        assert!(elsewhere.busy.is_empty());
    }
}
