//! The review thread's work (ADR 0122): what it keeps between requests, and reading one file's
//! sides into a reading.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs git and reads files.
//!
//! A reading is kept per path with its signature: the old side's blob and mode, and the new
//! side's blob or, for a work tree file, its stamp or a link's target. A refresh reads again
//! only the files whose signature changed, and the bytes of the sides kept are counted against
//! [`MAX_HELD`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use oso_core::review::{Base, Shape};
use oso_platform::file_meta::Directory;

use super::batch::{Batch, Blob};
use super::diff;
use super::git::{Git, GitError, Id, MAX_SHORT};
use super::index::Scratch;
use super::list::{Entry, MODE_SUBMODULE, Status};
use super::scope::{self, New, Scope, ScopeError, Sides};
use super::side::{self, Found, Got};
use super::{Content, Reading, Report, Request};

/// The most bytes of sides a review keeps, over all its files.
pub(crate) const MAX_HELD: usize = 64 * 1024 * 1024;
/// How long one file's diff may take.
const DIFF_DEADLINE: Duration = Duration::from_secs(1);
/// How long carrying one file's comments may spend matching lines.
const CARRY_DEADLINE: Duration = Duration::from_secs(1);

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
    Stamp(side::Stamp),
    Link(Box<[u8]>),
    Nothing,
}

struct Kept {
    signature: Signature,
    held: usize,
}

/// Where the repository is, once found.
struct Repository {
    git: Git,
    /// The repository's own index.
    index: PathBuf,
}

/// The thread's state.
pub(super) struct Reader {
    root: PathBuf,
    repository: Option<Repository>,
    scratch: Option<Scratch>,
    batch: Batch,
    sides: Option<Sides>,
    kept: HashMap<Box<[u8]>, Kept>,
    held: usize,
    generation: u64,
}

type Out<'a> = dyn FnMut(Report) -> bool + 'a;

impl Reader {
    /// State for the project at `root`, with index copies under `state`, swept now.
    pub(super) fn new(root: &Path, state: Option<&Path>) -> Reader {
        let scratch = state.and_then(|state| {
            Scratch::open(state)
                .map_err(|e| log::warn!("reviews have no private place for index copies: {e}"))
                .ok()
        });
        Reader {
            root: root.to_owned(),
            repository: None,
            scratch,
            batch: Batch::default(),
            sides: None,
            kept: HashMap::new(),
            held: 0,
            generation: 0,
        }
    }

    pub(super) fn answer(&mut self, request: Request, out: &mut Out<'_>) {
        let failed = match request {
            Request::Open(scope) => self.open(&scope, &[], out),
            Request::Refresh { first } => self.refresh(&first, out),
            Request::Baseline => self.baseline(out),
            Request::Carry(carry) => {
                self.carry(carry, out);
                Ok(())
            }
        };
        if let Err(e) = failed {
            out(match e {
                ScopeError::BaselineGone => Report::BaselineGone,
                e => Report::Failed(message(&e)),
            });
        }
    }

    fn repository(&mut self) -> Result<&Repository, ScopeError> {
        if self.repository.is_none() {
            self.repository = Some(find_repository(&self.root)?);
        }
        self.repository
            .as_ref()
            .ok_or(ScopeError::Git(GitError::Unexpected("no repository")))
    }

    fn open(
        &mut self,
        scope: &Scope,
        first: &[Box<[u8]>],
        out: &mut Out<'_>,
    ) -> Result<(), ScopeError> {
        let sides = scope::resolve(&self.repository()?.git, scope)?;
        if self.sides.as_ref() != Some(&sides) {
            self.kept.clear();
            self.held = 0;
        }
        self.sides = Some(sides);
        self.refresh(first, out)
    }

