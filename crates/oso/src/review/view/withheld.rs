//! Which files that leave a review's listing show their comments' count alone (ADR 0122, "The
//! views"): as a present file withheld shows none of its cards, one gone does not show them if
//! a reading of it was withheld or a name it had in the view may hold secrets (ADR 0044), read
//! or not; once so, always so for the view's life.
//!
//! Must not depend on: `app`, `dispatch`, git's runner or the file system; it uses git's name
//! check only.

use std::collections::HashSet;
use std::path::Path;

use super::file::File;
use crate::git::path_verdict;
use crate::review::Entry;

/// The files withheld so far, by path.
#[derive(Default)]
pub(super) struct Held(HashSet<Box<[u8]>>);

impl Held {
    /// The paths of a listing's `entries`, in order, noting each with a name, its path or the
    /// one it came from, that may hold secrets under `top`.
    pub(super) fn listed(&mut self, top: &Path, entries: &[Entry]) -> Vec<Box<[u8]>> {
        let mut paths = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.verdict(top).is_withheld() {
                self.0.insert(entry.path.clone());
            }
            paths.push(entry.path.clone());
        }
        paths
    }

    /// Whether `file`, leaving the listing, is withheld: its reading, or the one waiting, was,
    /// or a name it had under `top` may hold secrets, now or before.
    pub(super) fn left(&mut self, top: &Path, file: &File) -> bool {
        let path = &file.entry.path;
        let withheld =
            file.withheld() || file.entry.verdict(top).is_withheld() || self.0.contains(path);
        if withheld {
            self.0.insert(path.clone());
        }
        withheld
    }

    /// Whether the file at `path`, not listed and told of its comments, is withheld: it was, or
    /// its name under `top` may hold secrets, a name that is no path here included. No reading
    /// of it may be known.
    pub(super) fn told(&mut self, top: &Path, path: &[u8]) -> bool {
        let withheld = self.0.contains(path) || path_verdict(top, path).is_withheld();
        if withheld {
            self.0.insert(path.into());
        }
        withheld
    }
}
