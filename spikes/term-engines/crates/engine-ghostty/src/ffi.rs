//! Hand-written declarations for the parts of the libghostty-vt C API this crate uses.
//!
//! Each item mirrors a declaration in `include/ghostty/vt/*.h` of the libghostty-vt 1.0.1
//! release. Every C enum is declared `GHOSTTY_ENUM_TYPED`, which is `: int` (types.h), so
//! enum-typed values and fields are `c_int` here, with the enumerators as constants. The
//! one exception is `GhosttyStyleColor::tag`, a `uint8_t` holding a `GhosttyStyleColorTag`.
//! The tests at the bottom check every struct and enumerator against the linked library's
//! own ABI manifest.

#![allow(dead_code)]

use std::ffi::{c_int, c_void};
use std::marker::{PhantomData, PhantomPinned};

macro_rules! opaque {
    ($name:ident) => {
        #[repr(C)]
        pub struct $name {
            _data: [u8; 0],
            _marker: PhantomData<(*mut u8, PhantomPinned)>,
        }
    };
}

opaque!(TerminalImpl);
opaque!(RenderStateImpl);
opaque!(RowIteratorImpl);
opaque!(RowCellsImpl);

/// `GhosttyTerminal` (types.h).
pub type Terminal = *mut TerminalImpl;
/// `GhosttyRenderState` (types.h).
pub type RenderState = *mut RenderStateImpl;
/// `GhosttyRenderStateRowIterator` (types.h).
pub type RowIterator = *mut RowIteratorImpl;
/// `GhosttyRenderStateRowCells` (types.h).
pub type RowCells = *mut RowCellsImpl;

// ---- types.h ----

/// `GhosttyResult`.
pub type GResult = c_int;
pub const SUCCESS: GResult = 0;
pub const OUT_OF_SPACE: GResult = -3;

/// `GhosttyString`: borrowed bytes.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GString {
    pub ptr: *const u8,
    pub len: usize,
}

impl GString {
    pub const EMPTY: GString = GString {
        ptr: std::ptr::null(),
        len: 0,
    };
}

/// `GhosttyBuffer`: a caller-provided output buffer.
#[repr(C)]
pub struct GBuffer {
    pub ptr: *mut u8,
    pub cap: usize,
    pub len: usize,
}

// ---- color.h ----

/// `GhosttyColorRgb`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ColorRgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

// ---- style.h ----

pub const STYLE_COLOR_NONE: u8 = 0;
pub const STYLE_COLOR_PALETTE: u8 = 1;
pub const STYLE_COLOR_RGB: u8 = 2;

/// `GhosttyStyleColor`: a one-byte tag, then the three-byte `GhosttyStyleColorValue`
/// union (`uint8_t palette` or `GhosttyColorRgb rgb`, both at offset 0), read here as
/// bytes: `value[0]` is the palette index, `value[0..3]` are r, g, b.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct StyleColor {
    pub tag: u8,
    pub value: [u8; 3],
}

// GhosttyStyleFlags bits (style.h).
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

/// `GhosttyStyle`: fixed 16-byte layout, no size field.
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
    /// The default style: all zero bytes (style.h).
    pub fn zeroed() -> Style {
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
    }
}

// ---- screen.h ----

/// `GhosttyCell`: a packed cell, read only through `ghostty_cell_get*`.
pub type Cell = u64;

/// `GhosttyCellsView`.
#[repr(C)]
pub struct CellsView {
    pub ptr: *const Cell,
    pub len: usize,
}

// GhosttyCellContentTag
pub const CELL_CONTENT_CODEPOINT: c_int = 0;
pub const CELL_CONTENT_CODEPOINT_GRAPHEME: c_int = 1;
pub const CELL_CONTENT_BG_COLOR_PALETTE: c_int = 2;
pub const CELL_CONTENT_BG_COLOR_RGB: c_int = 3;

// GhosttyCellWide
pub const CELL_WIDE_NARROW: c_int = 0;
pub const CELL_WIDE_WIDE: c_int = 1;
pub const CELL_WIDE_SPACER_TAIL: c_int = 2;
pub const CELL_WIDE_SPACER_HEAD: c_int = 3;

