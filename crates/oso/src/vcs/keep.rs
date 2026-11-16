//! Discarding and writing back on git's threads (ADR 0127, step 1c): the copies kept first,
//! each discard's steps, and a copy written back to its file.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; a discard runs on the status's
//! thread, `oso-vcs`, and writing back on the repository's run thread, `oso-vcs-run`, and both
//! wait for git.
//!
//! - A path from git is taken only as names under the root: no `..`, no root of its own, and no
//!   directory on the way that is a link, so that nothing is deleted or written elsewhere.
//! - A copy is `git hash-object -w --no-filters` of the work tree's file, as its bytes are,
//!   with no time limit until `C-g`, and the record ([`discarded`]) gets it before anything is
//!   discarded. A link, a directory or a file gone gets no copy.
//! - Untracked files past [`MAX_DELETED_BYTES`] in all are refused before anything runs.
//! - Writing back reads the copy (`git cat-file blob`, at most [`MAX_RESTORED`] bytes), keeps
//!   what the file holds now as another copy, and writes the file beside itself, then renames
//!   it over, with its mode as it was.

use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::AtomicBool;

use super::discard::{Plan, Step};
use super::discarded::{self, Entry};
use super::read::{args, set};
use crate::git::{Git, GitError, TrustedRoot, os_path};

/// The most bytes of untracked files one discard deletes.
pub(crate) const MAX_DELETED_BYTES: u64 = 64 * 1024 * 1024;
/// The most bytes a copy written back may have.
pub(crate) const MAX_RESTORED: usize = 64 * 1024 * 1024;

/// Runs `plan` in the repository at `root`, keeping its copies first in the record under
/// `state`; `stop` stops what git runs.
pub(crate) fn discard(
    git: &Git,
    root: &TrustedRoot,
    plan: &Plan,
    state: Option<&Path>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let top = root.path();
    let mut bytes = 0u64;
    for step in &plan.steps {
        if let Step::Delete(paths) = step {
            for path in paths {
                bytes += fs::symlink_metadata(inside(top, path)?).map_or(0, |meta| meta.len());
            }
        }
    }
    if bytes > MAX_DELETED_BYTES {
        return Err("That deletes more than 64 MiB: use a shell".to_owned());
    }
    let mut kept = Vec::new();
    for (path, secret) in &plan.copies {
        if *secret {
            continue;
        }
        if let Some(id) = copy(git, top, path, stop)? {
            kept.push(entry_of(top, id, path));
        }
    }
    if !kept.is_empty() {
        let state = state.ok_or("oso has no state directory, so it keeps no copy: nothing ran")?;
        discarded::add(state, kept).map_err(|e| format!("cannot keep the copies' record: {e}"))?;
    }
    for step in &plan.steps {
        root.check()?;
        run(git, top, step, stop)?;
    }
    Ok(())
}

/// Writes the copy `entry` back to its file in the repository at `root`, keeping what the file
/// holds now as another copy first; says what it did.
pub(crate) fn restore(
    git: &Git,
    root: &TrustedRoot,
    entry: &Entry,
    state: Option<&Path>,
    stop: &AtomicBool,
) -> Result<String, String> {
    let top = root.path();
    if entry.root != top.as_os_str().as_encoded_bytes() || !discarded::is_id(&entry.id) {
        return Err("That copy is not this repository's".to_owned());
    }
    let full = inside(top, &entry.path)?;
    let now = fs::symlink_metadata(&full).ok();
    if now.as_ref().is_some_and(|meta| !meta.is_file()) {
        return Err("What is at that path now is not a file: use a shell".to_owned());
    }
    let words = ["cat-file", "blob", entry.id.as_str()];
    let text = git
        .run(&args(&words), &set(), MAX_RESTORED)
        .map_err(|e| e.to_string())?;
    root.check()?;
    if now.is_some()
        && let Some(id) = copy(git, top, &entry.path, stop)?
    {
        let state = state.ok_or("oso has no state directory, so it keeps no copy: nothing ran")?;
        discarded::add(state, vec![entry_of(top, id, &entry.path)])
            .map_err(|e| format!("cannot keep the copies' record: {e}"))?;
    }
    let parent = full.parent().unwrap_or(top);
    fs::create_dir_all(parent).map_err(|e| format!("cannot make its directory: {e}"))?;
    let next = parent.join(format!(".oso-restore-{}", std::process::id()));
    fs::write(&next, &text).map_err(|e| format!("cannot write it: {e}"))?;
    if let Some(meta) = &now {
        let _ = fs::set_permissions(&next, meta.permissions());
    }
    fs::rename(&next, &full).map_err(|e| {
        let _ = fs::remove_file(&next);
        format!("cannot put it in place: {e}")
    })?;
    let name = oso_core::editor::row_text(&String::from_utf8_lossy(&entry.path));
    Ok(format!("Wrote {name} back"))
}

