//! What syntax highlighting shares with the editor: the edits a parser follows, and a buffer's
//! highlights, a map of runs that follows edits until the next report (ADR 0111). The parsers
//! themselves are in `oso-syntax`, which only the tip crate uses.
//!
//! Must not depend on: `buffer`, `editor` or `display_map`; it sees only offsets.

mod compose;
mod highlights;

pub use compose::compose;
pub use highlights::{Highlights, Runs};

/// The largest text highlighted: a larger buffer stays plain, and keeps no log of its edits. A
/// tree costs 35 to 60 times its text (ADR 0111, as built).
pub const MAX_BYTES: usize = 2 << 20;

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

/// What a range of text is, as highlighting tells it. It names no colour: the renderer gives
/// each a face, and a theme its colours (ADR 0111).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Highlight {
    /// A comment, and a doc comment.
    Comment,
    /// A string or a character, with its escapes.
    String,
    /// A keyword, or a label.
    Keyword,
    /// A type, or a constructor.
    Type,
    /// A function or a method.
    Function,
    /// A constant, a number or a boolean.
    Constant,
    /// A field, or a key in TOML.
    Property,
    /// A macro or an attribute.
    Macro,
    /// A heading in Markdown.
    Heading,
    /// A link or a reference in Markdown.
    Link,
    /// Emphasis and strong text in Markdown, and its markers: of headings, lists and quotes,
    /// and thematic breaks.
    Markup,
}
