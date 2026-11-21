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
    /// A line a review's diff puts in, with its `+` (ADR 0122), or a git status's hunk (ADR
    /// 0127). Only the host's review view and status give these and the review faces below; no
    /// parser does.
    DiffAdded,
    /// A line a review's diff takes out, with its `-`.
    DiffRemoved,
    /// A file's header in a review.
    DiffFileHeader,
    /// A hunk's header in a review, `@@ -40,3 +40,7 @@`.
    DiffHunkHeader,
    /// The words of an added line that refinement marks, in a git status (ADR 0127, step 1c):
    /// drawn on the refined background, as a review's are.
    DiffRefineAdded,
    /// The words of a removed line that refinement marks.
    DiffRefineRemoved,
    /// The label of the person's comment in a review.
    CommentPerson,
    /// The label of an agent's comment in a review.
    CommentAgent,
    /// A comment whose line went, or what says so.
    Outdated,
    /// A comment of the person's sent to the agent, and not changed since: its bar, label and
    /// text, dimmed (ADR 0122, step 12).
    Sent,
    /// What says a hunk was accepted, rejected or viewed, and a file's count of them (ADR 0122,
    /// step 5).
    Decided,
    /// A session's mark that needs the person: needs input or failed (ADR 0131). Only the
    /// host's `*Spaces*` gives these; no parser does.
    SessionAlert,
    /// A waiting session's mark and name on the lit beat of its blink, drawn inverse on a fill
    /// in the alert's colour (ADR 0140, as amended): only the host's `*Spaces*` gives these.
    SessionAlertLit,
    /// A working session's mark.
    SessionWorking,
    /// The mark of a session done or ready for review.
    SessionDone,
    /// What is said quietly in `*Spaces*`: an idle or stopped session's mark, a place, a
    /// reason, and a whole row of a session run outside oso.
    SessionQuiet,
    /// The name of the worktree whose space is in front.
    SessionFront,
    /// A space's name in `*Spaces*` (ADR 0140).
    SessionSpace,
    /// A repository's name in `*Spaces*` (ADR 0140).
    SessionRepository,
    /// A directory listing's header line, its path (ADR 0134). Only the host's listings give
    /// these seven; no parser does.
    DiredHeader,
    /// A directory's name, or a link's that leads to one.
    DiredDirectory,
    /// A link's name.
    DiredSymlink,
    /// The name of a link that leads nowhere.
    DiredBrokenSymlink,
    /// The name of a file someone may run.
    DiredExecutable,
    /// The name of a pipe, a socket or a device.
    DiredSpecial,
    /// The mode, owner, size and time of a row, said quietly.
    DiredDetails,
    /// A row's mark, `*`.
    DiredMark,
    /// The name of a marked row.
    DiredMarked,
    /// A git status section's heading, such as `Unstaged changes (2)` (ADR 0127). Only the
    /// host's status buffers give these five; no parser does.
    GitSectionHeading,
    /// A local branch's name.
    GitBranchLocal,
    /// A remote branch's name, such as an upstream.
    GitBranchRemote,
    /// A commit's short id.
    GitHash,
    /// What a status says quietly: a file's counts, ahead and behind, notes and marks.
    GitDetail,
    /// What a commit's message holds past what its line should (ADR 0127, step 1b): the
    /// summary's characters past 50, a body line's past 72, and a second line that is not
    /// empty. Only the host gives these; no parser does.
    GitCommitOverlong,
    /// A graph's lanes (ADR 0127, step 4), coloured in turn: the lane at index `i` takes the
    /// `i % 8`th. Only the host's graph buffers give these.
    GitLane0,
    GitLane1,
    GitLane2,
    GitLane3,
    GitLane4,
    GitLane5,
    GitLane6,
    GitLane7,
    /// A tag's name on a graph's row.
    GitTag,
}
