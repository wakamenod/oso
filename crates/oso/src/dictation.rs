//! Dictation through TypeWhisper (ADR 0175): oso asks the TypeWhisper app running on the
//! person's Mac to record and recognise their speech, over TypeWhisper's local API, and types
//! the text it gives where the person was when they started.
//!
//! Must not depend on: `app`, `dispatch` or `config`; the host gives the settings, runs the
//! thread `state` starts, and carries out what it says.
//!
//! - `discovery` reads TypeWhisper's discovery file: its port and its token, checked.
//! - `api` is the calls, the curl configuration that makes them, and what an answer means.
//! - `state` is a dictation's thread, from the start to the text.
//! - `fix` is what `claude -p` is asked to fix the text with, and how its answer becomes edits
//!   (step 2). The editor keeps where the text went (`oso_core::editor`'s dictation).
//! - `learning` is the record of what fixes and the person's own rewrites taught, the history
//!   each fix is given, and the diff that finds a rewrite (step 3).
//!
//! TypeWhisper also types the text itself, by putting it on the clipboard marked with
//! [`MARKER`] and posting Command-V, which reaches oso as `M-v`. The dispatcher holds an `M-v`
//! that comes while a dictation's text is due, and drops it when the clipboard carries the
//! marker (`dispatch::paste_guard`).

pub mod api;
pub mod discovery;
pub mod fix;
pub mod learning;
pub mod state;

/// The type TypeWhisper puts beside the text it pastes on the clipboard, which tells its
/// synthetic Command-V from the person's.
pub const MARKER: &str = "com.typewhisper.SpeechTranscription";

/// The most bytes of a dictation's text typed: an hour of speech is less.
pub const MAX_TEXT: usize = 64 * 1024;

/// `text` as it is typed: without control characters but tabs and line breaks, carriage
/// returns among them, and cut to [`MAX_TEXT`] bytes at a character's end.
/// Says whether it was cut.
pub fn clean(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT));
    let mut cut = false;
    for c in text.chars() {
        let kept = match c {
            '\n' | '\t' => true,
            c => !(c.is_ascii_control() || ('\u{80}'..='\u{9f}').contains(&c)),
        };
        if !kept {
            continue;
        }
        if out.len() + c.len_utf8() > MAX_TEXT {
            cut = true;
            break;
        }
        out.push(c);
    }
    (out, cut)
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXT, clean};

    #[test]
    fn control_characters_go_and_tabs_and_line_breaks_stay() {
        assert_eq!(
            clean("a\u{1b}[31mb\r\nc\td\u{7}\u{85}e\u{0}"),
            ("a[31mb\nc\tde".to_owned(), false)
        );
        assert_eq!(clean("日本語 text"), ("日本語 text".to_owned(), false));
    }

    #[test]
    fn text_is_cut_at_a_characters_end() {
        let long = "あ".repeat(MAX_TEXT);
        let (cut, was) = clean(&long);
        assert!(was);
        assert!(cut.len() <= MAX_TEXT && cut.len() > MAX_TEXT - 3);
        assert!(cut.chars().all(|c| c == 'あ'));
        let (whole, was) = clean(&"a".repeat(MAX_TEXT));
        assert!(!was);
        assert_eq!(whole.len(), MAX_TEXT);
    }
}
