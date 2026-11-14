//! The mouse encoder and the one mouse event it encodes, which `Vt` owns beside its key
//! encoder (ADR 0120, step 4b-2). Each foreign call sits in an `unsafe` block of its own with
//! its `// SAFETY:` comment; the handles live and die on the terminal's thread, as `Vt` does.
//!
//! Must not depend on: the policy, the thread or the snapshot's types.

#![expect(unsafe_code, reason = "the library's C API (ADR 0120)")]

use std::ffi::c_int;
use std::ptr;

use super::super::ffi;
use super::Vt;

/// One mouse event as the encoder takes it, made from a `Pointer` by `pointers`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in super::super) struct PointerInput {
    pub action: c_int,
    /// None for a motion with no button held.
    pub button: Option<c_int>,
    pub mods: ffi::Mods,
    pub position: ffi::MousePosition,
    /// A button is held after this event.
    pub any_pressed: bool,
    pub size: ffi::MouseEncoderSize,
}

/// The mouse encoder's and event's handles, null until made.
pub(super) struct Handles {
    encoder: ffi::MouseEncoder,
    event: ffi::MouseEvent,
}

impl Handles {
    pub(super) const NONE: Handles = Handles {
        encoder: ptr::null_mut(),
        event: ptr::null_mut(),
    };

    /// Makes both handles; false when the library cannot allocate one.
    pub(super) fn make(&mut self) -> bool {
        // SAFETY: a NULL allocator is the library's default; `encoder` is written on success.
        let made = unsafe { ffi::ghostty_mouse_encoder_new(ptr::null(), &mut self.encoder) };
        if made != ffi::SUCCESS {
            return false;
        }
        // SAFETY: as above, for the event.
        let made = unsafe { ffi::ghostty_mouse_event_new(ptr::null(), &mut self.event) };
        made == ffi::SUCCESS
    }
}

impl Drop for Handles {
    fn drop(&mut self) {
        if !self.event.is_null() {
            // SAFETY: made by `make` and freed once, here.
            unsafe { ffi::ghostty_mouse_event_free(self.event) };
        }
        if !self.encoder.is_null() {
            // SAFETY: as above.
            unsafe { ffi::ghostty_mouse_encoder_free(self.encoder) };
        }
    }
}

impl Vt {
    /// Encodes `pointer` into `out` against the terminal's mouse modes as they are now: the
    /// tracking mode (9, 1000, 1002, 1003) and the format (X10, 1005, 1006, 1015, 1016).
    /// Returns the bytes written: 0 when the modes want no report, it would not fit, or the
    /// library refused a setting, so a report made with a stale one never goes.
    pub fn encode_pointer(&mut self, pointer: &PointerInput, out: &mut [u8]) -> usize {
        let mouse = &self.mouse;
        // SAFETY: both handles are live and this thread's alone. The library copies the
        // terminal's tracking mode and format into the encoder and keeps no pointer to the
        // terminal, so `Handles` may outlive it, as it does in `Vt`'s drop.
        let from_terminal =
            unsafe { ffi::ghostty_mouse_encoder_set_from_terminal(mouse.encoder, self.terminal) };
        // SAFETY: the encoder is live, and the value points to a live
        // `GhosttyMouseEncoderSize` whose `size` is its own and whose cells are not 0
        // (`pointers` makes them at least 1); the library copies it.
        let size = unsafe {
            ffi::ghostty_mouse_encoder_set(
                mouse.encoder,
                ffi::MOUSE_ENCODER_OPT_SIZE,
                (&raw const pointer.size).cast(),
            )
        };
        // SAFETY: the encoder is live, and the value points to a live `bool`, which the
        // library copies.
        let any_pressed = unsafe {
            ffi::ghostty_mouse_encoder_set(
                mouse.encoder,
                ffi::MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&raw const pointer.any_pressed).cast(),
            )
        };
        // SAFETY: the event is live; the action is a `GhosttyMouseAction` (`pointers` makes
        // only those).
        let action = unsafe { ffi::ghostty_mouse_event_set_action(mouse.event, pointer.action) };
        let button = match pointer.button {
            // SAFETY: the event is live; the button is a `GhosttyMouseButton`.
            Some(button) => unsafe { ffi::ghostty_mouse_event_set_button(mouse.event, button) },
            // SAFETY: the event is live.
            None => unsafe { ffi::ghostty_mouse_event_clear_button(mouse.event) },
        };
        // SAFETY: the event is live; any `GhosttyMods` is valid.
        let mods = unsafe { ffi::ghostty_mouse_event_set_mods(mouse.event, pointer.mods) };
        // SAFETY: the event is live; the position is passed by value.
        let position =
            unsafe { ffi::ghostty_mouse_event_set_position(mouse.event, pointer.position) };
        let set = [
            from_terminal,
            size,
            any_pressed,
            action,
            button,
            mods,
            position,
        ];
        if set.iter().any(|result| *result != ffi::SUCCESS) {
            return 0;
        }
        let mut written = 0;
        // SAFETY: both handles are live; `out` is writable for `out.len()` bytes, of which
        // the library writes at most that many, reporting how many in `written`.
        let encoded = unsafe {
            ffi::ghostty_mouse_encoder_encode(
                mouse.encoder,
                mouse.event,
                out.as_mut_ptr(),
                out.len(),
                &mut written,
            )
        };
        if encoded == ffi::SUCCESS {
            written.min(out.len())
        } else {
            0
        }
    }
}
