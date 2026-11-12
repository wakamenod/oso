//! Highlighting a document's text from its tree, in pieces of whole lines and in bounded
//! slices, the lines a window shows first (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - A piece is whole lines of at most [`PIECE`] bytes; a longer line is cut at a character,
//!   so a window on a long line waits only for the part it shows. In Markdown, a piece widens
//!   to the inline nodes and the fences with a language it meets: an edit can change a
//!   paragraph's inline text, or a fence's code, past its line.
//! - A query that passes its match limit leaves its piece plain. One stopped by the slice's
//!   end stays to do, unless it had most of a slice: then pieces halve, back up to [`PIECE`]
//!   after a few fit in a row. The document fails as too slow, as after a slow parse, if a
//!   piece of [`MIN_PIECE`] bytes, or a Markdown piece widened past the limit, still does not
//!   fit; if a piece runs far past its slice; or if highlighting takes [`HIGHLIGHT_LIMIT`] in
//!   all before it is done.
//! - Line breaks are never highlighted ([`crate::paint`]).

use std::cell::Cell;
use std::ops::Range;
use std::time::{Duration, Instant};

use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use oso_core::syntax::{Highlight, Highlights, Splice};
use tree_sitter::{
    Node, Query, QueryCursor, QueryCursorOptions, QueryCursorState, StreamingIterator, Tree,
};

use super::inline::{Nested, widen};
use super::{Document, Failure, Parsed, until};
use crate::language::Language;
use crate::paint::{Capture, paint};
use crate::queries::{Kind, Queries};

/// The most bytes one piece takes.
pub(super) const PIECE: usize = 16 << 10;

/// The fewest bytes a piece shrinks to.
const MIN_PIECE: usize = 1 << 10;

/// How far past its slice's end a piece may run, at least, before the document fails as too
/// slow: tree-sitter asks its progress callback only every so many steps. A slice shorter than
/// this is too short to judge a piece by.
const MIN_OVERRUN: Duration = Duration::from_millis(10);

/// How many pieces in a row must fit before pieces grow again, so that a slow text does not
/// shrink and grow them by turns.
const FITS_TO_GROW: u32 = 4;

/// How much of the worker's time highlighting may take in all until it is done, as a parse has
/// its limits. Past it, the document fails as too slow.
pub(super) const HIGHLIGHT_LIMIT: Duration = Duration::from_secs(2);

/// The longest text a predicate reads: the queries test names (`#match?` on identifiers), and
/// a longer node, which the predicate would copy for each piece, matches none.
const MAX_PREDICATE_BYTES: usize = 1 << 10;

/// The most matches a query keeps in progress at once: past it, its piece stays plain.
const MATCH_LIMIT: u32 = 256;

/// What a call to [`Document::highlight`] came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Highlighted {
    /// The tree is for the newest text, and all of it is highlighted.
    Done,
    /// The slice ran out; call again to go on.
    Paused,
    /// The document stopped for good, and its text is plain.
    Failed(Failure),
}

/// How a piece went.
pub(super) enum Piece {
    /// Its runs, and the blocks of the fences a fence's grammar painted in it.
    Done(Vec<(usize, Option<Highlight>)>, Vec<Range<usize>>),
    /// A query passed its match limit: the piece stays plain.
    Limit,
    /// The slice ran out.
    Stopped,
}

