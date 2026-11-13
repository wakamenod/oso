//! The review thread's requests (ADR 0122): what it keeps between them, and answering each:
//! opening a scope, refreshing it, taking a baseline and carrying comments.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs git and reads files.
//!
//! The repository is found once, from the trusted root, and refused unless its work tree's top
//! is that root. Each refresh lists the scope's files again, lets go of those no longer listed,
//! and reads the rest, the caller's shown files first, within [`REQUEST_BUDGET`].

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::Carry;
use oso_platform::file_meta::Directory;

use super::batch::{Batch, Blob};
use super::diff;
use super::git::{Git, GitError, MAX_SHORT};
use super::id::Id;
use super::id::trim_line;
use super::index::Scratch;
use super::read::{self, Context, Kept};
use super::scope::{self, Scope, ScopeError};
use super::{Asked, Report, Request};

/// How long one request may spend reading and diffing files.
pub(crate) const REQUEST_BUDGET: Duration = Duration::from_secs(5);
/// How long carrying one file's comments may spend reading old sides and matching lines.
const CARRY_DEADLINE: Duration = Duration::from_secs(1);

/// Where the repository is, once found.
struct Repository {
    git: Git,
    /// The repository's own index.
    index: PathBuf,
}

/// The thread's state.
pub(super) struct Reader {
    root: PathBuf,
    state: Option<PathBuf>,
    repository: Option<Repository>,
    scratch: Option<Scratch>,
    batch: Batch,
    scope: Option<Scope>,
    kept: Kept,
    generation: u64,
}

type Out<'a> = dyn FnMut(Report) -> bool + 'a;

/// Gives the next carry waiting, if any.
pub(super) type Carries<'a> = dyn FnMut() -> Option<Carry> + 'a;

impl Reader {
    /// State for the project at `root`, with index copies under `state`, swept now.
    pub(super) fn new(root: &Path, state: Option<&Path>) -> Reader {
        let scratch = state.and_then(|state| {
            Scratch::open(state)
                .map_err(|e| log::warn!("reviews have no private place for index copies: {e}"))
                .ok()
        });
        Reader {
            root: root.to_owned(),
            state: state.map(Path::to_owned),
            repository: None,
            scratch,
            batch: Batch::default(),
            scope: None,
            kept: Kept::default(),
            generation: 0,
        }
    }

    /// A new state after a request panicked, keeping the scope and the count of generations.
    pub(super) fn fresh(self) -> Reader {
        let mut fresh = Reader::new(&self.root, self.state.as_deref());
        fresh.scope = self.scope;
        fresh.generation = self.generation;
        fresh
    }

    /// Answers `request`, taking each carry `carries` gives between its files.
    pub(super) fn answer(
        &mut self,
        request: Request,
        stop: &dyn Fn() -> bool,
        carries: &mut Carries<'_>,
        out: &mut Out<'_>,
    ) {
        let asked = super::asked(&request);
        let answered = match request {
            Request::Open(scope) => self.open(&scope, stop, carries, out),
            Request::Refresh { first } => self.refresh(&first, stop, carries, out),
            Request::Baseline => self.baseline(stop, out),
            Request::Carry(carry) => self.carry(carry, out),
        };
        if let Err(e) = answered {
            out(match e {
                // Stopped as its root was revoked: the host knows why.
                ScopeError::Stopped => return,
                ScopeError::BaselineGone => Report::BaselineGone,
                e => Report::Failed {
                    asked,
                    why: message(&e),
                },
            });
        }
    }

    /// How many files `scope` differs in, listed as a review lists them with no file read, and
    /// how many more it left out. It runs git no more once `stop` says so.
    pub(super) fn count(
        &mut self,
        scope: &Scope,
        stop: &dyn Fn() -> bool,
    ) -> Result<(usize, usize), ScopeError> {
        let (git, index) = {
            let repository = self.repository()?;
            (repository.git.clone(), repository.index.clone())
        };
        let sides = scope::resolve(&git, scope, stop)?;
        let listing = scope::list(&git, &sides, &index, self.scratch.as_mut(), stop)?;
        Ok((
            listing.entries.len(),
            listing.more + usize::from(listing.cut),
        ))
    }

