//! Discarding in the app (ADR 0127, step 1c): the question before `k` runs, the list of what
//! discards kept, `*git discarded: NAME*`, and writing a copy back after its own question.
//!
//! Must not depend on: key presses or how a frame is drawn. Nothing here runs git or reads the
//! disk: the status's thread discards, and the repository's run thread reads the record and
//! writes a copy back.
//!
//! - The question holds the targets the table named as `k` was pressed; the `y` runs them only
//!   while that table is the one shown, else it says the status changed.
//! - A file whose buffer has unsaved edits is neither discarded nor written back (the author's
//!   choice): "save or revert it first".
//! - The list is an internal buffer whose keys are the person's: the agent channel neither
//!   shows nor changes it.

use std::sync::Arc;

use oso_core::buffer::{BufferId, Mode};
use oso_core::editor::{GitLines, row_text};
use oso_core::project::ProjectId;

use super::super::App;
use super::Ask;
use super::runs::RUNNING;
use crate::commands::vcs::RESTRICTED;
use crate::git::TrustedRoot;
use crate::vcs::discard;
use crate::vcs::discarded::{self, Entry};
use crate::vcs::layout::Built;
use crate::vcs::run::RunRequest;
use crate::vcs::stage::{self, Target};

/// The rows of a list before its first entry.
const HEADER_ROWS: usize = 2;

/// The discards asked about and the lists shown.
#[derive(Default)]
pub(in crate::app) struct Discards {
    /// The discard the open question asks about.
    pending: Option<Pending>,
    /// The copy the open question would write back, and the list it is in.
    restoring: Option<(BufferId, Entry)>,
    lists: Vec<List>,
    /// The projects whose list is shown once read.
    show: Vec<ProjectId>,
}

/// A discard asked about: the status, the table it named targets in, and the targets.
struct Pending {
    status: BufferId,
    built: Arc<Built>,
    targets: Vec<Target>,
}

/// A list of what discards kept.
struct List {
    buffer: BufferId,
    project: ProjectId,
    entries: Vec<Entry>,
}

impl App {
    /// `k` in the status `status`: asks about what `lines` name, or, `confirmed`, runs what
    /// the question asked about.
    pub(super) fn discard_git(&mut self, status: BufferId, lines: &[GitLines], confirmed: bool) {
        let Some(index) = self.git_index(status) else {
            return;
        };
        if confirmed {
            let Some(pending) = self.gits.commits.discards.pending.take() else {
                return;
            };
            let same = pending.status == status
                && Arc::ptr_eq(&pending.built, &self.gits.open[index].built);
            if !same {
                return self
                    .editor
                    .message("The status changed since the question: k again");
            }
            return self.git_ask(index, Ask::Discard(pending.targets));
        }
        let open = &self.gits.open[index];
        let (project, built) = (open.project, Arc::clone(&open.built));
        if self.repo_busy(project) {
            return self.editor.message(RUNNING);
        }
        let targets = match stage::targets(&built, lines) {
            Ok(targets) if targets.is_empty() => {
                return self.editor.message("Nothing here to discard");
            }
            Ok(targets) => targets,
            Err(why) => return self.editor.message(why),
        };
        if let Some(why) = discard::too_many(&built, &targets) {
            return self.editor.message(why);
        }
        // What the thread would refuse is said before the question, at most a thousand files'
        // worth of work, as `too_many` let none past that.
        if let Err(why) = discard::plan(&built, &targets) {
            return self.editor.message(why);
        }
        if let Some(name) = self.unsaved_among(index, &built, &targets) {
            return self
                .editor
                .message(format!("{name} has unsaved edits: save or revert it first"));
        }
        let question = discard::question(&built, &targets);
        self.gits.commits.discards.pending = Some(Pending {
            status,
            built,
            targets,
        });
        if let Err(e) = self
            .editor
            .ask_with(question, "git-discard", ["discard", "keep"])
        {
            self.editor.message(e.to_string());
        }
        self.request_redraw();
    }

