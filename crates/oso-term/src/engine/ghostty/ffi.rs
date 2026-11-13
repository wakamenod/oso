//! Hand-written declarations of the parts of libghostty-vt's C API that oso-term calls, as the
//! release pinned in `ghostty-vt.lock` declares them in `include/ghostty/vt/*.h` (ADR 0120).
//! Every C enum is `GHOSTTY_ENUM_TYPED`, an `int` (types.h), so enum values and fields are
//! `c_int` here, with the enumerators as constants. `manifest_tests` checks every struct and
//! constant below against the linked library's own type manifest.
//!
//! Must not depend on: anything in oso; only `raw` calls these.

#![expect(unsafe_code, reason = "the library's C API (ADR 0120)")]
// Some declarations are here only so that `manifest_tests` checks them: the options oso never
// sets, and the accessors the tests compare `layout` with.
#![cfg_attr(
    not(test),
    allow(dead_code, reason = "declarations that only manifest_tests reads")
)]

use std::ffi::{c_char, c_int, c_void};
use std::marker::{PhantomData, PhantomPinned};

/// An opaque library object, only ever behind a pointer.
#[repr(C)]
pub struct Opaque {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `GhosttyTerminal`.
pub type Terminal = *mut Opaque;
/// `GhosttyRenderState`.
pub type RenderState = *mut Opaque;
/// `GhosttyRenderStateRowIterator`.
pub type RowIterator = *mut Opaque;
/// `GhosttyRenderStateRowCells`.
pub type RowCells = *mut Opaque;
/// `GhosttyAllocator`; oso always passes NULL, the library's default allocator.
pub type Allocator = Opaque;

// ---- types.h ----

/// `GhosttyResult`.
pub type GResult = c_int;
pub const SUCCESS: GResult = 0;
pub const OUT_OF_SPACE: GResult = -3;

/// `GhosttyString`: borrowed bytes. Its fields are private, so that outside the library a
/// value is only ever empty or static text; how long the library's own stay readable is up
/// to the caller of `raw`'s `bytes`, an `unsafe fn` for that reason.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GString {
    ptr: *const u8,
    len: usize,
}

impl GString {
    pub const EMPTY: GString = GString {
        ptr: std::ptr::null(),
        len: 0,
    };

    /// Text that lives as long as the process, for the library to copy.
    pub const fn from_static(text: &'static str) -> GString {
        GString {
            ptr: text.as_ptr(),
            len: text.len(),
        }
    }

    pub fn parts(&self) -> (*const u8, usize) {
        (self.ptr, self.len)
    }

    /// The offsets of `ptr` and `len`, for `manifest_tests`.
    #[cfg(test)]
    pub const OFFSETS: (usize, usize) = (
        std::mem::offset_of!(GString, ptr),
        std::mem::offset_of!(GString, len),
    );
}

/// `GhosttyBuffer`: an output buffer the caller owns.
#[repr(C)]
pub struct GBuffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

// `GhosttyCursorStyle`.
pub const CURSOR_STYLE_BAR: c_int = 0;
pub const CURSOR_STYLE_BLOCK: c_int = 1;
pub const CURSOR_STYLE_UNDERLINE: c_int = 2;
pub const CURSOR_STYLE_BLOCK_HOLLOW: c_int = 3;

// ---- color.h ----

/// `GhosttyColorRgb`.
#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

// ---- style.h ----

// `GhosttyStyleColorTag`, held in a `uint8_t`.
pub const STYLE_COLOR_NONE: u8 = 0;
pub const STYLE_COLOR_PALETTE: u8 = 1;
pub const STYLE_COLOR_RGB: u8 = 2;

/// `GhosttyStyleColor`: a tag and a three-byte union (a palette index, or r, g, b).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StyleColor {
    pub tag: u8,
    pub value: [u8; 3],
}

