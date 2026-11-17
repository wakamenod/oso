//! The key a `cancelOperation:` stands for. AppKit's window sends it to the first responder in
//! place of `keyDown:` for Command-`.`, so `M-.` reaches oso only this way (ADR 0149). The
//! event is AppKit's current one, which stays current after its dispatch, so one is taken only
//! once, and never while another event is being handled. A `cancelOperation:` with no such key,
//! as from Accessibility or a stray sender, is no key.
//!
//! Must not depend on: the run loop's hooks, the delegates or the view's state, which the view
//! passes in.

use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSEvent, NSEventType, NSView};

use super::keys;

/// Which key-down an event is, to tell one the view has routed from a new one. An event's
/// address may be reused once it is freed, so it is not what tells them apart.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Stamp {
    time: f64,
    code: u16,
    window: isize,
}

/// The stamp of a key-down event.
pub(super) fn stamp(event: &NSEvent) -> Stamp {
    Stamp {
        time: event.timestamp(),
        code: event.keyCode(),
        window: event.windowNumber(),
    }
}

/// The key-down AppKit is dispatching, if `view` got `cancelOperation:` in place of `keyDown:`
/// for it: one for `view`'s window, with Command held, that is not the key-down the view
/// routed last (`routed`), while no event is being handled (`nested`).
pub(super) fn diverted_key(
    view: &NSView,
    nested: bool,
    routed: Option<Stamp>,
) -> Option<Retained<NSEvent>> {
    let event = NSApplication::sharedApplication(view.mtm()).currentEvent()?;
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let ours = view
        .window()
        .is_some_and(|window| window.windowNumber() == event.windowNumber());
    let fresh = is_fresh(nested, routed, stamp(&event));
    let held = keys::held(event.modifierFlags());
    keys::is_diverted_key(held, ours, fresh).then_some(event)
}

/// Whether the key-down stamped `now` is new: not the one the view routed last (`routed`),
/// and not taken while another event is being handled (`nested`).
fn is_fresh(nested: bool, routed: Option<Stamp>, now: Stamp) -> bool {
    !nested && routed != Some(now)
}

#[cfg(test)]
mod tests {
    use super::{Stamp, is_fresh};

    fn at(time: f64) -> Stamp {
        Stamp {
            time,
            code: 47,
            window: 1,
        }
    }

    #[test]
    fn a_key_down_is_taken_once_and_never_during_another_event() {
        assert!(is_fresh(false, None, at(1.0)));
        assert!(!is_fresh(false, Some(at(1.0)), at(1.0)), "routed already");
        assert!(
            is_fresh(false, Some(at(1.0)), at(1.5)),
            "a repeat of the key"
        );
        let other_window = Stamp {
            window: 2,
            ..at(1.0)
        };
        assert!(is_fresh(false, Some(at(1.0)), other_window));
        assert!(!is_fresh(true, None, at(1.0)), "during another event");
    }
}
