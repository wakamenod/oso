//! The prompt that reads a path, for `C-x C-f` and `C-x C-w` (ADR 0087): what is typed planned
//! as segments, the directories the host must list, the candidates once they are, and the path
//! an answer stands for.
//!
//! Must not depend on: `command` or `keymap`, or the host's window, threads or I/O; the host
//! lists directories and says what it found.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::Editor;
use super::completion::CompletionStyle;
use crate::buffer::{BufferId, Replacement};
use crate::complete::path::{self, Found, Name, NameKind, Plan, Reach, Reached};
use crate::complete::{Build, Built, Candidate, WORKER_PAST, listed, matched_part};
use crate::minibuffer::{PromptError, PromptId};

/// What the host found of a path a row shows, for its annotation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Looked {
    /// What the row says after the name, as the host wrote it: mode, size, age, where a link
    /// leads.
    pub annotation: String,
    /// Its size, which decides whether opening it asks first.
    pub bytes: u64,
    /// It is a directory, or a link to one, which `RET` goes into.
    pub directory: bool,
}

/// The most candidates a prompt that reads a path lists, as a listing keeps of one directory.
pub const MAX_FOUND: usize = 100_000;

/// What a prompt that reads a path keeps beside its candidates.
#[derive(Debug, Default)]
pub(super) struct FileList {
    prompt: Option<PromptId>,
    /// The directory a relative path starts from.
    base: PathBuf,
    /// The plan of the text matched last.
    plan: Option<Plan>,
    /// The names in each directory listed, or `None` for one that could not be read.
    listings: HashMap<PathBuf, Option<Arc<[Name]>>>,
    /// The directories asked for and not answered yet.
    asked: HashSet<PathBuf>,
    /// The directories the plan needs and nobody has asked for.
    wanted: Vec<PathBuf>,
    /// The directories the plan waits for, asked or not: once the last comes, it is expanded
    /// again, and not before, so that listings arriving one by one cost one expansion.
    waiting: Vec<PathBuf>,
    /// A segment matched more directories than are kept, or a listing or the list was cut.
    cut: bool,
    /// A listing kept fewer names than its directory has.
    listing_cut: bool,
    /// The candidates shown, in the order of their items.
    found: Vec<Found>,
    /// What a look found of each path, for its row's annotation and its size (ADR 0087).
    looks: HashMap<PathBuf, Looked>,
    /// The paths asked to be looked at and not answered yet.
    looking: HashSet<PathBuf>,
    /// The directories the prompt started in and those that hold it, which exist, so a segment
    /// naming one whole needs no listing of the one above (ADR 0087).
    known: Vec<PathBuf>,
    /// The answer must be a directory, as `C-u` asks for a project's (ADR 0114).
    wants_directory: bool,
}

impl Editor {
    /// Opens a prompt that reads a directory's path, as [`Editor::open_file_prompt`] reads a
    /// path, for a command that walks it (ADR 0114). `RET` on a file's row says it is not a
    /// directory and keeps the prompt open, unless what is typed ends with `/`, which gives that
    /// directory.
    ///
    /// # Errors
    ///
    /// As [`Editor::open_file_prompt`].
    pub fn open_directory_prompt(
        &mut self,
        label: &str,
        history: &'static str,
        initial: &str,
        base: PathBuf,
    ) -> Result<PromptId, PromptError> {
        let id = self.open_file_prompt(label, history, initial, base)?;
        self.file_list.wants_directory = true;
        Ok(id)
    }

