//! Markdown's indentation rules (ADR 0115), from lines alone: list items (`-`, `*`, `+`, or
//! a number with `.` or `)`, then a space) and fenced code blocks.
//!
//! Must not depend on: `buffer`, or the tree highlighting parses; it reads lines alone.
//!
//! A new line goes to the content column of the item the line it leaves starts, as
//! markdown-mode's `markdown-indent-on-enter` does, and copies the indentation of that line
//! elsewhere and in a fenced code block. `TAB` cycles through the content columns of the
//! items a line could belong to, innermost first, then 0. Only the buffer's start is an
//! anchor: a fence can hold any line, so none tells it is outside one.

use super::pass::{self, Head, Lexer, MAX_COLUMN, measure};
use super::{Answer, Budget, Settings};
use crate::rope::Rope;

/// The most items open at once the rules keep: deeper ones are not offered.
const MAX_ITEMS: usize = 32;

/// The most spaces between a list marker and its text: with more, the text is indented
/// code inside the item, and its content column is one past the marker (CommonMark).
const MAX_GAP: usize = 4;

/// The line before: its indentation, and the content column of the item it starts.
#[derive(Clone, Copy, Debug)]
struct Before {
    indent: usize,
    item: Option<usize>,
    fenced: bool,
}

pub(super) struct Markdown {
    settings: Settings,
    /// The fence character and its length, while inside a fenced code block.
    fence: Option<(u8, usize)>,
    /// The items open, outermost first: each one's indentation and content column.
    items: Vec<(usize, usize)>,
    before: Option<Before>,
    blank_before: bool,
}

impl Lexer for Markdown {
    fn fresh(settings: Settings) -> Self {
        Self {
            settings,
            fence: None,
            items: Vec::new(),
            before: None,
            blank_before: false,
        }
    }

    fn reset(&mut self, settings: Settings) {
        let mut items = std::mem::take(&mut self.items);
        items.clear();
        *self = Self::fresh(settings);
        self.items = items;
    }

    const HAS_ANCHORS: bool = false;

    fn is_anchor(_line: &[u8]) -> bool {
        false
    }

    fn is_inside(&self) -> bool {
        self.fence.is_some()
    }

    fn line(&mut self, text: &str, indent: usize) {
        let indentation = measure(text, self.settings.tab_width());
        let rest = text[indentation.bytes..].trim_end_matches('\r').as_bytes();
        let fenced = match self.fence {
            Some((byte, length)) => {
                if fence(rest)
                    .is_some_and(|(b, l, info)| b == byte && l >= length && info.is_empty())
                {
                    self.fence = None;
                }
                true
            }
            None => match fence(rest) {
                Some((byte, length, info)) if byte == b'~' || !info.contains(&b'`') => {
                    self.fence = Some((byte, length));
                    true
                }
                _ => false,
            },
        };
        let item = if fenced { None } else { item_content(rest) };
        if let Some(gap) = item {
            let content = indent.saturating_add(gap);
            self.items.retain(|&(open, _)| open < indent);
            if self.items.len() < MAX_ITEMS {
                self.items.push((indent, content));
            }
        } else if !fenced
            && !indentation.blank
            && indent == 0
            && (self.blank_before || rest.starts_with(b"#"))
        {
            // A paragraph after a blank line, or a heading, at the margin ends every list.
            self.items.clear();
        }
        self.before = Some(Before {
            indent,
            item: item.map(|gap| indent.saturating_add(gap)),
            fenced,
        });
        self.blank_before = indentation.blank;
    }

    fn answer(&self, _head: &Head, new: bool) -> Option<Answer> {
        if !new {
            return Some(Answer::Keep);
        }
        let column = match self.before {
            None => 0,
            Some(before) if before.fenced => before.indent,
            Some(before) => before.item.unwrap_or(before.indent),
        };
        Some(Answer::Column(column))
    }
}

/// The fence `rest`, a line past its indentation, opens or closes: its character, its
/// length and its info string.
fn fence(rest: &[u8]) -> Option<(u8, usize, &[u8])> {
    let byte = *rest.first().filter(|&&b| b == b'`' || b == b'~')?;
    let length = rest.iter().take_while(|&&b| b == byte).count();
    let info = rest[length..].trim_ascii();
    (length >= 3).then_some((byte, length, info))
}

/// The columns from where `rest`, a line past its indentation, starts to where its text
/// starts, if it starts a list item.
fn item_content(rest: &[u8]) -> Option<usize> {
    let marker = match rest.first()? {
        b'-' | b'*' | b'+' => 1,
        b'0'..=b'9' => {
            let digits = rest
                .iter()
                .take(10)
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits > 9 || !matches!(rest.get(digits), Some(b'.' | b')')) {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    let after = &rest[marker..];
    let gap = after.iter().take_while(|&&b| b == b' ').count();
    if gap == 0 && !after.is_empty() {
        return None;
    }
    let gap = if gap == 0 || gap > MAX_GAP || gap == after.len() {
        1
    } else {
        gap
    };
    Some(marker + gap)
}

/// See [`super::markdown_stops`].
pub(super) fn stops(
    rope: &Rope,
    settings: Settings,
    row: usize,
    budget: &mut Budget,
) -> Vec<usize> {
    let target = rope.line_start(row);
    let mut stops: Vec<usize> =
        match pass::pass_at::<Markdown>(rope, settings, target, None, false, budget) {
            Some(pass) if pass.lexer.fence.is_none() => {
                let items = pass.lexer.items.iter().rev();
                let contents = items.map(|&(_, content)| content);
                contents.filter(|&content| content <= MAX_COLUMN).collect()
            }
            _ => Vec::new(),
        };
    stops.push(0);
    stops.dedup();
    stops
}

/// Whether each of `offsets`, in order, is inside a fenced code block, read in one pass. Past
/// the budget, or where the pass cannot reach, false.
pub(super) fn fenced(
    rope: &Rope,
    settings: Settings,
    offsets: &[usize],
    budget: &mut Budget,
) -> Vec<bool> {
    let mut pass = None;
    offsets
        .iter()
        .map(|&at| {
            match pass::pass_at::<Markdown>(rope, settings, at, pass.take(), false, budget) {
                Some(reached) => {
                    let fenced = reached.lexer.fence.is_some();
                    pass = Some(reached);
                    fenced
                }
                None => false,
            }
        })
        .collect()
}
