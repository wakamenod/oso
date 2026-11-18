//! Markdown's nested text: which of the nodes Markdown's injection query finds the inline
//! grammar parses, and which are fenced code another grammar parses ([`super::fence`]); the
//! parsing and painting are [`super::nested`]'s (ADR 0111).
//!
//! Must not depend on: buffers, windows or faces; it sees a rope and the block tree, and runs
//! only on the worker.
//!
//! - An inline node longer than [`MAX_INLINE_BYTES`] stays plain: the inline grammar recurses as
//!   links nest, and deeply nested inline links overflow a thread's stack, which aborts the
//!   process (ADR 0111, as built).

use oso_core::rope::Rope;
use tree_sitter::Node;

use super::fence;
use super::nested::Found;

/// The longest inline node parsed, in bytes. A 16 KiB node of the worst nesting found needs
/// about 512 KiB of stack, and step 4 gives the worker 8 MiB (ADR 0111, as built).
pub(crate) const MAX_INLINE_BYTES: usize = 16 << 10;

/// What another grammar parses of `node`, which Markdown's injection query found: a fence's
/// code, an inline node short enough, or nothing.
pub(super) fn found<'tree>(text: &Rope, node: Node<'tree>) -> Found<'tree> {
    if let Some((content, language)) = fence::injectable(text, node) {
        Found::Code(content, language)
    } else if node.kind() == fence::KIND || node.byte_range().len() > MAX_INLINE_BYTES {
        // A fence no grammar parses here, or an inline node too long.
        Found::Skip
    } else {
        Found::Inline(node)
    }
}
