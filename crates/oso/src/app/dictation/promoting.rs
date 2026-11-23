//! TypeWhisper's dictionary in the app (ADR 0175, step 4): the pairs learned often enough are
//! promoted to it, and what oso added is taken back, on the learning's thread. Nothing of it is
//! shown; each change is logged.
//!
//! Must not depend on: how a frame is drawn.
//!
//! - **A pass** over TypeWhisper's dictionary is asked for once a dictation was typed and once a
//!   watch ended. It calls TypeWhisper only when something is to be added or removed, and starts
//!   no more changes once a dictation is under way or the person forgets. After one TypeWhisper
//!   did not answer as it should, none is made for [`QUIET`].
//! - **The list** of what oso added is kept as the learning record is, and changed only under
//!   its lock, which one instance of oso holds at a time; another's pass is left out meanwhile. A
//!   list oso may not read, or that is not whole, is left as it is, also by a forget, and nothing
//!   is promoted.
//! - **`dictation-forget-learned`** marks every entry to go, which holds until TypeWhisper
//!   answers, in this instance or a later one. An empty file says that it has yet to, so that a
//!   later pass, in any instance, marks them if another kept the lock or this one quit.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use oso_platform::privacy::PrivateDir;

use super::App;
use super::learning::{self, Shared, Work};
use crate::dictation::api::Curl;
use crate::dictation::dictionary::list::{self, List};
use crate::dictation::dictionary::sync;
use crate::dictation::discovery::{self, Discovery};
use crate::dictation::learning::{self as record, Record};
use crate::dictation::state::Heard;
use crate::sessions::record::{read_file, replace_file};

/// How long a forget waits for another instance's pass, which is longer than one takes, and
/// between its looks.
pub(super) const FORGET_WAIT: Duration = Duration::from_secs(30);
const FORGET_PAUSE: Duration = Duration::from_millis(200);

/// How long after a pass TypeWhisper did not answer as it should the next is made.
const QUIET: Duration = Duration::from_secs(10 * 60);

/// Where TypeWhisper's API is found: its discovery file and curl, looked at on the thread.
#[derive(Clone, Debug)]
pub(super) struct Endpoint {
    discovery: PathBuf,
    curl: PathBuf,
}

/// What came of a forget of TypeWhisper's entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Taken {
    /// None of oso's is left there; this many are the person's now.
    All { kept: usize },
    /// This many go once TypeWhisper answers.
    Left(usize),
    /// What oso added is not known, for a reason the log has.
    Unknown,
}

/// What the learning's thread keeps of its passes.
#[derive(Default)]
pub(super) struct Passes {
    blocked: sync::Blocked,
    /// No pass is made before this.
    quiet: Option<Instant>,
}

impl App {
    /// TypeWhisper, as the configuration names it now.
    pub(super) fn typewhisper(&self) -> Option<Endpoint> {
        Some(Endpoint {
            discovery: self.discovery_path()?,
            curl: self.configuration.dictation().curl(),
        })
    }

    /// What was heard was typed, in no buffer that may hold secrets: the entries it used are
    /// noted, and a pass follows.
    pub(super) fn note_dictation_heard(&mut self, heard: Heard) {
        self.learn(Work::Heard(heard));
        self.promote_learned();
    }

    /// Asks for a pass over TypeWhisper's dictionary, unless a dictation is under way.
    pub(super) fn promote_learned(&mut self) {
        self.note_dictation_under_way();
        if !self.dictation_under_way()
            && let Some(endpoint) = self.typewhisper()
        {
            self.learn(Work::Promote(endpoint));
        }
    }

    /// Whether a dictation is under way, or its text waits to be typed.
    pub(super) fn dictation_under_way(&self) -> bool {
        self.dictations.due.is_some()
            || (self.dictations.current.as_ref()).is_some_and(|current| !current.cancelled)
    }

    /// Tells the learning's thread whether one is: a pass starts no more changes meanwhile.
    pub(super) fn note_dictation_under_way(&self) {
        let shared = &self.dictations.learning.shared;
        (shared.busy).store(self.dictation_under_way(), Ordering::Relaxed);
    }
}

