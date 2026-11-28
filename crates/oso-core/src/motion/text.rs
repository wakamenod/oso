//! Reading a snapshot's text a character at a time, forward or back, within a command's budget,
//! for motion by paragraphs, sentences and pages (ADR 0190).
//!
//! Must not depend on: `buffer`, `window` or `editor`; it reads a display snapshot only.
//!
//! - Each character read costs its bytes; a reader that runs out stops, and the budget says so.
//! - Making a reader is a seek ([`SEEK_COST`]); the first of a cursor's motion step is an
//!   entry seek, which a command at several cursors has apart from its bytes (ADR 0101).

use super::{Edge, SEEK_COST, WalkBudget};
use crate::display_map::DisplaySnapshot;
use crate::selection::Selection;

/// Moves `selection` `n` steps, back if `n` is negative: `step` takes an offset and whether
/// it goes forward, and gives where the step goes, or `None` if the budget ran out, which
/// ends the count there. A step at the buffer's edge fails with it.
pub(super) fn steps(
    display: &DisplaySnapshot,
    selection: &mut Selection,
    n: isize,
    mut step: impl FnMut(usize, bool) -> Option<usize>,
) -> Result<(), Edge> {
    let len = display.len();
    let mut at = selection.cursor;
    let mut result = Ok(());
    for _ in 0..n.unsigned_abs() {
        if n > 0 && at >= len {
            result = Err(Edge::End);
            break;
        }
        if n < 0 && at == 0 {
            result = Err(Edge::Start);
            break;
        }
        match step(at, n > 0) {
            Some(to) => at = to,
            None => break,
        }
    }
    selection.move_to(at);
    result
}

/// What a line is to paragraphs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Line {
    /// Only spaces, tabs and form feeds, or nothing: it separates paragraphs.
    Blank,
    /// A Markdown heading or fence line, a paragraph of its own.
    Alone,
    /// It starts with a form feed, and starts a paragraph.
    Start,
    Text,
}

/// The first characters of a line that tell what it is: a Markdown heading's indentation, its
/// marks and the space after them.
const PREFIX: usize = 10;

/// What a line read so far is like, forward or back.
struct Shape {
    /// Forward, its first characters; back, the last [`PREFIX`] read, in a ring.
    chars: [char; PREFIX],
    count: usize,
    blank: bool,
    backward: bool,
}

impl Shape {
    fn new(backward: bool) -> Self {
        Self {
            chars: ['\0'; PREFIX],
            count: 0,
            blank: true,
            backward,
        }
    }

    fn push(&mut self, c: char) {
        if self.backward {
            self.chars[self.count % PREFIX] = c;
        } else if self.count < PREFIX {
            self.chars[self.count] = c;
        }
        self.count += 1;
        // A line break of two characters leaves its `\r` on the line.
        self.blank &= matches!(c, ' ' | '\t' | '\u{c}' | '\r');
    }

    fn kind(&self, markdown: bool) -> Line {
        if self.blank {
            return Line::Blank;
        }
        let shown = self.count.min(PREFIX);
        let mut prefix = ['\0'; PREFIX];
        for (i, c) in prefix.iter_mut().take(shown).enumerate() {
            *c = if self.backward {
                self.chars[(self.count - 1 - i) % PREFIX]
            } else {
                self.chars[i]
            };
        }
        let prefix = &prefix[..shown];
        if prefix.first() == Some(&'\u{c}') {
            Line::Start
        } else if markdown && stands_apart(prefix, self.count <= PREFIX) {
            Line::Alone
        } else {
            Line::Text
        }
    }
}

/// Whether a line that starts with `prefix`, all of it if `whole`, is a Markdown heading or a
/// fence line: up to three spaces, then one to six `#` and a space, a tab or its end, or three
/// backquotes or tildes.
fn stands_apart(prefix: &[char], whole: bool) -> bool {
    let indent = prefix.iter().take_while(|&&c| c == ' ').count();
    if indent > 3 {
        return false;
    }
    let rest = &prefix[indent..];
    let fence = |mark: char| rest.len() >= 3 && rest[..3].iter().all(|&c| c == mark);
    if fence('`') || fence('~') {
        return true;
    }
    let marks = rest.iter().take_while(|&&c| c == '#').count();
    if !(1..=6).contains(&marks) {
        return false;
    }
    match rest.get(marks) {
        None => whole,
        Some(c) => matches!(c, ' ' | '\t' | '\r'),
    }
}

/// The lines from a line's start on, each with its start and what it is, while the budget
/// pays for them.
pub(super) struct Lines<I> {
    chars: I,
    start: Option<usize>,
    len: usize,
    markdown: bool,
    spent: bool,
}

/// The lines from `start`, a line's start, on.
pub(super) fn lines_after<'a>(
    display: &'a DisplaySnapshot,
    start: usize,
    markdown: bool,
    budget: &'a mut WalkBudget,
) -> Lines<impl Iterator<Item = (usize, char)> + 'a> {
    Lines {
        chars: after(display, start, budget),
        start: Some(start),
        len: display.len(),
        markdown,
        spent: false,
    }
}

