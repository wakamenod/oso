//! The thread that reads the user configuration file at start, and again when it changes on
//! disk, checks it and parses it (ADR 0108, ADR 0110). It looks once a second, and when asked,
//! as files changed on disk are looked for (ADR 0083).
//!
//! Must not depend on: `app` or `dispatch`; it says what it found through the function it is
//! given, and the app applies it.
//!
//! It is not the loader's watcher: it reads and parses on its own thread, with the stack a
//! crafted file needs (`config::STACK`), and keeps one file, read as a whole. It looks as that
//! watcher does: by the same file stamp, once a second, and when asked, which also tries a file
//! it refused again, as a directory's mode may have changed without the file's.

use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::Duration;

use oso_core::buffer::FileStamp;
use oso_platform::file_meta::open_for_reading;
use oso_platform::privacy::{self, Writers};

use super::{Loaded, STACK, Source, parse};
use crate::loader::{same_file, stamp_of};
use crate::{bindings, commands};

/// The largest file read (ADR 0110).
pub const MAX_BYTES: u64 = 256 * 1024;

/// How often the file is looked at, as files changed on disk are (ADR 0083).
const INTERVAL: Duration = Duration::from_secs(1);

/// What a look at the file found.
#[derive(Debug)]
pub enum Report {
    /// There is no file: the compiled-in defaults apply.
    Missing,
    /// The file was not read: it failed a check, or reading it did. What was in force stays.
    Refused(String),
    /// The file was read and parsed. `group_writes` if the users of its group, or of its
    /// directory's, may change it, which is said beside what it holds.
    Read {
        loaded: Box<Loaded>,
        group_writes: bool,
    },
}

/// What the file was when last looked at, to tell when to read it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Seen {
    Missing,
    Stamp(FileStamp),
    /// It could not be looked at, for this reason.
    Unknown(io::ErrorKind),
}

/// The configuration's thread, which stops when this is dropped.
pub struct Reader {
    shared: Arc<Shared>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    /// What the last look found, until it is taken.
    report: Option<Report>,
    /// Whether that is the launch's: the first look, which no later one replaced before it was
    /// taken. Only it starts the agent channel (ADR 0108).
    launch: bool,
    /// Whether a look has said anything yet.
    delivered: bool,
    /// Called with the lock held: it only posts an event, and never calls back here.
    wake: Option<Box<dyn Fn() + Send>>,
    poked: bool,
    stopped: bool,
}

/// What the thread last said, to say it only when it changes: a refusal again only for a new
/// reason or a changed file, and a file read again only for new text, so that a look asked for
/// on every focus and save says nothing new.
#[derive(PartialEq, Eq)]
enum Said {
    Missing,
    Refused(String, Seen),
    Read(u64),
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn take(&self) -> Option<(Report, bool)> {
        let mut state = self.lock();
        let report = state.report.take()?;
        Some((report, std::mem::take(&mut state.launch)))
    }
}

impl Reader {
    /// Starts reading `source`'s file, at once. `None` for no file, or if the thread cannot
    /// start, which is logged: the defaults apply.
    pub fn start(source: &Source) -> Option<Self> {
        let Some(file) = source.file() else {
            if *source == Source::Home {
                log::info!("no configuration directory to read: the defaults apply");
            }
            return None;
        };
        let shared = Arc::new(Shared::default());
        let thread_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("oso-config".into())
            .stack_size(STACK)
            .spawn(move || run(&file, &thread_shared));
        if let Err(e) = spawned {
            log::warn!("cannot read the configuration: {e}");
            return None;
        }
        Some(Self { shared })
    }

    /// Calls `wake` whenever a look finds something new to take, and now if it already has.
    pub fn set_wake(&self, wake: impl Fn() + Send + 'static) {
        let mut state = self.shared.lock();
        if state.report.is_some() {
            wake();
        }
        state.wake = Some(Box::new(wake));
    }

    /// What the last look found, if it found something new since this was last asked, and
    /// whether it is the launch's: the first look, which no later one replaced before it was
    /// taken. A look done before the app takes any can replace a bad first one, and the file
    /// it found fixed is not the launch's.
    pub fn take(&self) -> Option<(Report, bool)> {
        self.shared.take()
    }

    /// Looks at the file now: oso gained the focus, or saved a file. A file refused is read
    /// again whatever it looks like.
    pub fn look_again(&self) {
        self.shared.lock().poked = true;
        self.shared.changed.notify_one();
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.shared.lock().stopped = true;
        self.shared.changed.notify_one();
    }
}

/// The thread: reads `file` at once, and again whenever it is found changed. A panic anywhere
/// on it, as the loader and the saver catch theirs, is said, and ends it.
fn run(file: &Path, shared: &Shared) {
    if catch_unwind(AssertUnwindSafe(|| watch(file, shared))).is_err() {
        let refused = Report::Refused("reading it failed".to_owned());
        let _ = catch_unwind(AssertUnwindSafe(|| deliver(shared, refused)));
    }
}