fn entry_of(top: &Path, id: String, path: &[u8]) -> Entry {
    Entry {
        time: discarded::now(),
        root: top.as_os_str().as_encoded_bytes().to_vec(),
        id,
        path: path.to_vec(),
    }
}

/// The id of a copy of `path`'s file as it is, or `None` if it is no file now.
fn copy(git: &Git, top: &Path, path: &[u8], stop: &AtomicBool) -> Result<Option<String>, String> {
    let full = inside(top, path)?;
    if !fs::symlink_metadata(&full).is_ok_and(|meta| meta.is_file()) {
        return Ok(None);
    }
    let name = os_path(path).map_err(|e| e.to_string())?;
    let words: Vec<&OsStr> = ["hash-object", "-w", "--no-filters", "--"]
        .iter()
        .map(OsStr::new)
        .chain([name])
        .collect();
    let out = git
        .run_long(&words, &set(), None, stop)
        .map_err(|e| e.to_string())?;
    let id = out
        .stderr
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if !out.status.success() || !discarded::is_id(&id) {
        return Err(format!("cannot keep a copy, so nothing ran: {}", id.trim()));
    }
    Ok(Some(id))
}

/// Runs one step of a discard.
fn run(git: &Git, top: &Path, step: &Step, stop: &AtomicBool) -> Result<(), String> {
    let (words, input): (Vec<&str>, Vec<u8>) = match step {
        Step::Reverse { patch, index } => {
            let mut words = vec!["-c", "apply.ignoreWhitespace=no", "apply"];
            if *index {
                words.push("--index");
            }
            words.extend(["-R", "--whitespace=nowarn", "-"]);
            (words, patch.clone())
        }
        Step::Restore(paths) => (
            vec![
                "restore",
                "--worktree",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
        ),
        Step::RestoreHead(paths) => (
            vec![
                "restore",
                "--staged",
                "--worktree",
                "--source=HEAD",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
        ),
        Step::Unadd(paths) => (
            vec![
                "rm",
                "--cached",
                "--quiet",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
        ),
        Step::Delete(paths) => return delete(top, paths),
    };
    let out = git
        .run_long(&args(&words), &set(), Some(input), stop)
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    if matches!(step, Step::Reverse { .. }) {
        let first = out
            .stderr
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        return Err(format!(
            "git refused, as the file changed since it was read: {first}"
        ));
    }
    Err(GitError::Failed {
        status: out.status.code(),
        stderr: out.stderr,
    }
    .to_string())
}

/// Deletes `paths` of the work tree under `top`, and the directories they leave empty.
fn delete(top: &Path, paths: &[Vec<u8>]) -> Result<(), String> {
    for path in paths {
        let full = inside(top, path)?;
        match fs::remove_file(&full) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("cannot delete a file: {e}")),
        }
        let mut dir = full.parent();
        while let Some(at) = dir.filter(|at| *at != top && at.starts_with(top)) {
            if fs::remove_dir(at).is_err() {
                break;
            }
            dir = at.parent();
        }
    }
    Ok(())
}

/// `path`, as git names it, under `top`: refused unless it is plain names and no directory on
/// the way is a link.
pub(crate) fn inside(top: &Path, path: &[u8]) -> Result<PathBuf, String> {
    let refused = || "git named a path oso does not follow".to_owned();
    let relative = Path::new(os_path(path).map_err(|_| refused())?);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        return Err(refused());
    }
    let mut at = top.to_path_buf();
    let names: Vec<_> = relative.components().collect();
    for name in &names[..names.len() - 1] {
        at.push(name);
        match fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(refused()),
        }
    }
    Ok(top.join(relative))
}

fn nul_joined(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path);
        out.push(0);
    }
    out
}