    /// Opens a prompt that says `label` and reads a path, kept in the history `history`,
    /// starting with `initial`, such as the buffer's directory with the home directory as `~/`.
    /// Relative paths start from `base`. It takes any text, as a new file is one (ADR 0087).
    ///
    /// # Errors
    ///
    /// [`PromptError::TooDeep`] with as many prompts open as may be. Nothing changes.
    pub fn open_file_prompt(
        &mut self,
        label: &str,
        history: &'static str,
        initial: &str,
        base: PathBuf,
    ) -> Result<PromptId, PromptError> {
        let style = CompletionStyle {
            given_order: false,
            takes_input: true,
            preview: false,
        };
        let id = self.open_completion_as(label, history, Arc::from([]), style)?;
        if let Some(prompt) = self.prompts.last_mut()
            && let Some((candidates, _)) = prompt.completion.as_mut()
        {
            candidates.match_last_segment();
        }
        let start = path::as_typed(&path::plan(initial, self.home.as_deref(), &base));
        self.file_list = FileList {
            prompt: Some(id),
            known: start.ancestors().map(Path::to_path_buf).collect(),
            base,
            ..FileList::default()
        };
        if let Some(prompt) = self.prompts.last() {
            let buffer = prompt.buffer;
            let edit = Replacement {
                range: 0..0,
                text: initial,
            };
            let _ = self.apply_edits(buffer, &[edit]);
            let end = initial.len();
            let window = self.selected;
            self.windows[window].selections = crate::selection::SelectionSet::new(end);
        }
        self.refresh_files();
        Ok(id)
    }

    /// Whether the innermost prompt reads a path.
    pub(super) fn reads_path(&self) -> bool {
        self.file_list
            .prompt
            .is_some_and(|id| self.prompts.last().is_some_and(|prompt| prompt.id == id))
    }

    /// The directories the innermost prompt that reads a path needs listed, which the host
    /// lists and answers with [`Editor::file_listed`]. Each is given once.
    pub fn take_file_requests(&mut self) -> Vec<PathBuf> {
        if !self.reads_path() {
            return Vec::new();
        }
        let wanted = std::mem::take(&mut self.file_list.wanted);
        self.file_list.asked.extend(wanted.iter().cloned());
        wanted
    }

    /// Whether the innermost prompt that reads a path left candidates out: past the limits of a
    /// listing, of a segment's directories or of the list, or of a project's walk or search
    /// (ADR 0114). The count says so with `+`.
    pub fn files_cut(&self) -> bool {
        (self.reads_path() && self.file_list.cut) || self.project_files_cut() || self.search_cut()
    }

    /// Whether a prompt that reads a path is open, innermost or not.
    pub fn file_prompt_open(&self) -> bool {
        self.file_list.prompt.is_some()
    }

    /// Whether the innermost prompt that reads a path waits for a listing: of a directory, or
    /// of a project whose walk has not ended or whose rows are not all in its list, or for a
    /// project's search that runs (ADR 0114).
    pub fn files_pending(&self) -> bool {
        (self.reads_path() && !self.file_list.asked.is_empty())
            || self.project_files_pending()
            || self.search_pending()
    }

    /// The directory the innermost prompt's path reaches, for the control socket, if it reads
    /// a path.
    pub fn file_directory(&self) -> Option<&Path> {
        self.file_list
            .plan
            .as_ref()
            .filter(|_| self.reads_path())
            .map(|plan| plan.root.as_path())
    }

    /// Takes in what listing `directory` found: its names, or `None` if it could not be read,
    /// which the prompt then says. Nothing happens once the prompt that asked has closed.
    pub fn file_listed(&mut self, directory: PathBuf, names: Option<Arc<[Name]>>, truncated: bool) {
        if self.file_list.prompt.is_none() {
            return;
        }
        self.file_list.asked.remove(&directory);
        if names.is_none() && self.reads_path() {
            self.prompt_says("Cannot list it");
        }
        self.file_list.listing_cut |= truncated;
        let list = &mut self.file_list;
        let awaited = list.waiting.iter().position(|other| *other == directory);
        list.listings.insert(directory, names);
        // One that this plan does not wait for, or one of several it waits for, is only kept.
        if let Some(index) = awaited {
            list.waiting.swap_remove(index);
            if list.waiting.is_empty() {
                list.plan = None;
                self.refresh_files();
            }
        }
    }

