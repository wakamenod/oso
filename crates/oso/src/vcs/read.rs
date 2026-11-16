//! A status's reads (ADR 0127): finding the repository git works in, the status's listing,
//! the counts, the head's subject, the push target and the commit sections (step 2), and an
//! open file's patch, each bounded, with the arguments and the variable every run of the
//! status's thread shares. The commit sections read the local refs that track the remote
//! only: nothing here fetches.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on the status's
//! thread, `oso-vcs`, and on the repository's run thread, and wait for git.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::history::{self, Log};
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

/// The push target, where it is not the upstream, and the commit sections of `branch`: the
/// unpushed and unpulled ones when it has an upstream, the recent ones when it has a commit. A
/// side the ahead and behind counts say is empty is not read, as most reads find both so. A
/// read git refuses leaves its part out; one cut at [`history::MAX_LOG`] keeps what came.
pub(super) fn logs(git: &Git, branch: &status::Branch) -> (Option<Vec<u8>>, Vec<(Section, Log)>) {
    let mut logs = Vec::new();
    if branch.oid.is_none() {
        return (None, logs);
    }
    let read = |range: &str, keep: usize| {
        let words = history::args(range, keep);
        let words: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
        let out = git.output(&words, &set(), history::MAX_LOG).ok()?;
        if out.cut {
            // The run was killed at the limit: the whole records read show, and more is said.
            let whole = out
                .stdout
                .iter()
                .rposition(|&byte| byte == 0)
                .map_or(0, |at| at + 1);
            let mut log = history::parse(&out.stdout[..whole], keep);
            log.more = true;
            return Some(log);
        }
        out.status
            .success()
            .then(|| history::parse(&out.stdout, keep))
    };
    let push = branch.head.as_ref().and_then(|_| {
        let words = args(&["rev-parse", "--abbrev-ref", "@{push}"]);
        let out = git.run(&words, &set(), MAX_SHORT).ok()?;
        let push = crate::git::trim_line(&out).to_vec();
        (!push.is_empty() && branch.upstream.as_deref() != Some(push.as_slice())).then_some(push)
    });
    if branch.upstream.is_some() {
        let (ahead, behind) = branch
            .ahead_behind
            .map_or((true, true), |(a, b)| (a > 0, b > 0));
        let pairs = [
            (Section::Unpushed, "@{upstream}..HEAD", ahead),
            (Section::Unpulled, "HEAD..@{upstream}", behind),
        ];
        for (section, range, _) in pairs.into_iter().filter(|(_, _, some)| *some) {
            if let Some(log) = read(range, history::MAX_COMMITS) {
                logs.push((section, log));
            }
        }
    }
    if let Some(log) = read("HEAD", history::RECENT) {
        logs.push((Section::Recent, log));
    }
    (push, logs)
}

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
    let mut patch = patch::parse(&out.stdout, out.cut);
    patch.refine();
    Ok(patch)
}
