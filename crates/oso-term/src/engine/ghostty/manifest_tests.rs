//! Checks every struct, constant and packed bit position `ffi` and `layout` declare against
//! the linked library's own type manifest (`ghostty_type_json`), so a layout mistake or a
//! renumbering in a new release fails here, not at run time (ADR 0120). It also tests
//! `build.rs`'s check of oso's patches (`patches`).
//!
//! Must not depend on: anything but `ffi`, `raw`, `layout` and `build.rs`'s `patch_check`.

mod enums;
mod mouse;
#[path = "../../../build/patch_check.rs"]
mod patch_check;
mod patches;

use std::ffi::c_int;
use std::mem::offset_of;

use serde_json::Value;

use super::{ffi, raw};

pub(super) fn types() -> Value {
    let manifest: Value = serde_json::from_str(raw::type_json()).expect("the manifest is JSON");
    assert_eq!(
        manifest["library_version"],
        env!("OSO_GHOSTTY_VT_VERSION"),
        "the lock's version"
    );
    manifest["types"].clone()
}

pub(super) fn number(value: &Value) -> u64 {
    value.as_u64().unwrap_or(u64::MAX)
}

pub(super) fn check_struct<T>(types: &Value, name: &str, fields: &[(&str, usize)]) {
    let ty = &types[name];
    assert!(ty.is_object(), "{name} is not in the manifest");
    assert_eq!(number(&ty["size"]), size_of::<T>() as u64, "{name}'s size");
    assert_eq!(
        number(&ty["align"]),
        align_of::<T>() as u64,
        "{name}'s alignment"
    );
    let declared = ty["fields"].as_object().expect("fields");
    assert_eq!(
        declared.len(),
        fields.len(),
        "{name}'s fields: {declared:?}"
    );
    for (field, offset) in fields {
        let at = number(&declared[*field]["offset"]);
        assert_eq!(at, *offset as u64, "{name}.{field}'s offset");
    }
}

pub(super) fn check_enum(types: &Value, name: &str, values: &[(&str, c_int)]) {
    let ty = &types[name];
    assert_eq!(number(&ty["size"]), 4, "{name} is an int");
    for (key, value) in values {
        let at = ty["values"][*key].as_i64();
        assert_eq!(at, Some(i64::from(*value)), "{name}::{key}");
    }
}

/// The `(lsb, width)` of a packed field, found by its path through the manifest's arms.
pub(super) fn bits(types: &Value, path: &[&str]) -> (u32, u32) {
    let mut fields = &types["GhosttyCell"]["bits"];
    let mut base = 0;
    for (i, step) in path.iter().enumerate() {
        let field = &fields[*step];
        if i + 1 == path.len() {
            let lsb = u32::try_from(number(&field["lsb"])).expect("lsb");
            let width = u32::try_from(number(&field["width"])).expect("width");
            return (base + lsb, width);
        }
        if field["arms"].is_object() {
            base += u32::try_from(number(&field["lsb"])).expect("lsb");
            fields = &field["arms"][path[i + 1]]["bits"];
        }
    }
    panic!("no packed field {path:?}");
}

#[test]
#[expect(clippy::too_many_lines, reason = "one line per declaration checked")]
fn structs_match_the_manifest() {
    let t = types();
    check_struct::<ffi::GString>(
        &t,
        "GhosttyString",
        &[
            ("ptr", ffi::GString::OFFSETS.0),
            ("len", ffi::GString::OFFSETS.1),
        ],
    );
    check_struct::<ffi::GBuffer>(
        &t,
        "GhosttyBuffer",
        &[
            ("ptr", offset_of!(ffi::GBuffer, ptr)),
            ("cap", offset_of!(ffi::GBuffer, cap)),
            ("len", offset_of!(ffi::GBuffer, len)),
        ],
    );
    check_struct::<ffi::ColorRgb>(
        &t,
        "GhosttyColorRgb",
        &[
            ("r", offset_of!(ffi::ColorRgb, r)),
            ("g", offset_of!(ffi::ColorRgb, g)),
            ("b", offset_of!(ffi::ColorRgb, b)),
        ],
    );
    check_struct::<ffi::StyleColor>(
        &t,
        "GhosttyStyleColor",
        &[
            ("tag", offset_of!(ffi::StyleColor, tag)),
            ("value", offset_of!(ffi::StyleColor, value)),
        ],
    );
    check_struct::<ffi::Style>(
        &t,
        "GhosttyStyle",
        &[
            ("fg_color", offset_of!(ffi::Style, fg_color)),
            ("bg_color", offset_of!(ffi::Style, bg_color)),
            ("underline_color", offset_of!(ffi::Style, underline_color)),
            ("flags", offset_of!(ffi::Style, flags)),
            ("reserved", offset_of!(ffi::Style, reserved)),
        ],
    );
    check_struct::<ffi::CellsView>(
        &t,
        "GhosttyCellsView",
        &[
            ("ptr", offset_of!(ffi::CellsView, ptr)),
            ("len", offset_of!(ffi::CellsView, len)),
        ],
    );
    check_struct::<ffi::SizeReportSize>(
        &t,
        "GhosttySizeReportSize",
        &[
            ("rows", offset_of!(ffi::SizeReportSize, rows)),
            ("columns", offset_of!(ffi::SizeReportSize, columns)),
            ("cell_width", offset_of!(ffi::SizeReportSize, cell_width)),
            ("cell_height", offset_of!(ffi::SizeReportSize, cell_height)),
        ],
    );
    check_struct::<ffi::ModeConfig>(
        &t,
        "GhosttyTerminalModeConfig",
        &[
            ("mode", offset_of!(ffi::ModeConfig, mode)),
            ("value", offset_of!(ffi::ModeConfig, value)),
        ],
    );
    check_struct::<ffi::RenderCursor>(
        &t,
        "GhosttyRenderStateCursor",
        &[
            ("size", offset_of!(ffi::RenderCursor, size)),
            (
                "viewport_has_value",
                offset_of!(ffi::RenderCursor, viewport_has_value),
            ),
            ("viewport_x", offset_of!(ffi::RenderCursor, viewport_x)),
            ("viewport_y", offset_of!(ffi::RenderCursor, viewport_y)),
            ("wide_tail", offset_of!(ffi::RenderCursor, wide_tail)),
            ("visible", offset_of!(ffi::RenderCursor, visible)),
            ("blinking", offset_of!(ffi::RenderCursor, blinking)),
            (
                "password_input",
                offset_of!(ffi::RenderCursor, password_input),
            ),
            ("visual_style", offset_of!(ffi::RenderCursor, visual_style)),
        ],
    );
}