    /// The base the branch scope compares with: the branch's upstream, unless it is the same
    /// branch on a remote (`origin/<branch>`), which holds what is being reviewed; else the
    /// branch `origin/HEAD` names; else `main` or `master` where there is one.
    pub(super) fn branch_base(&mut self) -> Result<Option<String>, ScopeError> {
        let git = self.repository()?.git.clone();
        let named = |args: &[&str]| -> Option<String> {
            let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
            let out = git.run(&args, &[], MAX_SHORT).ok()?;
            let name = std::str::from_utf8(trim_line(&out)).ok()?.to_owned();
            super::git::check_revision(&name).ok()?;
            Some(name)
        };
        // No `--end-of-options`, which `rev-parse` would print back: the one argument is fixed.
        let upstream = [
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ];
        let origin = [
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ];
        let branch = ["symbolic-ref", "--quiet", "--short", "HEAD"];
        let upstream = named(&upstream).filter(|upstream| {
            let tracked = upstream.split_once('/').map(|(_, name)| name);
            tracked.is_none() || tracked != named(&branch).as_deref()
        });
        if let Some(name) = upstream.or_else(|| named(&origin)) {
            return Ok(Some(name));
        }
        for name in ["main", "master"] {
            if git.resolve(name, "commit")?.is_some() {
                return Ok(Some(name.to_owned()));
            }
        }
        Ok(None)
    }

    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.kept.held()
    }

    fn repository(&mut self) -> Result<&Repository, ScopeError> {
        if self.repository.is_none() {
            let hooks = self.scratch.as_ref().ok_or(ScopeError::NoScratch)?.hooks();
            self.repository = Some(find_repository(&self.root, hooks)?);
        }
        self.repository
            .as_ref()
            .ok_or(ScopeError::Git(GitError::Unexpected("no repository")))
    }

    fn open(
        &mut self,
        scope: &Scope,
        stop: &dyn Fn() -> bool,
        carries: &mut Carries<'_>,
        out: &mut Out<'_>,
    ) -> Result<(), ScopeError> {
        if self.scope.as_ref() != Some(scope) {
            self.kept.clear();
        }
        self.scope = Some(scope.clone());
        self.refresh(&[], stop, carries, out)
    }

    fn refresh(
        &mut self,
        first: &[Box<[u8]>],
        stop: &dyn Fn() -> bool,
        carries: &mut Carries<'_>,
        out: &mut Out<'_>,
    ) -> Result<(), ScopeError> {
        let Some(scope) = self.scope.clone() else {
            return Err(ScopeError::Git(GitError::Unexpected("no scope opened")));
        };
        let (git, index) = {
            let repository = self.repository()?;
            (repository.git.clone(), repository.index.clone())
        };
        // Resolved again each time: `HEAD`, a branch or the index may have moved.
        let listed = scope::resolve(&git, &scope, stop).and_then(|sides| {
            let listing = scope::list(&git, &sides, &index, self.scratch.as_mut(), stop)?;
            Ok((sides, listing))
        });
        let (sides, listing) = match listed {
            // Stopped before the listing: the reading ends as one stopped among its files.
            Err(ScopeError::Stopped) => {
                out(Report::Read {
                    generation: self.generation,
                    stopped: true,
                });
                return Ok(());
            }
            listed => listed?,
        };
        self.generation += 1;
        let generation = self.generation;
        self.kept.keep_only(&listing.entries);
        let entries: Arc<[_]> = listing.entries.into();
        let listed = Report::Listed {
            generation,
            entries: Arc::clone(&entries),
            more: listing.more,
            cut: listing.cut,
        };
        if !out(listed) {
            return Ok(());
        }
        let top = Directory::open(git.top()).ok();
        let shown: HashSet<&[u8]> = first.iter().map(|path| &**path).collect();
        let order = entries
            .iter()
            .filter(|entry| shown.contains(&*entry.path))
            .chain(entries.iter().filter(|entry| !shown.contains(&*entry.path)));
        let mut cx = Context {
            git: &git,
            batch: &mut self.batch,
            kept: &mut self.kept,
            top: top.as_ref(),
            new: &sides.new,
            budget: Instant::now() + REQUEST_BUDGET,
        };
        for entry in order {
            // A file's comments are carried as they come, the running request going on. Their
            // time is not the request's: its budget moves on by what they took.
            let started = Instant::now();
            while let Some(carry) = carries() {
                if !out(carry_caught(&git, cx.batch, carry)) {
                    return Ok(());
                }
            }
            cx.budget += started.elapsed();
            if stop() {
                out(Report::Read {
                    generation,
                    stopped: true,
                });
                return Ok(());
            }
            let Some(reading) = read::read(&mut cx, entry) else {
                continue;
            };
            let reading = Arc::new(reading);
            if !out(Report::File {
                generation,
                reading,
            }) {
                return Ok(());
            }
        }
        out(Report::Read {
            generation,
            stopped: false,
        });
        Ok(())
    }

    fn baseline(&mut self, stop: &dyn Fn() -> bool, out: &mut Out<'_>) -> Result<(), ScopeError> {
        self.repository()?;
        let (Some(repository), Some(scratch)) = (&self.repository, &mut self.scratch) else {
            return Err(ScopeError::NoScratch);
        };
        let (tree, left_out) = scope::baseline(&repository.git, &repository.index, scratch, stop)?;
        if left_out > 0 {
            log::warn!("a review's baseline left out at least {left_out} files git could not read");
        }
        out(Report::Baseline { tree, left_out });
        Ok(())
    }

    fn carry(&mut self, carry: Carry, out: &mut Out<'_>) -> Result<(), ScopeError> {
        let git = self.repository()?.git.clone();
        out(carry_with(&git, &mut self.batch, carry));
        Ok(())
    }
}

