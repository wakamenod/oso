//! A review as it is saved across restarts (ADR 0122, "Persistence", step 6): the readings its
//! comments and marks are in, its comments with every state and id, and the person's marks, as
//! plain data the host writes and reads back.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O. The host
//! serializes a [`Saved`] on a thread, and builds the review back with [`Review::restore`] off
//! the UI thread, from data that came from disk and is checked whole here.
//!
//! A restored review holds the saved readings as its current ones, so nothing new carries it:
//! the first reading of each file is carried from the saved one as any refresh's is, its
//! comments moved or outdated by the shape and its marks kept by their hunks' text. That first
//! carry marks no hunk changed (Q-J): the review is new to this run, as one opened afresh is.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use super::comment::{Anchor, Author, Comment, CommentId, MAX_COMMENT_BYTES, MAX_COMMENTS};
use super::decision::{self, Decision, Mark};
use super::shape::Shape;
use super::{MAX_KEPT_READINGS, PathState, Reading, Review};

/// A review as it is saved: what [`Review::save`] gives and [`Review::restore`] takes.
#[derive(Clone, Debug, Default)]
pub struct Saved {
    /// The files whose current reading holds a mark or a comment, or whose dropped count is
    /// not zero, by path.
    pub files: Vec<SavedFile>,
    /// Every comment, oldest first.
    pub comments: Vec<SavedComment>,
    /// The id the next comment gets, so that none is reused across a restart.
    pub next_id: u64,
}

/// A file's current reading and the person's marks on its hunks.
#[derive(Clone, Debug)]
pub struct SavedFile {
    pub path: Box<[u8]>,
    pub shape: Arc<Shape>,
    /// Each hunk's decision and whether it is viewed, as many as the reading has hunks.
    pub marks: Vec<(Option<Decision>, bool)>,
    /// Decisions and viewed marks dropped since the person last looked.
    pub dropped: usize,
}

/// A comment with its state. `shape` is the reading its anchor is in: the file's current one,
/// the same `Arc` as its [`SavedFile`]'s, unless it is outdated; `None` for one that is
/// outdated for good, as one whose reading was let go or was not written. Ids are numbers
/// ([`CommentId::get`]), so that a [`CommentId`] is made only by a review.
#[derive(Clone, Debug)]
pub struct SavedComment {
    pub id: u64,
    pub author: Author,
    pub path: Arc<[u8]>,
    pub anchor: Anchor,
    pub shape: Option<Arc<Shape>>,
    pub text: Arc<str>,
    pub reply_to: Option<u64>,
    pub sent: bool,
    pub resolved: bool,
    pub outdated: bool,
    pub read: bool,
}

/// Why [`Review::restore`] refused a saved review: data that no review saves, as a file
/// changed by hand or cut short would give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// More than [`MAX_COMMENTS`] comments.
    TooManyComments,
    /// A comment's text is longer than [`MAX_COMMENT_BYTES`].
    TooLong { id: CommentId },
    /// Two comments have this id.
    SameId { id: CommentId },
    /// A comment's id is not below the next id, or the next id is past any a review reaches.
    IdTooLarge { id: CommentId },
    /// A reply's parent is missing, newer than the reply, or on another file.
    NoParent { id: CommentId },
    /// A comment's anchor is not a line or hunk its reading shows.
    NotInReading { id: CommentId },
    /// Two files have this path, at this index of the files.
    SameFile { file: usize },
    /// A file's marks are not as many as its reading's hunks, or more dropped than any review
    /// drops.
    Marks { file: usize },
}

/// The largest next id a saved review may hold: far past any a review reaches, and far from
/// the last number, so that adding to it never overflows.
const MAX_ID: u64 = 1 << 53;

/// The most dropped marks a saved file may hold.
const MAX_DROPPED: usize = 1 << 32;

impl Review {
    /// The review as it is saved: the files whose current reading holds a mark or a comment,
    /// or whose dropped count is not zero, and every comment. Costs a reference to each
    /// reading and comment text, and a copy of each saved file's path and marks.
    pub fn save(&self) -> Saved {
        let files = self
            .files
            .iter()
            .filter_map(|(path, state)| {
                let reading = state.reading.as_ref()?;
                let marked = reading.marks.iter().any(|mark| mark.is_set());
                let commented = state.comments.iter().any(|id| {
                    self.comments
                        .get(id)
                        .and_then(|comment| comment.shape.as_ref())
                        .is_some_and(|shape| Arc::ptr_eq(shape, &reading.shape))
                });
                (marked || commented || state.dropped > 0).then(|| SavedFile {
                    path: path.clone(),
                    shape: Arc::clone(&reading.shape),
                    marks: (reading.marks.iter())
                        .map(|mark| (mark.decision, mark.viewed))
                        .collect(),
                    dropped: state.dropped,
                })
            })
            .collect();
        let comments = self
            .comments
            .values()
            .map(|comment| SavedComment {
                id: comment.id.0,
                author: comment.author,
                path: Arc::clone(&comment.path),
                anchor: comment.anchor,
                shape: comment.shape.clone(),
                text: Arc::clone(&comment.text),
                reply_to: comment.reply_to.map(CommentId::get),
                sent: comment.sent,
                resolved: comment.resolved,
                outdated: comment.outdated,
                read: comment.read,
            })
            .collect();
        Saved {
            files,
            comments,
            next_id: self.next_id,
        }
    }

