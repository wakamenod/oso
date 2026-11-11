//! A project's files for `project-find-file` (ADR 0114): the prompt that lists them, the walk
//! the host runs for it, and the batches of paths the walk sends, merged into the list as they
//! come.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host walks the
//! project and hands over what it found.
//!
//! The prompt opens empty, and the count shows `…` until the walk ends. Each batch is put in
//! the list's order and matched on its own: on the UI thread up to [`WORKER_PAST`] candidates,
//! and past that by a job on the worker (ADR 0096). While a job adds batches, the ones that
//! come wait, and go together in the next job once it reports, so a slow job never makes a
//! newer one start over. The selected row stays selected as rows come. A newer listing, or
//! closing the prompt, moves the walk's generation on, and the walker stops at its next
//! directory.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use super::Editor;
use crate::complete::{Candidate, MAX_INPUT_BYTES, Merge, WORKER_PAST, matched_part};
use crate::minibuffer::{PromptError, PromptId};

/// The most paths a batch merges on the UI thread; a larger one goes to the worker, as folding
/// and placing each costs up to half a microsecond (ADR 0114: under 1 ms a batch).
const MERGED_HERE: usize = 1000;

/// Past [`WORKER_PAST`], a job adds the paths waiting once they are this fraction of the list.
const GROWTH: usize = 8;

/// A walk of a project's files the host should start, as `C-x p f` opens (ADR 0114). `Debug`
/// gives its generation only.
#[derive(Clone)]
pub struct WalkRequest {
    /// The listing it is for, which [`Editor::files_walked`] is given back.
    pub generation: u64,
    /// The project's root, which the walk starts from and the paths are relative to.
    pub root: PathBuf,
    /// The live listing's generation: the walker stops at its next directory once this has
    /// moved past [`WalkRequest::generation`].
    pub live: Arc<AtomicU64>,
}

impl fmt::Debug for WalkRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WalkRequest")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

/// A walk ended: every file was found, or `cut` says it stopped at
/// [`MAX_WALKED`](crate::project::MAX_WALKED), which the count shows as `+`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkEnd {
    pub cut: bool,
}

/// The buffer is in no project, and its directory cannot stand in for one: it has no file, or
/// its directory is the home directory or the root of the file system (ADR 0114).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotInProject;

impl fmt::Display for NotInProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Not in a project")
    }
}

impl std::error::Error for NotInProject {}

/// What the prompt that lists a project's files keeps beside its candidates.
#[derive(Default)]
pub(super) struct ProjectFiles {
    prompt: Option<PromptId>,
    root: PathBuf,
    /// The generation of this listing's walk.
    generation: u64,
    /// The newest listing's generation, shared with each walker, which stops once it moves on.
    live: Arc<AtomicU64>,
    /// The walk to start, until the host takes it.
    request: Option<WalkRequest>,
    /// The walk has not ended.
    walking: bool,
    /// The walk stopped at its limit.
    cut: bool,
    /// The history that orders the list, as it was when the prompt opened, so that every batch
    /// is ordered alike: none around a buffer that may hold secrets.
    history: Arc<[String]>,
    /// Batches of paths that came and are not in the list yet, the oldest first: those a job
    /// on the worker is adding, and those that came since.
    unmerged: Vec<Arc<[Box<str>]>>,
}

