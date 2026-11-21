//! YAML's indentation rules (ADR 0164), as Emacs's yaml-mode has them: indentation is YAML's
//! structure, so no line is ever reindented; a new line follows the line before, one level
//! deeper after a line that opens a block, and `TAB` cycles a line through the levels it could
//! take.
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it reads the line before.
//!
//! - A line opens a block when, past a comment, it ends with `:`, or with a block scalar's
//!   `|` or `>` (with its indicators), or it is a list item's `-` alone. A new line after it
//!   goes one level past the line's content: past its `- ` markers, so a mapping in a list item
//!   nests under the item's key. After any other line, it takes that line's indentation.
//! - `TAB`'s stops: the deepest a line could go, then each level out to column 0.

use super::pass::{self, Head, Lexer};
use super::{Answer, Budget, Settings};
use crate::rope::Rope;

/// The code line before: where its content starts past its `- ` markers, and where a new
/// line after it goes.
#[derive(Clone, Copy, Debug)]
struct Previous {
    content: usize,
    next: usize,
}

pub(super) struct Yaml {
    settings: Settings,
    previous: Option<Previous>,
}

impl Lexer for Yaml {
    fn fresh(settings: Settings) -> Self {
        Self {
            settings,
            previous: None,
        }
    }

    /// Any line at column 0 that is not a comment: YAML keeps no state across lines that
    /// the rules read.
    fn is_anchor(line: &[u8]) -> bool {
        line.first().is_some_and(|&byte| byte != b'#')
    }

    fn is_inside(&self) -> bool {
        false
    }

    fn line(&mut self, text: &str, indent: usize) {
        let content = text.trim_start_matches([' ', '\t']).as_bytes();
        if content.is_empty() || content == b"\r" || content.starts_with(b"#") {
            return;
        }
        self.previous = Some(previous(self.settings, content, indent));
    }

    fn answer(&self, _head: &Head, new: bool) -> Option<Answer> {
        if !new {
            return Some(Answer::Keep);
        }
        Some(Answer::Column(
            self.previous.map_or(0, |previous| previous.next),
        ))
    }

    fn relative(settings: Settings, line: &[u8], column: usize, _head: &[u8]) -> usize {
        previous(settings, line, column).next
    }
}

/// What a line whose text past its indentation `indent` is `content` leaves for the next:
/// after a `-` alone, the item's content column; after a line that opens a block, one level
/// past its content; after any other, its indentation.
fn previous(settings: Settings, content: &[u8], indent: usize) -> Previous {
    let (markers, rest) = markers(content);
    let column = indent.saturating_add(markers);
    let next = if rest.is_empty() && markers > 0 {
        column
    } else if opens(rest) {
        column.saturating_add(settings.indent_size())
    } else {
        indent
    };
    Previous {
        content: column,
        next,
    }
}

/// The columns of the `- ` markers `content` starts with, as in `- - a`, and what follows
/// them. A `-` that ends the line counts as `- `, with nothing after it.
fn markers(content: &[u8]) -> (usize, &[u8]) {
    let mut at = 0;
    while content[at..].starts_with(b"-") {
        let after = content.get(at + 1).copied();
        match after {
            None | Some(b'\r') => return (at + 2, &[]),
            Some(b' ' | b'\t') => {
                at += 1;
                at += content[at..]
                    .iter()
                    .take_while(|byte| matches!(byte, b' ' | b'\t'))
                    .count();
            }
            Some(_) => break,
        }
    }
    (at, &content[at..])
}

/// Whether a line whose text past its markers is `rest` opens a block: it ends, past a
/// comment, with `:`, or a block scalar's indicator, or nothing follows its markers.
fn opens(rest: &[u8]) -> bool {
    let code = without_comment(rest).trim_ascii_end();
    if code.is_empty() {
        return true;
    }
    if code.ends_with(b":") {
        return true;
    }
    // A block scalar: `|` or `>`, then its chomping and indentation indicators.
    let last = code
        .rsplit(|byte| matches!(byte, b' ' | b'\t'))
        .next()
        .unwrap_or(code);
    matches!(last.first(), Some(b'|' | b'>'))
        && last[1..]
            .iter()
            .all(|byte| matches!(byte, b'-' | b'+' | b'1'..=b'9'))
}

/// `text` up to a comment: a `#` at its start or after a space, outside quotes.
fn without_comment(text: &[u8]) -> &[u8] {
    let mut quote = None;
    let mut at = 0;
    while let Some(&byte) = text.get(at) {
        match (quote, byte) {
            (Some(b'"'), b'\\') => at += 1,
            (Some(open), _) if byte == open => quote = None,
            (None, b'"' | b'\'') => quote = Some(byte),
            (None, b'#') if at == 0 || matches!(text[at - 1], b' ' | b'\t') => {
                return &text[..at];
            }
            _ => {}
        }
        at += 1;
    }
    text
}

/// The columns `TAB` cycles line `row` through: the deepest it could go after the line
/// before, then each level of `indent_size` out, then 0. Past the budget, only 0.
pub(super) fn stops(
    rope: &Rope,
    settings: Settings,
    row: usize,
    budget: &mut Budget,
) -> Vec<usize> {
    let target = rope.line_start(row);
    let reached = pass::pass_at::<Yaml>(rope, settings, target, None, false, budget);
    let deepest = reached
        .and_then(|reached| reached.lexer.previous)
        .map_or(0, |previous| previous.next.max(previous.content))
        .min(pass::MAX_COLUMN);
    let size = settings.indent_size().max(1);
    let mut stops: Vec<usize> = (0..=deepest / size)
        .rev()
        .map(|level| level * size)
        .collect();
    if !deepest.is_multiple_of(size) {
        stops.insert(0, deepest);
    }
    stops
}

#[cfg(test)]
mod tests {
    use super::{markers, opens, without_comment};

    #[test]
    fn a_line_opens_a_block_after_a_colon_a_block_scalar_or_a_bare_marker() {
        for open in [
            &b"key:"[..],
            b"key: # note",
            b"run: |",
            b"text: >-",
            b"s: |2+",
            b"",
        ] {
            assert!(opens(open), "{:?}", String::from_utf8_lossy(open));
        }
        for closed in [
            &b"key: value"[..],
            b"url: http://a",
            b"k: 'a:'",
            b"a: \"#:\"",
            b"x: |a",
        ] {
            assert!(!opens(closed), "{:?}", String::from_utf8_lossy(closed));
        }
    }

    #[test]
    fn markers_are_counted_to_the_content() {
        assert_eq!(markers(b"- a: 1"), (2, &b"a: 1"[..]));
        assert_eq!(markers(b"- - b"), (4, &b"b"[..]));
        assert_eq!(markers(b"-"), (2, &b""[..]));
        assert_eq!(markers(b"- -"), (4, &b""[..]));
        assert_eq!(markers(b"-1"), (0, &b"-1"[..]));
        assert_eq!(without_comment(b"a: b # c"), b"a: b ");
        assert_eq!(without_comment(b"a: 'b # c'"), b"a: 'b # c'");
    }
}