    /// The name of a file `targets` of `built` name whose buffer has unsaved edits, if one has.
    fn unsaved_among(&self, index: usize, built: &Built, targets: &[Target]) -> Option<String> {
        let root = self.gits.open[index].root.path();
        let edited: std::collections::HashSet<Vec<u8>> = (self.editor.buffers())
            .filter(|(_, buffer)| buffer.is_modified())
            .filter_map(|(_, buffer)| buffer.path()?.strip_prefix(root).ok())
            .filter_map(super::git_bytes)
            .collect();
        if edited.is_empty() {
            return None;
        }
        let named = |path: &[u8]| edited.contains(path);
        let hit = targets.iter().find_map(|target| {
            let (section, file) = match *target {
                Target::Section(section) => {
                    let files = built.files(section);
                    return files
                        .iter()
                        .find(|file| named(&file.path))
                        .map(|f| f.path.clone());
                }
                Target::File { section, file }
                | Target::Hunk { section, file, .. }
                | Target::Lines { section, file, .. } => (section, file),
            };
            built
                .file(section, file)
                .filter(|found| named(&found.path))
                .map(|found| found.path.clone())
        })?;
        Some(row_text(&String::from_utf8_lossy(&hit)))
    }

    /// `M-x git-discarded`, or `g` in the list: reads what discards kept in the repository of
    /// `buffer`, and shows the list.
    pub(super) fn show_discarded(&mut self, buffer: BufferId) {
        let lists = &self.gits.commits.discards.lists;
        let project = (lists.iter().find(|list| list.buffer == buffer))
            .map(|list| list.project)
            .or_else(|| self.editor.git_status_of(buffer))
            .or_else(|| self.editor.buffer(buffer)?.project());
        let Some(project) = project else {
            return self.editor.message("Not in a project");
        };
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        let index = self.git_repo(project, &root);
        if self.ask_runner(index, RunRequest::Discarded) {
            let show = &mut self.gits.commits.discards.show;
            if !show.contains(&project) {
                show.push(project);
            }
        }
    }

    /// What discards kept in the repository at `index` was read.
    pub(super) fn discarded_read(&mut self, index: usize, read: Result<Vec<Entry>, String>) {
        let project = self.gits.commits.repos[index].project;
        let shows = (self.gits.commits.discards.show.iter()).position(|p| *p == project);
        let entries = match read {
            Ok(entries) => entries,
            Err(why) => {
                return self
                    .editor
                    .message(format!("Cannot read what was kept: {why}"));
            }
        };
        let at = (self.gits.commits.discards.lists.iter()).position(|list| list.project == project);
        let at = match at {
            Some(at)
                if self
                    .editor
                    .buffer(self.gits.commits.discards.lists[at].buffer)
                    .is_some() =>
            {
                at
            }
            _ if shows.is_none() => return,
            _ => {
                let name =
                    self.unique_name(&format!("*git discarded: {}*", self.project_name(project)));
                let buffer = self.editor.add_internal_buffer(name, Mode::GitDiscarded);
                let lists = &mut self.gits.commits.discards.lists;
                lists.retain(|list| list.project != project);
                lists.push(List {
                    buffer,
                    project,
                    entries: Vec::new(),
                });
                lists.len() - 1
            }
        };
        let buffer = self.gits.commits.discards.lists[at].buffer;
        let text = list_text(&self.project_name(project), &entries, discarded::now());
        self.editor.set_internal_text(buffer, &text);
        self.gits.commits.discards.lists[at].entries = entries;
        if let Some(shows) = shows {
            self.gits.commits.discards.show.remove(shows);
            self.editor.show(buffer);
        }
        self.request_redraw();
    }

