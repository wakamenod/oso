//! The options, data keys, enums and packed bit positions `ffi` and `layout` declare, against
//! the linked library's type manifest (ADR 0120).
//!
//! Must not depend on: anything but `ffi`, `raw`, `layout` and the manifest helpers.

use super::super::{ffi, layout, raw};
use super::{bits, check_enum, number, types};

#[test]
#[expect(clippy::too_many_lines, reason = "one line per declaration checked")]
fn options_and_data_match_the_manifest() {
    let t = types();
    check_enum(
        &t,
        "GhosttyTerminalOption",
        &[
            ("USERDATA", ffi::OPT_USERDATA),
            ("WRITE_PTY", ffi::OPT_WRITE_PTY),
            ("BELL", ffi::OPT_BELL),
            ("ENQUIRY", ffi::OPT_ENQUIRY),
            ("XTVERSION", ffi::OPT_XTVERSION),
            ("TITLE_CHANGED", ffi::OPT_TITLE_CHANGED),
            ("SIZE", ffi::OPT_SIZE),
            ("DEVICE_ATTRIBUTES", ffi::OPT_DEVICE_ATTRIBUTES),
            ("COLOR_FOREGROUND", ffi::OPT_COLOR_FOREGROUND),
            ("COLOR_BACKGROUND", ffi::OPT_COLOR_BACKGROUND),
            ("COLOR_CURSOR", ffi::OPT_COLOR_CURSOR),
            ("COLOR_PALETTE", ffi::OPT_COLOR_PALETTE),
            (
                "KITTY_IMAGE_STORAGE_LIMIT",
                ffi::OPT_KITTY_IMAGE_STORAGE_LIMIT,
            ),
            ("KITTY_IMAGE_MEDIUM_FILE", ffi::OPT_KITTY_IMAGE_MEDIUM_FILE),
            (
                "KITTY_IMAGE_MEDIUM_TEMP_FILE",
                ffi::OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
            ),
            (
                "KITTY_IMAGE_MEDIUM_SHARED_MEM",
                ffi::OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            ),
            ("APC_MAX_BYTES_KITTY", ffi::OPT_APC_MAX_BYTES_KITTY),
            ("PWD_CHANGED", ffi::OPT_PWD_CHANGED),
            ("CLIPBOARD_WRITE", ffi::OPT_CLIPBOARD_WRITE),
            ("SCROLLBACK_MAX_BYTES", ffi::OPT_SCROLLBACK_MAX_BYTES),
            ("SCROLLBACK_MAX_LINES", ffi::OPT_SCROLLBACK_MAX_LINES),
            ("DESKTOP_NOTIFICATION", ffi::OPT_DESKTOP_NOTIFICATION),
            ("PROGRESS_REPORT", ffi::OPT_PROGRESS_REPORT),
            ("TITLE_REPORT", ffi::OPT_TITLE_REPORT),
            ("UNKNOWN_SEQUENCE", ffi::OPT_UNKNOWN_SEQUENCE),
            ("UNKNOWN_MAX_BYTES", ffi::OPT_UNKNOWN_MAX_BYTES),
            ("CLIPBOARD_READ", ffi::OPT_CLIPBOARD_READ),
            (
                "CLIPBOARD_WRITE_MAX_BYTES",
                ffi::OPT_CLIPBOARD_WRITE_MAX_BYTES,
            ),
            ("SEMANTIC_PROMPT", ffi::OPT_SEMANTIC_PROMPT),
            ("XT_CHECKSUM_REPORT", ffi::OPT_XT_CHECKSUM_REPORT),
            ("RESIZE_WINDOW", ffi::OPT_RESIZE_WINDOW),
        ],
    );
    check_enum(
        &t,
        "GhosttyTerminalData",
        &[
            ("ACTIVE_SCREEN", ffi::DATA_ACTIVE_SCREEN),
            ("KITTY_KEYBOARD_FLAGS", ffi::DATA_KITTY_KEYBOARD_FLAGS),
            ("TITLE", ffi::DATA_TITLE),
            ("PWD", ffi::DATA_PWD),
            ("SCROLLBACK_ROWS", ffi::DATA_SCROLLBACK_ROWS),
            (
                "KITTY_IMAGE_STORAGE_LIMIT",
                ffi::DATA_KITTY_IMAGE_STORAGE_LIMIT,
            ),
            ("KITTY_IMAGE_MEDIUM_FILE", ffi::DATA_KITTY_IMAGE_MEDIUM_FILE),
            (
                "KITTY_IMAGE_MEDIUM_SHARED_MEM",
                ffi::DATA_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            ),
            ("MODE", ffi::DATA_MODE),
            (
                "CLIPBOARD_WRITE_MAX_BYTES",
                ffi::DATA_CLIPBOARD_WRITE_MAX_BYTES,
            ),
        ],
    );
    check_enum(
        &t,
        "GhosttyRenderStateData",
        &[
            ("COLS", ffi::RS_DATA_COLS),
            ("ROWS", ffi::RS_DATA_ROWS),
            ("DIRTY", ffi::RS_DATA_DIRTY),
            ("ROW_ITERATOR", ffi::RS_DATA_ROW_ITERATOR),
            ("CURSOR", ffi::RS_DATA_CURSOR),
        ],
    );
    check_enum(
        &t,
        "GhosttyRenderStateRowData",
        &[
            ("DIRTY", ffi::ROW_DATA_DIRTY),
            ("RAW", ffi::ROW_DATA_RAW),
            ("CELLS", ffi::ROW_DATA_CELLS),
            ("CELLS_RAW", ffi::ROW_DATA_CELLS_RAW),
        ],
    );
    check_enum(
        &t,
        "GhosttyRenderStateRowCellsData",
        &[
            ("STYLE", ffi::CELLS_DATA_STYLE),
            ("GRAPHEMES_UTF8", ffi::CELLS_DATA_GRAPHEMES_UTF8),
        ],
    );
    check_enum(&t, "GhosttyRowData", &[("WRAP", ffi::ROW_DATA_WRAP)]);
    check_enum(
        &t,
        "GhosttyBuildInfo",
        &[
            ("KITTY_GRAPHICS", ffi::BUILD_INFO_KITTY_GRAPHICS),
            ("OPTIMIZE", ffi::BUILD_INFO_OPTIMIZE),
            ("VERSION_STRING", ffi::BUILD_INFO_VERSION_STRING),
        ],
    );
}