// GhosttyCellData
pub const CELL_DATA_CODEPOINT: c_int = 1; // uint32_t
pub const CELL_DATA_CONTENT_TAG: c_int = 2; // GhosttyCellContentTag
pub const CELL_DATA_WIDE: c_int = 3; // GhosttyCellWide
pub const CELL_DATA_HAS_STYLING: c_int = 5; // bool
pub const CELL_DATA_STYLE_ID: c_int = 6; // uint16_t
pub const CELL_DATA_HAS_HYPERLINK: c_int = 7; // bool
pub const CELL_DATA_COLOR_PALETTE: c_int = 10; // uint8_t
pub const CELL_DATA_COLOR_RGB: c_int = 11; // GhosttyColorRgb

// ---- osc.h ----

// GhosttyOscTerminator
pub const OSC_TERMINATOR_ST: c_int = 0;
pub const OSC_TERMINATOR_BEL: c_int = 1;

// ---- device.h ----

/// `GhosttyDeviceAttributesPrimary`.
#[repr(C)]
pub struct DeviceAttributesPrimary {
    pub conformance_level: u16,
    pub features: [u16; 64],
    pub num_features: usize,
}

/// `GhosttyDeviceAttributesSecondary`.
#[repr(C)]
pub struct DeviceAttributesSecondary {
    pub device_type: u16,
    pub firmware_version: u16,
    pub rom_cartridge: u16,
}

/// `GhosttyDeviceAttributesTertiary`.
#[repr(C)]
pub struct DeviceAttributesTertiary {
    pub unit_id: u32,
}

/// `GhosttyDeviceAttributes`.
#[repr(C)]
pub struct DeviceAttributes {
    pub primary: DeviceAttributesPrimary,
    pub secondary: DeviceAttributesSecondary,
    pub tertiary: DeviceAttributesTertiary,
}

// ---- size_report.h ----

/// `GhosttySizeReportSize`.
#[repr(C)]
pub struct SizeReportSize {
    pub rows: u16,
    pub columns: u16,
    pub cell_width: u32,
    pub cell_height: u32,
}

// ---- modes.h ----

/// `GhosttyMode`: `(value & 0x7FFF) | (ansi << 15)`.
pub type Mode = u16;

pub const fn dec_mode(value: u16) -> Mode {
    value & 0x7FFF
}

// ---- terminal.h ----

/// `GhosttyTerminalModeConfig` (frozen layout).
#[repr(C)]
pub struct ModeConfig {
    pub mode: Mode,
    pub value: bool,
}

// GhosttyTerminalScreen
pub const TERMINAL_SCREEN_PRIMARY: c_int = 0;
pub const TERMINAL_SCREEN_ALTERNATE: c_int = 1;

/// `GhosttyTerminalMemoryUsage` (sized struct).
#[repr(C)]
#[derive(Default)]
pub struct MemoryUsage {
    pub size: usize,
    pub compression_supported: bool,
    pub primary_pages: u64,
    pub primary_virtual_bytes: u64,
    pub primary_resident_bytes: u64,
    pub primary_compressed_pages: u64,
    pub primary_compressed_bytes: u64,
    pub primary_image_bytes: u64,
    pub alternate_pages: u64,
    pub alternate_virtual_bytes: u64,
    pub alternate_resident_bytes: u64,
    pub alternate_compressed_pages: u64,
    pub alternate_compressed_bytes: u64,
    pub alternate_image_bytes: u64,
}

// GhosttyTerminalUnknownSequenceTag
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
    pub _padding: [u64; 16],
}

/// `GhosttyTerminalUnknownSequence`.
#[repr(C)]
pub struct UnknownSequence {
    pub tag: c_int,
    pub value: UnknownSequenceValue,
}

/// `GhosttyClipboardContent` (frozen layout).
#[repr(C)]
pub struct ClipboardContent {
    pub mime: GString,
    pub data: GString,
}

// GhosttyClipboardWriteResult
pub const CLIPBOARD_WRITE_RESULT_DENIED: c_int = 1;

