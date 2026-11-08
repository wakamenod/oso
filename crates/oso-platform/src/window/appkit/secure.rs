//! Secure event input from HIToolbox (Carbon), which keeps keystrokes for the app with the
//! keyboard focus, as a password field has it (ADR 0073). Only `State` calls these, through
//! `gate::SecureInput`, so that every enable has its disable.
//!
//! Must not depend on: the view, the delegates or the run loop's hooks.

use objc2_foundation::MainThreadMarker;

// HIToolbox is part of the Carbon framework, which AppKit loads already. No crate binds these.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn EnableSecureEventInput() -> i32;
    fn DisableSecureEventInput() -> i32;
    #[cfg(feature = "dev")]
    fn IsSecureEventInputEnabled() -> u8;
}

/// Enables secure input for this process. `false` if the system refused.
pub(super) fn enable(_mtm: MainThreadMarker) -> bool {
    // SAFETY: a Carbon call with no arguments, made on the main thread, which the marker
    // proves, and paired with `disable` by `gate::SecureInput`.
    unsafe { EnableSecureEventInput() == 0 }
}

/// Releases one enable of secure input. `false` if the system refused.
pub(super) fn disable(_mtm: MainThreadMarker) -> bool {
    // SAFETY: as for `enable`; called only for an enable this process holds.
    unsafe { DisableSecureEventInput() == 0 }
}

/// Whether any process has secure input on, for the dev socket's check only: the system's
/// flag, not oso's own.
#[cfg(feature = "dev")]
pub(super) fn any_enabled(_mtm: MainThreadMarker) -> bool {
    // SAFETY: a Carbon call with no arguments, which only reads, on the main thread.
    unsafe { IsSecureEventInputEnabled() != 0 }
}
