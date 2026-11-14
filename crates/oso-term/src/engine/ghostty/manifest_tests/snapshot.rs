//! The snapshot's, the formatter's and the writer's declarations in `ffi::snapshot`, against
//! the linked library's type manifest (ADR 0120, step 11).
//!
//! Must not depend on: anything but `ffi` and the manifest helpers.

use std::mem::offset_of;

use super::super::ffi;
use super::super::ffi::snapshot as s;
use super::{check_enum, check_struct, types};

#[test]
fn snapshot_and_formatter_declarations_match_the_manifest() {
    let t = types();
    check_enum(
        &t,
        "GhosttyResult",
        &[
            ("INVALID_VALUE", s::INVALID_VALUE),
            ("IO_ERROR", s::IO_ERROR),
        ],
    );
    check_enum(
        &t,
        "GhosttyTerminalOption",
        &[("CONTINUATION_MAX_BYTES", s::OPT_CONTINUATION_MAX_BYTES)],
    );
    check_enum(
        &t,
        "GhosttySnapshotDecoderOption",
        &[
            (
                "MAX_CONTINUATION_BYTES",
                s::SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
            ),
            (
                "RETAIN_CONTINUATION",
                s::SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
            ),
            ("COMPRESS_HISTORY", s::SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY),
            (
                "KITTY_IMAGE_STORAGE_LIMIT",
                s::SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            ),
        ],
    );
    check_enum(
        &t,
        "GhosttyFormatterOption",
        &[
            ("EMIT", s::FORMATTER_OPT_EMIT),
            ("UNWRAP", s::FORMATTER_OPT_UNWRAP),
            ("TRIM", s::FORMATTER_OPT_TRIM),
            ("EXTRAS", s::FORMATTER_OPT_EXTRAS),
        ],
    );
    check_enum(
        &t,
        "GhosttyFormatterFormat",
        &[("VT", s::FORMATTER_FORMAT_VT)],
    );
    check_struct::<s::Writer>(
        &t,
        "GhosttyWriter",
        &[
            ("write", offset_of!(s::Writer, write)),
            ("userdata", offset_of!(s::Writer, userdata)),
        ],
    );
    // The result codes oso-term already declares agree with the ones added here.
    assert_ne!(s::INVALID_VALUE, ffi::SUCCESS);
}
