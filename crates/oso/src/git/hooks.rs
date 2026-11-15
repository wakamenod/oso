//! The empty private directory every run of git is told holds the hooks, so that none the
//! repository names runs (ADR 0121, ADR 0127). A review keeps its own beside its index copies
//! (ADR 0122).
//!
//! Must not depend on: `app`, `dispatch` or winit; it makes and reads a directory, so it runs
//! on the threads that run git.

use std::path::{Path, PathBuf};

use oso_platform::privacy::PrivateDir;

/// `<feature>/hooks` under `state`, oso's per-user state directory, made private if missing and
/// checked to be empty.
///
/// # Errors
///
/// Why it cannot be used, in words for the agent or the echo area.
pub(crate) fn empty_hooks(state: &Path, feature: &str) -> Result<PathBuf, String> {
    let hooks = PrivateDir::open(state.join(feature).join("hooks"))
        .map_err(|e| format!("cannot make oso's directory for git: {e}"))?;
    let empty = std::fs::read_dir(hooks.path()).is_ok_and(|mut entries| entries.next().is_none());
    if !empty {
        return Err("oso's empty directory for git's hooks is not empty".to_owned());
    }
    Ok(hooks.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::empty_hooks;

    #[test]
    fn the_hooks_directory_is_made_private_and_refused_once_it_holds_anything() {
        let name = format!("oso-git-hooks-test-{}", std::process::id());
        let state = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(name);
        let _ = std::fs::remove_dir_all(&state);
        std::fs::create_dir(&state).unwrap();
        let hooks = empty_hooks(&state, "feature").unwrap();
        assert_eq!(hooks, state.join("feature").join("hooks"));
        std::fs::write(hooks.join("pre-commit"), b"#!/bin/sh\n").unwrap();
        let why = empty_hooks(&state, "feature").unwrap_err();
        assert_eq!(why, "oso's empty directory for git's hooks is not empty");
        let _ = std::fs::remove_dir_all(&state);
    }
}
