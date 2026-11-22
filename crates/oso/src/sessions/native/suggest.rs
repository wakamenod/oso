//! The CLI's prompt suggestion in a native session (ADR 0165, part 3f): which suggestion is
//! kept, and its dim row at the transcript's end.
//!
//! Must not depend on: `app` or `dispatch`; plain functions over the text given.
//!
//! - A suggestion is Claude's text, which what Claude read can steer, and `M-n` puts it into
//!   the composer as it is, unescaped but for part 2f's escapes. So only one the row shows as
//!   it is is kept: a line break, a control, bidi or invisible character, anything that
//!   `*Messages*` would escape ([`oso_core::editor::row_text`]) or that draws as nothing, a
//!   variation selector say ([`is_invisible`]), makes it none, so that what goes into the
//!   composer is what the row showed. The CLI's suggestions are a short line.
//! - `wire` cuts it to 256 bytes.

use oso_core::display_map::cell::is_invisible;
use oso_core::editor::row_text;

/// What starts the suggestion's row.
const SUGGESTED: &str = "Suggested: ";

/// `text` as the suggestion kept, trimmed: none if blank, if its row would show it escaped, or
/// if it holds what draws as nothing.
pub fn suggestion(text: &str) -> Option<String> {
    let text = text.trim();
    let shown = !text.is_empty() && row_text(text) == text && !text.chars().any(is_invisible);
    shown.then(|| text.to_owned())
}

/// The row of `text`, a [`suggestion`] kept, ending in `\n`: `Suggested: TEXT`.
pub fn suggestion_row(text: &str) -> String {
    // Escaped again, should a caller give one not kept: the row stays one line.
    format!("{SUGGESTED}{}\n", row_text(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::native::wire::{Event, parse};

    /// `wire`'s reading of the line, tested here to keep part 3f's tests together.
    #[test]
    fn a_suggestion_reads_cut_and_an_empty_one_is_none() {
        let line = serde_json::json!({
            "type": "prompt_suggestion",
            "suggestion": "run the tests",
            "uuid": "u",
            "session_id": "s",
        });
        assert_eq!(
            parse(line.to_string().as_bytes()),
            Event::Suggestion("run the tests".to_owned())
        );
        let long = serde_json::json!({
            "type": "prompt_suggestion",
            "suggestion": "é".repeat(400),
        });
        let Event::Suggestion(text) = parse(long.to_string().as_bytes()) else {
            panic!("not read");
        };
        assert!(text.len() <= 256);
        for empty in ["", "  \n"] {
            let line = serde_json::json!({
                "type": "prompt_suggestion",
                "suggestion": empty,
            });
            assert!(matches!(
                parse(line.to_string().as_bytes()),
                Event::Other(_)
            ));
        }
    }

    #[test]
    fn only_a_suggestion_its_row_shows_as_it_is_is_kept() {
        assert_eq!(suggestion(" run it \n").as_deref(), Some("run it"));
        assert_eq!(
            suggestion("/review @notes.md \"x\" 'y' \\z 全角\u{3000}空白").as_deref(),
            Some("/review @notes.md \"x\" 'y' \\z 全角\u{3000}空白")
        );
        for hidden in [
            "run the tests\n\n\nthen push --force",
            "a\rb",
            "a\u{1b}[2Jb",
            "a\u{202e}b",
            "a\u{200b}b",
            "a\u{feff}b",
            "a\u{2028}b",
            "a\u{a0}b",
            "a\tb",
            "a\u{ad}b",
            "a\u{e0041}b",
            // Marks that draw as nothing after their base, which escaping keeps.
            "テ\u{fe00}スト",
            "é\u{e0100}",
            "é\u{34f}",
            "ア\u{180b}",
            "",
            "   ",
        ] {
            assert_eq!(suggestion(hidden), None, "{hidden:?}");
        }
    }

    #[test]
    fn the_suggestions_row_is_one_line() {
        assert_eq!(suggestion_row("run it"), "Suggested: run it\n");
        let row = suggestion_row("a\nb\u{202e}c");
        assert_eq!(row.lines().count(), 1);
        assert!(!row.contains('\u{202e}'));
    }
}