/// `GhosttyClipboardWriteReply` (sized struct).
#[repr(C)]
pub struct ClipboardWriteReply {
    pub size: usize,
    pub result: c_int,
    pub remember: bool,
}

/// `GhosttyClipboardWriteReplyFn`.
pub type ClipboardWriteReplyFn =
    unsafe extern "C" fn(write: *const ClipboardWrite, reply: *const ClipboardWriteReply);

/// `struct GhosttyClipboardWrite`.
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

// GhosttyClipboardReadResult
pub const CLIPBOARD_READ_RESULT_DENIED: c_int = 1;

/// `GhosttyClipboardReadReply` (sized struct).
#[repr(C)]
pub struct ClipboardReadReply {
    pub size: usize,
    pub result: c_int,
    pub contents: *const ClipboardContent,
    pub contents_len: usize,
    pub available: *const GString,
    pub available_len: usize,
    pub remember: bool,
}

/// `GhosttyClipboardReadReplyFn`.
pub type ClipboardReadReplyFn =
    unsafe extern "C" fn(read: *const ClipboardRead, reply: *const ClipboardReadReply);

/// `struct GhosttyClipboardRead`.
#[repr(C)]
pub struct ClipboardRead {
    pub location: c_int,
    pub mimes: *const GString,
    pub mimes_len: usize,
    pub list: bool,
    pub name: GString,
    pub granted: bool,
    pub can_remember: bool,
    pub ctx: *const c_void,
    pub reply: Option<ClipboardReadReplyFn>,
}

/// `GhosttyTerminalDesktopNotification`.
#[repr(C)]
pub struct DesktopNotification {
    pub title: GString,
    pub body: GString,
}

/// `GhosttyTerminalProgressReport`.
#[repr(C)]
pub struct ProgressReport {
    /// `GhosttyTerminalProgressState`: 0 remove, 1 set, 2 error, 3 indeterminate, 4 pause.
    pub state: c_int,
    /// 0..=100, or -1 when omitted.
    pub progress: i8,
}

// GhosttySemanticPromptKind
pub const SEMANTIC_PROMPT_PROMPT_START: c_int = 1;
pub const SEMANTIC_PROMPT_INPUT_START: c_int = 2;
pub const SEMANTIC_PROMPT_OUTPUT_START: c_int = 3;
pub const SEMANTIC_PROMPT_COMMAND_END: c_int = 4;

// GhosttySemanticPromptPromptKind
pub const SEMANTIC_PROMPT_PROMPT_PRIMARY: c_int = 0;
pub const SEMANTIC_PROMPT_PROMPT_RIGHT: c_int = 1;
pub const SEMANTIC_PROMPT_PROMPT_CONTINUATION: c_int = 2;
pub const SEMANTIC_PROMPT_PROMPT_SECONDARY: c_int = 3;

/// `GhosttyTerminalSemanticPrompt`.
#[repr(C)]
pub struct SemanticPrompt {
    pub kind: c_int,
    pub prompt_kind: c_int,
    pub has_exit_code: bool,
    pub exit_code: i32,
    pub command: GString,
    pub error: GString,
}

// Callback types (terminal.h). The first argument is the terminal, the second the userdata.
pub type WritePtyFn = extern "C" fn(Terminal, *mut c_void, *const u8, usize);
pub type BellFn = extern "C" fn(Terminal, *mut c_void);
pub type TitleChangedFn = extern "C" fn(Terminal, *mut c_void);
pub type PwdChangedFn = extern "C" fn(Terminal, *mut c_void);
pub type SizeFn = extern "C" fn(Terminal, *mut c_void, *mut SizeReportSize) -> bool;
pub type DeviceAttributesFn = extern "C" fn(Terminal, *mut c_void, *mut DeviceAttributes) -> bool;
pub type ClipboardWriteFn = extern "C" fn(Terminal, *mut c_void, *const ClipboardWrite);
pub type ClipboardReadFn = extern "C" fn(Terminal, *mut c_void, *const ClipboardRead);
pub type DesktopNotificationFn = extern "C" fn(Terminal, *mut c_void, *const DesktopNotification);
pub type ProgressReportFn = extern "C" fn(Terminal, *mut c_void, *const ProgressReport);
pub type SemanticPromptFn = extern "C" fn(Terminal, *mut c_void, *const SemanticPrompt);
pub type UnknownSequenceFn = extern "C" fn(Terminal, *mut c_void, *const UnknownSequence);

