//! A file's comments as cards in a review buffer (ADR 0122, "The views"): where each thread
//! goes among a section's rows, and the rows of its label and its text.
//!
//! Must not depend on: `app`, `dispatch`, git or the file system; this lays out a snapshot of
//! the model's comments it is given, on the review's view thread.
//!
//! - Every card row starts with four spaces and a bar in its author's face. The label is drawn
//!   here; the text goes under it two columns further in, one row per line, so that no text,
//!   an agent's included, makes a row of its own or stands where a label stands.
//! - What a row is goes in the section's rows, a label as `RowKind::Label` and text as
//!   `RowKind::Card`, never read back from the text.
//! - A thread goes after the row of its line, or after its hunk's last line, or above the first
//!   hunk when it is outdated. Its replies follow their parent in id order, two columns further
//!   in per depth, at most three deep. Threads are walked with a stack of their own, so a long
//!   chain of replies costs no call stack.
//! - Past the review's text limit a card shows its label alone.
//! - Nothing here logs a comment's text.

use std::collections::HashMap;
use std::sync::Arc;

use oso_core::review::{Anchor, Author, Comment, CommentId, RowKind, Shape, Side};
use oso_core::syntax::Highlight;

use super::section::Writer;

/// The deepest a reply is drawn: deeper ones are drawn at this depth.
const MAX_DEPTH: usize = 3;

/// What starts every card row: four spaces, then the bar in the author's face.
const MARGIN: &[u8] = b"    ";
const BAR: &str = "\u{258d}";

/// What stands for the lines of a folded agent card after its first.
const MORE: &str = "\u{2026}";

/// A file's comments as the model last said them, and the model's reading of the file then.
#[derive(Clone, Default)]
pub(crate) struct Cards {
    pub(crate) comments: Vec<Comment>,
    pub(crate) reading: Option<Arc<Shape>>,
}

impl Cards {
    /// Whether an agent wrote any of them, so that folding agent cards changes the file.
    pub(crate) fn any_agent(&self) -> bool {
        self.comments
            .iter()
            .any(|comment| comment.author() == Author::Agent)
    }
}

/// What the comments are placed by.
#[derive(Clone, Copy)]
pub(crate) enum Placing<'a> {
    /// The snapshot is of the reading shown: each comment's own anchor, unless it is outdated.
    Model,
    /// The result of carrying the comments to the reading shown, which no snapshot has seen
    /// yet; a comment it does not name was added since, and is outdated.
    Placed(&'a [(CommentId, Option<Anchor>)]),
    /// The snapshot is of another reading, or no reading is shown: every comment is outdated.
    Outdated,
}

impl<'a> Placing<'a> {
    /// How to place `cards` on the reading `shown`, given where the last carry to it placed
    /// them.
    pub(crate) fn of(
        shown: Option<&Arc<Shape>>,
        cards: &Cards,
        placed: Option<&'a [(CommentId, Option<Anchor>)]>,
    ) -> Placing<'a> {
        let Some(shown) = shown else {
            return Placing::Outdated;
        };
        if cards
            .reading
            .as_ref()
            .is_some_and(|reading| Arc::ptr_eq(reading, shown))
        {
            return Placing::Model;
        }
        match placed {
            Some(placed) => Placing::Placed(placed),
            None => Placing::Outdated,
        }
    }
}

/// The header's count of a file's comments, such as `  · 3 comments (1 agent, 1 unread)`, or
/// `None` for none.
pub(super) fn count(comments: &[Comment]) -> Option<String> {
    if comments.is_empty() {
        return None;
    }
    let all = comments.len();
    let agent = comments
        .iter()
        .filter(|comment| comment.author() == Author::Agent)
        .count();
    let unread = comments.iter().filter(|comment| !comment.is_read()).count();
    let noun = if all == 1 { "comment" } else { "comments" };
    let mut text = format!("  \u{b7} {all} {noun}");
    match (agent, unread) {
        (0, 0) => {}
        (agent, 0) => text.push_str(&format!(" ({agent} agent)")),
        (agent, unread) => text.push_str(&format!(" ({agent} agent, {unread} unread)")),
    }
    Some(text)
}

