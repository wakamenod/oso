//! The record of trusted projects, `trusted-projects` beside `config.toml` (ADR 0114): a thread
//! of its own, `oso-trust`, reads it at start, once a second when it changed and when asked, as
//! `watch` reads the configuration, and makes what a person changed in it. A change is made in
//! the file as it is then, read again, so that a line written by hand since stays. Reads and
//! writes take turns on the one thread, and each write is read back and reported, so the editor
//! always ends up holding what the file says.
//!
//! Must not depend on: `app` or `dispatch`; it says what it found through the function it is
//! given, and the app hands it on.
//!
//! The file decides what runs, so it is read only when no one else can change it or the way
//! to it, a group included. A refused or broken file trusts nothing, and is never written
//! over: the commands that change it refuse until it is read. A file that is a link, as a
//! dotfiles setup makes, is read where it leads but never written: that would put a file in the
//! link's place.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Write as _};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use oso_core::project::{TrustChange, TrustList};
use oso_platform::file_meta::create_private;
use oso_platform::privacy::{self, Writers};

use super::checked::{Unread, read};
use super::watch::{Seen, look};

/// The file's name in oso's configuration directory.
pub const TRUST_FILE_NAME: &str = "trusted-projects";
/// Where a write goes first, so that the file is whole or not there, with the process's id
/// after it, so that two instances never rename each other's.
const PART: &str = "trusted-projects.part";

/// How often the file is looked at, as the configuration is.
const INTERVAL: Duration = Duration::from_secs(1);

/// What a look at the record found.
#[derive(Debug, Default)]
pub struct TrustLook {
    /// The record as it is now, or `None` if only something is to be said.
    pub record: Option<Record>,
    /// What to say in `*Messages*`, the oldest first.
    pub said: Vec<String>,
}

/// What the file holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Record {
    /// It holds these roots. A missing file holds none.
    Read(TrustList),
    /// It was refused, or reading it failed: nothing is trusted.
    Refused,
}

/// The record's thread, which stops when this is dropped.
pub struct TrustFile {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Told when the thread has made the changes it took.
    made: Condvar,
}

#[derive(Default)]
struct State {
    /// What the looks found, until it is taken.
    look: Option<TrustLook>,
    /// What a person changed, the oldest first, until the thread takes it to make in the file.
    changes: Vec<TrustChange>,
    /// Whether the thread is making changes it took.
    making: bool,
    /// Called with the lock held: it only posts an event, and never calls back here.
    wake: Option<Box<dyn Fn() + Send>>,
    poked: bool,
    stopped: bool,
}