// GhosttyTerminalOption (1.0 starts at OPT_INVALID = 0)
pub const OPT_INVALID: c_int = 0;
pub const OPT_USERDATA: c_int = 1;
pub const OPT_WRITE_PTY: c_int = 2;
pub const OPT_BELL: c_int = 3;
pub const OPT_TITLE_CHANGED: c_int = 6;
pub const OPT_SIZE: c_int = 7;
pub const OPT_DEVICE_ATTRIBUTES: c_int = 9;
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
pub const OPT_SEMANTIC_PROMPT: c_int = 43;

// GhosttyTerminalData
pub const DATA_ACTIVE_SCREEN: c_int = 6; // GhosttyTerminalScreen
pub const DATA_CURSOR_VISIBLE: c_int = 7; // bool
pub const DATA_KITTY_KEYBOARD_FLAGS: c_int = 8; // uint8_t
pub const DATA_TITLE: c_int = 12; // GhosttyString
pub const DATA_PWD: c_int = 13; // GhosttyString
pub const DATA_SCROLLBACK_ROWS: c_int = 15; // size_t
pub const DATA_KITTY_IMAGE_STORAGE_LIMIT: c_int = 26; // uint64_t
pub const DATA_KITTY_IMAGE_MEDIUM_FILE: c_int = 27; // bool
pub const DATA_KITTY_IMAGE_MEDIUM_TEMP_FILE: c_int = 28; // GhosttyString
pub const DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM: c_int = 29; // bool
pub const DATA_MODE: c_int = 37; // GhosttyTerminalModeConfig (in/out)
pub const DATA_MEMORY_USAGE: c_int = 42; // GhosttyTerminalMemoryUsage

// ---- render.h ----

// GhosttyRenderStateDirty
pub const RENDER_DIRTY_FALSE: c_int = 0;
pub const RENDER_DIRTY_PARTIAL: c_int = 1;
pub const RENDER_DIRTY_FULL: c_int = 2;

// GhosttyRenderStateData
pub const RS_DATA_COLS: c_int = 1; // uint16_t
pub const RS_DATA_ROWS: c_int = 2; // uint16_t
pub const RS_DATA_DIRTY: c_int = 3; // GhosttyRenderStateDirty
pub const RS_DATA_ROW_ITERATOR: c_int = 4; // GhosttyRenderStateRowIterator
pub const RS_DATA_CURSOR: c_int = 18; // GhosttyRenderStateCursor

// GhosttyRenderStateRowData
pub const ROW_DATA_DIRTY: c_int = 1; // bool
pub const ROW_DATA_CELLS: c_int = 3; // GhosttyRenderStateRowCells
pub const ROW_DATA_CELLS_RAW: c_int = 5; // GhosttyCellsView

// GhosttyRenderStateRowCellsData
pub const CELLS_DATA_STYLE: c_int = 2; // GhosttyStyle
pub const CELLS_DATA_GRAPHEMES_UTF8: c_int = 9; // GhosttyBuffer

/// `GhosttyRenderStateCursor` (sized struct).
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
            visual_style: 0,
        }
    }
}

// ---- build_info.h ----

// GhosttyBuildInfo
pub const BUILD_INFO_SIMD: c_int = 1; // bool
pub const BUILD_INFO_KITTY_GRAPHICS: c_int = 2; // bool
pub const BUILD_INFO_OPTIMIZE: c_int = 4; // GhosttyOptimizeMode
pub const BUILD_INFO_VERSION_STRING: c_int = 5; // GhosttyString

