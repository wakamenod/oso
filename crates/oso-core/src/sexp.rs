//! Motion by balanced expressions, as Emacs's `scan-lists` and `scan-sexps` (ADR 0190).
//!
//! Must not depend on: `editor`, `window` or `command`; it reads a display snapshot and a
//! buffer's highlights, works on offsets and selections, and does no I/O.
//!
//! - A sexp, read from point: space, punctuation and comments are passed; then it is a string,
//!   a bracket group with what it holds, or a run of symbol characters. A symbol character is a
//!   letter or a digit of any script, or `_`, and in a mode's own way `$` or `-`. A mark goes
//!   with what it follows, so every stop is a display cluster's start.
//! - Brackets are `()`, `[]` and `{}`, and in HTML a tag's `<` and `>`. Any closer closes any
//!   opener, as Emacs counts only the depth.
//! - Strings and comments come from the buffer's highlights, a run passed with one seek. With
//!   none, each line is lexed on its own with its mode's table (`lex`).
//! - A motion that starts inside a string or a comment reads only inside it, so its ends are a
//!   containing expression's; `up_list` leaves it first.
//! - A motion reads within its command's budget. Past it, the cursor stays and the motion fails
//!   with [`ScanError::TooFar`]: half a sexp is no place to stop.
//! - A motion that fails leaves the cursor where it was.

mod lex;
mod lines;
mod read;

use std::ops::Range;

use read::{Ahead, Behind, Read, Token};

use crate::buffer::Mode;
use crate::display_map::DisplaySnapshot;
use crate::motion::{TOO_MUCH, WalkBudget};
use crate::selection::Selection;
use crate::syntax::Highlights;

/// How a mode's text reads as expressions: its symbol characters, its brackets, and its
/// strings and comments where the buffer has no highlights.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    /// Brackets and symbols only.
    Fundamental,
    Rust,
    Go,
    /// JavaScript and TypeScript, JSX and TSX among them.
    Script,
    Css,
    Json,
    Html,
    Markdown,
    Toml,
    Yaml,
    Latex,
    Swift,
}

impl Table {
    /// The table of a buffer in `mode`.
    pub fn of(mode: Mode) -> Self {
        match mode {
            Mode::Rust => Self::Rust,
            Mode::Go => Self::Go,
            Mode::TypeScript | Mode::Tsx | Mode::JavaScript => Self::Script,
            Mode::Css => Self::Css,
            Mode::Json => Self::Json,
            Mode::Html => Self::Html,
            // A plan is Markdown (ADR 0165, part 3d).
            Mode::Markdown | Mode::Plan => Self::Markdown,
            Mode::Toml => Self::Toml,
            Mode::Yaml => Self::Yaml,
            Mode::Latex => Self::Latex,
            Mode::Swift => Self::Swift,
            Mode::Fundamental
            | Mode::CopyList
            | Mode::CopyView
            | Mode::Sessions
            | Mode::Brief
            | Mode::Composer
            | Mode::Worktrees
            | Mode::Review
            | Mode::ReviewFiles
            | Mode::ReviewCommits
            | Mode::ReviewMessage
            | Mode::Terminal
            | Mode::Directory
            | Mode::GitStatus
            | Mode::GitGraph
            | Mode::GitPr
            | Mode::GitPrList
            | Mode::GitPrMessage
            | Mode::GitIssueList
            | Mode::GitIssue
            | Mode::GitCommit
            | Mode::GitLog
            | Mode::GitDiscarded
            | Mode::Conversation
            | Mode::RenamePreview
            | Mode::SessionMove
            | Mode::NativeSession
            | Mode::Card
            | Mode::Request
            | Mode::Todo => Self::Fundamental,
        }
    }
}

/// What a run of text is to expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Code,
    Str,
    Comment,
}

/// A buffer's text as expressions read it: its display snapshot, its highlights if it has
/// them, and its mode's table.
#[derive(Clone, Copy)]
pub struct Syntax<'a> {
    display: &'a DisplaySnapshot,
    highlights: Option<&'a Highlights>,
    table: Table,
}

