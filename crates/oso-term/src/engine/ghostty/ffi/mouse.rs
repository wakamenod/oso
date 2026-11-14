//! The parts of libghostty-vt's `mouse/event.h` and `mouse/encoder.h` that oso-term uses, as
//! the release pinned in `ghostty-vt.lock` declares them (ADR 0120, step 4b-2): the encoder's
//! and the event's handles, the actions, the buttons, the options oso sets and the size
//! struct. `manifest_tests` checks every enumerator and the struct against the library's type
//! manifest.
//!
//! Must not depend on: anything in oso; only `raw` and `ghostty` read these.

use std::ffi::{c_int, c_void};
use std::mem::{offset_of, size_of};

use super::{Allocator, GResult, Mods, Opaque, Terminal};

/// `GhosttyMouseEncoder`.
pub type MouseEncoder = *mut Opaque;
/// `GhosttyMouseEvent`.
pub type MouseEvent = *mut Opaque;

// `GhosttyMouseAction`.
pub const MOUSE_ACTION_PRESS: c_int = 0;
pub const MOUSE_ACTION_RELEASE: c_int = 1;
pub const MOUSE_ACTION_MOTION: c_int = 2;

// `GhosttyMouseButton`: four to seven are the wheel's up, down, left and right.
pub const MOUSE_BUTTON_LEFT: c_int = 1;
pub const MOUSE_BUTTON_RIGHT: c_int = 2;
pub const MOUSE_BUTTON_MIDDLE: c_int = 3;
pub const MOUSE_BUTTON_FOUR: c_int = 4;
pub const MOUSE_BUTTON_FIVE: c_int = 5;
pub const MOUSE_BUTTON_SIX: c_int = 6;
pub const MOUSE_BUTTON_SEVEN: c_int = 7;

// `GhosttyMouseEncoderOption`: those oso sets. The tracking mode and the format come from
// the terminal (`set_from_terminal`).
pub const MOUSE_ENCODER_OPT_SIZE: c_int = 3;
pub const MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED: c_int = 4;

/// `GhosttyMousePosition`: surface pixels.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MousePosition {
    pub x: f32,
    pub y: f32,
}

/// `GhosttyMouseEncoderSize`, a sized struct: `size` holds its own size.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEncoderSize {
    pub size: usize,
    pub screen_width: u32,
    pub screen_height: u32,
    /// Must not be 0.
    pub cell_width: u32,
    /// Must not be 0.
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_right: u32,
    pub padding_left: u32,
}

impl MouseEncoderSize {
    /// A screen of `screen_width` × `screen_height` pixels in cells of `cell_width` ×
    /// `cell_height`, with no padding.
    pub const fn new(
        screen_width: u32,
        screen_height: u32,
        cell_width: u32,
        cell_height: u32,
    ) -> Self {
        MouseEncoderSize {
            size: size_of::<MouseEncoderSize>(),
            screen_width,
            screen_height,
            cell_width,
            cell_height,
            padding_top: 0,
            padding_bottom: 0,
            padding_right: 0,
            padding_left: 0,
        }
    }

    /// Each field's offset, for `manifest_tests`.
    pub const OFFSETS: [(&'static str, usize); 9] = [
        ("size", offset_of!(MouseEncoderSize, size)),
        ("screen_width", offset_of!(MouseEncoderSize, screen_width)),
        ("screen_height", offset_of!(MouseEncoderSize, screen_height)),
        ("cell_width", offset_of!(MouseEncoderSize, cell_width)),
        ("cell_height", offset_of!(MouseEncoderSize, cell_height)),
        ("padding_top", offset_of!(MouseEncoderSize, padding_top)),
        (
            "padding_bottom",
            offset_of!(MouseEncoderSize, padding_bottom),
        ),
        ("padding_right", offset_of!(MouseEncoderSize, padding_right)),
        ("padding_left", offset_of!(MouseEncoderSize, padding_left)),
    ];
}

unsafe extern "C" {
    pub fn ghostty_mouse_encoder_new(
        allocator: *const Allocator,
        encoder: *mut MouseEncoder,
    ) -> GResult;
    pub fn ghostty_mouse_encoder_free(encoder: MouseEncoder);
    pub fn ghostty_mouse_encoder_set(
        encoder: MouseEncoder,
        option: c_int,
        value: *const c_void,
    ) -> GResult;
    pub fn ghostty_mouse_encoder_set_from_terminal(
        encoder: MouseEncoder,
        terminal: Terminal,
    ) -> GResult;
    pub fn ghostty_mouse_encoder_encode(
        encoder: MouseEncoder,
        event: MouseEvent,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> GResult;
    pub fn ghostty_mouse_event_new(allocator: *const Allocator, event: *mut MouseEvent) -> GResult;
    pub fn ghostty_mouse_event_free(event: MouseEvent);
    pub fn ghostty_mouse_event_set_action(event: MouseEvent, action: c_int) -> GResult;
    pub fn ghostty_mouse_event_set_button(event: MouseEvent, button: c_int) -> GResult;
    pub fn ghostty_mouse_event_clear_button(event: MouseEvent) -> GResult;
    pub fn ghostty_mouse_event_set_mods(event: MouseEvent, mods: Mods) -> GResult;
    pub fn ghostty_mouse_event_set_position(event: MouseEvent, position: MousePosition) -> GResult;
}
