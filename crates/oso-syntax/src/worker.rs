//! The syntax worker's schedule (ADR 0111): one slot per buffer, the jobs the UI sends into
//! them, which document a thread works on next, and the reports that come of a slice. The
//! threads, their lock and their waking are the tip crate's; this is plain state, so that the
//! rules are tested without them.
//!
//! Must not depend on: threads, buffers, windows or faces; it sees jobs, documents and reports.
//!
//! - Nothing here builds, edits or frees a document: that is tree-sitter's work, which can take
//!   milliseconds, and the schedule's methods run under the lock, some on the UI thread. Jobs
//!   wait in their slot until a thread checks the document out, and [`Checkout::run`], with
//!   nothing locked, follows them. A document no longer wanted goes on a list a thread frees
//!   after it unlocks ([`Scheduler::take_doomed`]).
//! - Buffers a window shows go first. At most one checkout at a time is of a buffer no window
//!   shows now, so one slice that runs long on a background buffer never holds the shown ones.
//! - A dropped buffer's slot goes at once: a job that waited in it never runs, and a slice that
//!   was running gives back nothing, as each slot has a generation of its own.
//! - Each job carries the UI's epoch for its buffer, which a new start moves on, and each report
//!   the epoch of the document it is of and a stamp in the order slices ended, so that the UI
//!   drops a report from before a new start, or one that comes after a later one.
//! - A document no buffer owns (ADR 0122), such as a review's side, has a slot of its own whose
//!   text never changes ([`unowned`]). It runs only where background buffers do, one thread
//!   at a time, so a shown buffer always has a thread.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::syntax::{Highlights, Splice, compose};

pub use unowned::Refused;
use unowned::{Kind, Stale};

use crate::document::{Document, Failure, Highlighted, MAX_WAITING};
use crate::language::Language;
use crate::queries::Queries;

/// How long one slice takes at most, about.
pub(crate) const SLICE: Duration = Duration::from_millis(20);

/// How often a buffer whose shown text is done reports while the rest is highlighted.
const REPORT_EVERY: Duration = Duration::from_millis(50);

/// What the UI sends after a buffer changed, or what its windows show did.
#[derive(Clone, Debug)]
pub struct Job<Key> {
    /// Which buffer: the tip crate's id for it. A job for a document's key replaces the
    /// document with a buffer.
    pub buffer: Key,
    pub language: Language,
    /// The UI's count of new starts for this buffer: a job with `edits: None` moves it on.
    pub epoch: u64,
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
    /// Which buffer or document no buffer owns: the key it was sent or opened with.
    pub buffer: Key,
    /// The epoch of the job the document started from; 0 for a document no buffer owns.
    pub epoch: u64,
    /// Larger for a slice that ended later.
    pub stamp: u64,
    /// The revision whose text the highlights are for; 0 for a document no buffer owns, whose
    /// text never changes.
    pub revision: u64,
    pub outcome: Outcome,
}

/// New highlights, or the end of highlighting in a buffer.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// A snapshot of the highlights, O(1) to clone.
    Highlights(Highlights),
    /// The document stopped for good, and its slot is gone.
    Failed(Failure),
    /// A slice panicked: the document is dropped, and its slot is gone.
    Lost,
}

/// A document out of its slot for one slice, with what waited for it.
pub struct Checkout<Key> {
    buffer: Key,
    generation: u64,
    language: Language,
    /// None for a buffer whose document is not made yet.
    document: Option<Document>,
    waiting: Waiting,
    shown: Vec<Range<usize>>,
}

impl<Key> Checkout<Key> {
    /// Follows what waited, then runs one slice. A thread calls it with its own queries, and
    /// nothing locked: this is where documents are made and edited.
    pub fn run(&mut self, queries: &mut Queries) -> Highlighted {
        self.run_for(queries, SLICE)
    }

