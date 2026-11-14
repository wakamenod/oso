//! Other programs' sessions (ADR 0121 step 3): once a second while the app asks, the worker
//! looks at Claude Code's registry, and reads it again with the registry's reader only when a
//! `<pid>.json` file changed. Live entries go to the app when the set changes; the app leaves
//! out its own sessions.
//!
//! Must not depend on: `app` or `dispatch`. The instants come from the caller.
//!
//! Only names the reader opens are looked at: `<digits>.json`, never `*.key`, at most
//! [`registry::MAX_FILES`] of them. A look is one listing, a `stat` of each, and the start
//! time of each process read, as a process that ends leaves its file behind.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::sessions::identity::SessionId;
use crate::sessions::recap;
use crate::sessions::registry::{self, Live};

/// The most names of the registry's directory looked at, entries or not.
const MAX_LISTED: usize = 4 * registry::MAX_FILES;

/// How often the registry is looked at.
pub const POLL_EVERY: Duration = Duration::from_secs(1);

/// A live session of Claude Code's registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct External {
    pub pid: u32,
    pub session_id: Option<SessionId>,
    pub name: String,
    pub directory: Option<PathBuf>,
    /// The CLI says it is busy.
    pub working: bool,
}

/// The poll's state.
#[derive(Debug, Default)]
pub struct Poll {
    /// The registry's directory, while the app asks for the poll.
    dir: Option<PathBuf>,
    next: Option<Instant>,
    /// Each `<pid>.json` file's name and change time at the last look.
    stamps: Vec<(OsString, Option<SystemTime>)>,
    /// The entries read, by file name: whether each process lives is asked each look, and a
    /// file is read again only when it changed.
    entries: HashMap<OsString, registry::Entry>,
    /// What went to the app last.
    told: Option<Vec<External>>,
}

impl Poll {
    /// Starts looking at `dir`, or stops with `None`. What was read and told is forgotten, so
    /// that a poll started again tells the app what it finds, as the app forgot it.
    pub fn set(&mut self, dir: Option<PathBuf>, now: Instant) {
        self.next = dir.as_ref().map(|_| now);
        self.dir = dir;
        self.stamps.clear();
        self.entries.clear();
        self.told = None;
    }

    pub fn due(&self) -> Option<Instant> {
        self.next
    }

    /// Looks at the registry if due at `now`: the live sessions, if they changed since told.
    pub fn tick(&mut self, now: Instant) -> Option<Vec<External>> {
        let (Some(dir), Some(next)) = (&self.dir, self.next) else {
            return None;
        };
        if now < next {
            return None;
        }
        self.next = Some(now + POLL_EVERY);
        let stamps = stamps(dir);
        if stamps != self.stamps || self.told.is_none() {
            let before: HashMap<&OsString, &Option<SystemTime>> =
                self.stamps.iter().map(|(name, at)| (name, at)).collect();
            let mut entries = HashMap::new();
            for (name, at) in &stamps {
                let kept = (before.get(name) == Some(&at) && self.told.is_some())
                    .then(|| self.entries.remove(name))
                    .flatten();
                if let Some(entry) = kept.or_else(|| registry::read_file(dir, name)) {
                    entries.insert(name.clone(), entry);
                }
            }
            self.entries = entries;
            self.stamps = stamps;
        }
        let live = live(self.entries.values());
        if self.told.as_ref() == Some(&live) {
            return None;
        }
        self.told = Some(live.clone());
        Some(live)
    }
}

/// Each file of the registry the reader would open, with when it changed, in name order. At
/// most [`MAX_LISTED`] names are looked at, whatever they are.
fn stamps(dir: &Path) -> Vec<(OsString, Option<SystemTime>)> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stamps: Vec<_> = listing
        .take(MAX_LISTED)
        .filter_map(Result::ok)
        .filter(|entry| registry::is_entry_name(&entry.file_name()))
        .take(registry::MAX_FILES)
        .map(|entry| {
            let changed = entry.metadata().and_then(|meta| meta.modified()).ok();
            (entry.file_name(), changed)
        })
        .collect();
    stamps.sort();
    stamps
}

/// The entries whose process still runs, by process id.
fn live<'a>(entries: impl Iterator<Item = &'a registry::Entry>) -> Vec<External> {
    let started = oso_platform::process::start_time;
    let mut live: Vec<External> = entries
        .filter(|entry| registry::liveness(entry, started) == Live::Yes)
        .map(|entry| External {
            pid: entry.pid,
            name: shown_name(entry),
            directory: entry.cwd.clone(),
            working: entry.status.as_deref() == Some("busy"),
            session_id: entry.session_id.clone(),
        })
        .collect();
    live.sort_by_key(|external| external.pid);
    live
}

/// The name `*Sessions*` lists for `entry`. It is the registry's text, so it is cleaned as a
/// recap is, that it cannot reorder or break the line it is drawn on.
/// Characters that draw nothing, or a blank, are dropped too: a name of them alone would show
/// as no name, and among others they would make a copy of another session's name.
fn shown_name(entry: &registry::Entry) -> String {
    entry
        .name
        .as_deref()
        .map(|name| {
            let drawn: String = name.chars().filter(|&c| !draws_nothing(c)).collect();
            recap::clean(&drawn)
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("claude {}", entry.pid))
}

/// Whether `c` draws nothing, or a blank that is not white space.
fn draws_nothing(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
    )
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::Poll;

    #[test]
    fn nothing_is_looked_at_until_asked_and_an_empty_registry_is_told_once() {
        let dir = crate::review::tests::scratch_dir("externals-poll");
        let now = Instant::now();
        let mut poll = Poll::default();
        assert_eq!(poll.due(), None);
        assert_eq!(poll.tick(now), None);
        poll.set(Some(dir.clone()), now);
        assert_eq!(poll.tick(now), Some(Vec::new()));
        assert_eq!(poll.due(), Some(now + super::POLL_EVERY));
        // Too soon, then nothing changed.
        assert_eq!(poll.tick(now + Duration::from_millis(10)), None);
        assert_eq!(poll.tick(now + super::POLL_EVERY), None);
        // A file that is not an entry is never looked at.
        std::fs::write(dir.join("1.key"), b"secret").unwrap();
        assert_eq!(poll.tick(now + super::POLL_EVERY * 2), None);
        // An entry of a process that does not run is not live.
        std::fs::write(
            dir.join("999999.json"),
            br#"{"pid":999999,"startedAt":1,"status":"busy"}"#,
        )
        .unwrap();
        assert_eq!(poll.tick(now + super::POLL_EVERY * 3), None);
        poll.set(None, now);
        assert_eq!(poll.due(), None);
        // Started again, it tells what it finds, though that did not change.
        poll.set(Some(dir.clone()), now);
        assert_eq!(poll.tick(now), Some(Vec::new()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_name_is_listed_clean_on_one_line() {
        let named = |name: Option<&str>| crate::sessions::registry::Entry {
            pid: 42,
            name: name.map(str::to_owned),
            ..Default::default()
        };
        let shown = super::shown_name(&named(Some("fix\u{202E}txt.exe\tnow\nx")));
        assert_eq!(shown, "fixtxt.exe now x");
        assert_eq!(super::shown_name(&named(Some("\u{2066}\t"))), "claude 42");
        assert_eq!(super::shown_name(&named(None)), "claude 42");
        let blank = named(Some("\u{200B}\u{2800}\u{3164}\u{FEFF}"));
        assert_eq!(super::shown_name(&blank), "claude 42");
        assert_eq!(super::shown_name(&named(Some("fi\u{200B}x"))), "fix");
    }
}
