//! A pull request as a review compares it (ADR 0122, step 12b): what `gh` says its base and
//! head are, their commits fetched when this clone lacks them, and the worktree its head
//! branch is checked out in.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs `gh` and git on the
//! review's thread, and for a start in a space (ADR 0163) on `oso-review-head`, which finds
//! where the head is checked out before the review opens.
//!
//! - `gh` is asked as the pull request buffers ask it (ADR 0127, step 5a): about the GitHub
//!   repository the branch's remote names, else `origin`'s, from an empty private directory,
//!   killed past 30 s. It runs on a thread of its own, `oso-review-gh`, so that a stop ends the
//!   wait at once and carries are answered meanwhile; a run a stop left waiting is taken up by
//!   the next request of that pull request, so that at most one runs per review. Only its whole commit ids reach git, and
//!   its names are anyone's words: the base branch reaches git only as a plain branch name.
//! - A commit this clone lacks is fetched from that remote, `refs/pull/N/head` or the base
//!   branch, whichever is missing, writing objects alone (`--no-write-fetch-head`,
//!   `--refmap=`): no ref moves, and `FETCH_HEAD` names nothing a later merge would take. It
//!   keeps the review's empty hooks directory ([`Git::for_fetch`]), with no tags, submodules or
//!   upkeep, and no time limit, as a fetch the person starts has none: the review's stop ends
//!   it, and one still running as oso quits goes on until it ends or fails. Of what it says,
//!   only git's own last line is shown, never a server's own error or `remote:` line.
//! - The old side is where the head left the base (`git merge-base`), as GitHub's "Files
//!   changed" shows it, not the base itself, which also holds the base's later changes. When
//!   the head branch is checked out here, not a fork's, and `HEAD` holds the pull request's
//!   head commit, the new side is the work tree, so the person's changes show; a branch behind
//!   the pull request shows its head commit instead, so no pushed commit is hidden. Comments
//!   go to a worktree only under the same rule, looked at no more than [`MAX_CANDIDATES`].

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use serde_json::Value;

use super::scope::{New, ScopeError, Sides, merge_base};
use crate::git::{GH_LIMIT, Gh, Git, GitError, Id, MAX_GH};
use crate::git::{PullTarget, empty_hooks, list_worktrees, own_line, printable};

/// The fields of `gh pr view --json` a review of a pull request reads.
const SIDES_FIELDS: &str =
    "number,title,baseRefName,baseRefOid,headRefName,headRefOid,isCrossRepository";
/// The most bytes of a title or a branch's name kept.
const MAX_TEXT: usize = 4 * 1024;
/// How often a wait for `gh` looks at the stop and what waits meanwhile.
const POLL: Duration = Duration::from_millis(50);
/// The most bytes of a fetch's last output kept, to find git's own words in.
const MAX_SAID: usize = 16 * 1024;
/// The most worktrees of the head branch's name looked at, as git checks a branch out once.
const MAX_CANDIDATES: usize = 4;

/// A run of `gh` asked about pull request `number`, whose answer comes on `answer`.
pub(super) struct Asking {
    number: u64,
    answer: mpsc::Receiver<Result<Vec<u8>, String>>,
}

/// What a review compares of a pull request, as `gh pr view` gives it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrSides {
    number: u64,
    title: String,
    /// The branch it merges into, and that branch's commit now.
    base_branch: String,
    base: Id,
    /// Its head branch, and the head's commit.
    head_branch: String,
    head: Id,
    /// Its head branch is a fork's.
    fork: bool,
}

/// What a review compares of a pull request in `gh pr view`'s answer: `None` unless both
/// commits are whole ids.
fn read_sides(json: &[u8]) -> Option<PrSides> {
    let value = serde_json::from_slice::<Value>(json).ok()?;
    let id = |field: &str| Id::parse(value.get(field)?.as_str()?.as_bytes());
    Some(PrSides {
        number: value.get("number")?.as_u64()?,
        title: text(value.get("title")?)?,
        base_branch: text(value.get("baseRefName")?)?,
        base: id("baseRefOid")?,
        head_branch: text(value.get("headRefName")?)?,
        head: id("headRefOid")?,
        fork: value.get("isCrossRepository")?.as_bool()?,
    })
}

/// A string's text, at most [`MAX_TEXT`] bytes of it, cut at a character.
fn text(value: &Value) -> Option<String> {
    let text = value.as_str()?;
    Some(text[..text.floor_char_boundary(MAX_TEXT)].to_owned())
}

