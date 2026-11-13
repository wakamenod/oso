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
    /// The name the registry gives it, if any.
    pub name: Option<String>,
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
    /// Starts looking at `dir`, or stops with `None`.
    pub fn set(&mut self, dir: Option<PathBuf>, now: Instant) {
        self.next = dir.as_ref().map(|_| now);
        self.dir = dir;
        self.stamps.clear();
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
            name: entry.name.clone(),
            directory: entry.cwd.clone(),
            working: entry.status.as_deref() == Some("busy"),
            session_id: entry.session_id.clone(),
        })
        .collect();
    live.sort_by_key(|external| external.pid);
    live
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
        let _ = std::fs::remove_dir_all(dir);
    }
}
