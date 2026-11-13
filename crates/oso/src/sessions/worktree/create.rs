//! Making a worktree for a new session (ADR 0121, `session-worktree`), as a job in steps on a
//! worktree job's thread: the branch and base are checked, the brief is put together with the
//! starting session's hand-off facts, the worktree is added under `.claude/worktrees`, and
//! `.worktreeinclude`'s files are copied in. The app then starts the session.
//!
//! Must not depend on: `app`, `dispatch`, `commands` or the UI thread.
//!
//! - The brief is put together before anything is made, so one that is refused leaves nothing.
//! - `.claude` and `.claude/worktrees` are made or opened without following a link, and must be
//!   the person's own: a repository cannot send a worktree outside itself with a link there.
//! - A step that fails says why and leaves what earlier steps made; [`Created`] records what
//!   this job made, the branch included, for a clean-up later (ADR 0126).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use oso_platform::file_meta::Directory;

use super::brief::{self, Handoff, Header};
use super::git::{MAX_LISTING, Repo, run, said};
use super::include::{self, Copied};
use super::merged;
use super::porcelain;
use super::slug::{self, PARENT};
use super::transcript;
use crate::project::ours;
use crate::review::git::{MAX_SHORT, check_revision};

/// The most lines of the starting session's uncommitted changes the brief names.
const MAX_UNCOMMITTED: usize = 200;

/// The session a worktree session is handed from.
#[derive(Clone, Debug)]
pub struct From {
    /// Its name, only while it is live.
    pub name: Option<String>,
    /// Its id, for its transcript.
    pub id: String,
    /// The directory it runs in.
    pub directory: PathBuf,
}

/// What to make.
#[derive(Clone, Debug)]
pub struct Order {
    /// The branch: an existing local one, or a new one.
    pub branch: String,
    /// Where a new branch starts, as the person typed it; `HEAD` if none.
    pub base: Option<String>,
    /// The person's brief.
    pub brief: String,
    /// The session it is handed from, if any.
    pub from: Option<From>,
}

/// What a step reached, for the echo area while the job runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Checked,
    Added,
    Copied,
}

/// What the job made.
#[derive(Debug)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    /// Whether this job made the branch.
    pub made_branch: bool,
    pub copied: Copied,
    /// Why `.worktreeinclude` copied nothing, or stopped early, if it did.
    pub include_note: Option<String>,
    /// The session's first prompt: the brief with its header and facts.
    pub prompt: String,
}

/// The local branch `branch`'s state, once its name is checked.
struct Branch {
    name: String,
    /// Its tip, if it exists.
    tip: Option<String>,
}

/// Makes the worktree `order` asks for in `repo`, telling `progress` of each step.
///
/// # Errors
///
/// Why it stopped, for the echo area.
pub fn create(repo: &Repo, order: &Order, progress: &dyn Fn(Step)) -> Result<Created, String> {
    let branch = checked_branch(repo, &order.branch)?;
    refuse_checked_out(repo, &branch.name)?;
    let base = match (&branch.tip, &order.base) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "Branch {} exists; a base is only for a new branch",
                branch.name
            ));
        }
        (Some(_), None) => None,
        (None, base) => Some(base_of(repo, base.as_deref())?),
    };
    let slug = slug::slug(&branch.name).ok_or("That branch makes no directory name")?;
    worktrees_parent(repo.main())?;
    let path = slug::place(repo.main(), &slug, |p| p.symlink_metadata().is_ok())
        .ok_or("Ten worktrees of that name exist already")?;
    let shown_base = base.as_ref().map_or("its existing tip", |(name, _)| name);
    let header = Header {
        repo: repo.main(),
        path: &path,
        branch: &branch.name,
        base: shown_base,
    };
    let facts = order.from.as_ref().map(|from| Facts::gather(repo, from));
    let handoff = facts.as_ref().map(Facts::handoff);
    let prompt =
        brief::compose(&header, handoff.as_ref(), &order.brief).map_err(|e| e.to_string())?;
    progress(Step::Checked);
    add(
        repo,
        &path,
        &branch.name,
        base.as_ref().map(|(_, id)| id.as_str()),
    )?;
    progress(Step::Added);
    let (copied, include_note) = copy_included(repo, &path);
    progress(Step::Copied);
    Ok(Created {
        path,
        branch: branch.name,
        made_branch: base.is_some(),
        copied,
        include_note,
        prompt,
    })
}

/// The branch name, as git normalises it, refused if git would not take it as a branch.
fn checked_branch(repo: &Repo, name: &str) -> Result<Branch, String> {
    let refused = || format!("Not a branch name: {}", porcelain::printable(name, 80));
    check_revision(name).map_err(|_| refused())?;
    let out = run(
        repo.git(),
        &["check-ref-format", "--branch", name],
        MAX_SHORT,
    )
    .map_err(|_| refused())?;
    let name = String::from_utf8(out).map_err(|_| refused())?;
    let name = name.trim().to_owned();
    if name.is_empty() || name.starts_with('-') {
        return Err(refused());
    }
    let tip = merged::tip(repo, &name);
    Ok(Branch { name, tip })
}

