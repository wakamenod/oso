//! A review of changes (ADR 0122): host-owned state that every key, agent tool and refresh
//! changes through the same functions. It holds each file's current reading, the comments and
//! their threads, and the decisions on hunks, keyed by content. It holds no geometry: windows
//! keep their own places.
//!
//! Must not depend on: `editor`, `buffer` or any other module of the crate, git, processes or
//! I/O. Paths are bytes as git gives them; the host reads git and the files, diffs them on a
//! worker and hands in plain data, the longest common subsequence included.
//!
//! Comments are kept apart from the text and carried to their line on every reading by the
//! shape of each reading ([`LineMap`]), never by guessing from nearby text. Carrying runs in
//! three parts so that matching stays off the UI thread: [`Review::carry`] takes a snapshot,
//! [`Carry::run`] runs on the review's worker, and [`Review::apply`] writes the result. An
//! outdated comment keeps its last reading to come back by; a file keeps at most
//! 16 of them, and comments on older ones stay outdated.
//!
//! Who may change what (Q11): an agent adds comments, replies to any, and edits and removes its
//! own; it resolves or reopens its own only while no reply in the thread is the person's. It
//! never edits, resolves or removes the person's. The person edits only their own, and resolves
//! and removes any. Decisions, viewed marks, and marking comments sent or read are the host's,
//! at the person's word: no agent tool reaches them.

use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

mod carry;
mod comment;
mod decision;
mod map;
mod marks;
mod rows;
mod saved;
mod shape;

pub use carry::{Applied, Carried, Carry};
pub use comment::{
    Anchor, Author, Comment, CommentError, CommentId, MAX_COMMENT_BYTES, MAX_COMMENTS,
};
pub use decision::Decision;
pub use map::{BaseHunk, LineMap, base_pair};
pub(crate) use rows::Place;
pub use rows::{Row, RowKind, Rows, Section, Seek};
pub use saved::{RestoreError, Saved, SavedComment, SavedFile};
pub use shape::{Base, Hunk, Shape, ShapeError, Side};

use carry::Item;
use decision::{Mark, Marked};
use marks::Progress;

/// The most earlier readings a file's outdated comments keep to come back by.
const MAX_KEPT_READINGS: usize = 16;

/// A review: its files' readings, its comments and its decisions.
#[derive(Clone, Debug, Default)]
pub struct Review {
    files: BTreeMap<Box<[u8]>, PathState>,
    comments: BTreeMap<CommentId, Comment>,
    next_id: u64,
    hasher: RandomState,
    /// The marks of every current reading, counted as they change, so that a modeline asks
    /// at O(1).
    progress: Progress,
    /// Readings let go since [`Review::take_released`], for the host to free off the UI thread.
    released: Vec<Arc<Shape>>,
    /// Counts changes, so that the host saves the review only when it changed (step 6).
    revision: u64,
}

/// What the review keeps of one path, read now or once.
#[derive(Clone, Debug, Default)]
struct PathState {
    /// The current reading; `None` once the file left the review.
    reading: Option<Reading>,
    /// Decisions dropped since the person last looked.
    dropped: usize,
    comments: BTreeSet<CommentId>,
    /// Counts readings applied, so a snapshot taken before one is not applied after it.
    readings: u64,
    /// Counts comments added, so a snapshot that missed one is carried again.
    added: u64,
    /// Counts changes to the marks, so marks changed since a snapshot are carried in its place.
    remarked: u64,
    /// The current reading was restored from a saved review and no reading was applied since:
    /// the next one marks no hunk changed (step 6, Q-J).
    restored: bool,
}

/// A file's current reading, its hunks' hashes and the person's mark on each hunk.
#[derive(Clone, Debug)]
struct Reading {
    shape: Arc<Shape>,
    hashes: Arc<[u64]>,
    marks: Vec<Mark>,
}

impl Review {
    pub fn new() -> Review {
        Review::default()
    }

    /// A count of the review's changes: it differs whenever a reading, comment or mark
    /// changed since it was last asked, so that the host saves the review only then (ADR
    /// 0122, step 6).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The current reading of the file at `path`.
    pub fn file(&self, path: &[u8]) -> Option<&Arc<Shape>> {
        Some(&self.files.get(path)?.reading.as_ref()?.shape)
    }

    /// The files with a current reading, by path.
    pub fn files(&self) -> impl Iterator<Item = (&[u8], &Arc<Shape>)> {
        self.files
            .iter()
            .filter_map(|(path, state)| Some((&**path, &state.reading.as_ref()?.shape)))
    }