/// What the review found of a pull request, for the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrFound {
    pub(crate) number: u64,
    /// Its title, anyone's words.
    pub(crate) title: String,
    /// Its head branch, anyone's words.
    pub(crate) head_branch: String,
    /// The worktree its head branch is checked out in, holding its head commit, if one is and
    /// it is not a fork's.
    pub(crate) worktree: Option<PathBuf>,
    /// Its head branch is the one checked out here, holding its head commit, so the new side
    /// is the work tree.
    pub(crate) head_checked_out: bool,
    /// Its head commit (ADR 0167).
    pub(crate) head: Id,
}

/// What a resolve tells its caller while it waits.
pub(super) enum Meanwhile<'a> {
    /// Nothing new: answer what waits, as carries.
    Idle,
    /// A fetch of pull request `number` from `remote` starts.
    Fetching { number: u64, remote: &'a str },
}

/// What a resolve is given: when to stop, and whom to tell meanwhile, who says `false` once
/// the host has gone.
pub(super) struct Waits<'a> {
    pub(super) stop: &'a dyn Fn() -> bool,
    pub(super) tell: &'a mut dyn FnMut(Meanwhile<'_>) -> bool,
}

impl Waits<'_> {
    fn go_on(&mut self) -> Result<(), ScopeError> {
        if (self.stop)() || !(self.tell)(Meanwhile::Idle) {
            return Err(ScopeError::Stopped);
        }
        Ok(())
    }
}

/// The sides of pull request `number`, asking `gh` with its empty directory under `state`, and
/// fetching what this clone lacks.
pub(super) fn resolve(
    git: &Git,
    state: &Path,
    number: u64,
    asking: &mut Option<Asking>,
    waits: &mut Waits<'_>,
) -> Result<(Sides, PrFound), ScopeError> {
    waits.go_on()?;
    let asker = Asker::of(git, state)?;
    resolve_with(git, &asker.from(), number, asking, waits)
}

/// What `gh` is asked with: the program, its empty private directory and the repository asked
/// about, as both [`resolve`] and [`head_worktree`] find them.
struct Asker {
    gh: Gh,
    cwd: PathBuf,
    target: PullTarget,
}

impl Asker {
    fn of(git: &Git, state: &Path) -> Result<Asker, ScopeError> {
        let target = PullTarget::of(git).map_err(ScopeError::Refused)?;
        let gh = Gh::find(git.top()).ok_or_else(|| {
            ScopeError::Refused(
                "gh is not installed, or not on PATH: oso reads pull requests through gh".into(),
            )
        })?;
        let cwd = empty_hooks(state, "gh").map_err(ScopeError::Refused)?;
        Ok(Asker { gh, cwd, target })
    }

    fn from(&self) -> PullFrom<'_> {
        PullFrom {
            gh: &self.gh,
            cwd: &self.cwd,
            target: &self.target,
        }
    }
}

/// Where pull request `number`'s head is checked out, holding its head commit, for a start in
/// a space to open its review on (ADR 0163): the top of `git` itself, another worktree git lists
/// on the head branch, or none, as for a fork's. Asked of `gh` as [`resolve`] asks it, from an
/// empty private directory under `state`, on the caller's thread. Nothing is fetched: a clone
/// that lacks the head commit has no worktree holding it.
pub(crate) fn head_worktree(
    git: &Git,
    state: &Path,
    number: u64,
    stop: &dyn Fn() -> bool,
) -> Result<Option<PathBuf>, ScopeError> {
    let asker = Asker::of(git, state)?;
    let mut tell = |_: Meanwhile<'_>| true;
    let mut waits = Waits {
        stop,
        tell: &mut tell,
    };
    head_worktree_with(git, &asker.from(), number, &mut waits)
}

/// [`head_worktree`], asking as `from` says.
pub(super) fn head_worktree_with(
    git: &Git,
    from: &PullFrom<'_>,
    number: u64,
    waits: &mut Waits<'_>,
) -> Result<Option<PathBuf>, ScopeError> {
    let sides = ask_gh(from, number, &mut None, waits)?;
    if sides.fork || !has(git, &sides.head)? {
        return Ok(None);
    }
    waits.go_on()?;
    if checked_out_here(git, from, &sides)? {
        return Ok(Some(git.top().to_path_buf()));
    }
    worktree_of(git, &sides, waits)
}

/// Whether the pull request's head branch, not a fork's, is the one checked out at the top of
/// `git`, and `HEAD` holds its head commit, which this clone has: then the new side is the work
/// tree, and comments and questions go there.
fn checked_out_here(git: &Git, from: &PullFrom<'_>, sides: &PrSides) -> Result<bool, ScopeError> {
    if sides.fork || from.target.branch.as_deref() != Some(sides.head_branch.as_str()) {
        return Ok(false);
    }
    match git.resolve("HEAD", "commit")? {
        Some(head_here) => holds(git, &head_here, &sides.head),
        None => Ok(false),
    }
}

