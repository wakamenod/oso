//! Migemo on the editor (ADR 0116), for isearch and jumps: the `[search] migemo` setting,
//! what a string looks for, migemo's needle or the literal string, and the needles of the
//! last romaji strings, so `DEL`, a repeat and `C-s C-s` compile no pattern again.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! The cache holds strings as the history does, so a string typed around a buffer that may
//! hold secrets stays out of it, and `Debug` prints none of them (ADR 0044). A cached needle
//! is a clone, whose regexps for a cut a backward search may still build on a key, as for
//! any regexp a key compiled quickly.

use std::collections::VecDeque;
use std::fmt;

use super::Editor;
use crate::migemo;
use crate::search::{self, Needle, Query};

/// How many strings' needles the cache keeps.
const CACHED: usize = 32;

/// The setting and the cache.
pub(super) struct Migemo {
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
    /// What `string` looks for with the case `case_sensitive` says: migemo's needle if `on`
    /// and the string is romaji that gives kana, with `true`, else the literal string, with
    /// `false`. `None` for an empty string. A migemo needle built is kept if `keep`.
    pub(super) fn string_needle(
        &mut self,
        string: &str,
        case_sensitive: bool,
        (on, keep): (bool, bool),
    ) -> Option<(Needle, bool)> {
        if on && let Some(needle) = self.needle(string, case_sensitive, keep) {
            return Some((needle, true));
        }
        Query::new(string, case_sensitive).map(|query| (Needle::Literal(query), false))
    }

    /// Migemo's needle for `string`, `None` if it is not romaji that gives kana, from the
    /// cache, or built and kept if `keep`.
    fn needle(&mut self, string: &str, case_sensitive: bool, keep: bool) -> Option<Needle> {
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
    pub(super) fn cached(&self) -> usize {
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
    pub(super) fn migemo_on(&self) -> bool {
        self.migemo.on
    }

    /// What a jump's `string` looks for (ADR 0116): migemo's needle if the setting is on and
    /// the string is romaji that gives kana, else the literal string, with smart case. `None`
    /// for an empty string. The needle is kept unless a buffer that may hold secrets shows
    /// now or showed while the jump went on, as `around_secret` says.
    pub(super) fn needle_for(&mut self, string: &str, around_secret: bool) -> Option<Needle> {
        let case_sensitive = search::smart_case(string);
        let keep = !around_secret && !self.shows_secret();
        let on = self.migemo.on;
        self.migemo
            .string_needle(string, case_sensitive, (on, keep))
            .map(|(needle, _)| needle)
    }
}
