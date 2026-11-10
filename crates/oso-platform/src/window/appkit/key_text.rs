//! The text of an AppKit key event, read with AppKit: the key and its base key as oso takes
//! them (ADR 0033), which `keys` then routes and names with no AppKit call.
//!
//! Must not depend on: the view's state, the run loop or the delegates.

use objc2::rc::{AutoreleasePool, Retained};
use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use objc2_foundation::NSString;

use super::keys;
use crate::window::{Held, SystemKey};

/// The text of the key `event` is, and of its base key, borrowed for as long as `pool`
/// (ADR 0033). With Control or Command held, the key is what Shift alone types, or the
/// layout's other reading ([`keys::chosen_key`]). The base key is the key with no modifiers
/// with Control or Option held, and the key itself otherwise. Each reading is asked for once,
/// and the other only when it is needed.
pub(super) fn key_texts<'p>(
    event: &NSEvent,
    pool: AutoreleasePool<'p>,
) -> (Option<&'p str>, Option<&'p str>) {
    let flags = event.modifierFlags();
    let held = keys::held(flags);
    let shift = flags & NSEventModifierFlags::Shift;
    let needs_base = held.contains(Held::CONTROL) || held.contains(Held::ALT);
    let bare = || {
        text_in(
            event.charactersByApplyingModifiers(NSEventModifierFlags::empty()),
            pool,
        )
    };
    if !keys::shift_only(held) {
        let key = text_in(event.characters(), pool);
        return (key, if needs_base { bare() } else { key });
    }
    // With no Shift held, the key Shift alone types is the bare one.
    let unshifted = if needs_base || shift.is_empty() {
        bare()
    } else {
        None
    };
    let shifted = if shift.is_empty() {
        unshifted
    } else {
        text_in(event.charactersByApplyingModifiers(shift), pool)
    };
    let key = keys::chosen_key(shifted, || {
        let other = if held.contains(Held::LOGO) {
            event.charactersByApplyingModifiers(shift | NSEventModifierFlags::Command)
        } else {
            event.characters()
        };
        text_in(other, pool)
    });
    (key, if needs_base { unshifted } else { key })
}

/// `string`'s text, kept until `pool` ends.
fn text_in<'p>(string: Option<Retained<NSString>>, pool: AutoreleasePool<'p>) -> Option<&'p str> {
    let string = string?;
    // SAFETY: `pool` is the innermost autorelease pool here, as the view's `deliver_key` makes it, which
    // both calls need; `string` is kept in it, NSString's text does not change while it is
    // borrowed, and the key that borrows it lives inside the pool.
    Some(unsafe { Retained::autorelease(string, pool).to_str(pool) })
}

/// A key by the text it types.
pub(super) fn system_key(text: Option<&str>) -> SystemKey<'_> {
    match text {
        Some("") => SystemKey::Dead,
        Some(text) => SystemKey::Character(text),
        None => SystemKey::Unidentified,
    }
}
