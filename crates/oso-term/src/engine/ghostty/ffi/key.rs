//! The parts of libghostty-vt's `key/event.h` and `key/encoder.h` that oso-term uses, as the
//! release pinned in `ghostty-vt.lock` declares them (ADR 0120): the encoder's and the event's
//! handles, the physical keys of a US layout, the modifier bits, the action and the one
//! encoder option oso sets. The functions are in `ffi`'s one `extern` block. `manifest_tests`
//! checks every enumerator here against the library's type manifest; the modifier bits are
//! `#define`s it leaves out, so a test reads each back through the encoder instead.
//!
//! Must not depend on: anything in oso; only `raw` and `keys` read these.

use std::ffi::c_int;

use super::Opaque;

/// `GhosttyKeyEncoder`.
pub type KeyEncoder = *mut Opaque;
/// `GhosttyKeyEvent`.
pub type KeyEvent = *mut Opaque;

/// A `GhosttyString` oso lends to the key event, which keeps its pointer: a type of its own,
/// so that `GString` stays empty or static text outside the library.
#[repr(C)]
pub struct KeyText {
    pub ptr: *const u8,
    pub len: usize,
}

// `GhosttyKeyAction`.
pub const KEY_ACTION_PRESS: c_int = 1;

/// `GhosttyMods`, a `uint16_t` of `GHOSTTY_MODS_*` bits.
pub type Mods = u16;
pub const MODS_SHIFT: Mods = 1 << 0;
pub const MODS_CTRL: Mods = 1 << 1;
pub const MODS_ALT: Mods = 1 << 2;
pub const MODS_SUPER: Mods = 1 << 3;

// `GhosttyKeyEncoderOption`, and the `GhosttyOptionAsAlt` value oso gives it.
pub const KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT: c_int = 7;
pub const OPTION_AS_ALT_TRUE: c_int = 1;

/// `GhosttyKey`. Letters, digits and function keys run in order: `KEY_A` to `KEY_Z`,
/// `KEY_DIGIT_0` to `KEY_DIGIT_9`, `KEY_F1` to `KEY_F25`.
pub type Key = c_int;
pub const KEY_UNIDENTIFIED: Key = 0;
pub const KEY_BACKQUOTE: Key = 1;
pub const KEY_BACKSLASH: Key = 2;
pub const KEY_BRACKET_LEFT: Key = 3;
pub const KEY_BRACKET_RIGHT: Key = 4;
pub const KEY_COMMA: Key = 5;
pub const KEY_DIGIT_0: Key = 6;
pub const KEY_EQUAL: Key = 16;
pub const KEY_A: Key = 20;
pub const KEY_MINUS: Key = 46;
pub const KEY_PERIOD: Key = 47;
pub const KEY_QUOTE: Key = 48;
pub const KEY_SEMICOLON: Key = 49;
pub const KEY_SLASH: Key = 50;
pub const KEY_BACKSPACE: Key = 53;
pub const KEY_ENTER: Key = 58;
pub const KEY_SPACE: Key = 63;
pub const KEY_TAB: Key = 64;
pub const KEY_DELETE: Key = 68;
pub const KEY_END: Key = 69;
pub const KEY_HOME: Key = 71;
pub const KEY_INSERT: Key = 72;
pub const KEY_PAGE_DOWN: Key = 73;
pub const KEY_PAGE_UP: Key = 74;
pub const KEY_ARROW_DOWN: Key = 75;
pub const KEY_ARROW_LEFT: Key = 76;
pub const KEY_ARROW_RIGHT: Key = 77;
pub const KEY_ARROW_UP: Key = 78;
pub const KEY_ESCAPE: Key = 120;
pub const KEY_F1: Key = 121;
/// The last function key the library knows.
pub const KEY_F_LAST: u8 = 25;
