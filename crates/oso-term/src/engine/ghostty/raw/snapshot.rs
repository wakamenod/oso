//! Carrying a terminal over `oso dev`'s restart (ADR 0120, step 11), over libghostty-vt's
//! C API: the library's snapshot, which the next instance restores exactly (both screens,
//! every mode, a sequence the program had not finished), or, for a restart across a change of
//! the library, whose snapshot format may differ, the escape sequences its formatter writes to
//! redraw the screen in use and its scrollback. Each foreign call sits in an `unsafe` block of
//! its own with its `// SAFETY:` comment.
//!
//! Must not depend on: the policy, the thread or the snapshot's types.

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The library's declarations, `snapshot.h`'s and `formatter.h`'s among them.
mod ffi {
    pub use super::super::super::ffi::snapshot::*;
    pub use super::super::super::ffi::*;
}
use super::{Rows, Vt};
use crate::limits;

/// How a terminal is written for the next instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    /// The library's snapshot, for an instance with the same library.
    Snapshot,
    /// Escape sequences that redraw the screen in use and its scrollback.
    Escapes,
}

/// Why a terminal could not be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotEncoded {
    /// It takes more than the bytes it was given.
    TooLarge,
    /// The library refused, with this result.
    Refused(c_int),
}

/// What a write collects, taking each byte from `left`, the bytes left for every terminal
/// written for the same restart.
struct Collected<'a> {
    bytes: Vec<u8>,
    left: &'a AtomicUsize,
    over: bool,
}

/// The writer's callback: appends the bytes, or refuses them once they pass the budget. It
/// never unwinds into the library.
unsafe extern "C" fn collect(userdata: *mut c_void, data: *const u8, len: usize) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `userdata` is the `Collected` that `Vt::encode` lent the writer for this
        // call, alive and reached only through this callback meanwhile.
        let collected = unsafe { &mut *userdata.cast::<Collected<'_>>() };
        let took = collected
            .left
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |left| {
                left.checked_sub(len)
            });
        if took.is_err() {
            collected.over = true;
            return false;
        }
        if len == 0 {
            return true;
        }
        // SAFETY: the library passes `len` readable bytes at `data` for the call.
        let bytes = unsafe { std::slice::from_raw_parts(data, len) };
        collected.bytes.extend_from_slice(bytes);
        true
    }))
    .unwrap_or(false)
}

impl Vt {
    /// Keeps the bytes of a sequence the program has not finished, up to
    /// `limits::CONTINUATION_MAX`, so that a snapshot taken between two reads restores it.
    pub fn track_continuation(&mut self) -> bool {
        let max = limits::CONTINUATION_MAX;
        // SAFETY: the terminal is live; the option takes a pointer to a live `size_t`, which
        // the library copies.
        let set = unsafe {
            ffi::ghostty_terminal_set(
                self.terminal,
                ffi::OPT_CONTINUATION_MAX_BYTES,
                (&raw const max).cast(),
            )
        };
        set == ffi::SUCCESS
    }

    /// The terminal written as `encoding`, its bytes taken from `budget`, which other
    /// terminals of the same restart share; a failed write gives back what it took. A snapshot of a
    /// terminal whose program left a sequence longer than the library kept unfinished ends
    /// that sequence first, with CAN, as a terminal does.
    pub fn encode(
        &mut self,
        encoding: Encoding,
        budget: &AtomicUsize,
    ) -> Result<Vec<u8>, NotEncoded> {
        match self.encode_once(encoding, budget) {
            Err(NotEncoded::Refused(ffi::INVALID_VALUE)) if encoding == Encoding::Snapshot => {
                self.write(b"\x18");
                self.encode_once(encoding, budget)
            }
            other => other,
        }
    }

    fn encode_once(
        &mut self,
        encoding: Encoding,
        budget: &AtomicUsize,
    ) -> Result<Vec<u8>, NotEncoded> {
        self.at = Rows::Unset;
        let mut collected = Collected {
            bytes: Vec::new(),
            left: budget,
            over: false,
        };
        let writer = ffi::Writer {
            write: Some(collect),
            userdata: (&raw mut collected).cast(),
        };
        let done = match encoding {
            // SAFETY: the terminal is live and nothing else touches it during the call; the
            // writer's callback never calls the library, and its userdata is `collected`,
            // which outlives the call.
            Encoding::Snapshot => unsafe { ffi::ghostty_snapshot_encode(self.terminal, writer) },
            Encoding::Escapes => self.format(writer),
        };
        if done != ffi::SUCCESS || collected.over {
            budget.fetch_add(collected.bytes.len(), Ordering::AcqRel);
        }
        if collected.over {
            Err(NotEncoded::TooLarge)
        } else if done == ffi::SUCCESS {
            Ok(collected.bytes)
        } else {
            Err(NotEncoded::Refused(done))
        }
    }