    /// Plans what the innermost prompt that reads a path holds, if that changed or a listing
    /// came, and gives its candidates the names found, or asks for the directories it needs.
    pub(super) fn refresh_files(&mut self) {
        if !self.reads_path() {
            return;
        }
        let Some(text) = self
            .prompts
            .last()
            .and_then(|prompt| self.buffer(prompt.buffer))
            .map(|buffer| buffer.text().to_string())
        else {
            return;
        };
        let plan = path::plan(&text, self.home.as_deref(), &self.file_list.base);
        let same = self.file_list.plan.as_ref().is_some_and(|old| {
            old.root == plan.root
                && old.segments == plan.segments
                && old.last.starts_with('.') == plan.last.starts_with('.')
        });
        if same {
            return;
        }
        let list = &self.file_list;
        let reach = path::reach(
            &plan,
            &|directory| {
                list.listings
                    .get(directory)
                    .map(|names| names.as_deref().unwrap_or(&[]))
            },
            |directory| list.known.iter().any(|known| known == directory),
        );
        let hidden = plan.last.starts_with('.');
        self.file_list.plan = Some(plan);
        match reach {
            Reach::Need(directories) => {
                // A list built for the path before is not this one's.
                self.stop_completion_job();
                let list = &mut self.file_list;
                list.waiting.clone_from(&directories);
                for directory in directories {
                    if !list.asked.contains(&directory) && !list.wanted.contains(&directory) {
                        list.wanted.push(directory);
                    }
                }
            }
            Reach::Ready(reached, cut) => {
                self.file_list.waiting.clear();
                let build = self.file_build(reached, hidden, &text);
                let revision = self.prompt_revision().unwrap_or_default();
                // How many names it will have is known before it is made (ADR 0096).
                if build.size() > WORKER_PAST {
                    self.leave_build(build, revision, cut);
                } else {
                    // A list that is not built on a worker is no job's to stop.
                    self.stop_completion_job();
                    if let Some(built) = build.run(&|| false) {
                        self.install_files(built, cut, revision);
                    }
                }
            }
        }
    }

    /// The list of the directories `reached`, for the innermost prompt's answer `text`.
    fn file_build(&self, reached: Vec<Reached>, hidden: bool, text: &str) -> Build {
        let prompt = self.prompts.last();
        let history = prompt
            .and_then(crate::minibuffer::Prompt::history_name)
            .unwrap_or("");
        let records = prompt.is_some_and(|prompt| !prompt.around_secret);
        let history = if records {
            self.histories.entries(history).map(str::to_owned).collect()
        } else {
            Vec::new()
        };
        let keep = self.selected_file().map(|(found, _)| found.path.clone());
        let listings = &self.file_list.listings;
        Build {
            reached: listed(reached, |directory| {
                listings.get(directory).cloned().flatten()
            }),
            hidden,
            most: MAX_FOUND,
            keep,
            history,
            input: matched_part(text, true).map(str::to_owned),
        }
    }

    /// Makes the list `built` the innermost prompt's candidates, as matched for its answer at
    /// `revision`: each a name, matched by the last segment, with the directories that tell rows
    /// of one name apart beside it. The row selected stays selected, by its path.
    pub(super) fn install_files(&mut self, built: Built, cut: bool, revision: u64) {
        self.file_list.cut = cut || built.more || self.file_list.listing_cut;
        let Some((candidates, seen)) = self
            .prompts
            .last_mut()
            .and_then(|prompt| prompt.completion.as_mut())
        else {
            return;
        };
        let on_input = candidates.on_input();
        let next = candidates.rebuilt(built.items, built.order, built.matched);
        let before = std::mem::replace(candidates, next);
        *seen = revision;
        if let Some(place) = built.kept.and_then(|index| candidates.place_of(index))
            && place > 0
            && !on_input
        {
            candidates.select(place);
        }
        let found = std::mem::replace(&mut self.file_list.found, built.found);
        // The list before is dropped on the worker when it is long (ADR 0096).
        if before.total() > WORKER_PAST {
            self.free_on_the_worker(Box::new((before, found)));
        }
        self.keep_selection_in_view();
    }

    /// Shows on the rows in view what looks found of them, in place of their candidates, unless
    /// they show it already: the list's order and what matches stay (ADR 0096).
    pub(super) fn patch_looks(&mut self) {
        if !self.reads_path() {
            return;
        }
        let rows = self.candidate_rows();
        let list = &self.file_list;
        let Some((candidates, _)) = self
            .prompts
            .last_mut()
            .and_then(|prompt| prompt.completion.as_mut())
        else {
            return;
        };
        let top = candidates.top();
        for place in top..(top + rows).min(candidates.len()) {
            let Some(index) = candidates.item_at(place) else {
                continue;
            };
            if candidates.is_patched(index) {
                continue;
            }
            let Some(found) = list.found.get(index) else {
                continue;
            };
            if let Some(looked) = list.looks.get(&found.path) {
                candidates.patch(index, row(found, looked));
            }
        }
    }

