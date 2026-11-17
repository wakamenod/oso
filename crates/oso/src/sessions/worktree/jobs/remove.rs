//! A worktree's removal (ADR 0121, "Worktree removal, as fixed"): every check again, the app's
//! word, then `git worktree remove` with no time limit on a thread of its own,
//! `oso-sessions-remove`.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread.
//!
//! - Once git runs, every check has held and it may be deleting: nothing but the person's
//!   `worktree-remove-stop` stops it, as a kill would leave the worktree half removed. The
//!   pool's threads go on with other jobs meanwhile.
//! - A removal holds its path from the job's start until it ends ([`Removals`]): a second
//!   removal of it, its checks, and a session started in it are refused meanwhile, and at most
//!   [`MAX_REMOVALS`] run at once.
//! - Each removal is in one [`Phase`], changed by one atomic step: before git, in git, or
//!   stopped. A stop before git removes nothing, and only a stop the person confirmed reaches a
//!   removal in git, which then gets `SIGTERM`, then `SIGKILL` (ADR 0127's long runner; ADR
//!   0121, "Stopping a removal, as built").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{Asked, Go, Jobs, Report, Snapshot, Trust, check};
use crate::sessions::worktree::checks::{self, Check};
use crate::sessions::worktree::git::Repo;

/// How long a removal whose checks hold waits for the app's word to go on.
const CLEAR_WAIT: Duration = Duration::from_secs(10);
/// The most removals that run at once: each holds a thread and a git until git ends.
pub const MAX_REMOVALS: usize = 4;
/// Why a removal the person stopped before git ran ended.
pub const STOPPED_BEFORE_GIT: &str = "stopped by the person before git ran: nothing was removed";
/// Why a removal the person stopped after the app cleared its worktree, before git ran, ended.
pub const STOPPED_AFTER_CLEAR: &str = "stopped by the person before git ran: nothing was \
     removed, though its idle sessions were stopped";
/// Why a removal the person stopped while git ran ended, its `.git` still there.
pub const STOPPED_IN_GIT: &str = "stopped by the person while git ran: it may be half removed. \
     What git deleted stays deleted (tracked files are in its branch); remove the rest by hand";
/// Why a removal the person stopped while git ran ended, its `.git` gone.
pub const STOPPED_WITHOUT_GIT_FILE: &str = "stopped by the person while git ran: it is half \
     removed and its .git is gone, so git run there reaches the main repository. Remove the \
     directory by hand, then run git worktree prune";

/// Where a removal is: before git (its checks, or the wait for the app's word), in git, or
/// stopped. Only [`Removals::stop_confirmed`] moves one in git to stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    BeforeGit = 0,
    InGit = 1,
    Stopped = 2,
}

impl Phase {
    fn from(value: u8) -> Phase {
        match value {
            0 => Phase::BeforeGit,
            1 => Phase::InGit,
            _ => Phase::Stopped,
        }
    }
}

/// What a stop found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// It was before git, and is stopped: nothing is removed.
    BeforeGit,
    /// git may be deleting: the person is asked first, and nothing was stopped.
    Ask,
    /// git may be deleting, and was told to stop.
    InGit,
    /// No such removal runs: it has ended.
    Ended,
}

/// The removals asked for and not yet ended, by their jobs' tokens. Cloning it shares it.
/// Nothing reads a file under its lock, which the UI thread takes too.
#[derive(Clone, Debug, Default)]
pub struct Removals(Arc<Mutex<HashMap<u64, Entry>>>);

/// One removal asked for.
#[derive(Debug)]
struct Entry {
    /// Its path as it was asked for.
    asked: PathBuf,
    /// Its path resolved, once its job has started: it holds that path.
    resolved: Option<PathBuf>,
    state: State,
}

/// What a removal and the person's stop share.
#[derive(Clone, Debug, Default)]
struct State {
    /// Its [`Phase`].
    phase: Arc<AtomicU8>,
    /// What tells git's run to stop, set only in git.
    kill: Arc<AtomicBool>,
}

impl State {
    fn phase(&self) -> Phase {
        Phase::from(self.phase.load(Ordering::SeqCst))
    }