#[test]
#[expect(clippy::too_many_lines, reason = "one line per declaration checked")]
fn enums_match_the_manifest() {
    let t = types();
    check_enum(
        &t,
        "GhosttyResult",
        &[
            ("SUCCESS", ffi::SUCCESS),
            ("OUT_OF_SPACE", ffi::OUT_OF_SPACE),
        ],
    );
    check_enum(
        &t,
        "GhosttyCursorStyle",
        &[
            ("BAR", ffi::CURSOR_STYLE_BAR),
            ("BLOCK", ffi::CURSOR_STYLE_BLOCK),
            ("UNDERLINE", ffi::CURSOR_STYLE_UNDERLINE),
            ("BLOCK_HOLLOW", ffi::CURSOR_STYLE_BLOCK_HOLLOW),
        ],
    );
    check_enum(
        &t,
        "GhosttySgrUnderline",
        &[
            ("NONE", ffi::UNDERLINE_NONE),
            ("SINGLE", ffi::UNDERLINE_SINGLE),
            ("DOUBLE", ffi::UNDERLINE_DOUBLE),
            ("CURLY", ffi::UNDERLINE_CURLY),
            ("DOTTED", ffi::UNDERLINE_DOTTED),
            ("DASHED", ffi::UNDERLINE_DASHED),
        ],
    );
    check_enum(
        &t,
        "GhosttyCellContentTag",
        &[
            ("CODEPOINT", ffi::CELL_CONTENT_CODEPOINT),
            ("CODEPOINT_GRAPHEME", ffi::CELL_CONTENT_CODEPOINT_GRAPHEME),
            ("BG_COLOR_PALETTE", ffi::CELL_CONTENT_BG_COLOR_PALETTE),
            ("BG_COLOR_RGB", ffi::CELL_CONTENT_BG_COLOR_RGB),
        ],
    );
    check_enum(
        &t,
        "GhosttyCellWide",
        &[
            ("NARROW", ffi::CELL_WIDE_NARROW),
            ("WIDE", ffi::CELL_WIDE_WIDE),
            ("SPACER_TAIL", ffi::CELL_WIDE_SPACER_TAIL),
            ("SPACER_HEAD", ffi::CELL_WIDE_SPACER_HEAD),
        ],
    );
    check_enum(
        &t,
        "GhosttyCellData",
        &[
            ("CODEPOINT", ffi::CELL_DATA_CODEPOINT),
            ("CONTENT_TAG", ffi::CELL_DATA_CONTENT_TAG),
            ("WIDE", ffi::CELL_DATA_WIDE),
            ("STYLE_ID", ffi::CELL_DATA_STYLE_ID),
            ("COLOR_PALETTE", ffi::CELL_DATA_COLOR_PALETTE),
        ],
    );
    check_enum(
        &t,
        "GhosttyTerminalScreen",
        &[
            ("PRIMARY", ffi::TERMINAL_SCREEN_PRIMARY),
            ("ALTERNATE", ffi::TERMINAL_SCREEN_ALTERNATE),
        ],
    );
    check_enum(
        &t,
        "GhosttyTerminalUnknownSequenceTag",
        &[
            ("APC", ffi::UNKNOWN_SEQUENCE_APC),
            ("OSC", ffi::UNKNOWN_SEQUENCE_OSC),
        ],
    );
    check_enum(
        &t,
        "GhosttyClipboardWriteResult",
        &[("SUCCESS", ffi::CLIPBOARD_WRITE_RESULT_SUCCESS)],
    );
    check_enum(
        &t,
        "GhosttyTerminalProgressState",
        &[
            ("REMOVE", ffi::PROGRESS_REMOVE),
            ("SET", ffi::PROGRESS_SET),
            ("ERROR", ffi::PROGRESS_ERROR),
            ("INDETERMINATE", ffi::PROGRESS_INDETERMINATE),
            ("PAUSE", ffi::PROGRESS_PAUSE),
        ],
    );
    check_enum(
        &t,
        "GhosttySemanticPromptKind",
        &[
            ("PROMPT_START", ffi::SEMANTIC_PROMPT_PROMPT_START),
            ("INPUT_START", ffi::SEMANTIC_PROMPT_INPUT_START),
            ("OUTPUT_START", ffi::SEMANTIC_PROMPT_OUTPUT_START),
            ("COMMAND_END", ffi::SEMANTIC_PROMPT_COMMAND_END),
        ],
    );
    check_enum(
        &t,
        "GhosttyRenderStateDirty",
        &[
            ("FALSE", ffi::RENDER_DIRTY_FALSE),
            ("PARTIAL", ffi::RENDER_DIRTY_PARTIAL),
            ("FULL", ffi::RENDER_DIRTY_FULL),
        ],
    );
}

