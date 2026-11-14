//! The mouse encoder's enums and structs `ffi` declares, against the linked library's type
//! manifest (ADR 0120, step 4b-2).
//!
//! Must not depend on: anything but `ffi` and the manifest helpers.

use std::mem::offset_of;

use super::super::ffi;
use super::{check_enum, check_struct, types};

#[test]
fn mouse_enums_match_the_manifest() {
    let t = types();
    check_enum(
        &t,
        "GhosttyMouseAction",
        &[
            ("PRESS", ffi::MOUSE_ACTION_PRESS),
            ("RELEASE", ffi::MOUSE_ACTION_RELEASE),
            ("MOTION", ffi::MOUSE_ACTION_MOTION),
        ],
    );
    check_enum(
        &t,
        "GhosttyMouseButton",
        &[
            ("LEFT", ffi::MOUSE_BUTTON_LEFT),
            ("RIGHT", ffi::MOUSE_BUTTON_RIGHT),
            ("MIDDLE", ffi::MOUSE_BUTTON_MIDDLE),
            ("FOUR", ffi::MOUSE_BUTTON_FOUR),
            ("FIVE", ffi::MOUSE_BUTTON_FIVE),
            ("SIX", ffi::MOUSE_BUTTON_SIX),
            ("SEVEN", ffi::MOUSE_BUTTON_SEVEN),
        ],
    );
    check_enum(
        &t,
        "GhosttyMouseEncoderOption",
        &[
            ("SIZE", ffi::MOUSE_ENCODER_OPT_SIZE),
            (
                "ANY_BUTTON_PRESSED",
                ffi::MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
            ),
        ],
    );
}

#[test]
fn mouse_structs_match_the_manifest() {
    let t = types();
    check_struct::<ffi::MousePosition>(
        &t,
        "GhosttyMousePosition",
        &[
            ("x", offset_of!(ffi::MousePosition, x)),
            ("y", offset_of!(ffi::MousePosition, y)),
        ],
    );
    check_struct::<ffi::MouseEncoderSize>(
        &t,
        "GhosttyMouseEncoderSize",
        &ffi::MouseEncoderSize::OFFSETS,
    );
}