// `GhosttyStyleFlags` bits; bits 8 to 10 hold the underline style.
pub const STYLE_FLAG_BOLD: u16 = 1 << 0;
pub const STYLE_FLAG_ITALIC: u16 = 1 << 1;
pub const STYLE_FLAG_FAINT: u16 = 1 << 2;
pub const STYLE_FLAG_BLINK: u16 = 1 << 3;
pub const STYLE_FLAG_INVERSE: u16 = 1 << 4;
pub const STYLE_FLAG_INVISIBLE: u16 = 1 << 5;
pub const STYLE_FLAG_STRIKETHROUGH: u16 = 1 << 6;
pub const STYLE_FLAG_OVERLINE: u16 = 1 << 7;
pub const STYLE_UNDERLINE_SHIFT: u16 = 8;
pub const STYLE_UNDERLINE_MASK: u16 = 0x7 << STYLE_UNDERLINE_SHIFT;

// `GhosttySgrUnderline`, the values bits 8 to 10 hold.
pub const UNDERLINE_NONE: c_int = 0;
pub const UNDERLINE_SINGLE: c_int = 1;
pub const UNDERLINE_DOUBLE: c_int = 2;
pub const UNDERLINE_CURLY: c_int = 3;
pub const UNDERLINE_DOTTED: c_int = 4;
pub const UNDERLINE_DASHED: c_int = 5;

/// `GhosttyStyle`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Style {
    pub fg_color: StyleColor,
    pub bg_color: StyleColor,
    pub underline_color: StyleColor,
    pub flags: u16,
    pub reserved: u16,
}

impl Style {
    pub const DEFAULT: Style = {
        let none = StyleColor {
            tag: STYLE_COLOR_NONE,
            value: [0; 3],
        };
        Style {
            fg_color: none,
            bg_color: none,
            underline_color: none,
            flags: 0,
            reserved: 0,
        }
    };
}

// ---- screen.h ----

/// `GhosttyCell`: a packed cell, decoded with `layout`'s bit positions.
pub type Cell = u64;
/// `GhosttyRow`: a row, read with `ghostty_row_get`.
pub type Row = u64;

// `GhosttyRowData`.
pub const ROW_DATA_WRAP: c_int = 1;

/// `GhosttyCellsView`.
#[repr(C)]
pub struct CellsView {
    pub ptr: *const Cell,
    pub len: usize,
}

// `GhosttyCellContentTag`.
pub const CELL_CONTENT_CODEPOINT: c_int = 0;
pub const CELL_CONTENT_CODEPOINT_GRAPHEME: c_int = 1;
pub const CELL_CONTENT_BG_COLOR_PALETTE: c_int = 2;
pub const CELL_CONTENT_BG_COLOR_RGB: c_int = 3;

// `GhosttyCellWide`.
pub const CELL_WIDE_NARROW: c_int = 0;
pub const CELL_WIDE_WIDE: c_int = 1;
pub const CELL_WIDE_SPACER_TAIL: c_int = 2;
pub const CELL_WIDE_SPACER_HEAD: c_int = 3;

// `GhosttyCellData`, for the layout test's cross-check.
pub const CELL_DATA_CODEPOINT: c_int = 1;
pub const CELL_DATA_CONTENT_TAG: c_int = 2;
pub const CELL_DATA_WIDE: c_int = 3;
pub const CELL_DATA_STYLE_ID: c_int = 6;
pub const CELL_DATA_COLOR_PALETTE: c_int = 10;

// ---- size_report.h ----

/// `GhosttySizeReportSize`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

// ---- modes.h ----

/// `GhosttyMode`: the mode's number, with bit 15 set for an ANSI mode (`ghostty_mode_new`).
pub type Mode = u16;

/// A DEC private mode, as `ghostty_mode_new(value, false)` makes it.
pub const fn dec_mode(value: u16) -> Mode {
    value & 0x7FFF
}

// ---- terminal.h ----

/// `GhosttyTerminalModeConfig`.
#[repr(C)]
pub struct ModeConfig {
    pub mode: Mode,
    pub value: bool,
}

// `GhosttyTerminalScreen`.
pub const TERMINAL_SCREEN_PRIMARY: c_int = 0;
pub const TERMINAL_SCREEN_ALTERNATE: c_int = 1;

