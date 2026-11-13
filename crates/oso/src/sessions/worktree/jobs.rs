//! Worktree jobs as the app asks for them (ADR 0121): listing branches for `session-worktree`,
//! making a worktree, listing a repository's worktrees row by row, checking one before its
//! removal and removing it. Each runs on the [`Pool`], only for a trusted root whose main
//! repository is trusted too, and tells the app what it found through the function the app
//! gives.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread.
//!
//! - Each job checks its root again before it runs anything: a project no longer trusted, or
//!   whose root is no longer the directory that was trusted, runs no git.
//! - A listing looks at every row without `gh` first, then asks `gh` only for the rows that
//!   need it. A newer listing stops an older one between rows.
//! - A removal runs every check again, in the same job as `git worktree remove`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use oso_platform::privacy::PrivateDir;

use super::checks::{self, Check, Depth, Held, Looked};
use super::create::{self, Created, Order, Step};
use super::facts::RepoFacts;
use super::git::{Gh, Repo, said};
use super::merged::{self, Merged};
use super::pool::Pool;
use crate::project::{Trusted, linked_worktree};
use crate::review::TrustedRoot;
use crate::review::git::MAX_SHORT;

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
    /// Hosted sessions that are busy, by their directory and name.
    pub busy: Vec<(PathBuf, String)>,
    /// The process ids of the `claude`s oso hosts.
    pub hosted: Vec<u32>,
}

impl Snapshot {
    /// What it holds about the worktree at `path`.
    pub fn held(&self, path: &Path) -> Held {
        Held {
            unsaved: self.unsaved.iter().filter(|f| f.starts_with(path)).count(),
            unsent: self.briefs.iter().any(|b| b.starts_with(path)),
            busy: self
                .busy
                .iter()
                .filter(|(dir, _)| dir.starts_with(path))
                .map(|(_, name)| name.clone())
                .collect(),
        }
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
    /// What `gh` said of a listed row's branch, after every row.
    Merged {
        token: u64,
        path: PathBuf,
        merged: Merged,
    },
    /// A removal's checks: what git ignores there, which the removal deletes, or the first
    /// check that refuses.
    Checked {
        token: u64,
        path: PathBuf,
        result: Result<Vec<PathBuf>, Check>,
    },
    /// A removal ended; the branch is kept either way.
    Removed {
        token: u64,
        path: PathBuf,
        result: Result<(), String>,
    },
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
        let gh_host = self.gh_host.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let live = || newest.load(Ordering::SeqCst) == number;
                list(repo, &snapshot, gh_host.as_deref(), &live, token, send);
            },
            move |why| Report::Listed {
                token,
                result: Err(why),
            },
        );
    }

    /// Runs the checks of a removal of the worktree at `path`, in order.
    pub fn check(&self, token: u64, trust: Trust, path: PathBuf, snapshot: Snapshot) {
        let failed_path = path.clone();
        let gh_host = self.gh_host.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let result = check(repo, gh_host.as_deref(), &path, &snapshot);
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

    /// Runs every check of a removal of the worktree at `path` again, then removes it, never
    /// with `--force`, unless a check fails or git ignores more there than the `asked` names
    /// the question was asked about.
    pub fn remove(
        &self,
        token: u64,
        trust: Trust,
        path: PathBuf,
        snapshot: Snapshot,
        asked: usize,
    ) {
        let failed_path = path.clone();
        let gh_host = self.gh_host.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let result = match check(repo, gh_host.as_deref(), &path, &snapshot) {
                    Err(check) => Err(check.why()),
                    Ok(ignored) if ignored.len() > asked => Err(
                        "git ignores more there than the question named: run worktree-remove again"
                            .to_owned(),
                    ),
                    Ok(_) => checks::remove(repo, &path),
                };
                send(Report::Removed {
                    token,
                    path,
                    result,
                });
            },
            move |why| Report::Removed {
                token,
                path: failed_path,
                result: Err(why),
            },
        );
    }
}

/// git in the main checkout of the repository of `trust`, with the empty private directory
/// for hooks under `state`, if that main checkout is trusted too.
fn repo(trust: &Trust, state: Option<&Path>) -> Result<Repo, String> {
    let state = state.ok_or("oso has no state directory, so it runs no git for worktrees")?;
    let hooks = PrivateDir::open(state.join("worktrees").join("hooks"))
        .map_err(|e| format!("Cannot make oso's directory for git: {e}"))?;
    let empty = std::fs::read_dir(hooks.path()).is_ok_and(|mut e| e.next().is_none());
    if !empty {
        return Err("oso's empty directory for git's hooks is not empty".to_owned());
    }
    let main = main_of(trust.root.path(), hooks.path())?;
    // Worktrees take their trust from the main repository (ADR 0114).
    let main_trusted = trust
        .trusted
        .iter()
        .any(|known| oso_core::project::same_place(known, &main));
    if !main_trusted {
        return Err(MAIN_UNTRUSTED.to_owned());
    }
    Repo::open(&main, hooks.path())
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
    token: u64,
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
        let held = snapshot.held(&listed.path);
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
    let held = snapshot.held(&listed.path);
    let gh = Gh::find(repo.main());
    let facts = RepoFacts::gather(repo, gh_host, &snapshot.hosted);
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