    /// Takes the file at `path` out of the review: its comments go outdated, kept to come back
    /// with a later reading, and its decisions drop.
    pub fn remove_file(&mut self, path: &[u8]) {
        self.revision += 1;
        let Some(state) = self.files.get_mut(path) else {
            return;
        };
        if let Some(reading) = state.reading.take() {
            state.dropped += reading.marks.iter().filter(|mark| mark.is_set()).count();
            self.progress.count(&reading.marks, true);
            self.released.push(reading.shape);
        }
        state.readings += 1;
        for id in &state.comments {
            if let Some(comment) = self.comments.get_mut(id) {
                comment.outdated = true;
            }
        }
    }

    /// A snapshot of the comments and marks on `path`, to carry to the reading `to` on a
    /// worker. Costs a reference to each comment's reading, and a copy of the marks when the
    /// person marked a hunk.
    pub fn carry(&self, path: &[u8], to: Arc<Shape>) -> Carry {
        let state = self.files.get(path);
        let items = state
            .into_iter()
            .flat_map(|state| &state.comments)
            .filter_map(|id| self.comments.get(id))
            .filter_map(|comment| {
                Some(Item {
                    id: comment.id,
                    from: Arc::clone(comment.shape.as_ref()?),
                    anchor: comment.anchor,
                })
            })
            .collect();
        // The current reading always: its lines' map keeps the windows' places, and its hunks
        // tell which of the new reading's are new.
        let marked = state
            .and_then(|state| state.reading.as_ref())
            .map(|reading| {
                let shape = Arc::clone(&reading.shape);
                (shape, Arc::clone(&reading.hashes), reading.marks.clone())
            });
        Carry {
            path: path.into(),
            to,
            items,
            marked,
            unseen: false,
            restored: state.is_some_and(|state| state.restored),
            hasher: self.hasher.clone(),
            readings: state.map_or(0, |state| state.readings),
            added: state.map_or(0, |state| state.added),
            remarked: state.map_or(0, |state| state.remarked),
        }
    }

    /// Makes `carried`'s reading the file's current one, moves its comments and carries the
    /// marks, dropping those whose hunks went. A result taken before another reading was
    /// applied changes nothing and asks to be carried again. A comment the snapshot missed is
    /// outdated until then.
    pub fn apply(&mut self, carried: Carried) -> Applied {
        self.revision += 1;
        let state = self.files.entry(carried.path).or_default();
        if carried.readings != state.readings {
            return Applied {
                again: true,
                ..Applied::default()
            };
        }
        let mut applied = Applied {
            again: carried.added != state.added,
            marks_changed: carried.remarked != state.remarked,
            ..Applied::default()
        };
        let to = carried.to;
        let (marks, dropped) = match (&state.reading, applied.marks_changed) {
            // The marks changed since the snapshot: carry the current ones.
            (Some(reading), true) => {
                let from = Marked {
                    hunks: reading.shape.hunks(),
                    hashes: &reading.hashes,
                    marks: &reading.marks,
                };
                let (mut marks, dropped) =
                    decision::carry_marks(&from, to.hunks(), &carried.hashes);
                if state.restored {
                    decision::clear_changed(&mut marks);
                }
                (marks, dropped)
            }
            _ => (carried.marks, carried.dropped),
        };
        state.readings += 1;
        state.restored = false;
        for (id, placement) in carried.placed {
            let Some(comment) = self.comments.get_mut(&id) else {
                continue;
            };
            match placement {
                Some(anchor) => {
                    applied.placed += 1;
                    applied.returned += usize::from(comment.outdated);
                    comment.anchor = anchor;
                    comment.shape = Some(Arc::clone(&to));
                    comment.reading = state.readings;
                    comment.outdated = false;
                }
                None => {
                    applied.outdated += usize::from(!comment.outdated);
                    comment.outdated = true;
                }
            }
        }
        for id in &state.comments {
            if let Some(comment) = self.comments.get_mut(id)
                && !comment.outdated
                && comment
                    .shape
                    .as_ref()
                    .is_some_and(|shape| !Arc::ptr_eq(shape, &to))
            {
                // Added since the snapshot: outdated until it is carried again.
                comment.outdated = true;
            }
        }
        let_go(state, &mut self.comments, &to, &mut self.released);
        state.dropped += dropped;
        applied.dropped = dropped;
        if let Some(before) = state.reading.take() {
            self.progress.count(&before.marks, true);
            self.released.push(before.shape);
        }
        self.progress.count(&marks, false);
        state.reading = Some(Reading {
            shape: to,
            hashes: carried.hashes,
            marks,
        });
        applied
    }

    pub fn comment(&self, id: CommentId) -> Option<&Comment> {
        self.comments.get(&id)
    }

    /// Every comment, oldest first.
    pub fn comments(&self) -> impl Iterator<Item = &Comment> {
        self.comments.values()
    }