// `GhosttyTerminalUnknownSequenceTag`.
pub const UNKNOWN_SEQUENCE_APC: c_int = 0;
pub const UNKNOWN_SEQUENCE_OSC: c_int = 1;

/// `GhosttyTerminalUnknownStringSequence`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UnknownStringSequence {
    pub truncated: bool,
    pub content: GString,
}

/// `GhosttyTerminalUnknownOscSequence`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct UnknownOscSequence {
    pub truncated: bool,
    pub content: GString,
    pub terminator: c_int,
}

/// `GhosttyTerminalUnknownSequenceValue`.
#[repr(C)]
#[derive(Clone, Copy)]
pub union UnknownSequenceValue {
    pub apc: UnknownStringSequence,
    pub osc: UnknownOscSequence,
    pub padding: [u64; 16],
}

/// `GhosttyTerminalUnknownSequence`.
#[repr(C)]
pub struct UnknownSequence {
    pub tag: c_int,
    pub value: UnknownSequenceValue,
}

/// `GhosttyClipboardContent`.
#[repr(C)]
pub struct ClipboardContent {
    pub mime: GString,
    pub data: GString,
}

// `GhosttyClipboardWriteResult`.
pub const CLIPBOARD_WRITE_RESULT_SUCCESS: c_int = 0;

/// `GhosttyClipboardWriteReply` (a sized struct).
#[repr(C)]
pub struct ClipboardWriteReply {
    pub size: usize,
    pub result: c_int,
    pub remember: bool,
}

/// `GhosttyClipboardWriteReplyFn`.
pub type ClipboardWriteReplyFn =
    unsafe extern "C" fn(write: *const ClipboardWrite, reply: *const ClipboardWriteReply);

/// `GhosttyClipboardWrite`.
#[repr(C)]
pub struct ClipboardWrite {
    pub location: c_int,
    pub contents: *const ClipboardContent,
    pub contents_len: usize,
    pub name: GString,
    pub granted: bool,
    pub can_remember: bool,
    pub ctx: *const c_void,
    pub reply: Option<ClipboardWriteReplyFn>,
}

/// `GhosttyDesktopNotification`.
#[repr(C)]
pub struct DesktopNotification {
    pub title: GString,
    pub body: GString,
}

// `GhosttyProgressState`.
pub const PROGRESS_REMOVE: c_int = 0;
pub const PROGRESS_SET: c_int = 1;
pub const PROGRESS_ERROR: c_int = 2;
pub const PROGRESS_INDETERMINATE: c_int = 3;
pub const PROGRESS_PAUSE: c_int = 4;

/// `GhosttyProgressReport`; `progress` is -1 when the sequence gave no percent.
#[repr(C)]
pub struct ProgressReport {
    pub state: c_int,
    pub progress: i8,
}

// `GhosttySemanticPromptKind`.
pub const SEMANTIC_PROMPT_PROMPT_START: c_int = 1;
pub const SEMANTIC_PROMPT_INPUT_START: c_int = 2;
pub const SEMANTIC_PROMPT_OUTPUT_START: c_int = 3;
pub const SEMANTIC_PROMPT_COMMAND_END: c_int = 4;

/// `GhosttySemanticPrompt`.
#[repr(C)]
pub struct SemanticPrompt {
    pub kind: c_int,
    pub prompt_kind: c_int,
    pub has_exit_code: bool,
    pub exit_code: i32,
    pub command: GString,
    pub error: GString,
}

// The callbacks oso installs (terminal.h). The first argument is the terminal, the second
// the userdata. The library never gets a clipboard-read, enquiry or window-resize callback.
pub type WritePtyFn = extern "C" fn(Terminal, *mut c_void, *const u8, usize);
pub type BellFn = extern "C" fn(Terminal, *mut c_void);
pub type XtversionFn = extern "C" fn(Terminal, *mut c_void) -> GString;
pub type ChangedFn = extern "C" fn(Terminal, *mut c_void);
pub type SizeFn = extern "C" fn(Terminal, *mut c_void, *mut SizeReportSize) -> bool;
pub type ClipboardWriteFn = extern "C" fn(Terminal, *mut c_void, *const ClipboardWrite);
pub type DesktopNotificationFn = extern "C" fn(Terminal, *mut c_void, *const DesktopNotification);
pub type ProgressReportFn = extern "C" fn(Terminal, *mut c_void, *const ProgressReport);
pub type SemanticPromptFn = extern "C" fn(Terminal, *mut c_void, *const SemanticPrompt);
pub type UnknownSequenceFn = extern "C" fn(Terminal, *mut c_void, *const UnknownSequence);

