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

use super::discard::{MAX_COPIED, Plan, Step};
use super::discarded::{self, Entry};
use super::read::{args, set};
use crate::git::{Git, GitError, Next, TrustedRoot, os_path};

/// The most bytes of untracked files one discard deletes.
pub(crate) const MAX_DELETED_BYTES: u64 = 64 * 1024 * 1024;
/// The most bytes a copy written back may have.
pub(crate) const MAX_RESTORED: usize = 64 * 1024 * 1024;
/// The most bytes one discard keeps copies of.
pub(crate) const MAX_COPIED_BYTES: u64 = 256 * 1024 * 1024;

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
    let paths: Vec<&[u8]> = (plan.copies.iter())
        .filter(|(_, secret)| !secret)
        .map(|(path, _)| path.as_slice())
        .collect();
    let kept: Vec<Entry> = (copies(git, top, &paths, stop)?.into_iter())
        .map(|(path, id)| entry_of(top, id, &path))
        .collect();
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
    (entry, secret): (&Entry, bool),
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
    // What it holds now is kept first, and what may hold secrets gets no copy.
    if now.is_some() && (secret || crate::git::may_hold_secrets(top, &entry.path)) {
        return Err("It may hold secrets now, which get no copy: use a shell".to_owned());
    }
    let words = ["cat-file", "blob", entry.id.as_str()];
    let text = git
        .run(&args(&words), &set(), MAX_RESTORED)
        .map_err(|e| e.to_string())?;
    root.check()?;
    if now.is_some() {
        let kept = copies(git, top, &[entry.path.as_slice()], stop)?;
        let state = state.ok_or("oso has no state directory, so it keeps no copy: nothing ran")?;
        let kept = (kept.into_iter())
            .map(|(path, id)| entry_of(top, id, &path))
            .collect();
        discarded::add(state, kept).map_err(|e| format!("cannot keep the copies' record: {e}"))?;
    }
    // Looked at again just before the write: a directory on the way may have become a link.
    let full = inside(top, &entry.path)?;
    let parent = full.parent().unwrap_or(top);
    fs::create_dir_all(parent).map_err(|e| format!("cannot make its directory: {e}"))?;
    inside(top, &entry.path)?;
    let next = write_beside(parent, &text, now.as_ref().map(fs::Metadata::permissions))?;
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

/// `text` in a new file in `dir`, made there and never through a link (`create_new`), with
/// `mode` as its own before anything is in it; its path.
fn write_beside(dir: &Path, text: &[u8], mode: Option<fs::Permissions>) -> Result<PathBuf, String> {
    for tries in 0..8u32 {
        let nanos = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH))
            .map_or(0, |since| since.subsec_nanos());
        let next = dir.join(format!(
            ".oso-restore-{}-{nanos}-{tries}",
            std::process::id()
        ));
        let opened = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&next);
        let mut file = match opened {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot write it: {e}")),
        };
        let written = fill(&mut file, text, mode.clone());
        if let Err(e) = written {
            let _ = fs::remove_file(&next);
            return Err(format!("cannot write it: {e}"));
        }
        return Ok(next);
    }
    Err("cannot write it: no free name beside it".to_owned())
}

/// Gives `file` its `mode`, if one, then `text`, and has the disk keep it.
fn fill(file: &mut fs::File, text: &[u8], mode: Option<fs::Permissions>) -> std::io::Result<()> {
    if let Some(mode) = mode {
        file.set_permissions(mode)?;
    }
    std::io::Write::write_all(file, text)?;
    file.sync_all()
}

