//! The cursor's line for the input method (ADR 0070, ADR 0077): a bounded excerpt of the
//! selected window's line around its primary cursor, which the input method may read for
//! reconversion and better predictions.
//!
//! Must not depend on: the window layer, or how the input method counts text. The excerpt is
//! never logged.
//!
//! - It holds at most [`EXCERPT_REACH`] bytes before the cursor and as many after, cut on
//!   grapheme cluster boundaries and never past the line's ends. Its ends are found by
//!   walking at most those bytes from the cursor, never back to the start of the line, so a
//!   long line costs no more than a short one.
//! - Only a buffer whose verdict is `Ordinary`, and not an internal one, gives it, and none
//!   comes while a prompt is selected.

use unicode_segmentation::UnicodeSegmentation;

use super::Editor;
use crate::rope::Rope;
use crate::secret::Verdict;
use crate::sum_tree::Bias;

/// The most bytes of the cursor's line given on each side of the cursor.
pub(crate) const EXCERPT_REACH: usize = 2000;

impl Editor {
    /// The cursor's line for the input method: into `out`, emptied first, the selected
    /// window's line around its primary cursor, bounded as the module says. Returns where the
    /// cursor is in it, in bytes, or `None`, with `out` empty, for a buffer that may hold
    /// secrets or is internal, and while a prompt is selected.
    pub fn cursor_excerpt(&self, out: &mut String) -> Option<usize> {
        out.clear();
        let (window, buffer) = self.current();
        let gives = buffer.verdict() == Verdict::Ordinary && !buffer.is_internal();
        if !gives || self.prompt().is_some() {
            return None;
        }
        Some(excerpt(
            buffer.text(),
            window.selections.primary().cursor,
            out,
        ))
    }
}

/// The line of `text` around `cursor`, bounded as the module says, into `out`, which is
/// emptied first. Returns where the cursor is in it.
fn excerpt(text: &Rope, cursor: usize, out: &mut String) -> usize {
    out.clear();
    let cursor = text.clip_offset(cursor, Bias::Left);
    // The bytes within reach, on character boundaries.
    let start = text.clip_offset(cursor.saturating_sub(EXCERPT_REACH), Bias::Right);
    let end = text.clip_offset(cursor.saturating_add(EXCERPT_REACH), Bias::Left);
    for chunk in text.chunks_in_range(start..end) {
        out.push_str(chunk);
    }
    let at = cursor - start;
    // The line's ends, where they are within reach.
    let line_start = out[..at].rfind('\n').map(|i| i + 1);
    let line_end = out[at..].find('\n').map(|i| at + i);
    // Otherwise the reach cut the line, maybe inside a cluster: the cut moves inward to the
    // first boundary the window's own clusters give.
    let head = line_start.unwrap_or_else(|| {
        if start == 0 {
            0
        } else {
            out.grapheme_indices(true)
                .nth(1)
                .map_or(at, |(i, _)| i.min(at))
        }
    });
    let tail = line_end.unwrap_or_else(|| {
        if end == text.len() {
            out.len()
        } else {
            out.grapheme_indices(true)
                .next_back()
                .map_or(at, |(i, _)| i.max(at))
        }
    });
    // A line that ends in CR LF ends before the CR.
    let tail = if line_end.is_some() && out[..tail].ends_with('\r') && tail > at {
        tail - 1
    } else {
        tail
    };
    out.truncate(tail);
    out.drain(..head);
    at - head
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn cut(text: &str, cursor: usize) -> (String, usize) {
        let mut out = String::from("old");
        let at = excerpt(&Rope::from(text), cursor, &mut out);
        (out, at)
    }

    #[test]
    fn the_excerpt_is_the_cursors_line() {
        assert_eq!(cut("one\ntwo three\nfour", 8), ("two three".into(), 4));
        assert_eq!(cut("one\r\ntwo\r\n", 7), ("two".into(), 2), "CR LF");
        assert_eq!(cut("", 0), (String::new(), 0));
        assert_eq!(cut("ab\n", 3), (String::new(), 0), "the empty last line");
        assert_eq!(cut("かな", 4), ("かな".into(), 3), "inside a character");
    }

    #[test]
    fn a_long_line_gives_at_most_its_reach_on_each_side() {
        let line = "あ".repeat(400_000);
        let cursor = 3 * 200_000;
        let (out, at) = cut(&line, cursor);
        assert!(at <= EXCERPT_REACH && out.len() - at <= EXCERPT_REACH);
        // A cut drops the cluster it may have split, two characters here at most.
        assert!(at > EXCERPT_REACH - 6 && out.len() - at > EXCERPT_REACH - 6);
        assert!(out.chars().all(|c| c == 'あ'));
    }

    #[test]
    fn a_cut_falls_between_clusters() {
        // Accents join the letter before them: no cut leaves one alone at an end.
        let line = "e\u{301}".repeat(2000);
        let cursor = 3 * 1000;
        let (out, at) = cut(&line, cursor);
        assert!(out.starts_with('e') && out.ends_with('\u{301}'));
        assert_eq!(&out[at..at + 1], "e");
    }

    #[test]
    fn only_an_ordinary_buffer_gives_its_line() {
        let mut out = String::new();
        let editor = Editor::with_buffer(Buffer::from_text("t", "one\ntwo"));
        assert_eq!(editor.cursor_excerpt(&mut out), Some(0));
        assert_eq!(out, "one");
        let secret = Editor::with_buffer(Buffer::visiting(".env".into()));
        assert_eq!(secret.cursor_excerpt(&mut out), None);
        assert!(out.is_empty());
    }
}