// `GhosttyTerminalOption`. 1.0 starts at `OPT_INVALID` = 0.
pub const OPT_USERDATA: c_int = 1;
pub const OPT_WRITE_PTY: c_int = 2;
pub const OPT_BELL: c_int = 3;
pub const OPT_ENQUIRY: c_int = 4;
pub const OPT_XTVERSION: c_int = 5;
pub const OPT_TITLE_CHANGED: c_int = 6;
pub const OPT_SIZE: c_int = 7;
pub const OPT_DEVICE_ATTRIBUTES: c_int = 9;
pub const OPT_COLOR_FOREGROUND: c_int = 12;
pub const OPT_COLOR_BACKGROUND: c_int = 13;
pub const OPT_COLOR_CURSOR: c_int = 14;
pub const OPT_COLOR_PALETTE: c_int = 15;
pub const OPT_KITTY_IMAGE_STORAGE_LIMIT: c_int = 16;
pub const OPT_KITTY_IMAGE_MEDIUM_FILE: c_int = 17;
pub const OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE: c_int = 18;
pub const OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM: c_int = 19;
pub const OPT_APC_MAX_BYTES_KITTY: c_int = 21;
pub const OPT_PWD_CHANGED: c_int = 26;
pub const OPT_CLIPBOARD_WRITE: c_int = 27;
pub const OPT_SCROLLBACK_MAX_BYTES: c_int = 28;
pub const OPT_SCROLLBACK_MAX_LINES: c_int = 29;
pub const OPT_DESKTOP_NOTIFICATION: c_int = 30;
pub const OPT_PROGRESS_REPORT: c_int = 31;
pub const OPT_TITLE_REPORT: c_int = 33;
pub const OPT_UNKNOWN_SEQUENCE: c_int = 36;
pub const OPT_UNKNOWN_MAX_BYTES: c_int = 37;
pub const OPT_CLIPBOARD_READ: c_int = 39;
pub const OPT_CLIPBOARD_WRITE_MAX_BYTES: c_int = 40;
pub const OPT_SEMANTIC_PROMPT: c_int = 43;
pub const OPT_XT_CHECKSUM_REPORT: c_int = 45;
pub const OPT_RESIZE_WINDOW: c_int = 48;

// `GhosttyTerminalData`.
pub const DATA_ACTIVE_SCREEN: c_int = 6;
pub const DATA_KITTY_KEYBOARD_FLAGS: c_int = 8;
pub const DATA_TITLE: c_int = 12;
pub const DATA_PWD: c_int = 13;
pub const DATA_SCROLLBACK_ROWS: c_int = 15;
pub const DATA_KITTY_IMAGE_STORAGE_LIMIT: c_int = 26;
pub const DATA_KITTY_IMAGE_MEDIUM_FILE: c_int = 27;
pub const DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM: c_int = 29;
pub const DATA_SCROLLBACK_MAX_BYTES: c_int = 34;
pub const DATA_MODE: c_int = 37;
pub const DATA_CLIPBOARD_WRITE_MAX_BYTES: c_int = 40;

// ---- render.h ----

// `GhosttyRenderStateDirty`.
pub const RENDER_DIRTY_FALSE: c_int = 0;
pub const RENDER_DIRTY_PARTIAL: c_int = 1;
pub const RENDER_DIRTY_FULL: c_int = 2;

// `GhosttyRenderStateData`.
pub const RS_DATA_COLS: c_int = 1;
pub const RS_DATA_ROWS: c_int = 2;
pub const RS_DATA_DIRTY: c_int = 3;
pub const RS_DATA_ROW_ITERATOR: c_int = 4;
pub const RS_DATA_CURSOR: c_int = 18;