    fn refresh(&mut self, first: &[Box<[u8]>], out: &mut Out<'_>) -> Result<(), ScopeError> {
        let Some(sides) = self.sides.clone() else {
            return Err(ScopeError::Git(GitError::Unexpected("no scope opened")));
        };
        let (git, index) = {
            let repository = self.repository()?;
            (
                repository.git.moved(repository.git.top().to_owned()),
                repository.index.clone(),
            )
        };
        let listing = scope::list(&git, &sides, &index, self.scratch.as_ref())?;
        self.generation += 1;
        let generation = self.generation;
        let listed: std::collections::HashSet<&[u8]> =
            listing.entries.iter().map(|entry| &*entry.path).collect();
        let gone: Vec<Box<[u8]>> = self
            .kept
            .keys()
            .filter(|path| !listed.contains(&***path))
            .cloned()
            .collect();
        for path in gone {
            self.forget(&path);
        }
        let entries = listing.entries;
        if !out(Report::Listed {
            generation,
            entries: entries.clone(),
            more: listing.more,
            cut: listing.cut,
        }) {
            return Ok(());
        }
        let top = Directory::open(git.top()).ok();
        let order = entries
            .iter()
            .filter(|entry| first.contains(&entry.path))
            .chain(entries.iter().filter(|entry| !first.contains(&entry.path)));
        for entry in order {
            let Some(reading) = self.read(entry, &sides.new, top.as_ref()) else {
                continue;
            };
            if !out(Report::File {
                generation,
                reading: Arc::new(reading),
            }) {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Lets go of what is kept of `path`.
    fn forget(&mut self, path: &[u8]) {
        if let Some(kept) = self.kept.remove(path) {
            self.held -= kept.held;
        }
    }

    /// Reads `entry` again, or `None` if its signature did not change.
    fn read(&mut self, entry: &Entry, new: &New, top: Option<&Directory>) -> Option<Reading> {
        let mut found = None;
        let new_sign = match new {
            New::WorkTree { .. } => match top.map(|top| side::find(top, &entry.path)) {
                Some(Ok(Found::File(file, stamp))) => {
                    found = Some((file, stamp.size));
                    NewSign::Stamp(stamp)
                }
                Some(Ok(Found::Link(target))) => NewSign::Link(target.into()),
                Some(Ok(Found::Nothing)) | None | Some(Err(_)) => NewSign::Nothing,
            },
            New::Index | New::Tree(_) => NewSign::Blob(entry.new_id.clone()),
        };
        let signature = Signature {
            old: entry.old_id.clone(),
            old_mode: entry.old_mode,
            new_mode: entry.new_mode,
            new: new_sign,
        };
        if self
            .kept
            .get(&entry.path)
            .is_some_and(|kept| kept.signature == signature)
        {
            return None;
        }
        self.forget(&entry.path);
        let content = self.content(entry, &signature.new, found);
        let held = match &content {
            Content::Text { old, new, .. } => old.len() + new.len(),
            _ => 0,
        };
        self.held += held;
        self.kept
            .insert(entry.path.clone(), Kept { signature, held });
        Some(Reading {
            entry: entry.clone(),
            content,
        })
    }

    fn content(
        &mut self,
        entry: &Entry,
        new_sign: &NewSign,
        found: Option<(std::fs::File, u64)>,
    ) -> Content {
        let Some(repository) = &self.repository else {
            return Content::Unreadable("no repository".into());
        };
        if side::may_hold_secrets(repository.git.top(), &self.root, &entry.path) {
            return Content::Withheld;
        }
        if entry.old_mode == MODE_SUBMODULE || entry.new_mode == MODE_SUBMODULE {
            return Content::Submodule;
        }
        let git = &repository.git;
        let old = if entry.status == Status::Added || entry.old_id.is_zero() {
            Got::Bytes(Vec::new())
        } else {
            match self.batch.read(git, &entry.old_id) {
                Ok(Blob::Bytes(bytes)) => Got::Bytes(bytes),
                Ok(Blob::TooLarge(size)) => Got::TooLarge(size),
                Ok(Blob::Missing) => return Content::Unreadable("its old side is missing".into()),
                Err(e) => return Content::Unreadable(e.to_string()),
            }
        };
        let new = match (new_sign, found) {
            _ if entry.status == Status::Deleted => Got::Bytes(Vec::new()),
            (NewSign::Stamp(_), Some((file, size))) => match side::read(file, size) {
                Ok(got) => got,
                Err(e) => return Content::Unreadable(format!("cannot read it: {e}")),
            },
            (NewSign::Link(target), _) => Got::Bytes(target.to_vec()),
            (NewSign::Nothing | NewSign::Stamp(_), _) => Got::Bytes(Vec::new()),
            (NewSign::Blob(id), _) => match self.batch.read(git, id) {
                Ok(Blob::Bytes(bytes)) => Got::Bytes(bytes),
                Ok(Blob::TooLarge(size)) => Got::TooLarge(size),
                Ok(Blob::Missing) => return Content::Unreadable("its new side is missing".into()),
                Err(e) => return Content::Unreadable(e.to_string()),
            },
        };
        let base = if entry.status == Status::Added || entry.old_id.is_zero() {
            Base::Empty
        } else {
            Base::Blob(entry.old_id.as_str().as_bytes().into())
        };
        text(entry, base, old, new, MAX_HELD.saturating_sub(self.held))
    }

    fn baseline(&mut self, out: &mut Out<'_>) -> Result<(), ScopeError> {
        self.repository()?;
        let (Some(repository), Some(scratch)) = (&self.repository, &self.scratch) else {
            return Err(ScopeError::NoScratch);
        };
        let (tree, left_out) = scope::baseline(&repository.git, &repository.index, scratch)?;
        if left_out > 0 {
            log::warn!("a review's baseline left out {left_out} files git could not read");
        }
        out(Report::Baseline { tree, left_out });
        Ok(())
    }

    fn carry(&mut self, carry: oso_core::review::Carry, out: &mut Out<'_>) {
        let deadline = Instant::now() + CARRY_DEADLINE;
        let Ok(repository) = self.repository() else {
            return;
        };
        let git = repository.git.moved(repository.git.top().to_owned());
        let batch = &mut self.batch;
        let carried = carry.run(
            |a, b| {
                let read = |batch: &mut Batch, id: &[u8]| match batch.read(&git, &Id::parse(id)?) {
                    Ok(Blob::Bytes(bytes)) => Some(bytes),
                    _ => None,
                };
                let old = read(batch, a)?;
                let new = read(batch, b)?;
                diff::base_hunks(&old, &new, deadline)
            },
            |a, b| diff::matches(a, b, deadline),
        );
        out(Report::Carried(carried));
    }
}

/// The sides as a reading's content: the text and its hunks, or why there are none. `room` is
/// what may still be held.
fn text(entry: &Entry, base: Base, old: Got, new: Got, room: usize) -> Content {
    let size = |got: &Got| match got {
        Got::Bytes(bytes) => bytes.len() as u64,
        Got::TooLarge(size) => *size,
    };
    let (old_size, new_size) = (size(&old), size(&new));
    let (Got::Bytes(old), Got::Bytes(new)) = (old, new) else {
        return Content::TooLarge {
            old: old_size,
            new: new_size,
        };
    };
    if entry.binary || side::is_binary(&old) || side::is_binary(&new) {
        return Content::Binary {
            old: old_size,
            new: new_size,
        };
    }
    if old.len() + new.len() > room {
        return Content::TooLarge {
            old: old_size,
            new: new_size,
        };
    }
    let hunks = diff::hunks(
        &diff::lines(&old),
        &diff::lines(&new),
        Instant::now() + DIFF_DEADLINE,
    );
    let Some(hunks) = hunks else {
        return Content::TooLarge {
            old: old_size,
            new: new_size,
        };
    };
    match Shape::new(base, hunks) {
        Ok(shape) => Content::Text {
            shape: Arc::new(shape),
            old: old.into(),
            new: new.into(),
        },
        Err(_) => Content::Unreadable("its diff did not hold together".into()),
    }
}

/// The repository whose work tree holds `root`: git to run in its top, and its index.
fn find_repository(root: &Path) -> Result<Repository, ScopeError> {
    let git = Git::new(root.to_owned(), root)?;
    let args = ["rev-parse", "--show-toplevel", "--absolute-git-dir"].map(std::ffi::OsStr::new);
    let out = git.run(&args, &[], MAX_SHORT)?;
    let mut lines = out.split(|&byte| byte == b'\n');
    let (Some(top), Some(git_dir)) = (lines.next(), lines.next()) else {
        return Err(GitError::Unexpected("no work tree").into());
    };
    let (top, git_dir) = (path_of(top)?, path_of(git_dir)?);
    Ok(Repository {
        index: git_dir.join("index"),
        git: git.moved(top),
    })
}

#[cfg(unix)]
fn path_of(bytes: &[u8]) -> Result<PathBuf, ScopeError> {
    use std::os::unix::ffi::OsStrExt;
    if bytes.is_empty() {
        return Err(GitError::Unexpected("an empty path").into());
    }
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_of(bytes: &[u8]) -> Result<PathBuf, ScopeError> {
    let text = std::str::from_utf8(git_trim(bytes))
        .map_err(|_| ScopeError::Git(GitError::Unexpected("a path that is not UTF-8")))?;
    if text.is_empty() {
        return Err(GitError::Unexpected("an empty path").into());
    }
    Ok(PathBuf::from(text))
}

#[cfg(not(unix))]
fn git_trim(bytes: &[u8]) -> &[u8] {
    super::git::trim_line(bytes)
}

/// What a person reads for `error`, with no file's text.
fn message(error: &ScopeError) -> String {
    match error {
        ScopeError::Git(e) => e.to_string(),
        ScopeError::NoSuchRevision(revision) => format!("no commit is named {revision:?}"),
        ScopeError::BaselineGone => "the session's baseline is gone".into(),
        ScopeError::NoScratch => "there is no private place for a copy of the index".into(),
        ScopeError::Copy(e) => format!("cannot copy the index: {e}"),
    }
}
