//! Keeping what the panic hook and the exit read up to date, on the UI thread.
//!
//! Must not depend on: `app`, `dispatch` or winit; whoever changes a buffer calls this.
//!
//! A command that did nothing costs a walk over the buffers, comparing a few small fields.
//! A key that changed a buffer adds one rope clone, which is O(1), one lock nobody else is
//! waiting for, and a binary search. Nothing allocates unless a buffer has just come to be
//! at risk.

use std::cell::{Cell, RefCell};
use std::sync::atomic::Ordering;
use std::sync::{Arc, MutexGuard, PoisonError};

use oso_core::buffer::{Buffer, BufferId, ContentId, Verdict};
use oso_core::editor::Editor;

use super::{Entry, Identity, Shared, State};

/// What the state knows of one buffer, to tell cheaply whether it changed.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Seen {
    id: BufferId,
    content: ContentId,
    /// Differs from its file, or a save of it is running, so its text may be only here.
    at_risk: bool,
    saving: bool,
    verdict: Verdict,
}

impl Seen {
    fn of(id: BufferId, buffer: &Buffer) -> Self {
        Self {
            id,
            content: buffer.content_id(),
            at_risk: buffer.is_modified() || buffer.is_saving(),
            saving: buffer.is_saving(),
            verdict: buffer.verdict(),
        }
    }
}

/// What is published of a buffer while a panicking command's state is held back, given what
/// `old` already is. The rule: until a command ends normally, the only text that enters the
/// shared state is what a running save is writing, as that text was asked to reach the disk.
/// Any other buffer keeps the text it had, or none. The facts stay true: whether a save
/// runs, and a buffer that now holds what its file does is no longer at risk.
fn held_back(mut now: Seen, old: Option<Seen>) -> Seen {
    if now.at_risk && !now.saving {
        match old.filter(|old| old.at_risk) {
            Some(old) => {
                now.content = old.content;
                now.verdict = old.verdict;
            }
            None => now.at_risk = false,
        }
    }
    now
}

/// The UI thread's end of the shared state.
pub struct Publisher {
    shared: Arc<Shared>,
    /// In the order of [`Editor::buffers`], whose ids only grow.
    seen: RefCell<Vec<Seen>>,
    /// The last command panicked, and no command has ended normally since.
    held: Cell<bool>,
}

/// Marks a command as running, until dropped, also on the way out of a panic.
pub struct CommandScope<'a>(&'a Shared);

impl Drop for CommandScope<'_> {
    fn drop(&mut self) {
        self.0.in_command.store(false, Ordering::Relaxed);
        self.0.panicked_in_command.store(false, Ordering::Relaxed);
    }
}

impl Publisher {
    pub(super) fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            seen: RefCell::default(),
            held: Cell::new(false),
        }
    }

    /// A command is about to run under the catch at the command boundary (ADR 0014). A panic
    /// until this is dropped costs that command and not the session, so it saves nothing.
    pub fn enter_command(&self) -> CommandScope<'_> {
        self.shared.in_command.store(true, Ordering::Relaxed);
        CommandScope(&self.shared)
    }

    /// A command ended without panicking: the state takes in all that changed.
    pub fn command_ended(&self, editor: &Editor) {
        self.held.set(false);
        self.publish(editor);
    }

    /// A command panicked. What it left is held back until a command ends normally, which is
    /// when the person has seen that text and gone on from it. A save it started still counts.
    pub fn command_panicked(&self, editor: &Editor) {
        self.held.set(true);
        self.publish(editor);
    }

    /// Brings the shared state up to date with `editor`, for what changed since last time,
    /// after anything but a command that changes a buffer: a save reporting, a load. What a
    /// panicking command left stays held back: see [`held_back`].
    pub fn publish(&self, editor: &Editor) {
        let held = self.held.get();
        let mut seen = self.seen.borrow_mut();
        let mut index = 0;
        for (id, buffer) in editor.buffers() {
            while seen.get(index).is_some_and(|gone| gone.id < id) {
                self.forget(seen.remove(index));
            }
            let old = seen.get(index).filter(|old| old.id == id).copied();
            let mut now = Seen::of(id, buffer);
            if held {
                now = held_back(now, old);
            }
            if old != Some(now) {
                if old.is_some() {
                    seen[index] = now;
                } else {
                    seen.insert(index, now);
                }
                self.update(&now, buffer, old);
            }
            index += 1;
        }
        while seen.len() > index {
            if let Some(gone) = seen.pop() {
                self.forget(gone);
            }
        }
    }

    /// A buffer that was at risk is gone.
    fn forget(&self, gone: Seen) {
        if gone.at_risk {
            self.remove(gone.id);
        }
    }

    /// Brings a buffer's entry to `now`, where `old` is what was last seen of it. Text goes in
    /// only if it is another text from the entry's, which is what lets a held-back one stay.
    fn update(&self, now: &Seen, buffer: &Buffer, old: Option<Seen>) {
        let old = old.filter(|old| old.at_risk);
        if !now.at_risk {
            if old.is_some() {
                self.remove(now.id);
            }
            return;
        }
        let file = buffer.baseline().and_then(|baseline| baseline.file);
        let same_text =
            old.is_some_and(|old| old.content == now.content && old.verdict == now.verdict);
        let mut text = (!same_text && now.verdict.may_be_copied()).then(|| buffer.text().clone());
        if old.is_some() {
            let mut state = self.lock();
            if let Ok(index) = index_of(&state, now.id) {
                let entry = &mut state.entries[index];
                if !same_text {
                    std::mem::swap(&mut entry.text, &mut text);
                }
                entry.content = now.content;
                entry.saving = now.saving;
                entry.file = file;
            }
            drop(state);
            // The rope it replaced goes here, with no lock held.
            drop(text);
        } else {
            let entry = Entry {
                buffer: now.id,
                identity: Arc::new(identity_of(buffer)),
                text,
                content: now.content,
                saving: now.saving,
                file,
            };
            let mut state = self.lock();
            if let Err(index) = index_of(&state, now.id) {
                state.entries.insert(index, entry);
            }
        }
    }

    fn remove(&self, id: BufferId) {
        let mut state = self.lock();
        let removed = index_of(&state, id).map(|index| state.entries.remove(index));
        drop(state);
        drop(removed);
    }

    /// Nobody else ever holds this for long, and whatever a panic left is whole.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where `id`'s entry is, or where it would go.
fn index_of(state: &State, id: BufferId) -> Result<usize, usize> {
    state
        .entries
        .binary_search_by_key(&id, |entry| entry.buffer)
}

fn identity_of(buffer: &Buffer) -> Identity {
    let info = buffer.file_info();
    Identity {
        name: buffer.name().to_owned(),
        path: buffer.path().map(ToOwned::to_owned),
        location: buffer.baseline().map(|baseline| baseline.location.clone()),
        bom: info.bom,
        line_ending: info.line_ending,
    }
}
