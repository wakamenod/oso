//! Judging what a terminal's output asked for (ADR 0120): each effect through the policy,
//! into the replies, the holds and the next snapshot's events, each kept within its bound.
//! The terminal's thread and the fuzz target both judge here.
//!
//! Must not depend on: the PTY or the channels; this fills what it is given.

use std::time::Instant;

use super::Hold;
use crate::engine::Effect;
use crate::event::Event;
use crate::limits;
use crate::policy::{self, Verdict};
use crate::snapshot::Snapshot;

/// Judges the effects the output raised: replies queue, while 64 KiB wait at most; holds
/// begin and end at `now`; events go into `next`. A title, a directory, a progress or a
/// clipboard write replaces the one of its kind already there, and is never dropped, so a
/// snapshot holds the latest of each; the rest, at most 256, are counted past that, a bell
/// right after a bell joined to it. Returns whether an event came. The terminal's thread and
/// the fuzz target both judge through this.
pub(in crate::terminal) fn judge(
    effects: &mut Vec<Effect>,
    replies: &mut Vec<u8>,
    next: &mut Snapshot,
    hold: &mut Hold,
    now: Instant,
) -> bool {
    let mut evented = false;
    for effect in effects.drain(..) {
        match policy::judge(effect) {
            Verdict::Reply(bytes) => {
                if replies.len() + bytes.len() <= limits::REPLIES_PENDING_MAX {
                    replies.extend_from_slice(&bytes);
                }
            }
            Verdict::Event(event) => {
                evented = true;
                keep(event, next);
            }
            Verdict::Hold(held) => hold.set(held, now),
            Verdict::Drop => {}
        }
    }
    evented
}

/// Keeps `event` in `next`, as `judge` says.
pub(in crate::terminal) fn keep(event: Event, next: &mut Snapshot) {
    let kind = std::mem::discriminant(&event);
    let latest = matches!(
        event,
        Event::Title(_) | Event::Directory(_) | Event::Progress(_) | Event::ClipboardWrite(_)
    );
    if latest {
        if let Some(at) = next
            .events
            .iter_mut()
            .find(|e| std::mem::discriminant(*e) == kind)
        {
            *at = event;
        } else {
            next.events.push(event);
        }
        return;
    }
    if matches!(event, Event::Bell) && matches!(next.events.last(), Some(Event::Bell)) {
        return;
    }
    if next.events.len() < limits::EVENTS_MAX {
        next.events.push(event);
    } else {
        next.dropped_events = next.dropped_events.saturating_add(1);
    }
}
