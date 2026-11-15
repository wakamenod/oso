//! A status's reads (ADR 0127): finding the repository git works in, the status's listing,
//! the counts, the head's subject and an open file's patch, each bounded, with the arguments
//! and the variable every run of the status's thread shares.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on the status's
//! thread, `oso-vcs`, and on the repository's run thread, and wait for git.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::layout::Section;
use super::patch;
use super::status;
use crate::git::{Git, GitError, MAX_SHORT, os_path};

/// The most bytes of the status's listing read; past it, the files after are not shown.
pub(crate) const MAX_LIST: usize = 32 * 1024 * 1024;
/// The most bytes of one file's patch read.
pub(crate) const MAX_PATCH: usize = 1024 * 1024;

/// The variable every run here sets.
pub(super) const LITERAL: [(&str, &str); 1] = [("GIT_LITERAL_PATHSPECS", "1")];

/// `vcs/hooks` under `state`, made private if missing, and checked to be empty: git is told the
/// hooks are there, so that none runs, `post-index-change` neither (ADR 0127, step 1a).
pub(super) fn hooks_dir(state: Option<&Path>) -> Result<PathBuf, String> {
    let state = state.ok_or("oso has no state directory, so it runs no git for a status")?;
    crate::git::empty_hooks(state, "vcs")
}

/// git for the repository whose work tree's top is `root`; a top elsewhere is refused.
pub(super) fn find(root: &Path, hooks: &Path) -> Result<Git, String> {
    let git = Git::new(root, hooks).map_err(|e| e.to_string())?;
    let out = git
        .run(&args(&["rev-parse", "--show-toplevel"]), &set(), MAX_SHORT)
        .map_err(|e| e.to_string())?;
    let top = crate::git::trim_line(&out);
    let top = os_path(top).map_err(|e| e.to_string())?;
    let same = match (Path::new(top).canonicalize(), root.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same {
        return Err("git found a repository whose top is not the project's root".into());
    }
    Ok(git)
}

pub(super) fn args<'a>(words: &[&'a str]) -> Vec<&'a OsStr> {
    words.iter().map(|word| OsStr::new(*word)).collect()
}

pub(super) fn set() -> Vec<(&'static str, &'static OsStr)> {
    LITERAL
        .iter()
        .map(|(name, value)| (*name, OsStr::new(value)))
        .collect()
}

pub(super) fn numstat(git: &Git, args: &[&OsStr]) -> Option<HashMap<Vec<u8>, status::Counts>> {
    let out = git.output(args, &set(), MAX_LIST).ok()?;
    (out.status.success() && !out.cut).then(|| status::parse_numstat(&out.stdout))
}

/// `HEAD`'s short id and subject.
pub(super) fn head_commit(git: &Git) -> Option<(String, Vec<u8>)> {
    let words = [
        "log",
        "-1",
        "--no-show-signature",
        "--no-color",
        "--format=%h%x00%s",
        "HEAD",
        "--",
    ];
    let out = git.run(&args(&words), &set(), MAX_SHORT).ok()?;
    let line = crate::git::trim_line(&out);
    let at = line.iter().position(|&byte| byte == 0)?;
    let id = std::str::from_utf8(&line[..at]).ok()?;
    id.bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then(|| (id.to_owned(), line[at + 1..].to_vec()))
}

/// The patch of `path` in `section`, as git writes it for staging, from `from` if renamed.
pub(super) fn read_patch(
    git: &Git,
    section: Section,
    path: &[u8],
    from: Option<&[u8]>,
) -> Result<patch::Patch, String> {
    let mut words = args(&[
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "-U3",
    ]);
    if section == Section::Staged {
        words.push(OsStr::new("--cached"));
    }
    if from.is_some() {
        words.push(OsStr::new("-M"));
    }
    words.push(OsStr::new("--"));
    if let Some(from) = from {
        words.push(os_path(from).map_err(|e| e.to_string())?);
    }
    words.push(os_path(path).map_err(|e| e.to_string())?);
    let out = git
        .output(&words, &set(), MAX_PATCH)
        .map_err(|e| e.to_string())?;
    if !out.cut && !out.status.success() {
        return Err(GitError::Failed {
            status: out.status.code(),
            stderr: out.stderr,
        }
        .to_string());
    }
    Ok(patch::parse(&out.stdout, out.cut))
}
