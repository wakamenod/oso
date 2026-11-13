//! Worktree jobs as the app asks for them (ADR 0121): listing branches for `session-worktree`,
//! making a worktree, listing a repository's worktrees row by row, checking one before its
//! removal and removing it. Each runs on the [`Pool`], only for a trusted root, and tells the
//! app what it found through the function the app gives.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_platform::privacy::PrivateDir;

use super::checks::{self, Check, Depth, Held, Looked};
use super::create::{self, Created, Order, Step};
use super::git::{Gh, Repo, said};
use super::pool::Pool;
use crate::project::linked_worktree;
use crate::review::TrustedRoot;
use crate::review::git::MAX_SHORT;

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
            hosted: self.hosted.clone(),
        }
    }
}

/// What a job found.
#[derive(Debug)]
pub enum Report {
    /// The local branches, for `session-worktree`'s completion.
    Branches {
        branches: Result<Vec<String>, String>,
    },
    /// A create job reached a step.
    Step { token: u64, step: Step },
    /// A create job ended.
    Created {
        token: u64,
        result: Result<Created, String>,
    },
    /// A listing found this many linked worktrees; their rows follow.
    Listed {
        token: u64,
        result: Result<usize, String>,
    },
    /// One row of a listing.
    Row { token: u64, looked: Box<Looked> },
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
        Jobs {
            pool: Pool::default(),
            state,
            send: Arc::new(send),
        }
    }

    /// Runs `work` with the repository at `root` on the pool; `failed` makes the report when
    /// the repository cannot be run in.
    fn on_repo(
        &self,
        root: TrustedRoot,
        work: impl FnOnce(&Repo, &dyn Fn(Report)) + Send + 'static,
        failed: impl FnOnce(String) -> Report + Send + 'static,
    ) {
        let send = Arc::clone(&self.send);
        let state = self.state.clone();
        let started = self.pool.run(move || match repo(&root, state.as_deref()) {
            Ok(repo) => work(&repo, &*send),
            Err(why) => send(failed(why)),
        });
        if !started {
            log::warn!("a worktree job was dropped: no thread could start");
        }
    }

    /// Lists the local branches of the repository at `root`.
    pub fn branches(&self, root: TrustedRoot) {
        self.on_repo(
            root,
            move |repo, send| {
                send(Report::Branches {
                    branches: repo.branches(),
                });
            },
            move |why| Report::Branches { branches: Err(why) },
        );
    }

    /// Makes the worktree `order` asks for.
    pub fn create(&self, token: u64, root: TrustedRoot, order: Order) {
        self.on_repo(
            root,
            move |repo, send| {
                let progress = |step| send(Report::Step { token, step });
                let result = create::create(repo, &order, &progress);
                send(Report::Created { token, result });
            },
            move |why| Report::Created {
                token,
                result: Err(why),
            },
        );
    }

    /// Lists the linked worktrees of the repository at `root`, a row each as it is looked at.
    pub fn list(&self, token: u64, root: TrustedRoot, snapshot: Snapshot) {
        self.on_repo(
            root,
            move |repo, send| {
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
                for listed in linked {
                    let held = snapshot.held(&listed.path);
                    let looked = checks::look(repo, gh.as_ref(), listed, &held, Depth::All);
                    send(Report::Row {
                        token,
                        looked: Box::new(looked),
                    });
                }
            },
            move |why| Report::Listed {
                token,
                result: Err(why),
            },
        );
    }

    /// Runs the checks of a removal of the worktree at `path`, in order.
    pub fn check(&self, token: u64, root: TrustedRoot, path: PathBuf, snapshot: Snapshot) {
        let failed_path = path.clone();
        self.on_repo(
            root,
            move |repo, send| {
                let result = check(repo, &path, &snapshot);
                send(Report::Checked {
                    token,
                    path,
                    result,
                });
            },
            move |why| Report::Checked {
                token,
                path: failed_path,
                result: Err(Check::NotLinked(why)),
            },
        );
    }

    /// Removes the worktree at `path`, which passed its checks, never with `--force`.
    pub fn remove(&self, token: u64, root: TrustedRoot, path: PathBuf) {
        let failed_path = path.clone();
        self.on_repo(
            root,
            move |repo, send| {
                let result = checks::remove(repo, &path);
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

/// git in the main checkout of `root`, with the empty private directory for hooks under
/// `state`.
fn repo(root: &TrustedRoot, state: Option<&Path>) -> Result<Repo, String> {
    let state = state.ok_or("oso has no state directory, so it runs no git for worktrees")?;
    let hooks = PrivateDir::open(state.join("worktrees").join("hooks"))
        .map_err(|e| format!("Cannot make oso's directory for git: {e}"))?;
    let empty = std::fs::read_dir(hooks.path()).is_ok_and(|mut e| e.next().is_none());
    if !empty {
        return Err("oso's empty directory for git's hooks is not empty".to_owned());
    }
    let main = main_of(root.path(), hooks.path())?;
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

/// The checks of a removal of the worktree at `path`: what git ignores there, or the first
/// check that refuses.
fn check(repo: &Repo, path: &Path, snapshot: &Snapshot) -> Result<Vec<PathBuf>, Check> {
    let listed = repo.worktrees().map_err(Check::NotLinked)?;
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
    let looked = checks::look(repo, gh.as_ref(), listed, &held, Depth::UntilRefused);
    checks::removable(&looked, &held).map(|()| looked.ignored)
}
