//! The saved reviews as `review-resume` and `review-forget` list them (ADR 0122, step 6): each
//! a candidate of its own, made unique, and what picking one does.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here reads a file: the
//! store's thread lists them.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::complete::Candidate;

use super::super::scope_label;
use super::store;
use crate::app::App;
use crate::commands::review::{MAX_NAME, RESTRICTED, SAVED_LIST, one_line};
use crate::git::TrustedRoot;
use crate::review::saved::Header;

/// What `review-resume` and `review-layout` say of a saved review whose project at `root` oso
/// does not know yet (ADR 0184).
pub(in crate::app::reviews) fn unknown_project(root: &std::path::Path) -> String {
    format!(
        "oso does not know that project yet: open a file of {} first",
        one_line(&root.to_string_lossy(), 2 * MAX_NAME)
    )
}

/// Whether a saved review of the project at `root`, which oso knows as `project`, may open
/// again: only in a project oso knows and trusts, else what to say, and nothing runs.
///
/// # Errors
///
/// What to say: the project is not known, or not trusted.
pub(in crate::app::reviews) fn resumable(
    editor: &oso_core::editor::Editor,
    project: Option<oso_core::project::ProjectId>,
    root: &std::path::Path,
) -> Result<oso_core::project::ProjectId, String> {
    let project = project.ok_or_else(|| unknown_project(root))?;
    match TrustedRoot::of(editor, project) {
        Some(_) => Ok(project),
        None => Err(RESTRICTED.to_owned()),
    }
}

/// How many hex digits of a file's name tell two candidates of one text apart.
const NAME_DIGITS: usize = 8;

impl App {
    /// The saved reviews, as the store listed them, are `review-resume`'s candidates: each its
    /// scope and root, with its comments and when it was used, two of one text told apart by
    /// their files' names.
    pub(in crate::app::reviews) fn saved_reviews_listed(&mut self, listed: Vec<(String, Header)>) {
        let now = store::now();
        let texts: Vec<String> = (listed.iter())
            .map(|(_, header)| {
                let root = one_line(&header.root.to_string_lossy(), 2 * MAX_NAME);
                format!("{} {root}", scope_label(&header.scope))
            })
            .collect();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for text in &texts {
            *counts.entry(text.as_str()).or_default() += 1;
        }
        let texts: Vec<String> = (texts.iter().zip(&listed))
            .map(|(text, (name, _))| match counts.get(text.as_str()) {
                Some(1) => text.clone(),
                _ => format!("{text} #{}", &name[..NAME_DIGITS.min(name.len())]),
            })
            .collect();
        let mut candidates = Vec::with_capacity(listed.len());
        let mut kept = Vec::with_capacity(listed.len());
        for (text, (name, header)) in texts.into_iter().zip(listed) {
            let comments = match header.comments {
                1 => "1 comment".to_owned(),
                n => format!("{n} comments"),
            };
            let days = now.saturating_sub(header.opened) / (24 * 60 * 60 * 1000);
            let age = match days {
                0 => "today".to_owned(),
                1 => "1 day ago".to_owned(),
                n => format!("{n} days ago"),
            };
            let annotation = format!("{comments}, {} unsent \u{b7} {age}", header.unsent);
            candidates.push(Candidate::new(text.as_str(), "", annotation));
            kept.push((text, name, header));
        }
        self.editor.set_listed(SAVED_LIST, Arc::from(candidates));
        self.reviews.keep.listed_once = true;
        let old = std::mem::replace(&mut self.reviews.keep.listed, kept);
        self.freer.free(old);
    }

    /// The saved review `pick` names, by its file's name and header; refused when none, or
    /// more than one, has that text.
    fn saved_pick(&mut self, pick: &str) -> Option<(String, Header)> {
        let mut found = (self.reviews.keep.listed.iter()).filter(|(text, ..)| text == pick);
        match (found.next(), found.next()) {
            (Some((_, name, header)), None) => Some((name.clone(), header.clone())),
            _ => {
                self.editor.message("No such saved review");
                None
            }
        }
    }

    /// `review-resume` picked `pick`: the saved review opens again in its project, once oso
    /// knows the project and trusts it. A session review compares with its saved baseline.
    pub(in crate::app::reviews) fn resume_review(&mut self, pick: &str) {
        let Some((_, header)) = self.saved_pick(pick) else {
            return;
        };
        self.resume_saved(&header.root, header.scope);
    }

    /// Opens the saved review of `scope` in the project at `root` again, laid out, once oso
    /// knows the project and trusts it, as `review-resume` does: else says why, and nothing
    /// runs.
    pub(in crate::app::reviews) fn resume_saved(
        &mut self,
        root: &std::path::Path,
        scope: crate::review::Scope,
    ) -> Option<oso_core::editor::ReviewId> {
        match resumable(&self.editor, self.project_with_root(root), root) {
            Ok(project) => self.open_scope(project, scope, super::super::Show::Laid),
            Err(why) => {
                self.editor.message(why);
                None
            }
        }
    }

    /// `review-forget` picked `pick`: the saved review is removed now.
    pub(in crate::app::reviews) fn forget_review(&mut self, pick: &str) {
        let Some((name, _)) = self.saved_pick(pick) else {
            return;
        };
        if let Some(store) = &self.reviews.keep.store {
            store.remove(name);
        }
        self.editor.message("The saved review was removed");
    }
}