impl<'a> Syntax<'a> {
    /// The text of `display`, its strings and comments as `highlights` says. Highlights that do
    /// not cover the text are not read.
    pub fn new(
        display: &'a DisplaySnapshot,
        highlights: Option<&'a Highlights>,
        table: Table,
    ) -> Self {
        Self {
            display,
            highlights: highlights.filter(|highlights| highlights.len() == display.len()),
            table,
        }
    }
}

impl std::fmt::Debug for Syntax<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Syntax")
            .field("table", &self.table)
            .field("highlights", &self.highlights.is_some())
            .finish_non_exhaustive()
    }
}

/// Why an expression motion did not move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScanError {
    /// A closer came first, or the end of the string or comment the motion started in.
    EndsPrematurely,
    /// A bracket has no match, or there is no list to go up out of.
    Unbalanced,
    /// `transpose_ranges` found no two expressions to swap.
    NothingToTranspose,
    /// The motion would read more than its budget.
    TooFar,
}

impl ScanError {
    /// The message Emacs shows.
    pub fn message(self) -> &'static str {
        match self {
            Self::EndsPrematurely => "Containing expression ends prematurely",
            Self::Unbalanced => "Unbalanced parentheses",
            Self::NothingToTranspose => "Don't have two things to transpose",
            Self::TooFar => TOO_MUCH,
        }
    }
}

fn charge(budget: &mut WalkBudget, bytes: usize) -> Result<(), ScanError> {
    if budget.spend(bytes) {
        Ok(())
    } else {
        Err(ScanError::TooFar)
    }
}

/// Scans `count` expressions (`sexp`) or lists from `from`, starting `depth` levels in, as
/// Emacs's `scan_lists`: `Ok(None)` if the text ran out at depth 0 outside any string.
fn scan(
    syntax: &Syntax<'_>,
    from: usize,
    count: usize,
    forward: bool,
    depth: isize,
    sexp: bool,
    budget: &mut WalkBudget,
) -> Result<Option<usize>, ScanError> {
    if forward {
        scan_with(Ahead::new(syntax, from, budget)?, count, depth, sexp)
    } else {
        scan_with(Behind::new(syntax, from, budget)?, count, depth, sexp)
    }
}

fn scan_with(
    mut reader: impl Read,
    count: usize,
    start_depth: isize,
    sexp: bool,
) -> Result<Option<usize>, ScanError> {
    let min_depth = start_depth.min(0);
    for _ in 0..count {
        let mut depth = start_depth;
        loop {
            match reader.token()? {
                Token::Atom => {
                    if depth == 0 && sexp {
                        break;
                    }
                }
                Token::Enter => {
                    depth += 1;
                    if depth == 0 {
                        break;
                    }
                }
                Token::Leave => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    if depth < min_depth {
                        return Err(ScanError::EndsPrematurely);
                    }
                }
                Token::End => {
                    return if depth != 0 {
                        Err(ScanError::Unbalanced)
                    } else if reader.confined() {
                        Err(ScanError::EndsPrematurely)
                    } else {
                        Ok(None)
                    };
                }
            }
        }
    }
    Ok(Some(reader.pos()))
}

/// `offset`, or if it falls inside a display cluster, that cluster's end going forward and its
/// start going backward.
fn clip(display: &DisplaySnapshot, offset: usize, forward: bool) -> usize {
    let (start, end) = display.cluster_at(offset);
    if start == offset {
        offset
    } else if forward {
        end
    } else {
        start
    }
}

/// The buffer's edge in a direction.
fn edge(display: &DisplaySnapshot, forward: bool) -> usize {
    if forward { display.len() } else { 0 }
}