/// `GhosttyAllocator`: only ever passed as NULL (the default allocator).
#[repr(C)]
pub struct Allocator {
    _data: [u8; 0],
}

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
    pub fn ghostty_terminal_set(term: Terminal, option: c_int, value: *const c_void) -> GResult;
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
        out_iterator: *mut RowIterator,
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
        out_cells: *mut RowCells,
    ) -> GResult;
    pub fn ghostty_render_state_row_cells_free(cells: RowCells);
    pub fn ghostty_render_state_row_cells_select(cells: RowCells, x: u16) -> GResult;
    pub fn ghostty_render_state_row_cells_get(
        cells: RowCells,
        data: c_int,
        out: *mut c_void,
    ) -> GResult;

    pub fn ghostty_cell_get(cell: Cell, data: c_int, out: *mut c_void) -> GResult;
    pub fn ghostty_cell_get_multi(
        cell: Cell,
        count: usize,
        keys: *const c_int,
        values: *mut *mut c_void,
        out_written: *mut usize,
    ) -> GResult;

    pub fn ghostty_build_info(data: c_int, out: *mut c_void) -> GResult;
    pub fn ghostty_type_json() -> *const std::ffi::c_char;
}

#[cfg(test)]
mod layout {
    //! Checks every struct and enumerator declared above against the linked library's own
    //! ABI manifest (`ghostty_type_json`), so a layout mistake fails here, not at run time.
    use super::*;
    use crate::manifest::{number_after, type_section};
    use std::mem::offset_of;

    macro_rules! check_struct {
        ($ty:ty, $name:literal, [$($field:ident),* $(,)?]) => {{
            let sec = type_section($name).expect($name);
            assert_eq!(number_after(sec, "size"), Some(size_of::<$ty>() as u64), "{} size", $name);
            assert_eq!(number_after(sec, "align"), Some(align_of::<$ty>() as u64), "{} align", $name);
            let n = [$(stringify!($field)),*].len();
            assert_eq!(sec.matches("\"offset\":").count(), n, "{} field count: {sec}", $name);
            $(
                let pat = format!("\"{}\":{{\"offset\":{},", stringify!($field), offset_of!($ty, $field));
                assert!(sec.contains(&pat), "{} field {}: {pat} not in {sec}", $name, stringify!($field));
            )*
        }};
    }

    fn enum_value(name: &str, key: &str) -> Option<i64> {
        let sec = type_section(name)?;
        assert_eq!(
            number_after(sec, "size"),
            Some(4),
            "{name} must be int-sized"
        );
        // Only the `values` object: 1.0 adds a `keys` object after it with the same names.
        let values = &sec[sec.find("\"values\":{")?..];
        let values = &values[..values.find('}')?];
        for sep in ['{', ','] {
            let pat = format!("{sep}\"{key}\":");
            if let Some(i) = values.find(&pat) {
                let rest = &values[i + pat.len()..];
                let num: String = rest
                    .chars()
                    .take_while(|c| *c == '-' || c.is_ascii_digit())
                    .collect();
                return num.parse().ok();
            }
        }
        None
    }

    macro_rules! check_enum {
        ($name:literal, { $($key:literal => $value:expr),* $(,)? }) => {{
            $( assert_eq!(enum_value($name, $key), Some(i64::from($value)), "{}::{}", $name, $key); )*
        }};
    }