    /// The path the innermost prompt's selected candidate stands for, with whether it is a
    /// directory, if it reads a path and a candidate is selected.
    pub(super) fn selected_file(&self) -> Option<(&Found, String)> {
        if !self.reads_path() {
            return None;
        }
        let candidates = self.prompts.last()?.candidates()?;
        let index = candidates.item_at(candidates.selected()?)?;
        let found = self.file_list.found.get(index)?;
        let looked = self.file_list.looks.get(&found.path);
        let directory = found.directory || looked.is_some_and(|looked| looked.directory);
        let typed = path::typed_form(&found.path, directory, self.home.as_deref());
        Some((found, typed))
    }

    /// The path that what is typed names as it is, for an answer that is no candidate.
    pub(super) fn typed_file(&self) -> Option<PathBuf> {
        let text = self
            .prompts
            .last()
            .and_then(|prompt| self.buffer(prompt.buffer))?
            .text()
            .to_string();
        Some(path::as_typed(&path::plan(
            &text,
            self.home.as_deref(),
            &self.file_list.base,
        )))
    }

    /// What `RET`, or `M-RET` with `typed`, gives in a prompt that reads a path, if the innermost
    /// one does: a selected directory is gone into, as there is no directory editor, a selected
    /// file gives its whole path, and what is typed gives the path it names as it is.
    pub(super) fn submit_path(&mut self, typed: bool) -> Option<Result<(), PromptError>> {
        if !self.reads_path() {
            return None;
        }
        let on_input = self
            .prompt()
            .and_then(crate::minibuffer::Prompt::candidates)
            .is_some_and(crate::complete::Candidates::on_input);
        // A path typed with `/` at its end names a directory, which `C-x C-w` writes into.
        let slash = self
            .prompts
            .last()
            .and_then(|prompt| self.buffer(prompt.buffer))
            .is_some_and(|buffer| buffer.text().to_string().ends_with('/'));
        let wants_directory = self.file_list.wants_directory;
        // What is typed is given, not the selected row: `M-RET`, the input selected, or a
        // directory typed for a prompt that wants one.
        let as_typed = typed || on_input || (wants_directory && slash);
        if !as_typed && let Some((found, shown)) = self.selected_file() {
            let looked = self.file_list.looks.get(&found.path);
            if found.directory || looked.is_some_and(|looked| looked.directory) {
                return Some(self.insert_completion());
            }
            if wants_directory {
                self.prompt_says(format!("Not a directory: {shown}"));
                return Some(Err(PromptError::Invalid));
            }
            // A look that found 2 GiB or more makes the command ask at once (ADR 0087).
            let bytes = looked.map_or(0, |looked| looked.bytes);
            let path = found.path.clone();
            if bytes >= crate::load::LARGE {
                self.remember_large(path.clone(), bytes);
            }
            return Some(self.give_answer(path.display().to_string()));
        }
        let path = self.typed_file()?;
        // A file typed whole, or given with `M-RET`, is no directory either, where a listing of
        // its own directory says so.
        if wants_directory && !slash && self.listed_as_file(&path) {
            let shown = path::typed_form(&path, false, self.home.as_deref());
            self.prompt_says(format!("Not a directory: {shown}"));
            return Some(Err(PromptError::Invalid));
        }
        let mut answer = path.display().to_string();
        if slash && !answer.ends_with('/') {
            answer.push('/');
        }
        Some(self.give_answer(answer))
    }

