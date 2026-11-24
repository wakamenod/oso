//! A reply's Markdown as it streams: that it is the one drawn whole, and that a crafted one draws
//! little more than it holds.
//!
//! Must not depend on: rows, the syntax threads or the theme.

use proptest::prelude::*;

use super::super::{LineStart, MAX_MARKED, MAX_QUOTES, MAX_RULES, again, walk};
use super::shown;

#[test]
fn a_crafted_reply_draws_little_more_than_it_holds() {
    // Of any one mark, a whole reply of it.
    for unit in [
        "\n---\n",
        ">",
        "> ",
        "- ",
        "\n- a",
        "*",
        "*a* ",
        "**a",
        "`",
        "`` ` ",
        "[a](",
        "[a](b) ",
        "[a](https://b) ",
        "\\*",
        "```\n",
        "```\n▸\n",
        "a\n```\nb\n```\n",
        "|",
        "# a\n",
        "    ▸\n",
    ] {
        let text = unit.repeat((64 << 10) / unit.len());
        let drawn = shown(&text);
        let bound = 3 * text.len() + 96 * usize::from(MAX_RULES);
        assert!(
            drawn.len() <= bound,
            "{unit:?}: {} of {}",
            drawn.len(),
            text.len()
        );
    }
    let deep = format!("{} a", ">".repeat(MAX_QUOTES + 2));
    assert_eq!(shown(&deep), format!("{}>> a\n", "│ ".repeat(MAX_QUOTES)));
    // A long line is as written; those around it are not.
    let long = "**a** ".repeat(MAX_MARKED / 6 + 1);
    assert_eq!(
        shown(&format!("*b*\n{long}\n*c*")),
        format!("b\n{long}\nc\n")
    );
}

/// The lines drawn of `text` from `at`, `width` cells wide, and where the next starts.
fn from(text: &str, at: LineStart, streams: bool, width: usize) -> (Vec<String>, LineStart) {
    let mut lines = Vec::new();
    let mark = walk(text, at, streams, width, |pieces, _| {
        lines.push(pieces.iter().map(|piece| piece.text).collect());
    });
    (lines, mark)
}

proptest! {
    /// Drawn as it streams, cut anywhere, then whole: the same lines, each piece of the text
    /// what it says it is, and the text's letters in order.
    #[test]
    fn a_reply_that_streamed_is_the_reply_whole(
        parts in prop::collection::vec(prop::sample::select(vec![
            "\n", "\n\n", "\r\n", " ", "  ", "    ", "a", "é", "字", "*", "**", "***", "`", "```", "```rust\n",
            "```\n", "~~~\n", "# ", "- ", "1. ", "> ", "---", "[", "](", "](https://a.b)", ")", "|", "\\", "▸ ",
            "<b>", "_", "\t", "| a | b |\n", "|---|:-:|\n", "|-|-|", "| é | 字", "\u{202e}", "\u{200b}", "\r", "\u{2028}", "\u{301}", "(", "\"", ",", ".", "x",
        ]), 0..60),
        width in 0usize..40,
    ) {
        let text: String = parts.concat();
        let (whole, _) = from(&text, LineStart::default(), false, width);
        let mut streamed = String::new();
        let mut mark = LineStart::default();
        let mut kept: Vec<String> = Vec::new();
        for part in &parts {
            streamed.push_str(part);
            mark = again(&streamed, mark, true);
            kept.truncate(mark.lines);
            let (lines, next) = from(&streamed, mark, true, width);
            kept.extend(lines);
            prop_assert!(next.from >= mark.from && next.lines <= kept.len());
            mark = next;
            prop_assert_eq!(&kept, &from(&streamed, LineStart::default(), true, width).0);
        }
        mark = again(&text, mark, false);
        kept.truncate(mark.lines);
        kept.extend(from(&text, mark, false, width).0);
        prop_assert_eq!(&kept, &whole);

        let mut at = 0;
        walk(&text, LineStart::default(), false, width, |pieces, _| {
            for piece in pieces {
                assert!(!piece.text.is_empty() && !piece.text.contains('\n'));
                if let Some(from) = piece.from {
                    assert_eq!(&text[from..from + piece.text.len()], piece.text);
                    assert!(from >= at, "in order");
                    at = from + piece.text.len();
                }
            }
        });
        // No letter, sign or invisible character is hidden: only marks, fences and targets.
        for c in ['é', '字', '▸', '\u{202e}', '\u{200b}', '\u{2028}', '"', ','] {
            let drawn: usize = whole.iter().map(|line| line.matches(c).count()).sum();
            let written = text.matches(c).count();
            // A blank line's are not drawn; none of these is blank.
            prop_assert_eq!(drawn, written, "{}", c);
        }
    }
}