impl<I: Iterator<Item = (usize, char)>> Lines<I> {
    /// The next line, or `None` past the last or once the budget ran out.
    pub(super) fn next_line(&mut self) -> Option<(usize, Line)> {
        let start = self.start?;
        let mut shape = Shape::new(false);
        let mut end = start;
        for (at, c) in self.chars.by_ref() {
            if c == '\n' {
                self.start = Some(at + 1);
                return Some((start, shape.kind(self.markdown)));
            }
            end = at + c.len_utf8();
            shape.push(c);
        }
        self.start = None;
        self.spent = end < self.len;
        (!self.spent).then(|| (start, shape.kind(self.markdown)))
    }

    /// The buffer's end, once the lines are done, or `None` if the budget ran out.
    pub(super) fn at_end(&self) -> Option<usize> {
        (!self.spent).then_some(self.len)
    }
}

/// The lines before an offset, the nearest first, each with its start and what it is, while
/// the budget pays for them. The first is the line the offset is on, up to it.
pub(super) struct Behind<I> {
    chars: I,
    end: Option<usize>,
    markdown: bool,
    spent: bool,
}

/// The lines before `end`, its own first.
pub(super) fn lines_before<'a>(
    display: &'a DisplaySnapshot,
    end: usize,
    markdown: bool,
    budget: &'a mut WalkBudget,
) -> Behind<impl Iterator<Item = (usize, char)> + 'a> {
    Behind {
        chars: before(display, end, budget),
        end: Some(end),
        markdown,
        spent: false,
    }
}

impl<I: Iterator<Item = (usize, char)>> Behind<I> {
    /// The next line back, or `None` before the first or once the budget ran out.
    pub(super) fn next_line(&mut self) -> Option<(usize, Line)> {
        let end = self.end?;
        let mut shape = Shape::new(true);
        let mut start = end;
        for (at, c) in self.chars.by_ref() {
            if c == '\n' {
                self.end = Some(at);
                return Some((start, shape.kind(self.markdown)));
            }
            start = at;
            shape.push(c);
        }
        self.end = None;
        self.spent = start > 0;
        (!self.spent).then(|| (0, shape.kind(self.markdown)))
    }

    /// The line the reader starts on, or `None` if the budget ran out first.
    pub(super) fn into_first(mut self) -> Option<(usize, Line)> {
        self.next_line()
    }

    /// The buffer's start, once the lines are done, or `None` if the budget ran out.
    pub(super) fn at_start(&self) -> Option<usize> {
        (!self.spent).then_some(0)
    }
}

/// The characters from `from` on, each with its offset, while `budget` pays for them.
pub(super) fn after<'a>(
    display: &'a DisplaySnapshot,
    from: usize,
    budget: &'a mut WalkBudget,
) -> impl Iterator<Item = (usize, char)> + 'a {
    let mut at = from;
    let chars = display
        .text_in(from..display.len().max(from))
        .flat_map(move |piece| {
            let base = at;
            at += piece.len();
            piece.char_indices().map(move |(i, c)| (base + i, c))
        });
    charged(chars, budget)
}

/// The characters before `to`, the nearest first, each with its offset, while `budget` pays
/// for them.
pub(super) fn before<'a>(
    display: &'a DisplaySnapshot,
    to: usize,
    budget: &'a mut WalkBudget,
) -> impl Iterator<Item = (usize, char)> + 'a {
    let mut at = to;
    let chars = display.text_before(to).flat_map(move |piece| {
        at -= piece.len();
        let base = at;
        piece.char_indices().rev().map(move |(i, c)| (base + i, c))
    });
    charged(chars, budget)
}

/// `chars`, each paid for from `budget`, until it holds too little for one.
fn charged<'a>(
    chars: impl Iterator<Item = (usize, char)> + 'a,
    budget: &'a mut WalkBudget,
) -> impl Iterator<Item = (usize, char)> + 'a {
    chars.map_while(move |(at, c)| budget.spend(c.len_utf8()).then_some((at, c)))
}

/// Takes the seek a motion step starts with: an entry seek while the command has some, else
/// [`SEEK_COST`] bytes. False if the budget holds too little.
pub(super) fn enter(budget: &mut WalkBudget) -> bool {
    budget.stop().enter()
}

/// Takes a seek to another place in the text. False if the budget holds too little.
pub(super) fn seek(budget: &mut WalkBudget) -> bool {
    budget.spend(SEEK_COST)
}

/// The end of the display cluster `offset` is inside, so that a stop found in the text is one a
/// cursor can take; `offset` itself where a cluster starts.
pub(super) fn cluster_boundary(display: &DisplaySnapshot, offset: usize) -> usize {
    display.cluster_at(offset).1
}

/// What the tests of paragraphs, sentences and pages read text with.
#[cfg(test)]
pub(super) mod testing {
    use crate::display_map::{DisplayMap, DisplaySnapshot, TabWidth};
    use crate::rope::Rope;

    pub(in crate::motion) fn display(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    /// `text` without its `|`s, and the offsets where they were, in order.
    pub(in crate::motion) fn marked(text: &str) -> (String, Vec<usize>) {
        let mut plain = String::new();
        let mut marks = Vec::new();
        for c in text.chars() {
            if c == '|' {
                marks.push(plain.len());
            } else {
                plain.push(c);
            }
        }
        (plain, marks)
    }
}
