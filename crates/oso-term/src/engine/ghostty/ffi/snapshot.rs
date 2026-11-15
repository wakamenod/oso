//! The parts of libghostty-vt's `snapshot.h`, `formatter.h` and `io.h` that oso-term uses to
//! carry a terminal over a restart (ADR 0120, step 11; ADR 0133), as the release pinned in
//! `ghostty-vt.lock` declares them: the snapshot's encoder and decoder, the formatter's escape
//! sequences for a restart across a library change, the writer they write through, and
//! `ghostty_free`. `manifest_tests` checks the enumerators and the writer against the
//! library's type manifest; the extras bits are `#define`s it leaves out, and the tests of
//! `raw::snapshot` check what they restore.
//!
//! Must not depend on: anything in oso; only `raw` calls these.

use std::ffi::{c_int, c_void};

use super::{Allocator, GResult, Opaque, Terminal};

/// `GhosttySnapshotDecoder`.
pub type SnapshotDecoder = *mut Opaque;
/// `GhosttyFormatter`.
pub type Formatter = *mut Opaque;

/// `GhosttyResult`'s `GHOSTTY_INVALID_VALUE`: for a snapshot, also a program's sequence that
/// was unfinished past what the library kept of it.
pub const INVALID_VALUE: GResult = -2;
/// `GhosttyResult`'s `GHOSTTY_IO_ERROR`: the writer refused what it was given.
pub const IO_ERROR: GResult = -5;

/// `GhosttyTerminalOption`'s `CONTINUATION_MAX_BYTES`: how much of an unfinished sequence the
/// terminal keeps, for a snapshot.
pub const OPT_CONTINUATION_MAX_BYTES: c_int = 32;

// `GhosttySnapshotDecoderOption`.
pub const SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES: c_int = 1;
pub const SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION: c_int = 2;
pub const SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY: c_int = 3;
pub const SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT: c_int = 4;

// `GhosttyFormatterOption`.
pub const FORMATTER_OPT_EMIT: c_int = 1;
pub const FORMATTER_OPT_UNWRAP: c_int = 2;
pub const FORMATTER_OPT_TRIM: c_int = 3;
pub const FORMATTER_OPT_EXTRAS: c_int = 5;

// `GhosttyFormatterFormat`.
pub const FORMATTER_FORMAT_VT: c_int = 1;

/// `GHOSTTY_FORMATTER_EXTRAS_ALL`: every piece of state the escape sequences can restore.
pub const FORMATTER_EXTRAS_ALL: u32 = 0xFFF;

/// `GhosttyWriterFn`: takes `len` bytes at `data`; false refuses them, which ends the write.
pub type WriterFn =
    unsafe extern "C" fn(userdata: *mut c_void, data: *const u8, len: usize) -> bool;

/// `GhosttyWriter`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Writer {
    pub write: Option<WriterFn>,
    pub userdata: *mut c_void,
}

unsafe extern "C" {
    pub fn ghostty_snapshot_encode(terminal: Terminal, writer: Writer) -> GResult;
    pub fn ghostty_snapshot_decoder_new_buf(
        allocator: *const Allocator,
        decoder: *mut SnapshotDecoder,
        ptr: *const u8,
        len: usize,
    ) -> GResult;
    pub fn ghostty_snapshot_decoder_set(
        decoder: SnapshotDecoder,
        option: c_int,
        value: *const c_void,
    ) -> GResult;
    pub fn ghostty_snapshot_decoder_decode(
        decoder: SnapshotDecoder,
        terminal: *mut Terminal,
    ) -> GResult;
    pub fn ghostty_snapshot_decoder_free(decoder: SnapshotDecoder);
    pub fn ghostty_formatter_terminal_new(
        allocator: *const Allocator,
        formatter: *mut Formatter,
        terminal: Terminal,
    ) -> GResult;
    pub fn ghostty_formatter_set(
        formatter: Formatter,
        option: c_int,
        value: *const c_void,
    ) -> GResult;
    pub fn ghostty_formatter_format(formatter: Formatter, writer: Writer) -> GResult;
    pub fn ghostty_formatter_free(formatter: Formatter);
}
