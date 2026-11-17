//! The keys AppKit's dispatch never sends to `keyDown:`, which the view takes where AppKit
//! does send them. For Command-`.`, the window sends `cancelOperation:` to the first responder
//! in its place, so `M-.` reaches oso only that way (ADR 0149); the event is AppKit's current
//! one, which stays current after its dispatch, so one is taken only once, and never while
//! another event is being handled, and a `cancelOperation:` with no such key, as from
//! Accessibility or a stray sender, is no key. macOS's "Show Help menu" key, Command-Shift on
//! the key with code 44 (`/` on ANSI and JIS layouts, so `M-?`), AppKit offers to the view's
//! `performKeyEquivalent:`, then takes for itself as a symbolic hot key, with or without a
//! Help menu, so that key reaches oso only from there (ADR 0152).
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
pub(super) fn cancel_key(
    view: &NSView,
    nested: bool,
    routed: Option<Stamp>,
) -> Option<Retained<NSEvent>> {
    let event = NSApplication::sharedApplication(view.mtm()).currentEvent()?;
    let (ours, fresh) = ours_and_fresh(view, &event, nested, routed)?;
    let held = keys::held(event.modifierFlags());
    keys::is_cancel_key(held, ours, fresh).then_some(event)
}

/// Whether `event`, which AppKit offers to `view`'s `performKeyEquivalent:`, is the "Show
/// Help menu" key AppKit would take next: a key-down with Command and Shift alone on the key
/// with code 44, for `view`'s window, that is not the key-down the view routed last
/// (`routed`), while no event is being handled (`nested`). The key is read first, as nearly
/// every key equivalent is another and needs no more reading.
pub(super) fn is_help_menu_key(
    view: &NSView,
    event: &NSEvent,
    nested: bool,
    routed: Option<Stamp>,
) -> bool {
    // A key code only a key event has: the type first.
    event.r#type() == NSEventType::KeyDown
        && keys::is_help_menu_key(keys::held(event.modifierFlags()), event.keyCode())
        && ours_and_fresh(view, event, nested, routed) == Some((true, true))
}

/// For a key-down, whether it is for `view`'s window, and whether it is new ([`is_fresh`]);
/// `None` for any other event.
fn ours_and_fresh(
    view: &NSView,
    event: &NSEvent,
    nested: bool,
    routed: Option<Stamp>,
) -> Option<(bool, bool)> {
    if event.r#type() != NSEventType::KeyDown {
        return None;
    }
    let ours = view
        .window()
        .is_some_and(|window| window.windowNumber() == event.windowNumber());
    Some((ours, is_fresh(nested, routed, stamp(event))))
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
