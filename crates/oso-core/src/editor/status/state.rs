//! What a modeline says after the buffer's place: its position, its mode, its host's note and its
//! encoding.
//!
//! Must not depend on: `command`, `keymap`, or the host's window, threads or I/O.

use std::fmt::Write;
use std::ops::Range;

use super::note::NATIVE_NOTE_CELLS;
use super::{GAP, NO_POSITION, NOTE_CELLS, SESSION, push_cut};
use crate::buffer::{Buffer, Mode, Verdict};
use crate::decode::{FileInfo, LineEnding};
use crate::window::Window;

/// Writes what the modeline says of `buffer`'s state into `out`, and where a native session's
/// note is in it; `footer` if the window shows its footer, which says some of the note itself
/// (ADR 0170).
pub(super) fn push_state(
    out: &mut String,
    window: &Window,
    buffer: &Buffer,
    footer: bool,
) -> Option<Range<usize>> {
    // A terminal has no position, no file and no encoding: its mode and what the host notes,
    // such as its program's title (ADR 0120). A session's says `Session` (ADR 0132).
    if buffer.is_terminal() {
        out.push_str(if buffer.hosts_session() {
            SESSION
        } else {
            buffer.mode().name()
        });
        if !buffer.mode_note().is_empty() {
            out.push_str(GAP);
            push_cut(out, buffer.mode_note(), NOTE_CELLS);
        }
        return None;
    }
    // `*Spaces*` has no position worth saying: what the host notes, its name, grouping and
    // keys (ADR 0131, ADR 0140).
    if buffer.mode() == Mode::Sessions && !buffer.mode_note().is_empty() {
        push_cut(out, buffer.mode_note(), NOTE_CELLS);
        return None;
    }
    if let Some(progress) = buffer.loading() {
        let _ = match progress.total {
            Some(total) => write!(out, "Loading {}%{GAP}", percent(progress.read, total)),
            None => write!(out, "Loading {} MiB{GAP}", progress.read >> 20),
        };
    }
    match buffer.verdict() {
        // Nothing has loaded, and whether it may hold secrets is not known yet.
        Verdict::Unjudged => {}
        Verdict::Ordinary => {
            let point = window.cursor_point(&buffer.display());
            let _ = write!(out, "{}:{}", point.row + 1, point.column);
            let cursors = window.selections.len();
            if cursors > 1 {
                let _ = write!(out, " ({cursors} cursors)");
            }
            out.push_str(GAP);
        }
        Verdict::Unresolved
        | Verdict::SecretName
        | Verdict::Passphrase
        | Verdict::Yanked
        | Verdict::EchoOff => {
            out.push_str(NO_POSITION);
            out.push_str(GAP);
        }
    }
    out.push_str(buffer.mode().name());
    // A listing has no encoding: its order instead, as dired's `Dired by name` (ADR 0134). A git
    // status has none either (ADR 0127), nor a native session, whose state follows (ADR 0165).
    if matches!(
        buffer.mode(),
        crate::buffer::Mode::Directory
            | crate::buffer::Mode::GitStatus
            | crate::buffer::Mode::NativeSession
    ) {
        if buffer.mode() == Mode::NativeSession {
            return push_native_note(out, buffer, footer);
        }
        if !buffer.mode_note().is_empty() {
            out.push(' ');
            push_cut(out, buffer.mode_note(), NOTE_CELLS);
        }
        return None;
    }
    out.push_str(GAP);
    push_encoding(out, buffer.file_info(), !buffer.invalid_bytes().is_empty());
    None
}

/// Writes a native session's note into `out`, the one beside its footer if `footer`, and where it
/// is: beside a footer with nothing to say too, as what is before it gives way to the name alike.
fn push_native_note(out: &mut String, buffer: &Buffer, footer: bool) -> Option<Range<usize>> {
    let beside = buffer.beside_footer().filter(|_| footer);
    let note = beside.unwrap_or(buffer.mode_note());
    if note.is_empty() {
        return beside.map(|_| out.len()..out.len());
    }
    out.push(' ');
    let start = out.len();
    push_cut(out, note, NATIVE_NOTE_CELLS);
    Some(start..out.len())
}

/// How far along a load is. It stays under 100 while the load runs, since a file can
/// grow after its size was read.
pub(super) fn percent(read: u64, total: u64) -> u64 {
    if total == 0 {
        return 99;
    }
    let share = u128::from(read) * 100 / u128::from(total);
    u64::try_from(share).unwrap_or(99).min(99)
}

fn push_encoding(out: &mut String, info: FileInfo, invalid: bool) {
    out.push_str(match (info.binary, invalid, info.bom) {
        (true, _, _) => "Binary",
        (false, true, _) => "UTF-8 (invalid)",
        (false, false, true) => "UTF-8-BOM",
        (false, false, false) => "UTF-8",
    });
    out.push_str(match info.line_ending {
        LineEnding::Lf => " LF",
        LineEnding::Crlf => " CRLF",
    });
}
