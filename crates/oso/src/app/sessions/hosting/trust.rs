//! Whether a native session's project is trusted now, as an open file or terminal of it has it
//! (ADR 0114), as only a trusted project runs one (ADR 0165, Q5); the session tools' lookup,
//! which counts the transcripts too, is `open_projects`'. A native resume or fork with no
//! open buffer of its project asks the record of trusted projects instead, and its start
//! thread its root's identity (part 4a, `sessions::start::trust`), or, for a resume in a linked
//! worktree, its claim.
//!
//! Must not depend on: the disk; the editor's projects and trust list say it.
//!
//! - An open buffer is a file or a terminal here, never a transcript.
//! - An open project that is not trusted is not trusted, whatever the session began with or
//!   the record says.
//! - A native start fails closed: with no open project at its root, a new one does not run
//!   natively, a fork or an adoption only as the record has it, once read, and a resume as the
//!   record or its worktree's claim has it.

use std::path::Path;

use oso_core::editor::Editor;
use oso_core::project::ProjectId;

use super::open_projects::{of_files_and_terminals, trust_at};
use crate::app::App;
use crate::git::TrustedRoot;
use crate::sessions::start::trust::{self, ByRecord, Claims, Judged, NotTrusted, TrustRecord};
use crate::sessions::worker::SessionKey;

/// As [`App::open_root_trust`], of `editor`'s files and terminals.
fn open_root_trust(editor: &Editor, root: &Path) -> Option<Option<TrustedRoot>> {
    trust_at(editor, of_files_and_terminals(editor), root)
}

/// Whether a person's new session in project `id`, as [`App::project_at_root`] found it, runs
/// natively by its trust: only in the selected buffer's, or one a file or a terminal has open.
pub(super) fn starts_natively_in(editor: &Editor, id: ProjectId) -> bool {
    let selected = editor.current().1.project() == Some(id);
    (selected || of_files_and_terminals(editor).any(|open| open == id))
        && editor.project_trusted(id)
}

impl App {
    /// The trust of the project at `root` as an open file or terminal of it has it now, as
    /// native starts and resumes are judged: `None` when none has it open, `Some(None)` when
    /// it is not trusted. A transcript is none: its own must not decide its session's resume.
    pub(in crate::app) fn open_root_trust(&self, root: &Path) -> Option<Option<TrustedRoot>> {
        open_root_trust(&self.editor, root)
    }

    /// The trust a new native session in `directory`, of the project at `root`, runs under
    /// now: only at its project's root, and only a trusted open project's.
    pub(in crate::app::sessions::hosting) fn native_trust_now(
        &self,
        directory: &Path,
        root: Option<&Path>,
    ) -> Result<TrustedRoot, NotTrusted> {
        match self.native_trust(directory, root, false, false)? {
            Judged::Open(trusted) => Ok(trusted),
            Judged::Listed(_) | Judged::Claimed(_) | Judged::Waits => Err(NotTrusted::NotOpen),
        }
    }

    /// Where the trust a native session in `directory`, of the project at `root`, resumes or
    /// forks under comes from: an open buffer of its project, else the record of trusted
    /// projects, which it waits for while starts may wait (ADR 0173), or, only for a resume of
    /// a session oso has (`by_claim`), its worktree's claim.
    pub(in crate::app::sessions::hosting) fn native_trust_listed(
        &self,
        directory: &Path,
        root: Option<&Path>,
        by_claim: bool,
    ) -> Result<Judged, NotTrusted> {
        self.native_trust(directory, root, true, by_claim)
    }

    fn native_trust(
        &self,
        directory: &Path,
        root: Option<&Path>,
        by_record: bool,
        by_claim: bool,
    ) -> Result<Judged, NotTrusted> {
        let root = root.unwrap_or(directory);
        let record = by_record.then(|| self.trust_record());
        let may_wait = !self.starting.held.waited_out();
        let open = (self.open_root_trust(root)).or_else(|| self.open_same_root(root, record?));
        let claims = match by_claim {
            true => Claims::Asked {
                home: self.editor.home(),
            },
            false => Claims::Never,
        };
        trust::judge(root == directory, root, open, record, may_wait, claims)
    }