    /// Before git to in git, unless it was stopped.
    fn start_git(&self) -> bool {
        let (before, git) = (Phase::BeforeGit as u8, Phase::InGit as u8);
        (self.phase)
            .compare_exchange(before, git, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Whether git was told to stop.
    fn killed(&self) -> bool {
        self.kill.load(Ordering::SeqCst)
    }
}

/// A removal that runs, or waits for its job to start, as the UI thread sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Running {
    /// Its job's token, which a stop names.
    pub id: u64,
    /// Its path as it was asked for.
    pub asked: PathBuf,
    /// Its path resolved, as `*Worktrees*` names it, once its job has started.
    pub resolved: Option<PathBuf>,
    pub phase: Phase,
}

impl Running {
    /// The path to name it by: resolved if its job has started.
    pub fn path(&self) -> &Path {
        self.resolved.as_deref().unwrap_or(&self.asked)
    }
}

/// A removal's place among the removals, given back when it is dropped: when its git ended,
/// or its job ended, or was dropped, before git ran.
struct Hold {
    removals: Removals,
    id: u64,
    state: State,
}

impl Hold {
    fn stopped(&self) -> bool {
        self.state.phase() == Phase::Stopped
    }

    /// Holds `path` resolved for this removal, unless a removal of it runs or
    /// [`MAX_REMOVALS`] do. It reads the file system, so it runs on the job's thread.
    fn resolve(&self, path: &Path) -> Result<(), String> {
        let key = key(path);
        let mut held = self.removals.locked();
        let resolved = || held.values().filter_map(|entry| entry.resolved.as_ref());
        if resolved().any(|other| *other == key) {
            return Err(Check::Removing.why());
        }
        if resolved().count() >= MAX_REMOVALS {
            return Err(format!(
                "{MAX_REMOVALS} removals still run: try again once one has ended"
            ));
        }
        if let Some(entry) = held.get_mut(&self.id) {
            entry.resolved = Some(key);
        }
        Ok(())
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.removals.locked().remove(&self.id);
    }
}

impl std::fmt::Debug for Hold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hold").field("id", &self.id).finish()
    }
}

/// `path` as the removals hold it: resolved when it can be, so that two spellings of one
/// worktree are one. It reads the file system.
fn key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

impl Removals {
    fn locked(&self) -> MutexGuard<'_, HashMap<u64, Entry>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Enters the removal `id` of `path` as it is asked for, before its job waits for a thread:
    /// it can be stopped from then on. It reads no file.
    fn enter(&self, id: u64, path: &Path) -> Hold {
        let state = State::default();
        let entry = Entry {
            asked: path.to_owned(),
            resolved: None,
            state: state.clone(),
        };
        self.locked().insert(id, entry);
        Hold {
            removals: self.clone(),
            id,
            state,
        }
    }

    /// Whether a removal of the worktree at `path` runs. It resolves `path`, so it belongs off
    /// the UI thread.
    pub fn running_at(&self, path: &Path) -> bool {
        self.any_resolved(path, |resolved, key| resolved == key)
    }

    /// Why a session cannot start in `directory` now: a removal runs in a worktree it is in.
    pub fn running_under(&self, directory: &Path) -> Option<String> {
        self.any_resolved(directory, |resolved, key| key.starts_with(resolved))
            .then(|| "that worktree is being removed".to_owned())
    }

    /// Whether `test` holds for a resolved removal's path and `path` resolved, which is read
    /// before the lock is taken.
    fn any_resolved(&self, path: &Path, test: impl Fn(&Path, &Path) -> bool) -> bool {
        if self.locked().values().all(|entry| entry.resolved.is_none()) {
            return false;
        }
        let key = key(path);
        (self.locked().values())
            .filter_map(|entry| entry.resolved.as_deref())
            .any(|resolved| test(resolved, &key))
    }

    /// The removals that run or wait for their jobs. It reads no file.
    pub fn running(&self) -> Vec<Running> {
        let mut running: Vec<Running> = (self.locked().iter())
            .map(|(&id, entry)| Running {
                id,
                asked: entry.asked.clone(),
                resolved: entry.resolved.clone(),
                phase: entry.state.phase(),
            })
            .collect();
        running.sort_by_key(|r| r.id);
        running
    }

    fn state_of(&self, id: u64) -> Option<State> {
        self.locked().get(&id).map(|entry| entry.state.clone())
    }