/// The ids of copies of the files `paths` name, as they are, in one run of git, each with its
/// path; a path that is no file now gets none. Refused, before anything is kept: past
/// [`MAX_COPIED`] files or [`MAX_COPIED_BYTES`], a file with more than one name, which could be
/// another's under a harmless one, one that only its owner may read, whose copy would be read
/// more widely, and a name with a line break, which git cannot take as one.
fn copies(
    git: &Git,
    top: &Path,
    paths: &[&[u8]],
    stop: &AtomicBool,
) -> Result<Vec<(Vec<u8>, String)>, String> {
    let mut files = Vec::new();
    let mut bytes = 0u64;
    for path in paths {
        let Ok(meta) = fs::symlink_metadata(inside(top, path)?) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        let name = oso_core::editor::row_text(&String::from_utf8_lossy(path));
        refuse_copy(&meta, &name)?;
        if path.contains(&b'\n') || path.contains(&b'\r') {
            return Err(format!("{name} has a line break in its name: use a shell"));
        }
        bytes += meta.len();
        files.push(path.to_vec());
    }
    if files.len() > MAX_COPIED || bytes > MAX_COPIED_BYTES {
        return Err("That keeps more than 1000 files or 256 MiB: use a shell".to_owned());
    }
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut input = Vec::new();
    for path in &files {
        input.extend_from_slice(path);
        input.push(b'\n');
    }
    let words = ["hash-object", "-w", "--no-filters", "--stdin-paths"];
    let (ok, out) = run_collect(git, &args(&words), input, stop)?;
    let ids: Vec<String> = (String::from_utf8_lossy(&out).lines())
        .map(|line| line.trim().to_owned())
        .collect();
    if !ok || ids.len() != files.len() || !ids.iter().all(|id| discarded::is_id(id)) {
        return Err("cannot keep the copies, so nothing ran".to_owned());
    }
    Ok(files.into_iter().zip(ids).collect())
}

/// Why `meta`'s file, called `name`, gets no copy, if it does not.
#[cfg(unix)]
fn refuse_copy(meta: &fs::Metadata, name: &str) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() > 1 {
        return Err(format!("{name} has more than one name: use a shell"));
    }
    if meta.mode() & 0o044 == 0 {
        return Err(format!(
            "{name} is for its owner's eyes only, and its copy would not be: use a shell"
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn refuse_copy(_: &fs::Metadata, _: &str) -> Result<(), String> {
    Ok(())
}

/// Runs git with `input` on its standard input until it ends or `stop` is set, keeping at most
/// 1 MiB of what it wrote; whether it succeeded, and that.
fn run_collect(
    git: &Git,
    words: &[&OsStr],
    input: Vec<u8>,
    stop: &AtomicBool,
) -> Result<(bool, Vec<u8>), String> {
    const KEEP: usize = 1024 * 1024;
    let mut long = (git.start_long(words, &set(), Some(input))).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    loop {
        if stop.load(std::sync::atomic::Ordering::SeqCst) {
            long.stop();
        }
        match long
            .next(std::time::Duration::from_millis(20))
            .map_err(|e| e.to_string())?
        {
            Next::Out(piece) => {
                let room = KEEP.saturating_sub(out.len()).min(piece.len());
                out.extend_from_slice(&piece[..room]);
            }
            Next::Quiet => {}
            Next::Ended(_) if long.stopped() => return Err(GitError::Stopped.to_string()),
            Next::Ended(status) => return Ok((status.success(), out)),
        }
    }
}

/// Runs one step of a discard.
fn run(git: &Git, top: &Path, step: &Step, stop: &AtomicBool) -> Result<(), String> {
    let (words, input): (Vec<&str>, Vec<u8>) = match step {
        Step::Reverse { patch, cached } => {
            let mut words = vec!["-c", "apply.ignoreWhitespace=no", "apply"];
            if *cached {
                words.push("--cached");
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
    // Nothing of git's own directory, however its name is spelled to a file system that folds.
    let gits = |name: &OsStr| {
        let name = name.to_string_lossy().to_ascii_lowercase();
        let name = name.trim_end_matches(['.', ' ']);
        name == ".git" || name == "git~1"
    };
    let plain = |part: Component<'_>| matches!(part, Component::Normal(name) if !gits(name));
    if relative.as_os_str().is_empty() || !relative.components().all(plain) {
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