/// Where a pull request is asked about and fetched from.
pub(super) struct PullFrom<'a> {
    pub(super) gh: &'a Gh,
    /// The empty private directory `gh` runs in.
    pub(super) cwd: &'a Path,
    /// The repository asked about, the remote its commits are fetched from, and the branch
    /// checked out.
    pub(super) target: &'a PullTarget,
}

/// [`resolve`], asking as `from` says.
pub(super) fn resolve_with(
    git: &Git,
    from: &PullFrom<'_>,
    number: u64,
    asking: &mut Option<Asking>,
    waits: &mut Waits<'_>,
) -> Result<(Sides, PrFound), ScopeError> {
    waits.go_on()?;
    let sides = ask_gh(from, number, asking, waits)?;
    waits.go_on()?;
    let remote = from.target.remote.as_str();
    let wanted = missing(git, &sides)?;
    if !wanted.is_empty() {
        if !(waits.tell)(Meanwhile::Fetching { number, remote }) {
            return Err(ScopeError::Stopped);
        }
        fetch(git, remote, &sides, &wanted, waits)?;
        waits.go_on()?;
        if !missing(git, &sides)?.is_empty() {
            return Err(ScopeError::Refused(format!(
                "#{number}'s commits are not on {}: g reads it again",
                printable(remote, 80)
            )));
        }
    }
    waits.go_on()?;
    let here = checked_out_here(git, from, &sides)?;
    let new = if here {
        New::WorkTree { untracked: true }
    } else {
        New::Tree(sides.head.clone())
    };
    let old = merge_base(git, &sides.base, &sides.head)?;
    waits.go_on()?;
    let worktree = if sides.fork {
        None
    } else if here {
        Some(git.top().to_path_buf())
    } else {
        // The root's own branch was looked at for `here` already.
        worktree_of(git, &sides, waits)?
    };
    let found = PrFound {
        number,
        title: sides.title,
        head_branch: sides.head_branch,
        worktree,
        head_checked_out: here,
        head: sides.head.clone(),
    };
    Ok((
        Sides {
            old: Some(old),
            new,
        },
        found,
    ))
}

/// What `gh pr view` says of pull request `number`, waited for on a thread of its own so that a
/// stop ends the wait at once. The run a stop leaves waiting stays in `asking`, and a later ask
/// of the same pull request takes up its answer rather than starting another.
fn ask_gh(
    from: &PullFrom<'_>,
    number: u64,
    asking: &mut Option<Asking>,
    waits: &mut Waits<'_>,
) -> Result<PrSides, ScopeError> {
    let waiting = asking.take().filter(|asking| asking.number == number);
    let answer = match waiting {
        Some(waiting) => waiting.answer,
        None => start_gh(from, number)?,
    };
    let answer = loop {
        match answer.recv_timeout(POLL) {
            Ok(answer) => break answer,
            Err(RecvTimeoutError::Timeout) => {
                if let Err(stopped) = waits.go_on() {
                    *asking = Some(Asking { number, answer });
                    return Err(stopped);
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ScopeError::Refused("gh's run failed: g asks again".into()));
            }
        }
    };
    let answer = answer.map_err(ScopeError::Refused)?;
    read_sides(&answer)
        .filter(|sides| sides.number == number)
        .ok_or_else(|| ScopeError::Refused("gh gave an unexpected answer".into()))
}

/// Starts `gh pr view` of `number` on `oso-review-gh`; its answer comes on what this gives.
fn start_gh(
    from: &PullFrom<'_>,
    number: u64,
) -> Result<mpsc::Receiver<Result<Vec<u8>, String>>, ScopeError> {
    let (gh, cwd, repo) = (
        from.gh.clone(),
        from.cwd.to_owned(),
        from.target.github.clone(),
    );
    let (sent, answer) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("oso-review-gh".into())
        .spawn(move || {
            let n = number.to_string();
            let words = [
                "pr",
                "view",
                &n,
                "--repo",
                repo.as_str(),
                "--json",
                SIDES_FIELDS,
            ];
            let answer = (gh.run(&repo, &words, &cwd, MAX_GH, GH_LIMIT))
                .and_then(|answer| answer.into_stdout());
            let _ = sent.send(answer);
        })
        .map_err(|e| ScopeError::Refused(format!("cannot run gh: {e}")))?;
    Ok(answer)
}

/// Whether this clone holds commit `id`.
fn has(git: &Git, id: &Id) -> Result<bool, ScopeError> {
    Ok(git.resolve(id.as_str(), "commit")?.is_some())
}

/// Whether commit `tip` holds commit `id`: `id` is `tip` or one of its ancestors.
fn holds(git: &Git, tip: &Id, id: &Id) -> Result<bool, ScopeError> {
    Ok(git.is_ancestor(id.as_str(), tip.as_str())?)
}