#[test]
fn packed_cells_match_the_manifest() {
    let t = types();
    assert_eq!(number(&t["GhosttyCell"]["size"]), 8);
    assert_eq!(bits(&t, &["content_tag"]), layout::CONTENT_TAG);
    assert_eq!(
        bits(&t, &["content", "CODEPOINT", "codepoint"]),
        layout::CODEPOINT
    );
    assert_eq!(
        bits(&t, &["content", "CODEPOINT_GRAPHEME", "codepoint"]),
        layout::CODEPOINT
    );
    assert_eq!(
        bits(&t, &["content", "BG_COLOR_PALETTE", "index"]),
        layout::PALETTE_INDEX
    );
    assert_eq!(bits(&t, &["content", "BG_COLOR_RGB", "r"]), layout::RED);
    assert_eq!(bits(&t, &["content", "BG_COLOR_RGB", "g"]), layout::GREEN);
    assert_eq!(bits(&t, &["content", "BG_COLOR_RGB", "b"]), layout::BLUE);
    assert_eq!(bits(&t, &["style_id"]), layout::STYLE_ID);
    assert_eq!(bits(&t, &["wide"]), layout::WIDE);
}

/// `layout`'s decoding agrees with the library's own accessor on cells it made.
#[test]
fn packed_cells_decode_as_the_library_reads_them() {
    let samples: [u64; 6] = [
        0,
        u64::from(u32::from('a')) << 2,
        (u64::from(u32::from('漢')) << 2) | (1 << 42),
        2 | (200 << 2),
        3 | (10 << 2) | (20 << 10) | (30 << 18),
        (u64::from(u32::from('é')) << 2) | 1 | (7 << 26),
    ];
    for cell in samples {
        let tag = raw::cell_get_u32(cell, ffi::CELL_DATA_CONTENT_TAG);
        assert_eq!(
            u64::from(tag),
            layout::get(cell, layout::CONTENT_TAG),
            "{cell:x}"
        );
        let wide = raw::cell_get_u32(cell, ffi::CELL_DATA_WIDE);
        assert_eq!(u64::from(wide), layout::get(cell, layout::WIDE), "{cell:x}");
        let style = raw::cell_get_u32(cell, ffi::CELL_DATA_STYLE_ID);
        assert_eq!(
            u64::from(style),
            layout::get(cell, layout::STYLE_ID),
            "{cell:x}"
        );
        if tag < 2 {
            let code = raw::cell_get_u32(cell, ffi::CELL_DATA_CODEPOINT);
            assert_eq!(
                u64::from(code),
                layout::get(cell, layout::CODEPOINT),
                "{cell:x}"
            );
        } else if tag == 2 {
            let index = raw::cell_get_u32(cell, ffi::CELL_DATA_COLOR_PALETTE);
            assert_eq!(
                u64::from(index),
                layout::get(cell, layout::PALETTE_INDEX),
                "{cell:x}"
            );
        }
    }
}
