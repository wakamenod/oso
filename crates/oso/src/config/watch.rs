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
    Read { loaded: Loaded, group_writes: bool },
}

/// What the file was when last looked at, to tell when to read it again.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
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
    /// Called with the lock held: it only posts an event, and never calls back here.
    wake: Option<Box<dyn Fn() + Send>>,
    poked: bool,
    stopped: bool,
}

/// What the thread last found, to tell when to read the file again.
struct Last {
    seen: Seen,
    refused: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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

    /// What the last look found, if it found something new since this was last asked.
    pub fn take(&self) -> Option<Report> {
        self.shared.lock().report.take()
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

/// The thread: reads `file` at once, and again whenever it is found changed.
fn run(file: &Path, shared: &Shared) {
    let registry = commands::registry();
    let defaults = bindings::global_keymap(&registry);
    let themes: Vec<&str> = oso_render::theme::themes()
        .iter()
        .map(|theme| theme.name())
        .collect();
    let mut last: Option<Last> = None;
    let mut poked = false;
    loop {
        let now = look(file);
        let retry = poked && last.as_ref().is_some_and(|last| last.refused);
        if retry || last.as_ref().is_none_or(|last| last.seen != now) {
            let report = match &now {
                Seen::Missing => Report::Missing,
                Seen::Stamp(_) | Seen::Unknown(_) => {
                    // The loader and the saver catch their own panics too: one here would
                    // leave the file unread from then on, with nothing said.
                    catch_unwind(AssertUnwindSafe(|| {
                        read_and_parse(file, &registry, &defaults, &themes)
                    }))
                    .unwrap_or_else(|_| Report::Refused("reading it failed".to_owned()))
                }
            };
            let refused = matches!(report, Report::Refused(_));
            last = Some(Last { seen: now, refused });
            deliver(shared, report);
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

/// Reads `file` and parses it.
fn read_and_parse(
    file: &Path,
    registry: &oso_core::command::Registry,
    defaults: &oso_core::keymap::Keymap,
    themes: &[&str],
) -> Report {
    match read(file) {
        Ok((text, group_writes)) => Report::Read {
            loaded: parse(&text, registry, defaults, themes),
            group_writes,
        },
        Err(Unread::Missing) => Report::Missing,
        Err(Unread::Refused(why)) => Report::Refused(why),
    }
}

/// Hands `report` to the app, and wakes it.
fn deliver(shared: &Shared, report: Report) {
    let untaken = {
        let mut state = shared.lock();
        let untaken = state.report.replace(report);
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
fn look(file: &Path) -> Seen {
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
enum Unread {
    Missing,
    Refused(String),
}

/// The text of `file`, through links, if it passes the checks: a regular file that only this
/// user or root can change, in a directory that only they can, of at most [`MAX_BYTES`] of
/// UTF-8 (ADR 0108). With whether its group, or its directory's, may change it.
fn read(file: &Path) -> Result<(String, bool), Unread> {
    let refused = |why: &str| Err(Unread::Refused(why.to_owned()));
    let (mut opened, metadata) = match open_for_reading(file) {
        Ok(Some(opened)) => opened,
        Ok(None) => return refused("it is not a regular file"),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(Unread::Missing),
        Err(e) => return Err(Unread::Refused(format!("it cannot be opened: {e}"))),
    };
    let group_writes = match writers(file, &metadata)? {
        Writers::Nobody => false,
        Writers::Group => true,
        Writers::Anyone => return refused("other users can change it"),
    };
    if metadata.len() > MAX_BYTES {
        return Err(Unread::Refused(format!(
            "it is larger than {} KiB",
            MAX_BYTES / 1024
        )));
    }
    let mut bytes = Vec::new();
    if let Err(e) = (&mut opened).take(MAX_BYTES + 1).read_to_end(&mut bytes) {
        return Err(Unread::Refused(format!("it cannot be read: {e}")));
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Unread::Refused(format!(
            "it is larger than {} KiB",
            MAX_BYTES / 1024
        )));
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return refused("it is not UTF-8 text");
    };
    Ok((text, group_writes))
}

/// Who besides this user and root can change the file opened from `file`, whose `metadata` that
/// is, or the directory it is named in, or the one it really is in, through links: a link must
/// not take the file out of a directory other users can change. The file the path leads to is
/// checked to be the one opened, so the directory judged is the one that holds it.
fn writers(file: &Path, metadata: &fs::Metadata) -> Result<Writers, Unread> {
    let refused = |why: &str| Err(Unread::Refused(why.to_owned()));
    let Ok(resolved) = fs::canonicalize(file) else {
        return refused("its directory cannot be looked at");
    };
    match fs::metadata(&resolved) {
        Ok(now) if same_file(&now, metadata) => {}
        _ => return refused("it changed as it was read"),
    }
    let mut worst = privacy::others_can_write(metadata);
    for directory in [file.parent(), resolved.parent()] {
        let Some(Ok(directory)) = directory.map(fs::metadata) else {
            return refused("its directory cannot be looked at");
        };
        match privacy::others_can_write(&directory) {
            Writers::Anyone => return refused("other users can change the directory it is in"),
            Writers::Group => worst = worst.max(Writers::Group),
            Writers::Nobody => {}
        }
    }
    Ok(worst)
}

#[cfg(test)]
mod tests;