impl Document {
    /// Parses and highlights for at most about `slice`, going on from where the last call
    /// stopped. Of the text still to highlight, what meets `first` (the ranges windows show)
    /// goes first.
    pub fn highlight(
        &mut self,
        queries: &mut Queries,
        first: &[Range<usize>],
        slice: Duration,
    ) -> Highlighted {
        let budget = slice.min(self.limits.0);
        let deadline = Instant::now() + budget;
        if let Some(failure) = self.failed {
            return Highlighted::Failed(failure);
        }
        if !self.prepare(queries) {
            self.fail(Failure::Incompatible);
            return Highlighted::Failed(Failure::Incompatible);
        }
        // What compiling the queries left of the slice.
        match self.parse(deadline.saturating_duration_since(Instant::now())) {
            Parsed::Done => {}
            Parsed::Paused => return Highlighted::Paused,
            Parsed::Failed(failure) => return Highlighted::Failed(failure),
        }
        let started = Instant::now();
        let done = self.highlight_pieces(queries, first, budget, deadline);
        self.spent += started.elapsed();
        match done {
            Highlighted::Done => self.spent = Duration::ZERO,
            Highlighted::Paused if self.spent > self.limits.2 => {
                self.fail(Failure::TooSlow);
                return Highlighted::Failed(Failure::TooSlow);
            }
            Highlighted::Paused | Highlighted::Failed(_) => {}
        }
        done
    }

    /// Highlights pieces until `deadline` or none is left.
    fn highlight_pieces(
        &mut self,
        queries: &Queries,
        first: &[Range<usize>],
        budget: Duration,
        deadline: Instant,
    ) -> Highlighted {
        // tree-sitter asks its callback only so often, so one piece can run past the deadline.
        // A slice shorter than that is too short to fail a piece by, though pieces shrink in it:
        // only the total limit holds it.
        let judged = budget >= MIN_OVERRUN;
        let overrun = deadline + budget.max(MIN_OVERRUN);
        while let Some(piece) = self.next_piece(first) {
            let started = Instant::now();
            if started >= deadline {
                return Highlighted::Paused;
            }
            // A piece that had most of a slice and still did not fit will not fit the next.
            let had_a_slice = deadline - started >= budget / 2;
            let (piece, outcome) = self.highlight_piece(queries, piece, deadline);
            if judged && Instant::now() > overrun {
                self.fail(Failure::TooSlow);
                return Highlighted::Failed(Failure::TooSlow);
            }
            match outcome {
                Piece::Done(runs, blocks) => {
                    self.highlights.replace(piece.clone(), runs);
                    self.changes += 1;
                    self.pending.remove(piece.clone());
                    self.repainted(&piece, &blocks);
                    self.fits += 1;
                    if self.fits >= FITS_TO_GROW {
                        self.max_piece = (self.max_piece * 2).min(PIECE);
                        self.fits = 0;
                    }
                }
                Piece::Limit => self.leave_plain(piece),
                // A Markdown piece widened past the limit would not shrink with it.
                Piece::Stopped
                    if had_a_slice
                        && self.max_piece > MIN_PIECE
                        && piece.len() <= self.max_piece =>
                {
                    self.max_piece /= 2;
                    self.fits = 0;
                }
                // Too slow for any slice: the document stops, as after a slow parse.
                Piece::Stopped if had_a_slice && judged => {
                    self.fail(Failure::TooSlow);
                    return Highlighted::Failed(Failure::TooSlow);
                }
                Piece::Stopped => return Highlighted::Paused,
            }
        }
        Highlighted::Done
    }

    /// The highlights of the newest text: plain where it is not highlighted yet.
    pub fn highlights(&self) -> &Highlights {
        &self.highlights
    }

    /// A number that changes whenever the highlights may have, so that a report is sent only
    /// for a change.
    pub(crate) fn changes(&self) -> u64 {
        self.changes
    }

    /// Whether the tree is for the newest text and nothing that meets `ranges` is still to
    /// highlight.
    pub(crate) fn is_highlighted(&self, ranges: &[Range<usize>]) -> bool {
        if !self.current || self.waiting.is_some() || self.tree.is_none() {
            return false;
        }
        let pending = self.pending.ranges();
        ranges.iter().all(|range| {
            let at = pending.partition_point(|pending| pending.end <= range.start);
            pending
                .get(at)
                .is_none_or(|pending| pending.start >= range.end.max(range.start + 1))
        })
    }

