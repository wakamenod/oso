//! Whether a session's project is trusted now, as an open buffer of it has it (ADR 0114): the
//! session tools ask it of their caller's project (ADR 0121), and a native session's start of
//! theirs, which only a trusted project runs (ADR 0165, Q5). A native resume or fork with no
//! open buffer of its project asks the record of trusted projects instead, and its start
//! thread its root's identity (part 4a, `sessions::start::trust`).
//!
//! Must not depend on: the disk; the editor's projects and trust list say it.
//!
//! - An open project that is not trusted is not trusted, whatever the session began with or
//!   the record says.
//! - A native start fails closed: with no open project at its root, a new one does not run
//!   natively, and a resume or fork only as the record has it, once read.

use std::path::Path;

use crate::app::App;
use crate::git::TrustedRoot;
pub(in crate::app::sessions::hosting) use crate::sessions::start::trust::NotTrusted;
use crate::sessions::start::trust::{self, Judged, Record};

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

    /// The trust a new native session in `directory`, of the project at `root`, runs under
    /// now: only at its project's root, and only a trusted open project's.
    pub(in crate::app::sessions::hosting) fn native_trust_now(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<TrustedRoot, NotTrusted> {
        match self.native_trust(directory, root, false)? {
            Judged::Open(trusted) => Ok(trusted),
            Judged::Listed(_) | Judged::Waits => Err(NotTrusted::NotOpen),
        }
    }

    /// Where the trust a native session in `directory`, of the project at `root`, resumes or
    /// forks under comes from: an open buffer of its project, else the record of trusted
    /// projects, which it waits for while starts may wait (ADR 0173).
    pub(in crate::app::sessions::hosting) fn native_trust_listed(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<Judged, NotTrusted> {
        self.native_trust(directory, root, true)
    }

    fn native_trust(
        &self,
        directory: &Path,
        root: Option<&Path>,
        by_record: bool,
    ) -> Result<Judged, NotTrusted> {
        let root = root.unwrap_or(directory);
        let record = by_record.then(|| self.trust_record());
        let may_wait = !self.starting.held.waited_out();
        trust::judge(
            root == directory,
            root,
            self.open_root_trust(root),
            record,
            may_wait,
        )
    }

    /// The record of trusted projects as the editor holds it: unread until its thread reported
    /// once; with no thread, as with no configuration directory, there is none to read.
    fn trust_record(&self) -> Record<'_> {
        match self.editor.trust_list() {
            Some(list) => Record::Read(list),
            None if self.trust_file.is_some() && !self.starting.trust_read => Record::Unread,
            None => Record::Refused,
        }
    }
}