    /// The review `saved` holds, checked whole: refused if any comment or file is one no
    /// review saves. A comment on its file's current reading, the same `Arc`, is carried with
    /// it; one on any other reading is outdated; the person's comments are read; a file keeps at most
    /// [`MAX_KEPT_READINGS`] earlier readings, those of its newer comments, and comments on
    /// the others stay outdated for good. Costs the hunks of every reading, to hash them.
    pub fn restore(saved: Saved) -> Result<Review, RestoreError> {
        let Saved {
            files,
            comments,
            next_id,
        } = saved;
        if comments.len() > MAX_COMMENTS {
            return Err(RestoreError::TooManyComments);
        }
        if next_id > MAX_ID {
            return Err(RestoreError::IdTooLarge {
                id: CommentId(next_id),
            });
        }
        let mut review = Review {
            next_id,
            ..Review::default()
        };
        for (index, file) in files.into_iter().enumerate() {
            if file.marks.len() != file.shape.hunks().len() || file.dropped > MAX_DROPPED {
                return Err(RestoreError::Marks { file: index });
            }
            if review.files.contains_key(&file.path) {
                return Err(RestoreError::SameFile { file: index });
            }
            let hashes: Arc<[u64]> = (file.shape.hunks().iter())
                .map(|hunk| decision::hash(&review.hasher, hunk))
                .collect();
            let marks: Vec<Mark> = (file.marks.iter())
                .map(|&(decision, viewed)| Mark {
                    decision,
                    viewed,
                    changed: false,
                })
                .collect();
            review.progress.count(&marks, false);
            let state = PathState {
                reading: Some(Reading {
                    shape: file.shape,
                    hashes,
                    marks,
                }),
                dropped: file.dropped,
                readings: 1,
                restored: true,
                ..PathState::default()
            };
            review.files.insert(file.path, state);
        }
        let mut paths: HashMap<CommentId, Arc<[u8]>> = HashMap::new();
        let mut kept: BTreeMap<CommentId, Comment> = BTreeMap::new();
        for saved in comments {
            let comment = review.restored(saved, &paths)?;
            paths.insert(comment.id, Arc::clone(&comment.path));
            kept.insert(comment.id, comment);
        }
        for (id, comment) in &kept {
            let state = review.files.entry(Box::from(&*comment.path)).or_default();
            state.comments.insert(*id);
        }
        review.comments = kept;
        review.keep_readings();
        Ok(review)
    }

    /// The comment `saved` holds, checked against the files restored and the comments before
    /// it, whose paths are `paths`.
    fn restored(
        &self,
        saved: SavedComment,
        paths: &HashMap<CommentId, Arc<[u8]>>,
    ) -> Result<Comment, RestoreError> {
        let id = CommentId(saved.id);
        if saved.text.len() > MAX_COMMENT_BYTES {
            return Err(RestoreError::TooLong { id });
        }
        if paths.contains_key(&id) {
            return Err(RestoreError::SameId { id });
        }
        if id.0 >= self.next_id {
            return Err(RestoreError::IdTooLarge { id });
        }
        let reply_to = saved.reply_to.map(CommentId);
        if let Some(parent) = reply_to {
            // A reply is always newer than its parent, which rules out a cycle.
            let same_file = paths.get(&parent).is_some_and(|path| *path == saved.path);
            if parent >= id || !same_file {
                return Err(RestoreError::NoParent { id });
            }
        }
        if let Some(shape) = &saved.shape {
            let shown = match saved.anchor {
                Anchor::Line { side, number } => shape.line(side, number).is_some(),
                Anchor::Hunk { index } => index < shape.hunks().len(),
            };
            if !shown {
                return Err(RestoreError::NotInReading { id });
            }
        }
        let current = (self.files.get(&*saved.path))
            .and_then(|state| state.reading.as_ref())
            .map(|reading| &reading.shape);
        // On the current reading, by reference, as a save writes it: carried with it. On any
        // other, outdated, and carried from it; a reading equal to the current one is not
        // looked for, which would compare whole readings once per comment.
        let on_current = (current.zip(saved.shape.as_ref()))
            .is_some_and(|(current, shape)| Arc::ptr_eq(current, shape));
        let outdated = saved.outdated || !on_current;
        let shape = saved.shape;
        let reading = u64::from(!outdated);
        Ok(Comment {
            id,
            author: saved.author,
            path: saved.path,
            anchor: saved.anchor,
            outdated: outdated || shape.is_none(),
            shape,
            reading,
            text: saved.text,
            reply_to,
            sent: saved.sent,
            resolved: saved.resolved,
            read: saved.read || saved.author == Author::Person,
        })
    }

    /// Lets go of the earlier readings each file's outdated comments keep past
    /// [`MAX_KEPT_READINGS`], keeping those of its newest comments.
    fn keep_readings(&mut self) {
        for state in self.files.values() {
            let current = state.reading.as_ref().map(|reading| &reading.shape);
            let mut kept: BTreeSet<*const Shape> = BTreeSet::new();
            for id in state.comments.iter().rev() {
                let Some(comment) = self.comments.get_mut(id) else {
                    continue;
                };
                let Some(shape) = &comment.shape else {
                    continue;
                };
                if current.is_some_and(|current| Arc::ptr_eq(current, shape)) {
                    continue;
                }
                let pointer = Arc::as_ptr(shape);
                if !kept.contains(&pointer) && kept.len() >= MAX_KEPT_READINGS {
                    comment.shape = None;
                    comment.outdated = true;
                } else {
                    kept.insert(pointer);
                }
            }
        }
    }
}
