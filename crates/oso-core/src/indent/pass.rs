//! The pass the rules of every mode share (ADR 0115): from an anchor, or from where the line
//! asked about before left off, it reads the text forward a line at a time, within the
//! budget, hands each line to the mode's lexer, and asks it about the next line.
//!
//! Must not depend on: `buffer`, `motion` or `search`; it reads a rope.
//!
//! The lines of one command share the pass, in order: each starts from the later of its own
//! anchor and the line before, carrying the state, and starts afresh at every anchor it
//! passes outside a string or comment. A line near the one before reads on to it without
//! looking back; one farther looks back for its anchor no farther than the line before, and
//! no farther than [`LINE_LOOKBACK`].

use super::{Answer, Budget, LINE_COST, LINE_LOOKBACK, Line, Settings};
use crate::display_map::{CellKind, Clusters, TabWidth};
use crate::rope::{Chunks, Rope};
use crate::sum_tree::Bias;

/// What a mode's rules keep while reading forward, and what they answer. Private to this
/// module's rules: the editor chooses them with a match on the buffer's mode.
pub(super) trait Lexer: Sized {
    /// The state at an anchor, or at the buffer's start.
    fn fresh(settings: Settings) -> Self;

    /// Starts afresh, as at an anchor, keeping what it has allocated.
    fn reset(&mut self, settings: Settings) {
        *self = Self::fresh(settings);
    }

    /// Whether any line is an anchor; with none, only the buffer's start is, and no line
    /// looks back for one.
    const HAS_ANCHORS: bool = true;

    /// Whether a line whose text starts with `line` (at most [`HEAD_BYTES`] of it) is an
    /// anchor.
    fn is_anchor(line: &[u8]) -> bool;

    /// Whether the pass is inside a string or a comment, where a line that looks like an
    /// anchor proves nothing.
    fn is_inside(&self) -> bool;

    /// Reads one line, or the part of one before a line asked about, without its line
    /// break. `indent` is the line's indentation as the command leaves it.
    fn line(&mut self, text: &str, indent: usize);

    /// The answer for a line that starts here, which starts with `head`, or none if the
    /// rules cannot tell. `new` says whether it is a new line.
    fn answer(&self, head: &Head, new: bool) -> Option<Answer>;
}

/// The deepest brackets a lexer keeps. Past it, it counts them, and lines inside them get
/// the fallback answer.
pub(super) const MAX_DEPTH: usize = 256;

/// The most bytes [`Head`] holds of a line's text, and that an anchor is told by.
pub(super) const HEAD_BYTES: usize = 16;

/// How far a line may be from the one before for the pass to read on to it without looking
/// back for an anchor in between.
const NEAR: usize = 1024;

/// The most bytes read back at a time when looking for an anchor, and the first.
const WINDOW: usize = 4096;
const FIRST_WINDOW: usize = 256;

/// What looking for the line a fallback copies may read back: a little first, then more.
const FALLBACK_BACK: [usize; 2] = [128, 1024];

/// The most spaces and tabs read at the start of a line asked about: past them, the rules
/// cannot tell.
const MAX_LEAD: usize = 4096;

/// The deepest column answered: a deeper line keeps its indentation, so that no command
/// inserts more than this for one line.
pub(crate) const MAX_COLUMN: usize = 4096;

/// The start of a line asked about: its first bytes past its spaces and tabs.
pub(super) struct Head {
    bytes: [u8; HEAD_BYTES],
    len: usize,
}

impl Head {
    /// The start of a line with nothing on it.
    pub(super) fn blank() -> Self {
        Self {
            bytes: [0; HEAD_BYTES],
            len: 0,
        }
    }

