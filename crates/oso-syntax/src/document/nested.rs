//! Text another grammar parses inside a host's tree: Markdown's inline text and fenced code
//! (ADR 0111), and HTML's `<script>` and `<style>` elements (ADR 0157). A piece widens to the
//! whole of each node of such text it meets; each text is parsed on its own with its grammar and
//! highlighted over the host's captures; and the blocks of code another grammar painted are
//! kept, so that one that no longer holds such code is highlighted again.
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - Which nodes hold such text, and in which language, is each host's rule: [`super::inline`]
//!   for Markdown, [`super::embedded`] for HTML. This module only follows it.
//! - A nested text is parsed in its node's range less its named children (Markdown's block
//!   continuations), as Markdown's injection query says. It is read from the rope, and its tree
//!   is not kept.
//! - Children are walked with a `TreeCursor`, never by recursion.

use std::cell::Cell;
use std::ops::Range;
use std::time::Instant;

use oso_core::rope::Rope;
use tree_sitter::{Node, ParseOptions, ParseState, Parser, Tree};

use super::highlight::{Piece, lines, run};
use super::{Document, embedded, inline, reader, until};
use crate::language::Language;
use crate::paint::Capture;
use crate::queries::{Host, Kind, Queries};

/// What other grammars parse in a piece, in order: Markdown's inline nodes, and the code
/// (fenced code's content, or an HTML element's text) with the language each is in.
#[derive(Default)]
pub(super) struct Nested<'tree> {
    pub(super) inlines: Vec<Node<'tree>>,
    pub(super) code: Vec<(Node<'tree>, Language)>,
    /// Each piece of code's whole block: a fence with its delimiters and info string, an
    /// element with its tags.
    pub(super) blocks: Vec<Range<usize>>,
}

/// What a host's rule makes of a node its injection query found.
pub(super) enum Found<'tree> {
    /// Markdown's inline text, parsed with the inline grammar.
    Inline(Node<'tree>),
    /// Code: its text, in a language oso parses.
    Code(Node<'tree>, Language),
    /// Nothing another grammar parses here.
    Skip,
}

/// `piece` widened to the whole of each node of `host`'s tree it meets whose text another
/// grammar parses, with those texts; or how the piece went if the query to find them did not
/// run to its end. Such nodes never overlap one another, so the ones the widened piece meets
/// are those.
pub(super) fn widen<'tree>(
    text: &Rope,
    queries: &Queries,
    tree: &'tree Tree,
    (host, piece): (Host, Range<usize>),
    deadline: Instant,
) -> Result<(Range<usize>, Nested<'tree>), Piece> {
    let Some(injections) = queries.get(Kind::Injections(host)) else {
        return Err(Piece::Limit);
    };
    let rule = match host {
        Host::Markdown => inline::found,
        Host::Html => embedded::found,
    };
    let mut widened = piece.clone();
    let mut nested = Nested::default();
    let outcome = run(
        &injections.query,
        tree,
        text,
        piece,
        deadline,
        |node, _, _| {
            match rule(text, node) {
                Found::Code(content, language) => {
                    if nested.code.last() == Some(&(content, language)) {
                        return;
                    }
                    nested.code.push((content, language));
                    nested.blocks.push(node.byte_range());
                }
                Found::Inline(node) => {
                    if nested.inlines.last() == Some(&node) {
                        return;
                    }
                    nested.inlines.push(node);
                }
                Found::Skip => return,
            }
            widened.start = widened.start.min(node.start_byte());
            widened.end = widened.end.max(node.end_byte());
        },
    );
    match outcome {
        Some(outcome) => Err(outcome),
        None => Ok((widened, nested)),
    }
}