/// [`carry_with`], answered as a failed carry if it panics, so that each carry asked is
/// answered once, in order, and the request that took it goes on. `batch` starts over then, as
/// what it was reading is unknown.
fn carry_caught(git: &Git, batch: &mut Batch, carry: Carry) -> Report {
    caught(|| carry_with(git, batch, carry)).unwrap_or_else(|why| {
        *batch = Batch::default();
        why
    })
}

/// What `run` reports, or a failed carry's report if it panics.
fn caught(run: impl FnOnce() -> Report) -> Result<Report, Report> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).map_err(|_| {
        log::warn!("a review's carry panicked");
        Report::Failed {
            asked: Asked::Carry,
            why: "the review's thread failed to carry comments".into(),
        }
    })
}

/// Carries `carry` within [`CARRY_DEADLINE`], reading old sides through `batch`.
fn carry_with(git: &Git, batch: &mut Batch, carry: Carry) -> Report {
    let deadline = Instant::now() + CARRY_DEADLINE;
    let mut cut_short = false;
    let carried = carry.run(
        |a, b| {
            let mut blob = |id: &[u8]| {
                if Instant::now() >= deadline {
                    return None;
                }
                match batch.read(git, &Id::parse(id)?) {
                    Ok(Blob::Bytes(bytes)) => Some(bytes),
                    _ => None,
                }
            };
            let old = blob(a)?;
            let new = blob(b)?;
            diff::base_hunks(&old, &new, deadline)
        },
        |a, b| {
            let pairs = diff::matches(a, b, deadline);
            cut_short = cut_short || Instant::now() >= deadline;
            pairs
        },
    );
    let cut_short = cut_short || Instant::now() >= deadline;
    Report::Carried { carried, cut_short }
}

/// The repository whose work tree's top is `root`: git to run there, and its index. git never
/// looks above the root, looks for hooks only in `hooks`, and a top elsewhere is refused.
fn find_repository(root: &Path, hooks: &Path) -> Result<Repository, ScopeError> {
    let git = Git::new(root, hooks)?;
    let args = ["rev-parse", "--show-toplevel", "--absolute-git-dir"].map(OsStr::new);
    let out = git.run(&args, &[], MAX_SHORT)?;
    let mut lines = out.split(|&byte| byte == b'\n');
    let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else {
        return Err(GitError::Unexpected("no work tree").into());
    };
    let (top, git_dir) = (path_of(top)?, path_of(git_dir)?);
    let same = |a: &Path, b: &Path| match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same(&top, root) {
        return Err(ScopeError::Elsewhere);
    }
    Ok(Repository {
        index: git_dir.join("index"),
        git: git.in_top(&top),
    })
}

#[cfg(unix)]
fn path_of(bytes: &[u8]) -> Result<PathBuf, ScopeError> {
    use std::os::unix::ffi::OsStrExt;
    if bytes.is_empty() {
        return Err(GitError::Unexpected("an empty path").into());
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_of(bytes: &[u8]) -> Result<PathBuf, ScopeError> {
    let text = std::str::from_utf8(trim_line(bytes))
        .map_err(|_| ScopeError::Git(GitError::Unexpected("a path that is not UTF-8")))?;
    if text.is_empty() {
        return Err(GitError::Unexpected("an empty path").into());
    }
    Ok(PathBuf::from(text))
}

/// What a person reads for `error`, with no file's text.
fn message(error: &ScopeError) -> String {
    match error {
        ScopeError::Git(e) => e.to_string(),
        ScopeError::NoSuchRevision(revision) => format!("no commit is named {revision:?}"),
        ScopeError::BaselineGone => "the session's baseline is gone".into(),
        ScopeError::NoParent => "its parent is not in this clone".into(),
        ScopeError::Elsewhere => {
            "git found a repository whose top is not the project's root".into()
        }
        ScopeError::NoScratch => {
            "there is no private place for the review's copies of the index and its empty \
             directory for git's hooks"
                .into()
        }
        ScopeError::Copy(e) => format!("cannot copy the index: {e}"),
        ScopeError::Stopped => "stopped".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Asked, Report, caught};

    #[test]
    fn a_carry_that_panics_is_answered_as_a_failed_carry() {
        let failed = caught(|| panic!("forced")).unwrap_err();
        assert!(matches!(
            failed,
            Report::Failed {
                asked: Asked::Carry,
                ..
            }
        ));
    }
}