    /// Up to [`HEAD_BYTES`] of the line past its spaces and tabs, with no `\r`: empty if it
    /// is blank.
    pub(super) fn text(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// The first byte past the line's spaces and tabs.
    pub(super) fn first(&self) -> Option<u8> {
        self.text().first().copied()
    }
}

/// Whether `text` starts with the word `word`, not with a longer one.
pub(super) fn starts_with_word(text: &[u8], word: &[u8]) -> bool {
    text.starts_with(word) && text.get(word.len()).is_none_or(|&byte| !is_word_byte(byte))
}

/// Whether `byte` can be part of an identifier: ASCII letters, digits, `_`, and every
/// byte of a non-ASCII character.
pub(super) fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

/// A line's indentation as it stands: spaces and tabs only, never U+3000 (ADR 0115).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Indentation {
    /// Its length in bytes.
    pub(super) bytes: usize,
    /// The display column it reaches.
    pub(super) column: usize,
    /// Whether the line holds nothing else.
    pub(super) blank: bool,
}

/// The indentation `pieces`, a line's text in order, start with: its length in bytes and the
/// column it reaches.
pub(super) fn lead<'a>(
    pieces: impl IntoIterator<Item = &'a str>,
    tab_width: TabWidth,
) -> (usize, usize) {
    let mut column = 0usize;
    let mut bytes = 0;
    for piece in pieces {
        for byte in piece.bytes() {
            match byte {
                b' ' => column = column.saturating_add(1),
                b'\t' => column = tab_width.next_stop(column),
                _ => return (bytes, column),
            }
            bytes += 1;
        }
    }
    (bytes, column)
}

/// The indentation `text`, a line without its line break, starts with.
pub(super) fn measure(text: &str, tab_width: TabWidth) -> Indentation {
    let (bytes, column) = lead([text], tab_width);
    let blank = text.as_bytes()[bytes..].iter().all(|&byte| byte == b'\r');
    Indentation {
        bytes,
        column,
        blank,
    }
}

/// The display column `text` ends at, starting at `column`.
pub(super) fn width(text: &str, column: usize, tab_width: TabWidth) -> usize {
    if text.is_ascii() && !text.contains('\t') {
        return column.saturating_add(text.len());
    }
    Clusters::new(text).fold(column, |column, (_, _, kind, cells)| match kind {
        CellKind::Tab => tab_width.next_stop(column),
        _ => column.saturating_add(cells),
    })
}

/// The budget ran out.
struct Spent;

/// Reads a rope forward a line at a time. A line within one chunk is borrowed; a longer one
/// is copied, within the budget.
struct Reader<'a> {
    rope: &'a Rope,
    chunks: Chunks<'a>,
    rest: &'a str,
    at: usize,
    /// Whether `at` starts a line of the text, not only a line asked about.
    line_start: bool,
    /// Where the bytes charged to the budget end, so that none is charged twice.
    charged: usize,
    copy: String,
}

impl<'a> Reader<'a> {
    fn new(rope: &'a Rope, at: usize) -> Self {
        Self {
            rope,
            chunks: rope.chunks_in_range(at..rope.len()),
            rest: "",
            at,
            line_start: true,
            charged: at,
            copy: String::new(),
        }
    }

    /// The text from here to the next line break, which it passes, or to `stop`, where a
    /// line asked about starts. None at `stop` or at the end of the text.
    fn next(&mut self, stop: usize, budget: &mut Budget) -> Option<Result<&str, Spent>> {
        if self.at >= stop {
            return None;
        }
        self.copy.clear();
        let mut copied = false;
        let start = self.at;
        loop {
            if self.rest.is_empty() {
                match self.chunks.next() {
                    Some(chunk) => self.rest = chunk,
                    None if copied => return Some(Ok(&self.copy)),
                    None => return None,
                }
            }
            let rest: &'a str = self.rest;
            let ahead = (stop - self.at).min(rest.len());
            let (end, passed) = match rest[..ahead].find('\n') {
                Some(newline) => (newline, 1),
                None => (ahead, 0),
            };
            if !self.charge(self.at + end + passed, budget) {
                return Some(Err(Spent));
            }
            self.rest = &rest[end + passed..];
            self.at += end + passed;
            if passed == 0 && self.at < stop {
                self.copy.push_str(&rest[..end]);
                copied = true;
                continue;
            }
            self.line_start = passed == 1;
            if !budget.spend(LINE_COST.saturating_sub(self.at - start)) {
                return Some(Err(Spent));
            }
            if copied {
                self.copy.push_str(&rest[..end]);
                return Some(Ok(&self.copy));
            }
            return Some(Ok(&rest[..end]));
        }
    }