/// What the thread last said, to say it again only when it changes.
#[derive(Debug, PartialEq, Eq)]
enum Said {
    Missing,
    Refused(String, Seen),
    Read(u64),
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl TrustFile {
    /// Starts reading `file`, at once. `None` for no file, or if the thread cannot start,
    /// which is logged: nothing is trusted.
    pub fn start(file: Option<PathBuf>) -> Option<Self> {
        let Some(file) = file else {
            log::info!("no record of trusted projects to read: none is trusted");
            return None;
        };
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("oso-trust".into())
            .spawn(move || run(&file, &thread_shared));
        if let Err(e) = spawned {
            log::warn!("cannot read the record of trusted projects: {e}");
            return None;
        }
        Some(Self { shared })
    }

    /// Calls `wake` whenever a look finds something new to take, and now if it already has.
    pub fn set_wake(&self, wake: impl Fn() + Send + 'static) {
        let mut state = self.shared.lock();
        if state.look.is_some() {
            wake();
        }
        state.wake = Some(Box::new(wake));
    }

    /// What the looks found since this was last asked, if anything.
    pub fn take(&self) -> Option<TrustLook> {
        self.shared.lock().look.take()
    }

    /// Looks at the file now: oso gained the focus, or saved a file. A file refused is read
    /// again whatever it looks like.
    pub fn look_again(&self) {
        self.shared.lock().poked = true;
        self.shared.changed.notify_one();
    }

    /// Makes `changes` in the file as it is then, soon, and reads it back.
    pub fn change(&self, changes: Vec<TrustChange>) {
        if changes.is_empty() {
            return;
        }
        self.shared.lock().changes.extend(changes);
        self.shared.changed.notify_one();
    }
}

impl TrustFile {
    /// Waits up to `within` for the changes handed over to be made in the file: oso is going,
    /// and a change the person was told of must not be lost with the process.
    pub fn finish(&self, within: std::time::Duration) {
        let deadline = std::time::Instant::now() + within;
        let mut state = self.shared.lock();
        while !state.changes.is_empty() || state.making {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                log::warn!("a change to the record of trusted projects may not be written");
                return;
            }
            state = self
                .shared
                .made
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for TrustFile {
    fn drop(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.changed.notify_one();
    }
}

/// The thread. A panic anywhere on it trusts nothing, is said, and ends it.
fn run(file: &Path, shared: &Shared) {
    if catch_unwind(AssertUnwindSafe(|| watch(file, shared))).is_err() {
        let failed = TrustLook {
            record: Some(Record::Refused),
            said: vec![format!(
                "{TRUST_FILE_NAME}: reading it failed, so no project is trusted"
            )],
        };
        let _ = catch_unwind(AssertUnwindSafe(|| deliver(shared, failed)));
    }
}

/// Reads `file` whenever it is found changed or a look is asked for, and says what is new;
/// makes the changes it is given, and reads the file back.
fn watch(file: &Path, shared: &Shared) {
    let mut seen: Option<Seen> = None;
    let mut said: Option<Said> = None;
    let mut poked = false;
    let mut changes: Vec<TrustChange> = Vec::new();
    loop {
        let mut notes = Vec::new();
        let wrote = !changes.is_empty();
        if wrote && let Err(why) = make(file, &std::mem::take(&mut changes)) {
            notes.push(format!(
                "{TRUST_FILE_NAME} was not changed, and still says what it said: {why}"
            ));
        }
        let now = look(file);
        let refused = matches!(said, Some(Said::Refused(..)));
        if wrote || (poked && refused) || seen.as_ref() != Some(&now) {
            let (list, saying, note) = read_list(file, &now);
            seen = Some(now);
            let new = said.as_ref() != Some(&saying);
            if new {
                if let Some(note) = note {
                    notes.push(note);
                } else if refused {
                    notes.push(format!("{TRUST_FILE_NAME} is read again"));
                }
                said = Some(saying);
            }
            // Changes waiting will be read back after they are made: this is old already.
            let superseded = !shared.lock().changes.is_empty();
            if (wrote || new) && !superseded {
                let record = Some(list.map_or(Record::Refused, Record::Read));
                deliver(
                    shared,
                    TrustLook {
                        record,
                        said: notes,
                    },
                );
            } else if !notes.is_empty() {
                deliver_notes(shared, notes);
            }
        } else if !notes.is_empty() {
            deliver_notes(shared, notes);
        }
        let mut state = shared.lock();
        if wrote {
            state.making = false;
            shared.made.notify_all();
        }
        if !state.poked && !state.stopped && state.changes.is_empty() {
            state = shared
                .changed
                .wait_timeout(state, INTERVAL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        // What a person changed is made before the thread goes.
        if state.stopped && state.changes.is_empty() {
            return;
        }
        poked = std::mem::take(&mut state.poked);
        changes = std::mem::take(&mut state.changes);
        state.making = !changes.is_empty();
    }
}

/// The record `file` holds, which `now` describes, what that says, and what to tell the
/// person of it.
fn read_list(file: &Path, now: &Seen) -> (Option<TrustList>, Said, Option<String>) {
    let refused = |why: String| {
        let note = format!("{TRUST_FILE_NAME} is not read, so no project is trusted: {why}");
        (None, Said::Refused(why, now.clone()), Some(note))
    };
    if *now == Seen::Missing {
        return (Some(TrustList::default()), Said::Missing, None);
    }
    match read(file) {
        Ok((_, true)) => refused(
            "the users of a group can change it, or a directory or link on the way to it".into(),
        ),
        Ok((text, false)) => {
            let mut hash = DefaultHasher::new();
            text.hash(&mut hash);
            let (list, skipped) = TrustList::parse(&text);
            let note = (skipped > 0).then(|| {
                let lines = if skipped == 1 { "line" } else { "lines" };
                format!("{TRUST_FILE_NAME}: {skipped} {lines} could not be read, and trust nothing")
            });
            (Some(list), Said::Read(hash.finish()), note)
        }
        Err(Unread::Missing) => (Some(TrustList::default()), Said::Missing, None),
        Err(Unread::Refused(why)) => refused(why),
    }
}

/// Hands `look` to the app, after what it has not taken yet, and wakes it.
fn deliver(shared: &Shared, look: TrustLook) {
    let mut state = shared.lock();
    let mut said = state.look.take().map(|old| old.said).unwrap_or_default();
    said.extend(look.said);
    state.look = Some(TrustLook {
        record: look.record,
        said,
    });
    if let Some(wake) = &state.wake {
        wake();
    }
}

/// Hands the app what to say, with the record it has, and wakes it.
fn deliver_notes(shared: &Shared, notes: Vec<String>) {
    let mut state = shared.lock();
    state
        .look
        .get_or_insert_with(TrustLook::default)
        .said
        .extend(notes);
    if let Some(wake) = &state.wake {
        wake();
    }
}

/// Makes `changes` in `file` as it is now: read with the checks a look makes, changed, and
/// written whole. Why not, if it is not.
fn make(file: &Path, changes: &[TrustChange]) -> Result<(), String> {
    for _ in 0..ATTEMPTS {
        let seen = look(file);
        let (list, said, _) = read_list(file, &seen);
        let Some(mut list) = list else {
            return Err(match said {
                Said::Refused(why, _) => why,
                _ => "it is not read".to_owned(),
            });
        };
        for change in changes {
            list = list.apply(change).map_err(|why| why.to_string())?;
        }
        match write_file(file, &list, &seen) {
            Ok(Written::Replaced) => return Ok(()),
            // Another writer replaced it since it was read: made again on what it holds now.
            Ok(Written::Moved) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("it kept changing as it was written".to_owned())
}

/// How many times a change is made again on a file that another writer replaced meanwhile.
const ATTEMPTS: usize = 3;

/// How old another process's part file must be to be taken for one a crash left.
const STALE_PART: Duration = Duration::from_secs(60);

/// What a write did.
enum Written {
    /// The file holds the list now.
    Replaced,
    /// The file changed since it was read as `seen`, so nothing was written.
    Moved,
}

/// Writes `list` in place of `file`, read as `seen`: whole, through a part file renamed over
/// it, mode `0600`, in a directory only this user can change, which is made so if missing once
/// what it would be made in passes, and synced after the rename. Never over a link, which the
/// rename would replace, and not if `file` is no longer what `seen` says just before the rename.
fn write_file(file: &Path, list: &TrustList, seen: &Seen) -> io::Result<Written> {
    let refused = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            why.to_owned(),
        ))
    };
    let directory = file
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "it has no directory"))?;
    match fs::symlink_metadata(file) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return refused("it is a link; change the file it leads to by hand");
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let others = |place: &Path| -> io::Result<bool> {
        let who = privacy::who_can_change(place)?;
        Ok(who.file != Writers::Nobody || who.way != Writers::Nobody)
    };
    let mut existing = directory;
    while let Err(e) = fs::symlink_metadata(existing) {
        match existing.parent() {
            Some(parent) if e.kind() == io::ErrorKind::NotFound => existing = parent,
            _ => return Err(e),
        }
    }
    let changeable = "other users can change its directory, or the way to it";
    if others(existing)? {
        return refused(changeable);
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(directory)?;
    if others(directory)? {
        return refused(changeable);
    }
    remove_stale_parts(directory);
    let part = directory.join(format!("{PART}.{}", std::process::id()));
    match fs::remove_file(&part) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let written = (|| {
        let mut opened = create_private(&part)?;
        opened.write_all(list.format().as_bytes())?;
        opened.sync_all()?;
        if look(file) != *seen {
            return Ok(Written::Moved);
        }
        fs::rename(&part, file)?;
        crate::saver::sync_directory(directory)?;
        Ok(Written::Replaced)
    })();
    if !matches!(written, Ok(Written::Replaced)) {
        let _ = fs::remove_file(&part);
    }
    written
}

/// Removes the part files a crashed instance left in `directory`: this user's, not this
/// process's, and older than [`STALE_PART`], so that one another instance is writing stays.
fn remove_stale_parts(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let own = format!("{PART}.{}", std::process::id());
    let prefix = format!("{PART}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(&prefix) || name == own {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let old = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_PART);
        if metadata.is_file() && privacy::is_this_users(&metadata) && old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests;