/// Refuses a branch checked out in any worktree of the repository, naming where.
fn refuse_checked_out(repo: &Repo, branch: &str) -> Result<(), String> {
    let listed = repo.worktrees()?;
    match listed.iter().find(|w| w.branch.as_deref() == Some(branch)) {
        Some(w) => Err(format!(
            "Branch {branch} is checked out already, in {}",
            porcelain::printable(&w.path.display().to_string(), 200)
        )),
        None => Ok(()),
    }
}

/// A new branch's base, by the name to show and its commit: the one typed, else `HEAD`.
fn base_of(repo: &Repo, typed: Option<&str>) -> Result<(String, String), String> {
    let revision = typed.unwrap_or("HEAD");
    let id = repo
        .commit(revision)?
        .ok_or_else(|| format!("Not a commit: {}", porcelain::printable(revision, 80)))?;
    let name = match typed {
        Some(typed) => typed.to_owned(),
        None => run(
            repo.git(),
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
            MAX_SHORT,
        )
        .ok()
        .and_then(|out| String::from_utf8(out).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| id.chars().take(12).collect()),
    };
    Ok((porcelain::printable(&name, 120), id))
}

/// Makes or opens `<main>/.claude/worktrees` without following a link, each the person's.
fn worktrees_parent(main: &Path) -> Result<(), String> {
    let refused = |name: &str| {
        format!(
            "{name} in the repository is a link or not a directory of yours, so no worktree is made there"
        )
    };
    let mut dir = Directory::open(main).map_err(|e| format!("Cannot open the repository: {e}"))?;
    let mut path = main.to_owned();
    for name in PARENT {
        path.push(name);
        dir = dir
            .create_directory(OsStr::new(name))
            .map_err(|e| format!("Cannot make {}: {e}", path.display()))?
            .filter(|dir| dir.metadata().is_ok_and(|m| ours(&m)))
            .ok_or_else(|| {
                refused(
                    &path
                        .strip_prefix(main)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                )
            })?;
    }
    Ok(())
}

/// `git worktree add`, making `branch` at `base` when it is given.
fn add(repo: &Repo, path: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    let place = path.to_str().ok_or("The worktree's path is not UTF-8")?;
    let mut args = vec!["worktree", "add"];
    match base {
        Some(base) => args.extend(["-b", branch, "--", place, base]),
        None => args.extend(["--", place, branch]),
    }
    run(repo.git(), &args, MAX_LISTING)
        .map(drop)
        .map_err(|e| format!("git worktree add: {}", said(&e)))
}

/// Copies `.worktreeinclude`'s files into the worktree at `path`; the note says why nothing
/// or not all was copied.
fn copy_included(repo: &Repo, path: &Path) -> (Copied, Option<String>) {
    let patterns = match include::read(repo.main()) {
        Ok(Some(patterns)) => patterns,
        Ok(None) => return (Copied::default(), None),
        Err(e) => return (Copied::default(), Some(e.to_string())),
    };
    let args = [
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
    ];
    let ignored = match run(repo.git(), &args, MAX_LISTING) {
        Ok(out) => porcelain::paths(&out, usize::MAX),
        Err(e) => {
            return (
                Copied::default(),
                Some(format!(".worktreeinclude: {}", said(&e))),
            );
        }
    };
    match include::copy(&patterns, &ignored, repo.main(), path) {
        Ok(copied) => {
            let note = copied.stopped.map(|stop| {
                format!(
                    ".worktreeinclude: stopped after {} files ({} bytes): {stop:?} limit",
                    copied.files, copied.bytes
                )
            });
            (copied, note)
        }
        Err(e) => (Copied::default(), Some(format!(".worktreeinclude: {e}"))),
    }
}

/// The starting session's hand-off facts, as read.
struct Facts {
    name: Option<String>,
    transcript: Option<PathBuf>,
    plans: Vec<PathBuf>,
    uncommitted: Vec<String>,
    touched: Vec<PathBuf>,
}

impl Facts {
    /// Reads what can be read; a fact that cannot be is left out.
    fn gather(repo: &Repo, from: &From) -> Facts {
        let config = transcript::config_dir();
        let path = config
            .as_deref()
            .and_then(|config| transcript::find(config, &from.id));
        let touched = match (&path, &config) {
            (Some(path), Some(config)) => transcript::read_tail(path)
                .ok()
                .flatten()
                .map(|(tail, cut)| transcript::touched(&tail, cut, &config.join("plans"))),
            _ => None,
        };
        let git = if oso_core::project::same_place(&from.directory, repo.main()) {
            Some(repo.git().clone())
        } else {
            repo.in_worktree(&from.directory)
        };
        let uncommitted = git
            .and_then(|git| run(&git, &["status", "--porcelain", "-z"], MAX_LISTING).ok())
            .map(|out| porcelain::status_lines(&out, MAX_UNCOMMITTED))
            .unwrap_or_default();
        let (touched, plans) = touched.map_or_else(Default::default, |t| (t.files, t.plans));
        Facts {
            name: from.name.clone(),
            transcript: path,
            plans,
            uncommitted,
            touched,
        }
    }

    fn handoff(&self) -> Handoff<'_> {
        Handoff {
            name: self.name.as_deref(),
            transcript: self.transcript.as_deref(),
            plans: &self.plans,
            uncommitted: &self.uncommitted,
            touched: &self.touched,
        }
    }
}
