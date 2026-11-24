//! The root of a project trusted to run git (ADR 0009, ADR 0011, ADR 0114), which a review
//! (ADR 0122), the worktree jobs (ADR 0121) and a git status (ADR 0127) start from. A
//! language server's gate checks it before each start too (ADR 0138), so a change to what
//! [`TrustedRoot::check`] or the revocation means changes when a server may start.
//!
//! Must not depend on: `app`, `dispatch` or winit; [`TrustedRoot::check`],
//! [`TrustedRoot::replaced`] and [`TrustedRoot::for_worktree`] look at the disk, so they run on
//! the threads that run git.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use oso_core::editor::Editor;
use oso_core::project::{Found, ProjectId, RootIdentity, TrustList};

use super::path::identity_of;

/// The root of a project that is trusted to run git, with its device and inode as the project
/// was found. [`TrustedRoot::of`] makes one from a trusted project, and
/// [`TrustedRoot::of_member`] one from an issue space's repository the trust list holds by path
/// and identity, or a native session's root with no open buffer of its project (ADR 0165, part
/// 4a); the latter is made fresh for each use, so its users ask the trust list again.
/// [`TrustedRoot::of_claim`] makes one from a linked worktree's claim, read afresh.
/// [`TrustedRoot::for_worktree`] makes a worktree's. Its clones share one revocation: once one
/// is revoked, so are all, but for [`TrustedRoot::apart`]'s.
#[derive(Clone, Debug)]
pub(crate) struct TrustedRoot {
    root: RootIdentity,
    made: Made,
    revoked: Arc<AtomicBool>,
}

/// What a root was made from.
#[derive(Clone, Debug)]
enum Made {
    /// A project of the editor's table.
    Project(ProjectId),
    /// A root the trust list holds.
    Listed,
    /// The main checkout a worktree job found, or a claim checked, as it was then, which the
    /// root belongs with.
    Worktree(RootIdentity),
}

impl TrustedRoot {
    /// The root of project `id`, if the editor trusts it.
    pub(crate) fn of(editor: &Editor, id: ProjectId) -> Option<TrustedRoot> {
        if !editor.project_trusted(id) {
            return None;
        }
        let project = editor.project(id)?;
        Some(TrustedRoot {
            root: RootIdentity {
                path: project.root().to_owned(),
                identity: project.identity(),
            },
            made: Made::Project(id),
            revoked: Arc::default(),
        })
    }

    /// The root of a repository at `path` with `identity`, an issue space's member that no
    /// open buffer may show, if `list` trusts it by both (ADR 0136, step 4).
    pub(crate) fn of_member(
        list: &TrustList,
        path: &Path,
        identity: Option<(u64, u64)>,
    ) -> Option<TrustedRoot> {
        let root = RootIdentity {
            path: path.to_owned(),
            identity,
        };
        list.trusts(&root).then(|| TrustedRoot {
            root,
            made: Made::Listed,
            revoked: Arc::default(),
        })
    }

    /// The root of the linked worktree `found`, as the walk read it just now, if `list` trusts
    /// the main repository its claim, checked both ways, belongs with (ADR 0114): for a native
    /// session's resume (ADR 0165).
    pub(crate) fn of_claim(list: &TrustList, found: &Found) -> Option<TrustedRoot> {
        let main = found.checked_main.as_ref()?;
        list.trusts(main).then(|| TrustedRoot {
            root: found.root.clone(),
            made: Made::Worktree(main.clone()),
            revoked: Arc::default(),
        })
    }

    /// This root, taken back with `revoked`, which the UI holds for a native start judged by
    /// the trust list until it starts (ADR 0165, part 4a).
    pub(crate) fn revoked_by(self, revoked: Arc<AtomicBool>) -> TrustedRoot {
        TrustedRoot { revoked, ..self }
    }

    /// The device and inode it was trusted with, where the system has them.
    pub(crate) fn identity(&self) -> Option<(u64, u64)> {
        self.root.identity
    }

    #[cfg(test)]
    pub(crate) fn for_tests(path: PathBuf) -> TrustedRoot {
        let identity = identity_of(&path);
        TrustedRoot {
            root: RootIdentity { path, identity },
            made: Made::Listed,
            revoked: Arc::default(),
        }
    }

    /// The same trust for `path`, a directory a worktree job found or just made in the
    /// repository whose main checkout is at `main`, as the record of trusted projects spells
    /// it, which it belongs with as ADR 0114 has it (ADR 0121). Its revocation is this root's.
    /// It looks at the disk, so the job calls it.
    pub(crate) fn for_worktree(&self, path: PathBuf, main: &Path) -> TrustedRoot {
        let identity = identity_of(&path);
        let main = RootIdentity {
            path: main.to_owned(),
            identity: identity_of(main),
        };
        TrustedRoot {
            root: RootIdentity { path, identity },
            made: Made::Worktree(main),
            revoked: Arc::clone(&self.revoked),
        }
    }

    /// The same trust with a revocation of its own, for a session that starts under what a
    /// job or another session holds, so that each is taken back by itself. One taken back
    /// already stays so.
    pub(crate) fn apart(self) -> TrustedRoot {
        if self.revoked() {
            return self;
        }
        self.revoked_by(Arc::default())
    }

    /// Whether `list` trusts it now, by path and identity: the root, or the main repository
    /// its worktree job found.
    pub(crate) fn listed_in(&self, list: &TrustList) -> bool {
        list.trusts(&self.root) || matches!(&self.made, Made::Worktree(main) if list.trusts(main))
    }

    /// Whether the project it was made from is one `editor` trusts now, where it was made from
    /// one.
    pub(crate) fn project_trusted(&self, editor: &Editor) -> bool {
        matches!(self.made, Made::Project(id) if editor.project_trusted(id))
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
        &self.root.path
    }

    /// Whether another directory is now at the root's path than the one trusted. It looks at
    /// the disk, so it belongs off the UI thread.
    pub(crate) fn replaced(&self) -> bool {
        !self.still_there()
    }

    /// Whether the root is still the directory that was trusted, by its device and inode where
    /// the system has them, not following a link.
    fn still_there(&self) -> bool {
        match self.root.identity {
            Some(identity) => identity_of(&self.root.path) == Some(identity),
            None => {
                std::fs::symlink_metadata(&self.root.path).is_ok_and(|metadata| metadata.is_dir())
            }
        }
    }
}
