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
use crate::buffer::Replacement;
use crate::complete::Candidate;
use crate::complete::path::{self, Expanded, Found, Name, Plan};
use crate::minibuffer::{PromptError, PromptId};

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
}

impl Editor {
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
        self.file_list = FileList {
            prompt: Some(id),
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
    /// listing, of a segment's directories or of the list. The count says so with `+`.
    pub fn files_cut(&self) -> bool {
        self.reads_path() && self.file_list.cut
    }

    /// Whether a prompt that reads a path is open, innermost or not.
    pub fn file_prompt_open(&self) -> bool {
        self.file_list.prompt.is_some()
    }

    /// Whether the innermost prompt that reads a path waits for a listing.
    pub fn files_pending(&self) -> bool {
        self.reads_path() && !self.file_list.asked.is_empty()
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
        let expanded = path::expand(&plan, |directory| {
            list.listings
                .get(directory)
                .map(|names| names.as_deref().unwrap_or(&[]))
        });
        self.file_list.plan = Some(plan);
        match expanded {
            Expanded::Need(directories) => {
                let list = &mut self.file_list;
                list.waiting.clone_from(&directories);
                for directory in directories {
                    if !list.asked.contains(&directory) && !list.wanted.contains(&directory) {
                        list.wanted.push(directory);
                    }
                }
            }
            Expanded::Ready(mut found, cut) => {
                self.file_list.waiting.clear();
                // Matching runs on the UI thread, so the list is bounded until it runs on a
                // worker past 5000 (ADR 0076, ADR 0087).
                let more = found.len() > MAX_FOUND;
                found.truncate(MAX_FOUND);
                self.file_list.cut = cut || more || self.file_list.listing_cut;
                self.show_files(found);
            }
        }
    }

    /// Makes `found` the innermost prompt's candidates: each its name, matched by the last
    /// segment, with the directories that tell rows of one name apart beside it. The row
    /// selected stays selected, by its path.
    fn show_files(&mut self, found: Vec<Found>) {
        let items: Arc<[Candidate]> = found
            .iter()
            .map(|found| Candidate::new(found.name.as_str(), "", found.context.as_str()))
            .collect();
        let history = self
            .prompts
            .last()
            .and_then(|prompt| prompt.kind.history())
            .unwrap_or("");
        let records = self
            .prompts
            .last()
            .is_some_and(|prompt| !prompt.around_secret);
        let entries: Vec<String> = if records {
            self.histories.entries(history).map(str::to_owned).collect()
        } else {
            Vec::new()
        };
        let selected = self.selected_file().map(|(found, _)| found.path.clone());
        if let Some(prompt) = self.prompts.last_mut()
            && let Some((candidates, _)) = prompt.completion.as_mut()
        {
            let on_input = candidates.on_input();
            *candidates = candidates.with_items(items, entries.iter().map(String::as_str));
            let index = selected.and_then(|path| found.iter().position(|found| found.path == path));
            if let Some(place) = index.and_then(|index| candidates.place_of(index))
                && place > 0
                && !on_input
            {
                candidates.select(place);
            }
        }
        self.file_list.found = found;
        self.keep_selection_in_view();
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
        let typed = path::typed_form(&found.path, found.directory, self.home.as_deref());
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
        if !typed
            && !on_input
            && let Some((found, _)) = self.selected_file()
        {
            if found.directory {
                return Some(self.insert_completion());
            }
            let path = found.path.display().to_string();
            return Some(self.give_answer(path));
        }
        let path = self.typed_file()?;
        // A path typed with `/` at its end names a directory, which `C-x C-w` writes into.
        let text = self
            .prompts
            .last()
            .and_then(|prompt| self.buffer(prompt.buffer));
        let slash = text.is_some_and(|buffer| buffer.text().to_string().ends_with('/'));
        let mut answer = path.display().to_string();
        if slash && !answer.ends_with('/') {
            answer.push('/');
        }
        Some(self.give_answer(answer))
    }

    /// Asks the host to visit `path` in the selected window, reading at most `limit` bytes, as a
    /// command cannot start a load itself. A buffer that visits that path already is shown at
    /// once instead.
    pub fn request_visit(&mut self, path: PathBuf, limit: u64) {
        let open = self
            .buffers()
            .find(|(_, buffer)| buffer.path() == Some(path.as_path()))
            .map(|(id, _)| id);
        match open {
            Some(id) => self.show(id),
            None => self.visits.push((path, limit)),
        }
    }

    /// The visits commands asked for, for the host to start.
    pub fn take_visits(&mut self) -> Vec<(PathBuf, u64)> {
        std::mem::take(&mut self.visits)
    }

    /// Remembers that the file at `path` held `bytes` when a load stopped before reading it, so
    /// that the next `C-x C-f` of it asks (ADR 0087).
    pub fn remember_large(&mut self, path: PathBuf, bytes: u64) {
        self.large = Some((path, bytes));
    }

    /// The size the file at `path` had when a load stopped before it, if that is the one
    /// remembered.
    pub fn large_file(&self, path: &Path) -> Option<u64> {
        self.large
            .as_ref()
            .filter(|(large, _)| large == path)
            .map(|(_, bytes)| *bytes)
    }

    /// Takes the large file remembered, as a question about it is answered.
    pub fn take_large(&mut self) -> Option<(PathBuf, u64)> {
        self.large.take()
    }

    /// The directory oso started in, which a buffer with no file starts a path from.
    pub fn start_directory(&self) -> &Path {
        &self.start_directory
    }

    /// Tells the editor the directory oso started in, read once by the host at startup.
    pub fn set_start_directory(&mut self, directory: PathBuf) {
        self.start_directory = directory;
    }

    /// Forgets the prompt that read a path, once it has closed.
    pub(super) fn forget_files(&mut self, closed: PromptId) {
        if self.file_list.prompt == Some(closed) {
            self.file_list = FileList::default();
        }
    }
}

#[cfg(test)]
mod tests;