impl Editor {
    /// The root and the name of the project whose files `C-x p f` lists: the current buffer's
    /// project, or else the directory of its file, named after its last component.
    ///
    /// # Errors
    ///
    /// [`NotInProject`] for a buffer with no file, or whose file the loader has not found
    /// yet, and where the directory would be the home directory, one that holds it, or the
    /// root of the file system, which no one lists whole (ADR 0114). The directory is the one
    /// the file's path leads to, as detection starts from it.
    pub fn project_files_root(&self) -> Result<(PathBuf, String), NotInProject> {
        let (_, buffer) = self.current();
        if let Some(project) = buffer.project().and_then(|id| self.project(id)) {
            return Ok((project.root().to_path_buf(), project.name().to_owned()));
        }
        let directory = buffer
            .baseline()
            .and_then(|baseline| baseline.location.parent())
            .ok_or(NotInProject)?;
        let holds_home = self
            .home
            .as_deref()
            .is_some_and(|home| home.starts_with(directory));
        if directory.parent().is_none() || holds_home {
            return Err(NotInProject);
        }
        let name = directory.file_name().map_or_else(
            || directory.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Ok((directory.to_path_buf(), name))
    }

    /// Opens a prompt that lists the files of the project at `root`, named `name`, as paths
    /// from its root, kept in the history `history`, one for each project, and asks the host to walk it ([`Editor::take_walk_request`]). It opens
    /// empty, and its count shows `…` until the walk ends. Its history is the project's own.
    /// A walk of a listing before stops at its next directory.
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be, and
    /// [`PromptError::Invalid`] for a root whose path is not UTF-8, as an answer could not
    /// name a file under it, or while a listing is open already, which keeps its own.
    /// Nothing changes.
    pub fn open_project_files(
        &mut self,
        root: PathBuf,
        name: &str,
        history: &'static str,
    ) -> Result<PromptId, PromptError> {
        let Some(spelled) = root
            .to_str()
            .filter(|_| self.project_files.prompt.is_none())
        else {
            return Err(PromptError::Invalid);
        };
        // Each project has a history of its own, under the name given and its root.
        let key = format!("{history}:{spelled}");
        let id = self.open_completion(format!("Find file in {name}:"), history, Arc::from([]))?;
        let mut history: Arc<[String]> = Arc::from([]);
        if let Some(prompt) = self.prompts.last_mut() {
            if !prompt.around_secret {
                history = self.histories.entries(&key).map(str::to_owned).collect();
            }
            prompt.history_key = Some(key);
        }
        let live = Arc::clone(&self.project_files.live);
        let generation = live.fetch_add(1, Ordering::Relaxed) + 1;
        let request = WalkRequest {
            generation,
            root: root.clone(),
            live: Arc::clone(&live),
        };
        let before = std::mem::replace(
            &mut self.project_files,
            ProjectFiles {
                prompt: Some(id),
                root,
                generation,
                live,
                request: Some(request),
                walking: true,
                cut: false,
                history,
                unmerged: Vec::new(),
            },
        );
        self.free_unmerged(before.unmerged);
        Ok(id)
    }

    /// The walk the host should start, once.
    pub fn take_walk_request(&mut self) -> Option<WalkRequest> {
        self.project_files.request.take()
    }

    /// Takes in a batch of the files the walk `generation` found, as paths from the root, and
    /// with `end`, that it ended. Each is put in the list where its order puts it, matched on
    /// its own, and the selected row stays selected. A batch of a walk that is not the newest
    /// listing's, or whose prompt has closed, is dropped.
    pub fn files_walked(&mut self, generation: u64, paths: Vec<Box<str>>, end: Option<WalkEnd>) {
        if self.project_files.prompt.is_none() || generation != self.project_files.generation {
            if paths.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(paths));
            }
            return;
        }
        if !paths.is_empty() {
            self.project_files.unmerged.push(paths.into());
        }
        if let Some(end) = end {
            self.project_files.walking = false;
            self.project_files.cut = end.cut;
        }
        self.merge_pending_batches();
        self.relayout_prompt();
    }

    /// Whether the innermost prompt lists a project's files.
    pub(super) fn lists_project(&self) -> bool {
        self.project_files
            .prompt
            .is_some_and(|id| self.prompts.last().is_some_and(|prompt| prompt.id == id))
    }

    /// Whether the innermost prompt lists a project's files whose walk has not ended, or whose
    /// rows are not all in the list yet.
    pub(super) fn project_files_pending(&self) -> bool {
        self.lists_project()
            && (self.project_files.walking || !self.project_files.unmerged.is_empty())
    }

    /// Whether the innermost prompt lists a project's files and the walk stopped at its limit.
    pub(super) fn project_files_cut(&self) -> bool {
        self.lists_project() && self.project_files.cut
    }

    /// Whether the innermost prompt lists a project's files with batches not in its list yet.
    pub(super) fn has_pending_batches(&self) -> bool {
        self.lists_project() && !self.project_files.unmerged.is_empty()
    }

    /// The path an answer `text` of the innermost prompt names, if it lists a project's files:
    /// the file under the root.
    pub(super) fn project_file_answer(&self, text: &str) -> Option<String> {
        if !self.lists_project() {
            return None;
        }
        Some(self.project_files.root.join(text).to_str()?.to_owned())
    }

    /// Adds the batches that came to the innermost prompt's list, if it lists a project's
    /// files: here up to [`WORKER_PAST`] candidates and [`MERGED_HERE`] paths, and otherwise by a
    /// job, unless one is adding batches already, whose report sends the rest. Past
    /// [`WORKER_PAST`], as a job copies the list, it waits until the paths waiting are an eighth
    /// of the list or the walk has ended, so the list is copied a bounded number of times.
    pub(super) fn merge_pending_batches(&mut self) {
        if !self.has_pending_batches() || self.merge_job_running() {
            return;
        }
        let added: usize = self
            .project_files
            .unmerged
            .iter()
            .map(|batch| batch.len())
            .sum();
        let walking = self.project_files.walking;
        let Some((candidates, _)) = self.prompts.last_mut().and_then(|p| p.completion.as_mut())
        else {
            return;
        };
        let total = candidates.total();
        if total + added > WORKER_PAST {
            if !walking || added.saturating_mul(GROWTH) >= total {
                self.leave_batches_for_worker();
            }
            return;
        }
        if added > MERGED_HERE {
            self.leave_batches_for_worker();
            return;
        }
        let batches = std::mem::take(&mut self.project_files.unmerged);
        // A batch nothing else holds gives its paths up, so none is copied.
        let paths = batches
            .into_iter()
            .flat_map(|mut batch| match Arc::get_mut(&mut batch) {
                Some(paths) => paths.iter_mut().map(std::mem::take).collect::<Vec<_>>(),
                None => batch.to_vec(),
            });
        let history = self.project_files.history.iter().map(String::as_str);
        candidates.merge(paths.map(|path| Candidate::new(path, "", "")), history);
        self.keep_selection_in_view();
    }

    /// Leaves the batches that came, and the innermost prompt's answer now, for a job that adds
    /// them to its list. A job adding them already stops.
    pub(super) fn leave_batches_for_worker(&mut self) {
        let Some(prompt) = self.prompts.last() else {
            return;
        };
        let Some((candidates, _)) = &prompt.completion else {
            return;
        };
        let text = self
            .buffer(prompt.buffer)
            .filter(|buffer| buffer.text().len() <= MAX_INPUT_BYTES)
            .map(|buffer| buffer.text().to_string());
        let input = text
            .as_deref()
            .and_then(|text| matched_part(text, false))
            .map(str::to_owned);
        let matched = candidates.matched();
        let awaited = input != matched.input;
        let merge = Merge {
            items: Arc::clone(candidates.items()),
            order: Arc::clone(candidates.order()),
            matched,
            batches: self.project_files.unmerged.clone(),
            history: Arc::clone(&self.project_files.history),
            input,
        };
        let revision = self.prompt_revision().unwrap_or_default();
        self.leave_merge(merge, revision, awaited);
    }

    /// Drops the batches a job added, the oldest `carried` of those waiting, on the worker.
    pub(super) fn drop_merged_batches(&mut self, carried: usize) {
        let carried = carried.min(self.project_files.unmerged.len());
        let merged: Vec<_> = self.project_files.unmerged.drain(..carried).collect();
        self.free_on_the_worker(Box::new(merged));
    }

    /// Drops batches of paths, on the worker if they are many.
    fn free_unmerged(&mut self, batches: Vec<Arc<[Box<str>]>>) {
        let paths: usize = batches.iter().map(|batch| batch.len()).sum();
        if paths > WORKER_PAST {
            self.free_on_the_worker(Box::new(batches));
        }
    }

    /// Forgets the prompt that listed a project's files, once it has closed: its walk stops
    /// at its next directory.
    pub(super) fn forget_project_files(&mut self, closed: PromptId) {
        if self.project_files.prompt == Some(closed) {
            self.project_files.live.fetch_add(1, Ordering::Relaxed);
            let live = Arc::clone(&self.project_files.live);
            let list = std::mem::replace(
                &mut self.project_files,
                ProjectFiles {
                    live,
                    ..ProjectFiles::default()
                },
            );
            self.free_unmerged(list.unmerged);
        }
    }
}

#[cfg(test)]
mod tests;
