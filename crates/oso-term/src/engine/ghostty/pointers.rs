//! The mouse's `Pointer` as libghostty-vt's mouse event (ADR 0120, step 4b-2): the action, the
//! button, the modifiers as keys have them, and the place in pixels, kept inside the cell the
//! UI found, so that the library's cell and the UI's agree whatever their rounding. The size
//! is the PTY's, whose cells are whole pixels, at least one.
//!
//! Must not depend on: the policy or the thread. This file is safe code over `ffi`'s
//! constants.

use oso_platform::process::pty::Size;

use super::ffi;
use super::keys;
use super::raw::PointerInput;
use crate::snapshot::{Button, Pointer, PointerAction};

/// The mouse event for `pointer`, with `held` the button held down after it, if any.
pub(super) fn input(pointer: Pointer, held: Option<Button>, size: Size) -> PointerInput {
    let columns = u32::from(size.columns.max(1));
    let rows = u32::from(size.rows.max(1));
    let cell_width = (u32::from(size.width) / columns).max(1);
    let cell_height = (u32::from(size.height) / rows).max(1);
    // The screen holds every cell, though the PTY was given no pixels.
    let screen_width = u32::from(size.width).max(cell_width * columns);
    let screen_height = u32::from(size.height).max(cell_height * rows);
    let inside = |at: u32, cell: u16, cells: u32, per: u32| {
        let start = u32::from(cell).min(cells - 1) * per;
        pixel(at.clamp(start, start + per - 1))
    };
    let (action, button) = match pointer.action {
        PointerAction::Press(button) => (ffi::MOUSE_ACTION_PRESS, Some(button)),
        PointerAction::Release(button) => (ffi::MOUSE_ACTION_RELEASE, Some(button)),
        PointerAction::Motion => (ffi::MOUSE_ACTION_MOTION, held),
    };
    PointerInput {
        action,
        button: button.map(code),
        mods: keys::mods(pointer.modifiers),
        position: ffi::MousePosition {
            x: inside(pointer.x, pointer.column, columns, cell_width),
            y: inside(pointer.y, pointer.row, rows, cell_height),
        },
        any_pressed: held.is_some(),
        size: ffi::MouseEncoderSize::new(screen_width, screen_height, cell_width, cell_height),
    }
}

/// The library's number for `button`: the wheel's notches are buttons four to seven.
fn code(button: Button) -> std::ffi::c_int {
    match button {
        Button::Left => ffi::MOUSE_BUTTON_LEFT,
        Button::Middle => ffi::MOUSE_BUTTON_MIDDLE,
        Button::Right => ffi::MOUSE_BUTTON_RIGHT,
        Button::WheelUp => ffi::MOUSE_BUTTON_FOUR,
        Button::WheelDown => ffi::MOUSE_BUTTON_FIVE,
        Button::WheelLeft => ffi::MOUSE_BUTTON_SIX,
        Button::WheelRight => ffi::MOUSE_BUTTON_SEVEN,
    }
}

/// A pixel as the library's float: the PTY's pixels are at most `u16::MAX` × a cell's few,
/// and any past `u16` stay there, which only says the far edge.
fn pixel(at: u32) -> f32 {
    f32::from(u16::try_from(at).unwrap_or(u16::MAX))
}

#[cfg(test)]
mod tests;