    fn run_for(&mut self, queries: &mut Queries, slice: Duration) -> Highlighted {
        self.follow();
        let document = self
            .document
            .get_or_insert_with(|| Document::new(self.language, Rope::new()));
        document.highlight(queries, &self.shown, slice)
    }

    fn follow(&mut self) {
        match std::mem::take(&mut self.waiting) {
            Waiting::Nothing => {}
            Waiting::Edits { batch, text, .. } => match &mut self.document {
                Some(document) => document.edit(&batch, &text),
                None => self.document = Some(Document::new(self.language, text)),
            },
            Waiting::Reset { text, .. } => {
                // The old document is freed here, with nothing locked.
                self.document = Some(Document::new(self.language, text));
            }
        }
    }
}

/// Jobs that came since a document was last checked out.
#[derive(Default)]
enum Waiting {
    #[default]
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
        epoch: u64,
    },
}

impl Waiting {
    fn revision(&self) -> Option<u64> {
        match self {
            Self::Nothing => None,
            Self::Edits { revision, .. } | Self::Reset { revision, .. } => Some(*revision),
        }
    }
}

struct Slot {
    generation: u64,
    language: Language,
    /// None while a thread has it, and before its first slice.
    document: Option<Document>,
    checked_out: bool,
    /// The revision whose text the document follows, once what waited is followed.
    revision: u64,
    /// The epoch of the document, once what waited is followed.
    epoch: u64,
    waiting: Waiting,
    shown: Vec<Range<usize>>,
    /// Its place in the queue, if it has work: lower goes first.
    queued: Option<u64>,
    /// The document's changes when it last reported, and when.
    reported: Option<(u64, Instant)>,
    /// A buffer's, or a document's no buffer owns.
    kind: Kind,
}

impl Slot {
    /// A slot with nothing made yet, which starts from `start` at its first checkout.
    fn fresh(
        generation: u64,
        language: Language,
        start: Waiting,
        shown: Vec<Range<usize>>,
    ) -> Self {
        let (revision, epoch) = match &start {
            Waiting::Reset {
                revision, epoch, ..
            } => (*revision, *epoch),
            Waiting::Nothing | Waiting::Edits { .. } => (0, 0),
        };
        Self {
            generation,
            language,
            document: None,
            checked_out: false,
            revision,
            epoch,
            waiting: start,
            shown,
            queued: None,
            reported: None,
            kind: Kind::Buffer,
        }
    }
}

/// Every buffer's slot, and the documents to free.
pub struct Scheduler<Key> {
    slots: HashMap<Key, Slot>,
    generations: u64,
    /// Counts places in the queue, and the stamps of reports.
    order: u64,
    /// Documents no longer wanted, for a thread to free with nothing locked.
    doomed: Vec<Document>,
    /// Checkouts whose slot went while a thread had it: until given back, they still hold
    /// their lane and their tree.
    stale: Vec<Stale>,
    /// How many slots are documents no buffer owns, so that an open counts in O(1).
    unowned: usize,
}

impl<Key> Default for Scheduler<Key> {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            generations: 0,
            order: 0,
            doomed: Vec::new(),
            stale: Vec::new(),
            unowned: 0,
        }
    }
}

