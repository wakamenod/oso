//! The syntax worker's schedule (ADR 0111): one slot per buffer, the jobs the UI sends into
//! them, which document a thread works on next, and the reports that come of a slice. The
//! threads, their lock and their waking are the tip crate's; this is plain state, so that the
//! rules are tested without them.
//!
//! Must not depend on: threads, buffers, windows or faces; it sees jobs, documents and reports.
//!
//! - A thread checks a document out for one slice and gives it back. Jobs that come meanwhile
//!   wait in its slot, so the UI never waits on a slice.
//! - Buffers a window shows go first. At most one checkout at a time is of a buffer no window
//!   shows, so one slice that runs long on a background buffer never holds the shown ones.
//! - A dropped buffer's slot goes at once: a job that waited in it never runs, and a slice that
//!   was running gives back nothing, as each slot has a generation of its own.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::{Highlights, Splice, compose};

use crate::document::{Document, Failure, Highlighted, MAX_WAITING};
use crate::language::Language;
use crate::queries::Queries;

/// How long one slice takes at most, about.
pub const SLICE: Duration = Duration::from_millis(20);

/// How often a buffer whose shown text is done reports while the rest is highlighted.
const REPORT_EVERY: Duration = Duration::from_millis(50);

/// What the UI sends after a buffer changed, or what its windows show did.
#[derive(Clone, Debug)]
pub struct Job<Key> {
    /// Which buffer: the tip crate's id for it.
    pub buffer: Key,
    pub language: Language,
    /// The buffer's revision, whose text `text` is.
    pub revision: u64,
    /// An O(1) snapshot of the text.
    pub text: Rope,
    /// The splices since the last job for this buffer, or `None` to start from nothing.
    pub edits: Option<Vec<Splice>>,
    /// The byte ranges its windows show, to highlight first.
    pub shown: Vec<Range<usize>>,
}

/// What a slice came to, for the UI.
#[derive(Clone, Debug)]
pub struct Report<Key> {
    pub buffer: Key,
    /// The revision whose text the highlights are for.
    pub revision: u64,
    pub outcome: Outcome,
}

/// New highlights, or the end of highlighting in a buffer.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// A snapshot of the highlights, O(1) to clone.
    Highlights(Highlights),
    /// The buffer is no longer highlighted, and its slot is gone.
    Failed(Failure),
}

/// A document out of its slot for one slice.
pub struct Checkout<Key> {
    buffer: Key,
    generation: u64,
    document: Document,
    shown: Vec<Range<usize>>,
    /// Whether no window shows it.
    background: bool,
}

impl<Key> Checkout<Key> {
    /// Runs one slice. A thread calls it with its own queries, and nothing locked.
    pub fn run(&mut self, queries: &mut Queries) -> Highlighted {
        self.document.highlight(queries, &self.shown, SLICE)
    }
}

/// Jobs that came while a document was checked out.
enum Waiting {
    Nothing,
    /// One batch since the document's text, and the text after it.
    Edits {
        batch: Vec<Splice>,
        text: Rope,
        revision: u64,
    },
    /// Start from nothing on this text.
    Reset {
        text: Rope,
        revision: u64,
    },
}

struct Slot {
    generation: u64,
    language: Language,
    /// None while a thread has it.
    document: Option<Document>,
    /// The revision whose text the document follows.
    revision: u64,
    waiting: Waiting,
    shown: Vec<Range<usize>>,
    /// Its place in the queue, if it has work: lower goes first.
    queued: Option<u64>,
    /// The document's changes when it last reported, and when.
    reported: Option<(u64, Instant)>,
}

/// Every buffer's slot, and what the threads are doing.
pub struct Scheduler<Key> {
    slots: HashMap<Key, Slot>,
    generations: u64,
    /// Counts places in the queue.
    order: u64,
    /// Whether a thread has a buffer no window shows.
    background: bool,
}

impl<Key> Default for Scheduler<Key> {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            generations: 0,
            order: 0,
            background: false,
        }
    }
}

impl<Key: Copy + Eq + Hash> Scheduler<Key> {
    /// No buffer yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a job: its edits follow the document now, or wait for the slice that has it.
    pub fn submit(&mut self, job: Job<Key>) {
        let order = self.next_order();
        let Job {
            buffer,
            language,
            revision,
            text,
            edits,
            shown,
        } = job;
        let slot = match self.slots.get_mut(&buffer) {
            Some(slot) if slot.language == language => slot,
            // A new buffer, or one in another language now.
            _ => {
                self.generations += 1;
                let slot = Slot {
                    generation: self.generations,
                    language,
                    document: Some(Document::new(language, text)),
                    revision,
                    waiting: Waiting::Nothing,
                    shown,
                    queued: Some(order),
                    reported: None,
                };
                self.slots.insert(buffer, slot);
                return;
            }
        };
        let moved = revision != slot.revision || edits.is_none();
        slot.shown = shown;
        slot.queued.get_or_insert(order);
        if !moved {
            return;
        }
        match &mut slot.document {
            Some(document) => {
                match edits {
                    Some(edits) => document.edit(&edits, &text),
                    None => *document = Document::new(language, text),
                }
                slot.revision = revision;
            }
            None => {
                let waiting = std::mem::replace(&mut slot.waiting, Waiting::Nothing);
                slot.waiting = joined(waiting, text, revision, edits);
            }
        }
    }

