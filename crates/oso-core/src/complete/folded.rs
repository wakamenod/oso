//! A candidate's text changed for matching (ADR 0076, ADR 0116): in lower case, for an input
//! that ignores case, or with its kana and combining sound marks composed, for migemo, and
//! where each byte of it came from, so a match maps back to the original.
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use std::ops::Range;

use super::offset_u32;
use crate::migemo;

/// A string in lower case, for matching that ignores case, or with its kana composed, for
/// migemo, and where its bytes came from.
#[derive(Clone)]
pub(super) struct Folded {
    /// The folded string, or `None` where folding changes nothing.
    pub(super) text: Option<Box<str>>,
    /// For each byte of `text` and its end, the offset in the original of the character it
    /// came from: `None` where every character folds to one of the same length, so that
    /// offsets agree.
    pub(super) from: Option<Box<[u32]>>,
}

impl Folded {
    pub(super) fn of(text: &str) -> Self {
        if !text.chars().any(changes_when_folded) {
            return Self {
                text: None,
                from: None,
            };
        }
        let mut folded = String::with_capacity(text.len());
        let mut from = Vec::with_capacity(text.len() + 1);
        let mut same = true;
        for (offset, c) in text.char_indices() {
            let start = folded.len();
            folded.extend(c.to_lowercase());
            same &= folded.len() - start == c.len_utf8();
            from.resize(folded.len(), offset_u32(offset));
        }
        from.push(offset_u32(text.len()));
        Self {
            text: Some(folded.into()),
            from: (!same).then(|| from.into()),
        }
    }

    /// `text` with each kana and the combining sound mark after it composed (`か` and U+3099
    /// are `が`), `None` where it holds no such pair.
    pub(super) fn composed(text: &str) -> Option<Self> {
        // Most names are ASCII, which a vectorised look tells at once.
        if text.is_ascii() || !text.contains(migemo::SOUND_MARKS) {
            return None;
        }
        let mut composed = String::with_capacity(text.len());
        let mut from = Vec::with_capacity(text.len() + 1);
        let mut chars = text.char_indices().peekable();
        let mut changed = false;
        while let Some((offset, c)) = chars.next() {
            let mark = chars.peek().map(|&(_, mark)| mark);
            let one = mark.and_then(|mark| migemo::composed(c, mark));
            match one {
                Some(one) => {
                    composed.push(one);
                    chars.next();
                    changed = true;
                }
                None => composed.push(c),
            }
            from.resize(composed.len(), offset_u32(offset));
        }
        from.push(offset_u32(text.len()));
        changed.then(|| Self {
            text: Some(composed.into()),
            from: Some(from.into()),
        })
    }

    /// The range of the original that `range` of the folded string covers, widened to whole
    /// characters.
    pub(super) fn original(&self, range: Range<usize>) -> Range<usize> {
        let Some(from) = &self.from else {
            return range;
        };
        // Read with `get`: matching runs after a command's panic boundary, so a range it was
        // never given must give an empty range, not a panic.
        let start = from.get(range.start).map_or(0, |&offset| offset as usize);
        // The end is where the next original character starts, past the one `range` ends in.
        let last = from
            .get(range.end.saturating_sub(1).max(range.start))
            .copied()
            .unwrap_or(0);
        let end = from
            .get(range.end..)
            .and_then(|rest| rest.iter().find(|&&offset| offset > last))
            .map_or(start, |&offset| offset as usize);
        start..end.max(start)
    }
}

/// Whether ignoring case reads `c` as something else.
fn changes_when_folded(c: char) -> bool {
    let mut lower = c.to_lowercase();
    lower.next() != Some(c) || lower.next().is_some()
}
