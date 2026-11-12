//! The projects buffers are in (ADR 0114): the files whose project the host should find, a
//! table of what it found, which buffers point into, and which of them are trusted.
//!
//! Must not depend on: the host's threads or I/O. The host finds a project on a thread of its
//! own, after the file's text has come, and hands over what it found. It reads the record of
//! trusted projects on another, and writes what a person changed of it.

use std::collections::HashSet;
use std::path::PathBuf;

use super::Editor;
use crate::buffer::BufferId;
use crate::project::{
    Found, NotRecordable, Project, ProjectId, RootIdentity, TrustChange, TrustList,
};

/// The record of trusted projects as the editor holds it.
#[derive(Debug, Default)]
pub(super) struct Trust {
    /// The record as last read or changed, or `None` until it is read, or while it is refused
    /// or broken: nothing is trusted then, and nothing is written over it.
    list: Option<TrustList>,
    /// What a person changed of the record, the oldest first, until the host takes it to
    /// write.
    changes: Vec<TrustChange>,
    /// The root a question asked about trusting, until it is answered.
    asked: Option<RootIdentity>,
}

/// Why trusting or untrusting a project did not happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TrustRefused {
    /// The buffer is in no project.
    NoProject,
    /// The record is not read yet, or was refused or broken, so it is not written over.
    Unread,
    /// The project is trusted already.
    Already,
    /// The person has not said yes to trusting this root: ask them.
    Unconfirmed,
    /// Its root cannot be written in the record.
    NotRecordable(NotRecordable),
    /// The project is not trusted.
    NotTrusted,
    /// The project is trusted only as a worktree of the main repository at this root, whose
    /// line it is.
    ThroughMain(PathBuf),
}

/// A file whose project, or whose EditorConfig, the host should find: the buffer, and the real
/// place its path leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRequest {
    pub buffer: BufferId,
    pub location: PathBuf,
}

/// Adds `request` to `requests`, where only a buffer's newest stays.
pub(super) fn ask(requests: &mut Vec<ProjectRequest>, request: ProjectRequest) {
    requests.retain(|asked| asked.buffer != request.buffer);
    requests.push(request);
}

impl Editor {
    /// The request for `id`'s file, once where its path leads is known.
    pub(super) fn file_request(&self, id: BufferId) -> Option<ProjectRequest> {
        let location = self.buffer(id)?.baseline()?.location.clone();
        Some(ProjectRequest {
            buffer: id,
            location,
        })
    }

    /// Asks the host to find the project of `id`'s file, once where its path leads is known.
    /// A buffer with no such place is in no project.
    pub fn request_project(&mut self, id: BufferId) {
        if let Some(request) = self.file_request(id) {
            ask(&mut self.project_requests, request);
        }
    }

    /// The files whose project the host should find, the oldest first.
    pub fn take_project_requests(&mut self) -> Vec<ProjectRequest> {
        std::mem::take(&mut self.project_requests)
    }

    /// The host found `request`'s file in the project `found`, or in none. It is taken only if
    /// the buffer's path still leads where it did when asked.
    pub fn project_found(&mut self, request: &ProjectRequest, found: Option<Found>) {
        let current = self
            .buffer(request.buffer)
            .and_then(|buffer| buffer.baseline())
            .is_some_and(|baseline| baseline.location == request.location);
        if current {
            self.set_buffer_project(request.buffer, found);
        }
    }

