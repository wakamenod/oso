//! Markdown's fenced code: which fenced code blocks another grammar parses, and the fences a
//! grammar was last painted over, so that one that no longer is such a fence is highlighted
//! again (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and the block tree, and runs
//! only on the worker.
//!
//! - A fence's language is the first word of its info string, the grammar's `language` node:
//!   `rust` and `rs` are Rust's, `toml` TOML's, ignoring ASCII case. Any other, and a fence
//!   with none, stays plain, as does an indented code block. A fence only picks one of the
//!   compiled-in grammars, and nothing reads its content to guess one.
//! - A fence whose content is longer than [`MAX_FENCE_BYTES`] stays plain, so that the piece
//!   that holds it stays small: a piece widens to the whole of each fence it meets, as typing
//!   inside a fence (opening a block comment) can change its highlights to its end, while the
//!   block tree changes only that line.
//! - Children are walked with a `TreeCursor`, never by recursion.

use std::ops::Range;

use oso_core::rope::Rope;
use tree_sitter::Node;

use super::Document;
use super::highlight::lines;
use crate::language::Language;

/// The block grammar's node for a fenced code block.
pub(super) const KIND: &str = "fenced_code_block";

/// The longest fence content highlighted with its language, in bytes: about 400 lines of Rust,
/// parsed and highlighted in under a millisecond (ADR 0111, as built).
pub(crate) const MAX_FENCE_BYTES: usize = 16 << 10;

/// The longest language name read from an info string: longer ones name no grammar here.
const MAX_NAME_BYTES: usize = 16;

/// The content of `node`, if it is a fenced code block whose info string names a language oso
/// injects and whose content is short enough, with that language.
pub(super) fn injected<'tree>(text: &Rope, node: Node<'tree>) -> Option<(Node<'tree>, Language)> {
    if node.kind() != KIND {
        return None;
    }
    let (mut language, mut content) = (None, None);
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            match child.kind() {
                "info_string" => language = child.named_child(0).and_then(|name| named(text, name)),
                "code_fence_content" => content = Some(child),
                _ => {}
            }
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
    let content = content.filter(|content| content.byte_range().len() <= MAX_FENCE_BYTES)?;
    Some((content, language?))
}

/// The language `node`, an info string's first word, names, if oso injects it.
fn named(text: &Rope, node: Node) -> Option<Language> {
    let range = node.byte_range();
    if node.kind() != "language" || range.len() > MAX_NAME_BYTES {
        return None;
    }
    let name: String = text.chunks_in_range(range).collect();
    match name.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some(Language::Rust),
        "toml" => Some(Language::Toml),
        _ => None,
    }
}

impl Document {
    /// Notes that `piece` was painted again, with fences' grammars over the fences `blocks`:
    /// what such a grammar painted before past the piece's ends was not a fence the piece met
    /// whole, so it is one no longer, and is to highlight again.
    pub(super) fn repainted(&mut self, piece: &Range<usize>, blocks: &[Range<usize>]) {
        let injected = self.injected.ranges();
        let first = injected.partition_point(|range| range.end <= piece.start);
        let last = injected.partition_point(|range| range.start < piece.end);
        let mut span = piece.clone();
        if first < last {
            span.start = span.start.min(injected[first].start);
            span.end = span.end.max(injected[last - 1].end);
        }
        self.injected.remove(span.clone());
        self.injected.add(blocks.iter().cloned());
        let stale = [span.start..piece.start, piece.end..span.end];
        let max_piece = self.max_piece;
        let stale: Vec<_> = stale
            .into_iter()
            .filter(|range| range.start < range.end)
            .map(|range| lines(&self.text, range, max_piece))
            .collect();
        self.pending.add(stale);
    }
}
