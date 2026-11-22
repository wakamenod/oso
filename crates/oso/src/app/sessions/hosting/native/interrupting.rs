//! A native session's interrupt in the app (ADR 0165, step 2c): `C-c C-k` ends the turn under
//! way, and `C-u C-c C-c` sends the composer's text ahead of the queue once it ended. The rules
//! are `sessions::native::control`'s and `interrupt`'s, and the queue's `queue`'s: this
//! carries out what they come to, at the times they set.
//!
//! Must not depend on: key presses or how a frame is drawn.
//!
//! - On tier 2 the process stays; on tier 1 it is interrupted with a signal, stopped if it does
//!   not end, and resumed once it ended (`end`), as a resume is, under the trust judged then.
//!   Windows has no tier 1: a signal there would end `claude` at once and leave its tools.
//! - The queue pauses with the person's interrupt; a send now goes first and the queue goes on.
//! - Why tier 2 is off, and why a request fell back, are logged as the CLI said them, escaped.

use std::time::Instant;

use super::{Native, draw};
use crate::app::App;
use crate::sessions::native::control::Answered;
use crate::sessions::native::interrupt::{NotBegun, Then};
use crate::sessions::native::wire::Event;
use crate::sessions::native::wire::control::Request;
use crate::sessions::worker::SessionKey;

/// Whether a signal reaches the CLI alone, so that it ends its own tools: not on Windows, where
/// the process ends at once and what it started goes on.
pub(super) const SIGNALS: bool = cfg!(unix);

/// How the person's interrupt went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Interrupted {
    /// It is under way, or was already.
    Yes,
    /// No turn runs to interrupt.
    NoTurn,
    /// Nothing carries it: said.
    No,
}

impl App {
    /// Interrupts native session `key`'s turn under way, pausing its queue if `pause` (the
    /// person's `C-c C-k`, not a send now's, which goes first). Says how it went.
    pub(super) fn native_interrupt(&mut self, key: SessionKey, pause: bool) -> Interrupted {
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return Interrupted::NoTurn;
        };
        let (Some(control), true) = (native.control.as_mut(), native.composing.busy) else {
            return Interrupted::NoTurn;
        };
        let then = match native.interrupt.begin(control, Instant::now()) {
            Ok(then) => then,
            Err(NotBegun::Already) => {
                self.editor.message("The turn is being interrupted");
                return Interrupted::Yes;
            }
            Err(NotBegun::NoWay) => {
                self.editor.message(
                    "This claude takes no interrupt request: session-stop stops the session",
                );
                return Interrupted::No;
            }
        };
        if pause {
            native.composing.interrupted();
        }
        self.editor.message("Interrupting the turn…");
        self.interrupt_step(key, then);
        self.draw_queue(key);
        Interrupted::Yes
    }

    /// Carries out `then` for native session `key`.
    pub(super) fn interrupt_step(&mut self, key: SessionKey, then: Then) {
        if let Then::Log(why) = &then {
            return log::info!("native session control: {why:?}");
        }
        let Some(native) = self.hosting.natives.map.get_mut(&key) else {
            return;
        };
        let Some(stream) = &native.stream else {
            return;
        };
        match then {
            Then::Log(_) => {}
            Then::Request(id) => {
                if stream.request(id, Request::Interrupt).is_err()
                    && let Some(control) = native.control.as_mut()
                {
                    let then = native.interrupt.not_written(control, Instant::now());
                    for then in then {
                        self.interrupt_step(key, then);
                    }
                }
            }
            Then::Signal => {
                stream.interrupt();
                let change = native
                    .rows
                    .note("Interrupted with a signal: claude ends, and resumes its conversation.");
                let buffer = native.buffer;
                draw(&mut self.editor, native, [change]);
                // A frame only when the row is in sight.
                if self
                    .editor
                    .windows()
                    .any(|window| window.buffer() == buffer)
                {
                    self.request_redraw();
                }
            }
            Then::Stop => stream.stop(),
            Then::GaveUp => {
                self.editor
                    .message("The interrupt went unanswered: session-stop stops the session");
                self.write_native_note(key);
            }
        }
    }

    /// When a native session's control layer or interrupt is next due, if one is.
    pub(in crate::app) fn natives_deadline(&self) -> Option<Instant> {
        (self.hosting.natives.map.values())
            .flat_map(|native| {
                let control = native.control.as_ref().and_then(|c| c.deadline());
                control.into_iter().chain(native.interrupt.deadline())
            })
            .min()
    }

    /// Carries out what is due of each native session's control layer and interrupt at `now`.
    pub(in crate::app) fn natives_due(&mut self, now: Instant) {
        let due: Vec<(SessionKey, Vec<Then>)> = (self.hosting.natives.map.iter_mut())
            .filter_map(|(key, native)| {
                let control = native.control.as_mut()?;
                let mut then: Vec<Then> = control.due(now).map(Then::Log).into_iter().collect();
                then.extend(native.interrupt.due(control, now));
                (!then.is_empty()).then_some((*key, then))
            })
            .collect();
        for (key, then) in due {
            for then in then {
                self.interrupt_step(key, then);
            }
        }
    }
}

/// Notes what `event` of a native session says to its control layer and its interrupt, and
/// what that comes to in `then`.
pub(super) fn control_event(native: &mut Native, event: &Event, then: &mut Vec<Then>) {
    let Some(control) = native.control.as_mut() else {
        return;
    };
    match event {
        Event::Init(init) => then.extend(control.system_init(&init.capabilities).map(Then::Log)),
        Event::Answer(answer) => match control.answered(answer) {
            Answered::Initialize(said) => then.extend(said.map(Then::Log)),
            Answered::Request {
                request: Request::Interrupt,
                id,
                error,
            } => then.extend((native.interrupt).answered(
                control,
                id,
                error.as_deref(),
                Instant::now(),
            )),
            Answered::Request { .. } | Answered::Other => {}
        },
        Event::Result(_) => native.interrupt.turn_ended(),
        _ => {}
    }
}
