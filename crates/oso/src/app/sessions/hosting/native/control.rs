//! A native session's control layer in the app (ADR 0165, step 2c; parts 3a and 3b): what its
//! stream's answers and `system/init` say to the control layer, routed to the request each
//! answers, and what each request has due at the times they set. The rules are
//! `sessions::native::control`'s and each request's own module's; `interrupting`, `status` and
//! `mode` carry out what they come to.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - Only the first `initialize` answer is taken: its slash commands, models and mode.
//! - Each answer is noted in the stream's order; what it comes to, a step that needs more of
//!   the app than the session, is carried out after the batch's events were folded.

use std::time::Instant;

use super::{Native, mode, status};
use crate::app::App;
use crate::sessions::native::control::Answered;
use crate::sessions::native::interrupt;
use crate::sessions::native::wire::Event;
use crate::sessions::native::wire::control::Request;
use crate::sessions::worker::SessionKey;

/// What an event of the control layer's comes to, carried out after the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Follow {
    /// The interrupt's next step, or a log line.
    Interrupt(interrupt::Then),
    /// What the answer to a `set_permission_mode` request comes to.
    Mode(crate::sessions::native::mode::Then),
}

/// Notes what `event` of a native session says to its control layer and the requests it
/// answers, in the stream's order, and what that comes to in `follows`; `resumable` if no
/// resume of it is held now, for a mode's fallback. Returns whether the mode shown changed:
/// `initialize`'s answer says it.
pub(super) fn control_event(
    native: &mut Native,
    event: &mut Event,
    follows: &mut Vec<Follow>,
    resumable: bool,
) -> bool {
    // Where its process is now, as this event finds it, for a mode's fallback.
    let process = mode::process(native, resumable);
    let Some(control) = native.control.as_mut() else {
        return false;
    };
    let log = |why: String| Follow::Interrupt(interrupt::Then::Log(why));
    match event {
        Event::Init(init) => follows.extend(control.system_init(&init.capabilities).map(log)),
        Event::Answer(answer) => match control.answered(answer) {
            Answered::Initialize(said) => {
                follows.extend(said.map(log));
                // The slash commands it lists, moved: no row needs them (step 2d).
                if let Some(commands) = answer.commands.take() {
                    native.commands.initialized(commands);
                }
                // The models it lists, for the status line (part 3a), and its mode (part 3b).
                status::initialize_answered(native, answer);
                return mode::initialize_answered(native, answer);
            }
            Answered::Request {
                request: Request::SetPermissionMode(_),
                id,
                error,
            } => {
                let then = (native.mode).answered(control, id, error.as_deref(), process);
                follows.extend(then.into_iter().map(Follow::Mode));
            }
            Answered::Request {
                request: Request::GetUsage,
                id,
                error,
            } => follows
                .extend(status::usage_answered(native, id, error.as_deref(), answer).map(log)),
            Answered::Request {
                request: Request::Interrupt,
                id,
                error,
            } => follows.extend(
                (native.interrupt)
                    .answered(control, id, error.as_deref(), Instant::now())
                    .into_iter()
                    .map(Follow::Interrupt),
            ),
            Answered::Request { .. } | Answered::Other => {}
        },
        Event::Result(_) => native.interrupt.turn_ended(),
        _ => {}
    }
    false
}

impl App {
    /// Carries out `follow` for native session `key`.
    pub(super) fn follow(&mut self, key: SessionKey, follow: Follow) {
        match follow {
            Follow::Interrupt(then) => self.interrupt_step(key, then),
            Follow::Mode(then) => self.mode_step(key, then),
        }
    }

    /// When a native session's control layer, interrupt, mode switch, card focus or
    /// `get_usage` is next due, if one is. A request that waits on a process that ended waits
    /// for nothing.
    pub(in crate::app) fn natives_deadline(&self) -> Option<Instant> {
        // A permission card that waits for point, too (step 2e-2).
        let card = self.card_focus_deadline();
        (self.hosting.natives.map.values())
            .filter(|native| native.control.is_some())
            .flat_map(|native| {
                let control = native.control.as_ref().and_then(|c| c.deadline());
                (control.into_iter())
                    .chain(native.interrupt.deadline())
                    .chain(native.mode.deadline())
            })
            .chain(card)
            .chain(self.usage_deadline())
            .min()
    }

    /// Carries out what is due of each native session's control layer, interrupt, mode switch,
    /// card focus and `get_usage` at `now`.
    pub(in crate::app) fn natives_due(&mut self, now: Instant) {
        self.card_focus_due(now);
        self.usage_due(now);
        let processes: Vec<_> = (self.hosting.natives.map.keys())
            .filter_map(|key| Some((*key, self.mode_process(*key)?)))
            .collect();
        let mut due = Vec::new();
        for (key, process) in processes {
            let Some(native) = self.hosting.natives.map.get_mut(&key) else {
                continue;
            };
            let Some(control) = native.control.as_mut() else {
                continue;
            };
            let mut then: Vec<_> = control
                .due(now)
                .map(interrupt::Then::Log)
                .into_iter()
                .collect();
            then.extend(native.interrupt.due(control, now));
            let modes = native.mode.due(control, process, now);
            if !then.is_empty() || !modes.is_empty() {
                due.push((key, then, modes));
            }
        }
        for (key, then, modes) in due {
            for then in then {
                self.interrupt_step(key, then);
            }
            for then in modes {
                self.mode_step(key, then);
            }
        }
    }
}
