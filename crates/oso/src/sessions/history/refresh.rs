//! Listing and reading the transcripts for `session-search` (ADR 0145): newest first, those
//! that changed read again on up to `min(cores - 1, 8)` reading threads, at most [`AHEAD`]
//! past the one the search waits for, and each handed on in order as it is ready.
//!
//! Must not depend on: `app`, winit or the UI thread; the threads are scoped to one listing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use super::held::{self, Conversation, Listed};
use super::{AHEAD, Hold};

/// A transcript to read: its place in the listing, its file, and what was held of it.
type ToRead = (usize, Listed, Option<Conversation>);

/// How far the search has taken what the reading threads read: a reader waits before a
/// transcript more than [`AHEAD`] past it.
#[derive(Default)]
struct Gate {
    taken: Mutex<usize>,
    moved: std::sync::Condvar,
}

impl Gate {
    fn wait_for(&self, at: usize) {
        let mut taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        while at > *taken + AHEAD {
            taken = self
                .moved
                .wait(taken)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn took(&self, at: usize) {
        *self.taken.lock().unwrap_or_else(PoisonError::into_inner) = at + 1;
        self.moved.notify_all();
    }
}

/// Reads the transcripts of `slots` from `next` on, each once `gate` lets it, and sends each
/// one's conversation, or none if it could not be read or its reading panicked.
fn read_slots(
    slots: &[Mutex<Option<ToRead>>],
    next: &AtomicUsize,
    gate: &Gate,
    done: &Sender<(usize, Option<Conversation>)>,
) {
    while let Some(slot) = slots.get(next.fetch_add(1, Ordering::Relaxed)) {
        let taken = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some((at, listed, held)) = taken else {
            continue;
        };
        gate.wait_for(at);
        // The search waits for each transcript by its place, so one must always come.
        let read =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| held::read(&listed, held)))
                .unwrap_or(None);
        if done.send((at, read)).is_err() {
            return;
        }
    }
}

/// Lists the transcripts under `projects` into `hold`, newest first, reading those that
/// changed on the reading threads, and hands each conversation to `each` in that order as it
/// is ready, with those kept before it.
pub(super) fn refresh(
    projects: &Path,
    hold: &mut Hold,
    mut each: impl FnMut(&[Conversation], &Conversation),
) {
    let listed = held::list(projects);
    let mut before: HashMap<PathBuf, Conversation> = std::mem::take(&mut hold.conversations)
        .into_iter()
        .map(|conversation| (conversation.path.clone(), conversation))
        .collect();
    // What needs reading, by its place in the listing, with what was held of it.
    let mut ready: Vec<Option<Conversation>> = Vec::with_capacity(listed.len());
    let mut reads: Vec<ToRead> = Vec::new();
    for (at, listed) in listed.iter().enumerate() {
        match before.remove(&listed.path) {
            Some(conversation) if conversation.current(listed) => ready.push(Some(conversation)),
            held => {
                ready.push(None);
                reads.push((at, listed.clone(), held));
            }
        }
    }
    drop(before);
    let slots: Vec<Mutex<Option<ToRead>>> = reads
        .into_iter()
        .map(|read| Mutex::new(Some(read)))
        .collect();
    let (next, gate) = (AtomicUsize::new(0), Gate::default());
    let readers = thread::available_parallelism()
        .map_or(1, |count| count.get().saturating_sub(1))
        .clamp(1, 8)
        .min(slots.len().max(1));
    let (done, results) = mpsc::channel::<(usize, Option<Conversation>)>();
    let mut kept: Vec<Conversation> = Vec::with_capacity(listed.len());
    let mut bytes = 0;
    hold.left_out = 0;
    thread::scope(|scope| {
        for _ in 0..readers {
            let done = done.clone();
            let (slots, next, gate) = (&slots, &next, &gate);
            let _ = thread::Builder::new()
                .name("oso-history-read".into())
                .spawn_scoped(scope, move || read_slots(slots, next, gate, &done));
        }
        drop(done);
        let mut waiting: HashMap<usize, Option<Conversation>> = HashMap::new();
        for (at, slot) in ready.iter_mut().enumerate() {
            let conversation = match slot.take() {
                Some(conversation) => Some(conversation),
                None => loop {
                    if let Some(read) = waiting.remove(&at) {
                        break read;
                    }
                    match results.recv() {
                        Ok((got, read)) if got == at => break read,
                        Ok((got, read)) => {
                            waiting.insert(got, read);
                        }
                        // No reader is left: what was not read is not held.
                        Err(_) => break None,
                    }
                },
            };
            gate.took(at);
            let Some(mut conversation) = conversation else {
                continue;
            };
            if !conversation.held || bytes + conversation.bytes > hold.most {
                conversation.leave_out();
                hold.left_out += 1;
            } else {
                bytes += conversation.bytes;
                each(&kept, &conversation);
            }
            kept.push(conversation);
        }
    });
    hold.conversations = kept;
    hold.listed = Some(Instant::now());
}
