//! What the person sent from native sessions' composers (ADR 0165, step 2, "History"): one
//! history for all of them, the newest first, which `M-p` walks and `C-c C-r` lists. `file`
//! keeps it.
//!
//! Must not depend on: threads, the UI or the disk, but `file`.
//!
//! - Each text is in it once, at its newest; at most [`MAX_ENTRIES`] of them and [`MAX_TEXT`]
//!   bytes of text, the oldest going first.
//! - Each entry has a stamp, in milliseconds since the epoch, later than every one this
//!   instance knows, so a place in a walk is a stamp and another send does not move it.
//! - What was sent up to the last clear is not in it, whichever instance sent it.
//! - `Debug` gives counts and stamps, never a text.

pub mod file;

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

/// The most messages it keeps.
pub const MAX_ENTRIES: usize = 200;
/// The most bytes of text it keeps.
pub const MAX_TEXT: usize = 512 * 1024;

/// A message, and when it was sent.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    pub text: Arc<str>,
    pub sent: u64,
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("bytes", &self.text.len())
            .field("sent", &self.sent)
            .finish()
    }
}

/// Where a step of a walk comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
    Entry(&'a Entry),
    /// Past the newest: what the walk started from.
    Start,
}

/// The messages, the newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Entry>,
    /// What was sent up to then is gone.
    cleared: u64,
    /// The newest stamp given here.
    last: u64,
}

impl History {
    /// The messages, the newest first.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What was sent up to then is gone.
    pub fn cleared(&self) -> u64 {
        self.cleared
    }

    /// A stamp for `now`, later than every one known.
    fn stamp(&mut self, now: u64) -> u64 {
        let known = (self.entries.first())
            .map_or(0, |entry| entry.sent)
            .max(self.cleared)
            .max(self.last);
        self.last = now.max(known.saturating_add(1));
        self.last
    }

    /// Notes that the person sent `text` at `now`.
    pub fn note(&mut self, text: &str, now: u64) {
        let sent = self.stamp(now);
        self.entries.insert(
            0,
            Entry {
                text: text.into(),
                sent,
            },
        );
        self.settle();
    }

    /// Takes every message out, as of `now`, and gives how many there were.
    pub fn clear(&mut self, now: u64) -> usize {
        self.cleared = self.stamp(now);
        std::mem::take(&mut self.entries).len()
    }

    /// Takes in what another instance, or the file, has. A stamp after `now`, as of a clock set
    /// back, counts as `now`.
    pub fn merge(&mut self, other: &Self, now: u64) {
        self.cleared = self.cleared.max(other.cleared.min(now));
        let theirs = other.entries.iter().map(|entry| Entry {
            text: Arc::clone(&entry.text),
            sent: entry.sent.min(now),
        });
        self.entries.extend(theirs);
        self.settle();
    }

    /// Sorts the entries and drops those that may not stay, as the module says.
    fn settle(&mut self) {
        let cleared = self.cleared;
        self.entries.retain(|entry| entry.sent > cleared);
        // Stable: of two at once, the one first here stays first.
        self.entries.sort_by_key(|entry| Reverse(entry.sent));
        let mut seen = HashSet::new();
        self.entries
            .retain(|entry| seen.insert(Arc::clone(&entry.text)));
        self.entries.truncate(MAX_ENTRIES);
        let mut bytes = 0;
        let fits = (self.entries.iter())
            .take_while(|entry| {
                bytes += entry.text.len();
                bytes <= MAX_TEXT
            })
            .count();
        self.entries.truncate(fits);
    }