    /// The comments on the file at `path`, oldest first, outdated ones included. Costs its
    /// comments, not the review's.
    pub fn comments_on(&self, path: &[u8]) -> impl Iterator<Item = &Comment> {
        self.files
            .get(path)
            .into_iter()
            .flat_map(|state| &state.comments)
            .filter_map(|id| self.comments.get(id))
    }

    /// Adds `by`'s comment on a line or hunk the current reading of `path` shows.
    pub fn add_comment(
        &mut self,
        by: Author,
        path: &[u8],
        anchor: Anchor,
        text: String,
    ) -> Result<CommentId, CommentError> {
        self.check_room(&text)?;
        let state = self.files.get(path).ok_or(CommentError::NoSuchFile)?;
        let shape = &state
            .reading
            .as_ref()
            .ok_or(CommentError::NoSuchFile)?
            .shape;
        let shown = match anchor {
            Anchor::Line { side, number } => shape.line(side, number).is_some(),
            Anchor::Hunk { index } => index < shape.hunks().len(),
        };
        if !shown {
            return Err(CommentError::NotInDiff);
        }
        let comment = Comment {
            id: CommentId(0),
            author: by,
            path: path.into(),
            anchor,
            shape: Some(Arc::clone(shape)),
            reading: state.readings,
            text: text.into(),
            reply_to: None,
            sent: false,
            resolved: false,
            outdated: false,
            read: by == Author::Person,
        };
        Ok(self.insert(comment))
    }

    /// Adds `by`'s reply to the comment `to`, which takes its anchor.
    pub fn reply(
        &mut self,
        by: Author,
        to: CommentId,
        text: String,
    ) -> Result<CommentId, CommentError> {
        self.check_room(&text)?;
        let parent = self.comments.get(&to).ok_or(CommentError::NoSuchComment)?;
        let comment = Comment {
            author: by,
            text: text.into(),
            reply_to: Some(to),
            sent: false,
            resolved: false,
            read: by == Author::Person,
            ..parent.clone()
        };
        Ok(self.insert(comment))
    }

    /// Changes the text of `by`'s own comment `id`, which makes it unsent again.
    pub fn edit_comment(
        &mut self,
        by: Author,
        id: CommentId,
        text: String,
    ) -> Result<(), CommentError> {
        if text.len() > MAX_COMMENT_BYTES {
            return Err(CommentError::TooLong);
        }
        let comment = self.own(by, id)?;
        comment.text = text.into();
        comment.sent = false;
        comment.read = by == Author::Person;
        self.revision += 1;
        Ok(())
    }

    /// Resolves the comment `id`, or opens it again. The person may for any; an agent only for
    /// its own, while no reply under it is the person's.
    pub fn resolve(
        &mut self,
        by: Author,
        id: CommentId,
        resolved: bool,
    ) -> Result<(), CommentError> {
        if by == Author::Agent {
            self.own(by, id)?;
            self.only_agents_in(id)?;
        }
        let comment = self
            .comments
            .get_mut(&id)
            .ok_or(CommentError::NoSuchComment)?;
        comment.resolved = resolved;
        self.revision += 1;
        Ok(())
    }

    /// Removes the comment `id` with its replies, and returns how many went. An agent removes
    /// only its own, and not one with a reply of the person's.
    pub fn remove_comment(&mut self, by: Author, id: CommentId) -> Result<usize, CommentError> {
        if by == Author::Agent {
            self.own(by, id)?;
            self.only_agents_in(id)?;
        }
        let thread = self.thread(id).ok_or(CommentError::NoSuchComment)?;
        self.revision += 1;
        for id in &thread {
            if let Some(comment) = self.comments.remove(id)
                && let Some(state) = self.files.get_mut(&*comment.path)
            {
                state.comments.remove(id);
            }
        }
        Ok(thread.len())
    }

    /// Marks the comments `ids` sent.
    pub fn mark_sent(&mut self, ids: &[CommentId]) {
        self.revision += 1;
        for id in ids {
            if let Some(comment) = self.comments.get_mut(id) {
                comment.sent = true;
            }
        }
    }

    /// Marks the comments `ids` read by the person.
    pub fn mark_read(&mut self, ids: &[CommentId]) {
        self.revision += 1;
        for id in ids {
            if let Some(comment) = self.comments.get_mut(id) {
                comment.read = true;
            }
        }
    }

    fn check_room(&self, text: &str) -> Result<(), CommentError> {
        if text.len() > MAX_COMMENT_BYTES {
            return Err(CommentError::TooLong);
        }
        if self.comments.len() >= MAX_COMMENTS {
            return Err(CommentError::TooMany);
        }
        Ok(())
    }

