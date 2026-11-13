//! oso's escape-sequence policy (ADR 0120's table): each effect the engine reports is
//! allowed, denied, or answered, by one function per kind. A denied effect is dropped without
//! a reply. Text a sequence carries for oso to show is cleaned and cut here.
//!
//! Must not depend on: the thread, or any state: each function sees one effect.
//!
//! What the library itself is told, so that it never asks (no clipboard reads, no title
//! report, no checksum report, no window moves, no Kitty images until step 12), is in the
//! engine's settings; this judges what it still reports.

use oso_core::display_map::cell::is_invisible;

use crate::engine::Effect;
use crate::event::{Event, Prompt};
use crate::limits;

/// What becomes of one effect.
#[derive(Debug)]
pub(crate) enum Verdict {
    /// Write these bytes back to the program.
    Reply(Vec<u8>),
    /// Tell the host.
    Event(Event),
    /// Denied: nothing happens.
    Drop,
}

pub(crate) fn judge(effect: Effect) -> Verdict {
    match effect {
        Effect::Reply(bytes) => reply(bytes),
        Effect::Bell => Verdict::Event(Event::Bell),
        Effect::Title(title) => Verdict::Event(Event::Title(text(&title))),
        Effect::Directory(uri) => Verdict::Event(Event::Directory(text(&uri))),
        Effect::Notification { title, body } => Verdict::Event(Event::Notification {
            title: text(&title),
            body: text(&body),
        }),
        Effect::Progress(progress) => Verdict::Event(Event::Progress(progress)),
        Effect::Prompt(mark, command) => prompt(mark, &command),
        Effect::ClipboardWrite(data) => clipboard_write(data),
        Effect::Unknown(_) => unknown(),
    }
}

/// A report the engine answers from the terminal's own state (device attributes and status,
/// cursor position, modes, the kitty keyboard query, the text area's size, XTVERSION, the
/// theme's colors) goes back. `DECRQSS` is denied (ADR 0015): the library answers it through
/// the same path, so its reply, `DCS 0 $ r` or `DCS 1 $ r`, is dropped here.
fn reply(bytes: Vec<u8>) -> Verdict {
    let decrqss = [
        b"\x1bP1$r".as_slice(),
        b"\x1bP0$r",
        b"\x90\x31$r",
        b"\x90\x30$r",
    ];
    if decrqss.iter().any(|head| bytes.starts_with(head)) {
        Verdict::Drop
    } else {
        Verdict::Reply(bytes)
    }
}

fn prompt(mark: Prompt, command: &[u8]) -> Verdict {
    let mark = match mark {
        Prompt::Output { .. } => {
            let command = text(command);
            Prompt::Output {
                command: (!command.is_empty()).then_some(command),
            }
        }
        other => other,
    };
    Verdict::Event(Event::Prompt(mark))
}

/// OSC 52 writes go to the kill ring, at most 1 MiB (the engine drops larger ones); whether
/// they reach the OS clipboard is the host's (ADR 0120, Q8).
fn clipboard_write(data: Vec<u8>) -> Verdict {
    if data.len() > limits::CLIPBOARD_WRITE_MAX {
        return Verdict::Drop;
    }
    let text = String::from_utf8(data)
        .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned());
    Verdict::Event(Event::ClipboardWrite(text))
}

/// Sequences the engine does not know are denied: OSC 51, iTerm2's OSC 1337, OSC 50 and the
/// rest. oso's own shell command sequence comes with ADR 0120's step 10.
fn unknown() -> Verdict {
    Verdict::Drop
}

/// `bytes` as text for oso to show: invalid UTF-8 replaced, control, bidi and invisible
/// characters dropped, and cut to 256 bytes on a character boundary.
pub(crate) fn text(bytes: &[u8]) -> String {
    let mut out = String::new();
    for c in String::from_utf8_lossy(bytes).chars() {
        if c.is_control() || is_invisible(c) {
            continue;
        }
        if out.len() + c.len_utf8() > limits::TEXT_MAX {
            break;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests;