/// Where `n` expressions from `offset` end, as Emacs's `scan-sexps`: forward if `n` is
/// positive, backward to their start if negative, and `offset` itself for 0. Past the last
/// expression it is the buffer's edge.
pub fn sexp_end(
    syntax: &Syntax<'_>,
    offset: usize,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<usize, ScanError> {
    if n == 0 {
        return Ok(offset);
    }
    let forward = n > 0;
    let found = scan(syntax, offset, n.unsigned_abs(), forward, 0, true, budget)?;
    Ok(found.map_or_else(
        || edge(syntax.display, forward),
        |at| clip(syntax.display, at, forward),
    ))
}

/// Moves `selection` over `n` expressions, as Emacs's `forward-sexp`: backward if `n` is
/// negative, and to the buffer's edge past the last.
pub fn forward_sexp(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), ScanError> {
    let to = sexp_end(syntax, selection.cursor, n, budget)?;
    selection.move_to(to);
    Ok(())
}

/// Moves `selection` over `n` bracket groups, passing what is not one, as Emacs's
/// `forward-list`: backward if `n` is negative, and to the buffer's edge past the last.
pub fn forward_list(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), ScanError> {
    if n == 0 {
        selection.move_to(selection.cursor);
        return Ok(());
    }
    let forward = n > 0;
    let found = scan(
        syntax,
        selection.cursor,
        n.unsigned_abs(),
        forward,
        0,
        false,
        budget,
    )?;
    let to = found.map_or_else(
        || edge(syntax.display, forward),
        |at| clip(syntax.display, at, forward),
    );
    selection.move_to(to);
    Ok(())
}

/// Moves `selection` out of `n` levels of brackets: past the closer if `n` is positive, as
/// Emacs's `up-list`, and to the opener if negative, as `backward-up-list`. Inside a string or
/// a comment, the first level is leaving it.
pub fn up_list(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), ScanError> {
    let forward = n > 0;
    let mut at = selection.cursor;
    for _ in 0..n.unsigned_abs() {
        at = match read::around(syntax, at, budget)? {
            Some((range, _)) => {
                if forward {
                    range.end
                } else {
                    range.start
                }
            }
            None => scan(syntax, at, 1, forward, 1, false, budget)?.ok_or(ScanError::Unbalanced)?,
        };
    }
    selection.move_to(clip(syntax.display, at, forward));
    Ok(())
}

/// Moves `selection` into `n` levels of brackets: past the next opener if `n` is positive, as
/// Emacs's `down-list`, and before the previous closer if negative.
pub fn down_list(
    syntax: &Syntax<'_>,
    selection: &mut Selection,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(), ScanError> {
    let forward = n > 0;
    let mut at = selection.cursor;
    for _ in 0..n.unsigned_abs() {
        at = scan(syntax, at, 1, forward, -1, false, budget)?.ok_or(ScanError::Unbalanced)?;
    }
    selection.move_to(clip(syntax.display, at, forward));
    Ok(())
}

/// The expressions `transpose-sexps` swaps at `offset`, in the text's order: for a positive
/// `n`, the one before `offset` and the `n` after it, and point goes to the second's end; for a
/// negative `n`, the `-n` before the one before `offset`, and that one, and point goes to the
/// first's start plus the second's length. `n` of 0 swaps the expressions at point and the
/// mark, which the caller finds with [`sexp_end`]; here it has nothing to transpose.
pub fn transpose_ranges(
    syntax: &Syntax<'_>,
    offset: usize,
    n: isize,
    budget: &mut WalkBudget,
) -> Result<(Range<usize>, Range<usize>), ScanError> {
    let count = n.unsigned_abs();
    let found = |from: usize, forward: bool, count: usize, budget: &mut WalkBudget| {
        scan(syntax, from, count, forward, 0, true, budget)?
            .map(|at| clip(syntax.display, at, forward))
            .ok_or(ScanError::NothingToTranspose)
    };
    if n == 0 {
        return Err(ScanError::NothingToTranspose);
    }
    let start = found(offset, false, 1, budget)?;
    let end = found(start, true, 1, budget)?;
    let one = start..end;
    let other = if n > 0 {
        let other_end = found(end, true, count, budget)?;
        found(other_end, false, count, budget)?..other_end
    } else {
        let other_start = found(start, false, count, budget)?;
        other_start..found(other_start, true, count, budget)?
    };
    let (first, second) = if n > 0 { (one, other) } else { (other, one) };
    if first.is_empty() || second.is_empty() || first.end > second.start {
        return Err(ScanError::NothingToTranspose);
    }
    Ok((first, second))
}

#[cfg(test)]
mod tests;
