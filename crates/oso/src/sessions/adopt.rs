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
//!   transcript's; text from either is escaped before it is shown, without the `·` the
//!   annotations are split by.
//! - No two candidates share a text, whatever names the files hold: a candidate stands for
//!   one conversation only.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;

use oso_core::complete::Candidate;
use oso_core::editor::row_text;
use oso_platform::process;

use super::identity::SessionId;
use super::list::age_of;
use super::registry::{self, Live as Liveness, shown_directory};
use super::transcripts::{self, Cache, Conversation};

/// The most registry names kept that this oso saw for conversations, by id.
pub const MAX_REGISTRY_NAMES: usize = 256;

/// The longest name a candidate shows, in bytes.
const MAX_SHOWN_NAME: usize = 64;

/// What a name never shows: the dot the annotations are split by, and dots that look like it.
const ANNOTATION_DOTS: [char; 4] = ['·', '\u{0387}', '\u{2219}', '\u{22c5}'];

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

/// Lists on a thread of its own and calls `send` with what it found: nothing, with an empty
/// cache, if the list panicked, so that a later list starts afresh.
///
/// # Errors
///
/// If no thread starts: the order, with its cache, is dropped.
pub fn spawn(order: Order, send: impl FnOnce(Found) + Send + 'static) -> io::Result<()> {
    thread::Builder::new()
        .name("oso-session-adopt-list".into())
        .spawn(move || {
            let found =
                panic::catch_unwind(AssertUnwindSafe(|| find(order))).unwrap_or_else(|_| {
                    log::warn!("listing the sessions to adopt panicked; listing none");
                    Found::default()
                });
            send(found);
        })
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
pub struct RegistryNames {
    by_id: HashMap<SessionId, (String, bool)>,
    order: VecDeque<SessionId>,
}

impl RegistryNames {
    /// Conversation `id` was seen in the registry, named `name`, busy or not.
    pub fn note(&mut self, id: &SessionId, name: Option<&str>, busy: bool) {
        let name = name.unwrap_or_default().to_owned();
        if self.by_id.insert(id.clone(), (name, busy)).is_none() {
            self.order.push_back(id.clone());
            if self.order.len() > MAX_REGISTRY_NAMES
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

/// What the editor's `session-adopt` list stands for, and a newer list that waits while a
/// prompt shows the older: an answer is looked up in the list its prompt showed.
#[derive(Debug, Default)]
pub struct Offered {
    picks: HashMap<String, Pick>,
    waiting: Option<Choices>,
}

impl Offered {
    /// A new list was made: the candidates to give the editor now, or none while `shown`, as a
    /// prompt shows the older list then.
    pub fn list(&mut self, choices: Choices, shown: bool) -> Option<Arc<[Candidate]>> {
        if shown {
            self.waiting = Some(choices);
            return None;
        }
        self.waiting = None;
        self.picks = choices.picks;
        Some(choices.candidates)
    }

    /// Whether a list waits for no prompt to show the older.
    pub fn waiting(&self) -> bool {
        self.waiting.is_some()
    }

    /// No prompt shows the list now: the waiting list's candidates to give the editor, if one
    /// waits.
    pub fn unshown(&mut self) -> Option<Arc<[Candidate]>> {
        let choices = self.waiting.take()?;
        self.picks = choices.picks;
        Some(choices.candidates)
    }

    /// What the candidate `text` of the list the editor has stands for.
    pub fn pick(&self, text: &str) -> Option<&Pick> {
        self.picks.get(text)
    }
}

/// The choices of `found`, live ones first, then stopped ones, newest first, with the names
/// `names` kept. A name that two share, or none, gets the id's first 8 digits, and the whole
/// id if that is another's text too; a conversation listed twice is offered once.
pub fn choices(found: &Found, names: &RegistryNames, now: SystemTime) -> Choices {
    let mut rows = rows(found, names, now);
    let mut ids = HashSet::new();
    rows.retain(|(pick, _)| ids.insert(pick.id.clone()));
    let names: Vec<String> = rows
        .iter()
        .map(|(pick, _)| pick.name.as_deref().map(shown_name).unwrap_or_default())
        .collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in names.iter().filter(|name| !name.is_empty()) {
        *counts.entry(name.as_str()).or_default() += 1;
    }
    let mut choices = Choices::default();
    let mut candidates = Vec::with_capacity(rows.len());
    for ((pick, annotation), name) in rows.into_iter().zip(&names) {
        let id = pick.id.as_str();
        let mut texts = Vec::with_capacity(3);
        let prefix = if name.is_empty() {
            pick.directory
                .as_deref()
                .and_then(Path::file_name)
                .map(|place| shown_name(&place.to_string_lossy()))
                .unwrap_or_default()
        } else {
            if counts.get(name.as_str()) == Some(&1) {
                texts.push(name.clone());
            }
            name.clone()
        };
        texts.push(format!("{prefix} {}", &id[..8]).trim_start().to_owned());
        texts.push(format!("{prefix} {id}").trim_start().to_owned());
        // Not one another candidate has, nor another's name, which that one may have as is.
        let text = texts.into_iter().find(|text| {
            !choices.picks.contains_key(text)
                && (text == name || !counts.contains_key(text.as_str()))
        });
        let Some(text) = text else {
            log::debug!("session {id} is not offered: no text of its own is left");
            continue;
        };
        candidates.push(Candidate::new(text.as_str(), "", annotation));
        choices.picks.insert(text, pick);
    }
    debug_assert_eq!(choices.picks.len(), candidates.len());
    choices.candidates = candidates.into();
    choices
}

/// What each of `found` stands for and its annotation, live ones first, then stopped ones.
fn rows(found: &Found, names: &RegistryNames, now: SystemTime) -> Vec<(Pick, String)> {
    let mut rows = Vec::new();
    for running in &found.running {
        let seen = names.get(&running.id);
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
        let seen = names.get(&stopped.id);
        let name = seen
            .and_then(|(name, _)| name.map(str::to_owned))
            .or_else(|| stopped.title.clone());
        let ago = now.duration_since(stopped.changed).unwrap_or_default();
        let annotation = format!(
            "{} ago · {}",
            age_of(ago),
            shown_directory(&stopped.directory)
        );
        let pick = Pick {
            id: stopped.id.clone(),
            directory: Some(stopped.directory.clone()),
            name,
            running: None,
            busy: seen.is_some_and(|(_, busy)| busy),
        };
        rows.push((pick, annotation));
    }
    rows
}

/// The deepest of `roots` that holds `directory`, by its id: the open project a session there
/// belongs to. `directory` is compared by its text, so it must hold no `..`
/// ([`registry::is_plain_directory`]).
pub fn holding<'a, Id: Copy>(
    directory: &Path,
    roots: impl IntoIterator<Item = (Id, &'a Path)>,
) -> Option<Id> {
    roots
        .into_iter()
        .filter(|(_, root)| directory.starts_with(root))
        .max_by_key(|(_, root)| root.components().count())
        .map(|(id, _)| id)
}

/// A name from a file any process of the person's may write, on one row, without the dots
/// the annotations are split by, cut short.
fn shown_name(name: &str) -> String {
    let mut shown = row_text(name.trim()).replace(ANNOTATION_DOTS, "");
    if shown.len() > MAX_SHOWN_NAME {
        shown.truncate(shown.floor_char_boundary(MAX_SHOWN_NAME));
        shown.push('…');
    }
    shown
}

#[cfg(test)]
#[path = "adopt_tests.rs"]
mod tests;
