//! What `session-adopt` offers (ADR 0121, "Adopting a session started outside oso"): the
//! conversations of Claude Code that oso does not host, live ones from the registry and stopped
//! ones from the transcripts, found on a short `oso-session-adopt-list` thread, and the
//! candidates and annotations the prompt completes over.
//!
//! Must not depend on: `app`, `dispatch` or the UI thread; the caller gives an [`Order`] and
//! takes what was [`Found`] through the function it gives.
//!
//! - A live process cannot move between programs, so a live conversation is listed with its
//!   process, to be stopped where it runs first; the start thread refuses it while it runs.
//! - Ids oso hosts, and ids another oso instance remembers, are never offered.
//! - A name is the one the registry gave while it ran, as this oso saw it, then the
//!   transcript's; text from either is escaped before it is shown.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime};

use oso_core::complete::Candidate;
use oso_core::editor::row_text;
use oso_platform::process;

use super::identity::SessionId;
use super::registry::{self, Live as Liveness};
use super::transcripts::{self, Cache, Conversation};

/// The most registry names kept that this oso saw for conversations, by id.
pub const MAX_SEEN: usize = 256;

/// The longest name a candidate shows, in bytes.
const MAX_SHOWN_NAME: usize = 64;

/// The longest directory a candidate shows, in bytes.
const MAX_SHOWN_DIRECTORY: usize = 200;

/// A live conversation of the registry's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub pid: u32,
    pub id: SessionId,
    pub name: Option<String>,
    pub directory: Option<PathBuf>,
    /// The CLI says it is busy.
    pub busy: bool,
}

/// What the thread is asked.
#[derive(Debug)]
pub struct Order {
    /// What earlier lists read of the transcripts.
    pub cache: Cache,
    /// The ids never offered: oso's own and another instance's.
    pub left_out: HashSet<SessionId>,
    pub now: SystemTime,
}

/// What the thread found, and the cache for the next list.
#[derive(Debug, Default)]
pub struct Found {
    pub running: Vec<Running>,
    pub stopped: Vec<Conversation>,
    pub cache: Cache,
}

/// Lists on a thread of its own and calls `send` with what it found.
///
/// # Errors
///
/// If no thread starts: the order, with its cache, is dropped.
pub fn spawn(order: Order, send: impl FnOnce(Found) + Send + 'static) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-session-adopt-list".into())
        .spawn(move || send(find(order)))
        .map(drop)
}

/// The live and stopped conversations `order` does not leave out.
fn find(order: Order) -> Found {
    let Order {
        mut cache,
        mut left_out,
        now,
    } = order;
    let entries = registry::directory()
        .map(|dir| registry::list(&dir).entries)
        .unwrap_or_default();
    let running = running(entries, &left_out, process::start_time);
    // A live one is listed as live, not again as stopped.
    left_out.extend(running.iter().map(|running| running.id.clone()));
    let stopped = transcripts::directory()
        .map(|dir| transcripts::list(&dir, now, |id| left_out.contains(id), &mut cache))
        .unwrap_or_default();
    Found {
        running,
        stopped,
        cache,
    }
}

/// The entries that may run, by `started`, and are not left out, by process id. One whose
/// start cannot be told counts as running, as a resume would refuse it.
fn running(
    entries: Vec<registry::Entry>,
    left_out: &HashSet<SessionId>,
    started: impl Fn(u32) -> io::Result<Option<SystemTime>>,
) -> Vec<Running> {
    let mut running: Vec<Running> = entries
        .into_iter()
        .filter(|entry| registry::liveness(entry, &started) != Liveness::No)
        .filter_map(|entry| {
            let id = entry.session_id?;
            (!left_out.contains(&id)).then(|| Running {
                pid: entry.pid,
                id,
                name: entry.name,
                directory: entry.cwd,
                busy: entry.status.as_deref() == Some("busy"),
            })
        })
        .collect();
    running.sort_by_key(|running| running.pid);
    running
}

/// The registry's names this oso saw for conversations, and whether each was busy last, so
/// that one stopped elsewhere keeps the name it had while it ran.
#[derive(Debug, Default)]
pub struct Seen {
    by_id: HashMap<SessionId, (String, bool)>,
    order: VecDeque<SessionId>,
}

impl Seen {
    /// Conversation `id` was seen in the registry, named `name`, busy or not.
    pub fn note(&mut self, id: &SessionId, name: Option<&str>, busy: bool) {
        let name = name.unwrap_or_default().to_owned();
        if self.by_id.insert(id.clone(), (name, busy)).is_none() {
            self.order.push_back(id.clone());
            if self.order.len() > MAX_SEEN
                && let Some(oldest) = self.order.pop_front()
            {
                self.by_id.remove(&oldest);
            }
        }
    }

