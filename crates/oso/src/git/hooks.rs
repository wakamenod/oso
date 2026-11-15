//! The empty private directory every run of git is told holds the hooks, so that none the
//! repository names runs (ADR 0121, ADR 0127). A review keeps its own beside its index copies
//! (ADR 0122).
//!
//! Must not depend on: `app`, `dispatch` or winit; it makes and reads a directory, so it runs
//! on the threads that run git.

use std::path::{Path, PathBuf};

use oso_platform::privacy::PrivateDir;

/// `<user>/hooks` under `state`, oso's per-user state directory, made private if missing and
/// checked to be empty.
///
/// # Errors
///
/// Why it cannot be used, in words for the agent or the echo area.
pub(crate) fn empty_hooks(state: &Path, user: &str) -> Result<PathBuf, String> {
    let hooks = PrivateDir::open(state.join(user).join("hooks"))
        .map_err(|e| format!("cannot make oso's directory for git: {e}"))?;
    let empty = std::fs::read_dir(hooks.path()).is_ok_and(|mut entries| entries.next().is_none());
    if !empty {
        return Err("oso's empty directory for git's hooks is not empty".to_owned());
    }
    Ok(hooks.path().to_path_buf())
}
