//! Markdown's inline text: each `inline` node of the block tree in a piece, parsed on its own
//! with the inline grammar and highlighted over its block's captures (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and splices, and runs only on
//! the worker.
//!
//! - An inline node is parsed in its own range less its named children, the block
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

use super::highlight::{Piece, run};
use super::{Document, reader, until};
use crate::language::Language;
use crate::paint::Capture;
use crate::queries::{Kind, Queries};

/// The longest inline node parsed, in bytes. A 16 KiB node of the worst nesting found needs
/// about 512 KiB of stack, and step 4 gives the worker 8 MiB (ADR 0111, as built).
pub(crate) const MAX_INLINE_BYTES: usize = 16 << 10;

/// `piece` widened to the whole of each inline node it meets that is short enough to parse,
/// with those nodes, in order; or how the piece went if the query to find them did not run to
/// its end. Inline nodes never overlap, so the nodes the widened piece meets are those.
pub(super) fn widen<'tree>(
    text: &Rope,
    queries: &Queries,
    tree: &'tree Tree,
    piece: Range<usize>,
    deadline: Instant,
) -> Result<(Range<usize>, Vec<Node<'tree>>), Piece> {
    let Some(inlines) = queries.get(Kind::Inlines) else {
        return Err(Piece::Limit);
    };
    let mut widened = piece.clone();
    let mut nodes: Vec<Node> = Vec::new();
    let outcome = run(&inlines.query, tree, text, piece, deadline, |node, _, _| {
        if node.byte_range().len() <= MAX_INLINE_BYTES && nodes.last() != Some(&node) {
            widened.start = widened.start.min(node.start_byte());
            widened.end = widened.end.max(node.end_byte());
            nodes.push(node);
        }
    });
    match outcome {
        Some(outcome) => Err(outcome),
        None => Ok((widened, nodes)),
    }
}

impl Document {
    /// Adds the captures of the inline text of `nodes`, the inline nodes `widen` found, cut to
    /// `piece`, to `captures`. `None` when every inline node ran to its end, or how the piece
    /// went if one did not.
    pub(super) fn highlight_inlines(
        &mut self,
        queries: &Queries,
        nodes: &[Node],
        piece: Range<usize>,
        deadline: Instant,
        captures: &mut Vec<Capture>,
    ) -> Option<Piece> {
        if nodes.is_empty() {
            return None;
        }
        let Some(prepared) = queries.get(Kind::Highlights(Language::MarkdownInline)) else {
            return Some(Piece::Limit);
        };
        for &node in nodes {
            let Some(inline) = self.parse_inline(node, deadline) else {
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
                        layer: 1,
                        pattern,
                        class,
                    });
                }
            };
            if let Some(outcome) = run(&prepared.query, &inline, &self.text, range, deadline, found)
            {
                return Some(outcome);
            }
        }
        None
    }

    /// Parses one inline node's text until `deadline`; `None` if it did not finish, or its
    /// ranges could not be set.
    fn parse_inline(&mut self, node: Node, deadline: Instant) -> Option<Tree> {
        let ranges = own_ranges(node);
        if ranges.is_empty() {
            return None;
        }
        let parser = match &mut self.inline {
            Some(parser) => parser,
            None => {
                let mut parser = Parser::new();
                parser
                    .set_language(&Language::MarkdownInline.grammar())
                    .ok()?;
                self.inline.insert(parser)
            }
        };
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