// `GhosttyRenderStateRowData`.
pub const ROW_DATA_DIRTY: c_int = 1;
pub const ROW_DATA_RAW: c_int = 2;
pub const ROW_DATA_CELLS: c_int = 3;
pub const ROW_DATA_CELLS_RAW: c_int = 5;

// `GhosttyRenderStateRowCellsData`.
pub const CELLS_DATA_STYLE: c_int = 2;
pub const CELLS_DATA_GRAPHEMES_UTF8: c_int = 9;

/// `GhosttyRenderStateCursor` (a sized struct).
#[repr(C)]
pub struct RenderCursor {
    pub size: usize,
    pub viewport_has_value: bool,
    pub viewport_x: u16,
    pub viewport_y: u16,
    pub wide_tail: bool,
    pub visible: bool,
    pub blinking: bool,
    pub password_input: bool,
    pub visual_style: c_int,
}

impl RenderCursor {
    pub fn sized() -> RenderCursor {
        RenderCursor {
            size: size_of::<RenderCursor>(),
            viewport_has_value: false,
            viewport_x: 0,
            viewport_y: 0,
            wide_tail: false,
            visible: false,
            blinking: false,
            password_input: false,
            visual_style: CURSOR_STYLE_BLOCK,
        }
    }
}

// ---- build_info.h ----

// `GhosttyBuildInfo`.
pub const BUILD_INFO_KITTY_GRAPHICS: c_int = 2;
pub const BUILD_INFO_OPTIMIZE: c_int = 4;
pub const BUILD_INFO_VERSION_STRING: c_int = 5;

unsafe extern "C" {
    pub fn ghostty_terminal_new(
        allocator: *const Allocator,
        terminal: *mut Terminal,
        cols: u16,
        rows: u16,
    ) -> GResult;
    pub fn ghostty_terminal_free(terminal: Terminal);
    pub fn ghostty_terminal_resize(
        terminal: Terminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> GResult;
    pub fn ghostty_terminal_set(terminal: Terminal, opt: c_int, value: *const c_void) -> GResult;
    pub fn ghostty_terminal_vt_write(terminal: Terminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_get(terminal: Terminal, data: c_int, out: *mut c_void) -> GResult;

    pub fn ghostty_render_state_new(
        allocator: *const Allocator,
        state: *mut RenderState,
    ) -> GResult;
    pub fn ghostty_render_state_free(state: RenderState);
    pub fn ghostty_render_state_update(state: RenderState, terminal: Terminal) -> GResult;
    pub fn ghostty_render_state_clean(state: RenderState) -> GResult;
    pub fn ghostty_render_state_get(state: RenderState, data: c_int, out: *mut c_void) -> GResult;

    pub fn ghostty_render_state_row_iterator_new(
        allocator: *const Allocator,
        iterator: *mut RowIterator,
    ) -> GResult;
    pub fn ghostty_render_state_row_iterator_free(iterator: RowIterator);
    pub fn ghostty_render_state_row_iterator_next(iterator: RowIterator) -> bool;
    pub fn ghostty_render_state_row_get(
        iterator: RowIterator,
        data: c_int,
        out: *mut c_void,
    ) -> GResult;

    pub fn ghostty_render_state_row_cells_new(
        allocator: *const Allocator,
        cells: *mut RowCells,
    ) -> GResult;
    pub fn ghostty_render_state_row_cells_free(cells: RowCells);
    pub fn ghostty_render_state_row_cells_select(cells: RowCells, x: u16) -> GResult;
    pub fn ghostty_render_state_row_cells_get(
        cells: RowCells,
        data: c_int,
        out: *mut c_void,
    ) -> GResult;

    pub fn ghostty_cell_get(cell: Cell, data: c_int, out: *mut c_void) -> GResult;
    pub fn ghostty_row_get(row: Row, data: c_int, out: *mut c_void) -> GResult;
    pub fn ghostty_build_info(data: c_int, out: *mut c_void) -> GResult;
    pub fn ghostty_type_json() -> *const c_char;
}
