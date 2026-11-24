//! The lists the space prompts complete over (ADR 0136, step 1b): the repositories oso knows,
//! the space in front's, and the closed spaces, made when the sessions or the spaces change.
//!
//! Must not depend on: key presses or how a frame is drawn.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use oso_core::complete::Candidate;

use super::spelled;
use crate::app::App;
use crate::commands::agent_sessions::{ARCHIVED_LIST, MEMBERS_LIST, REPOSITORIES_LIST};
use crate::sessions::spaces::stored::is_plain;

impl App {
    /// Gives the space prompts their lists: the repositories oso knows, the space in front's,
    /// and the closed spaces.
    pub(in crate::app) fn space_lists(&mut self) {
        let spaces = self.watch.spaces();
        let members = spaces.member_candidates();
        let archived = spaces.archived_candidates();
        let known = self.known_repositories();
        self.editor.set_listed(REPOSITORIES_LIST, known);
        self.editor.set_listed(MEMBERS_LIST, members);
        self.editor.set_listed(ARCHIVED_LIST, archived);
        self.keep_space_scope(true);
    }

    /// The repositories oso knows, by path, with their names: the open spaces', the
    /// remembered sessions', and the projects of buffers that are no worktree git listed. A
    /// path with a control or invisible character is left out, as a candidate's text is the
    /// answer and is shown as it is. Each root is looked at once, and the listed worktrees
    /// through one index of them.
    fn known_repositories(&self) -> Arc<[Candidate]> {
        self.known_roots()
            .into_iter()
            .filter_map(|root| {
                let text = root.to_str()?.to_owned();
                if !text.chars().all(is_plain) {
                    return None;
                }
                let name = crate::sessions::spaces::stored::last_component(&root);
                Some(Candidate::new(text.as_str(), "", name))
            })
            .collect()
    }

    /// The roots of the repositories oso knows, each once: what `space-add-repo` completes
    /// over, and what an agent's `space_add_repo` may add without asking (ADR 0161).
    pub(in crate::app) fn known_roots(&self) -> Vec<PathBuf> {
        let listed = self.listed_worktrees();
        let worktree = |root: &Path| listed.contains(&spelled(root));
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut roots: Vec<PathBuf> = Vec::new();
        for root in self.watch.spaces().members() {
            if seen.insert(spelled(&root)) {
                roots.push(root);
            }
        }
        let projects: HashSet<_> = self.open_projects().collect();
        let project_roots = projects
            .into_iter()
            .filter_map(|id| self.editor.project(id))
            .map(|project| project.root().to_path_buf());
        let session_roots = self.hosting.table().iter().filter_map(|s| s.root.clone());
        for root in session_roots.chain(project_roots) {
            if !worktree(&root) && seen.insert(spelled(&root)) {
                roots.push(root);
            }
        }
        roots
    }
}