/// Where each thread of a file goes, each by the index of its first card in the snapshot, and
/// how its cards are drawn.
pub(super) struct Plan<'a> {
    comments: &'a [Comment],
    /// Each card's replies, by index, in id order.
    replies: HashMap<CommentId, Vec<usize>>,
    /// The outdated threads, in id order.
    outdated: Vec<usize>,
    lines: HashMap<(Side, u32), Vec<usize>>,
    hunks: HashMap<usize, Vec<usize>>,
    /// Whether agent cards fold to their first line.
    folded: bool,
    /// How many bytes the section may hold before cards show their label alone.
    room: usize,
}

impl<'a> Plan<'a> {
    /// Places `comments` on `shape`, the reading shown, if any, by `placing`. A place `shape`
    /// does not show makes a thread outdated.
    pub(super) fn new(
        comments: &'a [Comment],
        shape: Option<&Shape>,
        placing: Placing<'_>,
        folded: bool,
        room: usize,
    ) -> Plan<'a> {
        let index: HashMap<CommentId, usize> = comments
            .iter()
            .enumerate()
            .map(|(at, comment)| (comment.id(), at))
            .collect();
        let placed: HashMap<CommentId, Option<Anchor>> = match placing {
            Placing::Placed(placed) => placed.iter().copied().collect(),
            _ => HashMap::new(),
        };
        let mut plan = Plan {
            comments,
            replies: HashMap::new(),
            outdated: Vec::new(),
            lines: HashMap::new(),
            hunks: HashMap::new(),
            folded,
            room,
        };
        let mut roots = Vec::new();
        for (at, comment) in comments.iter().enumerate() {
            // A parent comes before its replies, so a cycle cannot be followed.
            match comment
                .reply_to()
                .filter(|parent| *parent < comment.id() && index.contains_key(parent))
            {
                Some(parent) => plan.replies.entry(parent).or_default().push(at),
                None => roots.push(at),
            }
        }
        let by_id = |at: &usize| comments[*at].id();
        for replies in plan.replies.values_mut() {
            replies.sort_unstable_by_key(by_id);
        }
        roots.sort_unstable_by_key(by_id);
        for root in roots {
            let comment = &comments[root];
            let anchor = match placing {
                Placing::Model => (!comment.is_outdated()).then(|| comment.anchor()),
                Placing::Placed(_) => placed.get(&comment.id()).copied().flatten(),
                Placing::Outdated => None,
            };
            match (anchor, shape) {
                (Some(Anchor::Line { side, number }), Some(shape))
                    if shape.line(side, number).is_some() =>
                {
                    plan.lines.entry((side, number)).or_default().push(root);
                }
                (Some(Anchor::Hunk { index }), Some(shape)) if index < shape.hunks().len() => {
                    plan.hunks.entry(index).or_default().push(root);
                }
                _ => plan.outdated.push(root),
            }
        }
        plan
    }

    /// Writes the outdated threads, above the first hunk.
    pub(super) fn outdated(&mut self, writer: &mut Writer) {
        for root in std::mem::take(&mut self.outdated) {
            self.thread(writer, root, true);
        }
    }

    /// Writes the threads on line `number` of `side`, after its row.
    pub(super) fn after_line(&mut self, writer: &mut Writer, side: Side, number: u32) {
        if let Some(roots) = self.lines.remove(&(side, number)) {
            for root in roots {
                self.thread(writer, root, false);
            }
        }
    }

    /// Writes the threads on hunk `index`, after its last line.
    pub(super) fn after_hunk(&mut self, writer: &mut Writer, index: usize) {
        if let Some(roots) = self.hunks.remove(&index) {
            for root in roots {
                self.thread(writer, root, false);
            }
        }
    }

    /// Writes every thread not written yet, the outdated first, then the rest in id order: for
    /// a section that shows no hunk.
    pub(super) fn rest(&mut self, writer: &mut Writer) {
        self.outdated(writer);
        let mut roots: Vec<usize> = self
            .lines
            .drain()
            .flat_map(|(_, roots)| roots)
            .chain(self.hunks.drain().flat_map(|(_, roots)| roots))
            .collect();
        roots.sort_unstable_by_key(|at| self.comments[*at].id());
        for root in roots {
            self.thread(writer, root, false);
        }
    }

    /// Writes the thread from `root`: each card, then its replies. A resolved card folds to its
    /// label, which says how many replies it hides.
    fn thread(&self, writer: &mut Writer, root: usize, outdated: bool) {
        let mut stack = vec![(root, 0)];
        while let Some((at, depth)) = stack.pop() {
            let comment = &self.comments[at];
            let label = Label {
                outdated: outdated && at == root,
                resolved: comment.is_resolved().then(|| self.replies_under(at)),
            };
            self.card(writer, comment, depth.min(MAX_DEPTH), &label);
            if comment.is_resolved() {
                continue;
            }
            if let Some(replies) = self.replies.get(&comment.id()) {
                stack.extend(replies.iter().rev().map(|&reply| (reply, depth + 1)));
            }
        }
    }

    /// How many replies are under the card at `at`, at any depth.
    fn replies_under(&self, at: usize) -> usize {
        let mut count = 0;
        let mut stack = vec![at];
        while let Some(at) = stack.pop() {
            if let Some(replies) = self.replies.get(&self.comments[at].id()) {
                count += replies.len();
                stack.extend(replies);
            }
        }
        count
    }

    /// Writes one card at `depth`: its label, then its text, unless the section is past its
    /// room.
    fn card(&self, writer: &mut Writer, comment: &Comment, depth: usize, label: &Label) {
        let face = Some(match comment.author() {
            Author::Person => Highlight::CommentPerson,
            Author::Agent => Highlight::CommentAgent,
        });
        let short = writer.len() >= self.room;
        writer.cut |= short;
        let indent = "  ".repeat(depth);
        let kind = RowKind::Label {
            comment: comment.id(),
        };
        let mut head = match comment.author() {
            Author::Person => "You".to_owned(),
            Author::Agent => "Agent".to_owned(),
        };
        head.push_str(&format!("  #{}", comment.id().get()));
        if !comment.is_read() {
            head.push_str(" \u{b7} new");
        }
        let was = label
            .outdated
            .then(|| format!(" \u{b7} outdated, was {}", was(comment.anchor())));
        let resolved = label.resolved.map(|replies| match replies {
            0 => " \u{b7} resolved".to_owned(),
            1 => " \u{b7} resolved \u{b7} 1 reply".to_owned(),
            n => format!(" \u{b7} resolved \u{b7} {n} replies"),
        });
        writer.line(
            kind,
            &[
                (MARGIN, None),
                (BAR.as_bytes(), face),
                (indent.as_bytes(), None),
                (head.as_bytes(), face),
                (
                    was.as_deref().unwrap_or("").as_bytes(),
                    Some(Highlight::Outdated),
                ),
                (resolved.as_deref().unwrap_or("").as_bytes(), face),
            ],
        );
        if short || label.resolved.is_some() {
            return;
        }
        let kind = RowKind::Card {
            comment: comment.id(),
        };
        let indent = format!("{indent}  ");
        let mut text = lines(comment.text());
        let shown = if self.folded && comment.author() == Author::Agent {
            1
        } else {
            usize::MAX
        };
        let mut row = |line: &str| {
            writer.line(
                kind,
                &[
                    (MARGIN, None),
                    (BAR.as_bytes(), face),
                    (indent.as_bytes(), None),
                    (line.as_bytes(), None),
                ],
            );
        };
        for line in text.by_ref().take(shown) {
            row(line);
        }
        if text.next().is_some() {
            row(MORE);
        }
    }
}

