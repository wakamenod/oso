//! The cursor's line for the input method (ADR 0070, ADR 0077): a bounded excerpt of the
//! selected window's line around its primary cursor, which the input method may read for
//! reconversion and better predictions.
//!
//! Must not depend on: the window layer, or how the input method counts text. The excerpt is
//! never logged.
//!
//! - It holds at most [`EXCERPT_REACH`] bytes before the cursor and as many after, cut where
//!   a cursor can stop (display clusters, so a flag or an emoji family is never split) and
//!   never past the line's ends. Its ends are found through the display map, never by
//!   walking back to the start of the line, so a long line costs no more than a short one.
//! - Only a buffer whose verdict is `Ordinary`, and not an internal one, gives it, and none
//!   comes while a prompt is selected. Nothing is read from any other.

use super::Editor;
use crate::buffer::BufferId;
use crate::display_map::DisplaySnapshot;
use crate::motion;
use crate::rope::Rope;
use crate::secret::Verdict;
use crate::sum_tree::Bias;

/// The most bytes of the cursor's line given on each side of the cursor.
const EXCERPT_REACH: usize = 2000;

impl Editor {
    /// What [`Editor::cursor_excerpt`] gives depends on: the selected buffer, its revision and
    /// the primary cursor, or `None` where it gives nothing. The host gives the excerpt again
    /// when this changes.
    pub fn excerpt_stamp(&self) -> Option<(BufferId, u64, usize)> {
        let (window, buffer) = self.current();
        let gives = buffer.verdict() == Verdict::Ordinary && !buffer.is_internal();
        (gives && self.prompt().is_none()).then(|| {
            let cursor = window.selections.primary().cursor;
            (window.buffer(), buffer.revision(), cursor)
        })
    }

    /// The cursor's line for the input method: into `out`, emptied first, the selected
    /// window's line around its primary cursor, bounded as the module says. Returns where the
    /// cursor is in it, in bytes, or `None`, with `out` empty and nothing read, where
    /// [`Editor::excerpt_stamp`] is `None`.
    pub fn cursor_excerpt(&self, out: &mut String) -> Option<usize> {
        out.clear();
        let (_, _, cursor) = self.excerpt_stamp()?;
        let buffer = self.current().1;
        let text = buffer.text();
        let cursor = text.clip_offset(cursor, Bias::Left);
        let (start, end) = excerpt_ends(text, &buffer.display(), cursor);
        for chunk in text.chunks_in_range(start..end) {
            out.push_str(chunk);
        }
        Some(cursor - start)
    }
}

/// The ends of the excerpt of `text` around `cursor`, which is where a cursor can stop: the
/// line's ends where they are within reach, and otherwise the nearest stops inside the reach.
fn excerpt_ends(text: &Rope, display: &DisplaySnapshot, cursor: usize) -> (usize, usize) {
    let row = display.offset_to_display_point(cursor).row;
    let line_start = motion::line_start(display, row);
    let line_end = motion::line_end(display, row).max(cursor);
    let reach_start = text.clip_offset(cursor.saturating_sub(EXCERPT_REACH), Bias::Right);
    let reach_end = text.clip_offset(cursor.saturating_add(EXCERPT_REACH), Bias::Left);
    let start = if line_start >= reach_start {
        line_start
    } else {
        motion::cluster_end(display, reach_start)
    };
    let end = if line_end <= reach_end {
        line_end
    } else {
        motion::cluster_start(display, reach_end)
    };
    (start.min(cursor), end.max(cursor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;
    use crate::minibuffer::Kind;
    use crate::selection::SelectionSet;

    fn cut(text: &str, cursor: usize) -> (String, usize) {
        let mut editor = Editor::with_buffer(Buffer::from_text("t", text));
        editor.current_mut().0.selections = SelectionSet::new(cursor);
        let mut out = String::from("old");
        let at = editor.cursor_excerpt(&mut out).unwrap();
        (out, at)
    }

    #[test]
    fn the_excerpt_is_the_cursors_line() {
        assert_eq!(cut("one\ntwo three\nfour", 8), ("two three".into(), 4));
        assert_eq!(cut("one\r\ntwo\r\n", 7), ("two".into(), 2), "CR LF");
        assert_eq!(cut("", 0), (String::new(), 0));
        assert_eq!(cut("ab\n", 3), (String::new(), 0), "the empty last line");
    }

    #[test]
    fn a_long_line_gives_at_most_its_reach_on_each_side() {
        let line = "あ".repeat(400_000);
        let (out, at) = cut(&line, 3 * 200_000);
        assert!(at <= EXCERPT_REACH && out.len() - at <= EXCERPT_REACH);
        assert!(at > EXCERPT_REACH - 3 && out.len() - at > EXCERPT_REACH - 3);
        assert!(out.chars().all(|c| c == 'あ'));
    }

    #[test]
    fn a_cut_never_splits_what_a_cursor_steps_over() {
        // Flags pair regional indicators from the start of their run, and a family joins
        // its people with zero-width joiners: a cut inside one moves to its edge.
        for unit in ["🇯🇵", "👨\u{200d}👩\u{200d}👧", "e\u{301}"] {
            let line = unit.repeat(3000);
            let (out, at) = cut(&line, unit.len() * 1500);
            assert!(
                at <= EXCERPT_REACH && out.len() - at <= EXCERPT_REACH,
                "{unit:?}"
            );
            assert_eq!(out.len() % unit.len(), 0, "{unit:?}");
            assert_eq!(at % unit.len(), 0, "{unit:?}");
            assert!(out.starts_with(unit) && out.ends_with(unit), "{unit:?}");
        }
    }

    #[test]
    fn only_an_ordinary_buffer_outside_a_prompt_gives_its_line() {
        let mut out = String::from("old");
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "one\ntwo"));
        assert_eq!(editor.cursor_excerpt(&mut out), Some(0));
        assert_eq!(out, "one");
        let mut none = |editor: &Editor, why: &str| {
            assert_eq!(editor.excerpt_stamp(), None, "{why}");
            assert_eq!(editor.cursor_excerpt(&mut out), None, "{why}");
            assert!(out.is_empty(), "{why}");
        };
        none(
            &Editor::with_buffer(Buffer::visiting(".env".into())),
            "unjudged",
        );
        let mut named = Buffer::visiting(".env".into());
        named.judge_secret(None);
        assert_eq!(named.verdict(), Verdict::SecretName);
        none(&Editor::with_buffer(named), "a secret name");
        let mut unresolved = Buffer::visiting("notes.txt".into());
        unresolved.judge_unresolved(&[]);
        assert_eq!(unresolved.verdict(), Verdict::Unresolved);
        none(&Editor::with_buffer(unresolved), "unresolved");
        let internal = Buffer::internal("*Messages*");
        assert_eq!(internal.verdict(), Verdict::Ordinary);
        none(&Editor::with_buffer(internal), "internal");
        let question = Kind::Question {
            yes: "exit",
            verbs: ["exit", "stay"],
        };
        editor.open_prompt("Exit? ", question).unwrap();
        none(&editor, "a question over an ordinary buffer");
        let mut editor = Editor::with_buffer(Buffer::from_text("t", "one"));
        editor
            .open_prompt("Passphrase: ", Kind::Passphrase)
            .unwrap();
        none(&editor, "a passphrase");
    }
}