    /// Stops the removal `id` if git has not run, in one step with its start of git: once git
    /// may be deleting, it stops nothing and says to ask.
    pub fn stop_before_git(&self, id: u64) -> Stop {
        let Some(state) = self.state_of(id) else {
            return Stop::Ended;
        };
        let (before, stopped) = (Phase::BeforeGit as u8, Phase::Stopped as u8);
        match (state.phase).compare_exchange(before, stopped, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Stop::BeforeGit,
            Err(now) => match Phase::from(now) {
                Phase::InGit => Stop::Ask,
                Phase::Stopped if state.killed() => Stop::InGit,
                Phase::BeforeGit | Phase::Stopped => Stop::BeforeGit,
            },
        }
    }

    /// Stops the removal `id`, in git too: the person was asked.
    pub fn stop_confirmed(&self, id: u64) -> Stop {
        let Some(state) = self.state_of(id) else {
            return Stop::Ended;
        };
        match Phase::from(state.phase.swap(Phase::Stopped as u8, Ordering::SeqCst)) {
            Phase::BeforeGit => Stop::BeforeGit,
            Phase::InGit => {
                state.kill.store(true, Ordering::SeqCst);
                Stop::InGit
            }
            Phase::Stopped if state.killed() => Stop::InGit,
            Phase::Stopped => Stop::BeforeGit,
        }
    }

    /// Whether the removal `id` was stopped, for the app as it clears a worktree.
    pub fn stopped(&self, id: u64) -> bool {
        self.state_of(id)
            .is_some_and(|state| state.phase() == Phase::Stopped)
    }
}

/// The words an agent hears for a removal that ended with `why`: a removal the person stopped
/// in git is left for the person, never for the agent to finish.
pub fn for_agent(why: &str) -> &str {
    if why == STOPPED_IN_GIT || why == STOPPED_WITHOUT_GIT_FILE {
        "stopped by the person while git ran: the worktree is left for the person. Do not remove \
         or change it"
    } else {
        why
    }
}

impl Jobs {
    /// The removals that run. It reads no file.
    pub fn running_removals(&self) -> Vec<Running> {
        self.removals.running()
    }

    /// Stops the removal `id` if git has not run; else says to ask (see
    /// [`Removals::stop_before_git`]).
    pub fn stop_removal_before_git(&self, id: u64) -> Stop {
        self.removals.stop_before_git(id)
    }

    /// Stops the removal `id`, in git too, once the person was asked.
    pub fn stop_removal_confirmed(&self, id: u64) -> Stop {
        self.removals.stop_confirmed(id)
    }

