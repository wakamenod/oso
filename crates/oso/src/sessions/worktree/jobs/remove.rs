//! A worktree's removal (ADR 0121, "Worktree removal, as fixed"): every check again, the app's
//! word, then `git worktree remove` with no time limit on a thread of its own,
//! `oso-sessions-remove`.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread.
//!
//! - Once git runs, every check has held and it may be deleting: oso never stops it, as a kill
//!   would leave the worktree half removed. The pool's threads go on with other jobs meanwhile.
//! - A removal holds its path from the job's start until it ends ([`Removals`]): a second
//!   removal of it, its checks, and a session started in it are refused meanwhile, and at most
//!   [`MAX_REMOVALS`] run at once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{Asked, Go, Jobs, Report, Snapshot, Trust, check};
use crate::sessions::worktree::checks;
use crate::sessions::worktree::git::Repo;

/// How long a removal whose checks hold waits for the app's word to go on.
const CLEAR_WAIT: Duration = Duration::from_secs(10);
/// The most removals that run at once: each holds a thread and a git until git ends.
pub const MAX_REMOVALS: usize = 4;

/// The removals asked for and not yet ended, by path. Cloning it shares it.
#[derive(Clone, Debug, Default)]
pub struct Removals(Arc<Mutex<HashSet<PathBuf>>>);

/// A removal's hold on its path, given back when it is dropped: when its git ended, or the job
/// ended before git ran.
struct Hold {
    removals: Removals,
    path: PathBuf,
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.removals.locked().remove(&self.path);
    }
}

/// `path` as the removals hold it: resolved when it can be, so that two spellings of one
/// worktree are one.
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl Removals {
    fn locked(&self) -> MutexGuard<'_, HashSet<PathBuf>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Holds `path` for a removal, unless one of it runs or [`MAX_REMOVALS`] do.
    fn hold(&self, path: &Path) -> Result<Hold, String> {
        let path = key(path);
        let mut held = self.locked();
        if held.contains(&path) {
            return Err(still_runs());
        }
        if held.len() >= MAX_REMOVALS {
            return Err(format!(
                "{MAX_REMOVALS} removals still run: try again once one has ended"
            ));
        }
        held.insert(path.clone());
        Ok(Hold {
            removals: self.clone(),
            path,
        })
    }

    /// Why the worktree at `path` cannot be checked now: a removal of it runs.
    pub fn running_at(&self, path: &Path) -> Option<String> {
        self.locked().contains(&key(path)).then(still_runs)
    }

    /// Why a session cannot start in `directory` now: a removal runs in a worktree it is in.
    pub fn running_under(&self, directory: &Path) -> Option<String> {
        let directory = key(directory);
        (self.locked().iter())
            .any(|path| directory.starts_with(path))
            .then(|| "that worktree is being removed".to_owned())
    }
}

fn still_runs() -> String {
    "a removal of it still runs: worktree_list shows whether it went".to_owned()
}

impl Jobs {
    /// Runs every check of a removal of the worktree at `path` again, then, once the app has
    /// cleared it and said [`Go`], removes it, never with `--force`, unless a check fails or git
    /// ignores more there than the `asked` names the question was asked about.
    pub fn remove(
        &self,
        token: u64,
        trust: Trust,
        path: PathBuf,
        snapshot: Snapshot,
        asked: Asked,
    ) {
        let failed_path = path.clone();
        let gh_host = self.gh_host.clone();
        let report_later = Arc::clone(&self.send);
        let removals = self.removals.clone();
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let ready = removals.hold(&path).and_then(|held| {
                    match (check(repo, gh_host.as_deref(), &path, &snapshot), asked) {
                        (Err(check), _) => Err(check.why()),
                        (Ok(ignored), Asked::Named(named)) if ignored.len() > named => Err(
                            "git ignores more there than the question named: run worktree-remove \
                             again"
                                .to_owned(),
                        ),
                        (Ok(ignored), _) => cleared(token, &path, send).map(|()| (held, ignored)),
                    }
                });
                match ready {
                    Ok((held, ignored)) => {
                        removing(repo.clone(), token, path, ignored, held, report_later);
                    }
                    Err(why) => send(Report::Removed {
                        token,
                        path,
                        result: Err(why),
                    }),
                }
            },
            move |why| Report::Removed {
                token,
                path: failed_path,
                result: Err(why),
            },
        );
    }
}

/// Removes the worktree at `path` of `repo`, whose checks held and which the app cleared, with
/// `ignored` what git ignores there, and says how it went, on a thread of its own: git runs
/// with no time limit there. `held` goes once git has ended. If no thread can start, nothing is
/// removed: git has not run yet.
fn removing(
    repo: Repo,
    token: u64,
    path: PathBuf,
    ignored: Vec<PathBuf>,
    held: Hold,
    send: Arc<dyn Fn(Report) + Send + Sync>,
) {
    let (failed_path, failed_send) = (path.clone(), Arc::clone(&send));
    let started = std::thread::Builder::new()
        .name("oso-sessions-remove".into())
        .spawn(move || {
            let run = std::panic::AssertUnwindSafe(|| checks::remove(&repo, &path));
            let result = std::panic::catch_unwind(run).unwrap_or_else(|_| {
                Err("the removal failed: worktree_list shows what is left".to_owned())
            });
            drop(held);
            send(Report::Removed {
                token,
                path,
                result: result.map(|()| ignored),
            });
        });
    if let Err(e) = started {
        log::warn!("no thread for a worktree removal: {e}");
        failed_send(Report::Removed {
            token,
            path: failed_path,
            result: Err("no thread could start, so nothing was removed".to_owned()),
        });
    }
}

/// Asks the app to clear the worktree at `path` and waits for its word, at most
/// [`CLEAR_WAIT`].
fn cleared(token: u64, path: &Path, send: &dyn Fn(Report)) -> Result<(), String> {
    let (go, word) = mpsc::sync_channel(1);
    send(Report::Clear {
        token,
        path: path.to_owned(),
        go: Go(go),
    });
    word.recv_timeout(CLEAR_WAIT)
        .map_err(|_| "the removal was called off".to_owned())
}
