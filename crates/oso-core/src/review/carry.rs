//! Carrying a file's comments and marks to a new reading (ADR 0122, "Refresh while the agent
//! edits"), in three parts so that the matching runs off the UI thread: [`Carry`] is a snapshot
//! taken from the review, [`Carry::run`] places each comment and carries the marks on a worker,
//! and the review applies the [`Carried`] result by id.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.
//!
//! A line comment moves only to a line whose text is its own, compared whole with its line in
//! the reading it keeps. A hunk comment carries each line of its hunk, each checked the same
//! way; if half or more went, it is outdated, else the hunk most of them are in wins, the first
//! of those on a tie. An outdated comment keeps the reading it was last placed in and is carried
//! from it each time, so it comes back when its line does. Comments with one anchor in one reading, a thread's replies among
//! them, are placed once.

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::sync::Arc;

use super::comment::{Anchor, CommentId};
use super::decision::{self, Mark, Marked};
use super::map::{BaseHunk, LineMap, Spans, base_pair};
use super::shape::{Shape, Side};

/// A file's comments to carry to a new reading, taken from the review.
#[derive(Clone, Debug)]
pub struct Carry {
    pub(super) path: Box<[u8]>,
    pub(super) to: Arc<Shape>,
    pub(super) items: Vec<Item>,
    /// The current reading, its hunks' hashes and their marks, when any hunk is marked.
    pub(super) marked: Option<MarkedReading>,
    pub(super) hasher: RandomState,
    pub(super) readings: u64,
    pub(super) added: u64,
    pub(super) remarked: u64,
}

/// One comment to carry: where it is, and in which reading.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub(super) id: CommentId,
    pub(super) from: Arc<Shape>,
    pub(super) anchor: Anchor,
}

/// Where each comment of a [`Carry`] goes, for the review to apply.
#[derive(Clone, Debug)]
pub struct Carried {
    pub(super) path: Box<[u8]>,
    pub(super) to: Arc<Shape>,
    pub(super) placed: Vec<(CommentId, Option<Anchor>)>,
    pub(super) hashes: Arc<[u64]>,
    pub(super) marks: Vec<Mark>,
    pub(super) dropped: usize,
    pub(super) readings: u64,
    pub(super) added: u64,
    pub(super) remarked: u64,
}

/// A reading, its hunks' hashes and their marks.
pub(super) type MarkedReading = (Arc<Shape>, Arc<[u64]>, Vec<Mark>);

/// Two old blobs, by id: the first carried from, the second to.
type BlobPair = (Box<[u8]>, Box<[u8]>);

/// An anchor as a key, by the reading it is in.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Line(Side, u32),
    Hunk(usize),
}

/// What applying a [`Carried`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Comments placed in the new reading, the returned ones included.
    pub placed: usize,
    /// Comments that went outdated with it.
    pub outdated: usize,
    /// Outdated comments that came back.
    pub returned: usize,
    /// Decisions and viewed marks dropped because their hunk's text went.
    pub dropped: usize,
    /// Whether comments were added to the file since the snapshot was taken, or another
    /// reading applied, so it must be carried again. Comments added meanwhile are outdated
    /// until then, and not counted in `outdated`.
    pub again: bool,
    /// Whether decisions or viewed marks changed since the snapshot was taken. The current
    /// ones were carried in its place: nothing needs carrying again.
    pub marks_changed: bool,
}

