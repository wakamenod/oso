//! The recap of a native session's turn that ended out of the person's sight (ADR 0165, part
//! 3f): `NAME (last reply): SENTENCE`, the first sentence of the turn's last reply, said in
//! the echo area when they come back to it.
//!
//! Must not depend on: `app` or `dispatch`; plain functions over the rows and the texts given.
//!
//! - It is Claude's text: escaped as `*Messages*` is ([`oso_core::editor::row_text`]), so that
//!   it stays on one row and cannot reorder it, and cut to the cells given.
//! - `(last reply)` marks it as the session's quoted words, not oso's own message (the
//!   author's choice).
//! - What it reads is bounded: the last [`MAX_ROWS`] rows, and the first [`MAX_LOOKED`] bytes
//!   of a reply.

use oso_core::editor::row_text;
use oso_core::minibuffer::cut_to;

use super::rows::{Kind, Row};

/// The most bytes of a reply looked through.
const MAX_LOOKED: usize = 1024;
/// The most rows looked back over for the last reply.
const MAX_ROWS: usize = 10_000;
/// What follows the session's name: the sentence is its own, not oso's.
const MARK: &str = " (last reply): ";

/// The first [`MAX_LOOKED`] bytes of `text`.
fn head(text: &str) -> &str {
    &text[..text.floor_char_boundary(MAX_LOOKED)]
}

/// The last reply of the last turn of `rows`, the session's own: the newest with text after
/// the person's last message and the turn before's end, the last turn's own end passed over.
pub fn last_reply(rows: &[Row]) -> Option<&str> {
    let mut ended = false;
    for row in rows.iter().rev().take(MAX_ROWS) {
        match &row.kind {
            Kind::Prompt(_) => return None,
            Kind::Result(_) if ended => return None,
            Kind::Result(_) => ended = true,
            Kind::Reply { text, .. } if row.depth == 0 && !head(text).trim().is_empty() => {
                return Some(text);
            }
            _ => {}
        }
    }
    None
}

/// The first sentence of `reply`: up to its first `.`, `!` or `?` that whitespace or the end
/// follows, or `。`, `！` or `？`, or the end of its first line with text, a Markdown heading's,
/// list item's or quote's mark left out.
fn first_sentence(reply: &str) -> &str {
    let Some(line) = head(reply)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
    else {
        return "";
    };
    let line = line
        .trim_start_matches('#')
        .trim_start_matches(['>', '-', '*', '+'])
        .trim_start();
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let end = at + c.len_utf8();
        match c {
            '。' | '！' | '？' => return &line[..end],
            '.' | '!' | '?' if chars.peek().is_none_or(|(_, next)| next.is_whitespace()) => {
                return &line[..end];
            }
            _ => {}
        }
    }
    line
}

/// The recap of session `name` whose last reply is `reply`, cut to `cells` with `…`: `NAME
/// (last reply): SENTENCE`, escaped. Nothing when the reply says nothing.
pub fn recap(name: &str, reply: &str, cells: usize) -> Option<String> {
    let sentence = first_sentence(reply);
    if sentence.is_empty() {
        return None;
    }
    let line = row_text(&format!("{name}{MARK}{sentence}"));
    let (kept, cut) = cut_to(&line, cells.max(1));
    Some(format!("{kept}{cut}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::native::wire::TurnResult;

    fn row(kind: Kind, depth: u8) -> Row {
        Row {
            kind,
            depth,
            folded: false,
        }
    }

    fn reply(text: &str) -> Kind {
        Kind::Reply {
            text: text.to_owned(),
            streaming: false,
            cut: false,
        }
    }

    fn ended() -> Kind {
        Kind::Result(TurnResult {
            subtype: "success".into(),
            error: false,
            reason: None,
            millis: None,
            cost_usd: None,
            turns: None,
            denials: 0,
            session_id: String::new(),
            windows: Vec::new(),
        })
    }

    #[test]
    fn the_last_reply_is_the_newest_of_the_last_turn_and_the_sessions_own() {
        let rows = [
            row(Kind::Prompt("one".into()), 0),
            row(reply("Old."), 0),
            row(ended(), 0),
            row(Kind::Prompt("two".into()), 0),
            row(reply("Mine."), 0),
            row(reply("A subagent's."), 1),
            row(Kind::Note("note".into()), 0),
            row(ended(), 0),
        ];
        assert_eq!(last_reply(&rows), Some("Mine."));
        assert_eq!(last_reply(&rows[..4]), None, "the turn said nothing yet");
        assert_eq!(last_reply(&[]), None);
        // A turn no message of the person's started stops at the turn before's end.
        let unprompted = [
            row(reply("Old."), 0),
            row(ended(), 0),
            row(Kind::Note("from elsewhere".into()), 0),
            row(ended(), 0),
        ];
        assert_eq!(last_reply(&unprompted), None);
        // A blank reply is passed over, looked at only at its start.
        let blank = [
            row(reply("Said."), 0),
            row(reply(&" ".repeat(MAX_LOOKED + 10)), 0),
            row(ended(), 0),
        ];
        assert_eq!(last_reply(&blank), Some("Said."));
    }

    #[test]
    fn the_last_reply_is_looked_for_in_the_last_rows_only() {
        let mut rows = vec![row(reply("Far back."), 0)];
        rows.extend((0..MAX_ROWS).map(|_| row(Kind::Note("n".into()), 0)));
        assert_eq!(last_reply(&rows), None);
        rows.remove(1);
        assert_eq!(last_reply(&rows), Some("Far back."));
    }

    #[test]
    fn the_first_sentence_ends_where_a_sentence_does() {
        let cases = [
            ("Done. The tests pass.", "Done."),
            ("Version 1.5 is out! Next.", "Version 1.5 is out!"),
            ("Is it? Yes", "Is it?"),
            ("完了しました。次は", "完了しました。"),
            ("\n\n## Summary\nText.", "Summary"),
            ("- Fixed the bug\n- Added a test", "Fixed the bug"),
            ("> quoted. rest", "quoted."),
            ("no stop at all", "no stop at all"),
            ("ends with a stop.", "ends with a stop."),
            ("", ""),
            ("   \n  ", ""),
        ];
        for (reply, sentence) in cases {
            assert_eq!(first_sentence(reply), sentence, "{reply:?}");
        }
        // A reply's first line is looked at up to its limit only.
        let long = "a".repeat(5000);
        assert_eq!(first_sentence(&long).len(), MAX_LOOKED);
    }

    #[test]
    fn the_recap_is_escaped_and_cut_to_its_cells() {
        assert_eq!(
            recap("proj", "Done. More.", 80).as_deref(),
            Some("proj (last reply): Done.")
        );
        let said = recap("p\u{202e}j", "x\u{1b}y.", 80).expect("said");
        assert!(!said.contains('\u{202e}') && !said.contains('\u{1b}'));
        let cut = recap("proj", &format!("{}.", "word ".repeat(40)), 20).expect("said");
        assert!(cut.ends_with('…'));
        assert!(oso_core::minibuffer::cells_of(&cut) <= 20);
        assert_eq!(recap("proj", "  ", 80), None);
    }
}
