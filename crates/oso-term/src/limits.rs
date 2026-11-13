//! The bounds every terminal keeps, whatever a program prints (ADR 0120): scrollback, queues,
//! the sizes of what a sequence may carry and what a key encodes to, how long a synchronized
//! update may hold the view, and the grid's size.
//!
//! Must not depend on: anything else in the crate.

use oso_platform::process::pty::Size;

/// Scrollback lines when `[terminal] scrollback` says nothing.
pub const SCROLLBACK_DEFAULT: usize = 10_000;
/// The most scrollback lines a terminal keeps.
pub const SCROLLBACK_MAX: usize = 100_000;
/// The most memory a terminal's scrollback holds (ADR 0120, step 1's condition 4): lines of
/// many cells, or of cells that each hold a long cluster, are dropped sooner than the line
/// count says.
pub(crate) const SCROLLBACK_BYTES_MAX: usize = 256 << 20;
/// The most input bytes queued for a program; `Terminal::send` refuses more.
pub const INPUT_QUEUE_MAX: usize = 16 << 20;

/// The most bytes one key encodes to. `Terminal::send_key` holds this much of the input
/// queue for a key until its thread has encoded it, and gives back what it did not take.
pub(crate) const KEY_BYTES_MAX: usize = 128;
/// The longest a synchronized update (mode 2026) holds snapshots back: Ghostty's own app's
/// limit. Past it a terminal sends snapshots as if no hold were open, until the program ends
/// that hold.
pub(crate) const RENDER_HOLD_MAX: std::time::Duration = std::time::Duration::from_secs(1);

/// The most bytes read from the PTY at once.
pub(crate) const READ_CHUNK: usize = 64 << 10;
/// The most bytes, and the longest time, parsed before the thread answers the UI's requests
/// and sends a snapshot again.
pub(crate) const PARSE_SLICE: usize = 1 << 20;
pub(crate) const PARSE_TIME: std::time::Duration = std::time::Duration::from_millis(4);
/// Replies to the program are dropped while this many reply bytes wait to be written.
pub(crate) const REPLIES_PENDING_MAX: usize = 64 << 10;
/// What the library keeps of a sequence it does not know (oso's own, from step 10).
pub(crate) const UNKNOWN_SEQUENCE_MAX: usize = 4096;
/// The most of a title or a directory read from the library.
pub(crate) const TERMINAL_STRING_MAX: usize = 4096;
/// The longest OSC 7 URI taken; a longer one is dropped.
pub(crate) const DIRECTORY_MAX: usize = 4096;
/// The most bytes of one cell's grapheme cluster: the library keeps a code point and at most
/// 64 more, of at most 4 bytes each, so every cluster fits.
pub(crate) const GRAPHEME_MAX: usize = 260;
/// Effects raised but not taken yet are dropped past this many.
pub(crate) const EFFECTS_PENDING_MAX: usize = 1024;
/// The most a Kitty graphics command may buffer (ADR 0120, step 1's condition 4).
pub(crate) const KITTY_APC_MAX: usize = 16 << 20;
/// The largest OSC 52 write taken; a larger one is dropped.
pub(crate) const CLIPBOARD_WRITE_MAX: usize = 1 << 20;
/// Text a sequence carries for oso to show is cut to this many bytes.
pub(crate) const TEXT_MAX: usize = 256;
/// The most events one snapshot carries; the rest are counted.
pub(crate) const EVENTS_MAX: usize = 256;
/// The smallest grid: a wide character always fits.
pub(crate) const MIN_COLUMNS: u16 = 2;
pub(crate) const MIN_ROWS: u16 = 1;
/// The largest grid.
pub(crate) const MAX_COLUMNS: u16 = 1000;
pub(crate) const MAX_ROWS: u16 = 500;

/// `size` within the grid's limits.
pub(crate) fn clamp(size: Size) -> Size {
    Size {
        columns: size.columns.clamp(MIN_COLUMNS, MAX_COLUMNS),
        rows: size.rows.clamp(MIN_ROWS, MAX_ROWS),
        ..size
    }
}