impl Document {
    /// Adds the captures of the nested texts of `nodes`, each with its grammar, which `widen`
    /// found, cut to `piece`, to `captures` as of `layer`. `None` when every one ran to its
    /// end, or how the piece went if one did not.
    pub(super) fn highlight_nested(
        &mut self,
        queries: &Queries,
        nodes: Vec<(Node<'_>, Language)>,
        (piece, layer): (Range<usize>, u8),
        deadline: Instant,
        captures: &mut Vec<Capture>,
    ) -> Option<Piece> {
        for (node, language) in nodes {
            // Code whose grammar's query did not compile stays plain, as code of a language
            // oso does not parse does (ADR 0147). The hosts' own are compiled before any piece.
            let Some(prepared) = queries.get(Kind::Highlights(language)) else {
                continue;
            };
            let Some(nested) = self.parse_nested(language, node, deadline) else {
                if Instant::now() >= deadline {
                    return Some(Piece::Stopped);
                }
                continue;
            };
            let range = node.start_byte().max(piece.start)..node.end_byte().min(piece.end);
            let found = |node: Node, pattern, index: u32| {
                if let Some(&Some(class)) = prepared.classes.get(index as usize) {
                    captures.push(Capture {
                        range: node.byte_range(),
                        layer,
                        pattern,
                        class,
                    });
                }
            };
            if let Some(outcome) = run(&prepared.query, &nested, &self.text, range, deadline, found)
            {
                return Some(outcome);
            }
        }
        None
    }

    /// Parses one nested text, `node`'s, with `language` until `deadline`; `None` if it did not
    /// finish, or its ranges could not be set.
    fn parse_nested(&mut self, language: Language, node: Node, deadline: Instant) -> Option<Tree> {
        let ranges = own_ranges(node);
        if ranges.is_empty() {
            return None;
        }
        let at = match self.nested.iter().position(|(made, _)| *made == language) {
            Some(at) => at,
            None => {
                let mut parser = Parser::new();
                parser.set_language(&language.grammar()).ok()?;
                self.nested.push((language, parser));
                self.nested.len() - 1
            }
        };
        let parser = &mut self.nested[at].1;
        parser.set_included_ranges(&ranges).ok()?;
        let stopped = Cell::new(false);
        let mut progress = until::<ParseState>(deadline, &stopped);
        let mut read = reader(&self.text);
        let options = ParseOptions::new().progress_callback(&mut progress);
        let tree = parser.parse_with_options(&mut read, None, Some(options));
        if tree.is_none() {
            // The next node is another text: nothing of this parse is wanted.
            parser.reset();
        }
        tree
    }

    /// Notes that `piece` was painted again, with other grammars over the blocks of code
    /// `blocks`: what such a grammar painted before past the piece's ends was not a block the
    /// piece met whole, so it holds such code no longer, and is to highlight again.
    pub(super) fn repainted(&mut self, piece: &Range<usize>, blocks: &[Range<usize>]) {
        if blocks.is_empty() && self.code_blocks.ranges().is_empty() {
            return;
        }
        // The blocks lie inside the piece, as it widened to each whole.
        let span = self.code_blocks.swap(piece.clone(), blocks);
        let stale = [span.start..piece.start, piece.end..span.end];
        let max_piece = self.max_piece;
        let stale: Vec<_> = stale
            .into_iter()
            .filter(|range| range.start < range.end)
            .map(|range| lines(&self.text, range, max_piece))
            .collect();
        if !stale.is_empty() {
            self.pending.add(stale);
        }
    }
}

/// The text of `range` copied into `buffer`, if it fits: a short name read without allocating.
pub(super) fn short_text<'buffer>(
    text: &Rope,
    range: Range<usize>,
    buffer: &'buffer mut [u8],
) -> Option<&'buffer mut [u8]> {
    if range.len() > buffer.len() {
        return None;
    }
    let mut len = 0;
    for chunk in text.chunks_in_range(range) {
        let slot = buffer.get_mut(len..len + chunk.len())?;
        slot.copy_from_slice(chunk.as_bytes());
        len += chunk.len();
    }
    buffer.get_mut(..len)
}

/// `node`'s range less its named children's, Markdown's block continuations, in order. Its
/// unnamed children are the block grammar's view of delimiters in the text, and stay in. An
/// HTML element's text has no children.
fn own_ranges(node: Node) -> Vec<tree_sitter::Range> {
    let mut ranges = Vec::new();
    let (mut byte, mut point) = (node.start_byte(), node.start_position());
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            if child.is_named() && child.start_byte() > byte {
                ranges.push(tree_sitter::Range {
                    start_byte: byte,
                    end_byte: child.start_byte(),
                    start_point: point,
                    end_point: child.start_position(),
                });
            }
            if child.is_named() {
                (byte, point) = (child.end_byte(), child.end_position());
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    if node.end_byte() > byte {
        ranges.push(tree_sitter::Range {
            start_byte: byte,
            end_byte: node.end_byte(),
            start_point: point,
            end_point: node.end_position(),
        });
    }
    ranges
}
