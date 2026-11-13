//! Reading one file of a review into a reading (ADR 0122): its sides, through `cat-file` or
//! from the work tree, checked for secrets, binary and size before anything is diffed.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this reads files and blobs
//! on the review's thread.
//!
//! A reading is kept per path with its signature: the old side's blob and modes, and the new
//! side's blob or, for a work tree file, its stamp or a link's target. A file whose signature
//! did not change is not read again. A file that could not be read, did not fit the room left
//! or ran out of the request's time keeps no signature, so the next refresh reads it again.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::buffer::FileStamp;
use oso_core::review::{Base, Shape};
use oso_platform::file_meta::Directory;

use super::batch::{Batch, Blob};
use super::diff;
use super::git::Git;
use super::id::Id;
use super::list::{Entry, MODE_SUBMODULE, Status};
use super::scope::New;
use super::side::{self, Found, Got};
use super::{Content, Reading};

/// The most bytes of sides and hunks a review keeps, over all its files.
pub(crate) const MAX_HELD: usize = 64 * 1024 * 1024;
/// How long one file's diff may take.
const DIFF_DEADLINE: Duration = Duration::from_secs(1);
/// What a line kept in a hunk costs besides its bytes.
const LINE_COST: usize = 16;

/// What makes a reading still good: when none of it changed, the file is not read again.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Signature {
    old: Id,
    old_mode: u32,
    new_mode: u32,
    new: NewSign,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum NewSign {
    Blob(Id),
    Stamp(FileStamp),
    Link(Box<[u8]>),
    /// A work tree file with more than one name.
    Shared,
    Nothing,
}

/// The readings a review keeps, by path, and the bytes they hold.
#[derive(Default)]
pub(super) struct Kept {
    files: HashMap<Box<[u8]>, (Signature, usize)>,
    held: usize,
}

impl Kept {
    /// Lets go of every path `listed` does not name.
    pub(super) fn keep_only(&mut self, listed: &[Entry]) {
        let listed: HashSet<&[u8]> = listed.iter().map(|entry| &*entry.path).collect();
        let held = &mut self.held;
        self.files.retain(|path, (_, bytes)| {
            let keep = listed.contains(&**path);
            if !keep {
                *held -= *bytes;
            }
            keep
        });
    }

    pub(super) fn clear(&mut self) {
        *self = Kept::default();
    }

    /// Whether `path` is kept by `signature`.
    fn has(&self, path: &[u8], signature: &Signature) -> bool {
        self.files
            .get(path)
            .is_some_and(|(kept, _)| kept == signature)
    }

    fn forget(&mut self, path: &[u8]) {
        if let Some((_, bytes)) = self.files.remove(path) {
            self.held -= bytes;
        }
    }

    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.held
    }
}

/// What reading a file needs.
pub(super) struct Context<'a> {
    pub(super) git: &'a Git,
    pub(super) batch: &'a mut Batch,
    pub(super) kept: &'a mut Kept,
    pub(super) top: Option<&'a Directory>,
    pub(super) new: &'a New,
    /// When the request's time for reading and diffing ends.
    pub(super) budget: Instant,
}

/// Reads `entry` again, or `None` if its signature did not change.
pub(super) fn read(cx: &mut Context<'_>, entry: &Entry) -> Option<Reading> {
    let secret = |path: &[u8]| side::may_hold_secrets(cx.git.top(), path);
    if secret(&entry.path) || entry.old_path.as_deref().is_some_and(secret) {
        let signature = signature(entry, NewSign::Nothing);
        return keep(cx, entry, signature, Content::Withheld, 0);
    }
    if entry.status == Status::Unmerged {
        let signature = signature(entry, NewSign::Nothing);
        return keep(cx, entry, signature, Content::Conflicted, 0);
    }
    if entry.old_mode == MODE_SUBMODULE || entry.new_mode == MODE_SUBMODULE {
        let signature = signature(entry, NewSign::Nothing);
        return keep(cx, entry, signature, Content::Submodule, 0);
    }
    let (new_sign, file) = look(cx, entry);
    if new_sign == NewSign::Shared {
        let signature = signature(entry, NewSign::Shared);
        return keep(cx, entry, signature, Content::Withheld, 0);
    }
    let signature = signature(entry, new_sign);
    if cx.kept.has(&entry.path, &signature) {
        return None;
    }
    cx.kept.forget(&entry.path);
    let reading = |content| {
        Some(Reading {
            entry: entry.clone(),
            content,
        })
    };
    if Instant::now() >= cx.budget {
        return reading(Content::OutOfTime);
    }
    let (old, new) = match sides(cx, entry, &signature.new, file) {
        Ok(sides) => sides,
        Err(why) => return reading(Content::Unreadable(why)),
    };
    let base = if added(entry) {
        Base::Empty
    } else {
        Base::Blob(entry.old_id.as_str().as_bytes().into())
    };
    let room = MAX_HELD.saturating_sub(cx.kept.held);
    match text(base, old, new, room, cx.budget) {
        Texted::Kept(content, held) => keep(cx, entry, signature, content, held),
        Texted::NotKept(content) => reading(content),
    }
}

