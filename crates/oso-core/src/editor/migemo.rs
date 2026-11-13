//! Migemo on the editor (ADR 0116), for isearch and jumps: the `[search] migemo` setting,
//! the dictionary the host read, what a string looks for, migemo's needle or the literal
//! string, and the needles of the last romaji strings, so `DEL`, a repeat and `C-s C-s`
//! compile no pattern again.
//!
//! Must not depend on: `command` or `keymap`, or the host's window or threads.
//!
//! The cache holds strings as the history does, so a string typed around a buffer that may
//! hold secrets stays out of it, and `Debug` prints none of them (ADR 0044). A cached needle
//! is a clone, which needs no regexps for a cut. A string looked up around a secret leaves
//! the cache's order as it was. A dictionary that comes or goes empties it, and the search on
//! screen looks again.
//!
//! The editor reads no file: it says when a search would have used a dictionary it does not
//! have ([`Editor::take_migemo_dictionary_wanted`]), and the host reads the one the person
//! named, off the UI thread, and gives it ([`Editor::set_migemo_dictionary`]).

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use super::Editor;
use crate::migemo::{self, Dictionary};
use crate::search::{self, Needle, Query};

/// How many strings' needles the cache keeps.
const CACHED: usize = 32;

/// The setting and the cache.
pub(super) struct Migemo {
    /// Whether a search starts with migemo on.
    starts_on: bool,
    /// The strings' needles, the newest last.
    cache: VecDeque<Entry>,
    /// The dictionary the host gave, if any.
    dictionary: Option<Arc<Dictionary>>,
    /// Whether migemo's needle was asked for with no dictionary since the host last looked,
    /// not around a buffer that may hold secrets.
    wanted: bool,
}

/// A string's needle in the cache.
struct Entry {
    string: String,
    case_sensitive: bool,
    needle: Needle,
}

/// What a string looks for.
pub(super) enum Sought {
    /// Migemo's needle, for romaji that gives kana.
    Migemo(Needle),
    /// The string as it is.
    Literal(Needle),
}

impl Sought {
    /// The needle, whichever it is.
    pub(super) fn into_needle(self) -> Needle {
        match self {
            Self::Migemo(needle) | Self::Literal(needle) => needle,
        }
    }
}

/// Whether a migemo needle built may go in the cache: not around a buffer that may hold
/// secrets, as the history keeps no string typed there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cache {
    Keep,
    Skip,
}

impl Cache {
    /// [`Cache::Skip`] around a secret, else [`Cache::Keep`].
    pub(super) fn unless(around_secret: bool) -> Self {
        if around_secret {
            Self::Skip
        } else {
            Self::Keep
        }
    }
}

impl Default for Migemo {
    fn default() -> Self {
        Self {
            starts_on: true,
            cache: VecDeque::new(),
            dictionary: None,
            wanted: false,
        }
    }
}

impl fmt::Debug for Migemo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Migemo")
            .field("starts_on", &self.starts_on)
            .field("dictionary", &self.dictionary.is_some())
            .finish_non_exhaustive()
    }
}

impl Migemo {
    /// What `string` looks for with the case `case_sensitive` says: migemo's needle if
    /// `use_migemo` and the string is romaji that gives kana, else the literal string. `None`
    /// for an empty string. A migemo needle built goes in the cache as `cache` says.
    pub(super) fn string_needle(
        &mut self,
        string: &str,
        case_sensitive: bool,
        use_migemo: bool,
        cache: Cache,
    ) -> Option<Sought> {
        if use_migemo && let Some(needle) = self.needle(string, case_sensitive, cache) {
            return Some(Sought::Migemo(needle));
        }
        Query::new(string, case_sensitive).map(|query| Sought::Literal(Needle::Literal(query)))
    }

    /// Migemo's needle for `string`, `None` if it is not romaji that gives kana, from the
    /// cache, or built and kept as `cache` says.
    fn needle(&mut self, string: &str, case_sensitive: bool, cache: Cache) -> Option<Needle> {
        if !migemo::is_romaji(string) {
            return None;
        }
        let at = (self.cache.iter())
            .position(|entry| entry.string == string && entry.case_sensitive == case_sensitive);
        // Not around a secret: the dictionary coming then would say what kind of string was
        // typed there.
        let wanted = self.wanted;
        self.wanted |= self.dictionary.is_none() && cache == Cache::Keep;
        if let Some(at) = at {
            // Around a secret, the order stays as it was, so it says nothing of the string.
            if cache == Cache::Skip {
                return Some(self.cache[at].needle.clone());
            }
            let entry = self.cache.remove(at)?;
            let needle = entry.needle.clone();
            self.cache.push_back(entry);
            return Some(needle);
        }
        // A string that gives no kana costs little to look at again, so it pushes no needle out.
        let Some(needle) = Needle::migemo(string, case_sensitive, self.dictionary.as_deref())
        else {
            // A string that gives no kana would use no dictionary.
            self.wanted = wanted;
            return None;
        };
        if cache == Cache::Keep {
            if self.cache.len() == CACHED {
                self.cache.pop_front();
            }
            self.cache.push_back(Entry {
                string: string.to_owned(),
                case_sensitive,
                needle: needle.clone(),
            });
        }
        Some(needle)
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
        self.migemo.starts_on = on;
    }

    /// Gives migemo the dictionary the person named, or takes it away with `None`
    /// (ADR 0116). The needles kept are dropped, and a search on screen, or a jump still
    /// reading its string, looks again with it.
    pub fn set_migemo_dictionary(&mut self, dictionary: Option<Arc<Dictionary>>) {
        self.migemo.dictionary = dictionary;
        self.migemo.cache.clear();
        self.isearch_look_again();
        self.jump_look_again();
    }

    /// Whether a search asked for migemo's needle with no dictionary since the last call, so
    /// the host reads the one the person named, if any. The next call says false until a
    /// search asks again.
    pub fn take_migemo_dictionary_wanted(&mut self) -> bool {
        std::mem::take(&mut self.migemo.wanted)
    }

    /// Whether a search starts with migemo on.
    pub(super) fn migemo_starts_on(&self) -> bool {
        self.migemo.starts_on
    }

    /// What a jump's `string` looks for (ADR 0116): migemo's needle if the setting is on and
    /// the string is romaji that gives kana, else the literal string, with smart case. `None`
    /// for an empty string. The needle is kept unless a buffer that may hold secrets shows
    /// now or showed while the jump went on, as `around_secret` says.
    pub(super) fn jump_needle(&mut self, string: &str, around_secret: bool) -> Option<Needle> {
        let case_sensitive = search::smart_case(string);
        let cache = Cache::unless(around_secret || self.shows_secret());
        let on = self.migemo.starts_on;
        (self.migemo)
            .string_needle(string, case_sensitive, on, cache)
            .map(Sought::into_needle)
    }
}