    /// Writes the escape sequences that redraw the terminal through `writer`, with every
    /// extra the formatter has: modes, palette, cursor, scrolling region, tabs, keyboard
    /// modes, character sets.
    fn format(&mut self, writer: ffi::Writer) -> ffi::GResult {
        let mut formatter: ffi::Formatter = ptr::null_mut();
        // SAFETY: a NULL allocator is the library's default; the terminal is live and
        // outlives the formatter, which is freed below.
        let made = unsafe {
            ffi::ghostty_formatter_terminal_new(ptr::null(), &mut formatter, self.terminal)
        };
        if made != ffi::SUCCESS {
            return made;
        }
        let emit = ffi::FORMATTER_FORMAT_VT;
        let extras = ffi::FORMATTER_EXTRAS_ALL;
        let off = false;
        let settings: [(c_int, *const c_void); 4] = [
            (ffi::FORMATTER_OPT_EMIT, (&raw const emit).cast()),
            (ffi::FORMATTER_OPT_EXTRAS, (&raw const extras).cast()),
            (ffi::FORMATTER_OPT_TRIM, (&raw const off).cast()),
            (ffi::FORMATTER_OPT_UNWRAP, (&raw const off).cast()),
        ];
        let set = settings.into_iter().all(|(option, value)| {
            // SAFETY: the formatter is live; each value points to a live local of the
            // option's type in formatter.h (`GhosttyFormatterFormat`, `uint32_t`, `bool`),
            // which the library copies.
            unsafe { ffi::ghostty_formatter_set(formatter, option, value) == ffi::SUCCESS }
        });
        let done = if set {
            // SAFETY: the formatter is live over a live terminal; the writer is as `encode`
            // says.
            unsafe { ffi::ghostty_formatter_format(formatter, writer) }
        } else {
            ffi::INVALID_VALUE
        };
        // SAFETY: made above and freed once, here.
        unsafe { ffi::ghostty_formatter_free(formatter) };
        done
    }

    /// A terminal of `columns` × `rows` restored from `bytes`, as `encode` wrote them as
    /// `encoding`, with oso's settings and callbacks as `new` installs them; `None` if the
    /// library refuses them. Kitty images stay off, as in a new terminal.
    pub fn restore(
        encoding: Encoding,
        bytes: &[u8],
        size: (u16, u16),
        scrollback_lines: usize,
        xtversion: &'static str,
    ) -> Option<Vt> {
        let (columns, rows) = size;
        match encoding {
            Encoding::Snapshot => {
                Vt::make(columns, rows, scrollback_lines, xtversion, |terminal| {
                    decode(bytes, terminal)
                })
            }
            Encoding::Escapes => {
                let mut vt = Vt::new(columns, rows, scrollback_lines, xtversion)?;
                vt.write(bytes);
                Some(vt)
            }
        }
    }
}

/// Decodes the snapshot `bytes` into `terminal`, a terminal the caller then owns.
fn decode(bytes: &[u8], terminal: &mut ffi::Terminal) -> ffi::GResult {
    let mut decoder: ffi::SnapshotDecoder = ptr::null_mut();
    // SAFETY: a NULL allocator is the library's default. The decoder borrows `bytes`, which
    // stay readable and unchanged until it is freed below.
    let made = unsafe {
        ffi::ghostty_snapshot_decoder_new_buf(
            ptr::null(),
            &mut decoder,
            bytes.as_ptr(),
            bytes.len(),
        )
    };
    if made != ffi::SUCCESS {
        return made;
    }
    let continuation = limits::CONTINUATION_MAX;
    let off = false;
    let no_images = 0u64;
    let settings: [(c_int, *const c_void); 4] = [
        (
            ffi::SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
            (&raw const continuation).cast(),
        ),
        (
            ffi::SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
            (&raw const off).cast(),
        ),
        (
            ffi::SNAPSHOT_DECODER_OPT_COMPRESS_HISTORY,
            (&raw const off).cast(),
        ),
        (
            ffi::SNAPSHOT_DECODER_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&raw const no_images).cast(),
        ),
    ];
    let set = settings.into_iter().all(|(option, value)| {
        // SAFETY: the decoder is live and has not started; each value points to a live local
        // of the option's type in snapshot.h (`size_t`, `bool`, `bool`, `uint64_t`), which
        // the library copies.
        unsafe { ffi::ghostty_snapshot_decoder_set(decoder, option, value) == ffi::SUCCESS }
    });
    let done = if set {
        // SAFETY: the decoder is live and has not started; `terminal` is written on success,
        // and set to NULL on every error.
        unsafe { ffi::ghostty_snapshot_decoder_decode(decoder, terminal) }
    } else {
        ffi::INVALID_VALUE
    };
    // SAFETY: made above and freed once, here; a decoded terminal stays the caller's.
    unsafe { ffi::ghostty_snapshot_decoder_free(decoder) };
    done
}
