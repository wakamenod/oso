//! What a prompt for a directory gives (ADR 0114): only what its listings and looks say is a
//! directory, and only once those that decide it have come.
//!
//! Must not depend on: `command` or `keymap`, or the host's window, threads or I/O; the host
//! lists directories and says what it found.

use std::path::{Path, PathBuf};

use crate::complete::path::{self, NameKind};
use crate::editor::Editor;

impl Editor {
    /// Why `path`, typed for a prompt that wants a directory, with `/` at its end if `slash`, is
    /// refused, if it is, by what the listings and looks the prompt has say (ADR 0114). Typed
    /// with `/`, its own listing must have been read; without, its directory's listing must name
    /// it as a directory, or a link a look found leads to one. What none of them tells, as a
    /// directory the prompt started in or a listing cut short, is given.
    pub(super) fn not_a_directory(&self, path: &Path, slash: bool) -> Option<String> {
        let list = &self.file_list;
        let home = self.home.as_deref();
        if slash {
            return matches!(list.listings.get(path), Some(None)).then(|| {
                let shown = path::typed_form(path, true, home);
                format!("Cannot read directory: {shown}")
            });
        }
        let (parent, name) = (path.parent()?, path.file_name()?);
        let names = match list.listings.get(parent)? {
            Some(names) => names,
            None => {
                let shown = path::typed_form(parent, true, home);
                return Some(format!("Cannot read directory: {shown}"));
            }
        };
        let shown = path::typed_form(path, false, home);
        match names.iter().find(|listed| *listed.name == *name) {
            Some(listed) => {
                let directory = match listed.kind {
                    NameKind::Directory => true,
                    NameKind::Link => list.looks.get(path).is_none_or(|looked| looked.directory),
                    NameKind::File | NameKind::Other => false,
                };
                (!directory).then(|| format!("Not a directory: {shown}"))
            }
            None if list.listing_cut => None,
            None => Some(format!("No such directory: {shown}")),
        }
    }

    /// Whether `RET` or `M-RET` in the innermost prompt, if it reads a directory, waits for what
    /// decides its answer: a listing its plan needs, or a look at the link selected or the link
    /// typed, asked for or about to be, as [`Editor::take_file_looks`] asks for each. The host
    /// holds them until it has come, as it holds `RET` for a worker (ADR 0096, ADR 0114).
    pub fn directory_answer_waits(&self) -> bool {
        if !self.reads_path() || !self.file_list.wants_directory {
            return false;
        }
        let list = &self.file_list;
        if !list.waiting.is_empty() {
            return true;
        }
        // Its rows are directories and links: a link is decided by its look.
        self.selected_found()
            .is_some_and(|found| !found.directory && !list.looks.contains_key(&found.path))
            || self.typed_link_unlooked().is_some()
    }

    /// The link that what is typed names whole, without `/` at its end, in a prompt that reads a
    /// directory, if its directory's listing names it as a link and no look at it has come:
    /// what decides `M-RET`'s answer, or `RET`'s on the input.
    pub(super) fn typed_link_unlooked(&self) -> Option<PathBuf> {
        let list = &self.file_list;
        if !self.reads_path() || !list.wants_directory {
            return None;
        }
        let prompt = self.prompts.last()?;
        if self
            .buffer(prompt.buffer)?
            .text()
            .to_string()
            .ends_with('/')
        {
            return None;
        }
        let path = self.typed_file()?;
        let (parent, name) = (path.parent()?, path.file_name()?);
        let names = list.listings.get(parent)?.as_ref()?;
        let link = names
            .iter()
            .any(|listed| *listed.name == *name && listed.kind == NameKind::Link);
        (link && !list.looks.contains_key(&path)).then_some(path)
    }
}