/// The refs to fetch for the sides this clone lacks: the pull request's head, its base branch.
fn missing(git: &Git, sides: &PrSides) -> Result<Vec<String>, ScopeError> {
    let mut wanted = Vec::new();
    if !has(git, &sides.head)? {
        wanted.push(format!("refs/pull/{}/head", sides.number));
    }
    if !has(git, &sides.base)? {
        if !plain_branch(&sides.base_branch) {
            return Err(ScopeError::Refused(format!(
                "#{}'s base branch is not a name oso fetches: {}",
                sides.number,
                printable(&sides.base_branch, 80)
            )));
        }
        wanted.push(format!("refs/heads/{}", sides.base_branch));
    }
    Ok(wanted)
}

/// Whether `name` is a branch's name of plain characters, which a refspec reads as that branch
/// alone: ASCII letters, digits and `-._/`, as GitHub's base branches are, so no `:`, `+`, `*`,
/// `^`, `@` or space; no `-` first; and none of the parts git refuses (an empty part, one
/// starting with `.` or ending with `.` or `.lock`, `..`).
fn plain_branch(name: &str) -> bool {
    let chars = (name.bytes()).all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b));
    chars
        && !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && !name.contains("..")
        && name.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with('.')
                && !part.ends_with(".lock")
        })
}

/// The arguments of the fetch of `wanted` from `remote`.
fn fetch_words(remote: &str, wanted: &[String]) -> Vec<String> {
    let words = [
        "fetch",
        "--no-tags",
        "--recurse-submodules=no",
        "--no-auto-gc",
        "--no-write-commit-graph",
        // Objects alone: no remote-tracking ref is updated, and `FETCH_HEAD` stays as it was,
        // naming nothing of the pull request's for a later `git merge FETCH_HEAD`.
        "--refmap=",
        "--no-write-fetch-head",
        "--",
    ];
    let mut words: Vec<String> = words.map(str::to_owned).to_vec();
    words.push(remote.to_owned());
    words.extend(wanted.iter().cloned());
    words
}

/// Fetches `wanted` from `remote`, until it ends or the review stops.
fn fetch(
    git: &Git,
    remote: &str,
    sides: &PrSides,
    wanted: &[String],
    waits: &mut Waits<'_>,
) -> Result<(), ScopeError> {
    let words = fetch_words(remote, wanted);
    let args: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let mut said: Vec<u8> = Vec::new();
    let gone = std::cell::Cell::new(false);
    let stop = waits.stop;
    let tell = &mut *waits.tell;
    let status =
        git.for_fetch()
            .run_long_with(&args, &[], None, &|| stop() || gone.get(), &mut |piece| {
                if let Some(piece) = piece {
                    said.extend_from_slice(piece);
                    if said.len() > MAX_SAID {
                        said.drain(..said.len() - MAX_SAID);
                    }
                }
                if !tell(Meanwhile::Idle) {
                    gone.set(true);
                }
            });
    let status = match status {
        Err(GitError::Stopped) => return Err(ScopeError::Stopped),
        status => status?,
    };
    if status.success() {
        return Ok(());
    }
    Err(ScopeError::Refused(format!(
        "git fetch of #{} from {} failed: {}",
        sides.number,
        printable(remote, 80),
        fetch_failure(&said)
    )))
}

/// git's own words in a failed fetch's last output, escaped: its last line of its own, never a
/// server's own error or `remote:` line.
fn fetch_failure(said: &[u8]) -> String {
    let said = String::from_utf8_lossy(said);
    match own_line(&said, MAX_SAID) {
        Some(line) => printable(line, 300),
        None => "git said why only in the server's words, which oso does not show".into(),
    }
}

/// The worktree `sides`' head branch is checked out in, if git lists one that is still there
/// and holds the pull request's head commit, at most [`MAX_CANDIDATES`] of them looked at, and
/// none once the review stops. The root is not one: whether it holds the head was asked.
fn worktree_of(
    git: &Git,
    sides: &PrSides,
    waits: &mut Waits<'_>,
) -> Result<Option<PathBuf>, ScopeError> {
    let listed = match list_worktrees(git) {
        Ok(listed) => listed,
        Err(e) => {
            log::debug!("a review cannot list the worktrees: {e}");
            return Ok(None);
        }
    };
    let top = git.top();
    let candidates = (listed.into_iter())
        .filter(|listed| !listed.bare && !listed.prunable && listed.path != top)
        .filter(|listed| listed.branch.as_deref() == Some(sides.head_branch.as_str()))
        .take(MAX_CANDIDATES);
    for listed in candidates {
        waits.go_on()?;
        let tip = (listed.head.as_deref()).and_then(|head| Id::parse(head.as_bytes()));
        if tip.is_some_and(|tip| holds(git, &tip, &sides.head).unwrap_or(false)) {
            return Ok(Some(listed.path));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