    /// `RET` in the list `buffer`: asks about writing back the copy at `line`, or, `confirmed`,
    /// writes back what the question asked about.
    pub(super) fn restore_discarded(&mut self, buffer: BufferId, line: usize, confirmed: bool) {
        let lists = &self.gits.commits.discards.lists;
        let Some(list) = lists.iter().find(|list| list.buffer == buffer) else {
            return;
        };
        let project = list.project;
        let Some(root) = TrustedRoot::of(&self.editor, project) else {
            return self.editor.message(RESTRICTED);
        };
        if confirmed {
            let Some((asked, entry)) = self.gits.commits.discards.restoring.take() else {
                return;
            };
            if asked != buffer {
                return;
            }
            // A buffer of the file that may hold secrets keeps what it holds from any copy.
            let secret = (self.editor.buffers()).any(|(_, shown)| {
                shown.is_secret()
                    && (shown.path())
                        .and_then(|path| path.strip_prefix(root.path()).ok())
                        .and_then(super::git_bytes)
                        .is_some_and(|bytes| bytes == entry.path)
            });
            let index = self.git_repo(project, &root);
            self.ask_runner(index, RunRequest::Restore { entry, secret });
            return;
        }
        let Some(entry) = line
            .checked_sub(HEADER_ROWS)
            .and_then(|at| list.entries.get(at))
        else {
            return self.editor.message("No copy on this line");
        };
        let entry = entry.clone();
        let name = row_text(&String::from_utf8_lossy(&entry.path));
        let edited = (self.editor.buffers()).any(|(_, shown)| {
            shown.is_modified()
                && shown.path().is_some_and(|path| {
                    path.strip_prefix(root.path())
                        .ok()
                        .and_then(super::git_bytes)
                        .is_some_and(|bytes| bytes == entry.path)
                })
        });
        if edited {
            return self
                .editor
                .message(format!("{name} has unsaved edits: save or revert it first"));
        }
        let ago = ago(entry.time, discarded::now());
        let question = format!("Write {name} back as it was {ago}? What it holds now is kept too");
        self.gits.commits.discards.restoring = Some((buffer, entry));
        if let Err(e) = (self.editor).ask_with(question, "git-restore-discarded", ["write", "keep"])
        {
            self.editor.message(e.to_string());
        }
        self.request_redraw();
    }

    /// A copy was written back in the repository at `index`, or could not be.
    pub(super) fn restored(&mut self, index: usize, done: Result<String, String>) {
        let project = self.gits.commits.repos[index].project;
        match done {
            Ok(said) => self.editor.message(said),
            Err(why) => return self.editor.message(format!("Not written back: {why}")),
        }
        if let Some(status) = (self.gits.open.iter()).position(|open| open.project == project) {
            self.git_ask(status, Ask::Read);
        }
        // The list gains what the file held.
        self.ask_runner(index, RunRequest::Discarded);
    }
}

/// The list's text: a line on its keys, then each copy, newest first, with how long ago, its
/// short id and its path, escaped on one row.
fn list_text(project: &str, entries: &[Entry], now: u64) -> String {
    let mut text = format!("Kept by discards in {project}, newest first: RET writes one back\n\n");
    if entries.is_empty() {
        text.push_str("Nothing was kept in the last two weeks.\n");
    }
    for entry in entries {
        let path = row_text(&String::from_utf8_lossy(&entry.path));
        let ago = ago(entry.time, now);
        let id = &entry.id[..entry.id.len().min(7)];
        text.push_str(&format!("{ago:>12}  {id}  {path}\n"));
    }
    text
}

/// How long ago `then` was at `now`.
fn ago(then: u64, now: u64) -> String {
    let minutes = now.saturating_sub(then) / 60;
    match minutes {
        0 => "just now".to_owned(),
        1 => "1 min ago".to_owned(),
        m if m < 60 => format!("{m} min ago"),
        m if m < 48 * 60 => format!("{} h ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

#[cfg(test)]
mod tests {
    use super::{ago, list_text};
    use crate::vcs::discarded::Entry;

    #[test]
    fn the_list_says_how_long_ago_and_escapes_names() {
        let entry = |time, path: &[u8]| Entry {
            time,
            root: b"/r".to_vec(),
            id: "0123456789".repeat(4),
            path: path.to_vec(),
        };
        let text = list_text("p", &[entry(1000, b"a\nb"), entry(0, b"c")], 4000);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(
            rows[0],
            "Kept by discards in p, newest first: RET writes one back"
        );
        assert_eq!(rows[2], "  50 min ago  0123456  a\\nb");
        assert_eq!(rows[3], "     1 h ago  0123456  c");
        assert_eq!(ago(0, 3 * 24 * 3600), "3 days ago");
        assert!(list_text("p", &[], 0).contains("Nothing was kept"));
    }

    #[test]
    fn a_path_is_compared_as_git_names_it() {
        assert_eq!(
            super::super::git_bytes(std::path::Path::new("src/a.rs")),
            Some(b"src/a.rs".to_vec())
        );
    }
}
