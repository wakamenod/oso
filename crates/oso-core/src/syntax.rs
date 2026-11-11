//! What syntax highlighting shares with the editor: the edits a parser follows (ADR 0111).
//! The parsers themselves are in `oso-syntax`, which only the tip crate uses.
//!
//! Must not depend on: `buffer`, `editor` or `display_map`; it sees only offsets.

/// One replacement in a batch of edits: `old_len` bytes at `start` became `new_len` bytes.
///
/// A batch is what one command did to a buffer. Its splices are ascending and apart, and every
/// offset in them is in the text before the batch, as the cursors follow them (ADR 0053).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Splice {
    /// Where the replaced text starts, in bytes.
    pub start: usize,
    /// How many bytes were replaced.
    pub old_len: usize,
    /// How many bytes replaced them.
    pub new_len: usize,
}