#[test]
fn scroll_and_key_structs_match_the_manifest() {
    let t = types();
    check_struct::<ffi::Scrollbar>(
        &t,
        "GhosttyTerminalScrollbar",
        &[
            ("total", offset_of!(ffi::Scrollbar, total)),
            ("offset", offset_of!(ffi::Scrollbar, offset)),
            ("len", offset_of!(ffi::Scrollbar, len)),
        ],
    );
    check_struct::<ffi::ScrollViewport>(
        &t,
        "GhosttyTerminalScrollViewport",
        &[
            ("tag", offset_of!(ffi::ScrollViewport, tag)),
            ("value", offset_of!(ffi::ScrollViewport, value)),
        ],
    );
    let value = &t["GhosttyTerminalScrollViewportValue"];
    assert_eq!(
        number(&value["size"]),
        size_of::<ffi::ScrollViewportValue>() as u64
    );
    assert_eq!(
        number(&value["align"]),
        align_of::<ffi::ScrollViewportValue>() as u64
    );
    // The key event's text is a `GhosttyString` under a type of oso's own.
    check_struct::<ffi::KeyText>(
        &t,
        "GhosttyString",
        &[
            ("ptr", offset_of!(ffi::KeyText, ptr)),
            ("len", offset_of!(ffi::KeyText, len)),
        ],
    );
    assert_eq!(t["GhosttyMods"]["size"], 2, "GhosttyMods is a uint16_t");
    assert_eq!(t["GhosttyKeyEncoder"]["kind"], "opaque");
    assert_eq!(t["GhosttyKeyEvent"]["kind"], "opaque");
}