    /// Gives `comment` the next id and keeps it.
    fn insert(&mut self, mut comment: Comment) -> CommentId {
        self.revision += 1;
        let id = CommentId(self.next_id);
        self.next_id += 1;
        comment.id = id;
        let state = self.files.entry(Box::from(&*comment.path)).or_default();
        state.comments.insert(id);
        state.added += 1;
        self.comments.insert(id, comment);
        id
    }

    /// The comment `id`, when it is `by`'s.
    fn own(&mut self, by: Author, id: CommentId) -> Result<&mut Comment, CommentError> {
        let comment = self
            .comments
            .get_mut(&id)
            .ok_or(CommentError::NoSuchComment)?;
        if comment.author != by {
            return Err(CommentError::NotYours);
        }
        Ok(comment)
    }

    /// Refuses when a reply under `id` is the person's.
    fn only_agents_in(&self, id: CommentId) -> Result<(), CommentError> {
        let thread = self.thread(id).ok_or(CommentError::NoSuchComment)?;
        let person = thread
            .iter()
            .filter_map(|id| self.comments.get(id))
            .any(|comment| comment.author == Author::Person);
        if person {
            return Err(CommentError::RepliesNotYours);
        }
        Ok(())
    }

    /// `id` and every reply under it, at any depth, found among its file's comments, where
    /// every reply is.
    pub(crate) fn thread(&self, id: CommentId) -> Option<Vec<CommentId>> {
        let root = self.comments.get(&id)?;
        let mut replies: HashMap<CommentId, Vec<CommentId>> = HashMap::new();
        let file = self.files.get(&*root.path).map(|state| &state.comments);
        for comment in file
            .into_iter()
            .flatten()
            .filter_map(|id| self.comments.get(id))
        {
            if let Some(parent) = comment.reply_to {
                replies.entry(parent).or_default().push(comment.id);
            }
        }
        let mut thread = vec![id];
        let mut at = 0;
        while let Some(&parent) = thread.get(at) {
            thread.extend(replies.remove(&parent).unwrap_or_default());
            at += 1;
        }
        Some(thread)
    }

    /// The mark on hunk `index` of `path`'s current reading.
    fn mark(&self, path: &[u8], index: usize) -> Option<Mark> {
        self.files
            .get(path)?
            .reading
            .as_ref()?
            .marks
            .get(index)
            .copied()
    }

    fn change_mark(&mut self, path: &[u8], index: usize, change: impl FnOnce(&mut Mark)) -> bool {
        let Some(state) = self.files.get_mut(path) else {
            return false;
        };
        let Some(mark) = state
            .reading
            .as_mut()
            .and_then(|reading| reading.marks.get_mut(index))
        else {
            return false;
        };
        self.progress.count(std::slice::from_ref(mark), true);
        self.revision += 1;
        change(mark);
        self.progress.count(std::slice::from_ref(mark), false);
        state.remarked += 1;
        true
    }
}

/// Lets go of the readings `state`'s outdated comments keep past what they may, `to` being its
/// new one: those comments stay outdated for good, and the readings go to `released`.
fn let_go(
    state: &PathState,
    comments: &mut BTreeMap<CommentId, Comment>,
    to: &Arc<Shape>,
    released: &mut Vec<Arc<Shape>>,
) {
    let kept = kept_readings(state, comments, to);
    for id in &state.comments {
        if let Some(comment) = comments.get_mut(id)
            && comment.outdated
            && let Some(shape) = &comment.shape
            && !Arc::ptr_eq(shape, to)
            && !kept.contains(&Arc::as_ptr(shape))
            && let Some(shape) = comment.shape.take()
        {
            released.push(shape);
        }
    }
}

/// The earlier readings a file's outdated comments may keep: the newest
/// [`MAX_KEPT_READINGS`] of them.
fn kept_readings(
    state: &PathState,
    comments: &BTreeMap<CommentId, Comment>,
    to: &Arc<Shape>,
) -> Vec<*const Shape> {
    let mut newest: HashMap<*const Shape, u64> = HashMap::new();
    for comment in state.comments.iter().filter_map(|id| comments.get(id)) {
        if let Some(shape) = &comment.shape
            && comment.outdated
            && !Arc::ptr_eq(shape, to)
        {
            let at = newest.entry(Arc::as_ptr(shape)).or_default();
            *at = (*at).max(comment.reading);
        }
    }
    let mut readings: Vec<(u64, *const Shape)> =
        newest.into_iter().map(|(p, at)| (at, p)).collect();
    readings.sort_unstable_by_key(|reading| std::cmp::Reverse(reading.0));
    readings.truncate(MAX_KEPT_READINGS);
    readings.into_iter().map(|(_, shape)| shape).collect()
}

#[cfg(test)]
mod tests;