    /// Compiles the queries the document's language needs, and says whether they compiled.
    fn prepare(&self, queries: &mut Queries) -> bool {
        let mut kinds = vec![Kind::Highlights(self.language)];
        if self.language == Language::Markdown {
            kinds.extend([
                Kind::Injections,
                Kind::Highlights(Language::MarkdownInline),
                Kind::Highlights(Language::Rust),
                Kind::Highlights(Language::Toml),
            ]);
        }
        kinds.into_iter().all(|kind| queries.prepare(kind))
    }

    fn leave_plain(&mut self, piece: Range<usize>) {
        self.highlights.replace(piece.clone(), []);
        self.changes += 1;
        self.pending.remove(piece.clone());
        self.repainted(&piece, &[]);
    }

    /// The next piece to highlight: of what is to do and meets `first`, the first; or else
    /// the first of the rest; with what else is to do within its reach.
    pub(super) fn next_piece(&self, first: &[Range<usize>]) -> Option<Range<usize>> {
        let pending = self.pending.ranges();
        let shown = first.iter().find_map(|range| {
            let range = lines(&self.text, range.clone(), self.max_piece);
            let at = pending.partition_point(|pending| pending.end <= range.start);
            pending
                .get(at)
                .filter(|pending| pending.start < range.end)
                .map(|pending| pending.start.max(range.start)..pending.end.min(range.end))
        });
        let range = shown.or_else(|| pending.first().cloned())?;
        // The pending ranges within reach of its start join it, so that edits at many cursors
        // cost a piece a reach, not a piece each; what lies between is highlighted again.
        let reach = range.start.saturating_add(self.max_piece);
        let at = pending.partition_point(|pending| pending.end <= range.start);
        let end = pending[at..]
            .iter()
            .take_while(|pending| pending.start < reach)
            .last()
            .map_or(range.end, |last| last.end.max(range.end));
        Some(cut(&self.text, range.start..end, self.max_piece))
    }

    /// Runs the queries over `piece` until `deadline`, and says what range they ran over: in
    /// Markdown, `piece` widened to whole inline nodes and fences.
    fn highlight_piece(
        &mut self,
        queries: &Queries,
        piece: Range<usize>,
        deadline: Instant,
    ) -> (Range<usize>, Piece) {
        // A new handle on the same tree, so that its nodes outlive the borrows of `self`.
        let Some(tree) = self.tree.clone() else {
            return (piece, Piece::Limit);
        };
        let mut nested = Nested::default();
        let piece = match self.language {
            Language::Markdown => {
                match widen(&self.text, queries, &tree, piece.clone(), deadline) {
                    Ok((widened, found)) => {
                        nested = found;
                        widened
                    }
                    Err(outcome) => return (piece, outcome),
                }
            }
            Language::Rust | Language::MarkdownInline | Language::Toml => piece,
        };
        let outcome = self.highlight_range(queries, &tree, nested, piece.clone(), deadline);
        (piece, outcome)
    }

    fn highlight_range(
        &mut self,
        queries: &Queries,
        tree: &Tree,
        nested: Nested,
        piece: Range<usize>,
        deadline: Instant,
    ) -> Piece {
        let Some(prepared) = queries.get(Kind::Highlights(self.language)) else {
            return Piece::Limit;
        };
        let mut captures = Vec::new();
        let found = |node: Node, pattern, index: u32| {
            if let Some(&Some(class)) = prepared.classes.get(index as usize) {
                captures.push(Capture {
                    range: node.byte_range(),
                    layer: 0,
                    pattern,
                    class,
                });
            }
        };
        let outcome = run(
            &prepared.query,
            tree,
            &self.text,
            piece.clone(),
            deadline,
            found,
        );
        if let Some(outcome) = outcome {
            return outcome;
        }
        // An injected grammar's captures win over the block grammar's on the same span.
        let inlines = nested
            .inlines
            .iter()
            .map(|&node| (node, Language::MarkdownInline));
        let layers = [(1, inlines.collect()), (2, nested.fences)];
        for (layer, nodes) in layers {
            let at = (piece.clone(), layer);
            if let Some(outcome) =
                self.highlight_nested(queries, nodes, at, deadline, &mut captures)
            {
                return outcome;
            }
        }
        Piece::Done(paint(&self.text, piece, captures), nested.blocks)
    }
}