    /// The project `id` names.
    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.projects.get(usize::try_from(id.0).ok()?)
    }

    /// The project of the buffer `id`, if its file is in one.
    #[cfg(test)]
    pub(crate) fn buffer_project(&self, id: BufferId) -> Option<&Project> {
        self.project(self.buffer(id)?.project()?)
    }

    /// Puts `id` in the project `found`, or in none. A project already in the table, by its
    /// root and the directory's identity, is shared, and takes what its `.git` file names this
    /// time; any other is added. So one root is one project.
    pub(crate) fn set_buffer_project(&mut self, id: BufferId, found: Option<Found>) {
        if self.buffer(id).is_none() {
            return;
        }
        let project = found.map(|found| self.intern_project(found));
        if let Some(buffer) = self.buffer_mut(id) {
            buffer.set_project(project);
        }
    }

    fn intern_project(&mut self, found: Found) -> ProjectId {
        if let Some(place) = self
            .projects
            .iter()
            .position(|known| known.found().root == found.root)
        {
            self.projects[place].set_worktree(found);
            return ProjectId(u32::try_from(place).unwrap_or(u32::MAX));
        }
        self.projects.push(Project::new(found));
        ProjectId(u32::try_from(self.projects.len() - 1).unwrap_or(u32::MAX))
    }

    /// Takes the record of trusted projects the host read, or `None` if it was refused or is
    /// broken, which trusts nothing. When a root's line comes or goes, the project of each
    /// buffer in a worktree that claims that root is asked for again, as the host checks a
    /// worktree's main repository only once that is trusted, and so is that of each file in no
    /// project yet, whose answer may be on its way, checked against the record before.
    pub fn set_trust_list(&mut self, list: Option<TrustList>) {
        if self.trust.list == list {
            return;
        }
        let roots = |list: &Option<TrustList>| -> HashSet<RootIdentity> {
            list.iter().flat_map(|list| list.roots()).cloned().collect()
        };
        let (before, after) = (roots(&self.trust.list), roots(&list));
        let changed: Vec<&RootIdentity> = before.symmetric_difference(&after).collect();
        self.trust.list = list;
        let claims_changed = |project: &Project| {
            project.found().worktree_of.as_deref().is_some_and(|claim| {
                changed
                    .iter()
                    .any(|root| crate::project::same_place(&root.path, claim))
            })
        };
        if changed.is_empty() {
            return;
        }
        let again: Vec<BufferId> = self
            .buffers
            .iter()
            .filter(|(_, buffer)| match buffer.project() {
                Some(id) => self.project(id).is_some_and(claims_changed),
                None => buffer.baseline().is_some(),
            })
            .map(|(id, _)| *id)
            .collect();
        for id in again {
            self.request_project(id);
        }
    }

    /// The record of trusted projects, if it was read, to check a worktree's claim against.
    pub fn trust_list(&self) -> Option<&TrustList> {
        self.trust.list.as_ref()
    }

    /// Whether the project `id` is trusted: the record holds its root, or the main repository
    /// it was checked to be a worktree of (ADR 0114). Never by its name.
    pub fn project_trusted(&self, id: ProjectId) -> bool {
        let (Some(project), Some(list)) = (self.project(id), &self.trust.list) else {
            return false;
        };
        project.trusted_by(list)
    }

    /// What a person changed of the record of trusted projects, the oldest first, for the host
    /// to make in the file as it is then and read back.
    pub fn take_trust_changes(&mut self) -> Vec<TrustChange> {
        std::mem::take(&mut self.trust.changes)
    }

    /// Trusts the project `id`, once the person said yes to a question about this root:
    /// `confirmed` after one, which must have asked about the root it has now.
    ///
    /// # Errors
    ///
    /// [`TrustRefused::Unconfirmed`] when the person should be asked, and the root is then
    /// remembered for their answer, or why it cannot be trusted.
    pub fn trust_project(&mut self, id: ProjectId, confirmed: bool) -> Result<(), TrustRefused> {
        let project = self.project(id).ok_or(TrustRefused::NoProject)?;
        let list = self.trust.list.as_ref().ok_or(TrustRefused::Unread)?;
        if project.trusted_by(list) {
            return Err(TrustRefused::Already);
        }
        let root = project.found().root.clone();
        let change = TrustChange::Trust(root.clone());
        let changed = list.apply(&change).map_err(TrustRefused::NotRecordable)?;
        let asked = self.trust.asked.take();
        if !confirmed || asked.as_ref() != Some(&root) {
            self.trust.asked = Some(root);
            return Err(TrustRefused::Unconfirmed);
        }
        self.change_trust(changed, change);
        Ok(())
    }

    /// Takes the line of the project `id`'s root out of the record, whatever identity it
    /// recorded. The project may stay trusted as a worktree of a trusted main repository.
    ///
    /// # Errors
    ///
    /// Why there is no line to take out.
    pub fn untrust_project(&mut self, id: ProjectId) -> Result<(), TrustRefused> {
        let project = self.project(id).ok_or(TrustRefused::NoProject)?;
        let list = self.trust.list.as_ref().ok_or(TrustRefused::Unread)?;
        let root = project.root();
        if !list.records(root) {
            return match &project.found().checked_main {
                Some(main) if list.trusts(main) => {
                    Err(TrustRefused::ThroughMain(main.path.clone()))
                }
                _ => Err(TrustRefused::NotTrusted),
            };
        }
        let changed = list.without(root);
        let change = TrustChange::Untrust(root.to_path_buf());
        self.change_trust(changed, change);
        Ok(())
    }

    /// Holds `list` until the host reads the file back, and `change` for the host to make.
    fn change_trust(&mut self, list: TrustList, change: TrustChange) {
        self.trust.changes.push(change);
        self.set_trust_list(Some(list));
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectRequest, TrustRefused};
    use crate::buffer::{Baseline, Buffer};
    use crate::editor::Editor;
    #[cfg(unix)]
    use crate::project::TrustChange;
    use crate::project::{Found, RootIdentity, TrustList};
    use std::path::PathBuf;

    fn found(path: &str, inode: u64) -> Found {
        Found {
            root: RootIdentity {
                path: PathBuf::from(path),
                identity: Some((1, inode)),
            },
            worktree_of: None,
            checked_main: None,
        }
    }

    /// A buffer visiting `path`, whose path was found to lead there.
    fn visited(path: &str) -> Buffer {
        let mut buffer = Buffer::visiting(PathBuf::from(path));
        buffer.set_baseline(Baseline {
            location: PathBuf::from(path),
            file: None,
        });
        buffer
    }

    fn asked(editor: &mut Editor, id: crate::buffer::BufferId) -> ProjectRequest {
        editor.request_project(id);
        editor.take_project_requests().pop().unwrap()
    }

    #[test]
    fn a_request_carries_where_the_path_leads_and_none_comes_without_it() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let request = asked(&mut editor, a);
        assert_eq!(request.location, PathBuf::from("/w/oso/a.rs"));
        let loose = editor.add_buffer(Buffer::visiting(PathBuf::from("/w/b.rs")));
        editor.request_project(loose);
        assert!(editor.take_project_requests().is_empty());
    }

    #[test]
    fn buffers_in_one_project_share_it_and_others_get_their_own() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/oso/b.rs"));
        let c = editor.add_buffer(visited("/w/other/c.rs"));
        for (id, root, inode) in [(a, "/w/oso", 7), (b, "/w/oso", 7), (c, "/w/other", 8)] {
            let request = asked(&mut editor, id);
            editor.project_found(&request, Some(found(root, inode)));
        }
        let project = |id| editor.buffer(id).unwrap().project().unwrap();
        assert_eq!(project(a), project(b));
        assert_ne!(project(a), project(c));
        assert_eq!(editor.buffer_project(a).unwrap().name(), "oso");
        assert_eq!(editor.buffer_project(c).unwrap().name(), "other");
    }

    #[test]
    fn a_root_made_again_at_the_same_path_is_another_project() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/oso/b.rs"));
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(b, Some(found("/w/oso", 9)));
        assert_ne!(
            editor.buffer(a).unwrap().project(),
            editor.buffer(b).unwrap().project()
        );
    }

    #[test]
    fn one_root_is_one_project_whatever_its_git_file_named() {
        let mut editor = Editor::with_buffer(visited("/w/wt/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/wt/b.rs"));
        let mut claims = found("/w/wt", 7);
        claims.worktree_of = Some(PathBuf::from("/w/main"));
        editor.set_buffer_project(a, Some(claims));
        editor.set_buffer_project(b, Some(found("/w/wt", 7)));
        let project = editor.buffer(a).unwrap().project();
        assert_eq!(project, editor.buffer(b).unwrap().project());
        assert_eq!(editor.buffer_project(a).unwrap().found().worktree_of, None);
    }

    #[test]
    fn an_answer_for_a_buffer_that_moved_since_is_dropped() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let request = asked(&mut editor, a);
        editor
            .buffer_mut(a)
            .unwrap()
            .set_path(PathBuf::from("/elsewhere/a.rs"));
        editor.project_found(&request, Some(found("/w/oso", 7)));
        assert_eq!(editor.buffer(a).unwrap().project(), None);
    }

    #[test]
    fn no_project_clears_it() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_buffer_project(a, None);
        assert_eq!(editor.buffer(a).unwrap().project(), None);
        assert!(editor.buffer_project(a).is_none());
    }

    #[test]
    fn a_buffer_has_one_request_waiting_the_newest() {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        let b = editor.add_buffer(visited("/w/oso/b.rs"));
        editor.request_project(a);
        editor.request_project(b);
        editor.buffer_mut(a).unwrap().set_baseline(Baseline {
            location: PathBuf::from("/w/moved/a.rs"),
            file: None,
        });
        editor.request_project(a);
        let requests = editor.take_project_requests();
        let asked: Vec<_> = requests
            .iter()
            .map(|r| (r.buffer, r.location.clone()))
            .collect();
        assert_eq!(
            asked,
            [
                (b, PathBuf::from("/w/oso/b.rs")),
                (a, PathBuf::from("/w/moved/a.rs"))
            ]
        );
    }

    /// An editor showing a file of the project `/w/oso`, with the record `list` read.
    fn in_oso(list: Option<&str>) -> (Editor, crate::project::ProjectId) {
        let mut editor = Editor::with_buffer(visited("/w/oso/a.rs"));
        let a = editor.tiled().0.buffer();
        editor.set_buffer_project(a, Some(found("/w/oso", 7)));
        editor.set_trust_list(list.map(|text| TrustList::parse(text).0));
        let id = editor.buffer(a).unwrap().project().unwrap();
        (editor, id)
    }

    #[cfg(unix)]
    #[test]
    fn trusting_asks_first_and_takes_only_the_yes_to_that_root() {
        let (mut editor, id) = in_oso(Some(""));
        assert!(!editor.project_trusted(id));
        assert_eq!(
            editor.trust_project(id, true),
            Err(TrustRefused::Unconfirmed),
            "never asked"
        );
        assert_eq!(
            editor.trust_project(id, false),
            Err(TrustRefused::Unconfirmed)
        );
        assert_eq!(editor.take_trust_changes(), []);
        assert_eq!(editor.trust_project(id, true), Ok(()));
        assert!(editor.project_trusted(id));
        let changes = editor.take_trust_changes();
        assert_eq!(changes, [TrustChange::Trust(found("/w/oso", 7).root)]);
        assert_eq!(editor.trust_project(id, false), Err(TrustRefused::Already));
    }

    #[cfg(unix)]
    #[test]
    fn a_yes_to_a_root_since_made_again_asks_again() {
        let (mut editor, id) = in_oso(Some(""));
        assert_eq!(
            editor.trust_project(id, false),
            Err(TrustRefused::Unconfirmed)
        );
        let a = editor.tiled().0.buffer();
        editor.set_buffer_project(a, Some(found("/w/oso", 9)));
        let again = editor.buffer(a).unwrap().project().unwrap();
        assert_eq!(
            editor.trust_project(again, true),
            Err(TrustRefused::Unconfirmed)
        );
        assert_eq!(editor.take_trust_changes(), []);
    }

    #[test]
    fn a_record_not_read_is_never_written_over() {
        let (mut editor, id) = in_oso(None);
        assert_eq!(editor.trust_project(id, false), Err(TrustRefused::Unread));
        assert_eq!(editor.untrust_project(id), Err(TrustRefused::Unread));
        assert_eq!(editor.take_trust_changes(), []);
    }

    #[cfg(unix)]
    #[test]
    fn untrusting_takes_out_the_roots_line_whatever_it_recorded() {
        let (mut editor, id) = in_oso(Some("/w/oso\tdev=1 ino=7\n/w/x\tdev=1 ino=1\n"));
        assert!(editor.project_trusted(id));
        assert_eq!(editor.untrust_project(id), Ok(()));
        assert!(!editor.project_trusted(id));
        let changes = editor.take_trust_changes();
        assert_eq!(changes, [TrustChange::Untrust(PathBuf::from("/w/oso"))]);
        let list = editor.trust_list().unwrap();
        assert_eq!(
            list.paths().collect::<Vec<_>>(),
            [std::path::Path::new("/w/x")]
        );
        assert_eq!(editor.untrust_project(id), Err(TrustRefused::NotTrusted));
        let (mut stale, id) = in_oso(Some("/w/oso\tdev=1 ino=5\n"));
        assert_eq!(
            stale.untrust_project(id),
            Ok(()),
            "a line of an older directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_checked_worktree_is_trusted_while_its_main_repository_is() {
        let mut editor = Editor::with_buffer(visited("/w/wt/a.rs"));
        let a = editor.tiled().0.buffer();
        let mut worktree = found("/w/wt", 8);
        worktree.worktree_of = Some(PathBuf::from("/w/oso"));
        editor.set_buffer_project(a, Some(worktree.clone()));
        let id = editor.buffer(a).unwrap().project().unwrap();
        editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=7\n").0));
        assert!(!editor.project_trusted(id), "a claim alone");
        // The record changed, so the worktree's project is asked for again.
        let request = editor.take_project_requests().pop().unwrap();
        assert_eq!(request.buffer, a);
        worktree.checked_main = Some(found("/w/oso", 7).root);
        editor.project_found(&request, Some(worktree));
        assert!(editor.project_trusted(id));
        assert_eq!(
            editor.untrust_project(id),
            Err(TrustRefused::ThroughMain(PathBuf::from("/w/oso")))
        );
        editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=6\n").0));
        assert!(
            !editor.project_trusted(id),
            "the main repository made again"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_record_that_changes_asks_again_only_for_worktrees_and_files_in_none_yet() {
        let (mut editor, _) = in_oso(Some(""));
        let loose = editor.add_buffer(visited("/w/loose.txt"));
        assert!(editor.take_project_requests().is_empty());
        editor.set_trust_list(Some(TrustList::parse("# nothing\n").0));
        assert!(editor.take_project_requests().is_empty(), "no line changed");
        editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=7\n").0));
        let asked: Vec<_> = editor
            .take_project_requests()
            .iter()
            .map(|request| request.buffer)
            .collect();
        assert_eq!(asked, [loose], "its answer may have been checked before");
    }

    #[cfg(unix)]
    #[test]
    fn only_a_worktree_whose_main_repositorys_line_changed_is_asked_again() {
        let mut editor = Editor::with_buffer(visited("/w/wt/a.rs"));
        let a = editor.tiled().0.buffer();
        let mut worktree = found("/w/wt", 8);
        worktree.worktree_of = Some(PathBuf::from("/w/oso"));
        editor.set_buffer_project(a, Some(worktree));
        editor.set_trust_list(Some(TrustList::parse("/w/other\tdev=1 ino=3\n").0));
        assert!(editor.take_project_requests().is_empty(), "another root");
        editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=7\n").0));
        assert_eq!(editor.take_project_requests().len(), 1, "its root came");
        editor.set_trust_list(Some(TrustList::parse("/w/oso\tdev=1 ino=9\n").0));
        assert_eq!(editor.take_project_requests().len(), 1, "made again");
        editor.set_trust_list(None);
        assert_eq!(editor.take_project_requests().len(), 1, "refused");
    }
}
