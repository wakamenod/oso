//! Migemo in isearch and jumps (ADR 0116): the `[search] migemo` setting, `M-m`, and the
//! needles of the last romaji strings, so `DEL`, a repeat and `C-s C-s` compile nothing.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! The cache holds strings as the history does, so a string typed around a buffer that may
//! hold secrets stays out of it, and `Debug` prints none of them (ADR 0044).

use std::collections::VecDeque;
use std::fmt;

use super::Editor;
use crate::migemo;
use crate::search::{Needle, Query};

/// How many strings' needles the cache keeps.
const CACHED: usize = 32;

/// The setting and the cache.
pub(in crate::editor) struct Migemo {
    /// Whether a search starts with migemo on.
    on: bool,
    /// A string, whether it is case-sensitive, and its needle, `None` if it gives no kana:
    /// the newest last.
    cache: VecDeque<(String, bool, Option<Needle>)>,
}

impl Default for Migemo {
    fn default() -> Self {
        Self {
            on: true,
            cache: VecDeque::new(),
        }
    }
}

impl fmt::Debug for Migemo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Migemo")
            .field("on", &self.on)
            .finish_non_exhaustive()
    }
}

impl Migemo {
    /// Migemo's needle for `string`, with the case `case_sensitive` says, `None` if it is not
    /// romaji that gives kana. A needle built is kept if `keep`.
    pub(in crate::editor) fn needle(
        &mut self,
        string: &str,
        case_sensitive: bool,
        keep: bool,
    ) -> Option<Needle> {
        if !migemo::is_romaji(string) {
            return None;
        }
        let at = self
            .cache
            .iter()
            .position(|(cached, case, _)| cached == string && *case == case_sensitive);
        if let Some(entry) = at.and_then(|at| self.cache.remove(at)) {
            let needle = entry.2.clone();
            self.cache.push_back(entry);
            return needle;
        }
        let needle = Needle::migemo(string, case_sensitive);
        if keep {
            if self.cache.len() == CACHED {
                self.cache.pop_front();
            }
            self.cache
                .push_back((string.to_owned(), case_sensitive, needle.clone()));
        }
        needle
    }

    /// How many strings' needles it keeps.
    #[cfg(test)]
    pub(in crate::editor) fn cached(&self) -> usize {
        self.cache.len()
    }
}

impl Editor {
    /// Whether a search starts with migemo on, as `[search] migemo` says (ADR 0116). A search
    /// under way keeps what it has.
    pub fn set_migemo(&mut self, on: bool) {
        self.migemo.on = on;
    }

    /// Whether a search starts with migemo on.
    pub(in crate::editor) fn migemo_on(&self) -> bool {
        self.migemo.on
    }

    /// `M-m`: turns migemo off or on for the rest of this search, and searches again from
    /// the match. In a regexp's search it shows once `M-r` makes the string literal.
    pub fn isearch_toggle_migemo(&mut self) {
        self.isearch_follow_text();
        let cursor = self.primary_cursor();
        let Some(state) = &mut self.isearch else {
            return;
        };
        state.push_step(cursor);
        state.migemo = !state.migemo;
        state.rebuild_query(&mut self.migemo);
        self.isearch_typed();
    }

    /// What a jump's `string` looks for (ADR 0116): migemo's needle if `migemo` is on and
    /// the string is romaji that gives kana, else the literal string, with smart case.
    /// `None` for an empty string.
    pub(in crate::editor) fn needle_for(&mut self, string: &str, migemo: bool) -> Option<Needle> {
        let case_sensitive = crate::search::smart_case(string);
        let keep = !self.shows_secret();
        migemo
            .then(|| self.migemo.needle(string, case_sensitive, keep))
            .flatten()
            .or_else(|| Query::new(string, case_sensitive).map(Needle::Literal))
    }
}
