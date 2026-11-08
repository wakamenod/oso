//! The minibuffer's histories (ADR 0072): the answers given to each, newest first.
//!
//! Must not depend on: `editor` or prompts; a history holds strings by name.

use std::collections::VecDeque;

use super::{HISTORY_LENGTH, MAX_ENTRY_BYTES};

/// The answers given to each history, newest first, in memory only: never written to
/// disk (ADR 0013, ADR 0051). Only `oso dev` carries them to the instance that replaces
/// this one (ADR 0090).
#[derive(Default)]
pub(crate) struct Histories {
    pub(crate) lists: Vec<(String, VecDeque<String>)>,
}

impl Histories {
    /// Records `text` as the newest of `name`. An answer given before moves to the front,
    /// and the oldest goes past [`HISTORY_LENGTH`]. Nothing empty or longer than
    /// [`MAX_ENTRY_BYTES`] is recorded.
    pub(crate) fn record(&mut self, name: &str, text: &str) {
        if text.is_empty() || text.len() > MAX_ENTRY_BYTES {
            return;
        }
        let list = self.list_mut(name);
        list.retain(|entry| entry != text);
        list.push_front(text.to_owned());
        list.truncate(HISTORY_LENGTH);
    }

    /// Puts `older` behind what `name` holds, the newest first, as far as there is room, and
    /// leaves out what it holds already and what [`Histories::record`] would not keep.
    pub(crate) fn extend_older(&mut self, name: &str, older: impl IntoIterator<Item = String>) {
        let list = self.list_mut(name);
        for text in older {
            if list.len() >= HISTORY_LENGTH {
                break;
            }
            if !text.is_empty() && text.len() <= MAX_ENTRY_BYTES && !list.contains(&text) {
                list.push_back(text);
            }
        }
    }

    fn list_mut(&mut self, name: &str) -> &mut VecDeque<String> {
        let index = match self.lists.iter().position(|(other, _)| other == name) {
            Some(index) => index,
            None => {
                self.lists.push((name.to_owned(), VecDeque::new()));
                self.lists.len() - 1
            }
        };
        &mut self.lists[index].1
    }

    /// Each history's name and answers, the newest first.
    pub(crate) fn all(&self) -> impl Iterator<Item = (&str, impl Iterator<Item = &str>)> {
        self.lists
            .iter()
            .map(|(name, list)| (name.as_str(), list.iter().map(String::as_str)))
    }

    /// The answers of `name`, newest first.
    pub(crate) fn entries(&self, name: &str) -> impl Iterator<Item = &str> {
        self.lists
            .iter()
            .filter(move |(other, _)| other == name)
            .flat_map(|(_, list)| list.iter().map(String::as_str))
    }

    /// The answer `index` steps back in `name`, 0 the newest.
    pub(crate) fn get(&self, name: &str, index: usize) -> Option<&str> {
        self.entries(name).nth(index)
    }
}
