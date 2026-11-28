//! A buffer's lines read one at a time, forward or backward, for finding defuns' heads (ADR
//! 0190): where each starts and ends, its first bytes, and whether it is blank.
//!
//! Must not depend on: `window` or `word`; it reads text over a display snapshot.
//!
//! A line costs what it holds, read once, and at least [`LINE_COST`], as an empty line takes
//! longer to read than its one byte. It keeps at most [`HEAD_MAX`] of its bytes, so a long one
//! costs no memory.

use super::super::heads::HEAD_MAX;
use super::super::{ScanError, charge};
use crate::display_map::DisplaySnapshot;
use crate::motion::WalkBudget;

/// What a line costs at least.
const LINE_COST: usize = 2;

/// A line read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LineAt {
    pub(super) start: usize,
    /// Past its line break, or the buffer's end.
    pub(super) end: usize,
    /// Its last byte that is not a blank.
    pub(super) last: Option<u8>,
    /// Whether it holds only blanks.
    pub(super) blank: bool,
}

/// What a line may hold and still be blank: Emacs's paragraph separators.
pub(super) fn is_blank(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\r' | b'\x0c')
}

/// Takes a `\r` before the line break off `text`, if `text` holds the whole line.
fn trim_cr(text: &mut Vec<u8>, whole: bool) {
    if whole && text.last() == Some(&b'\r') {
        text.pop();
    }
}

/// Lines going forward.
pub(super) struct LinesAhead<'a> {
    pieces: Box<dyn Iterator<Item = &'a str> + 'a>,
    piece: &'a [u8],
    at: usize,
    text: Vec<u8>,
}

impl<'a> LinesAhead<'a> {
    /// From `from`, a line's start.
    pub(super) fn new(display: &'a DisplaySnapshot, from: usize) -> Self {
        Self {
            pieces: Box::new(display.text_in(from..display.len())),
            piece: &[],
            at: from,
            text: Vec::new(),
        }
    }

    /// The first bytes of the line read last, at most [`HEAD_MAX`], with no line break.
    pub(super) fn text(&self) -> &[u8] {
        &self.text
    }

    pub(super) fn next(&mut self, budget: &mut WalkBudget) -> Result<Option<LineAt>, ScanError> {
        self.text.clear();
        let start = self.at;
        let (mut last, mut blank, mut body) = (None, true, 0);
        loop {
            if self.piece.is_empty() {
                match self.pieces.next() {
                    Some(piece) => {
                        self.piece = piece.as_bytes();
                        continue;
                    }
                    None => break,
                }
            }
            let (take, ended) = match self.piece.iter().position(|&b| b == b'\n') {
                Some(at) => (at + 1, true),
                None => (self.piece.len(), false),
            };
            charge(budget, take)?;
            let seg = &self.piece[..take - usize::from(ended)];
            let room = HEAD_MAX.saturating_sub(self.text.len());
            self.text.extend_from_slice(&seg[..seg.len().min(room)]);
            if let Some(&b) = seg.iter().rev().find(|&&b| !is_blank(b)) {
                (last, blank) = (Some(b), false);
            }
            body += seg.len();
            self.at += take;
            self.piece = &self.piece[take..];
            if ended {
                break;
            }
        }
        if self.at == start {
            return Ok(None);
        }
        charge(budget, LINE_COST.saturating_sub(self.at - start))?;
        trim_cr(&mut self.text, body <= HEAD_MAX);
        Ok(Some(LineAt {
            start,
            end: self.at,
            last,
            blank,
        }))
    }
}

/// Lines going backward.
pub(super) struct LinesBehind<'a> {
    pieces: Box<dyn Iterator<Item = &'a str> + 'a>,
    piece: &'a [u8],
    at: usize,
    text: Vec<u8>,
}

impl<'a> LinesBehind<'a> {
    /// From `to`, a line's start or the buffer's end, back.
    pub(super) fn new(display: &'a DisplaySnapshot, to: usize) -> Self {
        Self {
            pieces: Box::new(display.text_before(to)),
            piece: &[],
            at: to,
            text: Vec::new(),
        }
    }

    /// The first bytes of the line read last, at most [`HEAD_MAX`], with no line break.
    pub(super) fn text(&self) -> &[u8] {
        &self.text
    }

    pub(super) fn next(&mut self, budget: &mut WalkBudget) -> Result<Option<LineAt>, ScanError> {
        self.text.clear();
        let end = self.at;
        if end == 0 {
            return Ok(None);
        }
        let (mut last, mut first, mut ended) = (None, true, false);
        loop {
            if self.piece.is_empty() {
                match self.pieces.next() {
                    Some(piece) => {
                        self.piece = piece.as_bytes();
                        continue;
                    }
                    None => break,
                }
            }
            if first {
                first = false;
                if let Some((&b'\n', rest)) = self.piece.split_last() {
                    charge(budget, 1)?;
                    (self.piece, self.at, ended) = (rest, self.at - 1, true);
                    continue;
                }
            }
            let cut = self
                .piece
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |at| at + 1);
            let seg = &self.piece[cut..];
            charge(budget, seg.len())?;
            if last.is_none() {
                last = seg.iter().rev().copied().find(|&b| !is_blank(b));
            }
            // Keep the line's first bytes: these, then what is left of those after them.
            let keep = HEAD_MAX.saturating_sub(seg.len()).min(self.text.len());
            self.text.truncate(keep);
            self.text
                .splice(0..0, seg[..seg.len().min(HEAD_MAX)].iter().copied());
            self.at -= seg.len();
            self.piece = &self.piece[..cut];
            if cut > 0 {
                break;
            }
        }
        charge(budget, LINE_COST.saturating_sub(end - self.at))?;
        let body = end - self.at - usize::from(ended);
        trim_cr(&mut self.text, body <= HEAD_MAX);
        Ok(Some(LineAt {
            start: self.at,
            end,
            last,
            blank: last.is_none(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::rope::Rope;

    fn display(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    #[test]
    fn lines_read_the_same_both_ways() {
        let long = "y".repeat(HEAD_MAX + 9);
        let text = format!("a b\r\n\n  \t\n{long}\nlast  ");
        let display = display(&text);
        let mut budget = WalkBudget::new();
        let mut ahead = LinesAhead::new(&display, 0);
        let mut forward = Vec::new();
        while let Some(line) = ahead.next(&mut budget).unwrap() {
            forward.push((line, ahead.text().to_vec()));
        }
        let mut behind = LinesBehind::new(&display, text.len());
        let mut backward = Vec::new();
        while let Some(line) = behind.next(&mut budget).unwrap() {
            backward.push((line, behind.text().to_vec()));
        }
        backward.reverse();
        assert_eq!(forward, backward);
        let starts: Vec<_> = forward.iter().map(|(line, _)| line.start).collect();
        assert_eq!(starts, [0, 5, 6, 10, 11 + long.len()]);
        assert_eq!(forward[0].1, b"a b");
        assert_eq!(forward[0].0.last, Some(b'b'));
        assert!(forward[1].0.blank && forward[2].0.blank && !forward[3].0.blank);
        assert_eq!(forward[3].1.len(), HEAD_MAX);
        assert_eq!(forward[4].0.end, text.len());
        // The empty line costs one more each way.
        assert_eq!(
            budget.left(),
            WalkBudget::new().left() - 2 * (text.len() + 1)
        );
    }
}