    /// The message `count` before the one sent at `from`, after it if negative; from past the
    /// newest with none. The oldest is the last; with no message, only `Start`.
    pub fn step(&self, from: Option<u64>, count: isize) -> Step<'_> {
        // Its place, or where it was if it is gone, as one sent again is.
        let (at, there) = match from {
            None => (0, false),
            Some(sent) => {
                let at = self.entries.partition_point(|entry| entry.sent > sent);
                (at, self.entries.get(at).is_some_and(|e| e.sent == sent))
            }
        };
        // Past the newest, or at one gone, the next older is at `at` itself.
        let older = match there {
            false if count > 0 => count - 1,
            _ => count,
        };
        match at.checked_add_signed(older) {
            Some(to) => {
                let to = to.min(self.entries.len().wrapping_sub(1));
                match self.entries.get(to) {
                    Some(entry) if count != 0 || there => Step::Entry(entry),
                    _ => Step::Start,
                }
            }
            None => Step::Start,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(sent: &[(&str, u64)]) -> History {
        let mut history = History::default();
        for (text, now) in sent {
            history.note(text, *now);
        }
        history
    }

    fn texts(history: &History) -> Vec<&str> {
        (history.entries().iter()).map(|e| &*e.text).collect()
    }

    /// The text `count` steps from `from`, or `-` for the walk's start.
    fn step(history: &History, from: Option<u64>, count: isize) -> &str {
        match history.step(from, count) {
            Step::Entry(entry) => &entry.text,
            Step::Start => "-",
        }
    }

    #[test]
    fn a_text_sent_again_is_once_at_its_newest() {
        let history = history(&[("a", 1), ("b", 2), ("a", 3)]);
        assert_eq!(texts(&history), ["a", "b"]);
        assert_eq!(history.entries()[0].sent, 3);
    }

    #[test]
    fn stamps_grow_though_the_clock_stands_or_goes_back() {
        let history = history(&[("a", 5), ("b", 5), ("c", 1)]);
        assert_eq!(texts(&history), ["c", "b", "a"]);
        let sent: Vec<u64> = history.entries().iter().map(|e| e.sent).collect();
        assert_eq!(sent, [7, 6, 5]);
    }

    #[test]
    fn the_oldest_go_past_the_most_entries_or_bytes() {
        let mut many = History::default();
        for n in 0..MAX_ENTRIES + 5 {
            many.note(&n.to_string(), n as u64);
        }
        assert_eq!(many.entries().len(), MAX_ENTRIES);
        assert_eq!(&*many.entries()[MAX_ENTRIES - 1].text, "5");
        let mut large = History::default();
        for n in 0..12 {
            large.note(&format!("{}{n:02}", "0".repeat(MAX_TEXT / 8 - 2)), n);
        }
        assert_eq!(large.entries().len(), 8);
        assert!(large.entries()[0].text.ends_with("11"));
    }

    #[test]
    fn a_clear_takes_out_what_another_instance_sent_before_it_and_no_later() {
        let mut ours = history(&[("a", 1), ("b", 4)]);
        assert_eq!(ours.clear(6), 2);
        assert!(ours.is_empty());
        ours.merge(&history(&[("a", 1), ("theirs", 5), ("later", 9)]), 20);
        assert_eq!(texts(&ours), ["later"]);
        // And the other takes the clear in.
        let mut theirs = history(&[("old", 3), ("new", 8)]);
        theirs.merge(&ours, 20);
        assert_eq!(texts(&theirs), ["later", "new"]);
        // What is sent right after a clear stays.
        ours.note("c", 6);
        assert_eq!(texts(&ours), ["c", "later"]);
    }

    #[test]
    fn a_merge_keeps_each_text_at_its_newest_and_takes_no_future() {
        let mut ours = history(&[("a", 1), ("b", 4)]);
        ours.merge(&history(&[("c", 3), ("a", 5), ("far", u64::MAX - 1)]), 10);
        assert_eq!(texts(&ours), ["far", "a", "b", "c"]);
        assert_eq!(ours.entries()[0].sent, 10);
        ours.note("now", 11);
        assert_eq!(texts(&ours)[0], "now");
        // A clear from the future is one now.
        let mut other = History::default();
        other.clear(u64::MAX - 1);
        ours.merge(&other, 10);
        assert_eq!(texts(&ours), ["now"]);
    }

    #[test]
    fn a_walk_goes_to_the_oldest_and_back_to_its_start() {
        let history = history(&[("a", 1), ("b", 2), ("c", 3)]);
        assert_eq!(step(&history, None, 1), "c");
        assert_eq!(step(&history, None, 2), "b");
        assert_eq!(step(&history, Some(3), 1), "b");
        assert_eq!(step(&history, Some(1), 1), "a");
        assert_eq!(step(&history, None, 99), "a");
        assert_eq!(step(&history, Some(2), -1), "c");
        assert_eq!(step(&history, Some(3), -1), "-");
        assert_eq!(step(&history, Some(1), -99), "-");
        assert_eq!(step(&history, None, -1), "-");
        assert_eq!(step(&history, None, isize::MIN), "-");
        assert_eq!(step(&history, None, isize::MAX), "a");
        assert_eq!(step(&history, None, 0), "-");
        assert_eq!(step(&history, Some(2), 0), "b");
        assert_eq!(step(&History::default(), None, 1), "-");
        assert_eq!(step(&History::default(), Some(2), 3), "-");
    }

    #[test]
    fn a_walk_goes_on_from_where_an_entry_was() {
        let mut history = history(&[("a", 1), ("b", 2), ("c", 3)]);
        // The one shown was sent again elsewhere, and another was too.
        history.note("b", 4);
        assert_eq!(step(&history, Some(2), 1), "a");
        assert_eq!(step(&history, Some(2), -1), "c");
        assert_eq!(step(&history, Some(2), -2), "b");
        assert_eq!(step(&history, Some(2), -3), "-");
    }

    #[test]
    fn debug_says_no_text() {
        let said = format!("{:?}", history(&[("private words", 1)]));
        assert!(!said.contains("private"), "{said}");
    }
}