    /// Charges the bytes up to `to` not charged yet.
    fn charge(&mut self, to: usize, budget: &mut Budget) -> bool {
        let bytes = to.saturating_sub(self.charged);
        self.charged = self.charged.max(to);
        budget.spend(bytes)
    }

    /// Where the line after the one here starts, if the chunk at hand tells.
    fn next_line_start(&self) -> Option<usize> {
        self.rest.find('\n').map(|newline| self.at + newline + 1)
    }

    /// The start of the line here, read ahead without passing it, or none past the budget
    /// or [`MAX_LEAD`].
    fn head(&mut self, budget: &mut Budget) -> Option<Head> {
        let mut head = Head {
            bytes: [0; HEAD_BYTES],
            len: 0,
        };
        // Past the chunk at hand, from the rope: rare, so a seek is fine.
        let after = self.at + self.rest.len();
        let more = std::iter::once_with(|| self.rope.chunks_in_range(after..self.rope.len()));
        let pieces = std::iter::once(self.rest).chain(more.flatten());
        let mut read = 0;
        'pieces: for piece in pieces {
            for &byte in piece.as_bytes() {
                read += 1;
                match byte {
                    b'\n' => break 'pieces,
                    b'\r' => {}
                    b' ' | b'\t' if head.len == 0 && read > MAX_LEAD => return None,
                    b' ' | b'\t' if head.len == 0 => {}
                    _ => {
                        head.bytes[head.len] = byte;
                        head.len += 1;
                        if head.len == HEAD_BYTES {
                            break 'pieces;
                        }
                    }
                }
            }
        }
        self.charge(self.at + read, budget).then_some(head)
    }
}

/// Where a pass stands: at the start of a line, in the state the lines before it leave.
pub(super) struct Pass<'a, L> {
    reader: Reader<'a>,
    pub(super) lexer: L,
    /// The indentation the command gives the line here, if it answered one.
    indent: Option<usize>,
}

/// Where `line` starts in the text.
fn start_of(rope: &Rope, line: Line) -> usize {
    match line {
        Line::Row(row) => rope.line_start(row),
        Line::New(offset) | Line::Empty(offset) => rope.clip_offset(offset, Bias::Left),
    }
}

/// The answers of `L`'s rules for `lines`; see [`super::rust`].
pub(super) fn answers<L: Lexer>(
    rope: &Rope,
    settings: Settings,
    lines: &[Line],
    budget: &mut Budget,
) -> Vec<Answer> {
    let mut answers = Vec::with_capacity(lines.len());
    let mut pass: Option<Pass<'_, L>> = None;
    let mut last: Option<(usize, Answer, Line)> = None;
    let mut row_before = None;
    for &line in lines {
        // The line after the one before: where the pass is, past that line, with no anchor
        // in between to look back for.
        let follows = |row: usize| row_before.is_some_and(|before: usize| before + 1 == row);
        let adjacent = pass.is_some() && matches!(line, Line::Row(row) if follows(row));
        let target = match (&pass, line) {
            (Some(p), Line::Row(row)) if adjacent => p
                .reader
                .next_line_start()
                .unwrap_or_else(|| rope.line_start(row)),
            _ => start_of(rope, line),
        };
        row_before = match line {
            Line::Row(row) => Some(row),
            Line::New(_) | Line::Empty(_) => None,
        };
        // The line a break leaves behind it, when the one before is the empty line between
        // it and this one.
        let empty_before = match last {
            Some((at, answer, Line::Empty(_))) if at == target && matches!(line, Line::New(_)) => {
                Some(match answer {
                    Answer::Column(column) => column,
                    Answer::Keep => 0,
                })
            }
            _ => None,
        };
        let answer = match last {
            Some((at, answer, _)) if at == target && empty_before.is_none() => answer,
            Some((at, _, _)) if target < at => {
                // The pass stays where it is, past this line.
                row_before = None;
                fallback(rope, settings, line, budget)
            }
            _ => {
                let carried = pass.take();
                let asked = Asked {
                    line,
                    target,
                    adjacent,
                    empty_before,
                };
                match answer_at(rope, settings, asked, carried, budget) {
                    Some((p, answer)) => {
                        pass = Some(p);
                        answer
                    }
                    None => {
                        row_before = None;
                        fallback(rope, settings, line, budget)
                    }
                }
            }
        };
        last = Some((target, answer, line));
        answers.push(answer);
    }
    answers
}

