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

use crate::engine::{Effect, PromptMark};
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

/// What `XTVERSION` answers: oso's name and version. The engine answers it, from this.
pub(crate) const XTVERSION: &str = concat!("oso ", env!("CARGO_PKG_VERSION"));

pub(crate) fn judge(effect: Effect) -> Verdict {
    match effect {
        Effect::Reply(bytes) => reply(bytes),
        Effect::Bell => Verdict::Event(Event::Bell),
        Effect::Title(title) => Verdict::Event(Event::Title(text(&title))),
        Effect::Directory(uri) => directory(uri),
        Effect::Notification { title, body } => Verdict::Event(Event::Notification {
            title: text(&title),
            body: text(&body),
        }),
        Effect::Progress(progress) => Verdict::Event(Event::Progress(progress)),
        Effect::Prompt(mark) => prompt(mark),
        Effect::ClipboardWrite(data) => clipboard_write(data),
        Effect::Unknown(_) => unknown(),
    }
}

/// A report the engine answers from the terminal's own state goes back only in the shapes
/// ADR 0120's table names; anything else the library would answer is dropped:
///
/// - device attributes (`CSI ? … c`, `CSI > … c`, `DCS ! | … ST`), device status and the
///   cursor position (`CSI … n`, `CSI … R`), mode reports (`CSI ? … $ y`, `CSI … $ y`), the
///   kitty keyboard query (`CSI ? … u`), the text area and cell sizes (`CSI 4|6|8 ; … t`);
/// - XTVERSION (`DCS > | … ST`);
/// - the theme's colors (`OSC 4|10|11|12 ; … rgb:… ST|BEL`).
///
/// So `DECRQSS` (ADR 0015), XTGETTCAP, and every reply that would carry bytes the program
/// chose are denied. The library hands each reply over in one call, whole.
fn reply(bytes: Vec<u8>) -> Verdict {
    if allowed_reply(&bytes) {
        Verdict::Reply(bytes)
    } else {
        Verdict::Drop
    }
}

fn allowed_reply(bytes: &[u8]) -> bool {
    if let Some(csi) = bytes.strip_prefix(b"\x1b[") {
        csi_reply(csi)
    } else if let Some(dcs) = bytes.strip_prefix(b"\x1bP") {
        let body = dcs
            .strip_prefix(b">|")
            .or_else(|| dcs.strip_prefix(b"!|"))
            .and_then(|rest| rest.strip_suffix(b"\x1b\\"));
        body.is_some_and(|text| text.iter().all(|b| (0x20..0x7f).contains(b)))
    } else if let Some(osc) = bytes.strip_prefix(b"\x1b]") {
        color_reply(osc)
    } else {
        false
    }
}

/// `CSI`'s parameters and final byte, of the reports allowed.
fn csi_reply(csi: &[u8]) -> bool {
    let (private, rest) = match csi.first() {
        Some(&p @ (b'?' | b'>')) => (Some(p), &csi[1..]),
        _ => (None, csi),
    };
    let params_end = rest
        .iter()
        .position(|b| !(b.is_ascii_digit() || *b == b';' || *b == b':'))
        .unwrap_or(rest.len());
    let (params, end) = rest.split_at(params_end);
    let first = params.split(|b| *b == b';').next().unwrap_or_default();
    match (private, end) {
        (Some(b'?' | b'>'), b"c") | (None | Some(b'?'), b"n" | b"R" | b"$y") => true,
        (Some(b'?'), b"u") => true,
        (None, b"t") => matches!(first, b"4" | b"6" | b"8"),
        _ => false,
    }
}

/// `OSC 4 ; index`, `OSC 10`, `11` or `12`, then `rgb:` and hex digits in three parts.
fn color_reply(osc: &[u8]) -> bool {
    let body = osc
        .strip_suffix(b"\x07")
        .or_else(|| osc.strip_suffix(b"\x1b\\"));
    let Some(body) = body else {
        return false;
    };
    let color = if let Some(rest) = body.strip_prefix(b"4;") {
        let digits = rest.iter().take_while(|b| b.is_ascii_digit()).count();
        (digits > 0)
            .then(|| &rest[digits..])
            .and_then(|r| r.strip_prefix(b";"))
    } else {
        [b"10;".as_slice(), b"11;", b"12;"]
            .iter()
            .find_map(|head| body.strip_prefix(*head))
    };
    color
        .and_then(|c| c.strip_prefix(b"rgb:"))
        .is_some_and(|hex| {
            hex.split(|b| *b == b'/').count() == 3
                && hex.iter().all(|b| b.is_ascii_hexdigit() || *b == b'/')
        })
}

fn prompt(mark: PromptMark) -> Verdict {
    let mark = match mark {
        PromptMark::Start => Prompt::Start,
        PromptMark::Input => Prompt::Input,
        PromptMark::Output(command) => {
            let command = text(&command);
            Prompt::Output {
                command: (!command.is_empty()).then_some(command),
            }
        }
        PromptMark::End(exit) => Prompt::End { exit },
    };
    Verdict::Event(Event::Prompt(mark))
}

/// OSC 7's URI sets the terminal's directory, so it is kept as sent, not cleaned for showing:
/// one that is not UTF-8, holds a control character or is longer than 4 KiB is dropped
/// whole, never cut into another path. Whether it names a local directory is the host's.
fn directory(uri: Vec<u8>) -> Verdict {
    let Ok(uri) = String::from_utf8(uri) else {
        return Verdict::Drop;
    };
    if uri.is_empty() || uri.len() > limits::DIRECTORY_MAX || uri.chars().any(char::is_control) {
        return Verdict::Drop;
    }
    Verdict::Event(Event::Directory(uri))
}

/// Clipboard writes (OSC 52, and what the library reports through the same callback) go to
/// the kill ring, at most 1 MiB once read as text (the engine drops larger ones); whether
/// they reach the OS clipboard is the host's (ADR 0120, Q8).
fn clipboard_write(data: Vec<u8>) -> Verdict {
    let text = String::from_utf8(data)
        .unwrap_or_else(|invalid| String::from_utf8_lossy(invalid.as_bytes()).into_owned());
    if text.len() > limits::CLIPBOARD_WRITE_MAX {
        return Verdict::Drop;
    }
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