    /// The name seen for `id`, if not empty, and whether it was busy last.
    pub fn get(&self, id: &SessionId) -> Option<(Option<&str>, bool)> {
        self.by_id
            .get(id)
            .map(|(name, busy)| (Some(name.as_str()).filter(|name| !name.is_empty()), *busy))
    }
}

/// What a candidate stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub id: SessionId,
    /// The directory it runs or ran in, if known.
    pub directory: Option<PathBuf>,
    /// Its name as found, not cleaned.
    pub name: Option<String>,
    /// The process that runs it, if live.
    pub running: Option<u32>,
    /// The registry said it was busy, the last time this oso saw it.
    pub busy: bool,
}

/// The candidates `session-adopt` completes over, and what each stands for, by its text.
#[derive(Debug, Default)]
pub struct Choices {
    pub candidates: Arc<[Candidate]>,
    pub picks: HashMap<String, Pick>,
}

/// The choices of `found`, live ones first, then stopped ones, newest first, with the names
/// `seen` kept. A name that two share, or none, gets the id's first 8 digits.
pub fn choices(found: &Found, seen: &Seen, now: SystemTime) -> Choices {
    let mut rows: Vec<(Pick, String)> = Vec::new();
    for running in &found.running {
        let seen = seen.get(&running.id);
        let name = running
            .name
            .clone()
            .or_else(|| seen.and_then(|(name, _)| name.map(str::to_owned)));
        let place = running.directory.as_deref().map(shown_directory);
        let annotation = format!(
            "runs as process {}: stop it there first{}",
            running.pid,
            place.map(|place| format!(" · {place}")).unwrap_or_default()
        );
        let pick = Pick {
            id: running.id.clone(),
            directory: running.directory.clone(),
            name,
            running: Some(running.pid),
            busy: running.busy,
        };
        rows.push((pick, annotation));
    }
    for stopped in &found.stopped {
        let seen = seen.get(&stopped.id);
        let name = seen
            .and_then(|(name, _)| name.map(str::to_owned))
            .or_else(|| stopped.title.clone());
        let ago = now.duration_since(stopped.changed).unwrap_or_default();
        let annotation = format!("{} ago · {}", age(ago), shown_directory(&stopped.directory));
        let pick = Pick {
            id: stopped.id.clone(),
            directory: Some(stopped.directory.clone()),
            name,
            running: None,
            busy: seen.is_some_and(|(_, busy)| busy),
        };
        rows.push((pick, annotation));
    }
    let texts: Vec<String> = rows
        .iter()
        .map(|(pick, _)| pick.name.as_deref().map(shown_name).unwrap_or_default())
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for text in &texts {
        *counts.entry(text.as_str()).or_default() += 1;
    }
    let mut choices = Choices::default();
    let mut candidates = Vec::with_capacity(rows.len());
    for ((pick, annotation), text) in rows.into_iter().zip(&texts) {
        let short = &pick.id.as_str()[..8];
        let text = if text.is_empty() {
            let place = pick
                .directory
                .as_deref()
                .and_then(Path::file_name)
                .map(|name| shown_name(&name.to_string_lossy()))
                .unwrap_or_default();
            format!("{place} {short}").trim_start().to_owned()
        } else if counts.get(text.as_str()).copied().unwrap_or(0) > 1 {
            format!("{text} {short}")
        } else {
            text.clone()
        };
        candidates.push(Candidate::new(text.as_str(), "", annotation));
        choices.picks.insert(text, pick);
    }
    choices.candidates = candidates.into();
    choices
}

/// An age as the prompt shows it: `12s`, `5m`, `3h`, `2d`.
fn age(ago: Duration) -> String {
    let seconds = ago.as_secs();
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// Text from a file any process of the person's may write, on one row, cut short.
fn shown(text: &str, most: usize) -> String {
    let mut shown = row_text(text.trim());
    if shown.len() > most {
        shown.truncate(shown.floor_char_boundary(most));
        shown.push('…');
    }
    shown
}

fn shown_name(name: &str) -> String {
    shown(name, MAX_SHOWN_NAME)
}

fn shown_directory(directory: &Path) -> String {
    shown(&directory.to_string_lossy(), MAX_SHOWN_DIRECTORY)
}

#[cfg(test)]
#[path = "adopt_tests.rs"]
mod tests;