/// The lock on the list, held while this is kept: `None` if another instance holds it.
fn lock(private: &PrivateDir) -> std::io::Result<Option<File>> {
    let path = private.path().join(list::LOCK);
    match std::fs::symlink_metadata(&path) {
        Ok(there) if !there.is_file() => {
            return Err(std::io::Error::other("its lock is not a regular file"));
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        _ => {}
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// The list in `directory`: empty if there is none.
///
/// # Errors
///
/// When there is one that may not be read, or is not whole.
fn read_list(directory: &Path, today: u32) -> std::io::Result<List> {
    match read_file(directory, list::FILE, record::MAX_BYTES)? {
        None => Ok(List::default()),
        Some(bytes) => List::parse(&bytes, today).ok_or_else(|| {
            std::io::Error::other(format!("{} is not whole: oso leaves it", list::FILE))
        }),
    }
}

/// Keeps `list`, or removes its file when nothing is in it. Says whether it did.
fn write_list(private: &PrivateDir, list: &List) -> bool {
    let written = if list.is_empty() {
        learning::remove(private, list::FILE)
    } else {
        let text = list.to_text();
        if text.len() as u64 > record::MAX_BYTES {
            Err(std::io::Error::other("it is too large"))
        } else {
            replace_file(private, list::FILE, list::PART, &text)
        }
    };
    written
        .inspect_err(|e| log::warn!("what oso added to TypeWhisper is not written: {e}"))
        .is_ok()
}

/// The directory, the lock on the list and the list, if this instance may change it now.
fn held(directory: &Path, today: u32) -> Option<(PrivateDir, File, List)> {
    let opened = PrivateDir::open(directory).and_then(|private| {
        let Some(held) = lock(&private)? else {
            return Ok(None);
        };
        let list = read_list(directory, today)?;
        Ok(Some((private, held, list)))
    });
    opened.unwrap_or_else(|e| {
        log::warn!("what oso added to TypeWhisper is not read: {e}");
        None
    })
}

/// Notes which entries a dictation used.
pub(super) fn note_used(directory: &Path, heard: &Heard, at: u64) {
    let today = record::day_of(at);
    // Looked at first with no lock, as most dictations use no entry, or one used today.
    let used = |list: &mut List| list.note_used(&heard.text, heard.raw.as_deref(), today);
    if !read_list(directory, today).is_ok_and(|mut list| used(&mut list)) {
        return;
    }
    if let Some((private, _lock, mut list)) = held(directory, today)
        && used(&mut list)
    {
        let _ = write_list(&private, &list);
    }
}

/// The connection to TypeWhisper, if it can be made.
fn connect(endpoint: &Endpoint) -> Option<(Discovery, Curl)> {
    let discovery = discovery::read(&endpoint.discovery).ok()?;
    Some((discovery, Curl::at(&endpoint.curl).ok()?))
}

fn read_record(directory: &Path, today: u32) -> Option<Record> {
    read_file(directory, record::FILE, record::MAX_BYTES)
        .inspect_err(|e| log::warn!("dictation's learning record is not read: {e}"))
        .ok()
        .map(|bytes| bytes.map_or_else(Record::default, |bytes| Record::parse(&bytes, today)))
}

/// A pass over TypeWhisper's dictionary, if there is something to ask it for: `made` forgets had
/// come when it was asked for.
pub(super) fn promote(
    directory: &Path,
    endpoint: &Endpoint,
    at: u64,
    passes: &mut Passes,
    shared: &Shared,
    made: u64,
) {
    if std::fs::symlink_metadata(directory.join(list::DUE)).is_ok() {
        let _ = forget(directory, Some(endpoint), at, passes, Duration::ZERO);
    }
    let stopped =
        || shared.busy.load(Ordering::Relaxed) || shared.forgets.load(Ordering::Relaxed) != made;
    // A longer name is none the list would be read back with.
    let server = endpoint.discovery.to_string_lossy();
    if stopped()
        || server.len() > list::MAX_SERVER
        || (passes.quiet).is_some_and(|quiet| Instant::now() < quiet)
    {
        return;
    }
    let today = record::day_of(at);
    let wanted = |list: &List, passes: &Passes| {
        read_record(directory, today)
            .filter(|learned| sync::wanted(list, learned, &server, today, &passes.blocked))
    };
    // Looked at first with no lock, as most passes have nothing to do.
    match read_list(directory, today) {
        Ok(list) if wanted(&list, passes).is_some() => {}
        Ok(_) => return,
        Err(e) => return log::warn!("nothing is promoted: {e}"),
    }
    let Some((private, _lock, mut list)) = held(directory, today) else {
        return;
    };
    // The record under the lock, as another instance may have forgotten it just now.
    let Some(learned) = wanted(&list, passes) else {
        return;
    };
    let Some((discovery, mut curl)) = connect(endpoint) else {
        return;
    };
    let out = sync::Pass {
        transport: &mut curl,
        discovery: &discovery,
        server: &server,
        today,
        blocked: &mut passes.blocked,
        keep: &mut |list| write_list(&private, list),
        stop: &stopped,
    }
    .run(&mut list, &learned);
    passes.quiet = out.stuck.then(|| Instant::now() + QUIET);
    if !out.done.is_empty() && shared.forgets.load(Ordering::Relaxed) == made {
        let lines: Vec<String> = (out.done.iter())
            .map(|done| record::log_done(done, at))
            .collect();
        learning::append_to_log(directory, &lines, at);
    }
}

/// Marks every entry oso added to go, and takes them from TypeWhisper if it answers. It waits
/// at most `wait` for another instance's pass, and then takes away once more what that may have
/// written of the record and the log.
pub(super) fn forget(
    directory: &Path,
    endpoint: Option<&Endpoint>,
    at: u64,
    passes: &mut Passes,
    wait: Duration,
) -> Taken {
    let today = record::day_of(at);
    let not = |e: &dyn std::fmt::Display| {
        log::warn!("what oso added to TypeWhisper is not forgotten: {e}");
        Taken::Unknown
    };
    let private = match PrivateDir::existing(directory) {
        Ok(Some(private)) => private,
        Ok(None) => return Taken::All { kept: 0 },
        Err(e) => return not(&e),
    };
    // Written first, as oso may quit before the list is marked.
    if let Err(e) = std::fs::write(private.path().join(list::DUE), "") {
        return not(&e);
    }
    let until = Instant::now() + wait;
    let _lock = loop {
        match lock(&private) {
            Ok(Some(lock)) => break lock,
            Ok(None) if Instant::now() < until => std::thread::sleep(FORGET_PAUSE),
            Ok(None) => return not(&"another oso changes it; a later dictation's pass does"),
            Err(e) => return not(&e),
        }
    };
    let _ = learning::forget_record(&private);
    let _ = learning::remove(&private, list::PART);
    let mut list = match read_list(directory, today) {
        Ok(list) => list,
        // Never removed: what oso added would be forgotten.
        Err(e) => {
            let _ = learning::remove(&private, list::DUE);
            return not(&e);
        }
    };
    list.forget();
    if !write_list(&private, &list) {
        return Taken::Unknown;
    }
    let _ = learning::remove(&private, list::DUE);
    passes.blocked.clear();
    passes.quiet = None;
    let mut kept = 0;
    if !list.is_empty()
        && let Some(endpoint) = endpoint
        && let Some((discovery, mut curl)) = connect(endpoint)
    {
        kept = sync::Pass {
            transport: &mut curl,
            discovery: &discovery,
            server: &endpoint.discovery.to_string_lossy(),
            today,
            blocked: &mut passes.blocked,
            keep: &mut |list| write_list(&private, list),
            stop: &|| false,
        }
        .run(&mut list, &Record::default())
        .kept;
    }
    match list.entries.len() {
        0 => Taken::All { kept },
        left => Taken::Left(left),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::dictation::dictionary::Kind;
    use crate::dictation::dictionary::list::{Entry, State};

    fn private(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "oso-promoting-{name}-{}-{}",
            std::process::id(),
            learning::now_secs()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let directory = root.join(record::DIRECTORY);
        PrivateDir::open(&directory).unwrap();
        directory
    }

    fn clean(directory: &Path) {
        let _ = std::fs::remove_dir_all(directory.parent().unwrap());
    }

    /// A list of one correction, last used on day 100.
    fn one() -> List {
        List {
            entries: vec![Entry {
                kind: Kind::Correction,
                wrong: "機能".into(),
                right: "昨日".into(),
                unsure: false,
                added: 100,
                used: 100,
                state: State::Live,
                server: "here".into(),
            }],
            let_go: Vec::new(),
        }
    }

    #[test]
    fn the_list_is_changed_only_under_its_lock_and_is_the_persons_alone() {
        use std::os::unix::fs::PermissionsExt;
        let directory = private("lock");
        let (private, held_lock, _) = held(&directory, 100).unwrap();
        assert!(write_list(&private, &one()));
        assert_eq!(read_list(&directory, 100).unwrap(), one());
        for name in [list::FILE, list::LOCK] {
            let mode = std::fs::metadata(directory.join(name)).unwrap();
            assert_eq!(mode.permissions().mode() & 0o777, 0o600, "{name}");
        }
        let heard = Heard {
            text: "昨日".into(),
            raw: Some("機能".into()),
        };
        assert!(held(&directory, 100).is_none(), "another holds it");
        note_used(&directory, &heard, 105 * 86_400);
        assert_eq!(read_list(&directory, 105).unwrap(), one());
        drop(held_lock);
        note_used(&directory, &heard, 105 * 86_400);
        assert_eq!(read_list(&directory, 105).unwrap().entries[0].used, 105);
        assert!(write_list(&private, &List::default()));
        assert!(!directory.join(list::FILE).exists(), "nothing to keep");
        clean(&directory);
    }

    fn nobody(directory: &Path) -> Endpoint {
        Endpoint {
            discovery: directory.join("none.json"),
            curl: "/usr/bin/curl".into(),
        }
    }

    const ALL: Taken = Taken::All { kept: 0 };

    #[test]
    fn a_forget_with_no_answer_marks_what_oso_added_to_go_later() {
        let directory = private("forget");
        let private = PrivateDir::open(&directory).unwrap();
        let mut passes = Passes::default();
        let mut forget =
            |endpoint| super::forget(&directory, endpoint, 0, &mut passes, Duration::ZERO);
        assert_eq!(forget(None), ALL);
        assert!(write_list(&private, &one()));
        assert_eq!(forget(Some(&nobody(&directory))), Taken::Left(1));
        let left = read_list(&directory, 100).unwrap();
        assert_eq!(left.entries[0].state, State::Forgetting);
        clean(&directory);
    }

    #[test]
    fn a_forget_another_instance_holds_up_is_made_by_the_next_pass() {
        let directory = private("held");
        let (private, held_lock, _) = held(&directory, 100).unwrap();
        assert!(write_list(&private, &one()));
        let nobody = nobody(&directory);
        let mut gone = Passes::default();
        let taken = forget(&directory, Some(&nobody), 0, &mut gone, Duration::ZERO);
        assert_eq!(taken, Taken::Unknown);
        // That instance quits: the pass is another's.
        let mut passes = Passes::default();
        // The other instance wrote the record again meanwhile.
        std::fs::write(directory.join(record::FILE), "{}").unwrap();
        std::fs::write(directory.join(record::LOG), "").unwrap();
        drop(held_lock);
        promote(&directory, &nobody, 0, &mut passes, &Shared::default(), 0);
        let left = read_list(&directory, 100).unwrap();
        assert_eq!(left.entries[0].state, State::Forgetting);
        assert!(!directory.join(record::FILE).exists());
        assert!(!directory.join(record::LOG).exists());
        assert!(!directory.join(list::DUE).exists());
        clean(&directory);
    }

    #[test]
    fn a_list_that_is_not_whole_is_left_alone_also_by_a_forget() {
        let directory = private("broken");
        std::fs::write(directory.join(list::FILE), "{").unwrap();
        assert!(read_list(&directory, 0).is_err());
        assert!(held(&directory, 0).is_none());
        let mut passes = Passes::default();
        let nobody = nobody(&directory);
        promote(&directory, &nobody, 0, &mut passes, &Shared::default(), 0);
        let taken = forget(&directory, None, 0, &mut passes, Duration::ZERO);
        assert_eq!(taken, Taken::Unknown);
        assert_eq!(
            std::fs::read_to_string(directory.join(list::FILE)).unwrap(),
            "{"
        );
        clean(&directory);
    }

    #[test]
    fn a_lock_that_is_not_a_file_is_refused() {
        let directory = private("lock-link");
        std::os::unix::fs::symlink("/dev/null", directory.join(list::LOCK)).unwrap();
        assert!(held(&directory, 0).is_none());
        clean(&directory);
    }
}