    #[test]
    fn structs_match_the_manifest() {
        check_struct!(GString, "GhosttyString", [ptr, len]);
        check_struct!(GBuffer, "GhosttyBuffer", [ptr, cap, len]);
        check_struct!(ColorRgb, "GhosttyColorRgb", [r, g, b]);
        check_struct!(StyleColor, "GhosttyStyleColor", [tag, value]);
        check_struct!(
            Style,
            "GhosttyStyle",
            [fg_color, bg_color, underline_color, flags, reserved]
        );
        check_struct!(CellsView, "GhosttyCellsView", [ptr, len]);
        check_struct!(
            DeviceAttributesPrimary,
            "GhosttyDeviceAttributesPrimary",
            [conformance_level, features, num_features]
        );
        check_struct!(
            DeviceAttributesSecondary,
            "GhosttyDeviceAttributesSecondary",
            [device_type, firmware_version, rom_cartridge]
        );
        check_struct!(
            DeviceAttributesTertiary,
            "GhosttyDeviceAttributesTertiary",
            [unit_id]
        );
        check_struct!(
            DeviceAttributes,
            "GhosttyDeviceAttributes",
            [primary, secondary, tertiary]
        );
        check_struct!(
            SizeReportSize,
            "GhosttySizeReportSize",
            [rows, columns, cell_width, cell_height]
        );
        check_struct!(ModeConfig, "GhosttyTerminalModeConfig", [mode, value]);
        check_struct!(
            MemoryUsage,
            "GhosttyTerminalMemoryUsage",
            [
                size,
                compression_supported,
                primary_pages,
                primary_virtual_bytes,
                primary_resident_bytes,
                primary_compressed_pages,
                primary_compressed_bytes,
                primary_image_bytes,
                alternate_pages,
                alternate_virtual_bytes,
                alternate_resident_bytes,
                alternate_compressed_pages,
                alternate_compressed_bytes,
                alternate_image_bytes,
            ]
        );
        check_struct!(
            UnknownStringSequence,
            "GhosttyTerminalUnknownStringSequence",
            [truncated, content]
        );
        check_struct!(
            UnknownOscSequence,
            "GhosttyTerminalUnknownOscSequence",
            [truncated, content, terminator]
        );
        check_struct!(
            UnknownSequenceValue,
            "GhosttyTerminalUnknownSequenceValue",
            [apc, osc, _padding]
        );
        check_struct!(
            UnknownSequence,
            "GhosttyTerminalUnknownSequence",
            [tag, value]
        );
        check_struct!(ClipboardContent, "GhosttyClipboardContent", [mime, data]);
        check_struct!(
            ClipboardWriteReply,
            "GhosttyClipboardWriteReply",
            [size, result, remember]
        );
        check_struct!(
            ClipboardWrite,
            "GhosttyClipboardWrite",
            [
                location,
                contents,
                contents_len,
                name,
                granted,
                can_remember,
                ctx,
                reply,
            ]
        );
        check_struct!(
            ClipboardReadReply,
            "GhosttyClipboardReadReply",
            [
                size,
                result,
                contents,
                contents_len,
                available,
                available_len,
                remember,
            ]
        );
        check_struct!(
            ClipboardRead,
            "GhosttyClipboardRead",
            [
                location,
                mimes,
                mimes_len,
                list,
                name,
                granted,
                can_remember,
                ctx,
                reply,
            ]
        );
        check_struct!(
            DesktopNotification,
            "GhosttyTerminalDesktopNotification",
            [title, body]
        );
        check_struct!(
            ProgressReport,
            "GhosttyTerminalProgressReport",
            [state, progress]
        );
        check_struct!(
            SemanticPrompt,
            "GhosttyTerminalSemanticPrompt",
            [kind, prompt_kind, has_exit_code, exit_code, command, error,]
        );
        check_struct!(
            RenderCursor,
            "GhosttyRenderStateCursor",
            [
                size,
                viewport_has_value,
                viewport_x,
                viewport_y,
                wide_tail,
                visible,
                blinking,
                password_input,
                visual_style,
            ]
        );
        // The style color union is read as its bytes; both arms start at 0.
        let sec = type_section("GhosttyStyleColorValue").unwrap();
        assert!(sec.starts_with("\"kind\":\"union\",\"size\":3,"), "{sec}");
        assert!(
            sec.contains("\"palette\":{\"offset\":0,\"size\":1"),
            "{sec}"
        );
        assert!(sec.contains("\"rgb\":{\"offset\":0,\"size\":3"), "{sec}");
        // The style flag bits.
        let flags = type_section("GhosttyStyleFlags").unwrap();
        for (name, bit) in [
            ("bold", STYLE_FLAG_BOLD),
            ("italic", STYLE_FLAG_ITALIC),
            ("faint", STYLE_FLAG_FAINT),
            ("blink", STYLE_FLAG_BLINK),
            ("inverse", STYLE_FLAG_INVERSE),
            ("invisible", STYLE_FLAG_INVISIBLE),
            ("strikethrough", STYLE_FLAG_STRIKETHROUGH),
            ("overline", STYLE_FLAG_OVERLINE),
        ] {
            let pat = format!("\"{name}\":{{\"lsb\":{},\"width\":1,", bit.trailing_zeros());
            assert!(flags.contains(&pat), "{pat} not in {flags}");
        }
        let pat = format!("\"underline\":{{\"lsb\":{STYLE_UNDERLINE_SHIFT},\"width\":3,");
        assert!(flags.contains(&pat), "{pat} not in {flags}");
        // The style color tag is one byte holding a GhosttyStyleColorTag.
        let color = type_section("GhosttyStyleColor").unwrap();
        assert!(
            color.contains("\"tag\":{\"offset\":0,\"size\":1,"),
            "{color}"
        );
    }