    /// Whether the removal `id` was stopped: the app then clears nothing.
    pub fn removal_stopped(&self, id: u64) -> bool {
        self.removals.stopped(id)
    }

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
        // Entered now, so that a stop finds it while it waits for a thread.
        let held = self.removals.enter(token, &path);
        self.on_repo(
            trust,
            false,
            move |_, repo, send| {
                let resolved = held.resolve(&path).and_then(|()| {
                    // Stopped while it waited for a thread: nothing is checked or cleared.
                    if held.stopped() {
                        Err(STOPPED_BEFORE_GIT.to_owned())
                    } else {
                        Ok(())
                    }
                });
                let ready = resolved.and_then(|()| {
                    match (check(repo, gh_host.as_deref(), &path, &snapshot), asked) {
                        (Err(check), _) => Err(check.why()),
                        (Ok(ignored), Asked::Named(named)) if ignored.len() > named => Err(
                            "git ignores more there than the question named: run worktree-remove \
                             again"
                                .to_owned(),
                        ),
                        // A removal stopped during its checks clears nothing.
                        (Ok(_), _) if held.stopped() => Err(STOPPED_BEFORE_GIT.to_owned()),
                        // One stopped while the app was to clear it is called off there.
                        (Ok(ignored), _) => match cleared(token, &path, send) {
                            Err(_) if held.stopped() => Err(STOPPED_BEFORE_GIT.to_owned()),
                            cleared => cleared.map(|()| ignored),
                        },
                    }
                });
                match ready {
                    Ok(ignored) => {
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
            let result = if held.state.start_git() {
                let kill = &held.state.kill;
                let run = std::panic::AssertUnwindSafe(|| checks::remove(&repo, &path, kill));
                match std::panic::catch_unwind(run) {
                    Ok(Err(_)) if kill.load(Ordering::SeqCst) => left_after_stop(&path),
                    Ok(result) => result,
                    Err(_) => {
                        Err("the removal failed: worktree_list shows what is left".to_owned())
                    }
                }
            } else {
                Err(STOPPED_AFTER_CLEAR.to_owned())
            };
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

/// What a removal stopped in git left at `path`: nothing, as git may have finished first; the
/// worktree half removed; or that without its `.git`, where git finds the main repository.
fn left_after_stop(path: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    if std::fs::symlink_metadata(path.join(".git")).is_err() {
        return Err(STOPPED_WITHOUT_GIT_FILE.to_owned());
    }
    Err(STOPPED_IN_GIT.to_owned())
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use super::{
        Phase, Removals, STOPPED_IN_GIT, STOPPED_WITHOUT_GIT_FILE, Stop, for_agent, left_after_stop,
    };

    #[test]
    fn a_removal_waiting_for_its_job_can_be_stopped_and_git_then_never_starts() {
        let removals = Removals::default();
        let held = removals.enter(7, Path::new("/nowhere/a"));
        let running = removals.running();
        assert_eq!((running[0].id, running[0].resolved.as_deref()), (7, None));
        assert_eq!(running[0].path(), Path::new("/nowhere/a"));
        assert_eq!(removals.stop_before_git(7), Stop::BeforeGit);
        assert!(removals.stopped(7) && held.stopped());
        assert!(held.resolve(Path::new("/nowhere/a")).is_ok());
        assert!(!held.state.start_git(), "git does not start once stopped");
        assert_eq!(removals.stop_confirmed(7), Stop::BeforeGit);
    }

    #[test]
    fn a_removal_in_git_is_stopped_only_when_confirmed() {
        let removals = Removals::default();
        let held = removals.enter(7, Path::new("/nowhere/a"));
        assert!(held.state.start_git());
        // The person is asked first; nothing is stopped meanwhile.
        assert_eq!(removals.stop_before_git(7), Stop::Ask);
        assert!(!held.state.killed() && !removals.stopped(7));
        assert_eq!(removals.stop_confirmed(7), Stop::InGit);
        assert!(held.state.killed());
        // Asked again while git is being stopped, it says so.
        assert_eq!(removals.stop_before_git(7), Stop::InGit);
        assert_eq!(removals.stop_confirmed(7), Stop::InGit);
    }

    #[test]
    fn a_stop_names_its_removal_not_a_later_one_of_the_same_path() {
        let removals = Removals::default();
        let path = Path::new("/nowhere/a");
        let first = removals.enter(1, path);
        first.resolve(path).unwrap();
        let second = removals.enter(2, path);
        assert!(second.resolve(path).is_err(), "a removal of it runs");
        drop((first, second));
        let third = removals.enter(3, path);
        third.resolve(path).unwrap();
        assert!(third.state.start_git());
        assert_eq!(removals.stop_confirmed(1), Stop::Ended);
        assert_eq!(
            third.state.phase(),
            Phase::InGit,
            "the later one is not touched"
        );
        assert!(!third.state.killed());
    }

    #[test]
    fn only_one_of_a_quiet_stop_and_the_start_of_git_wins() {
        for _ in 0..500 {
            let removals = Removals::default();
            let held = Arc::new(removals.enter(1, Path::new("/nowhere/a")));
            let starter = Arc::clone(&held);
            let started = std::thread::spawn(move || starter.state.start_git());
            let stop = removals.stop_before_git(1);
            let started = started.join().unwrap();
            assert_ne!(started, stop == Stop::BeforeGit, "{stop:?}");
            assert!(
                !held.state.kill.load(Ordering::SeqCst),
                "never killed unasked"
            );
        }
    }

    #[test]
    fn what_a_stop_in_git_left_is_named() {
        let dir = std::env::temp_dir().join(format!("oso-left-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(left_after_stop(&dir), Ok(()), "gone: git finished first");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(left_after_stop(&dir), Err(STOPPED_WITHOUT_GIT_FILE.into()));
        std::fs::write(dir.join(".git"), "gitdir: x\n").unwrap();
        assert_eq!(left_after_stop(&dir), Err(STOPPED_IN_GIT.into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_agent_is_never_told_to_finish_a_stopped_removal() {
        for why in [STOPPED_IN_GIT, STOPPED_WITHOUT_GIT_FILE] {
            let said = for_agent(why);
            assert!(said.contains("left for the person"), "{said}");
            assert!(
                !said.contains("by hand") && !said.contains("prune"),
                "{said}"
            );
        }
        assert_eq!(for_agent("git failed"), "git failed");
    }
}