/// Both sides of `entry`, or why one could not be read.
fn sides(
    cx: &mut Context<'_>,
    entry: &Entry,
    new: &NewSign,
    file: Option<File>,
) -> Result<(Got, Got), String> {
    let old = if added(entry) {
        Got::Bytes(Vec::new())
    } else {
        blob(cx, &entry.old_id)?
    };
    let new = match (new, file) {
        _ if entry.status == Status::Deleted => Got::Bytes(Vec::new()),
        (NewSign::Stamp(stamp), Some(file)) => {
            side::read(file, stamp.len).map_err(|e| format!("cannot read it: {e}"))?
        }
        (NewSign::Link(target), _) => Got::Bytes(target.to_vec()),
        (NewSign::Blob(id), _) => blob(cx, id)?,
        (NewSign::Nothing | NewSign::Shared | NewSign::Stamp(_), _) => Got::Bytes(Vec::new()),
    };
    Ok((old, new))
}

fn added(entry: &Entry) -> bool {
    entry.status == Status::Added || entry.old_id.is_zero()
}

fn signature(entry: &Entry, new: NewSign) -> Signature {
    Signature {
        old: entry.old_id.clone(),
        old_mode: entry.old_mode,
        new_mode: entry.new_mode,
        new,
    }
}

/// Keeps `content` as the reading of `entry` by `signature`, holding `held` bytes; `None` if
/// it was kept so already.
fn keep(
    cx: &mut Context<'_>,
    entry: &Entry,
    signature: Signature,
    content: Content,
    held: usize,
) -> Option<Reading> {
    if cx.kept.has(&entry.path, &signature) {
        return None;
    }
    cx.kept.forget(&entry.path);
    cx.kept.held += held;
    cx.kept.files.insert(entry.path.clone(), (signature, held));
    Some(Reading {
        entry: entry.clone(),
        content,
    })
}

/// What the new side is now, and the file open if it is one.
fn look(cx: &Context<'_>, entry: &Entry) -> (NewSign, Option<File>) {
    match cx.new {
        New::WorkTree { .. } => match cx.top.map(|top| side::find(top, &entry.path)) {
            Some(Ok(Found::File(file, stamp))) => (NewSign::Stamp(stamp), Some(file)),
            Some(Ok(Found::Link(target))) => (NewSign::Link(target.into()), None),
            Some(Ok(Found::Shared)) => (NewSign::Shared, None),
            Some(Ok(Found::Nothing) | Err(_)) | None => (NewSign::Nothing, None),
        },
        New::Index | New::Tree(_) => (NewSign::Blob(entry.new_id.clone()), None),
    }
}

/// The blob `id`, or why it could not be read.
fn blob(cx: &mut Context<'_>, id: &Id) -> Result<Got, String> {
    match cx.batch.read(cx.git, id) {
        Ok(Blob::Bytes(bytes)) => Ok(Got::Bytes(bytes)),
        Ok(Blob::TooLarge(size)) => Ok(Got::TooLarge(size)),
        Ok(Blob::Missing) => Err("a side is missing from the repository".into()),
        Err(e) => Err(e.to_string()),
    }
}

/// A reading's content, and whether it is kept with what it holds.
enum Texted {
    Kept(Content, usize),
    NotKept(Content),
}

/// The sides as a reading's content: the text and its hunks, or why there are none. `room` is
/// what may still be held; `budget` when the request's time ends.
fn text(base: Base, old: Got, new: Got, room: usize, budget: Instant) -> Texted {
    let size = |got: &Got| match got {
        Got::Bytes(bytes) => bytes.len() as u64,
        Got::TooLarge(size) => *size,
    };
    let (old_size, new_size) = (size(&old), size(&new));
    let too_large = Content::TooLarge {
        old: old_size,
        new: new_size,
    };
    let (Got::Bytes(old), Got::Bytes(new)) = (old, new) else {
        return Texted::Kept(too_large, 0);
    };
    if side::is_binary(&old) || side::is_binary(&new) {
        let binary = Content::Binary {
            old: old_size,
            new: new_size,
        };
        return Texted::Kept(binary, 0);
    }
    if diff::line_count(&old) > diff::MAX_LINES || diff::line_count(&new) > diff::MAX_LINES {
        return Texted::Kept(too_large, 0);
    }
    if old.len() + new.len() > room {
        return Texted::NotKept(too_large);
    }
    let deadline = budget.min(Instant::now() + DIFF_DEADLINE);
    let Some(hunks) = diff::hunks(&diff::lines(&old), &diff::lines(&new), deadline) else {
        if Instant::now() >= budget {
            return Texted::NotKept(Content::OutOfTime);
        }
        return Texted::Kept(too_large, 0);
    };
    let hunk_bytes: usize = hunks
        .iter()
        .flat_map(|hunk| hunk.removed.iter().chain(&hunk.added))
        .map(|line| line.len() + LINE_COST)
        .sum();
    let held = old.len() + new.len() + hunk_bytes;
    if held > room {
        return Texted::NotKept(too_large);
    }
    match Shape::new(base, hunks) {
        Ok(shape) => Texted::Kept(
            Content::Text {
                shape: Arc::new(shape),
                old: old.into(),
                new: new.into(),
            },
            held,
        ),
        Err(_) => Texted::NotKept(Content::Unreadable("its diff did not hold together".into())),
    }
}