#[test]
#[expect(clippy::too_many_lines, reason = "one line per declaration checked")]
fn callback_structs_match_the_manifest() {
    let t = types();
    check_struct::<ffi::UnknownStringSequence>(
        &t,
        "GhosttyTerminalUnknownStringSequence",
        &[
            (
                "truncated",
                offset_of!(ffi::UnknownStringSequence, truncated),
            ),
            ("content", offset_of!(ffi::UnknownStringSequence, content)),
        ],
    );
    check_struct::<ffi::UnknownOscSequence>(
        &t,
        "GhosttyTerminalUnknownOscSequence",
        &[
            ("truncated", offset_of!(ffi::UnknownOscSequence, truncated)),
            ("content", offset_of!(ffi::UnknownOscSequence, content)),
            (
                "terminator",
                offset_of!(ffi::UnknownOscSequence, terminator),
            ),
        ],
    );
    check_struct::<ffi::UnknownSequence>(
        &t,
        "GhosttyTerminalUnknownSequence",
        &[
            ("tag", offset_of!(ffi::UnknownSequence, tag)),
            ("value", offset_of!(ffi::UnknownSequence, value)),
        ],
    );
    let value = &t["GhosttyTerminalUnknownSequenceValue"];
    assert_eq!(
        number(&value["size"]),
        size_of::<ffi::UnknownSequenceValue>() as u64
    );
    check_struct::<ffi::ClipboardContent>(
        &t,
        "GhosttyClipboardContent",
        &[
            ("mime", offset_of!(ffi::ClipboardContent, mime)),
            ("data", offset_of!(ffi::ClipboardContent, data)),
        ],
    );
    check_struct::<ffi::ClipboardWriteReply>(
        &t,
        "GhosttyClipboardWriteReply",
        &[
            ("size", offset_of!(ffi::ClipboardWriteReply, size)),
            ("result", offset_of!(ffi::ClipboardWriteReply, result)),
            ("remember", offset_of!(ffi::ClipboardWriteReply, remember)),
        ],
    );
    check_struct::<ffi::ClipboardWrite>(
        &t,
        "GhosttyClipboardWrite",
        &[
            ("location", offset_of!(ffi::ClipboardWrite, location)),
            ("contents", offset_of!(ffi::ClipboardWrite, contents)),
            (
                "contents_len",
                offset_of!(ffi::ClipboardWrite, contents_len),
            ),
            ("name", offset_of!(ffi::ClipboardWrite, name)),
            ("granted", offset_of!(ffi::ClipboardWrite, granted)),
            (
                "can_remember",
                offset_of!(ffi::ClipboardWrite, can_remember),
            ),
            ("ctx", offset_of!(ffi::ClipboardWrite, ctx)),
            ("reply", offset_of!(ffi::ClipboardWrite, reply)),
        ],
    );
    check_struct::<ffi::DesktopNotification>(
        &t,
        "GhosttyTerminalDesktopNotification",
        &[
            ("title", offset_of!(ffi::DesktopNotification, title)),
            ("body", offset_of!(ffi::DesktopNotification, body)),
        ],
    );
    check_struct::<ffi::ProgressReport>(
        &t,
        "GhosttyTerminalProgressReport",
        &[
            ("state", offset_of!(ffi::ProgressReport, state)),
            ("progress", offset_of!(ffi::ProgressReport, progress)),
        ],
    );
    check_struct::<ffi::SemanticPrompt>(
        &t,
        "GhosttyTerminalSemanticPrompt",
        &[
            ("kind", offset_of!(ffi::SemanticPrompt, kind)),
            ("prompt_kind", offset_of!(ffi::SemanticPrompt, prompt_kind)),
            (
                "has_exit_code",
                offset_of!(ffi::SemanticPrompt, has_exit_code),
            ),
            ("exit_code", offset_of!(ffi::SemanticPrompt, exit_code)),
            ("command", offset_of!(ffi::SemanticPrompt, command)),
            ("error", offset_of!(ffi::SemanticPrompt, error)),
        ],
    );
}

/// Each callback oso installs has the manifest's parameters and result.
#[test]
fn callbacks_match_the_manifest() {
    let t = types();
    let callbacks: [(&str, usize, &str); 12] = [
        ("GhosttyTerminalWritePtyFn", 4, "void"),
        ("GhosttyTerminalBellFn", 2, "void"),
        ("GhosttyTerminalXtversionFn", 2, "GhosttyString"),
        ("GhosttyTerminalTitleChangedFn", 2, "void"),
        ("GhosttyTerminalPwdChangedFn", 2, "void"),
        ("GhosttyTerminalSizeFn", 3, "bool"),
        ("GhosttyTerminalClipboardWriteFn", 3, "void"),
        ("GhosttyTerminalDesktopNotificationFn", 3, "void"),
        ("GhosttyTerminalProgressReportFn", 3, "void"),
        ("GhosttyTerminalSemanticPromptFn", 3, "void"),
        ("GhosttyTerminalUnknownSequenceFn", 3, "void"),
        ("GhosttyTerminalRenderHoldFn", 3, "void"),
    ];
    for (name, params, result) in callbacks {
        let ty = &t[name];
        assert_eq!(ty["kind"], "callback", "{name}");
        let declared = ty["params"].as_array().map_or(0, Vec::len);
        assert_eq!(declared, params, "{name}'s parameters");
        assert_eq!(ty["return"]["type"], result, "{name}'s result");
    }
}

#[test]
fn the_library_is_the_locked_release() {
    assert_eq!(raw::version(), env!("OSO_GHOSTTY_VT_VERSION"));
}

/// Every package the lock pins is described in the licence list.
#[test]
fn the_lock_and_the_licence_list_agree() {
    let lock = include_str!("../../../ghostty-vt.lock");
    let list = include_str!("../../../third-party.md");
    let packages: Vec<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("package "))
        .collect();
    assert_eq!(packages.len(), 5);
    for package in packages {
        assert!(
            list.contains(&format!("`{package}`")),
            "{package} is not in third-party.md"
        );
    }
    // Each of oso's patches is described there too.
    for line in lock.lines().filter_map(|line| line.strip_prefix("patch ")) {
        let name = line.split_whitespace().next().unwrap_or_default();
        assert!(
            list.contains(&format!("`patches/{name}`")),
            "{name} is not in third-party.md"
        );
    }
}