    /// Whether the listing of the directory that holds `path` names it as something other than a
    /// directory or a link. Unlisted, it is not known to be one.
    fn listed_as_file(&self, path: &Path) -> bool {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return false;
        };
        let Some(Some(names)) = self.file_list.listings.get(parent) else {
            return false;
        };
        names.iter().any(|listed| {
            *listed.name == *name && !matches!(listed.kind, NameKind::Directory | NameKind::Link)
        })
    }

    /// The directory oso started in, which a buffer with no file starts a path from.
    pub fn start_directory(&self) -> &Path {
        &self.start_directory
    }

    /// Tells the editor the directory oso started in, read once by the host at startup.
    pub fn set_start_directory(&mut self, directory: PathBuf) {
        self.start_directory = directory;
    }

    /// The rows shown that nobody has looked at yet, by directory, for the host to look at and
    /// answer with [`Editor::file_looked`]. Each is given once.
    pub fn take_file_looks(&mut self) -> Vec<(PathBuf, Vec<String>)> {
        if !self.reads_path() {
            return Vec::new();
        }
        let rows = self.candidate_rows();
        let Some(candidates) = self.prompts.last().and_then(|prompt| prompt.candidates()) else {
            return Vec::new();
        };
        let list = &self.file_list;
        let mut asked: Vec<(PathBuf, Vec<String>)> = Vec::new();
        let mut paths = Vec::new();
        let top = candidates.top();
        for place in top..(top + rows).min(candidates.len()) {
            let Some(found) = candidates
                .item_at(place)
                .and_then(|index| list.found.get(index))
            else {
                continue;
            };
            if list.looks.contains_key(&found.path) || list.looking.contains(&found.path) {
                continue;
            }
            let (Some(directory), Some(name)) = (found.path.parent(), found.path.file_name())
            else {
                continue;
            };
            let name = name.to_string_lossy().into_owned();
            match asked.iter_mut().find(|(other, _)| other == directory) {
                Some((_, names)) => names.push(name),
                None => asked.push((directory.to_path_buf(), vec![name])),
            }
            paths.push(found.path.clone());
        }
        self.file_list.looking.extend(paths);
        asked
    }

    /// Takes in what looking at the names in `directory` found, and shows it on their rows.
    pub fn file_looked(&mut self, directory: &Path, looks: Vec<(String, Looked)>) {
        if self.file_list.prompt.is_none() {
            return;
        }
        for (name, looked) in looks {
            let path = directory.join(name);
            self.file_list.looking.remove(&path);
            self.file_list.looks.insert(path, looked);
        }
        self.patch_looks();
    }

    /// Another buffer that visits the very file `id` loaded, by its device and inode, as a link
    /// or a second spelling of an open file leads to it (ADR 0087). A buffer the agent channel
    /// is still opening is neither, as the channel checks what it may show before it shows
    /// anything (ADR 0085); once shown, it is like any other.
    pub fn visiting_same_file(&self, id: BufferId) -> Option<BufferId> {
        let buffer = self.buffer(id).filter(|buffer| !buffer.agent_pending())?;
        let identity = buffer.baseline()?.file?.identity?;
        self.buffers().map(|(other, _)| other).find(|other| {
            *other != id
                && self
                    .buffer(*other)
                    .filter(|buffer| !buffer.agent_pending())
                    .and_then(|buffer| buffer.baseline()?.file?.identity)
                    == Some(identity)
        })
    }

    /// Forgets the prompt that read a path, once it has closed.
    pub(super) fn forget_files(&mut self, closed: PromptId) {
        if self.file_list.prompt == Some(closed) {
            let list = std::mem::take(&mut self.file_list);
            // Its 100,000 paths are dropped on the worker (ADR 0096).
            if list.found.len() > WORKER_PAST {
                self.free_on_the_worker(Box::new(list));
            }
        }
    }

    /// Forgets the plan of the prompt `id` that reads a path, so that it is planned and its
    /// list built again.
    pub(super) fn forget_file_plan(&mut self, id: PromptId) {
        if self.file_list.prompt == Some(id) {
            self.file_list.plan = None;
        }
    }
}

/// The row of `found` once a look found `looked` of it.
fn row(found: &Found, looked: &Looked) -> Candidate {
    let note = if found.context.is_empty() {
        looked.annotation.clone()
    } else {
        format!("{}  {}", found.context, looked.annotation)
    };
    // A link that leads to a directory reads as one.
    let name = if looked.directory && !found.name.ends_with('/') {
        format!("{}/", found.name)
    } else {
        found.name.clone()
    };
    Candidate::new(name, "", note)
}

#[cfg(test)]
mod tests;
