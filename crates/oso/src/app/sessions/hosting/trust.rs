//! Whether a session's project is trusted now, as an open buffer of it has it (ADR 0114): the
//! session tools ask it of their caller's project (ADR 0121), and a native session's resume
//! and fork of theirs, which only a trusted project runs (ADR 0165, Q5).
//!
//! Must not depend on: the disk; the editor's projects and trust list say it.
//!
//! - An open project that is not trusted is not trusted, whatever the session began with.
//! - A native resume or fork fails closed: with no open project at its root, it does not run
//!   natively.

use std::path::Path;

use crate::app::App;
use crate::git::TrustedRoot;

/// Why a native session does not run natively where it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app::sessions::hosting) enum NotTrusted {
    /// No buffer of a project at its root is open, so its trust is not known now.
    NotOpen,
    /// The project at its root is not trusted now.
    Untrusted,
    /// It runs elsewhere than at its project's root.
    NotAtRoot,
}

impl App {
    /// The trust of the project at `root` as an open buffer of it has it now: `None` when no
    /// buffer has it open, `Some(None)` when it is not trusted.
    pub(in crate::app) fn open_root_trust(&self, root: &Path) -> Option<Option<TrustedRoot>> {
        let id = self
            .editor
            .buffers()
            .filter_map(|(_, buffer)| buffer.project())
            .find(|&id| self.editor.project(id).is_some_and(|p| p.root() == root))?;
        Some(TrustedRoot::of(&self.editor, id))
    }

    /// The trust a native session in `directory`, of the project at `root`, runs under now:
    /// only at its project's root, and only a trusted open project's.
    pub(in crate::app::sessions::hosting) fn native_trust_now(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<TrustedRoot, NotTrusted> {
        let root = root.unwrap_or(directory);
        judged(root == directory, || self.open_root_trust(root))
    }
}

/// A native session's trust: only at its project's root (`at_root`), and only as an open
/// buffer of the project says it now (`open`): `None` when none is open, `Some(None)` when it
/// is not trusted.
fn judged(
    at_root: bool,
    open: impl FnOnce() -> Option<Option<TrustedRoot>>,
) -> Result<TrustedRoot, NotTrusted> {
    if !at_root {
        return Err(NotTrusted::NotAtRoot);
    }
    match open() {
        None => Err(NotTrusted::NotOpen),
        Some(None) => Err(NotTrusted::Untrusted),
        Some(Some(trusted)) if trusted.revoked() => Err(NotTrusted::Untrusted),
        Some(Some(trusted)) => Ok(trusted),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{NotTrusted, TrustedRoot, judged};

    #[test]
    fn a_native_session_runs_only_at_a_root_an_open_buffer_trusts_now() {
        let root = || TrustedRoot::for_tests(PathBuf::from("/nowhere/oso-native-trust"));
        assert!(judged(true, || Some(Some(root()))).is_ok());
        assert_eq!(judged(true, || None).unwrap_err(), NotTrusted::NotOpen);
        assert_eq!(
            judged(true, || Some(None)).unwrap_err(),
            NotTrusted::Untrusted
        );
        let revoked = root();
        revoked.revoke();
        assert_eq!(
            judged(true, || Some(Some(revoked))).unwrap_err(),
            NotTrusted::Untrusted
        );
        assert_eq!(
            judged(false, || Some(Some(root()))).unwrap_err(),
            NotTrusted::NotAtRoot,
            "below its root, whatever the root's trust"
        );
    }
}