impl<Key: Copy + Eq + Hash> Scheduler<Key> {
    /// No buffer yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes a job into its slot, where it waits for the next checkout. O(splices): nothing is
    /// parsed or freed here.
    pub fn submit(&mut self, job: Job<Key>) {
        let order = self.next_order();
        let Job {
            buffer,
            language,
            epoch,
            revision,
            text,
            edits,
            shown,
        } = job;
        let slot = match self.slots.get_mut(&buffer) {
            Some(slot) if slot.language == language && !slot.is_unowned() => slot,
            // A new buffer, one in another language now, or a document's key.
            _ => {
                self.generations += 1;
                let start = Waiting::Reset {
                    text,
                    revision,
                    epoch,
                };
                let slot = Slot {
                    queued: Some(order),
                    ..Slot::fresh(self.generations, language, start, shown)
                };
                self.replace(buffer, slot);
                return;
            }
        };
        slot.shown = shown;
        slot.queued.get_or_insert(order);
        let current = slot.waiting.revision().unwrap_or(slot.revision);
        if revision == current && edits.is_some() {
            return;
        }
        let waiting = std::mem::take(&mut slot.waiting);
        slot.waiting = match edits {
            Some(edits) => joined(waiting, text, revision, epoch, edits),
            None => Waiting::Reset {
                text,
                revision,
                epoch,
            },
        };
        if matches!(slot.waiting, Waiting::Reset { .. })
            && let Some(document) = slot.document.take()
        {
            // Not freed here: the UI thread may be the caller.
            self.doomed.push(document);
        }
    }

    /// Forgets `buffer` at once: what waited for it never runs, and a slice running on it
    /// gives back nothing. Its document goes on the list to free.
    pub fn drop_buffer(&mut self, buffer: Key) {
        if let Some(slot) = self.remove(&buffer) {
            self.retire(slot);
        }
    }

    /// Puts `slot` under `key`, retiring the one there.
    fn replace(&mut self, key: Key, slot: Slot) {
        self.unowned += usize::from(slot.is_unowned());
        if let Some(old) = self.slots.insert(key, slot) {
            self.unowned -= usize::from(old.is_unowned());
            self.retire(old);
        }
    }

    /// Takes `key`'s slot out: every slot leaves through here, so that the count holds.
    fn remove(&mut self, key: &Key) -> Option<Slot> {
        let slot = self.slots.remove(key)?;
        self.unowned -= usize::from(slot.is_unowned());
        Some(slot)
    }

    /// A slot no longer wanted: its document goes on the list to free, and if a thread has it,
    /// what that checkout holds counts until it is given back.
    fn retire(&mut self, slot: Slot) {
        if slot.checked_out {
            self.stale.push(Stale::of(&slot));
        }
        // Not freed here: the UI thread may be the caller.
        self.doomed.extend(slot.document);
    }

    /// The documents to free, for a thread to drop once it has unlocked.
    pub fn take_doomed(&mut self) -> Vec<Document> {
        std::mem::take(&mut self.doomed)
    }

    /// The next document a thread works on: of those with work, the first buffer a window
    /// shows, or else the first of the others in the lane's order, unless a thread already has
    /// one of those. A document no buffer owns that would make a tree waits while four hold one.
    pub fn take_next(&mut self) -> Option<Checkout<Key>> {
        let lane_busy = self.lane_busy();
        let ready = |slot: &&Slot| !slot.checked_out && slot.queued.is_some();
        let shown = self
            .slots
            .iter()
            .filter(|(_, slot)| ready(slot) && !slot.in_lane())
            .min_by_key(|(_, slot)| slot.queued);
        let buffer = match shown {
            Some((buffer, _)) => *buffer,
            None if lane_busy => return None,
            None => {
                let room = self.room_for_a_tree();
                let blocked = !room && self.slots.values().any(Slot::is_blocked);
                *self
                    .slots
                    .iter()
                    .filter(|(_, slot)| ready(slot) && (room || !slot.would_start()))
                    .min_by_key(|(_, slot)| slot.rank(blocked))?
                    .0
            }
        };
        let slot = self.slots.get_mut(&buffer)?;
        let waiting = std::mem::take(&mut slot.waiting);
        match &waiting {
            Waiting::Nothing => {}
            Waiting::Edits { revision, .. } => slot.revision = *revision,
            Waiting::Reset {
                revision, epoch, ..
            } => {
                (slot.revision, slot.epoch) = (*revision, *epoch);
                slot.reported = None;
            }
        }
        slot.checked_out = true;
        slot.queued = None;
        Some(Checkout {
            buffer,
            generation: slot.generation,
            language: slot.language,
            document: slot.document.take(),
            waiting,
            shown: slot.shown.clone(),
        })
    }