/// Reads `file` whenever it is found changed, or a look is asked for, and says what is new.
fn watch(file: &Path, shared: &Shared) {
    let registry = commands::registry();
    let defaults = bindings::global_keymap(&registry);
    let themes: Vec<&str> = oso_render::theme::themes()
        .iter()
        .map(|theme| theme.name())
        .collect();
    let mut seen: Option<Seen> = None;
    let mut said: Option<Said> = None;
    let mut poked = false;
    loop {
        let now = look(file);
        let refused = matches!(said, Some(Said::Refused(..)));
        if (poked && refused) || seen.as_ref() != Some(&now) {
            let (report, saying) = match &now {
                Seen::Missing => (Report::Missing, Said::Missing),
                Seen::Stamp(_) | Seen::Unknown(_) => {
                    match read_and_parse(file, &registry, &defaults, &themes) {
                        (Report::Refused(why), _) => (
                            Report::Refused(why.clone()),
                            Said::Refused(why, now.clone()),
                        ),
                        (Report::Missing, _) => (Report::Missing, Said::Missing),
                        (report, text) => (report, Said::Read(text)),
                    }
                }
            };
            seen = Some(now);
            if said.as_ref() != Some(&saying) {
                said = Some(saying);
                deliver(shared, report);
            }
        }
        let mut state = shared.lock();
        if !state.poked && !state.stopped {
            state = shared
                .changed
                .wait_timeout(state, INTERVAL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        if state.stopped {
            return;
        }
        poked = std::mem::take(&mut state.poked);
    }
}

/// Reads `file` and parses it, with a hash of what was read, to tell new text from old.
fn read_and_parse(
    file: &Path,
    registry: &oso_core::command::Registry,
    defaults: &oso_core::keymap::Keymap,
    themes: &[&str],
) -> (Report, u64) {
    let read = catch_unwind(AssertUnwindSafe(|| match read(file) {
        Ok((text, group_writes)) => {
            let mut hash = DefaultHasher::new();
            (&text, group_writes).hash(&mut hash);
            let mut loaded = parse(&text, registry, defaults, themes);
            if may_hold_secrets(file) {
                for problem in &mut loaded.problems {
                    problem.message = WITHHELD.to_owned();
                }
            }
            let report = Report::Read {
                loaded: Box::new(loaded),
                group_writes,
            };
            (report, hash.finish())
        }
        Err(Unread::Missing) => (Report::Missing, 0),
        Err(Unread::Refused(why)) => (Report::Refused(why), 0),
    }));
    read.unwrap_or_else(|_| (Report::Refused("reading it failed".to_owned()), 0))
}

/// What a problem says in a file whose name, or where it leads, says it may hold secrets
/// (ADR 0044): its line and column, and none of its text.
const WITHHELD: &str = "a problem here, not quoted, as the file may hold secrets";

/// Whether `file`'s name, or where it leads, says it may hold secrets, as a buffer of it would
/// be judged.
fn may_hold_secrets(file: &Path) -> bool {
    let mut probe = oso_core::buffer::Buffer::visiting(file.to_path_buf());
    crate::loader::resolve(file).judge(&mut probe);
    probe.verdict().is_withheld()
}

/// Hands `report` to the app, and wakes it.
fn deliver(shared: &Shared, report: Report) {
    let untaken = {
        let mut state = shared.lock();
        let untaken = state.report.replace(report);
        state.launch = !state.delivered;
        state.delivered = true;
        if let Some(wake) = &state.wake {
            wake();
        }
        untaken
    };
    // A report the app did not take may hold a large keymap: it goes with the lock released,
    // so that the UI thread never waits for it.
    drop(untaken);
}

/// What `file` is now, through links: a change in its time, size, mode, device or inode, or its
/// coming or going, reads it again.
pub(super) fn look(file: &Path) -> Seen {
    match fs::metadata(file) {
        Ok(metadata) => match stamp_of(&metadata) {
            Ok(stamp) => Seen::Stamp(stamp),
            Err(e) => Seen::Unknown(e.kind()),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Seen::Missing,
        Err(e) => Seen::Unknown(e.kind()),
    }
}

/// Why the file was not read.
pub(super) enum Unread {
    Missing,
    Refused(String),
}

/// The text of `file`, through links, if it passes the checks (ADR 0108, ADR 0110): a regular
/// file of at most [`MAX_BYTES`] of UTF-8 that only this user or root can change, nor anything
/// on the way to it, through every link. The way is judged before the file is opened, and the
/// file opened must be the one judged. With whether the users of a group can change it or the
/// way.
pub(super) fn read(file: &Path) -> Result<(String, bool), Unread> {
    let refused = |why: &str| Err(Unread::Refused(why.to_owned()));
    let who = match privacy::who_can_change(file) {
        Ok(who) => who,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(_) => return refused("the way to it cannot be looked at"),
    };
    if who.file == Writers::Anyone {
        return refused("other users can change it");
    }
    if who.way == Writers::Anyone {
        return refused("other users can change a directory or link on the way to it");
    }
    if !who.metadata.is_file() {
        return refused("it is not a regular file");
    }
    let too_large = || {
        Err(Unread::Refused(format!(
            "it is larger than {} KiB",
            MAX_BYTES / 1024
        )))
    };
    if who.metadata.len() > MAX_BYTES {
        return too_large();
    }
    let mut opened = match open_for_reading(file) {
        Ok(Some((opened, metadata))) if same_file(&metadata, &who.metadata) => opened,
        Ok(_) => return refused("it changed as it was read"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(e) => return Err(Unread::Refused(format!("it cannot be opened: {e}"))),
    };
    let mut bytes = Vec::new();
    if let Err(e) = (&mut opened).take(MAX_BYTES + 1).read_to_end(&mut bytes) {
        return Err(Unread::Refused(format!("it cannot be read: {e}")));
    }
    if bytes.len() as u64 > MAX_BYTES {
        return too_large();
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return refused("it is not UTF-8 text");
    };
    Ok((text, who.file.max(who.way) == Writers::Group))
}

#[cfg(test)]
mod tests;
