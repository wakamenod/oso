//! Motion by defuns, found by the lines they start on (ADR 0190, Q2).
//!
//! Must not depend on: `window` or `word`; it reads text and runs over a display snapshot.
//!
//! - A head is a line `heads` matches, outside strings and comments where the highlights say,
//!   at the head's first byte past its blanks. In Markdown its `#` must be a heading's marker.
//! - A head holds the heads of a greater level that follow it while it lasts: nested items
//!   are found by indentation, a Markdown section's by its level.
//! - A defun's bounds are whole lines, from the comment just above it (and in Rust its
//!   attributes) to the start of the line after its end.
//! - What the commands ask goes through [`Find`], so that the worker's ranges (step 7) can
//!   answer before the lines do.
//! - Each reads within its command's budget; past it, the motion stops at the last defun it
//!   found whole.

mod ends;
mod lines;

use std::ops::Range;

use lines::{LineAt, LinesAhead, LinesBehind};

use super::heads::{self, Ends, Head};
use super::{Kind, ScanError, Syntax, Table, charge, lex};
use crate::display_map::DisplaySnapshot;
use crate::motion::{self, Edge, SEEK_COST, WalkBudget};
use crate::selection::Selection;
use crate::syntax::Highlight;

/// Moves `selection` back to the start of the defun before it, `n` times, as Emacs's
/// `beginning-of-defun`: forward to the next defuns' starts if `n` is negative. A defun starts
/// at its head's first byte past its indentation. Past the last, it goes to the buffer's
/// start.
///
/// # Errors
///
/// None yet: the buffer's edge is reached quietly, as Emacs's is. The signature keeps the
/// `Edge` of the other motions (ADR 0190).
pub fn beginning_of_defun(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    let mut find = finder(syntax);
    let forward = n < 0;
    step(syntax, selection, n, forward, budget, |at, budget| {
        if forward {
            find.start_after(at, budget)
        } else {
            find.start_before(at, budget)
        }
    })
}

/// Moves `selection` past the end of the defun it is in, or of the next one, `n` times, as
/// Emacs's `end-of-defun`: back to the previous defuns' ends if `n` is negative. A defun ends
/// at the start of the line after its last. Past the last, it goes to the buffer's end.
///
/// # Errors
///
/// None yet, as [`beginning_of_defun`].
pub fn end_of_defun(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), Edge> {
    let mut find = finder(syntax);
    let forward = n > 0;
    step(syntax, selection, n, forward, budget, |at, budget| {
        if forward {
            find.end_after(at, budget)
        } else {
            find.end_before(at, budget)
        }
    })
}

/// The lines `mark-defun` marks at `offset`, as Emacs's: of the defun that holds it or
/// follows it, or past the last, the last; from the comment just above it to the start of the
/// line after it. `None` if the buffer has no defun there, or finding it would read more than
/// `budget` holds.
pub fn defun_bounds(
    syntax: &Syntax<'_>,
    offset: usize,
    budget: &mut WalkBudget,
) -> Option<Range<usize>> {
    finder(syntax).bounds(offset, budget).ok().flatten()
}

/// Moves `selection` to what `next` finds from where it is, `n` times. Past the last, it goes
/// to the buffer's edge with no error, as Emacs's does and as paragraphs do; past the budget,
/// it stays at the last found.
fn step(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    forward: bool,
    budget: &mut WalkBudget,
    mut next: impl FnMut(usize, &mut WalkBudget) -> Result<Option<usize>, ScanError>,
) -> Result<(), Edge> {
    let mut at = selection.cursor;
    for _ in 0..n.unsigned_abs() {
        match next(at, budget) {
            Ok(Some(to)) => at = to,
            Ok(None) => {
                at = if forward { syntax.display.len() } else { 0 };
                break;
            }
            Err(_) => break,
        }
    }
    selection.move_to(at);
    Ok(())
}