/// What a card's label says besides its author and number.
struct Label {
    /// Whether it heads an outdated thread.
    outdated: bool,
    /// The replies a resolved card folds away.
    resolved: Option<usize>,
}

/// Where an outdated comment was, as its label says it.
fn was(anchor: Anchor) -> String {
    match anchor {
        Anchor::Line { side, number } => {
            let side = match side {
                Side::Old => "old",
                Side::New => "new",
            };
            format!("L{number} ({side})")
        }
        Anchor::Hunk { index } => format!("hunk {}", index.saturating_add(1)),
    }
}

/// The lines of a comment's text, split at `\n`, `\r\n` and a lone `\r`; a text ending in a
/// line break has no empty line after it, and an empty text has none.
fn lines(text: &str) -> Lines<'_> {
    Lines { rest: text }
}

struct Lines<'a> {
    rest: &'a str,
}

impl<'a> Iterator for Lines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let rest = self.rest;
        if rest.is_empty() {
            return None;
        }
        match rest.find(['\n', '\r']) {
            None => {
                self.rest = "";
                Some(rest)
            }
            Some(at) => {
                let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
                self.rest = &rest[at + skip..];
                Some(&rest[..at])
            }
        }
    }
}

#[cfg(test)]
#[path = "cards_tests.rs"]
pub(super) mod tests;