/// A line asked about, where it starts, whether it is the row after the one asked before,
/// and the indentation of the empty line a break leaves before it, if there is one.
#[derive(Clone, Copy)]
struct Asked {
    line: Line,
    target: usize,
    adjacent: bool,
    empty_before: Option<usize>,
}

/// The answer for `asked` and the pass at its start, or none past the budget.
fn answer_at<'a, L: Lexer>(
    rope: &'a Rope,
    settings: Settings,
    asked: Asked,
    carried: Option<Pass<'a, L>>,
    budget: &mut Budget,
) -> Option<(Pass<'a, L>, Answer)> {
    let mut p = pass_at(
        rope,
        settings,
        asked.target,
        carried,
        asked.adjacent,
        budget,
    )?;
    let new = !matches!(asked.line, Line::Row(_));
    if let Some(column) = asked.empty_before {
        p.lexer.line("", column);
    } else if new && p.reader.line_start {
        // A break at a line's start leaves an empty line behind it.
        p.lexer.line("", 0);
    }
    let head = match asked.line {
        // Nothing follows the break on an empty line.
        Line::Empty(_) => Some(Head::blank()),
        _ => p.reader.head(budget),
    };
    let told = head.and_then(|head| p.lexer.answer(&head, new));
    // Where the rules cannot tell, the pass goes on all the same.
    let answer = match told.map(deepest) {
        Some(answer) => answer,
        None => fallback(rope, settings, asked.line, budget),
    };
    p.indent = match answer {
        Answer::Column(column) => Some(column),
        Answer::Keep => None,
    };
    Some((p, answer))
}

/// `answer`, unless it is past [`MAX_COLUMN`], when the line keeps its indentation.
fn deepest(answer: Answer) -> Answer {
    match answer {
        Answer::Column(column) if column > MAX_COLUMN => Answer::Keep,
        answer => answer,
    }
}

/// The state at `target`, a line's start, going on from `carried` when it is near enough
/// and from the nearest anchor otherwise, or none past the budget.
pub(super) fn pass_at<'a, L: Lexer>(
    rope: &'a Rope,
    settings: Settings,
    target: usize,
    carried: Option<Pass<'a, L>>,
    adjacent: bool,
    budget: &mut Budget,
) -> Option<Pass<'a, L>> {
    let floor = target.saturating_sub(LINE_LOOKBACK);
    let mut carried = carried.filter(|pass| (floor..=target).contains(&pass.reader.at));
    if let Some(pass) = &mut carried
        && pass.lexer.is_inside()
    {
        // A line that looks like an anchor inside a string or comment proves nothing:
        // read on to where it closes first.
        pass.read(settings, target, budget, true)?;
    }
    // Near enough, reading on costs less than looking back.
    let from = match &carried {
        Some(_) if adjacent => None,
        Some(pass) if target - pass.reader.at <= NEAR || pass.lexer.is_inside() => None,
        Some(pass) => Some(pass.reader.at),
        None => Some(floor),
    };
    let anchor = match from {
        Some(from) => find_anchor::<L>(rope, from, target, budget).ok()?,
        None => None,
    };
    let mut pass = match (anchor, carried) {
        (Some(anchor), Some(pass)) if anchor <= pass.reader.at => pass,
        (Some(anchor), _) => Pass {
            reader: Reader::new(rope, anchor),
            lexer: L::fresh(settings),
            indent: None,
        },
        (None, carried) => carried?,
    };
    pass.read(settings, target, budget, false)?;
    Some(pass)
}