impl Carry {
    /// Places every comment in the new reading and carries the marks. `base` is asked for the
    /// hunks between two old blobs ([`base_pair`]), at most once per pair, and answers `None`
    /// when they cannot be compared; `lcs` is the matcher of [`LineMap::new`], which must bound
    /// its own work.
    pub fn run<B, F>(self, mut base: B, mut lcs: F) -> Carried
    where
        B: FnMut(&[u8], &[u8]) -> Option<Vec<BaseHunk>>,
        F: FnMut(&[&[u8]], &[&[u8]]) -> Vec<(usize, usize)>,
    {
        let to = &*self.to;
        let to_spans = Arc::new(Spans::of(to));
        let mut bases: HashMap<BlobPair, Option<Vec<BaseHunk>>> = HashMap::new();
        // One map per reading carried from, and one place per anchor in it.
        let mut maps: HashMap<*const Shape, LineMap> = HashMap::new();
        let mut places: HashMap<(*const Shape, Key), Option<Anchor>> = HashMap::new();
        let mut placed = Vec::with_capacity(self.items.len());
        for item in &self.items {
            let from = &*item.from;
            let key = match item.anchor {
                Anchor::Line { side, number } => Key::Line(side, number),
                Anchor::Hunk { index } => Key::Hunk(index),
            };
            let place_key = (Arc::as_ptr(&item.from), key);
            let at = *places.entry(place_key).or_insert_with(|| {
                let map = maps.entry(Arc::as_ptr(&item.from)).or_insert_with(|| {
                    let hunks = base_pair(from, to).and_then(|(a, b)| {
                        bases
                            .entry((a.into(), b.into()))
                            .or_insert_with(|| base(a, b))
                            .as_deref()
                    });
                    LineMap::onto(from, to, &to_spans, hunks, &mut lcs)
                });
                place(from, item.anchor, map, to)
            });
            placed.push((item.id, at));
        }
        let hashes: Arc<[u64]> = to
            .hunks()
            .iter()
            .map(|hunk| decision::hash(&self.hasher, hunk))
            .collect();
        let (marks, dropped) = match &self.marked {
            Some((shape, from_hashes, marks)) => {
                let from = Marked {
                    hunks: shape.hunks(),
                    hashes: from_hashes,
                    marks,
                };
                decision::carry_marks(&from, to.hunks(), &hashes)
            }
            None => (vec![Mark::default(); to.hunks().len()], 0),
        };
        Carried {
            path: self.path,
            to: self.to,
            placed,
            hashes,
            marks,
            dropped,
            readings: self.readings,
            added: self.added,
            remarked: self.remarked,
        }
    }
}

/// Where `anchor`, in the reading `from`, is in the reading `to`, by `map`, or `None` when it
/// is outdated there.
fn place(from: &Shape, anchor: Anchor, map: &LineMap, to: &Shape) -> Option<Anchor> {
    match anchor {
        Anchor::Line { side, number } => {
            let there = map.carry(side, number)?;
            let same = from.line(side, number)? == to.line(side, there)?;
            same.then_some(Anchor::Line {
                side,
                number: there,
            })
        }
        Anchor::Hunk { index } => {
            let hunk = from.hunks().get(index)?;
            let lines = (hunk.old_start..)
                .take(hunk.removed.len())
                .map(|number| (Side::Old, number))
                .chain(
                    (hunk.new_start..)
                        .take(hunk.added.len())
                        .map(|number| (Side::New, number)),
                );
            // Each hunk's votes, and the order it got its first in, for ties.
            let mut votes: HashMap<usize, (usize, usize)> = HashMap::new();
            let mut stayed = 0;
            for (side, number) in lines {
                // As for a line comment, a line counts only where its text is its own.
                let Some(there) = map.carry(side, number).filter(|&there| {
                    from.line(side, number)
                        .is_some_and(|line| to.line(side, there) == Some(line))
                }) else {
                    continue;
                };
                let Some(there) = to.hunk_at(side, there) else {
                    continue;
                };
                stayed += 1;
                let first = votes.len();
                votes.entry(there).or_insert((first, 0)).1 += 1;
            }
            let members = hunk.removed.len() + hunk.added.len();
            if 2 * stayed <= members {
                return None;
            }
            let best = votes
                .into_iter()
                .max_by(|a, b| a.1.1.cmp(&b.1.1).then(b.1.0.cmp(&a.1.0)))?;
            Some(Anchor::Hunk { index: best.0 })
        }
    }
}