/// Runs `query` over `piece` of `tree` until `deadline`, giving each capture's node, pattern
/// and capture index to `found`. `None` when it ran to its end, or how the piece went if it did
/// not.
pub(super) fn run<'tree>(
    query: &Query,
    tree: &'tree Tree,
    text: &Rope,
    piece: Range<usize>,
    deadline: Instant,
    mut found: impl FnMut(Node<'tree>, usize, u32),
) -> Option<Piece> {
    let stopped = Cell::new(false);
    let mut progress = until::<QueryCursorState>(deadline, &stopped);
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(piece);
    cursor.set_match_limit(MATCH_LIMIT);
    // Never panics: a panic here would cross tree-sitter's C. A predicate copies the node's
    // text each time, so a node too long for a name reads as empty, and matches none.
    let source = |node: Node| {
        let range = node.byte_range();
        let range = match range.len() {
            len if len > MAX_PREDICATE_BYTES => range.start..range.start,
            _ => range,
        };
        text.chunks_in_range(range).map(str::as_bytes)
    };
    let options = QueryCursorOptions::new().progress_callback(&mut progress);
    let mut matches = cursor.matches_with_options(query, tree.root_node(), source, options);
    while let Some(found_match) = matches.next() {
        for capture in found_match.captures() {
            found(capture.node, found_match.pattern_index, capture.index);
        }
    }
    drop(matches);
    if stopped.get() {
        Some(Piece::Stopped)
    } else if cursor.did_exceed_match_limit() {
        Some(Piece::Limit)
    } else {
        None
    }
}

/// The lines a batch of splices left in `after`, ascending.
pub(super) fn edited<'a>(
    after: &'a Rope,
    splices: &'a [Splice],
    max_piece: usize,
) -> impl Iterator<Item = Range<usize>> + 'a {
    let mut delta = 0isize;
    splices.iter().map(move |splice| {
        let start = splice.start.saturating_add_signed(delta);
        delta += splice.new_len as isize - splice.old_len as isize;
        lines(after, start..start + splice.new_len, max_piece)
    })
}

/// `range` widened to whole lines, from the start of its first to the start of the line after
/// its last, but by at most `max_piece` bytes each way, at a character.
pub(super) fn lines(text: &Rope, range: Range<usize>, max_piece: usize) -> Range<usize> {
    let len = text.len();
    let (start, end) = (range.start.min(len), range.end.min(len));
    let line_start = text.line_start(text.offset_to_point(start).row);
    let start = if start - line_start <= max_piece {
        line_start
    } else {
        text.clip_offset(start, Bias::Left)
    };
    let row = text.offset_to_point(end).row;
    let next = if row + 1 < text.line_count() {
        text.line_start(row + 1)
    } else {
        len
    };
    let end = if next - end <= max_piece {
        next
    } else {
        text.clip_offset(end, Bias::Right)
    };
    start..end.max(start)
}

/// The first piece of `range`: whole lines of at most `max_piece` bytes, or `max_piece` bytes of a
/// longer line, cut at a character.
fn cut(text: &Rope, range: Range<usize>, max_piece: usize) -> Range<usize> {
    if range.end - range.start <= max_piece {
        return range;
    }
    let limit = range.start + max_piece;
    let line_start = text.line_start(text.offset_to_point(limit).row);
    let end = if line_start > range.start {
        line_start
    } else {
        text.clip_offset(limit, Bias::Left)
    };
    // A piece is never empty, or highlighting would not get on.
    range.start..if end > range.start { end } else { range.end }
}
