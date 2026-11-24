//! A native session's status line in the app (ADR 0165, part 3a): what its stream and its
//! control layer say of its model, mode, context, cost and rate limit, kept for its modeline's
//! note and its footer, which `note` writes; and its `get_usage`, asked once a process on tier 2. The rules are
//! `sessions::native::status`'s and `usage`'s.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - Answers reach it through the control layer (`control::control_event`), as the
//!   interrupt's do: `initialize`'s models, and `get_usage`'s usage.
//! - `get_usage` is asked once the process's stream is there to write it: the answer to
//!   `initialize` may come before the start thread says the process started (step 2c).
//! - The note is written again only when an event may change it, and a frame is asked for only
//!   when it changed while the session is shown.
//! - Why `get_usage` was refused, or went unanswered, is logged; nothing is said to the person,
//!   whose line waits for the stream.
//! - The model and effort the person chose with `/model` and `/effort`
//!   (`sessions::native::choice`, part 3c) are noted after each event here too: the effort
//!   noted is the line's to write, and each later process starts with both.

use std::time::Instant;

use oso_core::buffer::BufferId;

use super::{Native, NativeSessions};
use crate::app::App;
use crate::sessions::native::footer::Footer;
use crate::sessions::native::status::StatusLine;
use crate::sessions::native::usage::GetUsage;
use crate::sessions::native::wire::Event;
use crate::sessions::native::wire::control::{Answer, Request};
use crate::sessions::worker::SessionKey;

/// A native session's status line, and its process's `get_usage`.
#[derive(Debug, Default)]
pub(super) struct StatusState {
    pub(super) line: StatusLine,
    /// What its footer says, once written (ADR 0170).
    pub(super) footer: Option<Footer>,
    usage: GetUsage,
    /// An answer changed the line since the last event was noted.
    answered: bool,
}

impl StatusState {
    /// A new process: its `get_usage` is asked once its `initialize` is answered.
    pub(super) fn new_process(&mut self) {
        self.usage = GetUsage::default();
    }
}

/// `initialize` was answered, as `answer` says: its models, if it succeeded.
pub(super) fn initialize_answered(native: &mut Native, answer: &mut Answer) {
    if answer.error.is_none()
        && let Some(models) = answer.models.take()
    {
        native.status.line.initialized(models);
        native.status.answered = true;
    }
}

/// The control layer's answer to request `id`, a `get_usage`, refused with `error` if `Some`:
/// its usage taken from `answer`, or the log line of its refusal.
pub(super) fn usage_answered(
    native: &mut Native,
    id: u64,
    error: Option<&str>,
    answer: &mut Answer,
) -> Option<String> {
    let status = &mut native.status;
    match status.usage.answered(id, error)? {
        Ok(()) => {
            if let Some(usage) = answer.usage.take() {
                status.line.usage_answered(usage);
                status.answered = true;
            }
            None
        }
        Err(why) => Some(why),
    }
}

/// Notes what `event` of a native session says to its status line, after its control layer
/// read it: whether the line may read differently, and the id of a `get_usage` to write now.
pub(super) fn status_event(native: &mut Native, event: &Event) -> (bool, Option<u64>) {
    let status = &mut native.status;
    let changed = status.line.event(event) | std::mem::take(&mut status.answered);
    // A `/effort` the CLI took changes the effort shown (part 3c).
    let changed = native.choice.event(event) | changed;
    (changed, ask(native))
}

/// The id of native session `native`'s `get_usage` to write now, if it is due and its stream is
/// there to write it.
fn ask(native: &mut Native) -> Option<u64> {
    native.stream.as_ref()?;
    let control = native.control.as_mut()?;
    native.status.usage.ask(control, Instant::now())
}

impl NativeSessions {
    /// The footer of the session whose transcript `buffer` is, once it has one.
    pub(in crate::app) fn footer(&self, buffer: BufferId) -> Option<&Footer> {
        let native = self.map.values().find(|native| native.buffer == buffer)?;
        native.status.footer.as_ref()
    }

    /// Sessions that name their keys, one of them with the transcript `buffer`, which says `footer`.
    #[cfg(test)]
    pub(in crate::app) fn footed(buffer: BufferId, footer: Footer) -> Self {
        let mut natives = Self::saying(["hint"; 3], "", None);
        let mut native = super::markdown::tests::native(buffer);
        native.status.footer = Some(footer);
        natives.map.insert(1, native);
        natives
    }
}

impl App {
    /// Writes native session `key`'s `get_usage` if it is due: after its events, and once its
    /// process's stream is there, as its `initialize` may be answered first.
    pub(super) fn ask_usage(&mut self, key: SessionKey, id: Option<u64>) {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(id) = id.or_else(|| ask(native)) else {
            return;
        };
        let stream = native.stream.as_ref();
        let written = stream.is_some_and(|stream| stream.request(id, Request::GetUsage).is_ok());
        if !written && let Some(control) = native.control.as_mut() {
            native.status.usage.not_written(control);
        }
    }

    /// When a native session's `get_usage` stops waiting, if one waits on a process that runs:
    /// one whose process ended waits for nothing, and its deadline would never be met
    /// (`usage_due` needs the control layer), so it is not given.
    pub(super) fn usage_deadline(&self) -> Option<Instant> {
        (self.hosting.natives.map.values())
            .filter(|native| native.control.is_some())
            .filter_map(|native| native.status.usage.deadline())
            .min()
    }

    /// Forgets each native session's `get_usage` not answered by `now`, saying so in the log.
    pub(super) fn usage_due(&mut self, now: Instant) {
        for native in self.hosting.natives.map.values_mut() {
            let Some(control) = native.control.as_mut() else {
                continue;
            };
            if let Some(why) = native.status.usage.due(control, now) {
                log::info!("native session control: {why:?}");
            }
        }
    }

    /// Writes native session `key`'s note and footer again, and asks for a frame if it changed while the
    /// session is shown.
    pub(super) fn native_note_changed(&mut self, key: SessionKey) {
        let Some(buffer) = self
            .hosting
            .natives
            .map
            .get(&key)
            .map(|native| native.buffer)
        else {
            return;
        };
        if self.write_native_note(key)
            && self
                .editor
                .windows()
                .any(|window| window.buffer() == buffer)
        {
            self.request_redraw();
        }
    }
}