    /// `Some(None)` when an open project at `root` under another spelling, as a case-folding
    /// file system lets one be found, which the record pairs with `root` by its identity, is
    /// not trusted: it refuses as the one spelled so would. A trusted one decides nothing, so
    /// the record judges the root, and the start thread its identity where the session runs.
    fn open_same_root(&self, root: &Path, record: TrustRecord<'_>) -> Option<Option<TrustedRoot>> {
        // A root the record does not hold pairs with none, as a worktree's.
        let TrustRecord::Read(list) = record else {
            return None;
        };
        if !list.paths().any(|path| path == root) {
            return None;
        }
        let restricted = of_files_and_terminals(&self.editor).any(|id| {
            self.editor.project(id).is_some_and(|project| {
                project.identity().is_some()
                    && list.trusts(&oso_core::project::RootIdentity {
                        path: root.to_owned(),
                        identity: project.identity(),
                    })
            }) && TrustedRoot::of(&self.editor, id).is_none()
        });
        restricted.then_some(None)
    }

    /// The order's `trusted` and its start thread's judgement by the record, for native
    /// session `key` judged so; what is kept takes its trust back until it starts.
    pub(in crate::app::sessions::hosting) fn native_judged(
        &mut self,
        key: SessionKey,
        judged: Judged,
    ) -> (Option<TrustedRoot>, ByRecord) {
        match judged.kept() {
            Some(kept) => self.hosting.native_trusts.insert(key, kept),
            None => self.hosting.native_trusts.remove(&key),
        };
        judged.into_order()
    }

    /// Forgets what was kept of start `token`'s trust: its start thread reported.
    pub(in crate::app::sessions::hosting) fn native_start_reported(&mut self, token: SessionKey) {
        self.hosting.native_trusts.remove(&token);
        self.space_gate_reported(token);
    }

    /// Takes back the trust of each native resume or fork under way whose project is no
    /// longer trusted, by an open buffer of it or the record: its start thread refuses it at
    /// its look before the spawn. What started or was refused is forgotten. After each event.
    pub(in crate::app) fn revoke_untrusted_native_starts(&mut self) {
        if self.hosting.native_trusts.is_empty() {
            return;
        }
        // One no longer starting with no report, as when its buffer was killed, is taken back:
        // its start thread may still be under way. A report forgets it first.
        let starting = &self.hosting.starting;
        self.hosting.native_trusts.retain(|key, kept| {
            let under_way = starting.contains_key(key);
            if !under_way {
                kept.revoke();
            }
            under_way
        });
        let gone: Vec<SessionKey> = (self.hosting.native_trusts.iter())
            .filter(|(_, kept)| {
                let root = kept.root();
                let record = self.trust_record();
                let open =
                    (self.open_root_trust(root)).or_else(|| self.open_same_root(root, record));
                !kept.still_trusted(open, record)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in gone {
            if let Some(kept) = self.hosting.native_trusts.remove(&key) {
                kept.revoke();
            }
        }
    }

    /// The record of trusted projects as the editor holds it: unread until its thread reported
    /// once; with no thread, as with no configuration directory, there is none to read.
    pub(in crate::app::sessions::hosting) fn trust_record(&self) -> TrustRecord<'_> {
        match self.editor.trust_list() {
            Some(list) => TrustRecord::Read(list),
            None if self.trust_file.is_some() && !self.starting.trust_read => TrustRecord::Unread,
            None => TrustRecord::Refused,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use oso_core::editor::Editor;
    use oso_core::project::TrustList;

    use super::super::open_projects::tests::{file, record, transcript};
    use super::{open_root_trust, starts_natively_in};

    #[test]
    fn a_transcript_is_no_open_buffer_of_the_judges_trusted_or_not() {
        let mut editor = Editor::default();
        transcript(&mut editor, "/w/own");
        assert!(open_root_trust(&editor, Path::new("/w/own")).is_none());
        editor.set_trust_list(Some(record("/w/own")));
        assert!(open_root_trust(&editor, Path::new("/w/own")).is_none());
        file(&mut editor, "/w/own");
        let open = open_root_trust(&editor, Path::new("/w/own"));
        assert!(open.expect("a file's").is_some());
    }

    #[test]
    fn a_transcript_not_selected_never_makes_a_persons_new_session_native() {
        let mut editor = Editor::default();
        editor.set_trust_list(Some(record("/w/own")));
        let id = transcript(&mut editor, "/w/own");
        let project = editor.buffer(id).unwrap().project().unwrap();
        assert!(editor.project_trusted(project));
        assert!(!starts_natively_in(&editor, project));
        let scratch = editor.current().0.buffer();
        editor.show(id);
        assert!(
            starts_natively_in(&editor, project),
            "the selected buffer's"
        );
        editor.show(scratch);
        assert_eq!(file(&mut editor, "/w/own"), project);
        assert!(starts_natively_in(&editor, project), "a file's");
        editor.set_trust_list(Some(TrustList::default()));
        assert!(!starts_natively_in(&editor, project));
    }
}