    #[test]
    fn enumerators_match_the_manifest() {
        check_enum!("GhosttyResult", { "SUCCESS" => SUCCESS, "OUT_OF_SPACE" => OUT_OF_SPACE });
        check_enum!("GhosttyStyleColorTag", {
            "NONE" => STYLE_COLOR_NONE, "PALETTE" => STYLE_COLOR_PALETTE, "RGB" => STYLE_COLOR_RGB,
        });
        check_enum!("GhosttyCellContentTag", {
            "CODEPOINT" => CELL_CONTENT_CODEPOINT,
            "CODEPOINT_GRAPHEME" => CELL_CONTENT_CODEPOINT_GRAPHEME,
            "BG_COLOR_PALETTE" => CELL_CONTENT_BG_COLOR_PALETTE,
            "BG_COLOR_RGB" => CELL_CONTENT_BG_COLOR_RGB,
        });
        check_enum!("GhosttyCellWide", {
            "NARROW" => CELL_WIDE_NARROW, "WIDE" => CELL_WIDE_WIDE,
            "SPACER_TAIL" => CELL_WIDE_SPACER_TAIL, "SPACER_HEAD" => CELL_WIDE_SPACER_HEAD,
        });
        check_enum!("GhosttyCellData", {
            "CODEPOINT" => CELL_DATA_CODEPOINT, "CONTENT_TAG" => CELL_DATA_CONTENT_TAG,
            "WIDE" => CELL_DATA_WIDE, "HAS_STYLING" => CELL_DATA_HAS_STYLING,
            "STYLE_ID" => CELL_DATA_STYLE_ID, "HAS_HYPERLINK" => CELL_DATA_HAS_HYPERLINK,
            "COLOR_PALETTE" => CELL_DATA_COLOR_PALETTE, "COLOR_RGB" => CELL_DATA_COLOR_RGB,
        });
        check_enum!("GhosttyOscTerminator", { "ST" => OSC_TERMINATOR_ST, "BEL" => OSC_TERMINATOR_BEL });
        check_enum!("GhosttyTerminalScreen", {
            "PRIMARY" => TERMINAL_SCREEN_PRIMARY, "ALTERNATE" => TERMINAL_SCREEN_ALTERNATE,
        });
        check_enum!("GhosttyTerminalUnknownSequenceTag", {
            "APC" => UNKNOWN_SEQUENCE_APC, "OSC" => UNKNOWN_SEQUENCE_OSC,
        });
        check_enum!("GhosttyClipboardWriteResult", { "DENIED" => CLIPBOARD_WRITE_RESULT_DENIED });
        check_enum!("GhosttyClipboardReadResult", { "DENIED" => CLIPBOARD_READ_RESULT_DENIED });
        check_enum!("GhosttySemanticPromptKind", {
            "PROMPT_START" => SEMANTIC_PROMPT_PROMPT_START,
            "INPUT_START" => SEMANTIC_PROMPT_INPUT_START,
            "OUTPUT_START" => SEMANTIC_PROMPT_OUTPUT_START,
            "COMMAND_END" => SEMANTIC_PROMPT_COMMAND_END,
        });
        check_enum!("GhosttySemanticPromptPromptKind", {
            "PRIMARY" => SEMANTIC_PROMPT_PROMPT_PRIMARY, "RIGHT" => SEMANTIC_PROMPT_PROMPT_RIGHT,
            "CONTINUATION" => SEMANTIC_PROMPT_PROMPT_CONTINUATION,
            "SECONDARY" => SEMANTIC_PROMPT_PROMPT_SECONDARY,
        });
        check_enum!("GhosttyTerminalOption", {
            "INVALID" => OPT_INVALID, "USERDATA" => OPT_USERDATA, "WRITE_PTY" => OPT_WRITE_PTY, "BELL" => OPT_BELL,
            "TITLE_CHANGED" => OPT_TITLE_CHANGED, "SIZE" => OPT_SIZE,
            "DEVICE_ATTRIBUTES" => OPT_DEVICE_ATTRIBUTES, "PWD_CHANGED" => OPT_PWD_CHANGED,
            "CLIPBOARD_WRITE" => OPT_CLIPBOARD_WRITE,
            "SCROLLBACK_MAX_BYTES" => OPT_SCROLLBACK_MAX_BYTES,
            "SCROLLBACK_MAX_LINES" => OPT_SCROLLBACK_MAX_LINES,
            "DESKTOP_NOTIFICATION" => OPT_DESKTOP_NOTIFICATION,
            "PROGRESS_REPORT" => OPT_PROGRESS_REPORT, "TITLE_REPORT" => OPT_TITLE_REPORT,
            "UNKNOWN_SEQUENCE" => OPT_UNKNOWN_SEQUENCE, "UNKNOWN_MAX_BYTES" => OPT_UNKNOWN_MAX_BYTES,
            "CLIPBOARD_READ" => OPT_CLIPBOARD_READ, "SEMANTIC_PROMPT" => OPT_SEMANTIC_PROMPT,
        });
        check_enum!("GhosttyTerminalData", {
            "ACTIVE_SCREEN" => DATA_ACTIVE_SCREEN, "CURSOR_VISIBLE" => DATA_CURSOR_VISIBLE,
            "KITTY_KEYBOARD_FLAGS" => DATA_KITTY_KEYBOARD_FLAGS, "TITLE" => DATA_TITLE,
            "PWD" => DATA_PWD, "SCROLLBACK_ROWS" => DATA_SCROLLBACK_ROWS,
            "KITTY_IMAGE_STORAGE_LIMIT" => DATA_KITTY_IMAGE_STORAGE_LIMIT,
            "KITTY_IMAGE_MEDIUM_FILE" => DATA_KITTY_IMAGE_MEDIUM_FILE,
            "KITTY_IMAGE_MEDIUM_TEMP_FILE" => DATA_KITTY_IMAGE_MEDIUM_TEMP_FILE,
            "KITTY_IMAGE_MEDIUM_SHARED_MEM" => DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            "MODE" => DATA_MODE, "MEMORY_USAGE" => DATA_MEMORY_USAGE,
        });
        check_enum!("GhosttyRenderStateDirty", {
            "FALSE" => RENDER_DIRTY_FALSE, "PARTIAL" => RENDER_DIRTY_PARTIAL, "FULL" => RENDER_DIRTY_FULL,
        });
        check_enum!("GhosttyRenderStateData", {
            "COLS" => RS_DATA_COLS, "ROWS" => RS_DATA_ROWS, "DIRTY" => RS_DATA_DIRTY,
            "ROW_ITERATOR" => RS_DATA_ROW_ITERATOR, "CURSOR" => RS_DATA_CURSOR,
        });
        check_enum!("GhosttyRenderStateRowData", {
            "DIRTY" => ROW_DATA_DIRTY, "CELLS" => ROW_DATA_CELLS, "CELLS_RAW" => ROW_DATA_CELLS_RAW,
        });
        check_enum!("GhosttyRenderStateRowCellsData", {
            "STYLE" => CELLS_DATA_STYLE, "GRAPHEMES_UTF8" => CELLS_DATA_GRAPHEMES_UTF8,
        });
        check_enum!("GhosttyBuildInfo", {
            "SIMD" => BUILD_INFO_SIMD, "KITTY_GRAPHICS" => BUILD_INFO_KITTY_GRAPHICS,
            "OPTIMIZE" => BUILD_INFO_OPTIMIZE, "VERSION_STRING" => BUILD_INFO_VERSION_STRING,
        });
    }

    #[test]
    fn cell_layout_parses() {
        let l = crate::manifest::CellLayout::parse().expect("GhosttyCell layout from the manifest");
        assert_eq!(l.codepoint.width, 21);
        assert_eq!(l.style_id.width, 16);
    }
}
