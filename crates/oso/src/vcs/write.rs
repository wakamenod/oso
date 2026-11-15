//! A status's writes to the index (ADR 0127): each staging op run with no time limit until the
//! person stops it, its paths NUL-separated on standard input, and a hunk as git's own patch.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; these run on the status's
//! thread, `oso-vcs`, and wait for git.

use std::sync::atomic::AtomicBool;

use super::read::{args, set};
use super::stage::Op;
use crate::git::{Git, GitError};

/// Runs one op, with no time limit until `stop` is set; `initial` before the repository's first
/// commit, when unstaging is `rm --cached`.
pub(super) fn run(git: &Git, op: &Op, initial: bool, stop: &AtomicBool) -> Result<(), String> {
    let (words, input, refused): (Vec<&str>, Vec<u8>, bool) = match op {
        Op::Add(paths) => (
            vec!["add", "--pathspec-from-file=-", "--pathspec-file-nul"],
            nul_joined(paths),
            false,
        ),
        Op::Update(paths) => (
            vec![
                "add",
                "--update",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
            false,
        ),
        Op::Unstage(paths) if initial => (
            vec![
                "rm",
                "--cached",
                "--quiet",
                "--ignore-unmatch",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
            false,
        ),
        Op::Unstage(paths) => (
            vec![
                "restore",
                "--staged",
                "--pathspec-from-file=-",
                "--pathspec-file-nul",
            ],
            nul_joined(paths),
            false,
        ),
        Op::Apply { patch, reverse } => {
            let mut words = vec![
                "-c",
                "apply.ignoreWhitespace=no",
                "apply",
                "--cached",
                "--whitespace=nowarn",
            ];
            if *reverse {
                words.push("-R");
            }
            words.push("-");
            (words, patch.clone(), true)
        }
    };
    let out = git
        .run_long(&args(&words), &set(), Some(input), stop)
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let first = out
        .stderr
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if refused {
        return Err(format!(
            "git refused the hunk, as the file changed since it was read: {first}"
        ));
    }
    Err(GitError::Failed {
        status: out.status.code(),
        stderr: out.stderr,
    }
    .to_string())
}

fn nul_joined(paths: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(path);
        out.push(0);
    }
    out
}
