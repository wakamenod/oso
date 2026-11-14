//! Comments on a review (ADR 0122, "Comments and re-anchoring"): who wrote one, where it is,
//! its text and its state, and what each author may do with it (Q11).
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use std::sync::Arc;

use super::shape::{Shape, Side};

/// The most comments a review holds.
pub const MAX_COMMENTS: usize = 10_000;

/// The most bytes of a comment's text.
pub const MAX_COMMENT_BYTES: usize = 64 * 1024;

/// The most bytes of a commented line's text a saved review keeps; past them, its length and a
/// hash of the rest stand for it. In memory a comment keeps the reading its line is in, and its
/// line is compared whole.
pub const MAX_KEPT_LINE: usize = 4 * 1024;

/// A comment's id, never reused within a review.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommentId(pub(super) u64);

impl CommentId {
    /// The id as a number.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Who wrote a comment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Author {
    Person,
    Agent,
}

/// Where a comment is in the reading it was last placed in: a line of a side, or a whole hunk
/// by its index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Line { side: Side, number: u32 },
    Hunk { index: usize },
}

/// Why a change to the comments was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommentError {
    /// The review has no reading of that path.
    NoSuchFile,
    /// The line or hunk is not one the diff shows.
    NotInDiff,
    /// No comment has that id.
    NoSuchComment,
    /// The comment is another author's.
    NotYours,
    /// The text is longer than [`MAX_COMMENT_BYTES`].
    TooLong,
    /// The review holds [`MAX_COMMENTS`] already.
    TooMany,
    /// A reply to the comment is one this author may not remove.
    RepliesNotYours,
}

/// A comment, as the review keeps it.
#[derive(Clone, Debug)]
pub struct Comment {
    pub(super) id: CommentId,
    pub(super) author: Author,
    /// Shared, so that a copy of the comment for the review view costs a count.
    pub(super) path: Arc<[u8]>,
    pub(super) anchor: Anchor,
    /// The reading the anchor is in: the one the comment was last placed in. `None` once that
    /// reading was let go: the comment then stays outdated for good.
    pub(super) shape: Option<Arc<Shape>>,
    /// The count of the file's readings when it was last placed, to tell old readings apart.
    pub(super) reading: u64,
    pub(super) text: Arc<str>,
    pub(super) reply_to: Option<CommentId>,
    pub(super) sent: bool,
    pub(super) resolved: bool,
    pub(super) outdated: bool,
    pub(super) read: bool,
}

impl Comment {
    pub fn id(&self) -> CommentId {
        self.id
    }

    pub fn author(&self) -> Author {
        self.author
    }

    /// The file's path, as git gives it.
    pub fn path(&self) -> &[u8] {
        &self.path
    }

    /// Where the comment is: in the current reading, or, when it is outdated, in the reading it
    /// was last placed in.
    pub fn anchor(&self) -> Anchor {
        self.anchor
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The comment this one replies to.
    pub fn reply_to(&self) -> Option<CommentId> {
        self.reply_to
    }

    /// Whether it was sent to the agent, and not edited since.
    pub fn is_sent(&self) -> bool {
        self.sent
    }

    pub fn is_resolved(&self) -> bool {
        self.resolved
    }

    /// Whether its line or hunk is not in the current reading.
    pub fn is_outdated(&self) -> bool {
        self.outdated
    }

    /// Whether the person has read it: always for the person's own.
    pub fn is_read(&self) -> bool {
        self.read
    }
}
