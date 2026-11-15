//! The root of a project trusted to run git (ADR 0009, ADR 0011, ADR 0114), which a review
//! (ADR 0122), the worktree jobs (ADR 0121) and a git status (ADR 0127) start from.
//!
//! Must not depend on: `app`, `dispatch` or winit; [`TrustedRoot::check`] and
//! [`TrustedRoot::replaced`] look at the disk, so they run on the threads that run git.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oso_core::editor::Editor;
use oso_core::project::ProjectId;

use super::path::identity_of;

/// The root of a project that is trusted to run git, with its device and inode as the project
/// was found. [`TrustedRoot::of`] makes one from a trusted project, and
/// [`TrustedRoot::of_member`] one from an issue space's repository the trust list holds by path
/// and identity; the latter is made fresh for each use and never revoked, so its users ask the
/// trust list again. Its clones share one revocation: once one is revoked, so are all.
#[derive(Clone, Debug)]
pub(crate) struct TrustedRoot {
    path: PathBuf,
    identity: Option<(u64, u64)>,
    revoked: Arc<AtomicBool>,
}

impl TrustedRoot {
    /// The root of project `id`, if the editor trusts it.
    pub(crate) fn of(editor: &Editor, id: ProjectId) -> Option<TrustedRoot> {
        if !editor.project_trusted(id) {
            return None;
        }
        let project = editor.project(id)?;
        Some(TrustedRoot {
            path: project.root().to_owned(),
            identity: project.identity(),
            revoked: Arc::default(),
        })
    }

    /// The root of a repository at `path` with `identity`, an issue space's member that no
    /// open buffer may show, if `list` trusts it by both (ADR 0136, step 4).
    pub(crate) fn of_member(
        list: &oso_core::project::TrustList,
        path: &Path,
        identity: Option<(u64, u64)>,
    ) -> Option<TrustedRoot> {
        let root = oso_core::project::RootIdentity {
            path: path.to_owned(),
            identity,
        };
        list.trusts(&root).then(|| TrustedRoot {
            path: root.path,
            identity,
            revoked: Arc::default(),
        })
    }

    /// The device and inode it was trusted with, where the system has them.
    pub(crate) fn identity(&self) -> Option<(u64, u64)> {
        self.identity
    }

    #[cfg(test)]
    pub(crate) fn for_tests(path: PathBuf) -> TrustedRoot {
        let identity = identity_of(&path);
        TrustedRoot {
            path,
            identity,
            revoked: Arc::default(),
        }
    }

    /// The same trust for `path`, a linked worktree a worktree job just made in this root's
    /// repository, which belongs with it as ADR 0114 has it (ADR 0121). Its revocation is this
    /// root's. It looks at the disk, so the job calls it.
    pub(crate) fn for_worktree(&self, path: PathBuf) -> TrustedRoot {
        let identity = identity_of(&path);
        TrustedRoot {
            path,
            identity,
            revoked: Arc::clone(&self.revoked),
        }
    }

    /// Takes the trust back, as when the person no longer trusts the project: no thread that
    /// holds it runs git for it any more.
    pub(crate) fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }

    /// Whether the trust was taken back.
    pub(crate) fn revoked(&self) -> bool {
        self.revoked.load(Ordering::SeqCst)
    }

    /// Whether git may still run for the root: it is not revoked, and is still the directory
    /// that was trusted. It looks at the disk, so it belongs off the UI thread.
    ///
    /// # Errors
    ///
    /// Why not, for a person.
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.revoked() {
            return Err("the project is no longer trusted".into());
        }
        if !self.still_there() {
            return Err("the project's root is no longer the directory that was trusted".into());
        }
        Ok(())
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Whether another directory is now at the root's path than the one trusted. It looks at
    /// the disk, so it belongs off the UI thread.
    pub(crate) fn replaced(&self) -> bool {
        !self.still_there()
    }

    /// Whether the root is still the directory that was trusted, by its device and inode where
    /// the system has them, not following a link.
    fn still_there(&self) -> bool {
        match self.identity {
            Some(identity) => identity_of(&self.path) == Some(identity),
            None => std::fs::symlink_metadata(&self.path).is_ok_and(|metadata| metadata.is_dir()),
        }
    }
}