    /// Puts a document back after its slice, and says what to report: nothing for a buffer
    /// dropped meanwhile, whose document goes on the list to free.
    pub fn give_back(
        &mut self,
        checkout: Checkout<Key>,
        outcome: Highlighted,
    ) -> Option<Report<Key>> {
        let stamp = self.next_order();
        let Some(slot) = self
            .slots
            .get_mut(&checkout.buffer)
            .filter(|slot| slot.generation == checkout.generation)
        else {
            self.settle(checkout.generation);
            self.doomed.extend(checkout.document);
            return None;
        };
        slot.checked_out = false;
        let document = checkout.document?;
        let (epoch, revision) = (slot.epoch, slot.revision);
        let report = |outcome| Report {
            buffer: checkout.buffer,
            epoch,
            stamp,
            revision,
            outcome,
        };
        if let Highlighted::Failed(failure) = outcome {
            self.remove(&checkout.buffer);
            self.doomed.push(document);
            return Some(report(Outcome::Failed(failure)));
        }
        let sent = due(slot, &document, outcome).then(|| {
            slot.reported = Some((document.changes(), Instant::now()));
            report(Outcome::Highlights(document.highlights().clone()))
        });
        if let Kind::Unowned { place, finished } = &mut slot.kind {
            if outcome == Highlighted::Done {
                // Its text never changes: the tree is of no more use, and a thread frees it.
                *finished = true;
                slot.shown = Vec::new();
                self.doomed.push(document);
            } else {
                // It keeps its place, so that a started document finishes first.
                slot.queued.get_or_insert(*place);
                slot.document = Some(document);
            }
            return sent;
        }
        slot.document = Some(document);
        if outcome == Highlighted::Paused {
            // To the back of the queue, so that buffers take turns.
            slot.queued.get_or_insert(stamp);
        }
        sent
    }

    /// A slice panicked: its document, if any is left, goes on the list to free, and its slot
    /// goes.
    pub fn lost(&mut self, checkout: Checkout<Key>) -> Option<Report<Key>> {
        let stamp = self.next_order();
        self.settle(checkout.generation);
        self.doomed.extend(checkout.document);
        let slot = self
            .slots
            .get(&checkout.buffer)
            .filter(|slot| slot.generation == checkout.generation)?;
        let report = Report {
            buffer: checkout.buffer,
            epoch: slot.epoch,
            stamp,
            revision: slot.revision,
            outcome: Outcome::Lost,
        };
        self.remove(&checkout.buffer);
        Some(report)
    }

    /// Whether `buffer` has a slot.
    #[cfg(test)]
    pub(crate) fn has(&self, buffer: Key) -> bool {
        self.slots.contains_key(&buffer)
    }

    fn next_order(&mut self) -> u64 {
        self.order += 1;
        self.order
    }
}

/// Joins a job's edits to what waited: edits compose into one batch, and a reset takes them in
/// as its newest text. Edits that do not compose, or pass [`MAX_WAITING`], start from nothing,
/// in the same epoch, as the text's history is the same.
fn joined(waiting: Waiting, text: Rope, revision: u64, epoch: u64, edits: Vec<Splice>) -> Waiting {
    let batch = match waiting {
        Waiting::Nothing => Some(edits),
        Waiting::Edits { batch, .. } => compose(&batch, &edits),
        Waiting::Reset { epoch, .. } => {
            return Waiting::Reset {
                text,
                revision,
                epoch,
            };
        }
    };
    match batch.filter(|batch| batch.len() <= MAX_WAITING) {
        Some(batch) => Waiting::Edits {
            batch,
            text,
            revision,
        },
        None => Waiting::Reset {
            text,
            revision,
            epoch,
        },
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

mod unowned;

#[cfg(test)]
mod tests;