/// What finds defuns for the commands. Each answer is an offset past or before `offset`, or
/// `None` if there is no such defun.
trait Find {
    /// The start of the nearest defun that starts before `offset`.
    fn start_before(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError>;

    /// The start of the nearest defun that starts after `offset`.
    fn start_after(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError>;

    /// The end of the defun that holds `offset`, or of the next one in what holds it.
    fn end_after(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError>;

    /// The nearest end before `offset`.
    fn end_before(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError>;

    /// What [`defun_bounds`] gives.
    fn bounds(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<Range<usize>>, ScanError>;
}

/// What finds defuns in `syntax`'s text. Step 7 puts the worker's ranges first here.
fn finder<'a>(syntax: &Syntax<'a>) -> impl Find + 'a {
    ByLines {
        syntax: *syntax,
        json_indent: None,
    }
}

/// A head found on its line.
#[derive(Clone, Debug)]
struct Found {
    /// Where its line starts.
    line: usize,
    /// Its first byte past its indentation.
    at: usize,
    head: Head,
}

/// Defuns found by the lines their heads are on.
struct ByLines<'a> {
    syntax: Syntax<'a>,
    /// The indentation of a JSON buffer's top-level keys, once looked for.
    json_indent: Option<Option<usize>>,
}

/// The start of the line that holds `offset`.
fn line_of(display: &DisplaySnapshot, offset: usize) -> usize {
    motion::line_start(display, display.offset_to_display_point(offset).row)
}

/// The start of the line after the one that holds `offset`, or the buffer's end.
fn line_after(display: &DisplaySnapshot, offset: usize) -> usize {
    let row = display.offset_to_display_point(offset).row;
    if row + 1 < display.row_count() {
        motion::line_start(display, row + 1)
    } else {
        display.len()
    }
}

/// The least level a table's heads have, past which nothing holds them.
fn least_level(table: Table) -> usize {
    match table {
        Table::Markdown => 1,
        _ => 0,
    }
}

/// The blanks a line's first bytes start with.
fn indent(text: &[u8]) -> usize {
    text.iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count()
}

impl ByLines<'_> {
    fn display(&self) -> &DisplaySnapshot {
        self.syntax.display
    }

    /// The head `line` starts, whose first bytes are `text`, if it is one.
    fn head_of(
        &mut self,
        line: LineAt,
        text: &[u8],
        budget: &mut WalkBudget,
    ) -> Result<Option<Found>, ScanError> {
        let table = self.syntax.table;
        let json = if table == Table::Json {
            self.json_indent(budget)?
        } else {
            None
        };
        let shape = heads::Line {
            start: text,
            last: line.last,
        };
        let Some(head) = heads::head(table, shape, json) else {
            return Ok(None);
        };
        let at = line.start + head.indent;
        if let Some(highlights) = self.syntax.highlights {
            charge(budget, SEEK_COST)?;
            let highlight = highlights.run_at(at).and_then(|(_, highlight)| highlight);
            let code = match table {
                Table::Markdown => {
                    matches!(highlight, Some(Highlight::Markup | Highlight::Heading))
                }
                // A key may be highlighted as a string, and these have no long strings.
                Table::Json | Table::Yaml | Table::Toml => highlight != Some(Highlight::Comment),
                _ => !matches!(highlight, Some(Highlight::String | Highlight::Comment)),
            };
            if !code {
                return Ok(None);
            }
        }
        Ok(Some(Found {
            line: line.start,
            at,
            head,
        }))
    }

    /// The indentation of the first line that starts with a quote, in a JSON buffer.
    fn json_indent(&mut self, budget: &mut WalkBudget) -> Result<Option<usize>, ScanError> {
        if let Some(found) = self.json_indent {
            return Ok(found);
        }
        let mut lines = LinesAhead::new(self.syntax.display, 0);
        let mut found = None;
        while lines.next(budget)?.is_some() {
            let text = lines.text();
            let blanks = indent(text);
            if text.get(blanks) == Some(&b'"') {
                found = Some(blanks);
                break;
            }
        }
        self.json_indent = Some(found);
        Ok(found)
    }

    /// Whether a line goes with the head below it: a comment alone, or in Rust an attribute.
    fn is_above(
        &self,
        line: LineAt,
        text: &[u8],
        budget: &mut WalkBudget,
    ) -> Result<bool, ScanError> {
        let table = self.syntax.table;
        let blanks = indent(text);
        if line.blank {
            return Ok(false);
        }
        if table == Table::Rust && text[blanks..].starts_with(b"#[") {
            return Ok(true);
        }
        if let Some(highlights) = self.syntax.highlights {
            charge(budget, SEEK_COST)?;
            let at = line.start + blanks;
            return Ok(highlights
                .run_at(at)
                .is_some_and(|(_, highlight)| highlight == Some(Highlight::Comment)));
        }
        if !lex::lexes(table) {
            return Ok(false);
        }
        let mut runs = Vec::new();
        lex::lex(table, text, 0, &mut runs);
        Ok(runs
            .iter()
            .find(|(range, _)| range.contains(&blanks))
            .is_some_and(|(_, kind)| *kind == Kind::Comment))
    }

    /// Where the lines that go with `found` start above it, but not above `floor`.
    fn above(
        &self,
        found: &Found,
        floor: usize,
        budget: &mut WalkBudget,
    ) -> Result<usize, ScanError> {
        let mut start = found.line;
        let mut lines = LinesBehind::new(self.display(), found.line);
        while start > floor {
            let Some(line) = lines.next(budget)? else {
                break;
            };
            if line.start < floor || !self.is_above(line, lines.text(), budget)? {
                break;
            }
            start = line.start;
        }
        Ok(start)
    }

    /// The nearest head on the lines before `lines`' place, nearest first.
    fn prev(
        &mut self,
        lines: &mut LinesBehind<'_>,
        budget: &mut WalkBudget,
    ) -> Result<Option<Found>, ScanError> {
        while let Some(line) = lines.next(budget)? {
            if let Some(found) = self.head_of(line, lines.text(), budget)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// The nearest head on the lines from `lines`' place on, before `limit`.
    fn next(
        &mut self,
        lines: &mut LinesAhead<'_>,
        limit: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<Found>, ScanError> {
        while let Some(line) = lines.next(budget)? {
            if line.start >= limit {
                break;
            }
            if let Some(found) = self.head_of(line, lines.text(), budget)? {
                return Ok(Some(found));
            }
        }
        Ok(None)
    }

    /// The first head whose line starts after `offset` and before `limit`.
    fn head_after(
        &mut self,
        offset: usize,
        limit: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<Found>, ScanError> {
        charge(budget, SEEK_COST)?;
        let mut lines = LinesAhead::new(self.syntax.display, line_after(self.display(), offset));
        self.next(&mut lines, limit, budget)
    }

    /// The heads that hold the one at `level`, walking back with `lines`: the first whose
    /// defun lasts past `offset`, with its end.
    fn container(
        &mut self,
        lines: &mut LinesBehind<'_>,
        level: usize,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<(Found, usize)>, ScanError> {
        let least = least_level(self.syntax.table);
        let mut level = level;
        while level > least {
            let Some(found) = self.prev(lines, budget)? else {
                break;
            };
            // An environment holds one of its level, as LaTeX's nest.
            let env = matches!(found.head.ends, Ends::Env(_));
            if found.head.level < level || env {
                level = found.head.level;
                let end = self.end_of(&found, budget)?;
                if end > offset {
                    return Ok(Some((found, end)));
                }
            }
        }
        Ok(None)
    }

    /// The head of the defun that holds `offset` or follows it in what holds it, as
    /// `end-of-defun` finds it, with its end; or past the last, the last if `last` says so.
    fn at_or_after(
        &mut self,
        offset: usize,
        last: bool,
        budget: &mut WalkBudget,
    ) -> Result<Option<(Found, usize)>, ScanError> {
        charge(budget, SEEK_COST)?;
        let display = self.syntax.display;
        let mut lines = LinesBehind::new(display, line_after(display, offset));
        let before = self.prev(&mut lines, budget)?;
        let mut container = None;
        if let Some(before) = &before {
            let end = self.end_of(before, budget)?;
            if end > offset {
                return Ok(Some((before.clone(), end)));
            }
            container = self.container(&mut lines, before.head.level, offset, budget)?;
        }
        let limit = container.as_ref().map_or(display.len(), |(_, end)| *end);
        if let Some(next) = self.head_after(offset, limit, budget)? {
            let end = self.end_of(&next, budget)?;
            return Ok(Some((next, end)));
        }
        if container.is_some() {
            return Ok(container);
        }
        match before {
            Some(before) if last => {
                let end = self.end_of(&before, budget)?;
                Ok(Some((before, end)))
            }
            _ => Ok(None),
        }
    }
}

impl Find for ByLines<'_> {
    fn start_before(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError> {
        charge(budget, SEEK_COST)?;
        let display = self.syntax.display;
        let mut lines = LinesBehind::new(display, line_after(display, offset));
        while let Some(found) = self.prev(&mut lines, budget)? {
            if found.at < offset {
                return Ok(Some(found.at));
            }
        }
        Ok(None)
    }

    fn start_after(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError> {
        charge(budget, SEEK_COST)?;
        let display = self.syntax.display;
        let mut lines = LinesAhead::new(display, line_of(display, offset));
        while let Some(found) = self.next(&mut lines, display.len(), budget)? {
            if found.at > offset {
                return Ok(Some(found.at));
            }
        }
        Ok(None)
    }

    fn end_after(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError> {
        Ok(self.at_or_after(offset, false, budget)?.map(|(_, end)| end))
    }

    fn end_before(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<usize>, ScanError> {
        charge(budget, SEEK_COST)?;
        let display = self.syntax.display;
        let mut lines = LinesBehind::new(display, line_after(display, offset));
        let (mut best, mut level) = (None, 0);
        while let Some(found) = self.prev(&mut lines, budget)? {
            let end = self.end_of(&found, budget)?;
            if end < offset {
                (best, level) = (Some(end), found.head.level);
                break;
            }
        }
        // What holds the one found may end after it and still before `offset`.
        let least = least_level(self.syntax.table);
        while best.is_some() && level > least {
            let Some(found) = self.prev(&mut lines, budget)? else {
                break;
            };
            if found.head.level < level {
                level = found.head.level;
                let end = self.end_of(&found, budget)?;
                if end < offset && Some(end) > best {
                    best = Some(end);
                }
            }
        }
        Ok(best)
    }

    fn bounds(
        &mut self,
        offset: usize,
        budget: &mut WalkBudget,
    ) -> Result<Option<Range<usize>>, ScanError> {
        let display = self.syntax.display;
        // In the comment above a head: that head's.
        charge(budget, SEEK_COST)?;
        let mut lines = LinesAhead::new(display, line_of(display, offset));
        let mut first = true;
        while let Some(line) = lines.next(budget)? {
            if let Some(found) = self.head_of(line, lines.text(), budget)? {
                if !first {
                    let start = self.above(&found, 0, budget)?;
                    let end = self.end_of(&found, budget)?;
                    return Ok(Some(start..end));
                }
                break;
            }
            if !self.is_above(line, lines.text(), budget)? {
                break;
            }
            first = false;
        }
        let Some((found, end)) = self.at_or_after(offset, true, budget)? else {
            return Ok(None);
        };
        Ok(Some(self.above(&found, 0, budget)?..end))
    }
}

#[cfg(test)]
mod tests;