impl<L: Lexer> Pass<'_, L> {
    /// Reads on to `target`, or with `outside` only until the pass is outside any string
    /// or comment. None past the budget.
    fn read(
        &mut self,
        settings: Settings,
        target: usize,
        budget: &mut Budget,
        outside: bool,
    ) -> Option<()> {
        let mut answered = self.indent.take();
        while !outside || self.lexer.is_inside() {
            let line_start = self.reader.line_start;
            let Some(text) = self.reader.next(target, budget) else {
                break;
            };
            let text = text.ok()?;
            // A line the command answered is where it puts it: an anchor only at column 0.
            let (column, starts) = match answered.take() {
                Some(column) => (column, text.trim_start_matches([' ', '\t'])),
                None => (measure(text, settings.tab_width()).column, text),
            };
            if line_start && column == 0 && !self.lexer.is_inside() && L::is_anchor(head_of(starts))
            {
                self.lexer.reset(settings);
            }
            self.lexer.line(text, column);
        }
        Some(())
    }
}

fn head_of(text: &str) -> &[u8] {
    &text.as_bytes()[..text.len().min(HEAD_BYTES)]
}

/// The start of the last anchor line that starts from `from` up to before `target`, or
/// the buffer's start if `from` is 0, read backward a window at a time.
fn find_anchor<L: Lexer>(
    rope: &Rope,
    from: usize,
    target: usize,
    budget: &mut Budget,
) -> Result<Option<usize>, Spent> {
    if !L::HAS_ANCHORS {
        return Ok((from == 0).then_some(0));
    }
    let mut window = Vec::new();
    let mut end = target;
    // Most anchors are near: the windows grow from a small one.
    let mut size = FIRST_WINDOW;
    while end > from {
        let start = rope.clip_offset(end.saturating_sub(size).max(from), Bias::Right);
        size = (size * 2).min(WINDOW);
        // One byte before, to tell whether `start` starts a line, and a head's worth after.
        let base = rope.clip_offset(start.saturating_sub(1), Bias::Left);
        let tail = rope.clip_offset((end + HEAD_BYTES).min(target), Bias::Right);
        if !budget.spend(tail - base) {
            return Err(Spent);
        }
        window.clear();
        for chunk in rope.chunks_in_range(base..tail) {
            window.extend_from_slice(chunk.as_bytes());
        }
        let mut index = window.len();
        while let Some(newline) = window[..index].iter().rposition(|&byte| byte == b'\n') {
            index = newline;
            let line_start = base + newline + 1;
            if line_start >= end {
                continue;
            }
            if line_start < start {
                break;
            }
            let head = &window[newline + 1..];
            if L::is_anchor(&head[..head.len().min(HEAD_BYTES)]) {
                return Ok(Some(line_start));
            }
        }
        if start == 0 {
            return Ok(Some(0));
        }
        if start >= end {
            break;
        }
        end = start;
    }
    Ok((from == 0).then_some(0))
}

/// The answer for a line past the budget: an existing line keeps its indentation, and a
/// new one takes that of the nearest non-blank line it follows, as Helix's `simple` does.
/// What that reads comes from the budget's reserve for fallbacks; past it, no indentation.
pub(super) fn fallback(rope: &Rope, settings: Settings, line: Line, budget: &mut Budget) -> Answer {
    let (Line::New(offset) | Line::Empty(offset)) = line else {
        return Answer::Keep;
    };
    let target = rope.clip_offset(offset, Bias::Left);
    for back in FALLBACK_BACK {
        let base = rope.clip_offset(target.saturating_sub(back), Bias::Left);
        if !budget.spend_reserve(target - base) {
            break;
        }
        let text: String = rope.chunks_in_range(base..target).collect();
        // The lines it holds, nearest first; the first one is whole only at the buffer's start.
        let mut lines = text.rsplit('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() && base > 0 {
                break;
            }
            let indentation = measure(line, settings.tab_width());
            if !indentation.blank {
                return deepest(Answer::Column(indentation.column));
            }
        }
        if base == 0 {
            break;
        }
    }
    Answer::Column(0)
}
