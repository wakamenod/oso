//! Markdown's nested text: each `inline` node of the block tree in a piece, parsed on its own
//! with the inline grammar, and each fenced code block whose info string names a language,
//! parsed with that language's grammar ([`super::fence`]), all highlighted over their block's
//! captures (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - A nested text is parsed in its node's range less its named children, the block
//!   continuations, as the grammar's injection query says. It is read from the rope, and its
//!   tree is not kept.
//! - An inline node longer than [`MAX_INLINE_BYTES`] stays plain: the inline grammar recurses as
//!   links nest, and deeply nested inline links overflow a thread's stack, which aborts the
//!   process (ADR 0111, as built).
//! - Children are walked with a `TreeCursor`, never by recursion.

use std::cell::Cell;
use std::ops::Range;
use std::time::Instant;

use oso_core::rope::Rope;
use tree_sitter::{Node, ParseOptions, ParseState, Parser, Tree};

use super::fence;
use super::highlight::{Piece, run};
use super::{Document, reader, until};
use crate::language::Language;
use crate::paint::Capture;
use crate::queries::{Kind, Queries};

/// The longest inline node parsed, in bytes. A 16 KiB node of the worst nesting found needs
/// about 512 KiB of stack, and step 4 gives the worker 8 MiB (ADR 0111, as built).
pub(crate) const MAX_INLINE_BYTES: usize = 16 << 10;

/// What other grammars parse in a Markdown piece, in order: the inline nodes, and the
/// fenced code blocks' contents with the language each names.
#[derive(Default)]
pub(super) struct Nested<'tree> {
    pub(super) inlines: Vec<Node<'tree>>,
    pub(super) fences: Vec<(Node<'tree>, Language)>,
    /// Each fence's whole block, delimiters and info string too.
    pub(super) blocks: Vec<Range<usize>>,
}

/// `piece` widened to the whole of each inline node it meets that is short enough to parse,
/// and of each fenced code block whose content its language parses, with what they hold; or
/// how the piece went if the query to find them did not run to its end. Neither kind
/// overlaps another, so the nodes the widened piece meets are those.
pub(super) fn widen<'tree>(
    text: &Rope,
    queries: &Queries,
    tree: &'tree Tree,
    piece: Range<usize>,
    deadline: Instant,
) -> Result<(Range<usize>, Nested<'tree>), Piece> {
    let Some(injections) = queries.get(Kind::Injections) else {
        return Err(Piece::Limit);
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
            if let Some(fence) = fence::injected(text, node) {
                if nested.fences.last() == Some(&fence) {
                    return;
                }
                nested.fences.push(fence);
                nested.blocks.push(node.byte_range());
            } else if node.kind() == fence::KIND
                || node.byte_range().len() > MAX_INLINE_BYTES
                || nested.inlines.last() == Some(&node)
            {
                // A fence no grammar parses here, or an inline node too long or already found.
                return;
            } else {
                nested.inlines.push(node);
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
            let Some(prepared) = queries.get(Kind::Highlights(language)) else {
                return Some(Piece::Limit);
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
}

/// `node`'s range less its named children's, the block continuations, in order. Its unnamed
/// children are the block grammar's view of delimiters in the text, and stay in.
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
