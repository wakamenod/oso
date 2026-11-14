//! What a terminal writes to its program for the UI's input besides keys (ADR 0120's Input,
//! Q13): a paste, cleaned and bracketed as the program's mode is when its thread writes it,
//! and focus reports. Keys and the mouse's reports are the engine's, whose encoders read the
//! modes themselves.
//!
//! Must not depend on: the engine, the thread or any state: each function sees its arguments.

/// What bracketed paste's brackets add to a paste.
pub(crate) const BRACKETS: usize = START.len() + END.len();
const START: &[u8] = b"\x1b[200~";
const END: &[u8] = b"\x1b[201~";

/// A paste not sent: it holds `lines` lines, and was not allowed lines.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Refused {
    pub lines: usize,
}

/// The bytes a paste of `text` writes. CRLF and lone CR become LF; every other C0 control but
/// TAB, DEL and the C1 controls are removed, so a paste never carries an `ESC`, and cannot end
/// its own bracket with `ESC [ 201 ~`. With bracketed paste on, the rest goes in brackets. Off,
/// each LF is sent as CR, as RET is; a text with a line break is then refused unless
/// `lines_allowed`, as the person was asked while bracketed paste was on. An empty result
/// sends nothing, brackets included.
pub(crate) fn paste(text: &str, bracketed: bool, lines_allowed: bool) -> Result<Vec<u8>, Refused> {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                chars.next_if_eq(&'\n');
                clean.push('\n');
            }
            '\t' | '\n' => clean.push(c),
            c if c.is_control() => {}
            c => clean.push(c),
        }
    }
    if clean.is_empty() {
        return Ok(Vec::new());
    }
    if bracketed {
        let mut out = Vec::with_capacity(clean.len() + BRACKETS);
        out.extend_from_slice(START);
        out.extend_from_slice(clean.as_bytes());
        out.extend_from_slice(END);
        return Ok(out);
    }
    if clean.contains('\n') && !lines_allowed {
        return Err(Refused {
            lines: clean.lines().count(),
        });
    }
    Ok(clean.replace('\n', "\r").into_bytes())
}

/// The most bytes a paste of `len` bytes of text writes, for the input queue's reservation.
pub(crate) fn paste_bound(len: usize) -> usize {
    len.saturating_add(BRACKETS)
}

/// A focus report: `CSI I` when the terminal gains focus, `CSI O` when it loses it. Only for
/// a program that turned focus reports (mode 1004) on.
pub(crate) fn focus(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

#[cfg(test)]
mod tests {
    use super::{Refused, focus, paste, paste_bound};

    #[test]
    fn a_bracketed_paste_is_cleaned_and_wrapped() {
        let text = "a\r\nb\rc\nd\te\x1b[201~f\x7f\u{9b}g\u{85}h\x00";
        let out = paste(text, true, false).expect("sent");
        assert_eq!(out, b"\x1b[200~a\nb\nc\nd\te[201~fgh\x1b[201~");
        assert!(out.len() <= paste_bound(text.len()));
    }

    #[test]
    fn the_end_marker_cannot_be_faked() {
        let out = paste("x\x1b[201~\x1b\x1b[201~rm -rf ~\n", true, false).expect("sent");
        let inner = &out[6..out.len() - 6];
        assert!(!inner.contains(&0x1b), "{out:?}");
        assert!(out.ends_with(b"\x1b[201~"));
    }

    #[test]
    fn an_unbracketed_paste_sends_cr_and_needs_lines_allowed() {
        assert_eq!(paste("ls", false, false), Ok(b"ls".to_vec()));
        assert_eq!(paste("a\nb\r\nc", false, false), Err(Refused { lines: 3 }));
        assert_eq!(paste("a\n", false, false), Err(Refused { lines: 1 }));
        assert_eq!(paste("a\rb", false, false), Err(Refused { lines: 2 }));
        assert_eq!(paste("a\nb\r\nc", false, true), Ok(b"a\rb\rc".to_vec()));
    }

    #[test]
    fn an_empty_result_sends_nothing() {
        assert_eq!(paste("", true, false), Ok(Vec::new()));
        assert_eq!(paste("\x1b\x07\u{9b}", true, false), Ok(Vec::new()));
        assert_eq!(paste("\x00", false, false), Ok(Vec::new()));
    }

    #[test]
    fn focus_reports() {
        assert_eq!(focus(true), b"\x1b[I");
        assert_eq!(focus(false), b"\x1b[O");
    }
}