    /// Forgets `buffer` at once: what waited for it never runs, and a slice running on it
    /// gives back nothing.
    pub fn drop_buffer(&mut self, buffer: Key) {
        self.slots.remove(&buffer);
    }

    /// The next document a thread works on: of those with work, the first a window shows, or
    /// else the first of the others, unless a thread already has one of those.
    pub fn take_next(&mut self) -> Option<Checkout<Key>> {
        let ready = |slot: &&Slot| slot.document.is_some() && slot.queued.is_some();
        let shown = self
            .slots
            .iter()
            .filter(|(_, slot)| ready(slot) && !slot.shown.is_empty())
            .min_by_key(|(_, slot)| slot.queued);
        let pick = match shown {
            Some((buffer, _)) => Some((*buffer, false)),
            None if self.background => None,
            None => self
                .slots
                .iter()
                .filter(|(_, slot)| ready(slot))
                .min_by_key(|(_, slot)| slot.queued)
                .map(|(buffer, _)| (*buffer, true)),
        };
        let (buffer, background) = pick?;
        let slot = self.slots.get_mut(&buffer)?;
        let document = slot.document.take()?;
        slot.queued = None;
        self.background |= background;
        Some(Checkout {
            buffer,
            generation: slot.generation,
            document,
            shown: slot.shown.clone(),
            background,
        })
    }

    /// Puts a document back after its slice, with what waited for it, and says what to
    /// report: nothing for a buffer dropped meanwhile.
    pub fn give_back(
        &mut self,
        checkout: Checkout<Key>,
        outcome: Highlighted,
    ) -> Option<Report<Key>> {
        if checkout.background {
            self.background = false;
        }
        let order = self.next_order();
        let slot = self
            .slots
            .get_mut(&checkout.buffer)
            .filter(|slot| slot.generation == checkout.generation)?;
        let mut document = checkout.document;
        let revision = slot.revision;
        if let Highlighted::Failed(failure) = outcome {
            self.slots.remove(&checkout.buffer);
            return Some(failed(checkout.buffer, revision, failure));
        }
        let report = due(slot, &document, outcome).then(|| {
            slot.reported = Some((document.changes(), Instant::now()));
            Report {
                buffer: checkout.buffer,
                revision,
                outcome: Outcome::Highlights(document.highlights().clone()),
            }
        });
        let mut again = outcome == Highlighted::Paused;
        match std::mem::replace(&mut slot.waiting, Waiting::Nothing) {
            Waiting::Nothing => {}
            Waiting::Edits {
                batch,
                text,
                revision,
            } => {
                document.edit(&batch, &text);
                (slot.revision, again) = (revision, true);
            }
            Waiting::Reset { text, revision } => {
                document = Document::new(slot.language, text);
                (slot.revision, again) = (revision, true);
            }
        }
        slot.document = Some(document);
        if again {
            // To the back of the queue, so that buffers take turns.
            slot.queued = Some(slot.queued.map_or(order, |queued| queued.min(order)));
        }
        report
    }

    /// A slice panicked: its document is gone, and so is its slot.
    pub fn lost(&mut self, checkout: Checkout<Key>) -> Option<Report<Key>> {
        if checkout.background {
            self.background = false;
        }
        let slot = self
            .slots
            .get(&checkout.buffer)
            .filter(|slot| slot.generation == checkout.generation)?;
        let revision = slot.revision;
        self.slots.remove(&checkout.buffer);
        Some(failed(checkout.buffer, revision, Failure::Lost))
    }

    /// Whether `buffer` has a slot, for tests and logs.
    pub fn has(&self, buffer: Key) -> bool {
        self.slots.contains_key(&buffer)
    }

    fn next_order(&mut self) -> u64 {
        self.order += 1;
        self.order
    }
}

/// Joins a job to what waited for a checked-out document: edits compose into one batch, and a
/// reset, or edits that do not compose or pass [`MAX_WAITING`], start from nothing on the
/// newest text.
fn joined(waiting: Waiting, text: Rope, revision: u64, edits: Option<Vec<Splice>>) -> Waiting {
    let batch = match (waiting, edits) {
        (_, None) | (Waiting::Reset { .. }, Some(_)) => None,
        (Waiting::Nothing, Some(edits)) => Some(edits),
        (Waiting::Edits { batch, .. }, Some(edits)) => compose(&batch, &edits),
    };
    match batch.filter(|batch| batch.len() <= MAX_WAITING) {
        Some(batch) => Waiting::Edits {
            batch,
            text,
            revision,
        },
        None => Waiting::Reset { text, revision },
    }
}

/// Whether a slice that ended in `outcome` reports: only a change, and while its shown text is
/// not done, none; then at the end, or at most every [`REPORT_EVERY`].
fn due(slot: &Slot, document: &Document, outcome: Highlighted) -> bool {
    let changed = slot
        .reported
        .is_none_or(|(changes, _)| changes != document.changes());
    if !changed {
        return false;
    }
    match outcome {
        Highlighted::Done => true,
        Highlighted::Paused => {
            document.is_highlighted(&slot.shown)
                && slot
                    .reported
                    .is_none_or(|(_, at)| at.elapsed() >= REPORT_EVERY)
        }
        Highlighted::Failed(_) => false,
    }
}

fn failed<Key>(buffer: Key, revision: u64, failure: Failure) -> Report<Key> {
    Report {
        buffer,
        revision,
        outcome: Outcome::Failed(failure),
    }
}

#[cfg(test)]
mod tests;
